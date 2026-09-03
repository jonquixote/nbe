# Agent Prompt 04 — Basic View/Preview Compositor (crates/nbe-engine)

**Targets: SPEC v0.3.2 (`docs/spec.v0.3.md`) — Sections 5.6 (View/Preview buses), 5.9 (the render channel you already speak), 7.1 (constraints), 7.2 (render model), 7.3 (element kinds), 7.9 + 7.9.1 (transitions, take latency), 7.10 (overlay level), 7.13 (frame budget), 7.14 (fallback), 10.1.1 (telemetry ownership), 10.2/10.3 (dropped frames, watchdog), 10.5 (quality profiles, degradation ladder), 11 (master clock). Prerequisites: Agent Prompts 01, 02, 02b, 02c, 03, 03b, 03c merged.**

You are a senior Rust engineer building the `nbe` broadcast engine. **This prompt is the first GPU work in the project.** Bring up the `wgpu` compositor in `nbe-engine`: a master-clock-driven render loop producing the View and Preview targets, compositing slate, image, and solid-graphic elements, with cut and mix transitions, the fallback slate rendered and one frame away, and real dropped-frame accounting.

Read these first:

- `docs/spec.v0.3.md` — Section 7 is your contract. 7.13's non-blocking rule and 10.2's dropped-frame definition are the two you will be measured against.
- `agents/prompts/03-render-node-bridge.md` — the process, channel, clock, and watchdog you are filling in.
- `VOCABULARY.md` — term ledger. `View`, never Program. `Element`, never Layer.

## Step 0: What already exists — extend it, do not rebuild it

`crates/nbe-engine` is a running render node today. It connects, resyncs, applies directives, acknowledges them, and reports telemetry. **Your job is to put pixels behind it.** Read these modules before writing anything:

| Module | What it already does | What Prompt 04 does to it |
|---|---|---|
| `channel.rs` | WS client, render-role handshake, reconnect with backoff, `ConnectionGate` (resync gate, stale-`stateVersion` skip, seq-gap → `resyncRequest`), per-connection task abort | **Do not touch the gate.** Add nothing to the read path — the render loop is a separate task. |
| `directive.rs` | Applies `show.load` / `show.start` / `show.stop` / `view.take` / `view.cut` / `view.fallback` / `show.resync`; acks every applied directive; fallback residency at load; timed-item end events with generation tracking | Extend the handlers to drive the scene graph. Keep one ack per applied directive. |
| `state.rs` | `EngineState`: clock, fallback slate bytes (already resident in memory), `fallback_active`, applied-version tracking, `OutgoingQueue` | Add the render-side state (scene graph, targets). Keep the existing fields' meanings. |
| `clock.rs` | `MasterClock`: `STOPPED`/`RUNNING`, `frame() = floor(elapsed * house_rate)` | This is your frame source. Do not add a second clock. |
| `watchdog.rs` | `Watchdog::report_frame(frames_missed)`, fault counter, fallback trigger — **written but never called** | **You are its first caller.** See Step 7. |
| `telemetry.rs` | `build_tick` emits the §10.1.1 engine-owned shape with stub zeros | Replace the stubs with measured values. |
| `crates/nbe-protocol` | Every wire type, with a three-layer mirror audit against the spec and the TypeScript control plane | **Never define a wire type in `nbe-engine`.** If the wire must change, change it here and extend `tests/mirror.rs`. |

Two things the control plane already guarantees you, so do not re-derive them:

1. **Transitions arrive resolved.** `view.take` directives carry the resolved transition kind, duration, and audio block — the control plane expands `preset` (SPEC §16.2). Render what the payload says; never look up a preset name.
2. **References arrive resolved.** `target` holds resolved references (SPEC §5.7.1, §5.9.1). Do not parse reference syntax in the engine.

## Quality bar

Per the NBE Implementation Standards (`docs/implementation-standards.md`):

- **Schema-driven typed models (§1):** the element-kind and quality-profile enums are schema-derived. Audit `Element.kind` against the ten values in `schemas/manifest.v0.3.json`, and reuse `nbe_protocol::QualityProfile` for profiles — it is already audited against the schema enum and the spec. A hand-written second copy of either is a defect.
- **Strict CI contracts (§2):** GPU tests must fail loudly when there is no adapter, never skip silently. A headless test that no-ops on a machine without Metal is the gate that passes on any failure mode.
- **Prompt structure (§3):** Forbidden changes, new tests, and CI changes are listed below.

## Step 1: wgpu initialization and the quality probe

- Initialize `wgpu` (instance, adapter, device, queue); Metal on macOS.
- Probe the adapter and select the **effective** quality profile (SPEC §10.5). Cap it by the manifest's requested profile with `QualityProfile::capped_by` — fast hardware never promotes a `consumer` show (SPEC §10.1.1).
- Report it in `engineTelemetry` as `qualityProfile`. **The field already exists on the wire** (`nbe_protocol::EngineTelemetry::quality_profile`, and the TypeScript control plane merges effective-over-requested). Fill it in; do not invent a second path.

## Step 2: The render loop

- A frame task driven by the existing master clock. At each frame boundary run SPEC §7.2's sequence: resolve View and Preview items/scenes → resolve elements (scene extension and sub-scenes; sub-scenes render to their own texture) → composite low-z to high-z → apply the running transition → composite the overlay level → render View → render Preview → submit → report the frame to the watchdog.
- **Frame-determinism is normative:** the same master-clock frame and show state MUST produce identical pixels. This is a test (Step 10.1), not an aspiration.
- The render loop touches no disk, no network, and no WebSocket I/O (SPEC §7.13). All decode and upload happens at arm/load time. The channel task and the render task communicate through shared state, never by blocking each other — Prompt 03 structured it that way; keep it that way.
- Preview is independently rendered (SPEC §5.6), and may run at a reduced rate under degradation (Step 8). It is never the reason a View frame misses.

## Step 3: Element renderers

Scope note on vocabulary: `slate` is an **Item** kind (`Item.kind`); `clip`, `graphic`, `sceneRef`, and `group` are **Element** kinds. Both appear in `schemas/manifest.v0.3.json`; audit against it rather than against this list.

- **`slate` items:** generated solid-colour/test-pattern scenes. The engine can always render one with no assets present.
- **`clip` elements referencing `image`-kind assets:** decode once at load, upload once, hold the still every frame.
- **`clip` elements referencing video assets:** unsupported until Prompt 05. Log it as unsupported and render nothing for that element. Do **not** emit an `itemEvent: decodeError` — this is a scope boundary, not a fault, and reporting it as one would drive the item to `ERROR` in the control plane's Section 17.3 machine.
- **`graphic` elements:** solid fills and placeholder frames only. Text and template rendering belong to Prompt 07.
- Transforms (x/y/w/h/crop), opacity, and z-order per the manifest model.

## Step 4: Transitions — cut and mix

- `cut`: frame-boundary switch. Take latency ≤ 2 frames end-to-end, measured **from directive receipt to the changed View frame** (SPEC §7.9.1, AC-17).
- `mix`: whole-frame crossfade between outgoing and incoming bus frames over `durationFrames` (default 15), quantized to master-clock boundaries.
- State-diff precompute (move-class transitions) is post-MVP. Do not build it here.

## Step 5: The overlay level

Implement `View = overlay(transition(sceneA, sceneB))` (SPEC §7.10). Manifest-declared overlay elements composite after the transition and persist across it. Image and graphic overlay elements only; the ticker arrives in Prompt 07.

## Step 6: Fallback, rendered

The fallback slate is **already resident in memory** after `show.load` (`EngineState::fallback`, loaded by Prompt 03 and tested there). Prompt 04 uploads it to the GPU at load time and renders it on `view.fallback`, watchdog fault, or render failure — visible no later than one frame after the missed deadline (AC-7), with no disk read at trigger time.

## Step 7: Dropped frames and the watchdog, for real

- A dropped frame is a **View** frame not submitted by its deadline (SPEC §10.2). Count it in `droppedFramesTotal`. Preview misses are logged separately and never counted.
- **You are the watchdog's first caller.** `Watchdog::report_frame` exists in `watchdog.rs` from Prompt 03 with a fault counter and the fallback trigger path, and nothing has ever called it. Wire it into the render loop: report every frame, including on-time ones (a zero resets the streak).
- A View miss beyond the threshold logs the fault and activates the fallback slate (SPEC §10.3).
- **Report render failures upstream.** A decode or device failure that takes an item off air MUST emit the matching `itemEvent` (`decodeError`, `deviceLoss`) on the render channel — these are the frames that drive the Section 17.3 rows into `MISSING`/`ERROR`, and the control plane cannot observe them any other way (SPEC §5.9.3). The types exist in `nbe-protocol`; the engine already has an `OutgoingQueue`.

## Step 8: Degradation ladder, rung 1

Implement rung 0 (healthy) and rung 1 (preview frame rate halves) of the SPEC §10.5 ladder. Under sustained missed deadlines the preview degrades first; **the View is never degraded**. Report `degradationRung` in telemetry. The remaining rungs land with their features.

## Step 9: Output targets

Render to local display per SPEC §9.1: a wgpu surface showing View full-screen, plus a preview display (second window or split view). No encoder — record and stream are Prompts 09 and 10.

## Step 10: Tests

Headless rendering (wgpu render-to-texture with pixel readback; no surface required):

1. **Frame determinism:** a known scene renders to a texture and the read-back pixels match expected values exactly. Render the same frame twice and assert byte equality.
2. **AC-17 take latency:** a `view.take` directive delivered at a known master frame changes View output within ≤ 2 frames.
3. **Watchdog → fallback:** an artificially slowed render path trips the watchdog and the View shows the fallback slate within one frame of the missed deadline.
4. **Dropped-frame accounting:** a Preview miss is logged and does **not** increment `droppedFramesTotal`; a View miss does.
5. **First frame from the fixture:** `show.load` on `tests/fixtures/valid_show_v0.3/` produces a renderable first frame.
6. **Quality profile:** the probe result is capped by the manifest's requested profile, and appears in the emitted `engineTelemetry` frame.
7. **Enum audit:** the engine's element-kind enum matches `Element.kind` in `schemas/manifest.v0.3.json`, literal for literal.

**No-adapter policy (required):** if no wgpu adapter is available, GPU tests MUST fail with a message naming the missing adapter — they MUST NOT skip. CI runs on `macos-14`, where Metal exists; a silent skip would turn this whole suite into decoration.

## Step 11: CI

The existing `rust` job covers this crate. It must stay green with `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace`. Add to the `rust` job an explicit gate that the GPU suite actually ran — assert a non-zero count of executed render tests rather than trusting a green exit, for the same reason the control-plane job asserts a minimum test count.

## Constraints

**Forbidden changes:**

- No video decode, audio, camera or guest ingest, text layout, plugins, or move-class transitions.
- No wire-type definitions in `nbe-engine`. Wire changes go in `crates/nbe-protocol` with `tests/mirror.rs` extended, or they are not wire changes.
- No changes to the Prompt 03 channel semantics: the resync gate, per-connection `seq` expectations, stale-`stateVersion` skip, and one-ack-per-applied-directive are settled behaviour with tests. If you believe one is wrong, say so in the report — do not quietly alter it.
- No changes to Prompt 01's locked behaviours (`docs/prompt-01-definition-of-done.md`), and no edits to `schemas/manifest.v0.3.json` — schema changes are spec work.
- No second clock, and no re-resolution of transitions or references the control plane already resolved.

**Required properties:**

- Nothing but texture reads in the render loop; everything expensive happens at arm/load time.
- `anyhow` for the binary, `thiserror` for library errors.
- Vocabulary discipline: `View`, `Element`, `Sequence`, `Item`.

## Reporting obligations

The completion message must state:

1. Which SPEC sections are now implemented and which tests pin them.
2. Measured take latency for `cut` (AC-17 is ≤ 2 frames) and the frame budget headroom at the probed profile.
3. Test counts before and after, by suite — absolute numbers that match what `cargo test` prints.
4. Anything in Step 0's "already exists" table you had to change, and why.
5. Any gap you chose not to close, with the reason. Do not silently paper over one.
