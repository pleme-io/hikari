//! The version-coherence + coverage forcing-function.
//!
//! Every tree-sitter grammar hikari-ts registers MUST have a row here. If a
//! new grammar is added upstream and no row is added, `every_grammar_has_a_row`
//! fails the build — the guard that keeps the runtime ABI + the capture→HlClass
//! intent coherent as the go-wide language set grows (HIKARI.md §V).

use hikari_core::Highlighter;
use hikari_ts::TreeSitterHost;

/// One row per grammar hikari-ts intends to serve. Sample source proves the
/// grammar links + classifies (not all-Plain).
struct Row {
    grammar: &'static str,
    sample: &'static str,
}

const MATRIX: &[Row] = &[
    Row {
        grammar: "rust",
        sample: "fn main() { let x = 42; }",
    },
    Row {
        grammar: "python",
        sample: "def f(x):\n    return 42\n",
    },
    Row {
        grammar: "json",
        sample: "{\"a\": 1, \"b\": true}\n",
    },
    Row {
        grammar: "bash",
        sample: "echo \"hi\" # note\n",
    },
];

#[test]
fn every_grammar_has_a_row() {
    let host = TreeSitterHost::builtin().expect("builtin grammars");
    let registered: Vec<String> = host.languages().map(str::to_string).collect();
    let missing: Vec<&String> = registered
        .iter()
        .filter(|g| !MATRIX.iter().any(|r| r.grammar == g.as_str()))
        .collect();
    assert!(
        missing.is_empty(),
        "grammars registered by hikari-ts but missing a language_matrix row \
         (add a Row + its capture→HlClass intent): {missing:?}",
    );
}

#[test]
fn every_row_links_and_classifies() {
    let host = TreeSitterHost::builtin().expect("builtin grammars");
    for row in MATRIX {
        let spans = host.highlighter(row.grammar).highlight(row.sample);
        // coverage-complete
        let mut cursor = 0u32;
        for s in &spans {
            assert_eq!(s.span.start, cursor, "[{}] gap at {cursor}", row.grammar);
            cursor = s.span.end;
        }
        assert_eq!(
            cursor as usize,
            row.sample.len(),
            "[{}] partition must cover the sample",
            row.grammar,
        );
        assert!(
            spans.iter().any(|s| s.class != hikari_core::HlClass::Plain),
            "[{}] must classify at least one non-Plain span",
            row.grammar,
        );
    }
}
