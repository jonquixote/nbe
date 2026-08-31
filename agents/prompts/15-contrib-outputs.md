# Agent Prompt 15 — Contribution Outputs (WHIP)

**Targets: SPEC v0.3.1 (`docs/spec.v0.3.md`) — the contribution-output direction deferred from Section 9.4, with the survivability guarantees of Sections 9.5 and 9.7 unchanged. Prerequisites: Agent Prompts 01–12 merged (shared-frame encode, reconnect, and watchdog all exist to copy from).**

You are a senior Rust engineer adding the WHIP contribution output to the `nbe` engine: the program feed, sent to a WebRTC ingestion endpoint, with the same hardware encode and the same never-touch-the-local-show discipline as the RTMP/SRT path.

Read these first:

- `docs/spec.v0.3.md` — Sections 9.4, 9.5, and 9.7 remain the contract; this extends them.
- `agents/prompts/10-streaming.md` — the pattern you follow. If you find yourself inventing a second reconnect loop, stop and reuse the first one.

## Step 0: Scope discipline

Allowed now: WHIP output (H.264 + Opus over WebRTC-HTTP ingestion), command and telemetry parity with the existing stream output. Forbidden: CPU video encode, any WHIP work on the render thread, and changes to the RTMP/SRT path beyond sharing what should have been shared all along.

## Step 1: The session

- Hardware H.264 from the same shared frames per Section 9.7; Opus audio from the master bus. WebRTC transport via the workspace's chosen Rust WebRTC stack; WHIP signaling per the protocol.
- Configured as a new `outputs.stream.protocol` option; command surface gains nothing new — `stream.start`/`stream.stop` already say enough.

## Step 2: Health and isolation

- Same bounded-channel isolation as Prompt 10: a failing WHIP endpoint drops stream frames, never View frames.
- Same reconnect-with-backoff, same `streamState`/`streamBufferMs` telemetry, same quiescence truth table on `show.stop`.

## Step 3: Tests

1. **Loopback**: a local WHIP test double accepts the session and verifies the media shape (H.264, 1-second keyframes, Opus).
2. **Reconnect**: kill the endpoint mid-session; state goes live → reconnecting → live on recovery.
3. **Isolation**: endpoint failure under load drops zero View frames.
4. **Parity**: everything Section 16.14 says about `stream.*` is true for WHIP without new commands.

CI: the existing `rust` job with the loopback double. `cargo check --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` all pass.

## Constraints

- One streaming architecture, two transports. No parallel reconnect loops, no render-thread work, no CPU video encode.
- `anyhow` for the binary, `thiserror` for library errors.
- Vocabulary discipline: `View`, `Element`, `Sequence`, `Item`.
