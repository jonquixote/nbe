# Agent Prompt 05 — Video Decode Integration (crates/nbe-engine + nbe-preflight)

**Targets: SPEC v0.3.1 (`docs/spec.v0.3.md`) — Sections 6.2 (mezzanine format), 7.2 (render model), 7.13 (frame budget), 12 (deterministic loops — the whole section), 18 (cadence), 21 (decode-session risk), AC-9 (loop wrap), AC-21 (loop budget math), AC-3/AC-4 (preflight). Prerequisites: Agent Prompts 01–04 merged (validator, control plane, bridge, compositor rendering slates/images on the master clock).**

You are a senior Rust engineer building the `nbe` broadcast engine. This prompt integrates hardware video decode: video-backed `clip` elements play, `videoLoop` elements loop deterministically, and the loop cache becomes real. The same decode infrastructure deepens `nbe-preflight` so the decode-based checks from SPEC Section 19 finally run.

Read these first:

- `docs/spec.v0.3.md` — Section 12 is the heart of this prompt. Sections 6.2, 7.13, 18, and the AC-9/AC-21 acceptance criteria are your tests.
- `agents/prompts/04-basic-compositor.md` — the render loop you are feeding textures into.
- `VOCABULARY.md` — term ledger.

## Quality bar

This prompt complies with the NBE Implementation Standards (`docs/implementation-standards.md`). Specifically:

- **Schema-driven typed models:** This prompt introduces the loop-cache typed model and the frame-selection/pulldown enums; these must be round-trip tested and enum-audited against the LoopMetadata/Pulldown definitions, with the decode fixtures driving exact CI gates.
- **Strict CI contracts:** Any new binary or observable behaviour must have an exact CI gate (exit codes, key strings, behavioural invariants) (see Standards §2).
- **Prompt structure compliance:** This prompt explicitly lists Forbidden changes, New tests required, and CI changes required (see Standards §3).

## Step 0: Scope discipline

Allowed now: VideoToolbox hardware decode on macOS for H.264 clips and ProRes 4444 alpha loops, PNG-sequence alpha as fallback. Forbidden: audio (Prompt 06), camera/guest ingest, the encoder, HAP (later), HEVC, any software decode in the live path. The engine never repairs media live — out-of-spec media was already rejected by preflight (Assumption 3).

## Step 1: Decode core

- A `decode` module in `nbe-engine`: a VideoToolbox session decoding H.264/ProRes, producing frames as GPU textures. Use IOSurface-backed `CVPixelBuffer` → Metal texture for zero-copy handoff into `wgpu` (hal interop). No CPU readback in the live path.
- Frame delivery carries a presentation index, not just pixels: the compositor selects frames by index.

## Step 2: Master-clock frame selection

- Clips do not play on their own clock. The frame shown at master frame `F` is clip frame `(F - itemStartFrame)` — a pure function of the show clock.
- Loop elements: `sourceIndex = (F - t0) mod P` per Section 12.1. No restart events; a loop boundary is computationally indistinguishable from any other frame.

## Step 3: Arm-time preload

At arm time, per the preload rules: open the file, seek to the first frame, decode the first frame, upload the texture, mark the item ready. The render loop MUST never wait on any of this at take time.

## Step 4: The loop cache

Follow Section 12 exactly, in the mandated order:

1. **Format accounting first** (Section 26 sequencing): compute `frameCostMiB` from the selected texture format and `maxFramesByBudget` from the effective budget (Sections 12.5, 12.6). "NV12" means two-plane YUV (`R8Unorm` + `Rg8Unorm`) with shader-side BT.709 conversion per the v0.2.1 errata.
2. **VRAM ring buffer** for resident loops (Section 12.7): `textureSlot = sourceIndex mod P`; no decoder restart at wrap.
3. **Double-buffered read-ahead** for streamed long loops (Section 12.8): minimum read-ahead `max(2 * GOP length, 60 frames)`; wrap never blocks the render thread; read-ahead failure falls back to frozen frame, or to the fallback slate if live.
4. **Budgets enforced**: per-loop and total cache budgets, with the Apple unified-memory clamp against `recommendedMaxWorkingSetSize` (Section 12.6).

## Step 5: Decode-session budget

- Cap simultaneous active decode sessions; reuse sessions; evict idle ones (Section 21's VideoToolbox decode-session risk).
- Expose `decodeSessions` in telemetry.

## Step 6: Preflight deepening

`nbe-preflight` gains the decode-based checks from SPEC Section 19, powered by the same decoder:

1. First- and last-frame decode of every video/alpha asset.
2. CFR verification (`cfr: false` for VFR — AC-3).
3. Resolution and house frame rate.
4. `durationFrames` versus `expectedDurationFrames`.
5. Loop period matches `loop.periodFrames`.
6. Alpha presence for alpha assets.

Add a fixture generator: `scripts/generate-fixtures.sh` uses ffmpeg to synthesize tiny test clips — a valid 1080p30 CFR clip, a VFR clip (must fail), a wrong-resolution clip (must fail), a 12 fps animation for cadence verification. Seeded-failure fixtures land in `tests/fixtures/`.

## Step 7: Tests

Headless, using the generated fixtures:

1. **Frame-exact decode**: frame N of the reference clip readbacks to the reference pixels.
2. **AC-9 loop wrap**: ten consecutive wraps of a VRAM-resident loop with zero dropped frames, no decoder restart event, and modulo-computed indices.
3. **Cadence**: the 12 fps clip normalized to 30 presents the declared 2,3,2,3 hold pattern exactly (AC-4's live half).
4. **Arm-time preload**: an armed clip's first frame is available inside the arm deadline; a take never waits on decode.
5. **Decode-session cap**: the Nth+1 simultaneous session is refused or queued, and telemetry reports it.
6. **Preflight**: the VFR and wrong-resolution fixtures exit 2 with machine-readable reasons.

CI: the existing `rust` job covers this (macos-14 has Metal and VideoToolbox). `cargo check --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` all pass.

## Constraints

- No audio, no camera, no guests, no encoder, no HAP, no HEVC, no software decode in the live path.
- Nothing expensive at frame time. Decode, seek, and upload happen at arm/load or in the read-ahead buffers — never in the render loop.
- `anyhow` for the binary, `thiserror` for library errors.
- Vocabulary discipline: `View`, `Element`, `Sequence`, `Item`.
