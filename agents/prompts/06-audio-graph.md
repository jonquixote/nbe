# Agent Prompt 06 — Audio Graph (crates/nbe-engine)

**Targets: SPEC v0.3.3 (`docs/spec.v0.3.md`) — Section 8 (the audio engine, entire), 8.9 (audio and the master clock), 8.10 (audio faults), 7.13 (non-blocking rule), 10.1/10.1.1 (audio telemetry and ownership), 10.3 (the watchdog is video-only), 11.5.1 (drift), 12.1 (clock discipline), AC-8 (drift), AC-13 (soundboard latency), AC-14 (loudness), AC-18 (mix-minus isolation), AC-19 (click-free). Prerequisites: Agent Prompts 01–05 merged, including the 02b/02c, 03b/03c, 04b, and P5-closeout follow-ups.**

You are a senior Rust engineer building the `nbe` broadcast engine. This prompt builds the audio engine: the bus graph, the real-time callback, ducking, the soundboard, click-free transitions, mix-minus, and drift correction. Audio breaks more live shows than video does — this is the prompt where that stops being true.

Read these first:

- `docs/spec.v0.3.md` — Section 8 is your contract. **§8.9 and §8.10 are new in v0.3.3 and were written for this prompt**: they answer how audio follows the master clock, what an audio fault is, and why the video watchdog must not hear about it. Do not re-derive those answers.
- `docs/implementation-standards.md` — §2a (falsification) and §2b (test counts) are how this prompt is reviewed.
- `agents/prompts/05-video-decode.md` — the clip pipeline you are attaching audio to.
- `VOCABULARY.md` — term ledger. `View`, never Program. `Element`, never Layer.

## Step 0: What already exists — extend it, do not rebuild it

`crates/nbe-engine` is a working render node with real video: it decodes through VideoToolbox, resolves scenes from show state, composites on master-clock boundaries with cut and mix, counts dropped frames against real deadlines, and reports telemetry the control plane merges. **Your job is to give it sound.**

| Module | What it already does | What Prompt 06 does to it |
|---|---|---|
| `crates/nbe-decode` | VideoToolbox video decode, frames by presentation index, `probe_asset` for preflight. The project's **only** `unsafe` crate — CI fails if `allow(unsafe_code)` appears anywhere else | Audio track decode belongs **here**, beside video decode, not in a second FFI site. Keep the crate boundary. |
| `video.rs` | `SessionPool` (SPEC §24 decode-session cap, refusal counting), `VideoLibrary`, load-time decode | Audio decode sessions draw from the **same pool** — the platform limit is per process, not per media type. |
| `render.rs` | Textured-quad pipeline, `render_frame(frame, deadline)`, drop counting, watchdog feeding, rung 0/1, `BusScene { scene, alpha, t0 }` | **Do not put audio in the render loop.** §7.13 and §8.9 both forbid it. The audio graph is a separate thread with a harder deadline. |
| `scene.rs` | `PackageIndex`, `LayerSource::{Solid, Image, Video}`, `Transition { from_start_frame, start_frame, kind, duration_frames }` | Audio follows the same transition object — a mix crossfades audio over the same frames it crossfades pixels (§8.7.5). |
| `state.rs` | `view_item`, `view_item_start_frame` (§12.1's `t0`), `FrameSnapshot`, counters, rung, quality profile | Add audio state. `t0` is already the value audio needs for sample mapping. |
| `directive.rs` | Applies directives, one ack each, arms transitions at the next boundary | `soundboard.*`, `audio.*`, `guest.mute` land here. Keep one ack per applied directive. |
| `telemetry.rs` | Emits the §10.1 engine-owned shape with real values | `audioUnderrunsTotal`, `audioDriftMs`, `busPeakDbfs` are **already on the wire** (v0.3.3, both sides, mirror-audited). Fill them in; do not invent a second path. |
| `crates/nbe-protocol` | All wire types, three-layer mirror audit | Nothing to add. If you believe otherwise, stop and say so. |

### Two defects to fix while you are in here

Both were found reviewing Prompt 05 and are small, real, and adjacent to your work:

1. **A resync naming no item does not clear the old one.** `directive.rs::on_resync` sets `view_item` only when the snapshot names one, so a snapshot saying "nothing is on air" — the state after `show.stop` — leaves the previous item rendering. Handle the `null` case, and test it.
2. **Preview clips pin to master zero.** `render.rs::scene_for` passes `t0 = 0` for the Preview bus, so an armed clip previews as black once the master clock passes the clip's length. Preview needs its own "when was this armed" frame, the way the View bus got `view_item_start_frame` in the P5 closeout.

## The three questions this prompt does not leave to you

They were unanswered in v0.3.2 and are answered in v0.3.3. Read the sections; the summary here is orientation, not the contract.

1. **Audio follows the master clock, and there is only one clock (§8.9).** The device callback is a cadence, not a clock. A source's audio is read at `sampleForMasterFrame(F) = (F - t0) * sampleRate / houseFrameRate` — the same `t0` discipline §12.1 gives video, using the same `view_item_start_frame` the engine already tracks. No playback cursor advances on its own, so a frame and its audio cannot disagree about where they are.
2. **An underrun is the audio equivalent of a dropped frame (§8.10).** It is counted (`audioUnderrunsTotal`), the first of each episode is logged, and sustained underruns raise `E_AUDIO` in status. **It does not touch the video watchdog**: §10.3 now says so explicitly. Cutting the View to a slate because a soundboard sample underran turns a small fault into a visible one.
3. **Audio lives beside the render loop, never inside it (§8.9, §7.13).** The callback runs on the device's real-time thread at higher priority and with a harder deadline. It must not allocate, lock, block, or do I/O; the render loop must not call into it and it must not call into the renderer; control arrives and meters leave through lock-free structures.

## Quality bar

Per the NBE Implementation Standards:

- **Schema-driven typed models (§1):** the bus and audio-transition-mode enums are audited against the §8.1 bus table and the §8.7.3 mode list, and against `audio.bus.set`'s payload enum in `packages/control-plane/src/protocol.ts` — the control plane already validates those names, and two spellings of "guestReturn" is a bug waiting for a live show.
- **Strict CI contracts (§2):** exact gates, including the click-free and mix-minus behavioural invariants as measured numbers, not adjectives.
- **Falsification (§2a):** required, table in the report.
- **Test counts (§2b):** quote the CI summary lines.

## Step 1: Scope discipline

Allowed: `cpal` for device I/O (CoreAudio underneath on macOS), 48 kHz float32 internal, clip audio from the Prompt 05 pipeline, soundboard assets. Forbidden: WebRTC, the encoder, plugins. The `guest` bus exists and is synthetic — real guest audio arrives with the guest prompt.

**The graph must be renderable offline.** Every acceptance criterion below is a measurement over samples, and CI has no audio hardware. Build the graph so a test can render N samples into a buffer with no device present; the device is one consumer of that graph, not its owner. A design that only works with a device open is a design that cannot be tested.

## Step 2: The real-time thread

- The callback fills its buffer from the graph and does nothing else. No allocation, no locks, no I/O (§8.9).
- Control reaches it through a lock-free structure; meters and counters leave the same way.
- A callback that cannot be filled in time counts an underrun (§8.10) and emits silence rather than stale samples.

## Step 3: The bus graph

Implement §8.1's buses: `mic`, `clip`, `music`, `sfx`, `guest`, `master`, `guestReturn` (per guest), `ifb`.

- Each bus: gain −60 dB to +12 dB, mute, peak + RMS metering, solo as monitor-only PFL (§8.2).
- The master bus: limiter and loudness-safe output, peak metering.
- Bus peaks reach telemetry as `busPeakDbfs`.

## Step 4: Clip audio on the master clock

- Clip audio decodes at **arm/load** time into the warm buffer — never at take time.
- The sample read is `(F - t0) * 48000 / houseRate` per §8.9, using the item's existing start frame.
- Drift between the device's consumed samples and the master clock's implied position is measured and reported as `audioDriftMs`, corrected per §11.5.1.

## Step 5: Ducking

§8.3 on the music bus: depth −6 dB, attack 10 ms, release 250 ms, triggered by `audio.duck`. Ducking MUST NOT affect `mic` or `guest` unless explicitly configured.

## Step 6: The soundboard

- Assets preloaded into RAM at show load.
- Trigger latency under 20 ms (AC-13), measured as samples between the command and the first non-zero sample.
- Playback MUST NOT cause dropped video frames.

## Step 7: Click-free everything

- Every gain change is a ramp: minimum 5 ms, default 10 ms, maximum 50 ms. Sample-step cuts are forbidden (§8.7.1).
- Wire `view.take`'s audio object: `follow` (the item's `audioPolicy` — AFV), `crossfade`, `cut`, `mute` (§8.7.3). A video `mix` crossfades audio over the same duration, equal-power (§8.7.5); a video `cut` still ramps at least 5 ms (§8.7.6).

## Step 8: Mix-minus and IFB

Implement §8.6 structurally:

- `guestReturn(G) = mic + clip + music + sfx + every guest bus except G`.
- `ifb = program − anchor mic + talkback`.
- Echo prevention is **structural, not procedural**: `guestBus(G)` cannot route into `guestReturn(G)` because the graph has no such edge, not because a check rejects it. A violation is `E_AUDIO`.

## Step 9: Tests

Offline, over rendered samples. Each names the production path it covers:

1. **AC-19 click-free**: a take, an `item.stop`, a `soundboard.stop`, and a bus mute each produce no sample-to-sample discontinuity above −60 dBFS.
2. **AC-18 mix-minus**: a −20 dBFS 1 kHz tone on guest G's bus measures at or below −80 dBFS in `guestReturn(G)`, and is audible in `guestReturn(H)`.
3. **AC-13 soundboard latency**: fewer than 960 samples (20 ms at 48 kHz) between trigger and first non-zero output.
4. **Ducking**: music attenuates by 6 dB with the declared attack, and recovers over the declared release.
5. **AC-8 drift**: over an accelerated soak, audio position stays within ±1 frame of the master clock.
6. **Underrun accounting (§8.10)**: a starved graph counts underruns, and **the video fallback slate stays inactive** — this is the test that pins §10.3's scope.
7. **Metering**: bus peaks appear in the telemetry frame.
8. **Enum audit**: bus names match the §8.1 table and the control plane's `audio.bus.set` enum.

**No-hardware policy:** these tests must run and pass with no audio device present. A test that skips without hardware is not a gate.

## Step 10: CI

The `rust` job covers this crate. `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` all pass. The audio tests run headless on `macos-14` with no device — if they need one, the design in Step 1 is wrong.

## Constraints

**Forbidden changes:**

- No WebRTC, encoder, or plugins. The guest bus is synthetic.
- No `nbe-protocol` or schema changes: the audio telemetry fields already exist (v0.3.3).
- No `unsafe` outside `crates/nbe-decode` — CI enforces it.
- No changes to the Prompt 03 channel semantics, the Prompt 04 frame contract (one render per boundary, measured lateness to the watchdog, preview never blocking the View), or Prompt 01's locked preflight behaviours.
- No second clock. No audio in the render loop. No decode at take time.

**Required properties:**

- The callback is provably real-time-safe: no allocation, no locks, no I/O.
- `anyhow` for binaries, `thiserror` for library errors.
- Vocabulary discipline: `View`, `Element`, `Sequence`, `Item`.

## Reporting obligations

1. **The falsification table** (§2a) covering, at minimum: the ramp (clicks appear when it is removed), mix-minus isolation, soundboard latency, underrun counting, and the §10.3 boundary (an audio fault must not raise the fallback).
2. **Verbatim CI summary lines** for both jobs (§2b).
3. **What changed in the Step 0 table, and why.**
4. **Both Step 0 defects fixed**, each with the test that now covers it.
5. **Measured numbers**: soundboard trigger latency in samples, worst click magnitude in dBFS, mix-minus rejection in dB, drift over the soak.
6. Any gap you chose not to close, with the reason.
