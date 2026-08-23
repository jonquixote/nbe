# NBE — News Broadcasting Engine

Manifest-driven live news playout system: a Rust/wgpu real-time compositor driven by a TypeScript control plane. Built for an independent, self-hosted worker news network.

**Status:** founding scaffold. Normative spec: SPEC v0.2 (`SPEC.md`, which amends `docs/spec.v0.1.md`; v0.2 wins on conflict).

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

## Quickstart

```bash
cargo check --workspace
cargo run -p nbe-preflight -- --package-path ./tests/fixtures/valid_show
```

## License

MIT OR Apache-2.0
