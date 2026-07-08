//! hikari (光) — the tree-sitter backend.
//!
//! ONE generic [`TreeSitterHighlighter`] serving every tree-sitter grammar,
//! implementing [`hikari_core::Highlighter`] directly (tree-sitter carries its
//! own tree, so it does NOT go through `LanguageLexer`/`LineDriven`). It wraps
//! escriba-ts's shipping `GrammarRegistry` + `highlight()` — the fleet's
//! verified tree-sitter host is CONSUMED, not forked — and lowers escriba-ts's
//! `Semantic` result to hikari's [`HlClass`] through the coverage-by-
//! construction [`SpanSink`], so a gappy tree-sitter result becomes a
//! coverage-complete hikari partition (gaps auto-fill `Plain`).
//!
//! Fallible at construct time ([`TreeSitterHost::builtin`] → `Result`),
//! infallible at highlight time (a parse failure yields all-`Plain`, never a
//! panic — preserving hikari's panic-free contract).

#![forbid(unsafe_code)]

use std::sync::Arc;

use escriba_ts::{GrammarRegistry, Semantic, TsError};
use hikari_core::{
    HighlightSpan, Highlighter, HlClass, Language, LanguagePlugin, Selector, SpanSink,
};
use hikari_token::Semantic as HikariSemantic;

/// Map escriba-ts's `Semantic` (its own 16-variant copy) to the fleet
/// [`hikari_token::Semantic`] — a 1:1 name match — then into [`HlClass`] via
/// hikari-token's total conversion. When escriba-ts re-exports hikari-token's
/// `Semantic` (Phase 3), this collapses to `s.into()`.
fn semantic_to_hikari(s: Semantic) -> HikariSemantic {
    match s {
        Semantic::Keyword => HikariSemantic::Keyword,
        Semantic::Symbol => HikariSemantic::Symbol,
        Semantic::KeywordArg => HikariSemantic::KeywordArg,
        Semantic::String => HikariSemantic::String,
        Semantic::Number => HikariSemantic::Number,
        Semantic::Literal => HikariSemantic::Literal,
        Semantic::Comment => HikariSemantic::Comment,
        Semantic::Accent => HikariSemantic::Accent,
        Semantic::Muted => HikariSemantic::Muted,
        Semantic::Error => HikariSemantic::Error,
        Semantic::Warning => HikariSemantic::Warning,
        Semantic::Info => HikariSemantic::Info,
        Semantic::Hint => HikariSemantic::Hint,
        Semantic::Added => HikariSemantic::Added,
        Semantic::Removed => HikariSemantic::Removed,
        Semantic::Unchanged => HikariSemantic::Unchanged,
    }
}

/// The tree-sitter host — builds escriba-ts's built-in [`GrammarRegistry`]
/// once and hands out per-language [`TreeSitterHighlighter`]s + registers a
/// [`LanguagePlugin`] per grammar into a hikari `Ecosystem`.
#[derive(Clone)]
pub struct TreeSitterHost {
    registry: Arc<GrammarRegistry>,
}

impl TreeSitterHost {
    /// Build the built-in grammar registry (Rust today, more as escriba-ts
    /// adds them).
    ///
    /// # Errors
    /// Returns [`TsError`] if escriba-ts fails to construct a grammar.
    pub fn builtin() -> Result<Self, TsError> {
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
            // interned static language name — the registry's grammar names are
            // a fixed set, so leaking them once is bounded + gives the
            // 'static Language newtype hikari expects.
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

/// The generic tree-sitter [`Highlighter`]. Whole-document highlight via
/// escriba-ts's `highlight()`; the result (gappy `Semantic` spans) is funneled
/// through [`SpanSink`] so the output is a coverage-complete hikari partition.
pub struct TreeSitterHighlighter {
    registry: Arc<GrammarRegistry>,
    grammar: &'static str,
}

impl Highlighter for TreeSitterHighlighter {
    fn highlight(&self, text: &str) -> Vec<HighlightSpan> {
        let len = u32::try_from(text.len()).unwrap_or(u32::MAX);
        let mut sink = SpanSink::for_document(len);
        if let Some(grammar) = self.registry.get(self.grammar)
            && let Ok(spans) = escriba_ts::highlight(text, grammar, &self.registry)
        {
            for s in spans {
                let class: HlClass = semantic_to_hikari(s.semantic).into();
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
    use super::TreeSitterHost;
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
        // at least one non-Plain class was produced (real highlighting).
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
}
