# Draft for ruling: a dead output thread moves state and raises an alert (R42's silence)

**Not applied, and not approved.** By the user's ruling 3 of 2026-11-02: spec row first, no code. Nothing here is in the tree. Row number 16 is the next free in v0.4.8; the number is the user's. Any earlier approval of the four decisions below was withdrawn on 2026-11-02 until this text has been read.

## The row

| # | Change | Sections | Guarded by |
|---|---|---|---|
| 16 | **`recordHealth` on the wire, and a dead output is an alert.** The engine publishes the take's own health on the §10.1 tick as `"ok"`, `"dead"` or `"closed"`, always emitted, stubbed `"none"` before any take (the `recordTapPath` precedent). It is split from the commanded `recordState` by the same shape as v0.4.6's `streamTransportState`: `recordState` says what was commanded, and this field says what the thread is doing. A dead stream thread closes its publisher, so `streamTransportState` reads `closed`, and §10.1.2's existing `live` × `closed` row raises `stream.divergence`. Before this row, R42's panics left `streamState: live`, `streamTransportState: live`, `recordState: recording`, `alerts: []` for 21 and 22 minutes, and a dead take surfaced only at `record.stop` | 10.1 (field block, new note), 10.1.1 (ownership), 10.1.2 (alert table, comparison, mapping) | See "Guards" |

## The passages

**§10.1, field block.** After `"recordState": "recording",`:

```json
"recordHealth": "ok",
```

**§10.1, new note, after the stream transport's.**

> **The take's health (normative, new in v0.4.8 row 16).** `recordHealth` carries what the thread writing the take is doing: `"ok"` while it runs, `"dead"` once it has exited without reporting the take's end, `"closed"` after a take ends with that report. It is **always emitted, stubbed `"none"` before any take has started**, per §10.1.1, the `recordTapPath` precedent. It reads `"dead"` on the first report after the thread's exit, and keeps that value through the stop of that take, until the next `record.start` moves it to `"ok"`.
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
| `recording` | `dead` | fresh, postdating the anchor | none | (a) `record.dead`: the take's thread died; the file holds what reached it |
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

## What stays out of this row

- A thread that hangs without exiting. It is alive, so `dead` does not fire. A stall detector is a different row.
- What `record.stop` answers on a dead take. It stays `E_RECORD_TIMEOUT`, with `command.unacknowledged.record`.
- `droppedFramesTotal`, which still counts View frames only. R42's blocker on zero-drop claims stands either way.
- The panic itself. That is `vendor/wgpu-core`.

## Mechanism, for the implementing PR, not spec text

The record and stream sessions each hold their thread's `JoinHandle`. `is_finished()` is non-blocking, so the telemetry path can read it each report with no lock on the render path. `stream.rs:846`'s `TrySendError::Disconnected(_) => false` is the render path's own witness and moves no counter today.

## Guards, for the implementing PR

As v0.4.6's row did:
- the field is always on the wire and stubs before any take;
- a tick carrying `recordHealth` parses and the field is readable, on the control plane;
- a thread killed mid-take reads `dead` on the next report, and `record.dead` raises, with both audit rows;
- a stream thread killed mid-stream reads `closed`, does not redial, and raises `stream.divergence`;
- a clean stop reads `closed` and raises nothing.

Falsified by suppressing the field, suppressing the forwarding, and suppressing the raise.

These can now run on CI. PR #80's first run shows macos-15 has a hardware H.264 encoder: `# RECORD PATH: zeroCopy (Table)`, and a real take and a real stream in the dress. macos-14 had neither.

`docs/soak-protocol.md` §4 gains `recordHealth`'s distinct values per iteration, beside `streamTransportState`'s.
