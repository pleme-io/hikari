# hikari (光)

The pluggable syntax-highlighting spine for pleme-io editors and tools.

`hikari-core` is a small, **zero-dependency** library that owns the narrow
interface every language backend lowers to:

- **`ByteSpan` + `HlClass`** — a palette-independent, byte-offset highlight
  model produced **only** through a forward-cursor `SpanSink` that makes gapped
  / overlapping / reversed partitions *unrepresentable*: every byte is
  classified exactly once, by construction.
- **`LanguageLexer`** — the authored backend trait. Its associated
  `LineState: Copy + Eq` makes incremental line-restart re-lex a *type
  property*, not a hand-coded optimization.
- **`Highlighter`** — an object-safe render-facing trait, bridged from any
  `LanguageLexer` by the blanket `LineDriven` adapter (no backend hand-writes
  it).
- **`Ecosystem`** — a registry with total, panic-free resolution: an unknown
  extension resolves to `PLAIN_TEXT`, never a panic, never "everything is one
  language".
- **`Theme`** — `HlClass → Rgb` (Nord by default).

Batteries included: a table-driven backend covering **Rust, Python, Lisp,
JSON, TOML, Markdown** ships in the box.

```rust
use hikari_core::{Ecosystem, NordTheme, Theme};

let eco = Ecosystem::with_builtins();
let hl = eco.highlighter_for_path("src/main.rs");   // -> a Rust highlighter
for span in hl.highlight("fn main() {}") {
    let rgb = NordTheme.color(span.class);
    // paint &text[span.span.range()] in `rgb`
}
```

## Why zero dependencies

The spine must vendor with no transitive weight into any consumer — an
egui/GPU editor, a TUI, a CLI. Heavier backends (tree-sitter grammars, the
tatara-lisp macro-generated `(deflexer …)` output) ship as **separate
`hikari-*` crates** in this workspace and satisfy the same traits, so adding a
language is additive and never forces a dependency on consumers that don't use
it.

## Published

`hikari-core` is published to [crates.io](https://crates.io/crates/hikari-core)
on every merge to `main` via pleme-io AUTO-RELEASE.

## License

MIT.
