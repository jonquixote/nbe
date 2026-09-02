# Agent Prompt 06 — Audio Graph (crates/nbe-engine)

**Targets: SPEC v0.3.1 (`docs/spec.v0.3.md`) — Section 8 (the audio engine, entire), Section 11.5 (drift policy), Section 7.13 (non-blocking rule), AC-8 (drift), AC-13 (soundboard latency), AC-14 (loudness), AC-18 (mix-minus isolation), AC-19 (click-free). Prerequisites: Agent Prompts 01–05 merged (video playing under the master clock).**

You are a senior Rust engineer building the `nbe` broadcast engine. This prompt builds the audio engine: the bus graph, the real-time callback, ducking, the soundboard, click-free transitions, mix-minus, and drift correction. Audio breaks more live shows than video does — this is the prompt where that stops being true.

Read these first:

- `docs/spec.v0.3.md` — Section 8 is your contract. Section 8.7's transition behavior and Section 7.6/8.6's mix-minus rules are the parts everyone else gets wrong.
- `agents/prompts/05-video-decode.md` — the clip pipeline you are attaching audio to.
- `VOCABULARY.md` — term ledger.

## Quality bar

This prompt complies with the NBE Implementation Standards (`docs/implementation-standards.md`). Specifically:

- **Schema-driven typed models:** This prompt introduces the audio bus graph and the audio-transition-mode typed model; the bus and mode enums must be round-trip tested and enum-audited against the Section 8.1/8.7.3 tables.
- **Strict CI contracts:** Any new binary or observable behaviour must have an exact CI gate (exit codes, key strings, behavioural invariants) (see Standards §2), including the click-free and mix-minus behavioural invariants.
- **Prompt structure compliance:** This prompt explicitly lists Forbidden changes, New tests required, and CI changes required (see Standards §3).

## Step 0: Scope discipline

Allowed now: `cpal` for cross-platform device I/O (CoreAudio underneath on macOS), 48 kHz float32, the mic input via the audio interface, clip audio from the Prompt 05 pipeline. The guest bus exists but is synthetic for now — real WebRTC guest audio arrives with the guest prompt. Forbidden: WebRTC, the encoder, plugins.

## Step 1: The real-time audio thread

- The audio graph runs in the audio callback on a dedicated real-time thread. The callback MUST NOT allocate, lock, or touch I/O — ever. Control flows in via lock-free ring channels; meters flow out the same way.
- Neither the render loop nor the WebSocket tasks may block audio. This is Section 7.13's rule applied to the audio path.

## Step 2: The bus graph

Implement Section 8.1's buses: `mic`, `clip`, `music`, `sfx`, `guest`, `master`, `guestReturn` (per guest), `ifb`.

- Each bus: gain −60 dB to +12 dB, mute, peak + RMS metering, solo as monitor-only PFL (Section 8.2).
- The master bus: compressor, limiter, loudness-safe output, peak metering.

## Step 3: Clip audio

- Clips from Prompt 05 carry their 48 kHz AAC/PCM audio into the `clip` bus.
- Decode and buffer audio at arm time — the warm audio buffer from the preload rules. Nothing decodes at take time.
- Clip audio follows the master clock by sample mapping from `(F − itemStartFrame)`, never its own clock.

## Step 4: Ducking

Implement Section 8.3 on the music bus: depth −6 dB, attack 10 ms, release 250 ms, triggered by manual `audio.duck` or voice-detected mic. Ducking MUST NOT affect the `mic` or `guest` buses unless explicitly configured.

## Step 5: The soundboard

- Soundboard assets are preloaded into RAM at show load.
- Trigger latency under 20 ms on Tier-1 hardware (AC-13).
- Playback MUST NOT cause dropped video frames.

## Step 6: Click-free everything

- Every gain change is a ramp: minimum 5 ms, default 10 ms, maximum default 50 ms. Hard sample-step cuts are forbidden (Section 8.7.1).
- Wire the `view.take` audio object: modes `follow` (the item's `audioPolicy` — AFV), `crossfade`, `cut`, `mute` (Section 8.7.3). Video `mix` crossfades audio over the same duration, equal-power curve recommended (Section 8.7.5). Video `cut` applies at least a 5 ms ramp at any boundary (Section 8.7.6).

## Step 7: Mix-minus and IFB

Implement Section 8.6 structurally:

- `guestReturn(G) = mic + clip + music + sfx + all guest buses except G`.
- `ifb = program − anchor mic + talkback` (or without talkback if none exists).
- Echo prevention is structural, not procedural: `guestBus(G)` can never route into `guestReturn(G)`. A violation is an `E_AUDIO` fault.

## Step 8: Drift and sync

- Audio is synchronized to the master show clock (Section 8.1).
- Drift stays within ±1 frame over a 30-minute show on Tier-1 hardware (Section 11.5.1, AC-8). Correct by adjusting audio presentation or dropping/holding non-critical frames — never unbounded drift.

## Step 9: Tests

1. **AC-13**: a soundboard trigger produces audible output within 20 ms on Tier-1.
2. **AC-19**: in a silent test pass, any `view.take`, `item.stop`, `soundboard.stop`, or bus mute/unmute produces no click impulse exceeding −60 dBFS.
3. **AC-18 (synthetic)**: with a −20 dBFS 1 kHz test tone on a synthetic guest bus and no other program sources, that guest's `guestReturn` measures the tone at or below −80 dBFS.
4. **Ducking**: mic speech attenuates music by 6 dB with the declared attack and release, recovering cleanly.
5. **AC-8 (accelerated soak)**: audio/video sync drift stays within ±1 frame; the test may run at accelerated clock speed.
6. **Metering**: every bus reports peak + RMS in telemetry.

CI: the existing `rust` job covers this. macos-14 runners have no audio hardware — audio device I/O is stubbed behind a null device in CI; real-device tests run locally. `cargo check --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` all pass.

## Constraints

- No WebRTC, no encoder, no plugins. The guest bus is synthetic until the guest prompt.
- The audio callback is provably real-time-safe: no allocation, no locks, no I/O in the callback.
- `anyhow` for the binary, `thiserror` for library errors.
- Vocabulary discipline: `View`, `Element`, `Sequence`, `Item`.
