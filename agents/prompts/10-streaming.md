# Agent Prompt 10 — Streaming Output (crates/nbe-engine)

**Targets: SPEC v0.3.2 (`docs/spec.v0.3.md`) — Sections 9.2 (hardware encode), 9.4 (streaming), 9.5 (local network survivability), 9.7 (multi-output unification), 16.14 (output commands), AC-10 (internet-loss survivability). Prerequisites: Agent Prompts 01–09 merged (compositor + recording both live on shared frames).**

You are a senior Rust engineer building the `nbe` broadcast engine. This prompt builds the streaming output: the show goes out to the world over RTMP or SRT, hardware-encoded from the same shared frames the recorder uses, with a reconnect loop that never takes the local show down with it.

Read these first:

- `docs/spec.v0.3.md` — Sections 9.4 and 9.5 are your contract. AC-10 is the test you will be measured against.
- `agents/prompts/09-recording.md` — the encode-session pattern you share.
- `VOCABULARY.md` — term ledger.

## Quality bar

This prompt complies with the NBE Implementation Standards (`docs/implementation-standards.md`). Specifically:

- **Schema-driven typed models:** This prompt introduces the streaming-output typed model (protocol and stream-state enums); these must be round-trip tested and enum-audited against the OutputDefaults.stream definition and Section 9.4.
- **Strict CI contracts:** Any new binary or observable behaviour must have an exact CI gate (exit codes, key strings, behavioural invariants) (see Standards §2), including the reconnect and AC-10 WAN-loss invariants.
- **Prompt structure compliance:** This prompt explicitly lists Forbidden changes, New tests required, and CI changes required (see Standards §3).

## Step 0: Scope discipline

Allowed now: RTMP and SRT per `outputs.stream.protocol`, using the Section 9.4 stream shape. Forbidden: WHIP output (a later contribution-output prompt), CPU encode, any stream work on the render thread.

## Step 1: The stream encode session

- VideoToolbox H.264 High, 1080p30, 6–12 Mbps recommended, AAC 48 kHz 192 kbps, 1-second keyframe interval — per Section 9.4, fed by the shared GPU frames per Section 9.7. Never recomposite; never read back to CPU.
- The session is configurable via `outputs.stream` (protocol, bitrates).

## Step 2: The publisher

- Encode in-process; transport via a Rust RTMP/SRT writer. SRT in caller mode for contribution links (e.g., a MediaMTX-class server); RTMP for platform endpoints.
- The publisher runs off the render thread: encoded frames flow over a bounded channel; a full channel drops stream frames, never View frames, and reports it.

## Step 3: Reconnect and health

- Stream failure MUST NOT stop local playout (Section 9.5): local view continues, recording continues, the stream retries with exponential backoff, automatically, forever.
- Telemetry per Section 10.1: `streamState` (idle/live/reconnecting) and `streamBufferMs`. Failures surface as `E_NETWORK`; `E_NO_HARDWARE_ENCODER` refuses to start without hardware encode.

## Step 4: Commands

- `stream.start` / `stream.stop` per Section 16.14, wired through the show.stop quiescence truth table (Section 16.1): internal `stream.stop`, up to 2 seconds graceful, then force.
- `stream.start` accepts an optional `url` override per the command schema; otherwise the manifest's `outputs.stream` wins.

## Step 5: Tests

1. **AC-10 (the WAN-loss harness)**: with the stream live, cut the network path in the test environment — the local view continues, the recording continues, the stream enters reconnect/backoff, and zero VIEW frames drop as a result of the stream failure.
2. **Reconnect**: stop the local test server mid-stream; watch `streamState` go live → reconnecting → live on recovery, with no operator action.
3. **Stream shape**: the outbound stream verifies the Section 9.4 contract — H.264 High, 1-second keyframes, 48 kHz AAC, configured bitrate envelope.
4. **Isolation**: a publisher under a saturated/failing network never blocks the render thread — the bounded channel drops stream frames, not View frames, and reports the drops.
5. **Quiescence**: `show.stop` with an active stream stops it gracefully; the force path warns and stops it anyway.

CI: the existing `rust` job covers this, with a loopback test double speaking enough RTMP/SRT to accept and inspect the stream (MediaMTX-class locally; a mock in CI). `cargo check --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` all pass.

## Constraints

- No WHIP output yet, no CPU encode, no streaming work on the render thread.
- The stream is a shared-frame encoder session with a bounded channel — the View never pays for the network.
- `anyhow` for the binary, `thiserror` for library errors.
- Vocabulary discipline: `View`, `Element`, `Sequence`, `Item`.
