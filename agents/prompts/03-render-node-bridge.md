# Agent Prompt 03 — Render-Node Command Bridge (crates/nbe-engine)

**Targets: SPEC v0.3.1 (`docs/spec.v0.3.md`) — Sections 5.3 (render role), 5.4 (envelope), 7.13 (frame budget), 7.14 (fallback slate), 10 (telemetry/health/watchdog), 11 (master clock). Prerequisites: Agent Prompt 01 (`nbe-core` validates manifests) and Agent Prompt 02 (control plane with render bridge) merged.**

You are a senior Rust engineer building the `nbe` broadcast engine. This prompt brings the `nbe-engine` crate to life as the render-node process: it connects to the control plane as a `render`-role client, receives render directives, runs the master clock, and reports health and telemetry back. **No GPU work happens in this prompt** — no wgpu, no decode, no compositing. That is Prompt 04. This prompt is the nervous system of the render node: bridge, clock, health.

Read these first:

- `docs/spec.v0.3.md` — Sections 5.3, 5.4, 7.13, 7.14, 10, 11 are your normative contract.
- `VOCABULARY.md` — term ledger. View, never Program. Element, never Layer.
- `agents/prompts/02-control-plane.md` — defines the directive protocol you consume.

## Step 1: The render-node process

- Give `crates/nbe-engine` a binary target: `src/main.rs` (binary name `nbe-engine`). Keep `src/lib.rs` for future compositor modules.
- Dependencies from the workspace: `tokio`, `tracing`, `tracing-subscriber`, `serde`, `serde_json`, `anyhow`, `thiserror`. Add `tokio-tungstenite` for the WebSocket client.
- Boot: load config (control-plane URL default `ws://127.0.0.1:8462/nbe/v0.3`, auth token, role `render`), connect with the Section 5.3 handshake (`Authorization: Bearer <token>`, `X-NBE-Role: render`).

## Step 2: Connection discipline

- The WebSocket client runs on its own tokio task. The master clock and telemetry run on theirs. Per Section 7.13, control-plane I/O MUST never block the (future) render loop — structure now so it can't later.
- Reconnect with exponential backoff on connection loss. A control-plane outage MUST NOT stop the engine: the engine keeps its last known state and keeps ticking (local survivability, Section 9.5 analog).

## Step 3: Directive intake

- Parse render directives per the Prompt 02 bridge protocol: command name, resolved target references, payload, and the `stateVersion` the directive was issued at.
- Handle at minimum: `show.load` (read the package, validate via `nbe-core`, verify fallback residency per Section 7.14 — the fallback asset MUST be resident in memory after show load), `show.start`, `show.stop` (including the quiescence truth table), `view.take`, `view.cut`, `view.fallback`.
- Directives are fire-and-forget: the engine does not block command responses. Instead, the engine reports the last applied `stateVersion` through its health/telemetry reporting (Step 5).
- Directives arriving out of order (a `stateVersion` older than the last applied) are logged and skipped.

## Step 4: The master clock

Implement SPEC Section 11:

- Monotonic system clock; epoch set by `show.start`; `masterFrame = floor(elapsedSeconds * houseFrameRate)` (30 fps default).
- Clock states `STOPPED` and `RUNNING` (Section 11.4); `HELD` and `SLAVE` are reserved.
- The clock ticks and is queryable internally. Frame production arrives in Prompt 04; here the clock already drives telemetry's `masterClockFrame`.

## Step 5: Health and telemetry reporting

- The engine emits the Section 10.1 telemetry shape to the control plane over the render channel, once per second: `ts`, `masterClockFrame`, `viewItem`, `previewItem`, `droppedFramesTotal`, `renderGpuTimeMs`, `decodeSessions`, `vramUsedMib`, `textureCacheUsedMib`, `streamState`, `recordState`, `recordSpaceMib`, `masterClockDriftMs`, `fallbackActive`, `qualityProfile`, `degradationRung`, `automationHold`.
- Fields with no implementation yet report stub values (`0`, `false`, `null`-equivalents) — but the shape MUST be complete. A telemetry consumer must never see a missing field.
- The control plane aggregates this into `GET /nbe/v0.3/status` (Section 10.4); the engine's job is only to report.

## Step 6: Watchdog and fallback readiness

- Implement the Section 10.3 watchdog in skeleton form: a deadline checker with a fault counter, logging, and the fallback-slate trigger path. With no frames in flight yet, the watchdog validates the mechanism, not the pixels.
- `view.fallback` and watchdog faults both route to the resident fallback slate (loaded at `show.load`). Fallback MUST be reachable without any disk read at trigger time.

## Step 7: Tests

- Unit: master-clock math (known elapsed times → exact frame numbers at 30 fps; `STOPPED` never advances), directive parsing, out-of-order `stateVersion` rejection, fallback residency check fails loudly when the asset is missing.
- Integration: boot a test WebSocket server speaking the Prompt 02 bridge protocol, connect the engine, send `show.load` → `show.start` → `view.take`, assert ordered application and telemetry with an advancing `masterClockFrame`.
- No new CI job needed: the existing `rust` job covers `nbe-engine` as a workspace member. Confirm `cargo check --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace` all pass.

## Constraints

- No `wgpu`, no decode, no pixels. Bridge, clock, health only.
- `anyhow` for the binary, `thiserror` for library errors.
- The WebSocket client task must be provably non-blocking relative to the clock/telemetry tasks.
- Vocabulary discipline: `View`, `Element`, `Sequence`, `Item`.
