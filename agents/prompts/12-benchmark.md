# Agent Prompt 12 — OBS Baseline Benchmark Harness (tools/bench)

**Targets: SPEC v0.4 (`docs/spec.v0.4.md`) — AC-11 (OBS baseline comparison — metrics, parity bar), Section 21 (hardware tiers; AC-11 MUST also run on the floor device), AC-5 (30-minute zero-drop). NOTE: this prompt previously cited "Section 12" for benchmark metrics; §12 is Deterministic loops and always was. v0.4 has no benchmark *section*, but **AC-11 provides more than a name**: it enumerates six metrics a published comparison MUST report (dropped frames, CPU utilization, GPU utilization, glass-to-glass latency, take latency, recording crash safety), sets the parity bar (no worse than OBS on dropped frames, CPU and GPU on Tier-1 hardware), and carries a publication rule (a published comparison, and floor-device results published separately). **The one thing genuinely absent from the spec is the reference manifest** — that is this prompt's own addition, as are any metrics beyond AC-11's six. Prerequisites: Agent Prompts 01–11 merged — nbe must already pass **AC-5** before these numbers mean anything.**

You are a senior Rust engineer building the `nbe` benchmark harness. This prompt does not change the engine. It builds the instrument that compares nbe against OBS on the same machine, and the report that publishes the result — including the parts where OBS wins.

Read these first:

- `docs/spec.v0.4.md` — **AC-11 is your contract**, and it is a real one: six required metrics, a parity bar, and a publication rule. What it does not define is a reference manifest; that is new here, not inherited.
- `VOCABULARY.md` — term ledger.

## Quality bar

This prompt complies with the NBE Implementation Standards (`docs/implementation-standards.md`). Specifically:

- **Schema-driven typed models:** This prompt introduces the benchmark harness typed model (reference workload, metrics, report); these must be round-trip tested and enum-audited against **AC-11's six reported metrics**. The reference-workload and report types are this prompt's own — the spec defines no such model, so there is nothing upstream to audit them against.
- **Strict CI contracts:** Any new binary or observable behaviour must have an exact CI gate (exit codes, key strings, behavioural invariants) (see Standards §2), including the drift-check and measurement-completeness invariants.
- **Prompt structure compliance:** This prompt explicitly lists Forbidden changes, New tests required, and CI changes required (see Standards §3).

## Step 0: Scope discipline

Allowed now: the benchmark harness, the reference workload artifacts, the report template. Forbidden: engine changes (this prompt touches nothing under `crates/`), benchmark claims that cannot be reproduced from the committed artifacts.

## Step 1: The reference workload

- Commit **this prompt's reference manifest** (prompt-authored — the spec defines none) to the repo: camera, two pre-rolled clips, a lower-third, an image Element, record and stream outputs on, 1080p30.
- Commit the equivalent OBS scene collection. The harness runs a drift check first: if the nbe manifest and the OBS scene describe different workloads, it refuses to compare. Comparing unlike workloads is the classic benchmark lie; make it impossible.

## Step 2: The driver

- Runs both engines on the same machine — the **§21 floor device**, the 2019 dual-GPU Intel/Radeon MacBook Pro, which AC-11 explicitly requires the comparison also run on — pinned OBS version, N runs interleaved (nbe, OBS, nbe, OBS...) to cancel thermal drift.
- Collects **AC-11's six required metrics** per run (dropped frames, CPU %, GPU %, glass-to-glass latency, take latency, recording crash safety), **plus this prompt's additions**: cold start to ready, memory RSS, and sustained-load behaviour over a 10-minute soak. Keep the two sets distinguishable in the report — one is the spec's bar, the other is ours.
- The nbe side reads its real telemetry (`/metrics`). The OBS side reads obs-websocket stats plus system sampling. No estimated numbers — every cell comes from a measurement.

## Step 3: The report

- A report template producing the comparison table **AC-11 requires be published**: median plus spread per metric, per engine, hardware and date noted.
- The `Where OBS is expected to win` section is mandatory, not optional. The harness fills the numbers; a human writes the prose. A benchmark with no honesty section is marketing.

## Step 4: Publication

- Per **AC-11's publication rule** (and its separate floor-device requirement, §21): the script, the manifests, and the report template live in the repo, public. Results land under `docs/benchmarks/` with hardware and date.
- Anyone with the same floor device should be able to run the harness and get the same table within spread.

## Step 5: Tests

1. **Drift check**: mismatched workloads refuse to run and say why.
2. **Version pinning**: the harness verifies the OBS build it was calibrated against and warns on any other.
3. **Completeness**: the generated report has **every AC-11 metric** populated from measurement — plus this prompt's additions — or it fails.
4. **Source truth**: nbe numbers come from `/metrics`; OBS numbers from obs-websocket/system sampling — the harness asserts both paths are live before running.

CI: the harness itself is not CI-gated (it needs the bench machine), but any harness code in the workspace passes `cargo check --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`.

## Constraints

- No engine changes. No estimated cells. No missing honesty section.
- `anyhow` for the binary, `thiserror` for library errors.
- Vocabulary discipline: `View`, `Element`, `Sequence`, `Item`.
