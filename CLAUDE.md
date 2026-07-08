# hikari (光)

> **★★★ CSE / Knowable Construction.** This repo operates under
> **Constructive Substrate Engineering** — canonical spec at
> [`pleme-io/theory/CONSTRUCTIVE-SUBSTRATE-ENGINEERING.md`](https://github.com/pleme-io/theory/blob/main/CONSTRUCTIVE-SUBSTRATE-ENGINEERING.md).
> The Compounding Directive is in the org-level pleme-io/CLAUDE.md.

The pluggable syntax-highlighting spine for pleme-io editors + tools. Design
doc + roadmap: [`theory/HIKARI.md`](https://github.com/pleme-io/theory/blob/main/HIKARI.md).

## Layout

- `crates/hikari-core` — the spine: `ByteSpan`/`HlClass`/`SpanSink`,
  `LanguageLexer`/`LineDriven`/`Highlighter`, the incremental
  `IncrementalHighlighter`/`LineCache` (the `LineState`-fixpoint re-lex),
  `Ecosystem`/`LanguagePlugin`, `Theme`, and the batteries-included table
  backend (Rust/Python/Lisp/JSON/TOML/Markdown). **Zero dependencies** — heavy
  backends ship as sibling `hikari-*` crates.
- `crates/hikari-token` — the palette-independent `Semantic` collapse
  (16 variants) + total `From<Semantic> for HlClass` + `hlclass_to_semantic`;
  `schema` feature. The fleet's shared token vocabulary (escriba, caixa, AsterIDE
  all `pub use` this — no duplicated `Semantic`).
- `crates/hikari-ts` — the tree-sitter backend: `TreeSitterHighlighter` over
  `escriba-ts` funneled through `SpanSink::for_document`, `language_matrix.rs`
  forcing-function.

All three publish to crates.io on merge (currently 0.1.x).

Roadmap members (not yet landed): `hikari-theme` (ishou-sourced palette),
`hikari-lang-*` (fleet-owned lexer adapters, e.g. `hikari-lang-zsh` absorbing
frost), `hikari-wasm-host`.

## Build / test

```
cargo test --workspace     # unit tests (5 in hikari-core)
cargo clippy --workspace -- -D warnings
nix build                  # hikari-core via rustPlatform (zero-dep)
```

## Release

Every merge to `main` bumps + tags + publishes each member to crates.io via
`.github/workflows/auto-release.yml` (substrate `rust-auto-release.yml`).
Requires `CRATES_API_TOKEN` (inherited from org secrets).

## Waivers

`skip-magma-execution` / `skip-helm-native` / `skip-gitops` /
`skip-continuous-convergence` / `skip-platform-mediated` / `skip-urdume` /
`skip-tela` / `skip-quadro`: hikari is a pure Rust library — no runtime,
no cluster workload, no service/frontend surface. `skip-shigoto`: no work
graph. `skip-catalog`: fewer than three typed domains.
