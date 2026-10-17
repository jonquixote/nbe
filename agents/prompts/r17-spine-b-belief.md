# Agent Prompt R17-B — Spine Part B: the Output Belief Follows the Truth (packages/control-plane)

**Targets: SPEC v0.4 at v0.4.8 (`docs/spec.v0.4.md`).**
- §10.1.3, belief precedence (row 10, ratified by PR #51, `c1fe348`).
- §13.2's field domains and transition rule (row 9, ratified by PR #50, `9e6239c`).
- §10.1.2, the operator-signal lane (row 8, PR #46, `be7a5d8`).
- §5.9.5, `show.stop`'s hold (row 6).
- §13.4.1, the command → trigger effect table.
- §16, the output commands' preconditions.

**Prerequisites:** PRs #46, #50 and #51 merged; `main` at `c1fe348`.

**Status: DRAFT, held for the user's review before dispatch** (the user's word of 2026-10-17: "nothing implements unread"). The decisions in §2 belong to the user. An executor does not start until they are answered and this line is struck (§2c).

You are a senior engineer on `nbe`, working in TypeScript in the control plane. R17 is "the control plane reports a refused stream as live" (`docs/soak-protocol.md` §5; `docs/prompt-map-07-13.md`, queue position 5).
- **Spine part A** (PR #46) made every contradiction *visible*: the lane announces it.
- **This prompt** makes the belief *honest*: the control plane's `streamState` follows §10.1.3's order, takes `held` where §5.9.5 holds a stop, and corrects itself from a fresh engine report.

R17's refusal lag is the proving case.

Read these first:
- `docs/spec.v0.4.md`: §10.1.3 entire; §10.1.2 entire; §13.2 (the field domains, their exceptions, "Transitions are reported as they are"); §5.9.5; §13.4.1's `show.stop`, `stream.start` and `stream.stop` rows; §16's `stream.start` and `stream.stop` rows.
- `docs/prompt-map-07-13.md`, queue position 5: the spine's design record, the witness ruling, spine B's spec row, the precedence section (RATIFIED), and the refused start (a).
- `docs/implementation-standards.md`: §2a (falsification; rules 7 and 8 bite here), §2b (CI read per test), §2c (strike, never rewrite), §3, §4.
- `VOCABULARY.md`: Output belief, Idle (output), Live (output), Recording (output), Held (output), Belief claim, Belief precedence, Field domain.

---

## 0. What is already true (verified at `main` `c1fe348`)

### 0.1 The ratified law this prompt builds — carried verbatim

§10.1.3's belief order is split at dispatch:
- **Authority, before dispatch:** an explicit operator action outranks an automation action, enforced by the hold (§13.5).
- **Belief, after dispatch:** the latest accepted command for the output (the operator's or automation's alike) holds the belief until its acknowledgement; then a fresh engine report that postdates that acknowledgement; then the stale belief.

**The precision clause, verbatim (signed off by the user 2026-10-17):**

> Window expiry ends the command's claim on belief and raises the expiry alert where §10.1.2's (b) raises one, and does nothing else. The alert announces the fallback; it does not act on the claim. Expiry is not an acknowledgement: Amendment 4's "the command's own ack" stays exact, and the alert never auto-clears at window end. No engine change, no wire change, no ratified row amended.

**The late-acknowledgement rule, verbatim (signed off by the user 2026-10-17):**

> An acknowledgement arriving after its window's unacknowledged expiry is inert to belief: logged where the alert-clear writes its audit row, not applied. The dead claim cannot reclaim the belief. It still clears the expiry alert by §10.1.2's ratified late-acknowledgement rule.

**Outside the clause** (§10.1.3):
- A command acknowledged inside its window, by its own acknowledgement or a later one, was confirmed, not expired.
- A command superseded inside its window gave its claim to the newer command at that acceptance. The user kept this case on 2026-10-17 as behavioural: "a handoff never leaves the truth ordering".
- A superseded command's late acknowledgement never reclaims the belief. It is also **not logged**: the lane keeps only the newer entry per output, and an acknowledgement below that entry's version is skipped (`alerts.ts:198–199`). This is the same unlogged class as the `engine.notReporting` case under the scoped wording.

**The transition rule (§13.2, row 9):**
- A `stateChange` carries the field's actual values, once. live→held fires `{ from: "live", to: "held" }`, never `"idle"`, with no alias.
- A rule written `to: "idle"` does not match a hold. A rule meaning "left the air" is written `from: "live"`.

**`held`'s meaning** (the user's ratification of 2026-10-16): §5.9.5's limbo, a stop commanded with the output's state unknown. It is not a pause feature; an operator-initiated hold is new behaviour, outside this prompt. `recordState` has no `held` (§13.2 exception 2, owner work order 2).

### 0.2 What the tree does today

**Belief is set only by commands.**
- `stream.start` → `live` (`commands/output.ts:36`); `stream.stop` → `idle` (`:45`).
- `show.stop` sets both outputs `idle` only on its exact acknowledgement (`commands/show.ts:162–165`). Without one, its `afterForward` leaves them **at their prior state**: "reconciling from the engine's report is R17's" (`:166–170`).
- Nothing corrects a belief from a report.

**The domain already admits `held`. The type does not yet.**
- `STREAM_STATES` (`automation.ts`, `nbe_core::automation`) is `idle | live | held`.
- `StreamState` (`state.ts:17`) is `idle | live`.
- The compile-time audit `streamAudit` (`automation.ts`) pins the domain to `StreamState | "held"`. Removing that `| "held"` is this prompt's, and `tsc` will hold it.

**The engine's account.**
- `world.last` (`server.ts:248`) is the latest engine telemetry.
- The lane's freshness is `freshReport()` (`alerts.ts`, §10.1.1's threshold).
- The anchor is `streamAnchor`, moved by the render node's registration (`onRegistered`, `alerts.ts:150–154`), by a resync acknowledgement, and by a command's processing (`onApplied`, `:188–212`).

**The lane's mapping already defines consistency** (`alerts.ts:279–281`): belief `live` is consistent with `live` and `reconnecting`, and belief `idle` with `none` and `closed`. The `reconnecting` pairing is §10.1's survival shape: the belief stays `live` through a redial.

**A refusal is never acknowledged.** `directive.rs:132` returns on the handler's error before `set_last_applied` and `ack` (`:147–148`), and `channel.rs:265` only logs `directive failed`. So (b)'s window is where a refused command's claim ends (§10.1.3).

**The detection-transaction precedent** (`server.ts:904–925`, the engine's `itemEvent`):
1. mutate;
2. call `state.bump()` once;
3. `persistence.onDirty()`;
4. write an audit row (`kind: "command"`, `command: "engine:<event>"`);
5. fire automation's `stateChange` events from the before and after snapshots.

It does **not** broadcast a `stateChange` frame. Only accepted commands do (`server.ts:404`, §5.4.1).

**Every reader of the belief values, which `held` will meet:**
- `state.ts:231`, `outputsActive()`: `recordState === "recording" || streamState === "live"`.
- `server.ts:408`: `show.stop` raises the lane's held alert only `if (state.outputsActive())`.
- `alerts.ts:267`: the held alert heals when `!state.outputsActive()`.
- `alerts.ts:278–281`: (a)'s divergence reads `believed === "live"` / `"idle"`.
- `commands/output.ts:35`: `stream.start` refuses `streamState === "live"` ("stream already live").
- `commands/output.ts:44`: `stream.stop` refuses `streamState !== "live"` ("stream not live"). This is the held stop's own heal path.
- `commands/show.ts:135`: `show.stop` reads `outputsActive()`.
- §16 (`spec.v0.4.md:4095`): `stream.stop`'s precondition is "stream active".

**The tests that will move:**
- `dress-rehearsal.test.ts:909–911`: on CI the engine refuses `stream.start`, and the dress then sends `stream.stop` expecting `ok`. Once the belief self-corrects to `idle`, that `stream.stop` is a race against the correction.
- `operator-signals.e2e.ts`: the refused-start, lost-stop, held and restart legs.
- `show-stop-version.e2e.ts`'s held leg.
- `alerts.test.ts`, `alerts-server.test.ts`.
- `automation.test.ts` (the domain audit and the transition rules).

---

## 1. Goals

1. **`held` becomes a belief value.** Where §5.9.5 holds an unacknowledged `show.stop`, a stream that was `live` becomes `held`. A recording stays `recording` (exception 2). `StreamState` gains `held`, and `streamAudit` pins the domain to the type exactly.
2. **A claim ends at its window.** An output command's claim on the belief ends at its acknowledgement, or at (b)'s window ending without one. The ending is not an acknowledgement, and it does nothing else (the clause).
3. **A fresh report corrects the belief.** With no live claim, a fresh engine report that postdates the anchor moves `streamState` to the value consistent with it (0.2's mapping, D1), as a detection transaction (D4).
4. **A late acknowledgement is inert to belief.** It is logged only where the alert-clear writes its row, and it never reclaims.
5. **Transitions are reported as they are.** live→held fires once with the actual values. `to: "idle"` does not match a hold, and `from: "live"` does.
6. **R17 closes on its proving case.** A refused `stream.start` reads `live` until its window ends, then follows a fresh report to `idle`. The lane's (b) alert stays until §10.1.2's own clearing rules.

---

## 2. Decisions owed (the user's, before dispatch)

- **D1 — The report → belief mapping.** Proposed from the tree's consistency pairs (`alerts.ts:279–281`): transport `live` or `reconnecting` → `live`; `none` or `closed` → `idle`. A fresh `none` after a refused start reads `idle`.
- **D2 — `held` and a fresh report.** §5.9.5's limbo is "output state unknown". When a fresh report postdating the hold arrives, does the belief leave `held` for the mapped value (the state is now known), or stay `held` until the operator's explicit stop (today's heal)? The first follows §10.1.3's tiers literally; the second keeps `held` as an operator-facing state.
- **D3 — `held` in the preconditions and readers.** `stream.stop` must accept `held`, because it is the heal; otherwise the operator cannot clear a hold. Does `stream.start` refuse `held`, or treat it as not-live? Does `outputsActive()` count `held` as active, so that the held alert raises and does not self-heal? Each answer moves §16's precondition text and §13.4.1's rows.
- **D4 — How a correction lands.** Proposed: the `itemEvent` precedent (0.2), meaning one `bump()`, one audit row and automation's `stateChange` events, with no `stateChange` frame (§5.4.1: one frame per accepted command). Name the audit event (proposed `belief.corrected`, with its from, to and the report's token and age).
- **D5 — The spec delta.** §5.9.5's "holds the outputs at their prior state" becomes "the stream belief takes `held`" (row 9's exception 1 already names it). This prompt drafts its own spec row (v0.4.8 row 11) for D1–D4's answers, ratified on its merge. The clause's "no ratified row amended" binds window expiry, not this prompt's whole scope: name every amended passage §2c-visibly.
- **D6 — The dress race.** `dress-rehearsal.test.ts:911`'s `stream.stop` after a refused start: accept `E_FORBIDDEN_STATE` once the belief reads `idle`, or assert the belief's correction itself? The executor proposes; the user rules.

---

## 3. Quality bar

- Every guard enters the path it claims (§2a rule 7). Every new suite has two floors, ran and exercised (rule 8). Every guard is falsified, restored `cmp`-identical, and rebuilt where Rust is touched (this prompt expects none).
- Belief, lane and audit agree on every row of 0.2's mapping, on the real engine where the hardware allows. Remember that CI refuses the encoder, so the refused start exercises there, and the other legs exercise on the normative machine.
- No timing claim from a shared runner (R9). The window is §5.9.5's 2000 ms. Measure the correction's latency (window end → next fresh report → belief) on the normative machine, under the load and power rules.

## 4. Scope discipline

**In scope:**
- `packages/control-plane`: `state.ts`, `commands/output.ts`, `commands/show.ts`, `server.ts`, `alerts.ts`, `automation.ts`;
- tests and the CI floors;
- the spec row (D5), the prompt map, and `VOCABULARY.md` if a term moves.

**Out of scope:**
- the engine, the wire, and every Rust crate (the clause);
- `recordState`'s `held` (work order 2);
- engine refusal acknowledgements (a hardening candidate, after work order 2);
- the `equals` domain row (spine B's named follow-up);
- an operator-initiated hold (new behaviour, its own promotion path);
- output state on the resync snapshot (work order 2).

## 5. Work items

1. **Answer-dependent design, written first** (into the prompt map, position 5): D1–D6 as ruled, the correction's code path, and the audit event. Stop for the user's word if the tree contradicts a ruling (§4a).
2. **`held` in the type and the hold.**
   - `StreamState` gains `held`, and `streamAudit` drops its exception.
   - `show.stop`'s unacknowledged path writes `held` for a `live` stream, in the same command.
   - Every reader in 0.2 is handled per D3.
3. **Claims and windows.** Model the per-output claim beside the lane's pending entry, without changing the lane's alerting (row 8 is ratified):
   - the claim ends on the acknowledgement (§10.1.2's (b) proof) or at window end;
   - it passes to a newer command at acceptance;
   - a late acknowledgement never revives it.
4. **The correction.** With no live claim on the stream, a fresh report postdating the anchor whose token disagrees with the belief (D1) lands as D4. Belief, lane and audit move together. (a)'s `stream.divergence` clears by its own rule once the pair is consistent.
5. **Automation.** The transition rule holds end to end: a `to: "idle"` rule is silent on a hold; a `from: "live"` rule fires once on live→held and once on a correction live→idle; nothing fires twice.
6. **The spec row** (D5), drafted with the code, ratified on merge.

## 6. Tests required

Name every test; give each guard its own falsification. Every falsified test carries its mutation and its signature in the prompt map.

- **Unit (`alerts.test.ts` or a new `belief.test.ts`):**
  - every row of D1's mapping, with its negative;
  - claim end by exact acknowledgement, by a later acknowledgement, by window expiry, and by supersession;
  - the late acknowledgement inert to belief (it still clears (b) by row 8's rule, logged there and nowhere else);
  - `engine.notReporting` owning the window end (the claim expires; with no fresh report the stale belief stands).
- **Server (`alerts-server.test.ts` or a new suite):** a correction is one `bump()`, one audit row and no `stateChange` frame (D4); the held path writes `held` and raises the held alert (D3).
- **Automation (`automation.test.ts`):**
  - `to: "idle"` silent on a hold;
  - `from: "live"` fires once on a hold and once on a correction;
  - `to: "held"` now fires (the domain admitted it before any writer).
- **End to end, on the real engine:**
  - the refused start (CI): `live` until the window, then `idle` from a fresh report, the (b) alert standing;
  - the held stop (normative machine): `held`, then healed by D3's path;
  - the lost stop and the restart legs, with their beliefs.
- **The dress rehearsal:** D6's ruling, asserted.

## 7. What this prompt must NOT do

- Change the engine, the wire, or any Rust crate.
- Amend row 8's alerting, clearing rules or audit rows.
- Treat window expiry as an acknowledgement. Auto-clear an alert at window end. Let a late acknowledgement move the belief.
- Give `recordState` a `held`. Alias `held` to `idle`, or fire a transition twice.
- Edit an existing test's expectation without recording why, §2c-visibly, in the prompt map.

## 8. Done means

- D1–D6 answered and built.
- The spec row is drafted.
- Every guard is falsified.
- The gate passes on the normative machine (load and power rules; voids named).
- CI is read per test, with every floor held.
- R10's `[ci-load]` lines are read against the derived ceiling.
- R17's register row is closed by its proving case, with the evidence quoted.

## Report (what the executor's done-message must contain)

1. The PR number and head SHA.
2. D1–D6 as built.
3. The belief/lane/audit table for every mapping row, from the real engine.
4. The falsification table.
5. The gate with every window's load.
6. CI per test.
7. The correction's measured latency.
8. R17's proving run, quoted.

## Constraints (carried)

- **Forbidden:** engine, wire or Rust changes; any amendment to row 8; window expiry acting as an acknowledgement; scope growth into work order 2, the `equals` row or refusal acks.
- **New tests required** (§6), with their floors.
- **CI changes required:** the new suites' floors.
