# Agent Prompt 01 — Bootstrap nbe-core & nbe-preflight

You are a senior Rust engineer building the `nbe` broadcast engine. Bootstrap the `nbe-core` and `nbe-preflight` crates. The workspace `Cargo.toml` and `schemas/manifest.v0.2.json` already exist in the repository root.

## Step 1: Dependencies

- `nbe-core`: run `cargo add jsonschema --package nbe-core` (resolves the current version; serde/serde_json/thiserror/anyhow already come from workspace dependencies).
- `nbe-preflight`: wire `clap`, `tokio`, `tracing`, `tracing-subscriber` from workspace dependencies into `crates/nbe-preflight/Cargo.toml`.

## Step 2: Implement nbe-core

1. Create `src/manifest.rs` with serde structs mirroring `schemas/manifest.v0.2.json`.
2. Implement `validate_manifest(json: &serde_json::Value) -> Result<(), ValidationError>` using the `jsonschema` crate against the schema embedded via `include_str!("../../../schemas/manifest.v0.2.json")`.
3. Define a `PreflightReport` struct matching SPEC.md section 16.2 exactly: top-level `manifestValid`, `airReady`, `errors`, `warnings`, `assets` (each with `id`, `kind`, `exists`, `sha256Ok`, `decodeFirstFrameOk`, `decodeLastFrameOk`, `cfr`, `frameRate`, `width`, `height`, `durationFrames`, `cadenceOk`, `loudness{integratedLufs,truePeakDbtp}`), `loops` (each with `assetId`, `periodFrames`, `seamless`, `cachePolicySelected`), and `contactSheet`.

## Step 3: Implement nbe-preflight CLI

1. clap CLI with `--package-path <dir>` (required) and `--allow-warnings` (flag).
2. Read `<package-path>/manifest.json` and validate it via `nbe_core`.
3. Verify every `assets[].source` exists relative to the package path.
4. Write `<package-path>/preflight_report.json` in the section 16.2 shape.
5. Exit codes (SPEC section 16.1): `0` = air-ready; `1` = warnings only (warnings are things like loudness nearing tolerance or a missing optional thumbnail — NOT missing optional schema fields, which schema defaults absorb); `2` = errors. CI blocks on exit != 0 unless `--allow-warnings`.

## Step 4: Tests and fixtures

1. Unit test in nbe-core: a minimal valid manifest JSON string validates successfully.
2. Integration test in nbe-preflight: a temp-dir package whose manifest references a missing asset exits with code 2.
3. Verify `tests/fixtures/valid_show/` passes: manifest validates and all referenced media exist (placeholder files are fine at this stage).

## Constraints

- No video decoding or audio analysis yet — schema validation and file existence only.
- `thiserror` for library errors in nbe-core; `anyhow` for application errors in the CLI.
- `cargo check --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace` must all pass.
