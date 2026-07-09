//! hikari (光) — the tree-sitter backend.
//!
//! The fleet's tree-sitter host, owned here (not borrowed from an application
//! crate). It bundles the tree-sitter C runtime + grammars and exposes:
//!
//!   * [`GrammarRegistry`] / [`Grammar`] — language-name → grammar + highlight
//!     config, shipped with tree-sitter-rust (more grammars land here).
//!   * [`BufferParser`] — a per-buffer parser keeping a `tree_sitter::Tree`,
//!     with a full [`reparse`](BufferParser::reparse) and an incremental
//!     [`reparse_edit`](BufferParser::reparse_edit) (`Tree::edit` + subtree
//!     reuse via the typed byte-based [`TsEdit`]).
//!   * [`highlight`] — whole-document highlight → gappy [`Semantic`] spans.
//!   * [`TreeSitterHost`] / [`TreeSitterHighlighter`] — the hikari-facing
//!     wrapper: ONE generic highlighter for every grammar, implementing
//!     [`hikari_core::Highlighter`] directly (tree-sitter carries its own tree,
//!     so it does NOT go through `LanguageLexer`/`LineDriven`), lowering the
//!     `Semantic` result to hikari's [`HlClass`] through the coverage-by-
//!     construction [`SpanSink`] (gaps auto-fill `Plain`).
//!
//! Fallible at construct time ([`TreeSitterHost::builtin`] → `Result`),
//! infallible at highlight time (a parse failure yields all-`Plain`, never a
//! panic — preserving hikari's panic-free contract).
//!
//! `Semantic` is re-exported from `hikari-token` (the deduped fleet vocabulary);
//! escriba-ts re-exports THIS crate's host in turn, so the tree-sitter host
//! lives in exactly one place.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::Arc;

use hikari_core::{
    HighlightSpan as HlSpan, Highlighter, HlClass, Language, LanguagePlugin, Selector, SpanSink,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tree_sitter::{
    InputEdit, Language as TsLanguage, Parser, Point, Tree,
};
use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter as TsHighlighter};

// The fleet semantic highlight vocabulary — owned by hikari-token, re-exported
// so escriba-ts (which re-exports this crate) and every other consumer name one
// `Semantic`. Carries the total `From<Semantic> for HlClass`.
pub use hikari_token::Semantic;

// ───────────────────────────── errors ───────────────────────────

#[derive(Debug, Error)]
pub enum TsError {
    #[error("grammar not registered: {0}")]
    Unknown(String),
    #[error("tree-sitter: {0}")]
    Ts(String),
}

pub type Result<T> = std::result::Result<T, TsError>;

// ─────────────────────────── grammars ───────────────────────────

/// A registered grammar — name, language, highlight config, claimed extensions.
pub struct Grammar {
    pub name: String,
    pub language: TsLanguage,
    pub config: HighlightConfiguration,
    /// File extensions (no dot) this grammar claims. Mutable at runtime so a
    /// `defmode :extensions (…)` declaration can broaden the mapping without
    /// recompilation.
    pub extensions: Vec<String>,
}

/// Registry — language-name → [`Grammar`].
pub struct GrammarRegistry {
    grammars: HashMap<String, Grammar>,
    /// The highlight-name namespace — indices into this vector are what
    /// `HighlightEvent::HighlightStart(…)` returns.
    pub highlight_names: Vec<&'static str>,
}

impl GrammarRegistry {
    /// Build the built-in registry — the go-wide grammar set. Adding a grammar
    /// is one [`register`](Self::register) line + one `language_matrix` row.
    ///
    /// # Errors
    /// Returns [`TsError::Ts`] if a grammar's highlight query fails to compile.
    pub fn builtin() -> Result<Self> {
        let highlight_names = canonical_highlight_names();
        let mut reg = Self {
            grammars: HashMap::new(),
            highlight_names,
        };
        reg.register(
            "rust",
            &tree_sitter_rust::language(),
            tree_sitter_rust::HIGHLIGHTS_QUERY,
            tree_sitter_rust::INJECTIONS_QUERY,
            &["rs"],
        )?;
        reg.register(
            "python",
            &tree_sitter_python::language(),
            tree_sitter_python::HIGHLIGHTS_QUERY,
            "",
            &["py", "pyi"],
        )?;
        reg.register(
            "json",
            &tree_sitter_json::language(),
            tree_sitter_json::HIGHLIGHTS_QUERY,
            "",
            &["json"],
        )?;
        reg.register(
            "bash",
            &tree_sitter_bash::language(),
            tree_sitter_bash::HIGHLIGHT_QUERY,
            "",
            &["sh", "bash", "zsh"],
        )?;
        Ok(reg)
    }

    /// Register one grammar: compile its highlight config against the canonical
    /// name space and insert it under `name` claiming `extensions`. The one
    /// repeated shape, factored out so adding a grammar is a single call.
    ///
    /// # Errors
    /// Returns [`TsError::Ts`] if the highlight query fails to compile.
    fn register(
        &mut self,
        name: &str,
        language: &TsLanguage,
        highlights: &str,
        injections: &str,
        extensions: &[&str],
    ) -> Result<()> {
        let mut cfg =
            HighlightConfiguration::new(language.clone(), name, highlights, injections, "")
                .map_err(|e| TsError::Ts(format!("{name}: {e}")))?;
        cfg.configure(&self.highlight_names);
        self.grammars.insert(
            name.to_string(),
            Grammar {
                name: name.to_string(),
                language: language.clone(),
                config: cfg,
                extensions: extensions.iter().map(|s| (*s).to_string()).collect(),
            },
        );
        Ok(())
    }

    #[must_use]
    pub fn get(&self, language: &str) -> Option<&Grammar> {
        self.grammars.get(language)
    }

    /// Look up a language by file extension (e.g. `"rs"` → `"rust"`).
    #[must_use]
    pub fn from_extension(&self, ext: &str) -> Option<&Grammar> {
        self.grammars
            .values()
            .find(|g| g.extensions.iter().any(|e| e == ext))
    }

    /// Broaden a grammar's extension list. Returns `true` iff the grammar was
    /// registered; `false` means the caller referenced an unknown language.
    pub fn add_extension(&mut self, language: &str, ext: impl Into<String>) -> bool {
        if let Some(g) = self.grammars.get_mut(language) {
            let ext = ext.into();
            if !g.extensions.iter().any(|e| *e == ext) {
                g.extensions.push(ext);
            }
            true
        } else {
            false
        }
    }

    /// Iterate every registered language name.
    pub fn languages(&self) -> impl Iterator<Item = &str> {
        self.grammars.keys().map(String::as_str)
    }
}

// ────────────────────────── per-buffer parse ────────────────────

/// Per-buffer parser + last-parsed tree.
pub struct BufferParser {
    language: String,
    parser: Parser,
    tree: Option<Tree>,
}

impl BufferParser {
    /// A parser for `language` (must be registered).
    ///
    /// # Errors
    /// Returns [`TsError`] if the language is unknown or the parser rejects it.
    pub fn new(language: &str, registry: &GrammarRegistry) -> Result<Self> {
        let grammar = registry
            .get(language)
            .ok_or_else(|| TsError::Unknown(language.to_string()))?;
        let mut parser = Parser::new();
        parser
            .set_language(&grammar.language)
            .map_err(|e| TsError::Ts(e.to_string()))?;
        Ok(Self {
            language: language.to_string(),
            parser,
            tree: None,
        })
    }

    #[must_use]
    pub fn language(&self) -> &str {
        &self.language
    }

    /// Re-parse `src` from scratch. Passes `None` as the old tree on purpose:
    /// tree-sitter's incremental path requires the old tree to have been
    /// `Tree::edit`-ed to reflect exactly what changed. Handing `parse()` an
    /// *un-edited* old tree against changed source violates that contract and
    /// can yield an incorrect tree — so the correct answer for an unknown delta
    /// is a full parse. Callers that know the edit use
    /// [`reparse_edit`](Self::reparse_edit).
    ///
    /// # Errors
    /// Infallible today (tree-sitter returns `None` on failure, stored as-is);
    /// the `Result` reserves fallibility for future timeout/cancel support.
    pub fn reparse(&mut self, src: &str) -> Result<()> {
        self.tree = self.parser.parse(src, None);
        Ok(())
    }

    /// Incrementally re-parse after splicing `[start_byte, old_end_byte)` of
    /// `old_src` to produce `new_src`. Edits the retained tree by the splice
    /// ([`TsEdit`]) so tree-sitter reuses every unchanged subtree and reparses
    /// only the affected span — `O(edit)`, not `O(document)`. With no prior tree
    /// it falls back to a full parse. The result is identical to a full parse of
    /// `new_src` (the differential-equivalence invariant, tested).
    ///
    /// # Errors
    /// Same as [`reparse`](Self::reparse).
    pub fn reparse_edit(
        &mut self,
        old_src: &str,
        new_src: &str,
        start_byte: usize,
        old_end_byte: usize,
    ) -> Result<()> {
        if self.tree.is_some() {
            let edit = TsEdit::from_splice(old_src, new_src, start_byte, old_end_byte);
            if let Some(tree) = self.tree.as_mut() {
                tree.edit(&edit.to_input_edit());
            }
            self.tree = self.parser.parse(new_src, self.tree.as_ref());
        } else {
            self.tree = self.parser.parse(new_src, None);
        }
        Ok(())
    }

    #[must_use]
    pub fn tree(&self) -> Option<&Tree> {
        self.tree.as_ref()
    }
}

/// A typed, byte-based description of one contiguous splice, for incremental
/// tree-sitter reparse. tree-sitter's native unit is the byte offset + a
/// `(row, byte-column)` point, so this converts from a plain source splice —
/// no tree-sitter type crosses the caller boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TsEdit {
    pub start_byte: usize,
    pub old_end_byte: usize,
    pub new_end_byte: usize,
    /// `(row, byte-column)` of the splice start (identical in old + new).
    pub start_point: (usize, usize),
    pub old_end_point: (usize, usize),
    pub new_end_point: (usize, usize),
}

impl TsEdit {
    /// Compute the splice turning `old` into `new` by replacing
    /// `old[start_byte..old_end_byte]`. The unchanged suffix has the same length
    /// in `new`, so `new_end_byte = new.len() - (old.len() - old_end_byte)`.
    #[must_use]
    pub fn from_splice(old: &str, new: &str, start_byte: usize, old_end_byte: usize) -> Self {
        let new_end_byte = new.len() - (old.len() - old_end_byte);
        Self {
            start_byte,
            old_end_byte,
            new_end_byte,
            start_point: byte_to_point(old, start_byte),
            old_end_point: byte_to_point(old, old_end_byte),
            new_end_point: byte_to_point(new, new_end_byte),
        }
    }

    fn to_input_edit(self) -> InputEdit {
        let pt = |(row, column): (usize, usize)| Point { row, column };
        InputEdit {
            start_byte: self.start_byte,
            old_end_byte: self.old_end_byte,
            new_end_byte: self.new_end_byte,
            start_position: pt(self.start_point),
            old_end_position: pt(self.old_end_point),
            new_end_position: pt(self.new_end_point),
        }
    }
}

/// `(row, byte-column)` of `byte` within `text`. tree-sitter point columns are
/// byte offsets within the line, not char offsets.
#[must_use]
fn byte_to_point(text: &str, byte: usize) -> (usize, usize) {
    let byte = byte.min(text.len());
    let prefix = &text[..byte];
    let row = prefix.bytes().filter(|&b| b == b'\n').count();
    let col = prefix.len() - prefix.rfind('\n').map_or(0, |i| i + 1);
    (row, col)
}

// ─────────────────────────── highlight ──────────────────────────

/// A colored text span — byte range + canonical [`Semantic`] bucket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HighlightSpan {
    pub start: usize,
    pub end: usize,
    pub semantic: Semantic,
}

/// Compute highlight spans over `src` using `grammar`.
///
/// # Errors
/// Returns [`TsError::Ts`] if tree-sitter's highlighter errors.
pub fn highlight(
    src: &str,
    grammar: &Grammar,
    registry: &GrammarRegistry,
) -> Result<Vec<HighlightSpan>> {
    let mut highlighter = TsHighlighter::new();
    let events = highlighter
        .highlight(&grammar.config, src.as_bytes(), None, |_| None)
        .map_err(|e| TsError::Ts(e.to_string()))?;

    let mut stack: Vec<usize> = Vec::new();
    let mut spans: Vec<HighlightSpan> = Vec::new();
    let mut run_start: Option<(usize, usize)> = None;

    for ev in events {
        let ev = ev.map_err(|e| TsError::Ts(e.to_string()))?;
        match ev {
            HighlightEvent::HighlightStart(h) => stack.push(h.0),
            HighlightEvent::HighlightEnd => {
                stack.pop();
                run_start = None;
            }
            HighlightEvent::Source { start, end } => {
                if let Some(&top) = stack.last() {
                    let sem = highlight_index_to_semantic(top, &registry.highlight_names);
                    match run_start {
                        Some((rs, _)) if rs == start => {}
                        _ => {
                            spans.push(HighlightSpan {
                                start,
                                end,
                                semantic: sem,
                            });
                            run_start = Some((start, end));
                        }
                    }
                }
            }
        }
    }

    Ok(spans)
}

/// The canonical highlight-name namespace every grammar is configured against.
/// Indices into this vector map to [`Semantic`] buckets.
fn canonical_highlight_names() -> Vec<&'static str> {
    vec![
        "keyword",
        "function",
        "function.call",
        "function.method",
        "type",
        "type.builtin",
        "constant",
        "constant.builtin",
        "string",
        "string.special",
        "number",
        "boolean",
        "comment",
        "operator",
        "punctuation",
        "punctuation.bracket",
        "punctuation.delimiter",
        "variable",
        "variable.parameter",
        "variable.builtin",
        "attribute",
        "label",
        "tag",
    ]
}

fn highlight_index_to_semantic(index: usize, names: &[&'static str]) -> Semantic {
    let name = names.get(index).copied().unwrap_or("");
    match name {
        n if n.starts_with("keyword") => Semantic::Keyword,
        n if n.starts_with("function") => Semantic::Symbol,
        n if n.starts_with("type") => Semantic::Accent,
        n if n.starts_with("constant.builtin") || n == "boolean" => Semantic::Literal,
        n if n.starts_with("constant") => Semantic::Literal,
        n if n.starts_with("string") => Semantic::String,
        n if n == "number" => Semantic::Number,
        n if n.starts_with("comment") => Semantic::Comment,
        n if n.starts_with("operator") => Semantic::Accent,
        n if n.starts_with("punctuation") => Semantic::Muted,
        n if n.starts_with("variable") => Semantic::Symbol,
        n if n == "attribute" => Semantic::Hint,
        n if n == "label" => Semantic::Hint,
        n if n == "tag" => Semantic::Keyword,
        _ => Semantic::Symbol,
    }
}

// ─────────────────────── hikari Highlighter face ────────────────

/// The tree-sitter host — builds the built-in [`GrammarRegistry`] once and hands
/// out per-language [`TreeSitterHighlighter`]s + a [`LanguagePlugin`] per grammar
/// for registration into a hikari `Ecosystem`.
#[derive(Clone)]
pub struct TreeSitterHost {
    registry: Arc<GrammarRegistry>,
}

impl TreeSitterHost {
    /// Build the built-in grammar registry (Rust today, more as grammars land).
    ///
    /// # Errors
    /// Returns [`TsError`] if a grammar fails to construct.
    pub fn builtin() -> Result<Self> {
        Ok(Self {
            registry: Arc::new(GrammarRegistry::builtin()?),
        })
    }

    /// The languages this host can highlight.
    pub fn languages(&self) -> impl Iterator<Item = &str> {
        self.registry.languages()
    }

    /// A [`LanguagePlugin`] for each built-in grammar, ready to register into a
    /// hikari `Ecosystem` (each claims its grammar's extensions).
    #[must_use]
    pub fn plugins(&self) -> Vec<Box<dyn LanguagePlugin>> {
        let mut out: Vec<Box<dyn LanguagePlugin>> = Vec::new();
        for name in self.registry.languages() {
            // interned static name — the grammar set is fixed, so leaking once
            // is bounded + gives the 'static `Language` newtype hikari expects.
            let lang: &'static str = Box::leak(name.to_string().into_boxed_str());
            let selectors: Vec<Selector> = self
                .registry
                .get(name)
                .map(|g| {
                    g.extensions
                        .iter()
                        .map(|e| Selector::Extension(Box::leak(e.clone().into_boxed_str())))
                        .collect()
                })
                .unwrap_or_default();
            out.push(Box::new(TreeSitterPlugin {
                language: Language(lang),
                selectors: selectors.leak(),
                registry: self.registry.clone(),
                grammar: lang,
            }));
        }
        out
    }

    /// A highlighter for one grammar by name.
    #[must_use]
    pub fn highlighter(&self, grammar: &'static str) -> TreeSitterHighlighter {
        TreeSitterHighlighter {
            registry: self.registry.clone(),
            grammar,
        }
    }
}

/// A hikari [`LanguagePlugin`] backed by a tree-sitter grammar.
pub struct TreeSitterPlugin {
    language: Language,
    selectors: &'static [Selector],
    registry: Arc<GrammarRegistry>,
    grammar: &'static str,
}

impl LanguagePlugin for TreeSitterPlugin {
    fn language(&self) -> Language {
        self.language
    }
    fn selectors(&self) -> &'static [Selector] {
        self.selectors
    }
    fn make_highlighter(&self) -> Box<dyn Highlighter> {
        Box::new(TreeSitterHighlighter {
            registry: self.registry.clone(),
            grammar: self.grammar,
        })
    }
}

/// The generic tree-sitter [`Highlighter`]. Whole-document highlight; the result
/// (gappy `Semantic` spans) is funneled through [`SpanSink`] so the output is a
/// coverage-complete hikari partition (gaps become `Plain`).
pub struct TreeSitterHighlighter {
    registry: Arc<GrammarRegistry>,
    grammar: &'static str,
}

impl Highlighter for TreeSitterHighlighter {
    fn highlight(&self, text: &str) -> Vec<HlSpan> {
        let len = u32::try_from(text.len()).unwrap_or(u32::MAX);
        let mut sink = SpanSink::for_document(len);
        if let Some(grammar) = self.registry.get(self.grammar)
            && let Ok(spans) = highlight(text, grammar, &self.registry)
        {
            for s in spans {
                // hikari_token::Semantic → HlClass via the total fleet conversion.
                let class: HlClass = s.semantic.into();
                sink.push(
                    u32::try_from(s.start).unwrap_or(u32::MAX),
                    u32::try_from(s.end).unwrap_or(u32::MAX),
                    class,
                );
            }
        }
        sink.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::{BufferParser, GrammarRegistry, TreeSitterHost, TsEdit, byte_to_point};
    use hikari_core::{Highlighter, HlClass};

    #[test]
    fn builtin_host_highlights_rust_with_coverage() {
        let host = TreeSitterHost::builtin().expect("builtin grammars");
        let hl = host.highlighter("rust");
        let src = "fn main() {\n    let x = 42;\n}\n";
        let spans = hl.highlight(src);
        // coverage-complete + forward-only (SpanSink guarantees).
        let mut cursor = 0u32;
        for s in &spans {
            assert_eq!(s.span.start, cursor, "gap/overlap at {cursor}");
            cursor = s.span.end;
        }
        assert_eq!(cursor as usize, src.len(), "partition must cover the text");
        assert!(
            spans.iter().any(|s| s.class != HlClass::Plain),
            "tree-sitter should classify something in real Rust",
        );
    }

    #[test]
    fn rust_grammar_is_registered() {
        let host = TreeSitterHost::builtin().expect("builtin grammars");
        assert!(host.languages().any(|l| l == "rust"));
        assert!(!host.plugins().is_empty());
    }

    #[test]
    fn byte_to_point_counts_rows_and_byte_columns() {
        assert_eq!(byte_to_point("abc", 2), (0, 2));
        assert_eq!(byte_to_point("ab\ncd", 3), (1, 0));
        assert_eq!(byte_to_point("x\ny\nz", 4), (2, 0));
    }

    /// M5 seal: an incremental `Tree::edit` reparse equals a full parse.
    #[test]
    fn incremental_reparse_equals_full_parse() {
        let r = GrammarRegistry::builtin().unwrap();
        let old = "fn main() { let x = 1; }";
        let new = "fn main() { let x = 42; }";
        let start = old.find('1').unwrap();

        let mut inc = BufferParser::new("rust", &r).unwrap();
        inc.reparse(old).unwrap();
        inc.reparse_edit(old, new, start, start + 1).unwrap();

        let mut full = BufferParser::new("rust", &r).unwrap();
        full.reparse(new).unwrap();

        assert_eq!(
            inc.tree().unwrap().root_node().to_sexp(),
            full.tree().unwrap().root_node().to_sexp(),
        );
    }

    #[test]
    fn ts_edit_new_end_byte_accounts_for_length_delta() {
        let old = "x = 1;";
        let new = "x = 42;";
        let start = old.find('1').unwrap();
        let e = TsEdit::from_splice(old, new, start, start + 1);
        assert_eq!(e.new_end_byte, start + 2);
    }
}
