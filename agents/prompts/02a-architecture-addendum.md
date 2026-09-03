# Agent Prompt 02a — Architecture Addendum

Addendum to Agent Prompt 02 — architecture decisions and semantics that must be pinned before implementation proceeds.

**Targets: SPEC v0.3.2 (`docs/spec.v0.3.md`) — Sections 5, 10.1, 10.4, 10.7, 13, 16, 17. Applies to `agents/prompts/02-control-plane.md` and every prompt that consumes the control plane (03 render node, 08 Companion, 13 operator shell).**

Read alongside:

- `agents/prompts/02-control-plane.md` — the base prompt this amends.
- `agents/prompts/03-render-node-bridge.md` — the consumer of the directive protocol pinned here.
- `docs/implementation-standards.md` — the NBE Implementation Standards.
- `docs/prompt-01-definition-of-done.md` — Prompt 01 is locked; nothing here reopens it.

---

## 1. Architecture decisions (normative)

### 1.1 The render bridge is a WebSocket interface, not a second transport

The render bridge is an interface (`RenderBridge`) served over the existing `:8462/nbe/v0.3` WebSocket endpoint by the set of `render`-role sessions; there is no bespoke second transport, and the only other implementation is the in-process loopback/mock used by tests.

Pinned server-to-client directive frame, distinct from the Section 5.4 command envelope:

```json
{ "v": "0.3", "kind": "directive", "seq": 91, "stateVersion": 413, "command": "view.take", "target": {}, "payload": {} }
```

Pinned client-to-server render frames, which the control plane MUST accept from `render`-role sessions: engine telemetry (Section 10.1 shape), `appliedStateVersion`, health, and item lifecycle events (item-end, decode error, device loss). The item lifecycle events are what make the `PLAYING -> DONE` and `-> MISSING/ERROR` rows of the Section 17.3 table reachable; without them those transitions are unreachable code.

### 1.2 Telemetry ownership is split, with an explicit merge rule

Telemetry is assembled by the control plane from two authorities: the control plane owns the show-state fields (`viewItem`, `previewItem`, `automationHold`, `qualityProfile`, commanded `streamState`/`recordState`, deprecation warnings), the render node owns the clock and performance fields (`masterClockFrame`, `droppedFramesTotal`, `renderGpuTimeMs`, `decodeSessions`, `vramUsedMib`, `textureCacheUsedMib`, `masterClockDriftMs`, `fallbackActive`, `recordSpaceMib`, `degradationRung`), and the merge caches the last engine report with its `receivedAt` — when that report is stale beyond the configured threshold the engine-owned fields report stub values and the snapshot carries `engineConnected: false`. The emitted field shape is always complete; a telemetry consumer MUST never see a missing field.

### 1.3 One HTTP server

A single Node HTTP server handles the WebSocket upgrade, the Section 10.4 `GET /nbe/v0.3/status` endpoint (show load state, master clock state, render node health, stream health, recording health, preflight state, last error), and the future Companion HTTP command endpoint from Prompt 08.

### 1.4 Manifest parsing stays in `nbe-core`; the control plane shells out to `nbe-preflight`

The control plane MUST NOT re-implement manifest parsing or preflight decisions: `show.load` and `show.preflight` invoke the `nbe-preflight` binary and consume `preflight_report.json` plus the SPEC 19.1 exit code (`0` air-ready, `1` warnings, `2` errors, mapping to `E_PREFLIGHT_FAILED`), and any TypeScript view of the manifest is generated from `schemas/manifest.v0.3.json` rather than hand-rolled.

This keeps Prompt 01's locked behaviours single-sourced (`docs/prompt-01-definition-of-done.md` §3) and makes `show.start`'s "preflight passed" precondition a fact rather than a flag the control plane sets for itself.

---

## 2. Semantics to pin

1. **One `stateVersion` bump per accepted command.** An accepted command is exactly one state transaction: one `stateVersion` increment, one broadcast state-change event, one audit record, and N directives all tagged with that same version. Per-mutator bumping makes `baseStateVersion` unusable for clients and makes the version tagged onto directives ambiguous.
2. **No mutate-then-throw.** Precondition failures MUST NOT mutate state. Detections that are themselves legal Section 17.3 transitions (asset missing detected, decode error, device loss) are their own transaction; the originating command still returns its error, carrying the post-detection `stateVersion`.
3. **Exact `show.stop` behaviour.** The Section 16.1 truth table, the 2-second graceful window, the forced stop, and the logged warning are all asserted by exact value and exact key string, not by return code alone (Standards §2).
4. **Resolved transitions.** Directives carry the resolved transition — never a preset name. Preset merge order (explicit payload fields override the named preset) and the Section 16.2 rule that `audio.durationFrames` defaults to the video `durationFrames` when `transition == "mix"` are resolved in the control plane, once.
5. **Token-authoritative auth.** The bearer token determines the role; `X-NBE-Role` is client-asserted and MUST match the token's role or the connection fails with `E_AUTH` (error frame first, then close, per Section 5.3). Constant-time token comparison; no empty or default token; bind `127.0.0.1` by default. The TLS `:8463` endpoint is out of scope for Prompt 02 and MUST be stated as such.
6. **Full audit-log shape.** Section 10.7 requires every control-plane action and auth event, which includes rejected commands and role denials — the abuse model is the reason the log exists. Each record: `ts`, `requestId`, `role`, token id, `command`, `rawCommand` when aliased, outcome, `errorCode`, `stateVersion` before and after. Append-only, with a stated fsync and retention policy. Automation actions are audited too (AC-25 §4, Section 13.3 §2).
7. **Rate limiting.** Ticker manual injection and RSS refresh are rate-limited per Section 10.7 §2, per connection and per command family.
8. **Backpressure.** Per-client send queues are bounded: telemetry is coalesced or dropped under pressure, command responses are never dropped, and overflow disconnects the client with a log — the same discipline the base prompt already requires of the bridge queue.
9. **Asynchronous persistence.** State persistence never performs a synchronous filesystem write on the command path: a dirty flag with debounce, forced on show-state transitions, or an append-only journal with periodic compaction.
10. **Correct crash-recovery state.** Recovery MUST NOT restore a show state that the restored data cannot support (for example `RUNNING` with no loaded package). Restore the version and last-known package identity, mark the state as recovered, drop to a loaded-pending-reload state, and require an explicit `show.load` to resume. The crash-during-live path is tested.

---

## 3. Spec gaps — not Prompt 02 work

These are recorded here so Prompt 02 does not silently paper over them. Each needs a spec revision or its own prompt; none is resolved by implementation.

| Gap | Detail |
|---|---|
| Nested `sequenceRef` | `Item.kind = "sequenceRef"` and `sequence.arm { sequenceId }` exist, but the manifest declares a single non-recursive `rundown` Sequence with no registry to resolve a `sequenceRef` against. Schema change is spec work (Standards §1.4). |
| No rate-limit error code | The Section 16 error registry has no rate-limit code, so Section 10.7 §2 flood protection has no normative failure mode. |
| Automation engine runtime | Sections 13.2–13.4 and AC-25 (trigger evaluation, once-per-frame limit, runtime cycle suppression) have no implementing prompt; only `automation.hold` is in Prompt 02. |
| TURN vending and guest-link revocation | Section 5.1 §11 and 9.6.2 vending, plus Section 10.7 §1 JWT `jti` revocation, have payload schemas but no implementing prompt. |
| `GET /nbe/v0.3/status` | Section 10.4 is required of the control plane; decision 1.3 gives it a home, but its response contract is otherwise unowned. |
| `nbe-migrate` | Named normatively in Prompt 01's locked error message (AC-28); no prompt builds it. |
| `crates/nbe-protocol` | Empty while the wire protocol is defined in TypeScript. Decide: mirror it (with an enum audit against the TypeScript command and error lists) or delete the crate. |
| `marker.add` recording chapters | Section 16.11 requires a recording chapter when the container supports it — an unstated handoff to Prompt 09. |

---

This addendum is normative for Prompt 02. The base prompt must reference it, and the implementation must not deviate from these decisions without a spec revision.
