# Agent Prompt 01 — Bootstrap nbe-core & nbe-preflight

**Targets: SPEC v0.3.2 (`docs/spec.v0.3.md`) · `schemas/manifest.v0.3.json` · fixtures `tests/fixtures/valid_show_v0.3/` (must pass) and `tests/fixtures/valid_show/` (v0.2 — must be rejected).**

You are a senior Rust engineer building the `nbe` broadcast engine. Bootstrap the `nbe-core` and `nbe-preflight` crates. The workspace `Cargo.toml` and `schemas/manifest.v0.3.json` already exist in the repository root.

Read these first:

- `docs/spec.v0.3.md` — the normative spec. Key sections for this task: Section 15 (schema), Section 19 (preflight), Section 6.7 (migration), Appendix A (structural reference for agents).
- `VOCABULARY.md` — the term ledger. Use it: Element, not Layer; View, not Program.
- `schemas/manifest.v0.3.json` — the byte-exact normative schema.

## Step 1: Dependencies

- `nbe-core`: run `cargo add jsonschema --package nbe-core` (resolves the current version; serde/serde_json/thiserror/anyhow already come from workspace dependencies).
- `nbe-preflight`: wire `clap`, `tokio`, `tracing`, `tracing-subscriber` from workspace dependencies into `crates/nbe-preflight/Cargo.toml`.

## Step 2: Implement nbe-core

1. Create `src/manifest.rs` with serde structs mirroring `schemas/manifest.v0.3.json`. Appendix A of the spec gives the structural shape; the schema file is normative. At minimum: `Manifest`, `Network`, `Channel`, `Show`, `VideoSpec`, `AudioSpec`, `TransitionDefaults`, `OutputDefaults`, `Asset`, `Pulldown`, `LoopMetadata`, `LoudnessReport`, `GraphicTemplate`, `Transform`, `ChromaKey`, `LayerAudio`, `ClockConfig`, `Control`, `ControlBinding`, `Features`, `Element`, `Scene`, `Overlay`, `Animation`, `TransitionPreset`, `AutomationRule`, `Sequence`, `Item`, `Plugin`.
2. Implement `validate_manifest(json: &serde_json::Value) -> Result<(), ValidationError>` using the `jsonschema` crate against the schema embedded via `include_str!("../../../schemas/manifest.v0.3.json")`.
3. Implement the version gate (SPEC Section 6.7, Assumption 18, AC-28): a manifest whose `manifestVersion` is not `"0.3"` MUST produce a dedicated `ValidationError::MigrationRequired` variant whose message tells the user to run `nbe-migrate`. This check runs before schema validation and produces a machine-readable error.
4. Define a `PreflightReport` struct matching SPEC Section 19.2 exactly: top-level `manifestValid`, `airReady`, `errors`, `warnings`, `assets` (each with `id`, `kind`, `exists`, `sha256Ok`, `decodeFirstFrameOk`, `decodeLastFrameOk`, `cfr`, `frameRate`, `width`, `height`, `durationFrames`, `cadenceOk`, `loudness{integratedLufs,truePeakDbtp}`), `loops`, `scenes` (each with `sceneId`, `referencesOk`, `dagOk`), `plugins` (each with `pluginId`, `sandboxOk`), and `contactSheet`.

## Step 3: Implement nbe-preflight CLI

1. clap CLI with `--package-path <dir>` (required) and `--allow-warnings` (flag).
2. Read `<package-path>/manifest.json` and validate it via `nbe_core` — version gate first, then schema validation.
3. Verify every `assets[].source` exists relative to the package path.
4. Write `<package-path>/preflight_report.json` in the Section 19.2 shape.
5. Exit codes (SPEC Section 19.1): `0` = air-ready; `1` = warnings only (warnings are things like loudness nearing tolerance or a missing optional thumbnail — NOT missing optional schema fields, which schema defaults absorb); `2` = errors (schema validation failed, version gate failed, or missing required asset). CI blocks on exit != 0 unless `--allow-warnings`.

## Step 4: Tests and fixtures

1. Unit test in nbe-core: a minimal valid v0.3 manifest validates successfully.
2. Unit test in nbe-core: a v0.2 manifest produces `MigrationRequired` (the rejection half of AC-28).
3. Integration test in nbe-preflight: a temp-dir package whose manifest references a missing asset exits with code 2.
4. Verify both fixture directions:
   - `cargo run -p nbe-preflight -- --package-path tests/fixtures/valid_show_v0.3` exits 0.
   - `cargo run -p nbe-preflight -- --package-path tests/fixtures/valid_show` exits 2 with migration guidance in the report.

## Step 5: CI

Update `.github/workflows/ci.yml` so the preflight step gates both directions:

```yaml
      - name: v0.3 fixture passes
        run: cargo run -p nbe-preflight -- --package-path tests/fixtures/valid_show_v0.3
      - name: v0.2 fixture is rejected (migration gate)
        run: |
          if cargo run -p nbe-preflight -- --package-path tests/fixtures/valid_show; then
            echo "v0.2 package must be rejected" && exit 1
          fi
```

## Constraints

- No video decoding or audio analysis yet — schema validation, version gating, and file existence only.
- `thiserror` for library errors in nbe-core (including the `MigrationRequired` variant); `anyhow` for application errors in the CLI.
- `cargo check --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace` must all pass.
- Vocabulary discipline: `Element` (never Layer), `View` (never Program), `Sequence`/`Item` (Segment/Subsegment appear only in migration code and tests).
