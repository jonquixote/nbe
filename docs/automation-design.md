# The automation engine runtime — design note (Prompt 11, gate G1)

SPEC §13 (the rule model), AC-25 (automation), §13.4.1 (the command → trigger
effect table, UNRATIFIED), v0.4.7 (the `audioLevelCrossing` candidate).
Written with WU1 (2026-09-27); each section names the work unit that lands
it. Where this note and the tree disagree, the tree wins and this note is the
defect.

## 1. G1 — where the rule engine runs: the control plane

§13.1 makes a rule's action "any command-bus command" facing "the same
preconditions as a human operator's commands". The command bus, its
preconditions, the audit log (§10.7) and `automationHold` all live in the
control plane. An engine-side evaluator would need a second dispatch path and
a second audit writer — two places to disagree. So the evaluator runs in the
control plane (`packages/control-plane/src/automation.ts`), and the triggers
only the engine can see arrive as engine frames: `itemEvent` (media end) and,
new in v0.4.7, `audioLevelCrossing`.

**The one command path.** `server.ts`'s `runCommand` is the only way a
command executes: `dispatch()` (payload schema, role, preconditions, the one
`stateVersion` bump, directives) → the audit row → the §5.4.1 `stateChange` →
the evaluator hears what changed. A session's command and a rule's action both
run through it. A rule's action runs as role `operator` — §13.1's "a human
operator's commands" — with `connectionId` and audit `actor`
`automation:<ruleId>`, and is audited as `kind: "automation"` in the same
`AuditLog`. There is no second writer.

## 2. The params and conditions contract (WU1)

The schema types `trigger.params` and `conditions` as free objects, and §13.2
names each trigger kind without its parameters. The tree's reading follows.
The spec is silent, so this is a candidate for spec text, not law. Only
`audioLevel`'s reading is in the spec, in v0.4.7, because the engine consumes
it.

| Kind | Params | Fires when |
|---|---|---|
| `mediaEnd` | `{ itemRef? }` — a rundown item | the control plane records `PLAYING → DONE` from the engine's `itemEvent end` (a stopped item's late `end` is dropped by `markDone`, §13.4.1) |
| `mediaStart` | `{ itemRef? }` | a `view.take` / `view.cut` puts the item `LIVE` or `PLAYING` (B3: control-plane-side, the take applied) |
| `timer` | `{ atMs }` > 0 | `atMs` of show clock after `show.start` is accepted; once per run; `show.stop` cancels it |
| `timeOfDay` | `{ at: "HH:mm" \| "HH:mm:ss" }` | the local wall clock reaches `at`, daily, while the package is loaded |
| `audioLevel` | `{ bus, thresholdDbfs, direction? }` (SPEC v0.4.7) | the engine's `audioLevelCrossing` for that bus, threshold and direction |
| `hotkey` | `{ bindingId }` — a `control.bindings` entry whose trigger kind is `hotkey` | a command arrives carrying `intentSource` `…:<bindingId>`, whatever that command's own outcome (the binding fired either way) |
| `rssKeyword` | — | **refused at load: no source exists.** `ticker.refreshRss` fetches nothing (§13.4.1), so no RSS item ever arrives to match. The draft's "available (per refresh)" is contradicted by the tree |
| `streamHealth` | `{ state: "live" \| "reconnecting" \| "closed" }` | `streamTransportState` changes to `state` (§5 below) |
| `stateChange` | `{ field, itemRef?, from?, to? }` — `field` one of `showState`, `viewItem`, `previewItem`, `streamState`, `recordState`, `automationHold`, `fallbackActive`, `itemState` (which needs `itemRef`) | that field changes, matching `from` / `to` where given |

**Conditions** are `{ field, itemRef?, equals }` over the same fields, all of
which must hold (AND), read at firing time.

**The action** is a registered command whose payload validates against its
§16 schema.

**Who decides.** `nbe-preflight` refuses a rule that does not read, naming the
rule and the reason. The decision is preflight's (Addendum 02a §1.4). The
control plane's read at load holds the same verdicts:
`crates/nbe-core/tests/fixtures/automation_rules.json` is read by both suites,
and a rule one side accepts and the other refuses fails both. The action
payload's full validation is the control plane's (the command schemas live
there). Preflight checks the command name and required keys, as it does for a
control binding's action.

## 3. Evaluation (WU1, WU2)

For one trigger event, every rule it matches, in manifest order:

1. **disabled** (`automation.disable`, or `enabled: false` in the manifest —
   honoured since WU1; it used to be ignored): skipped, not an attempt.
2. **held** (`automationHold`): *suppressed*, audited
   `automation.suppressedByHold`.
3. **conditions** false: skipped. The trigger condition is not true, so this
   is not an attempt.
4. **already fired this frame**: *rate-limited*, audited
   `automation.rateLimited` (§13.3 #3).
5. otherwise **pending**: queued.

The queue drains one action at a time, in firing order, through the one
command path. The action is audited `automation.action`, `ok` or `rejected`
with the same `errorCode` an operator gets.

**The frame.** A frame is `1000 / houseRate` ms of the control plane's
monotonic clock (`performance.now()`), indexed from the clock's origin. The
limiter keys on (rule, frame index). Two triggers that straddle a frame
boundary may both fire; that is once per frame, which is what §13.3 #3 asks.

**§10.7's limiter applies too** (found by WU7). A rule's action runs on
connection `automation:<ruleId>`. `RateLimiter` allows 10 per burst, refilled
at 5/s per connection per command family, so a rule sustains at most 5
actions/s in one family. That is below once per frame (30/s). An action over
the limit is refused `E_RATE_LIMITED` and audited as a refused
`automation.action`, as an operator's command would be. This is recorded for
the user, not changed (`docs/09-measurements.md`, WU7).

**B5 — "pending".** Pending means fired but not yet dispatched: queued in the
evaluator, as B5 decided. A hold cancels everything pending: `automation.hold
{ hold: true }` accepted → `holdEngaged()` in the same event-loop turn as the
acceptance, and each cancelled action is audited `automation.cancelledByHold`.
The drain also re-checks the hold before each dispatch, so an action queued
behind the hold's own dispatch is cancelled rather than run (AC-25 #2).

## 4. Causality and self-triggers (WU5)

Every trigger event carries a **cause**: the chain of rule ids whose actions
led to it. It is empty for an operator's command, an engine frame or a clock.
A rule's action runs with the chain extended by that rule, and the triggers the
action raises in the same turn carry it. WU5 uses the chain for §13.4's runtime
suppression (a rule in its own chain is suppressed) and checks each suppression
against §13.4.1's row for the action's command. Deferred effects break the
chain: a take's `mediaEnd` arrives one duration later. §13.3's limiter bounds
those at runtime, and preflight's static check (WU5) owns them.

## 5. `streamHealth`: which tokens fire, and does a redial fire?

**Source.** `streamTransportState` on the §10.1 tick (v0.4.6). It is observed
once a second, so a `streamHealth` rule is observed within one tick of the
engine's change, not one frame. AC-25 #1 is measured from observation (§6), and
the engine→tick hop is outside the evaluator's budget. This is stated rather
than hidden.

**The mapping.**
- A rule names one of `live`, `reconnecting` or `closed` and fires on a change
  *to* it.
- `none` never fires: it is the stub for "no stream has started", not a state.
- The first value the control plane observes is the baseline and fires nothing.
  After a control-plane restart, a stream already `live` is not news.
- A change from `none` counts: the first stream going `live` fires a `live`
  rule.

**The evidence it is argued from.**
- **The rehearsal's measured values**, `closed:26 live:5 none:19` (v0.4.6 soak
  capture): a healthy stream reads `none → live → closed` at 1 Hz. The initial
  dial's `reconnecting` usually lasts under a tick and goes unseen.
- **The redial**, `transport_death_leaves_the_loop_untouched`: `live →
  reconnecting → live` with 0 View frames dropped. So `reconnecting` is
  transient and healthy — the survival path working, not an outage. A redial
  fires a `reconnecting` rule when it lasts long enough to be sampled. That
  makes the rule a warning hook, and a sub-second redial may go unseen.
- **`closed` while `streamState` is `live`** is the failure shape the keyless
  false-live produced before `40e96e6`. A `closed` rule is the one that means
  the stream is gone.

## 6. "Observed" — the latency's starting point, per kind (WU7)

`latencyMs` in each `automation.action` audit row is `dispatchedAt −
observedAt`. The row also carries `queuedAt` (WU7), the end of AC-25 #1's span:
observed ≤ queued ≤ dispatched. Each `automation.cancelledByHold` row carries
`heldAt`, `cancelledAt` and `latencyMs = cancelledAt − heldAt`, which is AC-25
#2's number. An `audioLevel` row's trigger also carries the engine's `ts`
(v0.4.7), so the span from crossing to queue can be measured across the
process boundary. The measurements are in `docs/09-measurements.md` (WU7).
`observedAt` is:

| Kind | Observed when |
|---|---|
| `stateChange`, `mediaStart` | the command that caused it is accepted (the same turn) |
| `mediaEnd` | the engine's `itemEvent end` frame arrives |
| `audioLevel` | the `audioLevelCrossing` frame arrives. The engine→control-plane hop is inside AC-25 #1's frame; the crossing is computed on the block it happened in |
| `hotkey` | the carrying command arrives |
| `streamHealth` | the telemetry tick that shows the change arrives (§5) |
| `timer`, `timeOfDay` | the scheduled instant |

## 7. What this runtime does not do

- **No `rssKeyword`**: no source exists (§2).
- ~~**No ladder rung 2** (WU6): the tree has no runtime streaming decode to
  evict loops to. This is blocked by §4a and reported with its options.~~
  *Struck (§2c): the user decided rung 2 on 2026-09-27 — eviction only of
  loops not on air, §10.5's freeze clause as the invariant — and WU6 landed it
  (`d455aa0`, recorded `7303d4f`; SPEC v0.4.7 row 2). This line outlived the
  decision by one commit; WU6's record did not reach this note.*
- **No hysteresis on `audioLevel`**: none is specified, and the limiter bounds
  firing (v0.4.7).
