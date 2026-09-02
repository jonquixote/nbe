# Agent Prompt 03 — Render-Node Command Bridge (crates/nbe-engine)

**Targets: SPEC v0.3.1 (`docs/spec.v0.3.md`) — Sections 5.3 (render role), 5.4 (envelope), 7.13 (frame budget), 7.14 (fallback slate), 10 (telemetry/health/watchdog), 11 (master clock). Prerequisites: Agent Prompt 01 (`nbe-core` validates manifests) and Agent Prompt 02 (control plane with render bridge) merged.**

You are a senior Rust engineer building the `nbe` broadcast engine. This prompt brings the `nbe-engine` crate to life as the render-node process: it connects to the control plane as a `render`-role client, receives render directives, runs the master clock, and reports health and telemetry back. **No GPU work happens in this prompt** — no wgpu, no decode, no compositing. That is Prompt 04. This prompt is the nervous system of the render node: bridge, clock, health.

Read these first:

- `docs/spec.v0.3.md` — Sections 5.3, 5.4, 7.13, 7.14, 10, 11 are your normative contract.
- `VOCABULARY.md` — term ledger. View, never Program. Element, never Layer.
- `agents/prompts/02-control-plane.md` — defines the directive protocol you consume.
- `agents/prompts/02a-architecture-addendum.md` — normative architecture decisions and semantics that must be pinned before proceeding.

## Quality bar

This prompt complies with the NBE Implementation Standards (`docs/implementation-standards.md`). Specifically:

- **Schema-driven typed models:** This prompt introduces the Rust render-node bridge (directive frames, engine-frame schemas, and the render-node state machine). These must be round-trip tested and enum-audited against the Section 16 command/error tables and the pinned directive/engine frame definitions (see Standards §1). Concretely: for every directive and engine frame, define a Rust type, serialize a sample to JSON, deserialize it back, and assert equality; for any enums (e.g., frame kinds, error codes), list every value in the spec, map each to a Rust variant, and verify no omissions or mismatches.
- **Strict CI contracts:** Any new binary or observable behaviour must have an exact CI gate (exit codes, key strings, behavioural invariants) (see Standards §2).
- **Prompt structure compliance:** This prompt explicitly lists Forbidden changes, New tests required, and CI changes required (see Standards §3).

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

## Step 3a: Pin the wire frame schemas (addendum §1.1)

These frames are a separate protocol layered on the same WebSocket connection — they are NOT the Section 5.4 command envelope. Define Rust types (serde) for each and round-trip test serialize/deserialize against JSON.

- **Server → engine directive frame:** `{ "v": "0.3", "kind": "directive", "seq": 91, "stateVersion": 413, "command": "view.take", "target": {}, "payload": {} }`. `target` is the resolved references object, `payload` is command-specific, `seq` is a monotonic per-connection sequence.
- **Engine → server frames (accept only from `render`-role sessions):**
  - `engineTelemetry` — the Section 10.1 shape (see Step 5 for ownership).
  - `appliedStateVersion` — `{ v, kind:"appliedStateVersion", stateVersion }`: the engine reports the last directive `stateVersion` it applied.
  - `itemEvent` — `{ v, kind:"itemEvent", itemRef, event: "end"|"decodeError"|"deviceLoss"|"missing", detail? }`; these make the `PLAYING -> DONE` and `-> MISSING/ERROR` rows of the Section 17.3 table reachable.
- Enum-audit every frame kind and every error code against the Section 16 registry; map each to a Rust variant; fail on omission or mismatch.

## Step 4: The master clock

Implement SPEC Section 11:

- Monotonic system clock; epoch set by `show.start`; `masterFrame = floor(elapsedSeconds * houseFrameRate)` (30 fps default).
- Clock states `STOPPED` and `RUNNING` (Section 11.4); `HELD` and `SLAVE` are reserved.
- The clock ticks and is queryable internally. Frame production arrives in Prompt 04; here the clock already drives telemetry's `masterClockFrame`.

## Step 5: Health and telemetry reporting (addendum §1.2 ownership)

- The engine is authoritative for the performance/clock fields: `masterClockFrame`, `droppedFramesTotal`, `renderGpuTimeMs`, `decodeSessions`, `vramUsedMib`, `textureCacheUsedMib`, `masterClockDriftMs`, `fallbackActive`, `recordSpaceMib`, `degradationRung`. Emit exactly these in the `engineTelemetry` frame at 1 Hz.
- The control plane owns the show-state fields (`viewItem`, `previewItem`, `automationHold`, `qualityProfile`, commanded `streamState`/`recordState`) and merges the engine report with a staleness threshold: when the control plane has not heard from the engine within the threshold it reports stub values for the engine-owned fields and sets `engineConnected: false`. The emitted shape is always complete — a telemetry consumer MUST never see a missing field.
- The engine's job is only to report its authoritative fields; aggregation into `GET /nbe/v0.3/status` (Section 10.4) is the control plane's.

## Step 6: Watchdog and fallback readiness

- Implement the Section 10.3 watchdog in skeleton form: a deadline checker with a fault counter, logging, and the fallback-slate trigger path. With no frames in flight yet, the watchdog validates the mechanism, not the pixels.
- `view.fallback` and watchdog faults both route to the resident fallback slate (loaded at `show.load`). Fallback MUST be reachable without any disk read at trigger time.

## Step 7: Tests

- Unit: master-clock math (known elapsed times → exact frame numbers at 30 fps; `STOPPED` never advances), directive parsing, out-of-order `stateVersion` rejection, fallback residency check fails loudly when the asset is missing, frame round-trip (serialize sample → deserialize → assert equality) for every directive and engine frame, and enum audit of frame kinds / error codes.
- **Render-node state machine (total coverage):** parse the relevant state-transition table from `docs/spec.v0.3.md` for the render node's states (`IDLE`, `ARMED`, `VIEW`, `TRANSITIONING`) — assert EVERY legal transition executes and EVERY illegal transition is rejected. This turns "sampled illegal transitions" into complete coverage, as in Prompt 02.
- **Telemetry staleness test:** simulate a stale engine connection (no engine frame within the threshold) and verify the control plane emits stub values for the engine-owned fields plus `engineConnected: false`; then send a fresh frame and verify real values + `engineConnected: true`.
- Integration: boot a test WebSocket server speaking the Prompt 02 bridge protocol (directive + engine frames), connect the engine, send `show.load` → `show.start` → `view.take`, assert ordered application and telemetry with an advancing `masterClockFrame`, and that the mock bridge records directives in order with correct `stateVersion`s.
- No new CI job needed: the existing `rust` job covers `nbe-engine` as a workspace member. Confirm `cargo check --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace` all pass.

## Step 8: CI expectations (lock these)

The `rust` job MUST stay green; the following gates are explicit and MUST NOT relax:

1. `cargo fmt --all -- --check` passes.
2. `cargo clippy --workspace --all-targets -- -D warnings` passes.
3. `cargo test --workspace` passes.
4. The new render-node tests — frame round-trip, directive/engine enum audit, render-node state-machine total coverage, telemetry staleness — all pass.
5. If the mock bridge is exercised in this prompt, the test hook confirms directives are recorded in order with correct `stateVersion`s.

## Step 9: Spec gaps — explicit disposition (addendum §3)

Work through the addendum's spec gaps and state, in the completion message, whether this prompt:

- Implements a workaround (and documents it), or
- Flags it as **not in scope for this prompt; requires spec revision.** Specifically weigh:
  - Nested `sequenceRef` resolution (no registry in the v0.3 manifest) — out of scope: a schema change is spec work.
  - Rate-limit error code (Section 16 has no flood-protection failure mode).
  - `crates/nbe-protocol` status: mirror the wire protocol with a Rust enum audit, or flag for deletion.

Do not silently paper over any gap.

## Step 10: Verification

Run `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace`; simulate the CI gates exactly as they run in the workflow. Report results in the completion message.

## Constraints

- No `wgpu`, no decode, no pixels. Bridge, clock, health only.
- `anyhow` for the binary, `thiserror` for library errors.
- The WebSocket client task must be provably non-blocking relative to the clock/telemetry tasks.
- Vocabulary discipline: `View`, `Element`, `Sequence`, `Item`.
- All architecture decisions and semantics in `02a-architecture-addendum.md` are normative for this prompt.
