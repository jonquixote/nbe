# Agent Prompt 05 — Video Decode Integration (crates/nbe-engine + nbe-preflight)

**Targets: SPEC v0.3.2 (`docs/spec.v0.3.md`) — Sections 6.2 (mezzanine format), 7.2 (render model), 7.13 (frame budget), 12 (deterministic loops — the whole section), 18 (cadence), 19 (preflight details), 24 (decode-session risk), AC-3/AC-4 (preflight), AC-9 (loop wrap), AC-21 (loop budget math). Prerequisites: Agent Prompts 01–04 merged, including the 02b/02c and 03b/03c/04b follow-ups.**

You are a senior Rust engineer building the `nbe` broadcast engine. This prompt integrates hardware video decode: video-backed `clip` elements play, `videoLoop` elements loop deterministically, and the loop cache becomes real. The same decode infrastructure deepens `nbe-preflight` so the decode-based checks from SPEC Section 19 finally run.

Read these first:

- `docs/spec.v0.3.md` — Section 12 is the heart of this prompt. Sections 6.2, 7.13, 18, and the AC-9/AC-21 acceptance criteria are your tests.
- `docs/implementation-standards.md` — in particular §2a (falsification) and §2b (test counts), which are how this prompt will be reviewed.
- `agents/prompts/04-basic-compositor.md` — the compositor you are feeding textures into.
- `VOCABULARY.md` — term ledger. `View`, never Program. `Element`, never Layer.

## Step 0: What already exists — extend it, do not rebuild it

`crates/nbe-engine` renders today: it resolves scenes from show state, composites layers, runs cut and mix transitions on master-clock boundaries, counts dropped frames against real deadlines, degrades the preview under pressure, and shows a decoded fallback slate. **Your job is to make video one of the things it can draw.**

| Module | What it already does | What Prompt 05 does to it |
|---|---|---|
| `scene.rs` | `PackageIndex` (items → scenes → elements), image decode at load, `LayerSource::{Solid, Image}`, `Transition` with frame-quantized progress | **This is your integration point.** Add a video layer source; keep `LayerSource` the single vocabulary of what a layer can be. |
| `render.rs` | Textured-quad pipeline, per-layer rect/opacity, `render_frame(frame, deadline)`, drop counting, watchdog feeding, rung 0/1 | Video textures become another `draw_for` case. **Do not add a second pipeline** and do not put decode in `render_frame`. |
| `gpu.rs` | Device/queue, `make_texture`, `upload_rgba`, `readback_rgba`, adapter probe + `capped_by` | Zero-copy interop lives here, next to the existing texture helpers. |
| `directive.rs` | `show.load` indexes the package and decodes images; take arms a transition at the next boundary; every applied directive acks | Arm-time preload hooks here. Keep one ack per applied directive. |
| `state.rs` | Package index, generation counter, view/preview item, transition, drop and preview-miss counters, rung, quality profile | Add decode-side state. Keep existing field meanings. |
| `watchdog.rs` | Fed measured lateness by the render loop | Unchanged. Do not feed it constants. |
| `crates/nbe-protocol` | All wire types, three-layer mirror audit | `decodeSessions` already exists in `EngineTelemetry`. **Never define a wire type in the engine**; if the wire must change, change it here and extend `tests/mirror.rs`. |

### The scope boundary you are inheriting

Prompt 04 draws a deliberate line, and **Prompt 05 owns erasing it**: a `clip` or `videoLoop` element whose asset is video is currently skipped with an `info!` log and renders nothing. It is explicitly **not** reported as `itemEvent: decodeError`, because a scope boundary is not a fault and reporting one would drive the item to `ERROR` in the control plane's §17.3 machine.

When video decode lands, that inverts:

1. A video element that decodes is drawn. The skip path and its log are deleted, not left dormant.
2. A video element that genuinely **fails** to decode IS a fault, and now MUST emit `itemEvent: decodeError` on the render channel (SPEC §5.9.3) — this is the first prompt where that frame has a real source.
3. `tests/prompt04.rs::the_v0_3_fixture_produces_a_renderable_first_frame` asserts the fixture's video element renders black *because* video is unsupported. That assertion becomes wrong the moment you implement this prompt. **Update it deliberately** and say so in your report; do not delete it to make the suite pass.

## Quality bar

Per the NBE Implementation Standards:

- **Schema-driven typed models (§1):** the loop-cache typed model and the frame-selection/pulldown enums must be round-trip tested and enum-audited against the `LoopMetadata` and `Pulldown` definitions in `schemas/manifest.v0.3.json`.
- **Strict CI contracts (§2):** exact gates — exit codes by value, required strings, behavioural invariants. GPU/decode tests fail loudly when hardware is absent; they never skip silently.
- **Falsification (§2a):** required. See Reporting obligations.
- **Test counts (§2b):** quote the CI summary lines; do not tally by hand.

## Step 1: Scope discipline

Allowed: VideoToolbox hardware decode on macOS for H.264 clips and ProRes 4444 alpha loops; PNG-sequence alpha as fallback. Forbidden: audio (Prompt 06), camera/guest ingest, the encoder, HAP, HEVC, and any software decode in the live path. The engine never repairs media live — out-of-spec media was already rejected by preflight (Assumption 3).

Not in this prompt, and not yours to pick up: the overlay level (deferred to Prompt 07) and the display surface (deferred to Prompt 09).

## Step 2: Decode core

- A `decode` module in `nbe-engine`: a VideoToolbox session decoding H.264/ProRes, producing frames as GPU textures. IOSurface-backed `CVPixelBuffer` → Metal texture for zero-copy handoff into `wgpu` (hal interop). No CPU readback in the live path.
- Frame delivery carries a presentation index, not just pixels: the compositor selects frames by index.

## Step 3: Master-clock frame selection

- Clips do not play on their own clock. The frame shown at master frame `F` is clip frame `F - itemStartFrame` — a pure function of the show clock, which is what makes the existing determinism test meaningful once video is in the picture.
- Loop elements: `sourceIndex = (F - t0) mod P` per §12.1. No restart events; a loop boundary is computationally indistinguishable from any other frame.

## Step 4: Arm-time preload

At arm time: open the file, seek to the first frame, decode it, upload the texture, mark the item ready. The render loop MUST never wait on any of this at take time (SPEC §7.13). The existing `render_frame` does no I/O — keep it that way.

## Step 5: The loop cache

Follow Section 12 exactly, in the mandated order:

1. **Format accounting first** (§26 sequencing): compute `frameCostMiB` from the selected texture format and `maxFramesByBudget` from the effective budget (§12.5, §12.6). "NV12" means two-plane YUV (`R8Unorm` + `Rg8Unorm`) with shader-side BT.709 conversion per the v0.2.1 errata.
2. **VRAM ring buffer** for resident loops (§12.7): `textureSlot = sourceIndex mod P`; no decoder restart at wrap.
3. **Double-buffered read-ahead** for streamed long loops (§12.8): minimum read-ahead `max(2 × GOP length, 60 frames)`; wrap never blocks the render thread; read-ahead failure falls back to the frozen frame, or to the fallback slate if live.
4. **Budgets enforced**: per-loop and total cache budgets, with the Apple unified-memory clamp against `recommendedMaxWorkingSetSize` (§12.6).

## Step 6: Decode-session budget

- Cap simultaneous active decode sessions; reuse sessions; evict idle ones. SPEC §24 names VideoToolbox decode-session limits as a high-severity risk with exactly this mitigation.
- Report `decodeSessions` in telemetry — the field already exists on the wire and currently reports a hardcoded `0`.

## Step 7: Preflight deepening

`nbe-preflight` gains the decode-based checks from SPEC §19, powered by the same decoder:

1. First- and last-frame decode of every video/alpha asset.
2. CFR verification (`cfr: false` for VFR — AC-3).
3. Resolution and house frame rate.
4. `durationFrames` versus `expectedDurationFrames`.
5. Loop period matches `loop.periodFrames`.
6. Alpha presence for alpha assets.

Add `scripts/generate-fixtures.sh`: ffmpeg synthesizes tiny test clips — a valid 1080p30 CFR clip, a VFR clip (must fail), a wrong-resolution clip (must fail), a 12 fps animation for cadence verification. Seeded-failure fixtures land in `tests/fixtures/`.

**Prompt 01 is locked** (`docs/prompt-01-definition-of-done.md`): do not change the 0/1/2 exit-code semantics, the meaning of `airReady`, or the `PreflightReport` field names. New checks add *warnings and errors within* that contract. This prompt is the designated landing site for the first warning producer, so the `#[ignore]`d `warnings_only_exits_1_without_flag_0_with_it` test in `crates/nbe-preflight/tests/integration.rs` should now be implemented and un-ignored.

## Step 8: Carried-forward cleanups

Small items found reviewing Prompt 04, to fold in where you are already working:

1. **Apply the quality cap at publish time.** `show.load` currently caps whatever probe value exists in state, so the order is load-after-probe by luck. When you add device-loss/re-init handling, make the cap apply where the effective profile is published, so a later re-init cannot silently drop it.
2. **Snapshot state once per frame.** `render.rs::scene_for` takes four mutexes per frame (`package`, `view_item`, `transition`, `preview_item`). Uncontended today; you are about to add decode threads that touch `package`. Take one consistent snapshot per frame instead.
3. **Log malformed elements.** `scene.rs::element_spec` drops elements with no id/kind via `filter_map` and says nothing, which is a black frame with no explanation. Lenient-on-air is right; silent is not — add a `warn!`.
4. **Latch the View-failure log.** A persistently failing `render_bus(Bus::View)` engages the fallback correctly but error-logs every frame forever. Log once per fault episode, and again on recovery.

## Step 9: Tests

Headless, using the generated fixtures. Each test names the production path it covers:

1. **Frame-exact decode**: frame N of the reference clip reads back to the reference pixels.
2. **AC-9 loop wrap**: ten consecutive wraps of a VRAM-resident loop with zero dropped frames, no decoder restart, and modulo-computed indices.
3. **Cadence**: the 12 fps clip normalized to 30 presents the declared 2,3,2,3 hold pattern exactly (AC-4's live half).
4. **Arm-time preload**: an armed clip's first frame is available inside the arm deadline, and a take never waits on decode.
5. **Decode-session cap**: the N+1th simultaneous session is refused or queued, and telemetry reports the count.
6. **Decode failure is a fault**: a deliberately corrupt asset emits `itemEvent: decodeError` — and a *video-unsupported* case no longer exists, because video is supported now.
7. **Preflight**: the VFR and wrong-resolution fixtures exit **exactly 2** with machine-readable reasons; the warnings-only fixture exits **exactly 1** without `--allow-warnings` and **0** with it.
8. **Loop budget math (AC-21)**: the computed `frameCostMiB` / `maxFramesByBudget` match the §12.5 formula for a known input.

**No-hardware policy:** if VideoToolbox or a wgpu adapter is unavailable, these tests MUST fail with a message naming what was missing. They never skip. CI runs on `macos-14`, which has both.

## Step 10: CI

The existing `rust` job covers this crate. It must stay green with `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace`. Extend the fixture gates so the new seeded-failure fixtures assert **exact** exit codes and report contents, in the style of the existing v0.2/v0.3 gates. Both CI jobs now echo their test summaries in a collapsed group — that output is the audit trail your report quotes.

## Deferred out of Prompt 05, recorded here

**§12.8 streaming read-ahead thread.** The §12.8 *policy* is implemented — the
cache plans `Streaming` when a loop exceeds its budget, computes the mandated
`max(2 × GOP, 60)` read-ahead window, and holds the last resident frame rather
than blocking the render thread. The background refill thread that keeps that
window ahead of the playhead is **not** built: a streamed loop currently shows
its resident window and then holds.

*Trigger condition:* this is the first lever to pull when streamed loops miss
deadlines. Until a show actually carries a loop larger than its budget, a
refill thread would be untested machinery coordinating decode with GPU upload
across threads — the kind of code that looks right and fails on air.

**Item start frame across a resync.** SPEC §5.9.4's snapshot names *what* is on
air but not *since when*, so a resynced timed item resumes from its first frame
rather than from where it actually was. Closing this needs a snapshot field
(`viewItemStartFrame`), which is a spec revision, not prompt work.

## Constraints

**Forbidden changes:**

- No audio, camera, guests, encoder, HAP, HEVC, or software decode in the live path.
- No wire-type definitions in `nbe-engine`; wire changes go in `crates/nbe-protocol` with `tests/mirror.rs` extended.
- No changes to Prompt 03's channel semantics (resync gate, per-connection `seq`, stale-`stateVersion` skip, one ack per applied directive) or Prompt 04's frame contract (one render per boundary, measured lateness to the watchdog, preview never blocking the View).
- No changes to Prompt 01's locked behaviours, and no edits to `schemas/manifest.v0.3.json` — schema changes are spec work.
- No second clock, no second render pipeline, and no re-resolution of transitions or references the control plane already resolved.

**Required properties:**

- Nothing expensive at frame time: decode, seek, and upload happen at arm/load or in read-ahead buffers.
- `anyhow` for binaries, `thiserror` for library errors.
- Vocabulary discipline: `View`, `Element`, `Sequence`, `Item`.

## Reporting obligations

The completion message must contain:

1. **The falsification table** (Standards §2a): each behaviour you deleted to test the suite, and which tests failed. At minimum cover decode-to-texture, master-clock frame selection, loop wrap, the decode-session cap, and the new preflight checks. Confirm the suite is green again afterwards and the tree is clean.
2. **Verbatim CI summary lines** for the workspace and the control-plane job (Standards §2b) — not a hand tally.
3. **What you changed in the Step 0 table**, and why.
4. **The `prompt04.rs` fixture-test update**: what it asserted before, what it asserts now, and why the change is correct rather than convenient.
5. **Measured numbers**: frame budget headroom at the probed profile, decode sessions at peak, and loop cache residency for the test loop.
6. Any gap you chose not to close, with the reason. Do not silently paper over one.
