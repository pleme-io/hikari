# hikari (光)

> **★★★ CSE / Knowable Construction.** This repo operates under
> **Constructive Substrate Engineering** — canonical spec at
> [`pleme-io/theory/CONSTRUCTIVE-SUBSTRATE-ENGINEERING.md`](https://github.com/pleme-io/theory/blob/main/CONSTRUCTIVE-SUBSTRATE-ENGINEERING.md).
> The Compounding Directive is in the org-level pleme-io/CLAUDE.md.

The pluggable syntax-highlighting spine for pleme-io editors + tools. Design
doc + roadmap: [`theory/HIKARI.md`](https://github.com/pleme-io/theory/blob/main/HIKARI.md).

## Layout

- `crates/hikari-core` — the spine: `ByteSpan`/`HlClass`/`SpanSink`,
  `LanguageLexer`/`LineDriven`/`Highlighter`, `Ecosystem`/`LanguagePlugin`,
  `Theme`, and the batteries-included table backend (Rust/Python/Lisp/JSON/
  TOML/Markdown). **Zero dependencies** — heavy backends ship as sibling
  `hikari-*` crates.

Roadmap members (not yet landed): `hikari-ts` (tree-sitter), `hikari-token`
(the shared `Semantic`/`HlClass` collapse), `hikari-theme` (ishou-sourced),
`hikari-lang-*` (fleet-owned lexer adapters), `hikari-wasm-host`.

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
