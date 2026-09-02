# Prompt 01 — Definition of Done

**Scope:** `agents/prompts/01-bootstrap-core-preflight.md`, executed against SPEC v0.3.1 and refined by review prompts 01a/01b.
**Status:** Complete and locked. Future prompts MUST NOT change the behaviours enumerated here without an explicit spec revision and a dedicated new prompt authorizing the change.

---

## 1. What Prompt 1 delivered, in plain language

1. **Typed manifest model** — `crates/nbe-core/src/manifest.rs`: Rust serde types mirroring every `$defs` entry in `schemas/manifest.v0.3.json`. The schema is normative; the model conforms to it, never the reverse.
2. **Schema validation + version gate** — `crates/nbe-core/src/validate.rs`: `validate_manifest` with a `OnceLock`-cached `jsonschema` validator, gated first by a version check that distinguishes a missing `manifestVersion` (malformed, `MissingVersion`) from a wrong one (`MigrationRequired`, naming `nbe-migrate` per AC-28).
3. **Preflight CLI** — `crates/nbe-preflight`: validates the manifest, checks asset existence relative to the package root, writes `preflight_report.json` (SPEC 19.2 shape), and returns the SPEC 19.1 exit codes.
4. **Fixtures + CI gates** — `tests/fixtures/valid_show_v0.3/` must pass; `tests/fixtures/valid_show/` (v0.2) must be rejected; CI asserts both directions with exact exit codes, not just success/failure.

## 2. Acceptance criteria (locked)

Preflight is done, and remains done, only while all of the following hold:

| # | Criterion | Proof |
|---|---|---|
| P1-D1 | Any schema-valid `manifest.v0.3.json` MUST deserialize into the typed `Manifest` and round-trip (serialize → re-validate against the schema) without drift. | `crates/nbe-core/tests/model.rs::fixture_round_trips_through_typed_model_and_revalidates` |
| P1-D2 | A v0.2 manifest MUST fail the version gate with `ValidationError::MigrationRequired`, whose message names `nbe-migrate`. A missing `manifestVersion` MUST fail as malformed (`MissingVersion`), not as a migration target. | `validate.rs` unit tests + `preflight` integration tests |
| P1-D3 | `preflight_report.json` MUST be written on every run, including error paths, in the SPEC 19.2 shape. | `preflight_report_shape_matches_spec_19_2` + integration tests asserting the written file |
| P1-D4 | Exit codes MUST follow the 0/1/2 contract: `0` = air-ready (and only then `airReady` true), `1` = warnings only (blocked unless `--allow-warnings`), `2` = errors. | `valid_package_exits_0`, `missing_asset_exits_2`, `v02_manifest_exits_2_with_migration_guidance`; CI asserts the v0.2 case is exactly exit 2 |
| P1-D5 | The CI gate MUST assert exact outcomes for both fixture directions, not merely "command failed." | `.github/workflows/ci.yml` v0.3/v0.2 gate steps |

## 3. Locked surfaces

Future prompts MUST NOT, without a spec revision and a dedicated prompt:

1. Weaken or reorder the version gate / schema-validation pipeline.
2. Change the 0/1/2 exit-code semantics or the meaning of `airReady`.
3. Change the `PreflightReport` field names or drop fields from the SPEC 19.2 shape.
4. Relax either CI fixture gate (exact exit codes and report content assertions stay).
5. Touch `schemas/manifest.v0.3.json` as part of code work. Schema changes are spec work, not prompt work.

Forward evolution (Prompt 05's decode-based checks, warning producers, the exit-1 live test) extends this foundation; it does not reopen it. The `#[ignore]`d `warnings_only_exits_1_without_flag_0_with_it` test is the designated landing site for the first warning producer.
