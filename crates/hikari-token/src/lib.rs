//! hikari (光) — the one fleet-facing semantic highlight vocabulary.
//!
//! The pleme-io fleet grew several byte-identical copies of a 16-variant
//! highlight-class enum (`escriba_ts::Semantic`, `caixa_theme::Semantic`, …).
//! This crate owns the single canonical definition — [`Semantic`] — plus a
//! **total** morphism to/from [`hikari_core::HlClass`] (the palette-independent
//! class the highlighter backends emit). Consumers re-export [`Semantic`] from
//! here and delete their local copy, so the vocabulary lives in one place and
//! every consumer inherits changes on the next dep bump.
//!
//! A consumer takes ONE dependency: [`HlClass`], [`ByteSpan`], and
//! [`HighlightSpan`] are re-exported.

#![forbid(unsafe_code)]

pub use hikari_core::{ByteSpan, HighlightSpan, HlClass};

use serde::{Deserialize, Serialize};

/// The fleet semantic highlight class — the theme-facing vocabulary a renderer
/// maps to color. Variant set + **order** are load-bearing: they are
/// byte-identical to the historical `escriba_ts::Semantic` /
/// `caixa_theme::Semantic` so serde / `JsonSchema` output does not drift when
/// those crates re-export this one.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Semantic {
    Keyword,
    Symbol,
    KeywordArg,
    String,
    Number,
    Literal,
    Comment,
    Accent,
    Muted,
    Error,
    Warning,
    Info,
    Hint,
    Added,
    Removed,
    Unchanged,
}

impl Semantic {
    /// The full variant set, in canonical order.
    #[must_use]
    pub const fn all() -> [Semantic; 16] {
        use Semantic::{
            Accent, Added, Comment, Error, Hint, Info, Keyword, KeywordArg, Literal, Muted, Number,
            Removed, String, Symbol, Unchanged, Warning,
        };
        [
            Keyword, Symbol, KeywordArg, String, Number, Literal, Comment, Accent, Muted, Error,
            Warning, Info, Hint, Added, Removed, Unchanged,
        ]
    }
}

/// `Semantic -> HlClass` — total. Round-trips to identity through
/// [`hlclass_to_semantic`] for every `Semantic` variant (see the test), now
/// that `HlClass` carries the diagnostic + diff variants.
impl From<Semantic> for HlClass {
    fn from(s: Semantic) -> Self {
        match s {
            Semantic::Keyword => HlClass::Keyword,
            Semantic::Symbol => HlClass::Punctuation,
            Semantic::KeywordArg => HlClass::KeywordArg,
            Semantic::String => HlClass::Str,
            Semantic::Number => HlClass::Numeric { float: false },
            Semantic::Literal => HlClass::Constant,
            Semantic::Comment => HlClass::Comment { multiline: false },
            Semantic::Accent => HlClass::Special,
            Semantic::Muted => HlClass::Plain,
            Semantic::Error => HlClass::Error,
            Semantic::Warning => HlClass::Warning,
            Semantic::Info => HlClass::Info,
            Semantic::Hint => HlClass::Hint,
            Semantic::Added => HlClass::Added,
            Semantic::Removed => HlClass::Removed,
            Semantic::Unchanged => HlClass::Unchanged,
        }
    }
}

/// `HlClass -> Semantic` — total. `HlClass` is richer (it carries `Type` /
/// `Function` / `Escape` / … that `Semantic` lacks), so this direction is
/// lossy for those: they fold to the nearest `Semantic` (documented per arm).
/// The mapping is chosen so `Semantic -> HlClass -> Semantic` is the identity.
#[must_use]
pub fn hlclass_to_semantic(c: HlClass) -> Semantic {
    match c {
        HlClass::Keyword => Semantic::Keyword,
        HlClass::KeywordArg => Semantic::KeywordArg,
        HlClass::Str => Semantic::String,
        HlClass::Numeric { .. } => Semantic::Number,
        HlClass::Boolean | HlClass::Constant => Semantic::Literal,
        HlClass::Comment { .. } => Semantic::Comment,
        // accent-colored identifiers + emphasis fold to Accent.
        HlClass::Type
        | HlClass::Function
        | HlClass::Namespace
        | HlClass::Attribute
        | HlClass::Escape
        | HlClass::Special
        | HlClass::Hyperlink => Semantic::Accent,
        // symbolic tokens fold to Symbol.
        HlClass::Punctuation | HlClass::Operator => Semantic::Symbol,
        HlClass::Error => Semantic::Error,
        HlClass::Warning => Semantic::Warning,
        HlClass::Info => Semantic::Info,
        HlClass::Hint => Semantic::Hint,
        HlClass::Added => Semantic::Added,
        HlClass::Removed => Semantic::Removed,
        // normal / muted / whitespace text.
        HlClass::Plain => Semantic::Muted,
        HlClass::Variable | HlClass::Whitespace | HlClass::Unchanged => Semantic::Unchanged,
    }
}

#[cfg(test)]
mod tests {
    use super::{HlClass, Semantic, hlclass_to_semantic};

    #[test]
    fn semantic_hlclass_roundtrips_to_identity() {
        for s in Semantic::all() {
            let hl: HlClass = s.into();
            assert_eq!(
                hlclass_to_semantic(hl),
                s,
                "Semantic -> HlClass -> Semantic must be identity for {s:?}",
            );
        }
    }

    #[test]
    fn serde_is_snake_case_stable() {
        assert_eq!(
            serde_json::to_string(&Semantic::KeywordArg).unwrap(),
            "\"keyword_arg\"",
        );
        let back: Semantic = serde_json::from_str("\"unchanged\"").unwrap();
        assert_eq!(back, Semantic::Unchanged);
    }
}
