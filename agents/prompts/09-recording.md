# Agent Prompt 09 — Recording Output (crates/nbe-engine)

**Targets: SPEC v0.3.2 (`docs/spec.v0.3.md`) — Sections 9.2 (hardware encode), 9.3 (recording), 9.7 (multi-output unification), 16.14 (output commands), 16.1 (show.stop quiescence), 10.1 (recordSpaceMib telemetry), AC-6 (crash safety), AC-10 (WAN-loss survivability). Prerequisites: Agent Prompts 01–08 merged.**

You are a senior Rust engineer building the `nbe` broadcast engine. This prompt builds the recording output: the composited View plus the master audio bus, hardware-encoded and written crash-safe. Every show becomes a permanent record.

Read these first:

- `docs/spec.v0.3.md` — Sections 9.3 and 9.7 are your contract. AC-6 is the test you will be measured against.
- `agents/prompts/05-video-decode.md` — the decode side of the IOSurface/Metal interop you now use for encode.
- `VOCABULARY.md` — term ledger.

## Quality bar

This prompt complies with the NBE Implementation Standards (`docs/implementation-standards.md`). Specifically:

- **Schema-driven typed models:** This prompt introduces the recording-output typed model (container enum, fragment policy); these must be round-trip tested and enum-audited against the OutputDefaults.record definition.
- **Strict CI contracts:** Any new binary or observable behaviour must have an exact CI gate (exit codes, key strings, behavioural invariants) (see Standards §2), including the SIGKILL crash-safety invariant (AC-6).
- **Prompt structure compliance:** This prompt explicitly lists Forbidden changes, New tests required, and CI changes required (see Standards §3).

## Step 0: Scope discipline

Allowed now: VideoToolbox H.264 hardware encode of the composited View plus the master audio bus, written to fragmented MP4 (default) or Matroska. Forbidden: RTMP/SRT streaming (Prompt 10), ISO recording (the `isolation` hook is reserved — master only in v1), CPU x264 anywhere, GPU readback of frames.

## Step 1: The encode session

- VideoToolbox H.264 hardware encode fed by the compositor's shared GPU frames via the IOSurface path — Section 9.7: one composite, one GPU frame, N encoder sessions sharing it. Never recomposite, never read back to CPU.
- Keyframe interval: 1 second. Generous default bitrate for the recording, configurable via `outputs.record`.

## Step 2: The writer

- Container per `outputs.record.container`: `fragmentedMp4` (default) or `matroska`.
- Fragment policy per Section 9.3: fragments ≤ 1 second, init-segment safe, audio interleaved, no finalization required.
- Files land in `outputs.record.directory`, named by show/episode plus start timestamp. Timecode metadata where available.

## Step 3: Commands

- `record.start` / `record.stop` per Section 16.14, with `E_NO_HARDWARE_ENCODER` and `E_DISK` failure modes. If no hardware encoder is available, recording refuses to start rather than falling back to CPU.
- `show.stop` quiescence per Section 16.1's truth table: an internal `record.stop`, up to 2 seconds of graceful shutdown, then force-stop with a warning. The file stays playable throughout because fragments are already written.

## Step 4: Markers

- `marker.add` during a recording becomes a recording chapter where the container supports it — Matroska, yes — and a sidecar JSON beside the file for fragmented MP4, which does not carry chapters cleanly. The sidecar is always written so the markers pipeline never depends on container support.

## Step 5: Telemetry

- `recordState` and `recordSpaceMib` (real free space on the target volume) report in the Section 10.1 telemetry shape.
- Recording failures surface as `E_DISK` and never touch the render loop.

## Step 6: Tests

1. **AC-6 (the kill test)**: SIGKILL the render process mid-recording; `ffprobe` parses the resulting file and at least one reference player plays it.
2. **Fragment policy**: verify ≤ 1-second fragments, init-segment safety, audio interleaving.
3. **Quiescence**: `show.stop` leaves a finalized, playable file; the force path logs its warning and the file still plays.
4. **Markers**: Matroska chapters are readable; the fMP4 sidecar carries the marker list with timecodes.
5. **Multi-output (Section 9.7)**: a 5-minute headless record run with the compositor live drops zero View frames — recording must not recomposite.
6. **Telemetry**: `recordSpaceMib` reports real free space; `E_DISK` fires against an unwritable target.

CI: the existing `rust` job on macos-14 covers this (Metal and VideoToolbox are available; the kill test runs there). `cargo check --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` all pass.

## Constraints

- No streaming, no ISO tracks, no CPU encode, no readback. Recording is a shared-frame encoder session, nothing more.
- Recording never blocks the render loop; fragment writes are off the frame path.
- `anyhow` for the binary, `thiserror` for library errors.
- Vocabulary discipline: `View`, `Element`, `Sequence`, `Item`, `Marker`.
