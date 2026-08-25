# NBE — News Broadcasting Engine

Manifest-driven live news playout system: a Rust/wgpu real-time compositor driven by a TypeScript control plane. Built for an independent, self-hosted worker news network.

**Status:** founding scaffold. Normative spec: **SPEC v0.2.5** (`SPEC.md`) — a single self-contained document consolidating v0.1 + v0.2 + the v0.2.1 errata.

## Specification

| Document | Role |
|---|---|
| `SPEC.md` | Current normative specification (v0.2.5, consolidated and self-contained) |
| `schemas/manifest.v0.2.json` | Normative show-manifest JSON Schema (byte-exact artifact, CI-validated) |
| `docs/spec.v0.1.md` | Historical base specification (WNBE-era naming) |
| `docs/spec.v0.2.md` | Historical amendment document |

## Layout

| Path | Contents |
|---|---|
| `schemas/manifest.v0.2.json` | Normative show-manifest JSON Schema |
| `crates/nbe-core` | Shared types, manifest model, rundown state machine |
| `crates/nbe-engine` | wgpu compositor, audio graph, master clock |
| `crates/nbe-preflight` | CI-runnable show-package validator |
| `crates/nbe-protocol` | WebSocket command API types (serde) |
| `packages/control-plane` | TypeScript/Node rundown engine + command bus |
| `agents/prompts` | Reproducible coding-agent prompts |
| `templates/graphics` | Graphics template layouts + packaged fonts |
| `tests/fixtures` | Seeded valid/invalid show packages |
| `tests/integration` | Integration tests |
| `docs/` | Historical spec versions |

## Quickstart

```bash
cargo check --workspace
cargo run -p nbe-preflight -- --package-path ./tests/fixtures/valid_show
```

## License

MIT OR Apache-2.0
