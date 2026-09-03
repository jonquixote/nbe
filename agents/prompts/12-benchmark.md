# Agent Prompt 12 — OBS Baseline Benchmark Harness (tools/bench)

**Targets: SPEC v0.3.2 (`docs/spec.v0.3.md`) — Section 12 (12.1 metrics, 12.2 reference manifest, 12.3 artifact publication), Section 20.5 (performance acceptance). Prerequisites: Agent Prompts 01–11 merged — nbe must already pass Section 20.5 before these numbers mean anything.**

You are a senior Rust engineer building the `nbe` benchmark harness. This prompt does not change the engine. It builds the instrument that compares nbe against OBS on the same machine, and the report that publishes the result — including the parts where OBS wins.

Read these first:

- `docs/spec.v0.3.md` — Section 12 is your contract. The comparison table in 12.1, the reference manifest in 12.2, the publication rule in 12.3.
- `VOCABULARY.md` — term ledger.

## Quality bar

This prompt complies with the NBE Implementation Standards (`docs/implementation-standards.md`). Specifically:

- **Schema-driven typed models:** This prompt introduces the benchmark harness typed model (reference workload, metrics, report); these must be round-trip tested and enum-audited against the Section 12.1/12.2 definitions.
- **Strict CI contracts:** Any new binary or observable behaviour must have an exact CI gate (exit codes, key strings, behavioural invariants) (see Standards §2), including the drift-check and measurement-completeness invariants.
- **Prompt structure compliance:** This prompt explicitly lists Forbidden changes, New tests required, and CI changes required (see Standards §3).

## Step 0: Scope discipline

Allowed now: the benchmark harness, the reference workload artifacts, the report template. Forbidden: engine changes (this prompt touches nothing under `crates/`), benchmark claims that cannot be reproduced from the committed artifacts.

## Step 1: The reference workload

- Commit the Section 12.2 reference manifest to the repo: camera, two pre-rolled clips, a lower-third, an image Element, record and stream outputs on, 1080p30.
- Commit the equivalent OBS scene collection. The harness runs a drift check first: if the nbe manifest and the OBS scene describe different workloads, it refuses to compare. Comparing unlike workloads is the classic benchmark lie; make it impossible.

## Step 2: The driver

- Runs both engines on the same Mac mini, pinned OBS version, N runs interleaved (nbe, OBS, nbe, OBS...) to cancel thermal drift.
- Collects the Section 12.1 metrics per run: cold start to ready, live latency camera→program, dropped frames over a 10-minute soak, CPU/GPU %, memory RSS, sustained load.
- The nbe side reads its real telemetry (`/metrics`). The OBS side reads obs-websocket stats plus system sampling. No estimated numbers — every cell comes from a measurement.

## Step 3: The report

- A report template producing the Section 12.1 comparison table: median plus spread per metric, per engine, hardware and date noted.
- The `Where OBS is expected to win` section is mandatory, not optional. The harness fills the numbers; a human writes the prose. A benchmark with no honesty section is marketing.

## Step 4: Publication

- Section 12.3: the script, the manifests, and the report template live in the repo, public. Results land under `docs/benchmarks/` with hardware and date.
- Anyone with the same Mac mini should be able to run the harness and get the same table within spread.

## Step 5: Tests

1. **Drift check**: mismatched workloads refuse to run and say why.
2. **Version pinning**: the harness verifies the OBS build it was calibrated against and warns on any other.
3. **Completeness**: the generated report has every Section 12.1 field populated from measurement, or it fails.
4. **Source truth**: nbe numbers come from `/metrics`; OBS numbers from obs-websocket/system sampling — the harness asserts both paths are live before running.

CI: the harness itself is not CI-gated (it needs the bench machine), but any harness code in the workspace passes `cargo check --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`.

## Constraints

- No engine changes. No estimated cells. No missing honesty section.
- `anyhow` for the binary, `thiserror` for library errors.
- Vocabulary discipline: `View`, `Element`, `Sequence`, `Item`.
