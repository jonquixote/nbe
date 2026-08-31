# Agent Prompt 11 — Performance Watchdog & Fallback (crates/nbe-engine)

**Targets: SPEC v0.3.1 (`docs/spec.v0.3.md`) — Sections 9.6 (GPU oversubscription fallback), 10.1 (telemetry), 10.4 (structured logging), 20.5 (performance acceptance). Prerequisites: Agent Prompts 01–10 merged (compositor, telemetry, and outputs all live).**

You are a senior Rust engineer building the `nbe` broadcast engine. This prompt builds the watchdog: the engine notices it is oversubscribed, sheds the least-important work before the show stutters, says so loudly, and puts the work back when there is headroom.

Read these first:

- `docs/spec.v0.3.md` — Section 9.6 is your contract. Section 20.5 is the acceptance bar you protect.
- `agents/prompts/01-foundation.md` (metrics) and `03-compositor.md` (telemetry) — the signals you consume.
- `VOCABULARY.md` — term ledger.

## Step 0: Scope discipline

Allowed now: detection, fallback, reporting, and restoration for GPU oversubscription. Forbidden: changes to the compositor's per-frame design (Prompt 03 stands), any watchdog work on the render thread, and any auto-fallback that touches the primary program layer.

## Step 1: Detection

- The watchdog consumes the existing telemetry: frame time vs budget, `droppedFramesTotal` slope, GPU queue depth, memory ceiling. It engages on sustained evidence (a window of repeated drops), never on a single slow frame.
- It samples off the render thread; the render thread only publishes the numbers it already has.

## Step 2: Fallback tiers

- On engagement, shed compositor load in priority order per Section 9.6: drop the lowest-priority optional Element first, then the next, as far as needed. The primary program layer is never a fallback candidate.
- Each tier is logged loudly with a reason code and the measured numbers that caused it (`droppedFramesTotal` plus reason, per Section 10.4).

## Step 3: Reporting

- The degraded state is visible in `show.getState` and on the metrics endpoint: which Elements were shed, at what tier, and why.
- The operator sees the warning without reading logs — state and telemetry both carry it.

## Step 4: Restoration

- When headroom returns and holds through a cooldown window, restore shed Elements in reverse order. Hysteresis is mandatory: no flapping between tiers on a borderline machine.
- Restoration is logged and visible the same way as shedding.

## Step 5: Tests

1. **Detection**: a synthetic GPU overload engages the watchdog within a bounded number of frames; a single slow frame does not.
2. **Fallback**: the watchdog sheds the lowest-priority optional Element first; the primary program layer is never shed; frame time recovers after the drop.
3. **Reporting**: state and logs both show the degradation with reason codes and the measured numbers.
4. **Restoration**: after load clears and holds, shed Elements return in reverse order; a borderline load does not flap.
5. **Guardrail**: the watchdog adds zero work to the render thread — it reads published telemetry only.

CI: the existing `rust` job on macos-14 covers this. `cargo check --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` all pass.

## Constraints

- No render-thread work, no primary-layer shedding, no silent degradation — the machine degrades gracefully and tells on itself.
- `anyhow` for the binary, `thiserror` for library errors.
- Vocabulary discipline: `View`, `Element`, `Sequence`, `Item`.
