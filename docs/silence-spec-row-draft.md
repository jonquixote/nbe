# Draft for ruling: a dead output thread moves state and raises an alert (R42's silence)

**Not applied, not ratified.** By the user's ruling 3 of 2026-11-02: spec row first, no code. Nothing here is in the tree. Row number 16 is the next free in v0.4.8; the number is the user's.

**Read by the user on 2026-11-02 (night), and the four decisions are approved.** Before ratification, the user asked for two additions: (a) what a take that ends by itself with a report does to `recordHealth` while the belief is `recording`, and (b) whether the engine allows more than one take at once. Both are below. Answering (a) changed four passages, each marked *(a)* where it changed:
- `"dead"` now means a take that ended without a stop, with or without a report. Before, it meant a thread that exited without a report.
- `"closed"` now means a take that a stop ended.
- The take mapping's `dead` row and the mechanism note now cover all three ways a take ends unbidden.
- The guards gain three cases.

(b) adds one passage to §16.14. The spec-revision PR (docs only) follows, for the second key.

## The row

| # | Change | Sections | Guarded by |
|---|---|---|---|
| 16 | **`recordHealth` on the wire, and a dead output is an alert.** The engine publishes the take's own health on the §10.1 tick as `"ok"`, `"dead"` or `"closed"`, always emitted, stubbed `"none"` before any take (the `recordTapPath` precedent). It is split from the commanded `recordState` by the same shape as v0.4.6's `streamTransportState`: `recordState` says what was commanded, and this field says what the thread is doing. A dead stream thread closes its publisher, so `streamTransportState` reads `closed`, and §10.1.2's existing `live` × `closed` row raises `stream.divergence`. Before this row, R42's panics left `streamState: live`, `streamTransportState: live`, `recordState: recording`, `alerts: []` for 21 and 22 minutes, and a dead take surfaced only at `record.stop` | 10.1 (field block, new note), 10.1.1 (ownership), 10.1.2 (alert table, comparison, mapping), 16.14 (`record.start`'s row) *(b)* | See "Guards" |

## The passages

**§10.1, field block.** After `"recordState": "recording",`:

```json
"recordHealth": "ok",
```

**§10.1, new note, after the stream transport's.**

> **The take's health (normative, new in v0.4.8 row 16).** `recordHealth` carries what the take is doing: `"ok"` while its thread runs; `"dead"` once the take has ended without a stop, whether its thread exited with an error report, exited with none, or the engine ended the take itself; `"closed"` once a stop has ended it. *(a)* It is **always emitted, stubbed `"none"` before any take has started**, per §10.1.1, the `recordTapPath` precedent. It reads `"dead"` on the first report after the take's end, and keeps that value through the stop of that take, until the next `record.start` moves it to `"ok"`. There is one take at a time (§16.14), so the field is one value, keyed by nothing, as `recordState` is.
>
> It is a separate field from `recordState`, deliberately. `recordState` is the control plane's, *as commanded*: it stays `"recording"` when a take dies, because the operator commanded a recording and has not commanded otherwise. `recordHealth` is the render node's and says what the thread is doing. A take that died reads `recordState: "recording"` beside `recordHealth: "dead"`, and that pair is the event this field exists to expose.

**§10.1, the stream transport's note.** After "`"closed"` when the transport is gone.", add:

> A stream whose encode thread has died is not flowing media, so its publisher closes and the field reads `"closed"`. It does not redial: a redial answers a lost socket, and this socket was not lost.

**§10.1.1, render-node row.** Add `recordHealth` (new in v0.4.8 row 16).

**§10.1.2, alert table.** New row:

| `id` | Raised when | `severity` | `auditEvent` |
|---|---|---|---|
| `record.dead` | (a) below: the control plane believes `recording`, and a fresh report postdating the anchor reads `recordHealth: dead` | `critical` | `record.dead` |

**§10.1.2, (a).** Replace "The comparison covers the stream only: the render node reports no record state." with:

> The comparison covers the stream, and from v0.4.8 row 16 the take: `recordHealth` is the render node's report of it.

**§10.1.2, a mapping for the take.** After the stream's mapping:

| CP `recordState` | Engine `recordHealth` | Report | Pending | Alert |
|---|---|---|---|---|
| any | any | any | an output command whose window elapsed unprocessed | (b) `command.unacknowledged.record` |
| `recording` | `dead` | fresh, postdating the anchor | none | (a) `record.dead`: the take ended without a stop; the file holds what reached it *(a)* |
| `recording` | `ok` | — | — | none |
| `idle` | any | — | — | none |
| any | any (the stale stub) | stale | — | no alert from this row; `engine.notReporting` instead |

As `stream.divergence` does, `record.dead` clears when the contradiction ends: a stop moves the belief to `idle`, or a new take reads `ok`.

**§10.1.2, the stream mapping's `live` × `closed` row.** Its text says "its publisher closes only on a stop". It becomes: "its publisher closes only on a stop, or when its encode thread dies. That row's alert covers both, and its `reason` names which."

## Decisions for the ruling, my recommendation first

1. **Stream half.** Close the publisher and reuse `stream.divergence`, as drafted. The spec already says `live` means "media is flowing", so today's reading is false, and this makes it true with no new field; `streamHealth` automation rules also fire, since they read this field. Against: a new engine behaviour, closing the socket. The alternative is a parallel stream health field, but the name `streamHealth` is taken by the automation trigger (`docs/automation-design.md`), and an open socket with no media would stay on air as dead air.
2. **Belief.** `recordState` stays `recording` until the operator stops, as drafted, matching §9.5's split. The alternative is a correction to `idle`, which hides that a take was commanded and lost.
3. **Severity.** `critical`, as drafted, because a show is losing its only copy while reading green. `stream.divergence` is `critical` too.
4. **Tokens.** `ok`, `dead`, `closed`, `none`. Name them as you like; the shape does not depend on them.

All four approved by the user, 2026-11-02 (night).

## (a) A take that ends by itself, with a report, while the belief is `recording`

**What happens today** (read on main, `c6dedb5`). There are three ways a take ends without a stop.
1. **Its thread exits with an error report.** A writer failure ends the take where it happens: a failed write (a full disk) is `E_DISK` (`record/writer.rs:532`), an AAC failure `E_AAC_UNAVAILABLE`, a malformed unit `E_RECORD_INPUT` (`record/thread.rs`, `push_video(..)?` and `drain_tap_fatal`). The report waits in the session's channel until `record.stop` reads it. Until then the engine holds the take as `Recording`, the wire says nothing, and the stop answers with the report's error.
2. **Its thread exits with no report.** This is R42's panic. The stop answers `E_RECORD_TIMEOUT`.
3. **The engine ends the take itself.** A lost zero-copy chain (`end_take_on_chain_loss`, `record/feed.rs:424`) abandons the session, keeps the file, moves the engine's own state to idle, and writes an ERROR line. The belief stays `recording`. The stop that follows is refused `E_FORBIDDEN_STATE`, because the engine holds no take (`zerocopy_migration.rs` asserts that refusal).

**The row's answer.** All three read `recordHealth: "dead"` on the first report after the end, and raise `record.dead`. A report changes what the stop returns, not whether the take is dead: in each case the operator commanded a recording and nothing is recording. `"closed"` is kept for a take that a stop ended, whether its finalize succeeded or failed.

**What the field does not carry: the cause.** Cases 1 and 2 return it to the stop, as today. Case 3 only logs it. Recommendation: no cause field in this row. The alert's `reason` reads "the take ended without a stop". The alternative, for the ruling: a second field, `recordEndCause`, carrying the token (`E_DISK`, `E_NO_ZEROCOPY`, `none` when no report came), so that the alert names the cause before any stop. That is a second wire field and a second guard.

**What does not end a take.** If the encoder refuses a frame, or failed to open on the thread, the frame is shed and the thread lives (`record/thread.rs`, `count_skip`). The take reads `"ok"` while it sheds. See "What stays out".

## (b) More than one take at once

**No.** Both sides allow one take at a time today:
- **The control plane** refuses `record.start` while the belief is `recording`: `E_FORBIDDEN_STATE`, "already recording" (`commands/output.ts:14`).
- **The engine** refuses it while it holds a take: `E_FORBIDDEN_STATE`, "record.start while already recording" (`directive.rs:913`). It has one session slot, `EngineState::record_session`.
- **`outputId`** selects the single configured record target and names the file. It does not open a second take (`directive.rs:877`).

So `recordHealth` is one value, like `recordState`.

After case 1 or 2 the engine still holds the dead take, so a new `record.start` is refused until a stop. After case 3 the engine is idle, but the belief refuses the start until the operator stops.

The spec does not say "one take at a time" in words: §16.14's `record.start` row names `E_NO_HARDWARE_ENCODER` and `E_DISK`, not the `E_FORBIDDEN_STATE` both sides return.

**§16.14, `record.start`'s row.** Preconditions gain "no take active". Failure modes gain `E_FORBIDDEN_STATE`, as `stream.start`'s row names its `held` refusal. If more than one take is ever wanted, `recordHealth` becomes per output: that is a later row.

## What stays out of this row

- A thread that hangs without exiting. It is alive, so `dead` does not fire. A stall detector is a different row.
- A take whose encoder refuses frames. The frames are shed, the thread lives, and the field reads `ok`. `skippedRecordFrames` counts them, and it is not on the tick.
- What `record.stop` answers on a dead take. It stays `E_RECORD_TIMEOUT`, with `command.unacknowledged.record`.
- `droppedFramesTotal`, which still counts View frames only. R42's blocker on zero-drop claims stands either way.
- The panic itself. That is `vendor/wgpu-core`.

## Mechanism, for the implementing PR, not spec text

The record and stream sessions each hold their thread's `JoinHandle`. `is_finished()` is non-blocking, so the telemetry path can read it each report with no lock on the render path. `stream.rs:846`'s `TrySendError::Disconnected(_) => false` is the render path's own witness and moves no counter today.

*(a)* The handle covers cases 1 and 2 only. In case 3 the session is gone before the next report, so the health is a per-take value held in `EngineState`:
- `record.start` sets it to `ok`;
- `end_take_on_chain_loss` sets it to `dead`;
- the telemetry path sets it to `dead` when the handle reads finished before any stop;
- the stop sets it to `closed`.

## Guards, for the implementing PR

As v0.4.6's row did:
- the field is always on the wire and stubs before any take;
- a tick carrying `recordHealth` parses and the field is readable, on the control plane;
- a thread killed mid-take reads `dead` on the next report, and `record.dead` raises, with both audit rows;
- a stream thread killed mid-stream reads `closed`, does not redial, and raises `stream.divergence`;
- a clean stop reads `closed` and raises nothing;
- *(a)* a take whose writer fails mid-take (a full disk, by a seam) reads `dead` and raises `record.dead`, and its stop still answers `E_DISK`;
- *(a)* a take that loses its zero-copy chain (the `lose_surface_pool` seam) reads `dead` and raises `record.dead`;
- *(b)* a second `record.start` during a take is refused `E_FORBIDDEN_STATE` on both sides, and `recordHealth` does not change.

Falsified by suppressing the field, suppressing the forwarding, and suppressing the raise.

These can now run on CI. PR #80's first run shows macos-15 has a hardware H.264 encoder: `# RECORD PATH: zeroCopy (Table)`, and a real take and a real stream in the dress. macos-14 had neither.

`docs/soak-protocol.md` §4 gains `recordHealth`'s distinct values per iteration, beside `streamTransportState`'s.
