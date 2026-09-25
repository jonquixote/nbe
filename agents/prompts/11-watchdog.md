# Agent Prompt 11 — The Automation Engine Runtime, and the Watchdog's Remainder (crates/nbe-engine, packages/control-plane)

**Targets: SPEC v0.4 at v0.4.5 (`docs/spec.v0.4.md`) — Section 13 (automation engine), AC-25 (automation), Section 10.3 (watchdog), Section 10.5 and AC-27 (degradation ladder), Section 10.1 (telemetry), Section 10.7 (audit), AC-7 (fallback latency). Prerequisites: Prompts 01–10 merged (Prompt 10 merged 2026-09-25 as PR #30, `57dd4b9`).**

**Upgraded for the tree, 2026-09-25.** The 2026-09-10 draft (superseded text in §12) asked for a watchdog the tree already has, cited two prompt files that do not exist, and forbade the one placement AC-7 requires. The prompt map had moved this slot's real work to the automation runtime on 2026-09-04, before the draft existed (`docs/prompt-map-07-13.md` § 11: *"The watchdog itself exists and is gated … What 11 must now add is the automation engine runtime (§13, AC-25), assigned by [RI-5]"*). This upgrade makes the prompt say so, states what is already true, and names the decisions that belong to the user before an executor starts.

You are a senior engineer on `nbe`. Rust for the engine, TypeScript for the control plane. This prompt builds the runtime that turns `trigger + conditions → command` rules into commands on the command bus, and closes the small remainder of the watchdog/ladder work that has a subject in the tree today.

Read these first:

- `docs/spec.v0.4.md` — §13 (the rule model, triggers, execution semantics, cycle detection, hold), AC-25, §10.3, §10.5, AC-27, AC-7.
- `docs/implementation-standards.md` — §2a rules 1–8; rule 7 (a test must enter the path it claims) and rule 8 (floors count ran and exercised) bite hardest here.
- `docs/prompt-map-07-13.md` § 11 and the "Still owed" list under § 10.
- `docs/v0.5-outline.md` §7 — two UNRATIFIED wire candidates this prompt depends on.
- `VOCABULARY.md` — `View`, `Element`, `Sequence`, `Item`.

---

## 0. What is already true (verified at `main` `57dd4b9`)

### 0.1 The watchdog is built, on the render loop, and gated

`crates/nbe-engine/src/watchdog.rs`: a consecutive-miss counter; `render.rs` constructs it with threshold 2 and reports every frame — `frames_missed = ceil(late / budget)` when the View misses its deadline, `0` otherwise. Crossing the threshold logs loudly and sets `fallback_active` (the fallback slate, §7.14). Tests in `prompt03`, `prompt04`, `prompt06` and `integration` gate it; the midpoint review's pass 4 confirmed deadline accounting and the fallback trip both fail when deleted.

It runs on the render loop **on purpose**: AC-7 requires the cut to the fallback slate "no later than one frame after the missed deadline", and only the loop knows the deadline was missed in time to act on the next frame. The 2026-09-10 draft's "Forbidden: any watchdog work on the render thread" contradicts AC-7 and the built code; it is struck (§12).

**One question to settle, not assume (WU5):** §10.3 says the watchdog acts when the loop "misses a deadline by more than 1 frame". The built rule accumulates `ceil(late / budget)` and trips when the sum exceeds 2 — so one frame late by 1.5 budgets (`frames_missed` 2) does not trip it. Read the gating tests and §10.3 together and decide whether the threshold matches the text; record the answer either way.

### 0.2 The degradation ladder has one rung

`render.rs` `update_rung`: two consecutive late View frames → `Rung::PreviewHalfRate` (Preview renders every other frame); thirty consecutive on-time frames → `Rung::Nominal`. That is §10.5 rung 1 with hysteresis, and `degradationRung` is on the §10.1 tick. The View is never degraded.

§10.5's rungs 2–4 and AC-27 items 2–4, against the tree:

| Rung | Subject in the tree | Status |
|---|---|---|
| 2 — loop caches evict to streaming | loops already have a streaming mode (`video.rs`: "when streaming it is the read-ahead window") | **buildable** — a mechanism exists to degrade to |
| 3 — effect quality | no effect pipeline (§14 plugins unbuilt) | **no subject** |
| 4 — multiview tiles | `multiview_mask` is always `None`; no multiview output exists | **no subject** |

### 0.3 What the ladder decides on — and the stubs beside it

The ladder's input is **wall-clock View lateness**, measured on the loop. `renderGpuTimeMs`, `vramUsedMib` and `textureCacheUsedMib` are 0.0 stubs on the §10.1 tick (`telemetry.rs`). The prompt map's S1 note ("the ladder is currently deciding on a constant") is therefore half right: the *telemetry field* is a constant; the *ladder* decides on lateness. Correct the record, do not build GPU timing here unless USER CHOICE C1 says so (it is Prompt 12's measurement problem first).

The quality-profile probe (`gpu.rs` `probe_quality`) is a heuristic: a CPU adapter → `potato`, anything else → `consumer`. `pro` and `reference` are never selected.

### 0.4 Automation: the model exists, the runtime does not

- **Schema and types.** `manifest.automation: [AutomationRule]` — `{ id, trigger: { kind, params? }, conditions?: [ {…} ], action: { command, payload? }, enabled = true }`, the nine §13.2 trigger kinds as an enum. Typed in `nbe-core` (`manifest.rs`: `AutomationRule`, `AutomationTrigger`, `AutomationTriggerKind`, `AutomationAction`).
- **Commands.** `automation.enable` / `automation.disable` `{ ruleId }` (control plane: `E_NOT_FOUND` for an unknown rule; toggles `automationRules`), `automation.hold { hold }` (sets `automationHold`, which is on the §10.1 tick). `nbe-preflight` knows all three command names and their required fields.
- **Audit.** `audit.ts` reserves `kind: "automation"` for automation actions (AC-25 §4); nothing writes it.
- **Missing:** an evaluator; every trigger adapter; conditions evaluation; the once-per-frame limiter (§13.3 #3); runtime self-trigger suppression (§13.4); **preflight cycle detection** (§13.4 — nothing in `nbe-preflight` or `nbe-core` validates rules); hold's "within 1 frame" guarantee (§13.5, AC-25 #2).
- **`autoFollow` is normative since v0.1 and unimplemented.** Items carry it (`nbe-core` `auto_follow`, control-plane `state.ts`); nothing advances on media end. §3.1's time-axis table lists it as "subsumed by Automation", and §13.5 #2 requires hold to suppress it — so it lands here.

### 0.5 Trigger sources, against the tree

| §13.2 trigger | Source today | Status |
|---|---|---|
| `mediaEnd` | engine `itemEvent { event: "end" }` (`nbe-protocol` `ItemEvent::End`) reaches the control plane | **wired source** |
| `mediaStart` | no engine "started" event; the control plane knows when a take applies (`appliedStateVersion`) | **needs a definition** (B3) |
| `timer` | the show clock | available |
| `timeOfDay` | wall clock | available |
| `audioLevel` | `busPeakDbfs` only on the **1 Hz** telemetry tick | **cannot meet AC-25 #1** (B1) |
| `hotkey` | keyboard adapter + Companion profiles (`keyboard.ts`, intent `Hotkey` trigger kind) | available |
| `rssKeyword` | ticker RSS refresh (`commands/ticker.ts` `ticker.refreshRss`) | available (per refresh) |
| `streamHealth` | transport state (`PublisherState`) is engine-internal; the wire's `streamState` is commanded and stays `live` through a redial | **blocked on a wire decision** (B2) |
| `stateChange` | the control plane's state machine | available |

---

## 1. The scope question — USER CHOICE C1

The slot carries two things with very different sizes. **Recommendation:**

- **In:** the automation engine runtime (§13, AC-25) including `autoFollow`; the watchdog remainder that has a subject — §10.3's threshold question (0.1), and ladder rung 2 (loops evict to streaming) with its AC-27 item.
- **Out, recorded:** ladder rungs 3–4 (no effect pipeline, no multiview output — their owners are §14 and whichever prompt builds multiview); GPU timing and the other §10.1 stubs (Prompt 12's measurement work; the Intel/AMD reference machine is where those numbers mean something).

Alternative: split into 11a (automation) and 11b (watchdog remainder). The split costs a second review cycle for ~a day of work; the recommendation keeps one prompt with two gated work units.

## 2. Design gate G1 — where the rule engine runs

**Answer to defend, not assume: the control plane.** §13.1 makes a rule's action "any command-bus command" facing "the same preconditions as a human operator's commands"; the command bus, its preconditions, the audit log (§10.7) and `automationHold` all live in the control plane. An engine-side evaluator would need a second dispatch path and a second audit writer — two chances to disagree. Engine-originated triggers arrive as engine frames (`itemEvent`, and whatever B1/B2 decide).

What the gate must measure: **trigger observed → command dispatched** latency in the control plane, against AC-25 #1's one frame (33.3 ms at the house rate). Define "observed" precisely per trigger kind in the design note — for engine-originated triggers the WebSocket hop is inside the budget, and the note says how much of it is.

## 3. Blockers — decisions owed before execution

| # | Question | Options | Recommendation |
|---|---|---|---|
| **B1** | `audioLevel` must fire within one frame (AC-25 #1), but bus levels reach the control plane once a second | (a) the engine emits a level-crossing event on the render channel — a wire addition, UNRATIFIED candidate; (b) evaluate `audioLevel` rules in the engine (splits the rule engine); (c) narrow AC-25 #1 for `audioLevel` to the telemetry cadence (spec change) | **(a)** — the engine already emits `itemEvent`; a threshold-crossing event keeps one evaluator |
| **B2** | `streamHealth` needs transport state, which is not on the wire | ratify `streamTransportState` (v0.5 §7, UNRATIFIED), or an engine event on publisher transitions | **ratify the candidate** — it also closes the soak row's "no telemetry consumer sees the redial" gap |
| **B3** | `mediaStart` has no engine event | control-plane-side (the take whose item goes on air applied) vs an engine "first frame presented" event | **control-plane-side** for v1 — the take is the operator-visible start; an engine event can follow if a show needs frame-exact starts |
| **B4** | §13.4 wants *transitive* cycle rejection at preflight, which needs to know which commands can cause which triggers — the spec has no such table | (a) draft a command → trigger effect table (UNRATIFIED candidate) and check transitively; (b) reject direct self-trigger statically, suppress the rest at runtime only | **(a)**, drafted as a candidate — the table is small (take/mix/cut/stop → `stateChange`/`mediaStart`/`mediaEnd`; ticker → `rssKeyword` never) and makes §13.4 checkable |
| **B5** | AC-25 #2 cancels "pending actions" within a frame, but a rule has no delay field, so nothing is pending for long | define "pending" as fired-but-not-yet-dispatched within the current frame (the limiter's queue) | **that definition**, stated in the design note and pinned by a test |

B1, B2 and B4 each produce a §10.1 / spec candidate; per `docs/implementation-standards.md` and PR #30's precedent, candidates ship marked UNRATIFIED with their guards and are ratified by the user separately, never inside this feature PR.

## 4. Quality bar

Every §2a rule applies. The ones that bite:

- **Rule 7.** Every AC-25 test drives a rule through the real dispatch path — the same handler registry a human command uses — and asserts the audit row. A test that calls the evaluator's internals and asserts a return value is not a guard of AC-25.
- **Rule 8.** A new CI floor per new suite (ran and exercised). Automation is control-plane TypeScript and needs no hardware, so exercised == ran is expected; a lower exercised count is a finding.
- **Measurement discipline.** AC-25's one-frame latency is a timing claim: measure quiescent (load ≤ 3.0, pasted), 300+ firings, counts two ways, and let the soak own the threshold (a soak row and a required capture line, not a CI assertion that goes red on a busy runner).
- **Falsify every guard** by removing the behaviour it pins (limiter, self-trigger suppression, hold, cycle rejection, audit write), paste the failing signature, restore, and run the suite green after.

## 5. Telemetry obligation

`automationHold` is already law and already on the tick. Any **new** field (e.g. an automation-fired counter, the B1 level-crossing event, B2's transport state) is a §10.1 wire-addition candidate from day one and carries the five obligations `agents/prompts/10-streaming.md` §5 lists: always emitted; a stub that is not a legal value; token-stable enums with a token test; the mirror fixture samples a value; marked UNRATIFIED until the user ratifies it.

## 6. Scope discipline

**Allowed:** the rule evaluator and its trigger adapters in the control plane; `autoFollow` as the first built-in consumer of `mediaEnd`; preflight cycle checks in `nbe-preflight` / `nbe-core`; the engine events B1/B3 decide on; ladder rung 2 and the §10.3 threshold answer per C1.

**Forbidden:** rule evaluation on the render loop; any path by which an automation action skips a command precondition or the audit log; ladder rungs 3–4 (no subject); GPU timing (C1 permitting otherwise); changes to the watchdog's placement on the loop (AC-7 needs it there); ratifying any candidate inside the feature PR.

## 7. Work items

- **WU1 — Evaluator core (control plane).** Load rules with the package; `enabled` and `automation.enable/disable`; conditions; dispatch through the command registry as actor `automation:<ruleId>`; audit every attempt — fired, refused by a precondition, suppressed by hold, suppressed as a self-trigger, rate-limited — with `kind: "automation"`.
- **WU2 — Limiter and suppression.** At most one firing per rule per frame (§13.3 #3); a rule whose action re-triggers itself is suppressed at runtime (§13.4); hold suppresses all triggers and `autoFollow` within one frame and cancels pending (B5) actions (§13.5, AC-25 #2).
- **WU3 — Trigger adapters.** `timer`, `timeOfDay`, `stateChange`, `hotkey`, `rssKeyword`, `mediaEnd`, `mediaStart` (per B3); `audioLevel` (per B1) and `streamHealth` (per B2) only once their decisions land — until then they are refused at package load with a named reason, never accepted and silently inert.
- **WU4 — `autoFollow`.** Media end on an item with `autoFollow` advances as §3.1 describes, through the same dispatch path, suppressed by hold.
- **WU5 — Preflight cycle detection.** Reject direct self-triggers; transitive per B4. Refusals carry a reason, as `check_transport` does for refused transports.
- **WU6 — Watchdog remainder (per C1).** The §10.3 threshold question answered and pinned; ladder rung 2 (loop eviction to streaming) with AC-27 item 2's test and falsification, never touching the View.
- **WU7 — Measurements.** Trigger → dispatch latency per trigger kind; hold → cancellation latency; quiescent, loads pasted; recorded in `docs/09-measurements.md` with a soak row.

## 8. Tests required

1. Each available trigger kind fires its rule within the measured budget, through the real dispatch path, with an audit row.
2. A disabled rule never fires; `automation.enable` re-arms it.
3. A held engine fires nothing and cancels pending actions within one frame; release resumes; `autoFollow` is suppressed while held.
4. A rule that would fire twice in one frame fires once (the limiter), audited.
5. A self-triggering rule fires once and is then suppressed at runtime, audited.
6. Preflight rejects a direct self-trigger and (per B4) a transitive cycle, with a reason; a non-cyclic set passes.
7. An automation action refused by a command precondition is refused exactly as a human's would be, and audited as refused.
8. `autoFollow` advances on media end and not otherwise.
9. `audioLevel` / `streamHealth` rules are refused at load with a named reason until B1 / B2 land.
10. WU6 per C1: the §10.3 threshold pin; rung 2 engages under sustained load before any View drop and restores with hysteresis.

## 9. What Prompt 11 must NOT do

- Put rule evaluation on the render loop, or move the watchdog off it.
- Add a second command dispatch path or a second audit writer.
- Accept a trigger kind it cannot evaluate and leave the rule silently inert.
- Build ladder rungs 3–4, or GPU timing, without C1 saying so.
- Ratify a wire or spec candidate inside the feature PR.

## 10. Done means

- C1, B1–B5 answered by the user and recorded in the prompt map before the executor starts.
- AC-25 items 1–4 each pinned by a rule-7 test and falsified; the soak owns the latency threshold with a required capture line.
- `autoFollow` works and is held by hold.
- Every automation attempt is in the audit log.
- CI green at head, fetched fresh, run ID quoted; both keys turned; the merge word is the user's.

## 11. Carried notes

- **From PR #30:** `streamHealth` depends on the transport-state candidate (v0.5 §7) — B2. The soak row for stream reconnect says no telemetry consumer sees a redial; B2's ratification would change that row.
- **From the prompt map's § 12 entry:** its line "the spec declares Apple Silicon the primary target" predates v0.4, whose §0.1 assumption 2 names the Intel reference machine and says Apple Silicon "MUST NOT be assumed". Measure on the reference machine.
- **Still owed from 10, not this prompt's:** nginx-rtmp witness, RTMP acknowledgements, the extended-timestamp real-ingest witness, the second `Acquire` fence.

## 12. §2c — what this upgrade superseded

The 2026-09-10 draft, kept here so the change is visible:

1. ~~"Read … `agents/prompts/01-foundation.md` (metrics) and `03-compositor.md` (telemetry)"~~ — neither file exists (`01-bootstrap-core-preflight.md`, `04-basic-compositor.md`).
2. ~~"Forbidden: … any watchdog work on the render thread"~~ — contradicts AC-7 (fallback within one frame of the missed deadline) and the built, gated watchdog (§0.1).
3. ~~Steps 1–4: detection, fallback tiers, reporting, restoration~~ — detection, the fallback trip, `degradationRung` reporting and rung-1 restoration with hysteresis are built (§0.1–0.2). What remains is rung 2 and the §10.3 threshold question (WU6).
4. ~~"shed … drop the lowest-priority optional Element first"~~ — §10.5's yield order is Preview rate, loop caches, effect quality, multiview tiles; "optional Elements" is not a §10.5 rung.
5. The draft never mentioned §13 or AC-25, although the prompt map had assigned the automation runtime to this slot on 2026-09-04 (the midpoint integration review, `[RI-5]`, commit `a9ff3a7`) — six days before the draft was written.

## Report (what the executor's done-message must contain)

1. C1 and B1–B5 as decided, and where each landed.
2. G1's design note and the latency measurement per trigger kind (quiescent, loads pasted, counts two ways).
3. Every falsification signature, complete, with the restore confirmed and the suite green after.
4. Any wire or spec candidate, marked UNRATIFIED, with its guards.
5. The battery verbatim, the new floors' ran/exercised readings, CI's run ID at head.
6. The soak row(s) added and their required capture lines.
7. What was left undone and why.

## Constraints (carried)

- `anyhow` for binaries, `thiserror` for library errors; TypeScript errors are `CpError` with registry tokens.
- Vocabulary discipline: `View`, `Element`, `Sequence`, `Item`.
- An automation action is a command: same preconditions, same audit, same acknowledgement rules as the operator's.
