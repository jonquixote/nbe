# Agent Prompt 03 — Render-Node Command Bridge (crates/nbe-engine)

**Targets: SPEC v0.3.2 (`docs/spec.v0.3.md`) — Sections 5.3 (render role), 5.4 (envelope), 5.9 (the render channel — directive frame, engine frames, `seq`, resync, quiescence ack), 7.13 (frame budget), 7.14 (fallback slate), 10.1.1 (telemetry ownership), 10.3 (watchdog), 11 (master clock). Prerequisites: Agent Prompt 01 (`nbe-core` validates manifests), Agent Prompt 02 (control plane), and Agent Prompt 02c (render-channel control-plane half — see Step 0) merged.**

> The render channel was promoted from `02a-architecture-addendum.md` into the spec at v0.3.2 (§5.9). Where this prompt and the addendum differ, **the spec wins** — the addendum is now a historical record of how those decisions were reached.

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

## Step 1a: The wire types already exist — use them, do not redefine them

`crates/nbe-protocol` is the Rust mirror of the wire protocol (SPEC §5.4, §5.4.1, §5.9, §16). It is populated and audited; `nbe-engine` depends on it and **MUST NOT** define its own envelope, error, directive, or engine-frame types.

What it gives you:

| Type | Covers |
|---|---|
| `Envelope`, `Response`, `ErrorBody`, `ErrorCode` | SPEC §5.4 command envelope and responses; the complete §16 error registry including `E_RATE_LIMITED` |
| `PushFrame` | §5.4.1 `stateChange` / `telemetry` |
| `DirectiveFrame`, `DirectiveKind` | §5.9.1, with `seq` semantics documented on the type |
| `EngineFrame`, `EngineTelemetry`, `ItemEvent`, `ResyncReason` | §5.9.3 — the four frames the engine sends |
| `command::ALL`, `command::RESYNC`, `command::is_known` | the §16 command surface; `show.resync` is deliberately excluded from `ALL` because no client may issue it |
| `Role`, `PROTOCOL_VERSION`, `WS_PATH`, `DEFAULT_PORT` | §5.3 connection constants |

`crates/nbe-protocol/tests/mirror.rs` already enforces the Standards §1 obligations for these types in three layers: serde round-trips, an audit against the spec's Section 16 tables, and a **direct audit against `packages/control-plane/src/protocol.ts`** so a command or code added on one side and forgotten on the other fails `cargo test`. Step 7's "frame round-trip and enum audit" requirement is therefore already satisfied for the mirrored types — **extend `mirror.rs` when you add anything to the wire, and do not duplicate those tests in `nbe-engine`.** What remains for this prompt is the engine's *behaviour*: clock math, directive application ordering, resync handling, watchdog.

Add the dependency: `nbe-protocol = { path = "../nbe-protocol" }`.

## Step 2: Connection discipline

- The WebSocket client runs on its own tokio task. The master clock and telemetry run on theirs. Per Section 7.13, control-plane I/O MUST never block the (future) render loop — structure now so it can't later.
- Reconnect with exponential backoff on connection loss. A control-plane outage MUST NOT stop the engine: the engine keeps its last known state and keeps ticking (local survivability, Section 9.5 analog).

## Step 0: Prerequisite — the control-plane half must exist first

**Do not start this prompt until Prompt 02c has landed.** Steps 2a, 3a, and 4a below consume three control-plane behaviours that do not exist in `packages/control-plane` as of this writing:

| Needed | Current state |
|---|---|
| `show.resync` directive on every render connect | Not implemented; not a Section 16 command — it is a directive-only command name (SPEC §5.9.4). |
| `appliedStateVersion` handled and awaited | `AppliedStateVersionFrameSchema` exists in `src/protocol.ts` and parses, but `src/server.ts` has no branch for it — the frame is silently discarded. |
| `show.stop` awaiting that ack | `waitForGrace` in `src/commands/show.ts` is an optional test seam that production never sets, so the 2-second window elapses instantly. |

Verify each of the three before starting: connect a `render`-role session and confirm a `show.resync` directive arrives; send an `appliedStateVersion` frame and confirm the control plane records it. If they are absent, stop and report — building the engine half against a control plane that cannot answer produces a green test suite and a dead link, which is exactly how the Prompt 02 bridge blockers happened twice.

Normative references for all three: SPEC v0.3.2 §5.9 (render channel), §5.9.4 (reconnect resync), §5.9.5 (quiescence acknowledgement).

## Step 2a: Reconnect resync (NEW — mandatory)

A bounded, fire-and-forget bridge plus an engine that reconnects means directives issued during the outage are lost and the engine could come back on-air showing the wrong item. SPEC §5.9.4 is the contract; this prompt implements the engine half:

1. On **every** render-role connection (initial connect AND reconnect), the control plane sends a `show.resync` directive containing a full snapshot: current `viewItem`, `previewItem`, item/scene states, visible overlays, `automationHold`, and the issuing `stateVersion`. The engine applies this snapshot and confirms with `appliedStateVersion`.
2. The render node, on reconnect, MUST NOT assume its previous state is still current — it waits for `show.resync` before trusting any subsequent directive. Between reconnect and resync it holds the last applied frame.
3. Test: drop the connection mid-show, reconnect, assert the engine receives `show.resync` and applies it; assert a directive issued during the outage is NOT replayed and the resynced `stateVersion` is the latest.

## Step 3: Directive intake

- Parse render directives per the Prompt 02 bridge protocol: command name, resolved target references, payload, and the `stateVersion` the directive was issued at.
- Handle at minimum: `show.load` (read the package, verify fallback residency per Section 7.14 — the fallback asset MUST be resident in memory after show load; **do not** re-run full manifest validation here, that is `nbe-core`/`nbe-preflight`'s job — the control plane already validated it, and only fail if the on-disk asset it needs is missing), `show.start`, `show.stop` (execute the `record.stop`/`stream.stop` directives the control plane sends as part of quiescence — do NOT re-derive the Section 16.1 truth table, that is control-plane policy), `view.take`, `view.cut`, `view.fallback`.
- **Validation authority:** the control plane is authoritative for manifest validity (it ran `nbe-preflight`). The engine's own checks are limited to what it needs to operate: fallback residency and opening the referenced assets. If the control plane and engine disagree (asset validated but cannot be opened), the ENGINE is fatal — it logs `E_DECODE`/`E_ENGINE` on its side and reports via telemetry; the control plane treats that as authoritative — a package that validates statically but cannot be decoded at runtime is not airworthy, and only the engine can know that.
- Directives are fire-and-forget: the engine does not block command responses. Instead, the engine reports the last applied `stateVersion` through Step 3a's `appliedStateVersion` frame (the ack that makes Step 4's `show.stop` grace window real).
- Directives arriving out of order (a `stateVersion` older than the last applied) are logged and skipped.

## Step 3a: Pin the wire frame schemas (addendum §1.1)

These frames are a separate protocol layered on the same WebSocket connection — they are NOT the Section 5.4 command envelope. **The Rust types already exist in `crates/nbe-protocol` (Step 1a); this section documents the contract they implement.** Read it to know what the bytes mean, not to re-declare them.

- **Server → engine directive frame:** `{ "v": "0.3", "kind": "directive", "seq": 91, "stateVersion": 413, "command": "view.take", "target": {}, "payload": {} }`. `target` is the resolved references object, `payload` is command-specific.
- **`seq` semantics (pin these, do not leave ambiguous):**
  - `seq` is a **per-connection** monotonic counter maintained by the control plane on each `render`-role session, starting at `0` when that connection is established.
  - It resets to `0` on every (re)connect. The render node MUST treat `seq` as a continuity check **within a single connection only**. A fresh connection starting at `seq 0` is a "joined here" marker, NOT a "lost N directives" signal.
- The render node detects loss by `seq` discontinuity within one connection and by the `stateVersion` gap; on any discontinuity it MUST NOT guess — it sends `resyncRequest` and waits (see Step 2a).
  - **`resyncRequest`** — `{ v, kind: "resyncRequest", reason: "seqGap" | "reconnect" | "internal" }` (SPEC §5.9.3). This is the engine→server frame that makes "request a resync" actionable; without it the requirement is unimplementable. The control plane answers with a fresh `show.resync` on that connection.
- **Engine → server frames (accept only from `render`-role sessions):**
  - `engineTelemetry` — the Section 10.1 shape (see Step 5 for ownership).
  - `appliedStateVersion` — `{ v, kind:"appliedStateVersion", stateVersion }`: the engine reports the last directive `stateVersion` it applied. This is the signal the control plane awaits to know the show is quiesced; **without it, `show.stop`'s 2-second window is never satisfied in production.** The engine MUST send one after applying a quiesce-triggering directive (`record.stop`, `stream.stop`, `show.stop`) once it confirms the outputs have actually stopped.
  - `itemEvent` — `{ v, kind:"itemEvent", itemRef, event: "end"|"decodeError"|"deviceLoss"|"missing", detail? }`; these make the `PLAYING -> DONE` and `-> MISSING/ERROR` rows of the Section 17.3 table reachable.
- Enum-audit every frame kind and every error code against the Section 16 registry; map each to a Rust variant; fail on omission or mismatch.

## Step 4: The master clock

Implement SPEC Section 11:

- Monotonic system clock; epoch set by `show.start`; `masterFrame = floor(elapsedSeconds * houseFrameRate)` (30 fps default).
- Clock states `STOPPED` and `RUNNING` (Section 11.4); `HELD` and `SLAVE` are reserved.
- The clock ticks and is queryable internally. Frame production arrives in Prompt 04; here the clock already drives telemetry's `masterClockFrame`.
- The render node's own state machine (this prompt's responsibility) is the **clock finite-state machine** — `STOPPED`, `RUNNING` (with `HELD`/`SLAVE` reserved) — plus directive-application state (last applied `stateVersion`, last applied directive seq). This is NOT `IDLE`/`ARMED`/`VIEW`/`TRANSITIONING`, which are control-plane **scene** states (Section 17.2) owned by Prompt 02. Do not conflate them.

## Step 4a: show.stop grace window (depends on the appliedStateVersion ack)

`show.stop`'s 2-second window is only real if the engine confirms it quiesced. Pin it:

1. Control plane sends `record.stop` / `stream.stop` directives (per Prompt 02's quiescence decision, engine just executes).
2. Engine stops the outputs and sends `appliedStateVersion` for the stop directive's `stateVersion`.
3. Control plane awaits that `appliedStateVersion`. If it arrives within 2 s, the stop is graceful; if not, control plane force-stops and logs the exact warning `show.stop: graceful output shutdown exceeded 2 s; force-stopping outputs`.
4. The engine MUST NOT send `appliedStateVersion` for a quiesce directive until the outputs are actually quiesced — an early ack would defeat the window.

## Step 5: Health and telemetry reporting (addendum §1.2 ownership)

- The engine is authoritative for the performance/clock fields: `masterClockFrame`, `droppedFramesTotal`, `renderGpuTimeMs`, `decodeSessions`, `vramUsedMib`, `textureCacheUsedMib`, `masterClockDriftMs`, `fallbackActive`, `recordSpaceMib`, `degradationRung`. Emit exactly these in the `engineTelemetry` frame at 1 Hz.
- The control plane owns the show-state fields (`viewItem`, `previewItem`, `automationHold`, `qualityProfile`, commanded `streamState`/`recordState`) and merges the engine report with a staleness threshold: when the control plane has not heard from the engine within the threshold it reports stub values for the engine-owned fields and sets `engineConnected: false`. The emitted shape is always complete — a telemetry consumer MUST never see a missing field.
- The engine's job is only to report its authoritative fields; aggregation into `GET /nbe/v0.3/status` (Section 10.4) is the control plane's.

## Step 6: Watchdog and fallback readiness

- Implement the Section 10.3 watchdog in skeleton form: a deadline checker with a fault counter, logging, and the fallback-slate trigger path. With no frames in flight yet, the watchdog validates the mechanism, not the pixels.
- `view.fallback` and watchdog faults both route to the resident fallback slate (loaded at `show.load`). Fallback MUST be reachable without any disk read at trigger time.

## Step 7: Tests

- Unit: master-clock math (known elapsed times → exact frame numbers at 30 fps; `STOPPED` never advances), directive parsing, out-of-order `stateVersion` rejection, fallback residency check fails loudly when the asset is missing, frame round-trip (serialize sample → deserialize → assert equality) for every directive and engine frame, and enum audit of frame kinds / error codes.
- **Clock finite-state machine (total coverage):** parse the Section 11.4 clock states (`STOPPED`, `RUNNING`; `HELD`/`SLAVE` reserved) from `docs/spec.v0.3.md` — assert EVERY legal transition executes and EVERY illegal transition is rejected. (Do NOT test `IDLE`/`ARMED`/`VIEW`/`TRANSITIONING` here; those are control-plane scene states, out of scope for the Rust engine.)
- **Reconnect resync test:** drop the connection mid-show, reconnect, assert the engine receives `show.resync`, applies it, and confirms with `appliedStateVersion` matching the latest `stateVersion`; assert a directive issued during the outage is not replayed.
- **AppliedStateVersion ack test:** issue a quiesce directive and assert the control plane only considers the engine quiesced after it sends `appliedStateVersion` for that directive's `stateVersion` — and that the engine does NOT ack before outputs are actually stopped.
- Integration: boot a test WebSocket server speaking the Prompt 02 bridge protocol (directive + engine frames), connect the engine, send `show.load` → `show.start` → `view.take`, assert ordered application and telemetry with an advancing `masterClockFrame`.
- **Telemetry staleness:** the merge (engine-owned fields vs. stubs + `engineConnected: false`) is implemented in the TypeScript control plane (`packages/control-plane/src/telemetry.ts`), NOT in this Rust prompt. That test belongs in a Prompt-02 follow-up; if this prompt must still verify the contract, declare it explicitly as a cross-language integration test with the TypeScript owner named (Prompt 02) and the control plane side asserting it.
- No new CI job needed: the existing `rust` job covers `nbe-engine` as a workspace member. Confirm `cargo check --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace` all pass.

## Step 8: CI expectations (lock these)

The `rust` job MUST stay green; the following gates are explicit and MUST NOT relax:

1. `cargo fmt --all -- --check` passes.
2. `cargo clippy --workspace --all-targets -- -D warnings` passes.
3. `cargo test --workspace` passes.
4. The new render-node tests — frame round-trip, directive/engine enum audit, clock finite-state-machine total coverage, reconnect resync, appliedStateVersion ack — all pass.
5. The mock-bridge ordering guarantee (directives recorded in order with correct `stateVersion`s) is a **TypeScript** control-plane guarantee from Prompt 02; it is enforced by the control-plane CI job (`npm test`), not the `rust` job. Do not add it here — if it must be cross-validated, declare it explicitly as a cross-language integration gate owned by Prompt 02.

## Step 9: Spec gaps — explicit disposition (addendum §3)

Work through the addendum's spec gaps and state, in the completion message, whether this prompt:

- Implements a workaround (and documents it), or
- Flags it as **not in scope for this prompt; requires spec revision.** Specifically weigh:
  - Nested `sequenceRef` resolution (no registry in the v0.3 manifest) — out of scope: a schema change is spec work, deferred to v0.4. SPEC §16.4 records the disposition; do not invent a nesting convention.
  - `crates/nbe-protocol` status — **DECIDED: mirror.** See Step 1a; the crate is populated and audited. Do not define frame types in `nbe-engine`.

Gaps closed since this prompt was written, listed so they are not re-litigated:

  - **Rate-limit error code — CLOSED.** `E_RATE_LIMITED` is in the SPEC v0.3.2 §16 registry and implemented by Prompt 02c. The enum audit in Step 3a must include it.
  - **Render channel, resync, and quiescence ack — CLOSED.** Promoted into SPEC §5.9 and implemented control-plane-side by Prompt 02c (see Step 0).

Do not silently paper over any gap.

## Step 10: Verification

Run `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace`; simulate the CI gates exactly as they run in the workflow. Report results in the completion message.

## Constraints

- No `wgpu`, no decode, no pixels. Bridge, clock, health only.
- `anyhow` for the binary, `thiserror` for library errors.
- The WebSocket client task must be provably non-blocking relative to the clock/telemetry tasks.
- Vocabulary discipline: `View`, `Element`, `Sequence`, `Item`.
- All architecture decisions and semantics in `02a-architecture-addendum.md` are normative for this prompt.
