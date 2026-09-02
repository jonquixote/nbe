# Agent Prompt 01b — P1 Review Fixes: Make Prompt 01 Flawless

**Targets: branch `P1` of this repo. Fixes a two-pass human review of the Prompt 01 work (`agents/prompts/01-bootstrap-core-preflight.md`). Do not touch scope: no new features, no new files beyond the one test enumerated below. Prompts 02+ must find zero known defects here.**

You are a senior Rust engineer. The Prompt 01 implementation on `P1` is architecturally correct — version gate before schema, `MigrationRequired` naming `nbe-migrate`, exit codes 0/1/2 per SPEC 19.1, `PreflightReport` matching Section 19.2, temp-dir integration tests. The review found latent bugs the test suite cannot see, because the typed model is never exercised. Fix them, pin them with tests, and prove it in CI.

## Step 0: The core insight to internalize

Nothing in the current code deserializes a manifest into the typed `Manifest` struct. Validation runs on raw `serde_json::Value`; asset enumeration in `main.rs` reads raw JSON; integration tests build raw `json!` values. The 29-type model in `manifest.rs` is Prompt 01's central deliverable and has zero coverage. Every bug below is a consequence. The round-trip test in Step 2 is the permanent fix — the individual bugs are just its first catches.

## Step 1: Fix the deserialization bugs in `crates/nbe-core/src/manifest.rs`

1. **`FrameRate` (critical).** `schemas/manifest.v0.3.json` declares `"frameRate": { "enum": [30, 60] }` — JSON numbers. The enum's `#[serde(rename = "30")]` / `"60"` renames match only strings, so deserializing `{"frameRate": 30}` into `VideoSpec` fails. Every schema-valid manifest trips this. Fix with numeric-aware deserialization (e.g. `serde_repr` over `u32`, or `#[serde(from = "u32")]` on `VideoSpec`); do not weaken the schema — it is normative.
2. **`ClockFormat::HhMmSs`.** Missing `#[serde(rename = "HH:mm:ss")]`; the schema enum is `["HH:mm", "HH:mm:ss", "hh:mm A", "locale"]`. A manifest writing the default explicitly fails to deserialize.
3. **`TextureFormat::Auto`.** Missing `#[serde(rename = "auto")]` — the schema's convention is lowercase (`nv12`, `rgba8`, `vram`); confirm against the schema and match it.
4. **Audit sweep.** Diff every enum's serde representation against its schema enum, and every struct's field names and optionality against its `$defs` entry. Fix every mismatch you find, not just the three named. Note for the record: `Manifest.assets` has `#[serde(default)]` while the schema requires `assets` — harmless post-validation, but align it.
5. **Consistency:** `Asset` and `Channel` lack `deny_unknown_fields` where siblings have it and the schema sets `additionalProperties: false`. Align.

## Step 2: Pin the model with tests

1. **Round-trip (the important one):** deserialize `tests/fixtures/valid_show_v0.3/manifest.json` into `Manifest`, serialize back, and re-validate the output with `validate_manifest`. This test would have caught all of Step 1. It lives in nbe-core and is non-negotiable.
2. **Explicit-defaults test:** a manifest identical to the fixture but with `frameRate`, clock `format`, and loop `textureFormat` written explicitly with their default spellings deserializes cleanly.
3. **Report shape lock:** serialize a fully-populated `PreflightReport` and assert the exact camelCase field names of Section 19.2 — top level plus `assets[]` (including `loudness.integratedLufs` / `truePeakDbtp`), `loops[]`, `scenes[]`, `plugins[]`, `contactSheet`.
4. **Leave a marker:** the exit-1/`--allow-warnings` path in `main.rs` is wired but unreachable today (nothing calls `push_warning`). Add a `#[ignore]`d test (or a clearly-named TODO) stating: the first prompt that introduces a warning producer must add the exit-1 test.

## Step 3: Stricten CI (`.github/workflows/ci.yml`)

1. The v0.2 gate currently passes on *any* non-zero exit. Replace with: run preflight on `tests/fixtures/valid_show`, capture the exit code, assert it is exactly `2`, and grep the written `preflight_report.json` for both `migrationRequired` and `nbe-migrate`.
2. Add a v0.3-gate assertion that the run wrote a report with `"airReady": true` (e.g. via `grep` or `jq`).
3. Do not broaden triggers — `push: [main]` + `pull_request` is right. State in your final report that CI only runs on PRs, so the reviewer knows the green check must come from the PR.

## Step 4: Hygiene

1. Drop `anyhow` from `crates/nbe-core/Cargo.toml` — libraries are thiserror-only; nothing uses it.
2. Add `tests/fixtures/**/preflight_report.json` to `.gitignore` — the CI gate (and any local run) writes reports into the fixture packages, and the tree must stay clean.
3. In `validate.rs`, cache the compiled schema validator in a `std::sync::OnceLock` instead of recompiling per call.
4. In `crates/nbe-preflight/tests/integration.rs`, locate the binary with `env!("CARGO_BIN_EXE_nbe-preflight")` instead of the hand-rolled `target/debug` path.
5. In `check_version`, distinguish a missing `manifestVersion` from a wrong one in the error message — `found: ""` reads like a migration target when it is really a malformed manifest.
6. `valid_package_exits_0` should also assert the written report has `airReady == true` and empty `errors`.

## Step 5: Prove it

- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace` all pass — on macos-14, not just claimed.
- Both CI fixture gates pass as rewritten.
- Your final report lists: every schema/model mismatch found in the Step 1.4 sweep (even if the list is just the three named bugs), the new test count, and confirmation that the report files are git-ignored.

## Constraints

- No scope growth: no new CLI flags, no new report fields, no decode or hash checks (Prompt 05 deepens preflight), no changes to `schemas/manifest.v0.3.json` — the schema is normative; the model conforms to it.
- `thiserror` in nbe-core, `anyhow` only in the binary. Workspace dependency inheritance and `[lints] workspace` stay.
- Vocabulary discipline: `View`, `Element`, `Sequence`, `Item`, `Marker`.
