# Agent Prompt 02c — Render Channel, Push Frames, and v0.3.2 Conformance (packages/control-plane)

**Targets: SPEC v0.3.2 (`docs/spec.v0.3.md`) — Sections 5.4.1 (server-push frames), 5.9 (the render channel), 10.1.1 (telemetry ownership), 10.7.1 (audit record shape), 16.0 (authorization matrix), 16.1 (`show.stop` quiescence, `show.start` warnings policy), 16.4 (`item.reset`), 9.6.2 (TURN derivation). Prerequisites: Agent Prompts 01, 02, and 02b merged.**

This prompt closes the gap between the control plane as built and SPEC v0.3.2 as written. It is the control-plane half of the render channel; **Prompt 03 cannot start until it lands**, because the render node it describes talks to behaviours that do not exist yet.

Read these first:

- `docs/spec.v0.3.md` — §5.4.1, §5.9, §10.1.1, §10.7.1, §16.0 are the normative contract for this prompt.
- `docs/implementation-standards.md` — the NBE Implementation Standards; this prompt must comply.
- `agents/prompts/02a-architecture-addendum.md` — historical record of how these decisions were reached. **The spec now supersedes it**; where they differ, the spec wins.
- `docs/prompt-01-definition-of-done.md` — Prompt 01 is locked; do not reopen its behaviours.

## Quality bar

Per the NBE Implementation Standards:

- **Schema-driven typed models (§1):** the new frames (`show.resync` directive payload, `resyncRequest`) and the new command (`item.reset`) join the existing zod registry and the round-trip test. The error registry gains `E_RATE_LIMITED` and must stay literal-for-literal with the §16 table.
- **Strict CI contracts (§2):** every behaviour below gets a test that fails for the right reason. Assertions are by value — exact `stateVersion`s, exact warning strings, exact error codes. A test that passes when the behaviour is absent is not a test.
- **Vocabulary discipline:** `View`, `Element`, `Sequence`, `Item`.

## Step 1: Server-push frames (SPEC §5.4.1)

The control plane currently emits telemetry but never emits state-change events, though Section 5.1 §5 has always required them.

- Broadcast exactly one `stateChange` frame per accepted command, after the `stateVersion` bump, to every connected non-`render` session: `{ v, kind: "stateChange", stateVersion, changed, state }`.
- The frame carries the same `stateVersion` as the command's success response and must be observable no later than that response.
- A rejected command emits nothing.
- Push frames are droppable: apply the same `bufferedAmount` backpressure rule already used for telemetry. Command responses are never dropped.
- `changed` lists the top-level state keys the command touched; `state` carries that subset.

## Step 2: `show.resync` (SPEC §5.9.4)

- On **every** render-role connection — first connect and every reconnect — send a `show.resync` directive **before any other directive on that connection**. Payload: `showState`, `viewItem`, `previewItem`, item states, scene states, visible overlays, `automationHold`, and the `stateVersion` the snapshot was taken at.
- `show.resync` is a directive-only command name. It is **not** a Section 16 command: no client may issue it, and it needs no payload schema in `CommandPayloadSchemas`.
- Handle the `resyncRequest` engine frame (`{ v, kind: "resyncRequest", reason }`) by sending a fresh `show.resync` on that connection.
- Directives issued while no render node was connected are **not** replayed. The snapshot is the recovery mechanism.

## Step 3: `appliedStateVersion` (SPEC §5.9.5)

`AppliedStateVersionFrameSchema` already exists in `src/protocol.ts` and parses; `src/server.ts` has no branch for it, so the frame is silently discarded today.

- Record the last applied `stateVersion` per render session, with its arrival time.
- Expose it to the status endpoint (Step 6) and to any waiter (Step 4).
- An `appliedStateVersion` older than one already recorded for that session is logged and ignored.

## Step 4: Make `show.stop`'s grace window real (SPEC §16.1, §5.9.5)

Today `waitForGrace` is an optional test seam that production never supplies, so the two-second window elapses instantly and the timeout branch is unreachable outside tests.

- Production wiring: after emitting the `record.stop` / `stream.stop` directives, wait up to 2 seconds for an `appliedStateVersion` ≥ the stop command's `stateVersion` from at least one connected render session.
- Acknowledged in time → graceful stop.
- Timed out, or no render node connected → force-stop and log exactly `show.stop: graceful output shutdown exceeded 2 s; force-stopping outputs`.
- The wait must not block other commands on other connections.

## Step 5: v0.3.2 command-surface conformance

1. **`item.reset`** (§16.4): `{ itemId }`, permitted from `DONE`/`MISSING`/`ERROR` → `READY`. An item marked unrecoverable stays `ERROR` and returns `E_FORBIDDEN_STATE`.
2. **`E_RATE_LIMITED`** (§16 registry): add the code and return it from the rate limiter instead of overloading `E_FORBIDDEN_STATE`. A rate-limited command must not mutate state and must not bump `stateVersion`.
3. **Authorization matrix** (§16.0): reconcile `roleAllowed` against the matrix. `plugin.reload` is admin-only and must move out of the operator set.
4. **`show.start` warnings policy** (§16.1): a warnings-only preflight (exit 1) refuses `show.start` with `E_FORBIDDEN_STATE` unless `allowWarnings: true` is passed. Loading stays permitted; going to air on warnings becomes an explicit decision. Do not change Prompt 01's exit codes or the meaning of `airReady`.

## Step 6: Status endpoint completeness (SPEC §10.4)

`GET /nbe/v0.3/status` must carry all seven required fields. It currently has five — master-clock state and render-node health are missing, though both are already available from the cached engine report.

## Step 7: Spec conformance repairs

1. **Telemetry `qualityProfile`** (§10.1.1) is control-plane-owned and hardcoded `null`; read it from the loaded manifest.
2. **Deprecation warnings** are drained by the first subscriber's tick, so with two subscribers only one ever sees the warning. Give each subscription its own cursor.
3. **TURN credentials** (§9.6.2): derive `username = <unixExpiry>:<guestId>` and `credential = base64(HMAC-SHA1(secret, username))` from a configured shared secret. With no secret configured, fail with `E_UNSUPPORTED_FEATURE` — never vend placeholder credentials.
4. **Audit log** (§10.7.1): if no destination is configured, refuse to start rather than run unaudited. Records already cover rejects and auth failures; add the failure `reason` field to rejected auth records.

## Step 8: Tests

Each of these must fail if its behaviour is removed:

1. `stateChange` broadcast: an accepted command produces exactly one frame, with the response's `stateVersion`; a rejected command produces none.
2. `show.resync`: a render session receives it as its **first** directive; a second render session connecting later also receives one; `resyncRequest` triggers a fresh one.
3. `appliedStateVersion`: recorded per session; a stale (lower) value is ignored.
4. `show.stop` grace: acknowledged within the window → graceful, no warning; unacknowledged → forced, with the exact warning string. Both branches, with a shortened window for the test.
5. `item.reset`: `DONE`/`MISSING`/`ERROR` → `READY`; unrecoverable `ERROR` → `E_FORBIDDEN_STATE`.
6. `E_RATE_LIMITED`: returned on flood, and `stateVersion` is unchanged after the rejection.
7. Authorization matrix: table-driven over §16.0 — every marked cell permitted, every unmarked cell `E_AUTH`. Include `plugin.reload` as operator-denied.
8. `show.start` on a warnings-only package: refused without the flag, permitted with it.
9. `/status` carries all seven §10.4 fields.
10. Per-subscriber deprecation cursor: two subscribers, one deprecated command, both ticks carry the warning.

**Strengthen the existing spec-conformance tests while you are here:** `protocol.test.ts` hand-copies the Section 16 command and error lists, so when the spec moved to v0.3.2 nothing went red — `item.reset` and `E_RATE_LIMITED` were missing from the code and every test passed. Parse the tables out of `docs/spec.v0.3.md` and diff against the registry. Same technique for the §17.3 transition table: assert every legal row executes and every pair outside the table is rejected, replacing the current single-path test.

## Step 9: CI

The `control-plane` job must additionally fail when:

1. The generated manifest types drift from the schema: `npm run gen:manifest-types` followed by `git diff --exit-code src/generated`.
2. The test run reports fewer than the expected number of tests (a glob that matches nothing must not pass).
3. `program`/`layer` appear outside the alias table and its tests.

## Constraints

- **Forbidden changes:** no schema edits (`schemas/manifest.v0.3.json` is normative and unchanged at v0.3.2); no changes to Prompt 01's exit codes, `airReady` semantics, or `PreflightReport` shape; no engine dependencies and no video in this package.
- Every command remains schema-validated before any state mutation, one `stateVersion` bump per accepted command, no mutate-then-throw.
- `tsc --noEmit` clean, all tests green, CI job passing.
- Where this prompt and `02a-architecture-addendum.md` differ, SPEC v0.3.2 wins.

## Reporting obligations

State: which spec sections are now implemented and tested; the spec-vs-code command/error diff (must be zero); the test count before and after; and any gap you chose not to close, with the reason.
