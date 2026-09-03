# NBE — News Broadcasting Engine

Manifest-driven live news playout system: a Rust/wgpu real-time compositor driven by a TypeScript control plane. Built for an independent, self-hosted worker news network.

**Status:** founding scaffold. Normative spec: **SPEC v0.3.2** (`docs/spec.v0.3.md`; `SPEC.md` is the signpost) — the self-contained composable broadcast language: two-axis model, state-diff transitions, overlays, automation, plugins, quality profiles, abuse model. The v0.3.2 patch level adds the render-channel protocol, server-push frame shapes, and the command authorization matrix.

## Specification

| Document | Role |
|---|---|
| `docs/spec.v0.3.md` | Current normative specification (v0.3, patch level v0.3.2, self-contained) |
| `SPEC.md` | Signpost to the current version |
| `schemas/manifest.v0.3.json` | Normative show-manifest JSON Schema (byte-exact artifact, CI-validated) |
| `schemas/manifest.v0.2.json` | Prior schema version |
| `VOCABULARY.md` | Canonical expandable vocabulary ledger |
| `docs/spec.v0.1.md` | Historical base specification (WNBE-era naming) |
| `docs/spec.v0.2.md` | Historical amendment document |

## Layout

| Path | Contents |
|---|---|
| `schemas/` | Normative manifest JSON Schemas |
| `crates/nbe-core` | Shared types, manifest model, rundown state machine |
| `crates/nbe-engine` | wgpu compositor, audio graph, master clock |
| `crates/nbe-preflight` | CI-runnable show-package validator |
| `crates/nbe-protocol` | WebSocket command API types (serde) |
| `packages/control-plane` | TypeScript/Node rundown engine + command bus |
| `agents/prompts` | Reproducible coding-agent prompts |
| `templates/graphics` | Graphics template layouts + packaged fonts |
| `tests/fixtures` | Seeded valid/invalid show packages (v0.2 + v0.3) |
| `tests/integration` | Integration tests |
| `docs/` | Historical spec versions |

## Quickstart

```bash
cargo check --workspace
cargo run -p nbe-preflight -- --package-path ./tests/fixtures/valid_show
```

## License

MIT OR Apache-2.0
