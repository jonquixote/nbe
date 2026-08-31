# Agent Prompt 04 — Basic View/Preview Compositor (crates/nbe-engine)

**Targets: SPEC v0.3.1 (`docs/spec.v0.3.md`) — Sections 7.1 (constraints), 7.2 (render model), 7.3 (element kinds), 7.9 + 7.9.1 (transitions, take latency), 7.10 (overlay level), 7.13 (frame budget), 7.14 (fallback), 10.2/10.3 (dropped frames, watchdog), 10.5 (quality profiles/degradation ladder), 11 (master clock). Prerequisites: Agent Prompts 01–03 merged (validator, control plane, render-node bridge with master clock and fallback residency).**

You are a senior Rust engineer building the `nbe` broadcast engine. **This prompt is the first GPU work in the project.** Bring up the `wgpu` compositor in `nbe-engine`: a master-clock-driven render loop producing the View and Preview targets, compositing slate, image, and solid-graphic elements, with cut and mix transitions, the fallback slate rendered and one frame away, and real dropped-frame accounting.

Read these first:

- `docs/spec.v0.3.md` — Section 7 is your contract. 7.13's non-blocking rule and 10.2's dropped-frame definition are the two you will be measured against.
- `agents/prompts/03-render-node-bridge.md` — the process, clock, and watchdog skeleton you are filling in.
- `VOCABULARY.md` — term ledger.

## Step 0: Scope discipline

Allowed now: `wgpu`, the `image` crate (PNG/JPEG decode at load time), Metal on macOS. Forbidden: video decode (Prompt 05), audio (later), camera/guest ingest (later), text/template rendering (the ticker/lower-third prompt), plugins, move-class transitions. Elements in play: `slate` items (generated), image-backed `clip` elements (held stills), and solid-fill `graphic` elements.

## Step 1: wgpu initialization

- Initialize `wgpu` (instance, adapter, device, queue), Metal backend on macOS.
- Probe the adapter and select the Section 10.5 quality profile (`potato` | `consumer` | `pro` | `reference`). Report it in telemetry as `qualityProfile`.

## Step 2: The render loop

- A frame task driven by the Prompt 03 master clock. At each master-clock frame boundary, execute Section 7.2's per-frame sequence: resolve View and Preview items/scenes, resolve elements (including scene extension and sub-scenes — sub-scenes render to texture), composite low-z to high-z at the M/E level, apply the running transition if any, composite the overlay level, render View target, render Preview target, submit, emit telemetry.
- The output MUST be frame-deterministic: the same master-clock frame and show state MUST produce identical pixels.
- All decode and texture upload happens at arm/load time, never in the render loop (Section 7.13). The render loop never touches disk, network, or WebSocket I/O.

## Step 3: Element renderers

- `slate` items: generated solid-color/test-pattern scenes — the engine can always render one with no assets.
- `clip` elements referencing `image`-kind assets: decode once at load, upload once, hold the still every frame. `clip` elements referencing video assets: skip and log as unsupported until Prompt 05.
- `graphic` elements: solid fills and simple placeholder template frames only (text/template rendering is a later prompt).
- Transforms (x/y/w/h/crop), opacity, and z-order per the manifest model.

## Step 4: Transitions — cut and mix

- `cut`: frame-boundary switch, take latency ≤ 2 frames end-to-end (Section 7.9.1, AC-17).
- `mix`: whole-frame crossfade between the outgoing and incoming bus frames over `durationFrames` (default 15). Quantized to master-clock boundaries.
- State-diff precompute (arm-time diffing) is for move-class transitions and is post-MVP — do not build it here.

## Step 5: The overlay level

- Implement the composition order `View = overlay(transition(sceneA, sceneB))`. Manifest-declared overlay elements composite after the transition and persist across it. Only image/graphic overlay elements render in this prompt; the ticker arrives later.

## Step 6: Fallback, rendered

- The Prompt 03 resident fallback slate becomes a rendered target: on `view.fallback`, watchdog fault, or decode failure, the View shows it no later than one frame after the missed deadline (AC-7). No disk read at trigger time — it is already resident.

## Step 7: Dropped frames and the watchdog, for real

- A dropped frame is a VIEW frame not submitted by its deadline (Section 10.2): count `droppedFramesTotal`. Preview misses are logged separately and never counted.
- The watchdog now watches real frames: a miss by more than 1 frame logs the fault, increments the fault counter, and activates the fallback slate if the fault affects the View (Section 10.3).

## Step 8: Degradation ladder, rung 1

- Implement rung 0 (healthy) and rung 1 (preview frame rate halves) of the Section 10.5 ladder. Under sustained missed deadlines, the preview degrades first; the View is never degraded. Report `degradationRung` in telemetry. The rest of the ladder lands as its features land.

## Step 9: Output targets

- Render to local display per Section 9.1: a wgpu surface showing View full-screen, plus a preview display (second window or split view). No encoder yet — record/stream are later prompts.

## Step 10: Tests

- Headless rendering tests (wgpu without a surface, render-to-texture with pixel readback):
  1. A known scene renders to a texture; readback pixels match expected values exactly (frame-determinism as a test, not a promise).
  2. A `view.take` issued at a known master frame changes the View output within ≤ 2 frames (the AC-17 harness).
  3. An artificially slowed render path triggers the watchdog and the fallback slate.
  4. A preview miss is logged but never increments `droppedFramesTotal`.
  5. `show.load` on the v0.3 fixture produces a renderable first frame.
- CI: the existing `rust` job covers this (macos-14 runners have Metal; headless wgpu works there). `cargo check --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` all pass.

## Constraints

- No video decode, no audio, no camera or guest ingest, no text layout, no plugins, no move-class transitions.
- Nothing but texture reads in the render loop. Everything expensive happens at arm/load time.
- `anyhow` for the binary, `thiserror` for library errors.
- Vocabulary discipline: `View`, `Element`, `Sequence`, `Item`.
