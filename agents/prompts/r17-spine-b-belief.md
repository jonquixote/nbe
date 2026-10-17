# Agent Prompt R17-B — Spine Part B: the Output Belief Follows the Truth (packages/control-plane)

**Targets: SPEC v0.4 at v0.4.8 (`docs/spec.v0.4.md`).**
- §10.1.3, belief precedence (row 10, ratified by PR #51, `c1fe348`).
- §13.2's field domains and transition rule (row 9, ratified by PR #50, `9e6239c`).
- §10.1.2, the operator-signal lane (row 8, PR #46, `be7a5d8`).
- §5.9.5, `show.stop`'s hold (row 6).
- §13.4.1, the command → trigger effect table.
- §16, the output commands' preconditions.

**Prerequisites:** PRs #46, #50 and #51 merged; `main` at `c1fe348`.

~~**Status: DRAFT, held for the user's review before dispatch** (the user's word of 2026-10-17: "nothing implements unread"). The decisions in §2 belong to the user. An executor does not start until they are answered and this line is struck (§2c).~~

**Status: APPROVED 2026-10-17, with the user's rulings on D1–D6 folded into §2** (§2c). **Dispatch is the user's word.**
- One question is still open (§2, D2a). It must be answered before dispatch, because the work items depend on it.
- Appendix A holds SPEC row 11's candidate text. It is reviewed before dispatch (D5), lands in the spec with the implementing PR, and is ratified on that merge.

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

**Unlogged late acknowledgements are a named hardening candidate** (the user's word of 2026-10-17), beside engine refusal-acks: "same class, terminal outcomes dropped on the floor". The current behaviour is correct under the ratified semantics. The evidence gap is recorded, not patched, and it is out of this prompt's scope.

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
3. **A fresh report corrects the belief.** With no live claim, a fresh engine report that postdates the anchor moves `streamState` to D1's value, as a detection transaction (D4). There is one bump and one `belief.corrected` audit row, with no `stateChange` frame and no automation trigger.
4. **A late acknowledgement is inert to belief.** It is logged only where the alert-clear writes its row, and it never reclaims.
5. **Transitions are reported as they are.** live→held fires once with the actual values. `to: "idle"` does not match a hold, and `from: "live"` does. Automation's `stateChange` fires on command-path transitions only: acceptance, acknowledgement and window expiry. It never fires on a correction (D4).
6. **R17 closes on its proving case.** A refused `stream.start` reads `live` until its window ends, then follows a fresh report to `idle`. The lane's (b) alert stays until §10.1.2's own clearing rules.

---

## 2. ~~Decisions owed (the user's, before dispatch)~~ Decisions, ruled by the user on 2026-10-17 (§2c)

The proposals this brief carried before the rulings are superseded. Where a ruling differs from them, the ruling stands. Each ruling is quoted, followed by what it requires of the tree.

- **D1, the report → belief mapping: RATIFIED as proposed.** "live/reconnecting → live; none/closed → idle. reconnecting→live keeps belief stable through blips, matching the lane's blip-quiet semantics. The mapping table lands in row 11, not just in code."
  - *The tree:* this is the lane's consistency pairs (`alerts.ts:279–281`).
  - *Required:* the table in row 11 (Appendix A), and one test per row with its negative.

- **D2, `held` and a fresh report.** "held gives way to a fresh report on the precision clause's schedule: the stop's claim stands inside the window; at expiry the claim ends and a fresh report rules. Belief must never insist on a state the engine contradicts after the claim has expired. The alert persists until the real ack, as the witness."
  - *The sequence this builds* (the builder's reading of D2 with D4; confirm at review):
    1. inside `show.stop`'s window, the stop's claim holds;
    2. at expiry, a `live` stream's belief becomes `held` on the command path (D4), and automation sees live→held;
    3. the next fresh report that postdates the expiry corrects `held` to D1's value, as a correction: no automation.

    While the engine is reporting, `held` therefore lasts until that report, about one tick. It lasts longer only while no fresh report arrives (`engine.notReporting`).
  - *The tree today:* the held alert heals when the belief reads outputs inactive (`alerts.ts:267`, "when the operator's explicit stops land"; row 8's guard F22). D2 replaces that with "until the real ack". §10.1.2 states no clearing rule for `outputs.held`, so row 11 must state the new one.
  - **D2a, OPEN, owed before dispatch: the alert can be stranded.** Suppose `show.stop`'s acknowledgement never comes, because it was withheld after a failed quiesce, or lost. A fresh `closed` report then corrects `held → idle`, and `stream.stop` refuses `idle` (D3, D6). No real acknowledgement can arrive, so the held alert has no clearing path. Two questions are the user's:
    - which acknowledgements count as "the real ack": `show.stop`'s own late one, or also the acknowledgement of an operator's `stream.stop`/`record.stop` that ends the hold?
    - what clears the alert when none can come?

- **D3, `held` is an output-active state.** "outputsActive() includes held; stream.stop accepts held — the operator's only way out of a hold; stream.start refuses held, naming the state in the error. Every other reader and precondition names held explicitly, no default fall-through; the prompt audits every belief-enum consumer, and #50's compile-time enum audit enforces exhaustiveness."
  - *The tree:* #50's audit (`automation.ts`'s `Same<…>`) pins each **domain** to its **type** exactly. That catches a value added to one but not the other. It does not make the **consumers** exhaustive.
  - *Required:* every belief-enum consumer handles `idle`, `live` and `held` explicitly, in an exhaustive `switch` whose default is a `never` check, so `tsc` enforces exhaustiveness at each consumer and the audit enforces the domain. The consumers are 0.2's list: `outputsActive()`; `server.ts:408`; `alerts.ts:267` and `:278–281`; `commands/output.ts:35` and `:44`; `commands/show.ts:135`. Any found beyond it are added, and the prompt map records the full list.
  - **`stream.start`** refuses `held` with `E_FORBIDDEN_STATE`, naming the state ("stream is held"). **`stream.stop`** accepts `live` and `held`, and refuses `idle` naming the state. §16's "stream active" reads "`live` or `held`" in row 11.

- **D4, how a correction lands: RATIFIED, with the boundary stated.** "corrections land like itemEvent — one version bump, one audit row (belief.corrected), no stateChange frame. stateChange fires on command-path belief transitions (acceptance, ack, window expiry), not on corrections. Automation reacts to committed intent, not environmental reconciliation. The live→held rule stands: window-expiry driven, command path."
  - *The tree:* `itemEvent` **does** fire automation's `stateChange` events from its snapshots (`server.ts:924`). A correction deliberately does not: like `itemEvent` in its bump, its audit row and the absence of a frame; unlike it for automation.
  - *A consequence, recorded:* a refused start corrected `live → idle` fires no `from: "live"` rule. Automation reacts to the stream leaving the air only on the command path.
  - *Required:* `belief.corrected`'s audit detail carries the field, `from`, `to`, the report's token and the report's age.

- **D5, row 11: required.** "drafted with the implementing prompt's design and reviewed before dispatch, same discipline as this brief: §5.9.5's 'prior state', D1's mapping table, D4's frame and boundary."
  - *Done:* Appendix A, the candidate text, is drafted here for review. The executor lands it in `docs/spec.v0.4.md`, with §2c strikes, in the implementing PR, and row 11 is ratified on that merge.
  - It is not ratified ahead of the code. Unlike row 10, row 11 amends passages that the tree builds today (`show.stop`'s prior-state hold, §16's preconditions), so ratifying it early would make the spec contradict the built behaviour.

- **D6, the dress rehearsal.** "the dress does not hide the race; it rehearses the new truth. After a refused start it waits out the correction to idle (bounded), then asserts stream.stop refuses with the idle-state error — the race becomes a falsification of D3's precondition. Each leg records which path it took."
  - *Required:*
    - `dress-rehearsal.test.ts`'s refused-start branch (`:909–911`) waits, bounded, for the belief to read `idle`;
    - it asserts that `stream.stop` returns `E_FORBIDDEN_STATE` with the idle-state error;
    - every leg logs the path it took (exercised stream, or refused start with its correction latency).
  - The bound is §5.9.5's window plus two telemetry ticks. That is a backstop, not a timing claim (R9).

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
- engine refusal acknowledgements, and the logging of unlogged late acknowledgements (both named hardening candidates, the same class; after work order 2);
- the `equals` domain row (spine B's named follow-up);
- an operator-initiated hold (new behaviour, its own promotion path);
- output state on the resync snapshot (work order 2).

## 5. Work items

1. **The design, written first** (into the prompt map, position 5): D1–D6 as ruled, D2a's answer, the correction's code path, `belief.corrected`'s detail, and the full consumer list (D3). Stop for the user's word if the tree contradicts a ruling (§4a).
2. **`held` in the type and the hold.**
   - `StreamState` gains `held`, and `streamAudit` drops its exception.
   - `show.stop`'s unacknowledged path writes `held` for a `live` stream, in the same command.
   - Every belief-enum consumer is an exhaustive `switch` with a `never` default (D3): `outputsActive()` includes `held`; `stream.start` refuses `held`; `stream.stop` accepts `live` and `held`.
3. **Claims and windows.** Model the per-output claim beside the lane's pending entry, without changing the lane's alerting (row 8 is ratified):
   - the claim ends on the acknowledgement (§10.1.2's (b) proof) or at window end;
   - it passes to a newer command at acceptance;
   - a late acknowledgement never revives it.
4. **The correction.** With no live claim on the stream, a fresh report postdating the anchor whose token disagrees with the belief (D1) lands as D4. Belief, lane and audit move together. (a)'s `stream.divergence` clears by its own rule once the pair is consistent.
5. **Automation.** The transition rule holds end to end: a `to: "idle"` rule is silent on a hold; a `from: "live"` rule fires once on live→held and once on a correction live→idle; nothing fires twice.
6. **The spec row:** land Appendix A in `docs/spec.v0.4.md`, with §2c strikes, adjusted only by D2a's answer and by what the code proves. Ratified on merge.
7. **The dress rehearsal** (D6).

## 6. Tests required

Name every test; give each guard its own falsification. Every falsified test carries its mutation and its signature in the prompt map.

- **Unit (`alerts.test.ts` or a new `belief.test.ts`):**
  - every row of D1's mapping, with its negative;
  - claim end by exact acknowledgement, by a later acknowledgement, by window expiry, and by supersession;
  - the late acknowledgement inert to belief (it still clears (b) by row 8's rule, logged there and nowhere else);
  - `engine.notReporting` owning the window end (the claim expires; with no fresh report the stale belief stands).
- **Server (`alerts-server.test.ts` or a new suite):** a correction is one `bump()`, one audit row and no `stateChange` frame (D4); the held path writes `held` and raises the held alert (D3).
- **D3, exhaustiveness:** each consumer's `never` default is falsified by adding a fourth `StreamState` value, which must fail `tsc` at every consumer. `stream.start` refuses `held` with the state named; `stream.stop` accepts `held` and refuses `idle`.
- **D2:** `held` lasts until the next fresh report postdating the expiry, then corrects to D1's value; the held alert persists until the real acknowledgement (D2a as ruled).
- **D4:** a correction fires no automation `stateChange`. A `from: "live"` rule stays silent on a refused start's correction and fires on the command path's live→held.
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
- Fire an automation `stateChange` on a correction (D4), or broadcast a `stateChange` frame for one.
- Leave a belief-enum consumer with a default branch that absorbs `held` (D3).
- Edit an existing test's expectation without recording why, §2c-visibly, in the prompt map.

## 8. Done means

- D1–D6 built as ruled (§2), and D2a as the user answers it.
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

---

## Appendix A — SPEC v0.4.8 row 11, candidate text (D5)

This text is reviewed before dispatch. The executor lands it in the implementing PR, and it is ratified on that merge. `[D2a]` marks the one clause that waits on the open question.

**The table row (v0.4.8):**

> | 11 | **The belief follows the truth: `held` as §5.9.5's limbo, correction from a fresh report, the boundary for automation** (the user's rulings of 2026-10-17 on R17 spine B's brief, D1–D6; queue position 5). §10.1.3 ratified the order the belief follows (row 10) ahead of its implementation. This row builds it. An unacknowledged `show.stop` gives a `live` stream the belief `held` at its window's expiry, in place of its prior state. With no live claim, a fresh engine report that postdates the anchor corrects the stream belief by §10.1.4's mapping (`live`/`reconnecting` → `live`, `none`/`closed` → `idle`). The correction lands as a detection transaction: one version bump and one `belief.corrected` audit row, with no `stateChange` frame and no automation trigger. Automation reacts to command-path transitions only. `held` is an output-active state: `stream.stop` accepts it, `stream.start` refuses it by name, and every consumer is exhaustive. The held alert persists until a real acknowledgement [D2a] | 5.9.5 (the "prior state" hold, struck and amended), 10.1.4 (new), 13.2 (the transition rule, annotated: corrections fire no trigger), 13.4.1 (`show.stop`'s row, annotated: `held`), 16 (`stream.start` and `stream.stop`'s preconditions) | the implementing PR's guards (§6 of the brief) |

**§5.9.5, struck and amended (§2c):** ~~"holds the outputs at their prior state"~~. The rule becomes: a `live` stream's belief becomes `held` at the window's expiry (§10.1.3's command path), and the recording stays `recording` (§13.2 exception 2). The next fresh report that postdates the expiry corrects `held` by §10.1.4.

**§10.1.4 Belief correction (normative, new in v0.4.8 row 11):**

> **The mapping.** With no live claim on the stream (§10.1.3), a fresh engine report (§10.1.1) that postdates the anchor (§10.1.2's (a)) sets `streamState` to:
>
> | Engine `streamTransportState` | Belief |
> |---|---|
> | `live` | `live` |
> | `reconnecting` | `live`: a redial is §9.5's survival shape, and the belief stays stable through a blip, as the lane does |
> | `none` | `idle` |
> | `closed` | `idle` |
>
> **The landing.** A correction is a detection transaction: one `stateVersion` bump and one audit row, `belief.corrected`, whose detail is the field, `from`, `to`, the report's token and the report's age. It broadcasts no `stateChange` frame (§5.4.1: one per accepted command) and fires no automation `stateChange` trigger. Automation reacts to committed intent, through command-path transitions: acceptance, acknowledgement and window expiry. A rule meaning "the stream left the air" (`from: "live"`) therefore fires on live→held and on an accepted `stream.stop`, and never on a correction.
>
> **`held` gives way on the precision clause's schedule.** The stop's claim stands inside its window. At expiry the claim ends, and a fresh report rules. The belief never insists on a state the engine contradicts after the claim has expired. The `outputs.held` alert persists until a real acknowledgement, as the witness [D2a: which acknowledgements count, and what clears the alert when none can come].
>
> **`held` is output-active.** `stream.stop` accepts `live` and `held`; `stream.start` refuses `held` with `E_FORBIDDEN_STATE`, naming the state. Every consumer of the belief enum handles `idle`, `live` and `held` explicitly, with no default fall-through.

**§13.2, annotated:** "Transitions are reported as they are" gains: *(v0.4.8 row 11: on the command path. A correction (§10.1.4) fires no trigger.)*

**§13.4.1, annotated:** `show.stop`'s row gains: *(v0.4.8 row 11: unacknowledged, a `live` stream's belief becomes `held`, not its prior state.)*

**§16:**
- `stream.stop`'s precondition "stream active" reads *stream active (`live` or `held`)*.
- `stream.start`'s preconditions gain *the stream not `held`*.
