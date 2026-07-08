//! hikari (光) — the pluggable syntax-highlighting spine.
//!
//! The fleet-shared foundation for syntax highlighting across every pleme-io
//! editor (AsterIDE · escriba) and tool. It owns the narrow interface every
//! language backend lowers to; heavy backends (tree-sitter grammars, the
//! tatara-lisp macro-generated `(deflexer …)` output) ship as separate
//! `hikari-*` crates in this workspace so `hikari-core` stays zero-dependency
//! and any consumer vendors it with no transitive weight.
//!
//! The design (a stable trait spine so backends are additive, never rewrites):
//!
//!   * ONE narrow-waist output — a coverage-complete, non-overlapping,
//!     forward-only `Vec<HighlightSpan>` partition produced ONLY through
//!     [`SpanSink`], which makes gaps / overlaps / reversals structurally
//!     unrepresentable (the caller cannot fabricate the `Vec`).
//!   * ONE authored backend trait [`LanguageLexer`] whose associated
//!     `LineState: Copy + Eq` makes incremental line-restart re-lex a *type
//!     property* (a consumer may cache per-line and re-lex only until the
//!     entry state stops changing).
//!   * ONE object-safe [`Highlighter`] the render layer holds as `Box<dyn>`,
//!     bridged from any `LanguageLexer` by the blanket [`LineDriven`] adapter.
//!   * ONE [`Ecosystem`] registry with total, panic-free resolution — an
//!     unknown extension resolves to [`PLAIN_TEXT`], never a panic, never
//!     "everything is one language".
//!   * palette-independent [`HlClass`] + a [`Theme`] mapping class → [`Rgb`]
//!     so classification and color never entangle (Nord default).

#![forbid(unsafe_code)]
// The workspace sets `clippy::pedantic = warn`. These arms are intentional in
// a hand-rolled byte scanner + a fixed palette, so they are allowed with a
// reason rather than contorted: single-char cursor vars (`i`/`c`/`n`/`s`/`e`)
// are the idiom for a lexer; byte offsets provably fit `u32` (documents cap at
// 4 GiB); proper nouns (AsterIDE, pleme-io) aren't code; the resume-match and
// same-color palette arms read clearer as written.
#![allow(
    clippy::many_single_char_names,
    clippy::cast_possible_truncation,
    clippy::doc_markdown,
    clippy::single_match_else,
    clippy::collapsible_if,
    clippy::match_same_arms
)]

// ───────────────────────────── span ─────────────────────────────

/// A byte-offset span into the document. `start <= end` by construction, and
/// (when produced through [`SpanSink`]) both ends land on UTF-8 char
/// boundaries.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ByteSpan {
    pub start: u32,
    pub end: u32,
}

impl ByteSpan {
    /// The sole constructor.
    ///
    /// # Panics
    /// Panics only on a caller bug (`start > end`). The lexer driver never
    /// constructs a reversed span, so this is unreachable in normal use.
    #[must_use]
    pub fn new(start: u32, end: u32) -> Self {
        assert!(start <= end, "ByteSpan::new: start {start} > end {end}");
        Self { start, end }
    }

    #[must_use]
    pub fn len(&self) -> u32 {
        self.end - self.start
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }

    /// The span as a `usize` range, for slicing the source text.
    #[must_use]
    pub fn range(&self) -> std::ops::Range<usize> {
        self.start as usize..self.end as usize
    }
}

// ───────────────────────────── class ────────────────────────────

/// A palette-independent semantic highlight class. A superset of the classes
/// egui / tree-sitter / the fleet's `Semantic` enums produce, so every backend
/// lowers to this one waist and the theme layer maps it to color exactly once.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum HlClass {
    Comment {
        multiline: bool,
    },
    Keyword,
    /// A `:keyword`-style argument / symbol (lisp `:kw`, etc.).
    KeywordArg,
    Type,
    Function,
    Namespace,
    Variable,
    Constant,
    Str,
    Escape,
    Numeric {
        float: bool,
    },
    Boolean,
    Punctuation,
    Operator,
    Attribute,
    Special,
    Hyperlink,
    Whitespace,
    Error,
    /// A diagnostic/severity class (LSP + the fleet `Semantic` collapse).
    Warning,
    Info,
    Hint,
    /// A diff class (git signs, review UIs).
    Added,
    Removed,
    /// Diff context / normal-fg text in a diff view.
    Unchanged,
    /// The default / unclassified class. Coverage gaps are filled with this.
    Plain,
}

/// One classified region of the document.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct HighlightSpan {
    pub span: ByteSpan,
    pub class: HlClass,
}

// ───────────────────────────── sink ─────────────────────────────

/// The ONLY way to produce highlight spans. A forward-only cursor over a
/// single line: [`push`](SpanSink::push) fills any gap before the pushed
/// region with [`HlClass::Plain`] and clamps backwards writes, so the emitted
/// sequence is a coverage-complete, non-overlapping, monotonically-increasing
/// partition **by construction** — a lexer cannot emit a gap, an overlap, or a
/// reversal. The backing `Vec` is private; only the driver calls
/// [`finish`](SpanSink::finish).
pub struct SpanSink {
    cursor: u32,
    line_end: u32,
    out: Vec<HighlightSpan>,
}

impl SpanSink {
    /// A sink covering `[line_start, line_start + line_len)`. Public so a
    /// direct [`Highlighter`] impl (e.g. a tree-sitter backend) can construct
    /// one and keep coverage-by-construction, not only the [`LineDriven`]
    /// bridge.
    #[must_use]
    pub fn new(line_start: u32, line_len: u32) -> Self {
        Self {
            cursor: line_start,
            line_end: line_start + line_len,
            out: Vec::new(),
        }
    }

    /// A sink covering a whole document `[0, len)` — for a non-line backend
    /// that pushes absolute offsets and wants gap-fill for free.
    #[must_use]
    pub fn for_document(len: u32) -> Self {
        Self::new(0, len)
    }

    /// Classify `[start, end)` (absolute byte offsets) as `class`. A gap
    /// `[cursor, start)` is filled with [`HlClass::Plain`]; a backwards
    /// `start` is clamped to the cursor; an empty region is dropped.
    pub fn push(&mut self, start: u32, end: u32, class: HlClass) {
        let start = start.max(self.cursor);
        let end = end.min(self.line_end);
        if end <= start {
            return;
        }
        if start > self.cursor {
            self.out.push(HighlightSpan {
                span: ByteSpan::new(self.cursor, start),
                class: HlClass::Plain,
            });
        }
        self.out.push(HighlightSpan {
            span: ByteSpan::new(start, end),
            class,
        });
        self.cursor = end;
    }

    /// Finish: fill any trailing gap with [`HlClass::Plain`] and return the
    /// coverage-complete, non-overlapping, forward-only partition.
    #[must_use]
    pub fn finish(mut self) -> Vec<HighlightSpan> {
        if self.cursor < self.line_end {
            self.out.push(HighlightSpan {
                span: ByteSpan::new(self.cursor, self.line_end),
                class: HlClass::Plain,
            });
        }
        self.out
    }
}

// ─────────────────────── backend + render traits ────────────────

/// The AUTHORED backend trait — implement this to add a language. One total
/// method lexes a single line given the cross-line state carried out of the
/// previous line; returning the state at the line's end. `LineState: Eq` is
/// what makes incremental re-lex a type property (re-lex stops at the first
/// line whose entry state is unchanged).
pub trait LanguageLexer: Send + Sync {
    type LineState: Copy + Eq + Default + Send + Sync;

    fn lex_line(
        &self,
        line: &str,
        line_start: u32,
        entry: Self::LineState,
        sink: &mut SpanSink,
    ) -> Self::LineState;
}

/// The object-safe, render-facing trait the editor holds as `Box<dyn>`. The
/// default [`LineDriven`] bridge re-lexes the whole document; a consumer may
/// implement `Highlighter` directly for an incremental line cache.
pub trait Highlighter: Send + Sync {
    fn highlight(&self, text: &str) -> Vec<HighlightSpan>;
}

/// The blanket bridge: any [`LanguageLexer`] is a [`Highlighter`]. No backend
/// author ever hand-writes `Highlighter`.
pub struct LineDriven<L: LanguageLexer> {
    pub lexer: L,
}

impl<L: LanguageLexer> LineDriven<L> {
    #[must_use]
    pub fn new(lexer: L) -> Self {
        Self { lexer }
    }
}

impl<L: LanguageLexer> Highlighter for LineDriven<L> {
    fn highlight(&self, text: &str) -> Vec<HighlightSpan> {
        let mut out = Vec::new();
        let mut state = L::LineState::default();
        let mut offset: u32 = 0;
        // split_inclusive keeps the trailing '\n' on each line, so offsets are
        // contiguous and the partition covers every byte of `text`.
        for line in text.split_inclusive('\n') {
            let line_len = u32::try_from(line.len()).unwrap_or(u32::MAX);
            let mut sink = SpanSink::new(offset, line_len);
            state = self.lexer.lex_line(line, offset, state, &mut sink);
            out.extend(sink.finish());
            offset = offset.saturating_add(line_len);
        }
        out
    }
}

// ───────────────────── incremental line cache ───────────────────

/// The object-safe incremental face the render layer holds as `Box<dyn>`. A
/// stateful highlighter that reuses prior work: on each call it re-lexes only
/// from the first content-changed line until the carried entry state
/// re-converges (the `LineState` fixpoint), reusing every cached line before
/// and after. Its output is **byte-identical** to the equivalent one-shot
/// [`Highlighter::highlight`] — incrementality is an optimization, never a
/// semantic change (the differential-equivalence invariant, fuzz-tested).
pub trait IncrementalHighlighter: Send + Sync {
    /// Re-highlight `text`, reusing cached per-line spans where the document
    /// is unchanged. `&mut self` because the cache advances.
    fn highlight(&mut self, text: &str) -> Vec<HighlightSpan>;

    /// How many lines the most recent [`highlight`](Self::highlight) call
    /// actually re-lexed — `0` on a fully-cached (idle re-render) call. The
    /// seal's idle-work witness: an unchanged document re-lexes nothing.
    fn last_relexed(&self) -> usize;
}

/// One cached line: the `LineState` carried *into* it, the state carried *out*
/// of it, the exact line bytes (incl. trailing `\n`) for the content compare,
/// and the line's spans stored **line-relative** (0-based) so a length change
/// above never invalidates them — they are re-based to absolute on emit.
struct CachedLine<S> {
    entry: S,
    exit: S,
    text: Box<str>,
    rel_spans: Vec<HighlightSpan>,
}

/// The incremental re-lex cache over any [`LanguageLexer`]. The `LineState`
/// fixpoint made operational: re-lexing halts at the first line whose entry
/// state *and* bytes both match the cache, so an edit costs
/// `O(changed lines + lines until the carried state re-converges)`, not
/// `O(document)`.
///
/// Zero-dependency by design (hikari-core's invariant): the content compare is
/// std `str` equality and the cache stores the line bytes. A `hikari-*` sibling
/// may swap the `Box<str>` for a BLAKE3 row hash to drop the O(document) memory
/// — that is a strictly-internal change behind this same object-safe trait.
pub struct LineCache<L: LanguageLexer> {
    lexer: L,
    lines: Vec<CachedLine<L::LineState>>,
    last_relexed: usize,
}

impl<L: LanguageLexer> LineCache<L> {
    #[must_use]
    pub fn new(lexer: L) -> Self {
        Self {
            lexer,
            lines: Vec::new(),
            last_relexed: 0,
        }
    }

    /// Lex one line at base offset 0 (line-relative spans) carrying `entry`.
    fn lex_relative(&self, line: &str, entry: L::LineState) -> (L::LineState, Vec<HighlightSpan>) {
        let line_len = u32::try_from(line.len()).unwrap_or(u32::MAX);
        let mut sink = SpanSink::new(0, line_len);
        let exit = self.lexer.lex_line(line, 0, entry, &mut sink);
        (exit, sink.finish())
    }
}

impl<L: LanguageLexer> IncrementalHighlighter for LineCache<L> {
    fn highlight(&mut self, text: &str) -> Vec<HighlightSpan> {
        let mut next: Vec<CachedLine<L::LineState>> = Vec::new();
        let mut out: Vec<HighlightSpan> = Vec::new();
        let mut entry = L::LineState::default();
        let mut offset: u32 = 0;
        let mut relexed = 0usize;

        for (i, line) in text.split_inclusive('\n').enumerate() {
            let line_len = u32::try_from(line.len()).unwrap_or(u32::MAX);
            // Reuse iff the same-index cached line carried the same entry state
            // AND holds the same bytes. Both must hold: same bytes with a
            // different entry state (e.g. a block comment opened above) lexes
            // differently, and the fixpoint is exactly "entry state matches".
            let (exit, rel_spans) = match self.lines.get(i) {
                Some(c) if c.entry == entry && &*c.text == line => (c.exit, c.rel_spans.clone()),
                _ => {
                    relexed += 1;
                    self.lex_relative(line, entry)
                }
            };

            // Re-base the line-relative spans to absolute document offsets.
            for s in &rel_spans {
                out.push(HighlightSpan {
                    span: ByteSpan::new(offset + s.span.start, offset + s.span.end),
                    class: s.class,
                });
            }
            next.push(CachedLine {
                entry,
                exit,
                text: line.into(),
                rel_spans,
            });
            entry = exit;
            offset = offset.saturating_add(line_len);
        }

        self.lines = next;
        self.last_relexed = relexed;
        out
    }

    fn last_relexed(&self) -> usize {
        self.last_relexed
    }
}

/// The total fallback: any [`Highlighter`] as an [`IncrementalHighlighter`] by
/// re-lexing the whole document every call. Correct but not incremental — the
/// default a backend gets when it does not carry a `LineState` cache (e.g. a
/// tree-sitter backend that reparses via its own `Tree::edit`).
pub struct WholeReHighlighter {
    inner: Box<dyn Highlighter>,
    last_relexed: usize,
}

impl WholeReHighlighter {
    #[must_use]
    pub fn new(inner: Box<dyn Highlighter>) -> Self {
        Self {
            inner,
            last_relexed: 0,
        }
    }
}

impl IncrementalHighlighter for WholeReHighlighter {
    fn highlight(&mut self, text: &str) -> Vec<HighlightSpan> {
        self.last_relexed = text.split_inclusive('\n').count();
        self.inner.highlight(text)
    }
    fn last_relexed(&self) -> usize {
        self.last_relexed
    }
}

// ───────────────────────── plugin + registry ────────────────────

/// A language identity — an interned static name (closed by linkage, no open
/// enum to keep exhaustive).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Language(pub &'static str);

/// The unclassified fallback language — its highlighter emits everything as
/// [`HlClass::Plain`].
pub const PLAIN_TEXT: Language = Language("plaintext");

/// How a plugin claims a document.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Selector {
    /// A file extension without the dot, lowercase (e.g. `"rs"`).
    Extension(&'static str),
    /// An exact file name (e.g. `"Cargo.toml"`).
    Filename(&'static str),
}

/// A pluggable language backend: identity + how it's selected + its factory.
pub trait LanguagePlugin: Send + Sync {
    fn language(&self) -> Language;
    fn selectors(&self) -> &'static [Selector];
    fn make_highlighter(&self) -> Box<dyn Highlighter>;

    /// The incremental (line-cached) highlighter for this language. The default
    /// wraps [`make_highlighter`](Self::make_highlighter) in a
    /// [`WholeReHighlighter`] (correct, re-lexes the whole document); a
    /// `LineState`-carrying backend overrides this to return a real
    /// [`LineCache`] and gains the fixpoint re-lex for free.
    fn make_incremental(&self) -> Box<dyn IncrementalHighlighter> {
        Box::new(WholeReHighlighter::new(self.make_highlighter()))
    }
}

/// The registry. Total resolution: an unmatched path resolves to
/// [`PLAIN_TEXT`], never a panic.
pub struct Ecosystem {
    plugins: Vec<Box<dyn LanguagePlugin>>,
}

impl Default for Ecosystem {
    fn default() -> Self {
        Self::with_builtins()
    }
}

impl Ecosystem {
    /// An empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            plugins: Vec::new(),
        }
    }

    /// The batteries-included default: every built-in language plugin.
    #[must_use]
    pub fn with_builtins() -> Self {
        let mut eco = Self::new();
        for p in langs::builtins() {
            eco.plugins.push(p);
        }
        eco
    }

    pub fn register(&mut self, plugin: Box<dyn LanguagePlugin>) {
        self.plugins.push(plugin);
    }

    /// All languages the registry can highlight.
    #[must_use]
    pub fn languages(&self) -> Vec<Language> {
        self.plugins.iter().map(|p| p.language()).collect()
    }

    /// Resolve a file path to a language. Filename match wins over extension;
    /// no match resolves to [`PLAIN_TEXT`].
    #[must_use]
    pub fn resolve(&self, path: &str) -> Language {
        let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
        for p in &self.plugins {
            for sel in p.selectors() {
                if let Selector::Filename(f) = sel {
                    if name.eq_ignore_ascii_case(f) {
                        return p.language();
                    }
                }
            }
        }
        if let Some(ext) = name.rsplit_once('.').map(|(_, e)| e) {
            for p in &self.plugins {
                for sel in p.selectors() {
                    if let Selector::Extension(e) = sel {
                        if ext.eq_ignore_ascii_case(e) {
                            return p.language();
                        }
                    }
                }
            }
        }
        PLAIN_TEXT
    }

    /// A highlighter for a language, or the plain-text highlighter if none is
    /// registered for it.
    #[must_use]
    pub fn highlighter_for(&self, lang: Language) -> Box<dyn Highlighter> {
        for p in &self.plugins {
            if p.language() == lang {
                return p.make_highlighter();
            }
        }
        Box::new(PlainHighlighter)
    }

    /// The one call an editor needs: path → highlighter.
    #[must_use]
    pub fn highlighter_for_path(&self, path: &str) -> Box<dyn Highlighter> {
        self.highlighter_for(self.resolve(path))
    }

    /// An **incremental** highlighter for a language, or the plain-text
    /// fallback. This is the call an editor's render loop holds across frames
    /// — it re-lexes only what changed.
    #[must_use]
    pub fn incremental_highlighter_for(&self, lang: Language) -> Box<dyn IncrementalHighlighter> {
        for p in &self.plugins {
            if p.language() == lang {
                return p.make_incremental();
            }
        }
        Box::new(WholeReHighlighter::new(Box::new(PlainHighlighter)))
    }

    /// Path → incremental highlighter. The render-loop entry point.
    #[must_use]
    pub fn incremental_highlighter_for_path(
        &self,
        path: &str,
    ) -> Box<dyn IncrementalHighlighter> {
        self.incremental_highlighter_for(self.resolve(path))
    }
}

/// The plain-text highlighter: one `Plain` span over the whole text.
pub struct PlainHighlighter;

impl Highlighter for PlainHighlighter {
    fn highlight(&self, text: &str) -> Vec<HighlightSpan> {
        if text.is_empty() {
            return Vec::new();
        }
        vec![HighlightSpan {
            span: ByteSpan::new(0, u32::try_from(text.len()).unwrap_or(u32::MAX)),
            class: HlClass::Plain,
        }]
    }
}

// ───────────────────────────── theme ────────────────────────────

/// An 8-bit sRGB color. Palette-independent; the consumer maps it to its own
/// color type (egui `Color32`, ratatui `Color`, …).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    #[must_use]
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

/// Maps a [`HlClass`] to a color. The default is Nord (the fleet look). A
/// future `hikari-theme` crate will source this from `ishou_tokens`.
pub trait Theme: Send + Sync {
    fn color(&self, class: HlClass) -> Rgb;
}

/// The Nord Polar Night / Snow Storm / Aurora / Frost palette.
pub struct NordTheme;

impl Theme for NordTheme {
    fn color(&self, class: HlClass) -> Rgb {
        // Nord anchors: fg snow #D8DEE9, comment polar #616E88, keyword frost
        // #81A1C1, string aurora-green #A3BE8C, number aurora-purple #B48EAD,
        // type frost #8FBCBB, function frost #88C0D0, constant/bool orange
        // #D08770, punctuation snow #ECEFF4, error red #BF616A.
        match class {
            HlClass::Comment { .. } => Rgb::new(0x61, 0x6E, 0x88),
            HlClass::Keyword => Rgb::new(0x81, 0xA1, 0xC1),
            HlClass::KeywordArg | HlClass::Attribute => Rgb::new(0xB4, 0x8E, 0xAD),
            HlClass::Type | HlClass::Namespace => Rgb::new(0x8F, 0xBC, 0xBB),
            HlClass::Function => Rgb::new(0x88, 0xC0, 0xD0),
            HlClass::Str => Rgb::new(0xA3, 0xBE, 0x8C),
            HlClass::Escape | HlClass::Special => Rgb::new(0xEB, 0xCB, 0x8B),
            HlClass::Numeric { .. } => Rgb::new(0xB4, 0x8E, 0xAD),
            HlClass::Boolean | HlClass::Constant => Rgb::new(0xD0, 0x87, 0x70),
            HlClass::Operator => Rgb::new(0x81, 0xA1, 0xC1),
            HlClass::Punctuation => Rgb::new(0xEC, 0xEF, 0xF4),
            HlClass::Hyperlink => Rgb::new(0x5E, 0x81, 0xAC),
            HlClass::Error | HlClass::Removed => Rgb::new(0xBF, 0x61, 0x6A),
            HlClass::Warning => Rgb::new(0xEB, 0xCB, 0x8B),
            HlClass::Info => Rgb::new(0x81, 0xA1, 0xC1),
            HlClass::Hint => Rgb::new(0x5E, 0x81, 0xAC),
            HlClass::Added => Rgb::new(0xA3, 0xBE, 0x8C),
            HlClass::Variable | HlClass::Whitespace | HlClass::Unchanged | HlClass::Plain => {
                Rgb::new(0xD8, 0xDE, 0xE9)
            }
        }
    }
}

// ─────────────────────── the table-driven backend ───────────────

/// The hand-rolled backend: one char-class scanner parameterized by a
/// per-language [`LangTable`], covering the common shape of the built-in
/// languages. Heavier backends (tree-sitter, macro-generated) live in sibling
/// `hikari-*` crates and satisfy the same [`LanguageLexer`] / [`LanguagePlugin`]
/// traits.
pub mod langs {
    use super::{HlClass, Language, LanguageLexer, LanguagePlugin, LineDriven, Selector, SpanSink};

    /// Per-language lexing table.
    pub struct LangTable {
        pub keywords: &'static [&'static str],
        pub line_comments: &'static [&'static str],
        pub block_comment: Option<(&'static str, &'static str)>,
        pub string_delims: &'static [char],
        /// Treat `:name` tokens as [`HlClass::KeywordArg`] (lisp keywords).
        pub colon_keywords: bool,
    }

    /// Cross-line state: continuing a block comment or a string literal.
    #[derive(Clone, Copy, PartialEq, Eq, Default)]
    pub enum LineMode {
        #[default]
        Normal,
        InBlockComment,
        /// Inside a string opened on a previous line; carries the delimiter.
        InString(char),
    }

    /// The table lexer.
    pub struct TableLexer {
        pub table: &'static LangTable,
    }

    #[inline]
    fn is_ident_start(c: char) -> bool {
        c == '_' || c.is_alphabetic()
    }
    #[inline]
    fn is_ident_continue(c: char) -> bool {
        c == '_' || c.is_alphanumeric()
    }

    impl LanguageLexer for TableLexer {
        type LineState = LineMode;

        #[allow(clippy::too_many_lines)]
        fn lex_line(
            &self,
            line: &str,
            line_start: u32,
            entry: LineMode,
            sink: &mut SpanSink,
        ) -> LineMode {
            let t = self.table;
            let n = line.len();
            let base = line_start;
            let push = |sink: &mut SpanSink, s: usize, e: usize, class: HlClass| {
                sink.push(base + s as u32, base + e as u32, class);
            };
            let mut i = 0usize;
            let mut mode = entry;

            // Resume a continuation from the previous line.
            match mode {
                LineMode::InBlockComment => {
                    if let Some((_, close)) = t.block_comment {
                        if let Some(rel) = line.find(close) {
                            let e = rel + close.len();
                            push(sink, 0, e, HlClass::Comment { multiline: true });
                            i = e;
                            mode = LineMode::Normal;
                        } else {
                            push(sink, 0, n, HlClass::Comment { multiline: true });
                            return LineMode::InBlockComment;
                        }
                    } else {
                        mode = LineMode::Normal;
                    }
                }
                LineMode::InString(delim) => {
                    let e = scan_string_body(line, 0, delim);
                    match e {
                        Some(end) => {
                            push(sink, 0, end, HlClass::Str);
                            i = end;
                            mode = LineMode::Normal;
                        }
                        None => {
                            push(sink, 0, n, HlClass::Str);
                            return LineMode::InString(delim);
                        }
                    }
                }
                LineMode::Normal => {}
            }

            let _ = mode;
            'scan: while i < n {
                let c = line[i..].chars().next().unwrap();
                let cl = c.len_utf8();

                // whitespace run
                if c.is_whitespace() {
                    let s = i;
                    while i < n {
                        let d = line[i..].chars().next().unwrap();
                        if !d.is_whitespace() {
                            break;
                        }
                        i += d.len_utf8();
                    }
                    push(sink, s, i, HlClass::Whitespace);
                    continue 'scan;
                }

                // line comments
                for lc in t.line_comments {
                    if line[i..].starts_with(lc) {
                        push(sink, i, n, HlClass::Comment { multiline: false });
                        i = n;
                        continue 'scan;
                    }
                }

                // block comment open
                if let Some((open, close)) = t.block_comment {
                    if line[i..].starts_with(open) {
                        if let Some(rel) = line[i + open.len()..].find(close) {
                            let e = i + open.len() + rel + close.len();
                            push(sink, i, e, HlClass::Comment { multiline: true });
                            i = e;
                            continue 'scan;
                        }
                        push(sink, i, n, HlClass::Comment { multiline: true });
                        return LineMode::InBlockComment;
                    }
                }

                // string literal
                if t.string_delims.contains(&c) {
                    match scan_string_body(line, i + cl, c) {
                        Some(end) => {
                            push(sink, i, end, HlClass::Str);
                            i = end;
                            continue 'scan;
                        }
                        None => {
                            push(sink, i, n, HlClass::Str);
                            return LineMode::InString(c);
                        }
                    }
                }

                // number
                if c.is_ascii_digit() {
                    let s = i;
                    let mut is_float = false;
                    i += cl;
                    while i < n {
                        let d = line[i..].chars().next().unwrap();
                        if d.is_ascii_alphanumeric() || d == '_' {
                            i += d.len_utf8();
                        } else if d == '.' {
                            is_float = true;
                            i += 1;
                        } else {
                            break;
                        }
                    }
                    push(sink, s, i, HlClass::Numeric { float: is_float });
                    continue 'scan;
                }

                // colon keyword (lisp `:name`)
                if t.colon_keywords && c == ':' && i + 1 < n {
                    let next = line[i + 1..].chars().next().unwrap();
                    if is_ident_start(next) {
                        let s = i;
                        i += 1;
                        while i < n {
                            let d = line[i..].chars().next().unwrap();
                            if !is_ident_continue(d) {
                                break;
                            }
                            i += d.len_utf8();
                        }
                        push(sink, s, i, HlClass::KeywordArg);
                        continue 'scan;
                    }
                }

                // identifier / keyword
                if is_ident_start(c) {
                    let s = i;
                    i += cl;
                    while i < n {
                        let d = line[i..].chars().next().unwrap();
                        if !is_ident_continue(d) {
                            break;
                        }
                        i += d.len_utf8();
                    }
                    let word = &line[s..i];
                    let class = if t.keywords.contains(&word) {
                        HlClass::Keyword
                    } else if matches!(
                        word,
                        "true" | "false" | "True" | "False" | "None" | "nil" | "null"
                    ) {
                        HlClass::Boolean
                    } else if word.chars().next().is_some_and(char::is_uppercase) {
                        HlClass::Type
                    } else {
                        HlClass::Variable
                    };
                    push(sink, s, i, class);
                    continue 'scan;
                }

                // punctuation / operator (single char)
                let class = if "+-*/%=<>!&|^~".contains(c) {
                    HlClass::Operator
                } else {
                    HlClass::Punctuation
                };
                push(sink, i, i + cl, class);
                i += cl;
            }

            LineMode::Normal
        }
    }

    /// Scan a string body starting at `from` (just after the opening delim),
    /// honoring `\` escapes. Returns the byte index just past the closing
    /// delim, or `None` if the line ends first (string continues).
    fn scan_string_body(line: &str, from: usize, delim: char) -> Option<usize> {
        let n = line.len();
        let mut i = from;
        while i < n {
            let c = line[i..].chars().next().unwrap();
            let cl = c.len_utf8();
            if c == '\\' && i + cl < n {
                let e = line[i + cl..].chars().next().unwrap();
                i += cl + e.len_utf8();
                continue;
            }
            i += cl;
            if c == delim {
                return Some(i);
            }
        }
        None
    }

    /// A [`LanguagePlugin`] backed by a static [`LangTable`].
    pub struct TablePlugin {
        pub language: Language,
        pub selectors: &'static [Selector],
        pub table: &'static LangTable,
    }

    impl LanguagePlugin for TablePlugin {
        fn language(&self) -> Language {
            self.language
        }
        fn selectors(&self) -> &'static [Selector] {
            self.selectors
        }
        fn make_highlighter(&self) -> Box<dyn super::Highlighter> {
            Box::new(LineDriven::new(TableLexer { table: self.table }))
        }
        fn make_incremental(&self) -> Box<dyn super::IncrementalHighlighter> {
            // A real LineState-carrying cache — the table backend gains the
            // fixpoint re-lex, not the whole-document fallback.
            Box::new(super::LineCache::new(TableLexer { table: self.table }))
        }
    }

    // ── the built-in language tables ──

    static RUST_KW: &[&str] = &[
        "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum",
        "extern", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut",
        "pub", "ref", "return", "self", "Self", "static", "struct", "super", "trait", "type",
        "unsafe", "use", "where", "while",
    ];
    static RUST_TABLE: LangTable = LangTable {
        keywords: RUST_KW,
        line_comments: &["//"],
        block_comment: Some(("/*", "*/")),
        string_delims: &['"'],
        colon_keywords: false,
    };
    static RUST_SEL: &[Selector] = &[Selector::Extension("rs")];

    static PY_KW: &[&str] = &[
        "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del",
        "elif", "else", "except", "finally", "for", "from", "global", "if", "import", "in", "is",
        "lambda", "nonlocal", "not", "or", "pass", "raise", "return", "try", "while", "with",
        "yield",
    ];
    static PY_TABLE: LangTable = LangTable {
        keywords: PY_KW,
        line_comments: &["#"],
        block_comment: None,
        string_delims: &['"', '\''],
        colon_keywords: false,
    };
    static PY_SEL: &[Selector] = &[Selector::Extension("py")];

    static LISP_KW: &[&str] = &[
        "def", "defn", "defmacro", "defcaixa", "deflexer", "let", "lambda", "fn", "if", "cond",
        "when", "unless", "do", "quote",
    ];
    static LISP_TABLE: LangTable = LangTable {
        keywords: LISP_KW,
        line_comments: &[";"],
        block_comment: Some(("#|", "|#")),
        string_delims: &['"'],
        colon_keywords: true,
    };
    static LISP_SEL: &[Selector] = &[
        Selector::Extension("lisp"),
        Selector::Extension("lsp"),
        Selector::Extension("el"),
        Selector::Extension("scm"),
    ];

    static JSON_TABLE: LangTable = LangTable {
        keywords: &["true", "false", "null"],
        line_comments: &[],
        block_comment: None,
        string_delims: &['"'],
        colon_keywords: false,
    };
    static JSON_SEL: &[Selector] = &[Selector::Extension("json")];

    static TOML_TABLE: LangTable = LangTable {
        keywords: &["true", "false"],
        line_comments: &["#"],
        block_comment: None,
        string_delims: &['"', '\''],
        colon_keywords: false,
    };
    static TOML_SEL: &[Selector] = &[
        Selector::Extension("toml"),
        Selector::Filename("Cargo.lock"),
    ];

    static MD_TABLE: LangTable = LangTable {
        keywords: &[],
        line_comments: &[],
        block_comment: None,
        string_delims: &['`'],
        colon_keywords: false,
    };
    static MD_SEL: &[Selector] = &[Selector::Extension("md"), Selector::Extension("markdown")];

    /// Every built-in language plugin (the batteries-included default set).
    #[must_use]
    pub fn builtins() -> Vec<Box<dyn LanguagePlugin>> {
        vec![
            Box::new(TablePlugin {
                language: Language("rust"),
                selectors: RUST_SEL,
                table: &RUST_TABLE,
            }),
            Box::new(TablePlugin {
                language: Language("python"),
                selectors: PY_SEL,
                table: &PY_TABLE,
            }),
            Box::new(TablePlugin {
                language: Language("lisp"),
                selectors: LISP_SEL,
                table: &LISP_TABLE,
            }),
            Box::new(TablePlugin {
                language: Language("json"),
                selectors: JSON_SEL,
                table: &JSON_TABLE,
            }),
            Box::new(TablePlugin {
                language: Language("toml"),
                selectors: TOML_SEL,
                table: &TOML_TABLE,
            }),
            Box::new(TablePlugin {
                language: Language("markdown"),
                selectors: MD_SEL,
                table: &MD_TABLE,
            }),
        ]
    }
}

// ───────────────────────────── tests ────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn covers(text: &str, spans: &[HighlightSpan]) {
        // Coverage-complete + non-overlapping + forward-only by construction.
        let mut cursor = 0u32;
        for s in spans {
            assert_eq!(s.span.start, cursor, "gap/overlap at {cursor}");
            assert!(s.span.end > s.span.start);
            cursor = s.span.end;
        }
        assert_eq!(cursor as usize, text.len(), "partition does not cover text");
    }

    #[test]
    fn partition_is_coverage_complete() {
        let eco = Ecosystem::with_builtins();
        for (path, src) in [
            ("a.rs", "fn main() {\n    let x = 42; // hi\n}\n"),
            ("b.py", "def f(x):\n    return \"s\"  # c\n"),
            ("c.lisp", "(defcaixa :name \"x\" 42) ; c\n"),
            ("d.txt", "no language here\n"),
        ] {
            let h = eco.highlighter_for_path(path);
            let spans = h.highlight(src);
            covers(src, &spans);
        }
    }

    #[test]
    fn resolves_by_extension_not_always_rust() {
        let eco = Ecosystem::with_builtins();
        assert_eq!(eco.resolve("src/main.rs"), Language("rust"));
        assert_eq!(eco.resolve("app.py"), Language("python"));
        assert_eq!(eco.resolve("x.lisp"), Language("lisp"));
        assert_eq!(eco.resolve("Cargo.lock"), Language("toml"));
        // The bug: a non-Rust file must NOT resolve to rust.
        assert_eq!(eco.resolve("notes.txt"), PLAIN_TEXT);
        assert_ne!(eco.resolve("app.py"), Language("rust"));
    }

    #[test]
    fn rust_keyword_is_classified() {
        let eco = Ecosystem::with_builtins();
        let spans = eco.highlighter_for_path("a.rs").highlight("fn x");
        assert_eq!(spans[0].class, HlClass::Keyword); // `fn`
    }

    #[test]
    fn multiline_string_and_block_comment_thread_state() {
        let eco = Ecosystem::with_builtins();
        let spans = eco.highlighter_for_path("a.rs").highlight("/* a\nb */ x\n");
        covers("/* a\nb */ x\n", &spans);
        assert!(matches!(
            spans[0].class,
            HlClass::Comment { multiline: true }
        ));
    }

    #[test]
    fn plain_text_is_one_plain_span() {
        let h = PlainHighlighter;
        let spans = h.highlight("hello");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].class, HlClass::Plain);
    }

    // ── incremental line cache (the LineState-fixpoint seal) ──

    /// A tiny deterministic LCG so the differential fuzz is reproducible
    /// without a `rand` dependency (hikari-core is zero-dep).
    fn lcg(state: &mut u64) -> u64 {
        *state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        *state >> 33
    }

    /// S5 seal — the load-bearing invariant: an incremental re-lex is
    /// byte-identical to a one-shot re-lex, for every edit. Differential fuzz
    /// over random insert/delete edits against a Rust-ish corpus.
    #[test]
    fn incremental_is_byte_identical_to_one_shot() {
        let eco = Ecosystem::with_builtins();
        let one_shot = eco.highlighter_for_path("f.rs");
        let mut cache = eco.incremental_highlighter_for_path("f.rs");

        let alphabet: Vec<char> = "fn xy=42;{}\n/*/ \"ab\"//c".chars().collect();
        let mut text = String::from("fn main() {\n    let x = 1;\n}\n");
        let mut seed = 0x1234_5678_9abc_def0u64;

        for _ in 0..400 {
            // Random edit: insert a char or delete one, at a random boundary.
            let len = text.chars().count();
            let at = if len == 0 {
                0
            } else {
                (lcg(&mut seed) as usize) % (len + 1)
            };
            let byte_at = text.char_indices().nth(at).map_or(text.len(), |(b, _)| b);
            if len > 4 && lcg(&mut seed) % 2 == 0 {
                // delete one char
                if let Some((b, c)) = text[byte_at..].char_indices().next() {
                    let start = byte_at + b;
                    text.replace_range(start..start + c.len_utf8(), "");
                }
            } else {
                let c = alphabet[(lcg(&mut seed) as usize) % alphabet.len()];
                text.insert(byte_at, c);
            }

            let inc = cache.highlight(&text);
            let full = one_shot.highlight(&text);
            assert_eq!(inc, full, "incremental != one-shot for {text:?}");
            covers(&text, &inc);
        }
    }

    /// S6 seal — an unchanged document re-lexes NOTHING on the second call.
    #[test]
    fn idle_rehighlight_relexes_zero_lines() {
        let eco = Ecosystem::with_builtins();
        let mut cache = eco.incremental_highlighter_for_path("f.rs");
        let text = "fn a() {}\nfn b() {}\nfn c() {}\n";
        let _ = cache.highlight(text);
        let _ = cache.highlight(text); // idle re-render
        assert_eq!(cache.last_relexed(), 0, "idle re-render must re-lex nothing");
    }

    /// The fixpoint: a one-line edit re-lexes only the lines up to where the
    /// carried `LineState` re-converges — here, a leaf edit re-lexes just its
    /// own line, not the whole 60-line document.
    #[test]
    fn single_line_edit_relexes_locally() {
        let eco = Ecosystem::with_builtins();
        let mut cache = eco.incremental_highlighter_for_path("f.rs");
        let mut text = String::new();
        for i in 0..60 {
            text.push_str(&format!("let v{i} = {i};\n"));
        }
        let _ = cache.highlight(&text); // prime: 60 lines lexed
        // Edit line 30's value only (no block-comment/string state crosses).
        let edited = text.replacen("let v30 = 30;", "let v30 = 999;", 1);
        let _ = cache.highlight(&edited);
        assert_eq!(
            cache.last_relexed(),
            1,
            "a local edit must re-lex exactly its own line (state re-converges immediately)"
        );
    }

    /// A block comment opened mid-document propagates: re-lex continues past
    /// the edited line until the carried state re-converges (here, the `*/`).
    #[test]
    fn cross_line_state_change_propagates_then_converges() {
        let eco = Ecosystem::with_builtins();
        let mut cache = eco.incremental_highlighter_for_path("f.rs");
        let text = "let a = 1;\nlet b = 2;\nlet c = 3;\nlet d = 4;\n";
        let _ = cache.highlight(text);
        // Open a block comment on line 0 that closes on line 2.
        let edited = "let a = 1; /*\nstill comment\n*/ let c = 3;\nlet d = 4;\n";
        let inc = cache.highlight(edited);
        assert_eq!(inc, eco.highlighter_for_path("f.rs").highlight(edited));
        // Lines 0..=2 re-lexed (state in flight); line 3 reused (state reconverged).
        assert!(
            cache.last_relexed() <= 3,
            "re-lex must stop once the block comment closes and state reconverges"
        );
    }
}
