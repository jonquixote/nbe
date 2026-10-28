# Prompt map 07–13, re-scoped — `[RI-6]`

Prompts 07–13 measured against what the engine **actually is** after P1–P6, not against what the original build order assumed. One paragraph each: what it must now contain, and what it inherits from the deferral ledger.

Produced by the midpoint integration review. Ledger states are in `docs/review-midpoint-report.md` §6; findings referenced as R*n* (rehearsal), F*n* (pass 4), H*n* (hardware), S*n* (`/status`).

Note on numbering: the build order names 14 (packaging) and 15 (contrib outputs) beyond this range. They are untouched by this review and remain as written.

---

## 07 — Graphics and templates (overlays, ticker, clock)

**Inherits three promoted deferrals** — the overlay level itself (§7.10, deferred by P4 naming 07 as owner), §8.7.5's true crossfade via per-source envelopes, and the `sfx` ramp-advance invariant. It also inherits **R1 as P0**: the item→asset audio miss must be at the front of 07's fix list, because a show with no sound is not a foundation to build overlays on. The scope that P4 deferred is unchanged — `View = overlay(transition(A, B))`, overlays composite after the transition and persist across it, and AC-24 (a ticker survives a complex move transition untouched) is the acceptance case. What has changed is the surrounding evidence: 07 now knows the audio graph works in isolation and does not reach a real take, and that per-source envelopes are load-bearing on **both** sides — video overlays and audio crossfade — so they should be designed once. §6.5's forbidding of per-frame relayout and its packaged-font rule are portability assets (`docs/portability.md` row 7) and must not be weakened. R2, R3 and R5 are assigned here too: they are metering and timing work that 07's audio changes will touch anyway.

### Carried into 07 from the v0.4 review passes (recorded 2026-09-06)

Three observations the fourth adversarial pass over `c092f20` reproduced and
deliberately did not file as findings. They are hardening and conformance work,
not defects in what merged, and they are recorded here so they have an owner
rather than fading with the pass that found them.

1. **The `CacheBudget` struct-literal lint has a bypass.**
   `nbe-core/tests/spec_budgets.rs` refuses a `CacheBudget { … }` literal outside
   `loop_cache.rs`, which is how 1024/4096 came to exist beside 256/512. It does
   not refuse `let mut b = index.loop_budget(id); b.per_loop_mib = 1024;` — the
   fields are `pub` on a `Copy` struct, so a divergent budget can exist with no
   literal for the grep to see. The pass wrote that code and it compiled and
   passed. `#[non_exhaustive]` closes cross-crate literals at compile time;
   private fields with accessors close the mutation too. **Hardening, not a
   defect** — no such site exists, and the primary guarantee is structural (two
   consumers, one constructor each).

2. **§12.4's 900-frame conjunct is enforced only in preflight.** §12.4 states
   residency as a conjunction — `periodFrames <= 900` **and**
   `<= maxFramesByBudget` **and** `totalShortLoopCache <= totalBudget`.
   `nbe_core::loop_cache::plan()` implements the second only;
   `ABSOLUTE_LOOP_FRAME_CAP` now lives in that module but is applied by preflight
   alone. An engine run directly on a package with a >900-frame small-resolution
   loop — the dress rehearsal path, which does not gate on preflight — holds it
   VRAM-resident against the spec. Unreachable through the sanctioned path
   (preflight refuses the package by name first), which is why it did not block
   the merge. **07 owns applying the conjunct inside `plan()`,** where the
   constant already sits.

3. **`has_alpha` is derived differently on the two sides — no action.**
   Preflight infers it from `kind == "alphaVideo"`; the engine detects it from
   decoded pixels. It cannot change the engine's selected format while
   `yuv_sampling: false` (both alpha states select RGBA8), so the difference has
   no consequence today. Documented here so a future reader does not rediscover
   it as a bug. It becomes live the moment shader-side YUV lands — the same
   moment §12.3's format ladder does, and the two should be closed together.

### The 46 s `show.load` and the preflight bound shared one root (recorded 2026-09-07)

Step 3d's gate for promoting the dress rehearsal to a required job includes
*"`show.load` stops taking 46 s under contention"*. The spine's second review
pass found, separately, that the control plane's preflight timeout was crossed
by an ordinary package. **They are the same cost seen twice.**

Measured on this repo's `dress_show` fixture, same machine, same run:

| build | `dress_show` preflight | per 1080p frame |
|---|---:|---:|
| debug | 30.05 s | 130 ms |
| release | 3.45 s | 16.7 ms |

The control plane was resolving `target/debug/nbe-preflight`, and CI built it
without `--release`. It now prefers `target/release`, and both jobs that shell
preflight build it that way.

**Disposition: this shrinks 3d, it does not close it.** The 46 s line can now
cite the release binary — a ~9x reduction on the fixture, which takes the cost
out of the flakiness budget entirely. What remains of 3d is unchanged: 3a-3c and
steps 1-2 closing, and green on `macos-14` **twice consecutively**. When 3d is
next revisited, its "46 s addressed" criterion should be rewritten to cite the
release binary rather than an outstanding fix, and the rehearsal's own timing
assumptions re-measured against it — a step that used to wait 46 s for a load
may now be measuring something else entirely.

### `refusalBypassed` keys on the derivation, not the applied bound (recorded 2026-09-08, renamed 2026-09-09)

`preflight.bound_decision.refusalBypassed` is defined as **the override was passed AND
`derivedMs` exceeded `ceilingMs`**. That definition stands; implementation and tests follow
it, and this note changes neither.

The observation as originally filed: the condition turns on the *derivation*, not on what
the override actually did. An operator setting `NBE_PREFLIGHT_TIMEOUT_MS=1200` against a
package deriving 75,000,000 ms records true — yet 1,200 ms is far **below** the 3,600,000 ms
ceiling, so no ceiling was overridden; the override made the bound *tighter*. Under the
field's original name, `overrideUsed`, a reader who took it to mean "the operator knowingly
went past the ceiling" was misled in exactly the case the field exists to flag.

**The rename to `refusalBypassed` (2026-09-09) closes that misreading rather than moving
it.** In the 1,200 ms case a refusal genuinely was bypassed: 75,000,000 ms exceeds the
ceiling, so absent the override `loadPackage` would have refused, and the override is why it
ran instead. The field is now named for the thing its condition actually tests:
**`refusalBypassed` keys on the refusal, not on where the operator set the bound.** What the
field does not report is whether the bound *in force* exceeds the ceiling — that is
`appliedMs > ceilingMs`, derivable from the stored fields and deliberately not stored.

Consequently the definition change this note once queued is **closed by the rename**, not
carried. The next-spec-revision queue still holds `error.details` (§5.4/§16, below) and
`decodeFailuresTotal` (§10.1, from the F1/F2 round).

This section is a merge. `8538701` added the mandated semantics sentence as its own
section; `3014c9d` folded it into this one because the two were disagreeing about whether
`appliedMs > ceilingMs` was a queued replacement definition or a derivable non-field.
Reviewer finding **F2** recorded that the merge deleted prompt-mandated verbatim text
without ratification; the merge is now **ratified retroactively** and both halves of the
mandated substance are stated above. No separate verbatim section returns.

### Recorded deviation: `error.details` on the §5.4 envelope (recorded 2026-09-08)

§5.4 and §16 define an error response's `error` as `{code, message}`; the control plane now
adds an optional `details` member, carried today by `E_PREFLIGHT_FAILED` on a decode-bound
refusal so a caller can act on the numbers rather than parse the sentence — an input to the
next spec revision, which should absorb `error.details` as an optional member.

(The §19.2 note proposed by an earlier prompt is **cancelled**: there is no §19.2 deviation,
because the decode bound never reaches `preflight_report.json` — that file is written by the
Rust binary, which has no knowledge of the caller's bound.)

### Open observation: the control-plane gate greps TAP, and the TAP is a reporter choice (recorded 2026-09-08)

The control-plane job's "tests pass and actually run" gate parses `node --test`
output for `^# pass` / `^# fail` summary lines — the TAP reporter's format. A
future Node bump that changes the default reporter (newer Node defaults to the
spec reporter) breaks this gate **loudly, not vacuously**: reproduced by running
the green suite under the spec reporter and feeding that log through the gate
verbatim — node exits 0 with all tests passing, the log contains zero `# pass`
lines, and the gate exits 1 on the `expected 0 failures` check (`${failed:-1}`
is 1 when the grep matches nothing; the `passed < 30` check would trip next).
No silent green is possible from this direction, which is why this is an
observation and not a finding.

**Disposition is the maintainer's.** The two readings are: pin the reporter
(`--test-reporter=tap`) so the gate's input format is a contract rather than a
coincidence, or rewrite the gate to parse the reporter's machine-readable
output. Whoever bumps the Node version past the TAP default owns choosing.

### Correction: the `8538701` falsification produced four tsc errors, not three (recorded 2026-09-09)

`8538701`'s commit message states that reverting the field in the emitter produced "three
errors (the structural collector type and two property accesses)". It produced **four**.
There are **two** structural collector declarations, not one — `v04.test.ts:763` and
`:901`, consumed at `:773` and `:905` — so the signature is two `TS2345` plus two `TS2339`:

```
src/v04.test.ts(773,66): error TS2345: Argument of type 'BoundDecision' is not assignable to parameter of type '{ event: string; outcome: string; refusalBypassed: boolean; basis: string; }'.
  Property 'refusalBypassed' is missing in type 'BoundDecision' but required in type '{ event: string; outcome: string; refusalBypassed: boolean; basis: string; }'.
src/v04.test.ts(835,20): error TS2339: Property 'refusalBypassed' does not exist on type 'BoundDecision'.
src/v04.test.ts(852,32): error TS2339: Property 'refusalBypassed' does not exist on type 'BoundDecision'.
src/v04.test.ts(905,64): error TS2345: Argument of type 'BoundDecision' is not assignable to parameter of type '{ event: string; outcome: string; refusalBypassed: boolean; basis: string; }'.
  Property 'refusalBypassed' is missing in type 'BoundDecision' but required in type '{ event: string; outcome: string; refusalBypassed: boolean; basis: string; }'.
```

The cause was a `head -4` pipe on the evidence paste, which cut the fourth error exactly at
the boundary. The falsification itself was sound and reached further than claimed; only the
record understated it. Raised as reviewer finding **F1**, and it is the third recorded
instance of a truncating pipe producing a false claim in this project — hence the standards
rule now forbidding them in evidence.

### PR #21's two-key findings — both closed by guarding, not by trimming (2026-09-18)

The pass found two of the three drafted v0.4.3 sentences making claims their
cited tests could not fail for. Neither sentence was wrong about the code; both
were unfalsifiable as law, which is worse than an unwritten rule because the
citation reads as a guarantee.

**F2 — §7.10's guards never entered the take path they name.** The sentence says
`anim_start` and duration are "untouched by the take path", and both cited tests
installed a transition straight into `state.transition` via a `set_mix` helper.
Re-keying every in-flight overlay animation inside `on_take` left all thirteen
tests in the file green. Closed by making `overlay_persists_across_take` and
`animation_immune_to_take` dispatch a real `view.take` through
`DirectiveHandler`, with the armed transition asserted (start frame 0 on a
stopped clock, duration 15) and the overlay runtime compared field-by-field
across the take. The same mutation now fails:

```
test animation_immune_to_take ... FAILED
assertion `left == right` failed: a take must not re-key anim_start
test result: FAILED. 12 passed; 1 failed
```

The helper survives only where the claim is about compositing against an
already-armed transition, renamed `install_transition_directly` with a doc
comment saying it is not the take path. The trap now has a standards rule —
§2a rule 7 — because it had already invalidated a falsification probe in the
step-5c pass, and a review that names an un-entered path is making a finding
about the test, not the code.

**F1 — §7.14's View-bus qualifier was unguarded.** Deleting `bus == Bus::View &&`
from `render.rs` left the workspace at 309 passed, 0 failed. The preferred
outcome was taken rather than the trim: the code shows Preview keeps compositing
its own scene while the View shows the slate, and `fallback_covers_overlays` now
arms Preview on A2 (SCN_BLUE) and asserts the Preview readback is BLUE and not
SLATE. Preview renders at `PREVIEW_W x PREVIEW_H`, so it needed its own sampler —
`px_at` indexes by the View's geometry and ran off the end of the buffer, which
is why the first attempt panicked rather than failing an assertion. With the
guard in place, deleting the qualifier fails:

```
test fallback_covers_overlays ... FAILED
assertion `left == right` failed: Preview composites its own scene
  (A2 -> SCN_BLUE) while the View shows the slate
```

Both sentences are now law the tree can keep. §16.6 needed no work — both its
mutations already bit.

### Finding R9 — a wall-clock threshold microbenchmark sits in the default suite (recorded 2026-09-18)

`audio_tap_push_never_blocks_contention_micro`
(`crates/nbe-engine/tests/prompt09_record_file.rs:749`) asserts that the worst
single `AudioTap::push` over 10,000 pushes stays under **1 ms**. It was added by
the transitions branch (`29efe89`, "review round 3 — SPSC contract") and merged
in PR #20, and it runs in every `cargo test --workspace`.

Measured on the normative machine, same binary, same commit:

| Load (1m) | Result |
|---:|---|
| 2.58 | passes on the first attempt, 0.98 s |
| ~30 | **fails** — worst pushes of 13.4 ms and 32.1 ms across its retry attempts, best 10.9 ms against a 1 ms bar |

It already carries a retry loop (up to 10 attempts, keeping the best), which is
an acknowledgement that the number is not stable — but retries widen the window
rather than close it, because the contended case is precisely when the assertion
is unmeetable.

**This is the exact class the gate split assigned to the soak**: a wall-clock
threshold whose value depends on machine quiescence, living in a suite that runs
on 3 arm64 CI cores and on a developer machine mid-build. Its home is
`docs/soak-protocol.md` §1, where quiescence is a checked precondition and a
violated one produces VOID rather than FAIL.

**ADJUDICATED AND ADOPTED 2026-09-18 (PR #21 fix round): rebound by work, not
wall.** Of the three options below, the chosen one is the second — the SPSC
promise asserted without a clock. The test is now
`audio_tap_push_is_bounded_and_lossless_accounting_under_a_hammer`, pinning what
the contract actually says: capacity fixed across 10k pushes (a ring that
reallocates allocates on the audio deadline), occupancy never above capacity, 10k
pushes into an undrained ring completing rather than stalling, and every sample
either retained or counted dropped — exactly once, arithmetic rather than timing.

The old comment had already conceded the gap in passing — *"the lock-freedom
itself is asserted by code inspection in review"* — so the one property that
mattered was the one the test never checked.

Falsified both ways: removing `dropped.fetch_add` fails the accounting assertion
(`pushed 10241024, retained 480000, counted 0`); asserting a grown capacity fails
the bounded assertion. Load-independence demonstrated rather than claimed — the
rebound test passes in **0.72 s at load 6.19**, on the same machine where the
wall-clock version failed at load 5.08 in 8.07 s while this rebind was being
written. The suite fell from ~19 s to 2.4 s, because the timing loops were most
of its runtime. The wall-clock number moves to the soak's threshold list.

Original disposition, kept per §2c:

**Not fixed here.** Work order V05-PHASE0 scopes Mission 1 to docs and Mission 2
to `packages/control-plane`; this is a Rust test. Recorded rather than touched,
with the measurement above as the evidence a fix can start from. The options are
to move it behind the soak, to relax it to something load-independent (a bound on
work done rather than wall time — the SPSC contract it means to pin is "push does
not block", which is a property of the code path, not of the clock), or to delete
it in favour of the contention test that does not assert a duration.

It also cost this session a false reading: the workspace summary said "0 failed"
while this test was failing, because the ad-hoc awk in use split on `;` and read
an empty field for the failure count. CI's own summary uses the default separator
and is correct.

### PR #21's fix-round pass — a third rule-7 instance, found by the new rule (2026-09-18)

Rule 7 earned its place on its first application. The pass swept the engine
suites for tests whose names claim a dispatched path, and found one the previous
rounds had not:

**`a_cut_and_a_mix_do_not_reshape_the_ticker`** (`prompt07b_graphics.rs:292`)
bound `handler`, then performed "a cut" by writing `state.view_item` and "a mix"
by writing `state.transition` — neither dispatched. Third instance, third suite.
Proven with the positive control the rule's own entry now recommends: `panic!` on
entry to `on_take` left the ticker test **green** while the repaired overlay
guards **failed**.

Repaired the same way: both the cut and the mix are dispatched `view.take`
directives, and the armed transition is asserted (kind Mix, duration 15, start
frame 0 on a stopped clock) rather than hand-written. The control now bites:

```
test a_cut_and_a_mix_do_not_reshape_the_ticker ... FAILED
thread '...' panicked at crates/nbe-engine/src/directive.rs:428:9
test result: FAILED. 0 passed; 1 failed
```

Seven other sweep hits were name-collisions and are clean — `sigkill_shape_drop_mid_take`
means a *record* take, and `record_start_on_running_show_opens_pipeline`
dispatches through `start_recording` → `.apply(&directive(…))`. The sweep's value
was not its hit rate; it was that the one real instance had survived two passes
that were looking directly at it.

### R7 — FIFTH sighting settled it, and my first mechanism was WRONG (2026-09-18)

The retirement recorded below was premature, and this entry supersedes its
mechanism while leaving it standing per §2c. The fix in it was real but partial;
the diagnosis was not the cause.

**What happened.** The very PR that retired R7 flaked again on push — run on
`d48240c`, `not ok 56`, `expected 3 directives, got 4` — and this time on the
FIRST count, after all three command-name waits had completed. That alone
refuted the aliasing explanation: with the waits keyed by name, all three
directives are confirmed present, and a fourth still existed.

**The payload dump, added two rounds earlier for exactly this, named it:**

```
received: [{"command":"show.resync","seq":0,"stateVersion":0},
           {"command":"show.load","seq":1,"stateVersion":1},
           {"command":"preview.set","seq":2,"stateVersion":2},
           {"command":"view.take","seq":3,"stateVersion":3}]
```

**The mechanism, finally.** Neither a redelivery nor an extra `stateVersion`
bump — both of which I had ruled out correctly. It is `show.resync`, the §5.9.4
snapshot the server sends the moment a render session registers, "before any
other directive on this connection" (`server.ts:367`). It is **always sent**. The
test attached its collector *after* `await connect(render)`, so whether that
frame was observed was a race between the handshake resolving and the listener
binding. Locally the listener loses and the test sees three; on a loaded 1-3 core
runner it sometimes wins and the test sees four.

Every recorded property follows: load-sensitivity, always green on rerun, two of
five sightings on docs-only branches, and a count that is always exactly one too
many rather than a duplicate of anything.

**Why I missed it.** I checked for a connect-time directive and concluded there
was none — from a local run that printed exactly three. That was the race
resolving the usual way, read as evidence of absence. A frame that is always sent
and only sometimes seen looks identical, from one sample, to a frame that is
never sent. §2a rule 7's lesson generalises here: I inferred a property of the
server from a test whose observation window did not cover it.

**The fix.** The collector is attached BEFORE the handshake, so the resync is
observed deterministically rather than raced for, and it is asserted as the
contract §5.9.4 says it is: first on the connection, and alone until the first
command. Falsified: deleting `sendResync(session)` from the connect path fails
with `directive 'show.resync' never arrived; have []`.

~~That sentence of §5.9.4 had no guard at all before this.~~ **CORRECTED
2026-09-18 (§2c) — that claim was false, and the tree says so.**
`render-channel.test.ts` already guarded the sentence from the channel's side:
`:223` "show.resync is the first directive on a render connection" with its
payload-key assertions, `:236` the mid-show reconnect case, and `:257`
`resyncRequest`. Deleting `sendResync` from the connect path fails **four**
tests — 42, 43, 44 and 56 — three of them pre-existing. The new assertion is a
**duplicate at a different layer**, which is worth having (it is what stops
`server.test.ts` from counting a frame it never meant to observe) but is not a
first guard.

How the false claim was made is the instructive part: the falsification that
produced it ran only `server.test.ts`, saw one failure, and inferred
exclusivity. That is the same shape as reading one sample of a race as evidence
of absence — the error this very entry was written to correct, repeated one
paragraph later against a different question. A falsification that is scoped to
one file answers "does this test guard it", never "is this the only guard".

Two smaller over-assertions were corrected with it: `seq` 0 and `stateVersion` 0
are facts about an **initial connect against a fresh server**, not §5.9.4
properties — a reconnect mid-show carries the current `stateVersion`. They are
now labelled as fixture facts in the test, and the reconnect half of the
sentence is named as living at `render-channel.test.ts:236`.

The count assertions move from 3 to 4 and still bite a real extra: making
`view.take` emit a duplicate gives `expected 4 directives (resync + three
commands), got 5`.

**THE REFUTATION CONDITION, written before the next run rather than after it.**
This retirement rests on one mechanism: the §5.9.4 `show.resync` is always sent
on render-session registration, and the test used to race its own collector
against it. **A sixth sighting of the `expected 3 directives, got 4` signature —
or its post-fix form, `expected 4 directives (resync + three commands), got 5` —
refutes this retirement and returns R7 to open.** The condition is recorded now,
before any run that could satisfy it, because the fifth sighting's lesson is that
a confident mechanism can be wrong: the fourth retirement attempt named aliasing,
was argued from code and a 75-run streak, and was refuted by the next push. A
standard stated after the fact is a standard fitted to the outcome.

**One process note.** The first attempt at that falsification mutated the wrong
call site: the pattern `        sendResync(session);` (8 spaces) is a substring of
the 10-space occurrence in the `resyncRequest` handler, so it matched there,
`count == 1` passed, and the connect path was untouched — which is why the
mutation appeared not to bite. Anchoring the match to the line boundary found
the real site. A substring match that silently hits the wrong instance is the
same failure as a test that never enters its path.

### Finding R7 — CLOSED 2026-09-18 (work order V05-PHASE0). Original heading and every sighting kept below per §2c

**Mechanism.** Not a product defect: the test's synchronisation could not
express what it meant to wait for, and the assertion was therefore taken at an
arbitrary moment.

The ok-response travels on the `admin` socket and the directive on the `render`
socket, so a response can be observed before its own directive lands. The test
read `directives.at(-1)` immediately after each response to learn "the seq this
command produced". When the directive had not landed yet, `at(-1)` returned the
**previous** command's directive, the `waitForSeq` built from it found that
already-present entry and returned instantly, and the test moved on without ever
waiting for the directive it meant to wait for. Two of the three waits were
therefore capable of being no-ops. What remained was a fixed
`await setTimeout(30)` and a count — a duration standing in for a condition, on
runners between 1 and 3 arm64 cores.

That explains every property of the register: load-sensitivity, always green on
rerun, and two of four sightings on branches whose diff was docs-only.

**What was ruled out, by reading rather than by assumption.** Every emit path
produces exactly one directive per command — `view.take` emits one
extraDirective, `show.stop` emits via `emitDirectivesNow` and then returns
`{ warnings }` with no second send, and there is no replay, resend or retry
anywhere on the render channel. A normal run collects exactly three, seqs 1-2-3
against stateVersions 1-2-3, with no connect-time directive. Each test gets a
fresh server on an ephemeral port, so cross-test leakage is not available either.

**Fix.** The waits are now by **command name** (`waitForCommand`), which cannot
alias: a directive for `preview.set` is the only thing that satisfies the wait
for `preview.set`. The fixed 30 ms sleep is replaced by a settle window, and the
count is asserted a **second** time after a further settle window, so a
duplicate or extra directive arriving late still fails the test instead of
slipping past the first count — which the 30 ms sleep never caught at all. The
4-detection is kept and made deterministic; what changed is WHEN the count is
taken, never what counts as wrong.

**Verification, and which kind it is.** Proof by construction, not by
reproduction, and the distinction is the honest part: R7 could not be forced
locally either — 70 runs before the fix (40 idle, 30 at load 4.9) produced zero
sightings, so the 75 clean runs after it (50 idle, 25 at load 14.2) are
*consistent with* the fix rather than proof of it. What is proof: the aliasing
path is gone by construction, and the assertion still bites a real fourth.
Falsified by making `view.take` emit a duplicate extraDirective:

```
expected 3 directives, got 4
received: [{"command":"show.load","seq":1,"stateVersion":1},
           {"command":"preview.set","seq":2,"stateVersion":2},
           {"command":"view.take","seq":3,"stateVersion":3},
           {"command":"view.take","seq":4,...}]
```

The diagnostic added at the fourth sighting is what makes that legible, and it
stays. **No production code was touched** — the root cause was in the test, which
is why the work order's stop-and-report condition was never reached.

**If R7 returns**, it is now a different finding: the waits are unambiguous, so a
fourth directive would be a real one, and the dump names it.

### Finding R7 (new) — control-plane test 34 saw a fourth directive where three were expected (recorded 2026-09-09)

`render-role session receives directives in order with correct stateVersion` failed once
with `expected 3, actual 4`, on the run immediately following a heavy Rust build. It passed
nine further runs in the same tree, giving an observed rate of roughly **1 in 11**. The run
that failed was the most heavily loaded one, so the working reading is **load-sensitive
timing**, the same family as R2 (a 33 ms window sampled at 1 Hz) and R5 (the clock not
advancing within 2 ticks of `show.start`).

**Unconfirmed as pre-existing.** The attempt to reproduce it at `5e2b595` used a scratch
`git worktree`, which has its own empty `target/` and therefore no release
`nbe-preflight` — three unrelated tests failed environmentally there, so that run proves
nothing in either direction and its numbers are not recorded here. Establishing whether the
flake predates the `refusalBypassed` rename needs a build in the same tree, not a
side-by-side worktree.

**The open question is which failure it is.** `expected 3, actual 4` has two readings that
call for different fixes: the fourth directive was a **duplicate** — the same directive
redelivered, a redelivery bug — or it was an **extra `stateVersion` bump**, an ordering bug
in which a directive that should not have been counted advanced the version. The assertion
counts, so it cannot distinguish them. Whoever picks this up should capture the directive
payloads on failure before theorising; a retry loop that only re-runs until green will
discard the one artifact that answers the question.

**Second sighting, 2026-09-11 (P7b two-key pass).** The same test failed again —
on a different branch, against different code, during an unrelated mutation (a
non-stable ticker sort, which cannot touch directive ordering). It passed 3/3 on
the restored tree. That strengthens the half the original record left open: the
flake is **pre-existing and independent of the change under review**, not
something either branch introduced. Still unresolved is which failure it is —
the redelivery-vs-extra-bump question below stands, and the payloads still need
capturing before anyone theorises.

**Disposition: Prompt 07, alongside R2 and R5.** Numbering continues the R-series filed in
`docs/review-midpoint-report.md` §3.4–§3.8 and §11.2 (R1–R6); that report is a sealed CLEAN
verdict and is not amended to hold this.

**Third sighting, 2026-09-13 (P9 groundwork, PR #17 run `34745014794`).** The same test
failed with the same signature (`expected 3 directives, got 4`) on a docs+CI-only
branch whose code was identical to green main, passed 6/6 locally in the same
tree, and passed on CI rerun. This is R7's test, not a new entry: same name, same
signature, same load-sensitive family, now 3 sightings across unrelated changes.
The redelivery-vs-extra-bump question still stands — payloads still uncaught.

### Finding R10 — a stream drain missed its 5 s bound ~~once~~ ~~**twice**~~ **three times** on CI (recorded 2026-09-25, PR #33; sighting 2 recorded 2026-10-16; sighting 3 recorded 2026-10-26)

*Numbered after R9, the highest filed; R8 appears nowhere in the tree.*

**Signature.** `prompt10_rtmp::telemetry_tick_wires_the_live_session_counter`
panicked at `crates/nbe-engine/tests/prompt10_rtmp.rs:1570`: `drained reads 0 on
the tick`, with `test result: FAILED. 19 passed; 1 failed; 1 ignored`. This was CI
attempt 1 of run `36120910087`, at head `000159f`, in the `rust` job (job
`108025920278`, hosted `macos-14`, runner `GitHub Actions 1000002963`). The
re-run, attempt 2 of the same run, passed. Locally, on the normative machine,
the test passed 10/10 at 0.37–0.46 s each (load 2.75 → 3.09 across the loop).
It is the first recorded failure of this test. The eleven CI runs before it,
back to `35976288284`, all contain the test (it landed in `200816f`), and all
were green.

**Not the change under review.** The test opens `StreamSession::open` directly
and never calls `resolve_stream_url`, the only engine code PR #33's fix round
changed.

**The bound.** The test publishes 64 × 60 KB video payloads into a stalled peer.
The bounded channel admits what it can and sheds the rest. It then un-stalls the
peer and gives the tick 5 s (`poll_until(Duration::from_secs(5), …)`) to read
`0.0`. Draining means the in-process test server reads and parses every message
on one thread per connection. On the runner the test ran about 5.4 s (the
preceding test finished at 09:54:34.90Z; this one failed at 09:54:40.28Z), which
fits the drain poll expiring. Locally the whole test takes about 0.4 s: a gap of
more than 13× that is not explained.

**The class: R9's.** A wall-clock bound that measures the machine as well as the
code, and VOID-shaped above the quiescence ceiling. **The unanswered question is
the runner's load at the failure, and this run's logs cannot answer it.** The
workflow echoes no load: `.github/workflows/ci.yml` contains no `vm.loadavg` or
`uptime` anywhere. The job API records only the runner's name and labels.
Answering it for a future sighting needs a `sysctl -n vm.loadavg` echoed around
the workspace test step. ~~That capture is owed and not made here (this entry is
records only).~~ **Landed 2026-10-16, branch `r10-ci-load-capture`** (§2c; the
entry below, after sighting 2).

**THE REFUTATION CONDITION, written before the next run rather than after it.**
Either of these reopens R10 as a defect in the transport's drain or in the test
server, not a load flake:
- **a second sighting with the load recorded and under the 3.0 ceiling** — a
  soak iteration qualifies, because the soak checks quiescence first and runs
  `prompt10_rtmp` whole; a CI sighting qualifies only once the load capture
  above exists; *(for a CI sighting, the ceiling is envelope-relative and per
  CPU, by the user's ruling of 2026-10-16, recorded after sighting 2; the 3.0
  ceiling stands for the normative machine and the soak. §2c.)*
- **the drain approaching its bound on a quiescent run** — say, any quiescent
  run of this test above 2.5 s, against about 0.4 s today.

Until then it rides the watch list (`docs/soak-protocol.md` §5), in R7's shape.
The re-run's green is not evidence of absence; it is the reason the entry exists.

**The widened class, linked.** PR #33's keyless fix (`40e96e6`) means the
hardware-gated stream tests now open real publishers and stream threads. Their
stops ~~are bounded by the same 500 ms `STREAM_THREAD_STOP_TIMEOUT` as~~
*were* bounded by the same 500 ms `STREAM_THREAD_STOP_TIMEOUT` as
`close_error_seam_fails_loudly_with_the_network_token`'s flake — **Finding R11**,
below. *(Since `bb7d4a2`, 2026-09-26: that wait hangs off the thread's exit
event with an 1100 ms deadlock backstop — R11 resolved. This drain's 5 s bound
is unchanged and still open.)* ~~…'s load flake, recorded under § 11's "SPEC v0.4.6" entry. So the stream
suites now carry two wall-clock bounds in R9's class, this drain and that
teardown, and a loaded run can trip either.~~ *Corrected 2026-09-25, a day after
it was written (§2c): R11's second sighting was under the quiescence ceiling, so
the 500 ms flake does NOT track load and is not R9's class.* What links the two
is narrower and true: **both are wall-clock bounds on another thread's
progress** — this drain on the test server's reader, that teardown on the stream
thread's exit. A run that trips either is a sighting to record, not a load
excuse.

**Sighting 2, recorded 2026-10-16 (§2c: the entry above stands unedited, its
heading's "once" struck).** Attempt 1 of main's run `38909746941`, at
`be7a5d8` (PR #46's merge), failed the `rust` job in the workspace test step
(job `117002280437`, hosted `macos-14-arm64` image 20260831.0302.1, runner
`GitHub Actions 1000003173`). The signature is identical:
`prompt10_rtmp::telemetry_tick_wires_the_live_session_counter` panicked at
`prompt10_rtmp.rs:1570:5`, `drained reads 0 on the tick`, with `test result:
FAILED. 19 passed; 1 failed; 1 ignored`. The test ran about 5.46 s: the
preceding test finished at 13:43:31.58Z, and this one failed at
13:43:37.04Z, the drain poll's 5 s expiring, as in sighting 1. It was green
on rerun: attempt 2 (job `117004945478`, runner `GitHub Actions
1000003175`) read workspace 511/0/4, the test `ok`, and all 30 floor lines
at or above their floors.
- **Not the change under review.** `be7a5d8`'s tree (`f85ec76`) is identical
  to `4bbf409`'s, PR #46's head run `38844489887` passed this job, and PR #46
  touched no Rust.
- **It does not meet the refutation condition.** The workflow still echoed
  no load (`ci.yml` had no `vm.loadavg` or `uptime`), so a CI sighting could
  not qualify. R10 stays open (watch): 2 sightings, both on CI, both with
  the load unrecorded.
- ~~**The CI load capture stays owed.** It is the only thing that lets a CI
  sighting count.~~ **Landed in the same PR** (§2c, 2026-10-16; the entry
  below).

**Sighting 3, recorded 2026-10-26 (§2c: the entries above stand unedited,
the heading's "twice" struck).** Main's merge run `40250323983` at `3afb73b`
(PR #67's merge), attempt 1, its only attempt, rust job `121340580334`
(hosted `macos-14-arm64`, read job-exact by `gh api --allow-escape-sequences
…/actions/jobs/121340580334/logs`), the `prompt10_rtmp` floor step (named
step 19, UI 25), 00:42:55–00:43:10Z:
`prompt10_rtmp::stream_buffer_ms_moves_with_load_not_a_constant` panicked at
`prompt10_rtmp.rs:1354:5`, `a drained transport reads 0 (read
78.3355091383812 ms)`, with `test result: FAILED. 19 passed; 1 failed; 1
ignored`. The same test was `ok` in the workspace step at 00:41:19.42Z. Not
rerun.
- **A sibling test, R10's bound exactly.** It publishes 64 × 60,005-byte video
  payloads (a 5-byte header and 60,000) into a stalled double, un-stalls it,
  and polls 5 s for `buffer_ms() == 0.0`: the drain this entry's first
  sighting describes, read from the publisher's counter instead of the tick.
  The test is unchanged since `619846c`; PR #67's `crates/` diff is
  `prompt04.rs` and `prompt11_watchdog.rs` alone.
- **The remainder:** 78.3355 ms × 6,128,000 bps / 8000 = **60,005 bytes, one
  whole frame**, on a frame boundary. The counter is credited per frame at
  send and debited per frame only after `write_media` returns (`rtmp.rs:336`,
  `:870`), so a reading is always whole frames, and stuck cannot be told from
  slow. The double resumes reading within 20 ms of the un-stall
  (`prompt10_rtmp.rs:852–857`).
- **The blind spot:** the floor steps sat outside both `[ci-load]` windows.
  The nearest reading was gpu-post, 5.97 per CPU at 00:42:29Z, 41 s before.
  So the refutation condition's first leg ("the load recorded and under the
  ceiling") could be neither met nor failed.
- **The defenses:** R10's own test (`telemetry_tick_wires…`) was `ok` in both
  runs of the suite in the same job (00:41:23.58Z, 00:43:10.89Z); the control
  plane (200/200) and the dress (29/29) were green; the workspace step was
  green, 549/0/4.
- **Ruled 2026-10-25 (the user's word):** R10's sighting 3, **load-blind, not
  clean, not environment.** R10 moves to **hunt pending-active**. The
  `[ci-load]` extension is a small workflow PR ahead of the device-audio
  build, covering the `prompt10_rtmp` floor step and every floor step that
  carries a network or timing assertion: PR #68, merged as `0421933` (the
  prompt map's test-hygiene records, below). From `0421933` a floor-step
  sighting carries `floor-*` readings, read against the floor-bracket
  envelope.

**The CI load capture, landed 2026-10-16** (branch `r10-ci-load-capture`, the
user's order of 2026-10-16: arm R10's reopen rule).
- **The workflow delta** (`ci.yml`, the rust job only; 46 lines added, none
  removed). Two steps bracket the workspace test step, which is unchanged:
  the same command, flags and parallelism.
  - Before it: a `[ci-load] pre` line, then a detached sampler that writes a
    `[ci-load] sample` line every 10 s to `$RUNNER_TEMP`.
  - After it, `if: always()`: a `[ci-load] post` line, the sampler stopped,
    and every sample printed, so a failing run carries them.
  - Each line carries a UTC timestamp, the 1, 5 and 15 minute load averages
    (`sysctl vm.loadavg`), the CPU count (`hw.ncpu`) and memory in MiB
    (`hw.memsize`; `vm_stat`'s free, inactive and speculative pages; swap
    used). A sighting's ~5 s window is matched to the samples by the log's
    own timestamps.
  - **The tree differs from the order's wording.** The order named
    `/proc/loadavg`, `nproc` and `free -m`. The runner is macOS
    (`macos-14-arm64`), where none of the three exists, so the same readings
    come from `sysctl` and `vm_stat`. **Approved as recorded** (the user's
    ruling of 2026-10-16, item 2; below).
  - The logging cannot fail the job: every command is guarded with
    `|| true`, the sampler runs detached with its output in a file, and both
    steps are continue-on-error. Locally, both scripts exited 0 under
    `bash -e` and `bash -eo pipefail` with `RUNNER_TEMP` unset, unwritable,
    and with the post step run without the pre step.
- **The failure path, proved on CI** (the PR #47 pattern). Throwaway branch
  `r10-load-falsify-ci-temp`, commit `99b8054` on `32cc054`, as draft PR #48:
  the workspace test step exits 1 after 25 s, before cargo runs.
  Run `38994605707`, the rust job (job `117284268391`, runner `GitHub
  Actions 1000003178`, `macos-14-arm64` image 20260831.0302.1):
  - The capture's first step succeeded:
    `[ci-load] pre 2026-10-17T02:22:28Z load1=10.38 load5=11.06 load15=6.94 ncpu=3 mem_mib total=7168 free=454 inactive=2364 speculative=675 swap_used=0.00M`.
  - The test step failed as forced: `##[error]Process completed with exit
    code 1.` at 02:22:53.79Z.
  - The `if: always()` step succeeded. It printed the post line and all
    three samples, 10 s apart:

        [ci-load] post 2026-10-17T02:22:53Z load1=12.53 load5=11.54 load15=7.23 ncpu=3 mem_mib total=7168 free=115 inactive=2471 speculative=609 swap_used=0.00M
        [ci-load] sample 2026-10-17T02:22:28Z load1=10.38 load5=11.06 load15=6.94 ncpu=3 mem_mib total=7168 free=387 inactive=2364 speculative=676 swap_used=0.00M
        [ci-load] sample 2026-10-17T02:22:38Z load1=11.61 load5=11.31 load15=7.08 ncpu=3 mem_mib total=7168 free=167 inactive=2275 speculative=769 swap_used=0.00M
        [ci-load] sample 2026-10-17T02:22:48Z load1=12.41 load5=11.49 load15=7.19 ncpu=3 mem_mib total=7168 free=106 inactive=2343 speculative=710 swap_used=0.00M

    The samples at :38 and :48 were written during the failing step, so
    the sampler outlives the step that starts it.
  - The job's conclusion is failure, from the forced step alone. The steps
    after it skipped, as on any red test step. The control-plane and dress
    jobs ran unchanged and succeeded.
  - **The first CI load datum.** Straight after clippy, the 3-CPU runner
    read a one-minute load of 10.38 rising to 12.53 across the 25 s, with
    free memory falling from 454 to 106 MiB of 7168.
  - PR #48 was closed unmerged (2026-10-17T02:26:31Z), and its branch was
    deleted, locally and on `origin`.
- **R10's reopen rule is armed.** From this PR on, a CI sighting in the
  workspace test step carries its load: the samples on either side of the
  failing test's window decide it. ~~With the load recorded and under the
  3.0 ceiling, it meets the refutation condition's first leg and reopens
  R10 as a defect. At or above the ceiling, it is a load sighting, and the
  watch holds.~~ Its load is read against the CI ceiling ruled below
  (§2c, 2026-10-16).
- ~~**Put to the user, not decided here: the ceiling on a CI runner.**~~
  **The ceiling on a CI runner: ruled 2026-10-16, below** (§2c). The 3.0
  ceiling is the normative machine's (12 CPUs). The runner has 3, and its
  first datum read 10.38–12.53 at clippy's end. Read as written, a CI
  sighting at that load is a load sighting, so the armed rule can fire on
  CI only if the runner falls under 3.0 during the tests. Whether the CI
  reading should use a ceiling of its own (say, per CPU) is the user's
  ruling. The capture logs `ncpu` on every line so either reading can be
  made from the same samples.

**The CI ceiling, ruled 2026-10-16** (the user's ruling for Finding R10:
adopt and record).
- **On the normative machine the quiescence ceiling stands unchanged**: 3.0
  on 12 CPUs.
- **On CI the ceiling is envelope-relative and per CPU.** A CI sighting
  counts toward reopening when load1/ncpu during the sighting's window ~~sits
  within the observed green envelope for the rust test step~~ *(amended
  2026-10-16: a sighting at or below the derived ceiling counts, below the
  envelope included; the amended ruling, below)*.
  - ~~**Interim ceiling: 5.0 per CPU** (load1 15 on the current 3-CPU
    runner), set from the first two instrumented runs (green envelope
    ≈2.5–4.5 per CPU).~~ *Struck by the amended ruling of 2026-10-16,
    below: the ceiling is derived (§2c).*
  - A sighting above the ceiling is attributed to environment and does not
    count.
  - Every instrumented run extends the envelope. Each sighting is recorded
    with its samples and the envelope to date.
- **Rationale, recorded with the ruling.** CI load is endogenous: the suite
  saturates its runner by design. An absolute ceiling (3.0, or 3.0 scaled
  per CPU to 0.75) would never be met on CI, and the rule would stay
  disarmed on the only surface R10 has ever appeared on. What marks a
  real-bug signal on CI is failure under typical load, not under low load.
- **The envelope to date, read from the tree** (load1/ncpu; ncpu 3 on both):

  | Run | The step | load1/ncpu |
  |---|---|---|
  | `38994994256`, PR #49 at `70712f3`, green (511/0/4) | the workspace tests: the test build finished at 02:29:04.41 (`Finished` in 43.92 s), the tests ran from 02:29:04.49 to the post line at 02:29:56 | whole step 1.25–4.50 (pre, 10 samples, post). While building: 1.25, 1.31, 2.50, 3.75, 4.15. While the tests ran: 4.50, 4.23, 4.40, 4.36, 3.74, so 3.74–4.50. The R10 test passed at 02:29:35.13, between 4.40 and 4.36 |
  | `38994605707`, the failure-path proof, red by design | no tests: a forced `exit 1` after 25 s, straight after clippy | 3.46–4.18 (pre, 3 samples, post) |
  | `38996601613`, PR #49 at `68a8388`, green (511/0/4) | the workspace tests: the test build finished at 02:54:00.37 (`Finished` in 48.37 s); the post line at 02:54:49 | whole step 3.21–4.90 (pre, 10 samples, post). While building: 3.54, 3.21, 3.34, 3.96, 4.77. While the tests ran: 4.66, 4.63, 4.57, 4.90, 4.28, so 4.28–4.90. The R10 test passed at 02:54:26.87, between 4.57 and 4.90 |
  | `38996990687`, main at `ac99d49` (PR #49's merge), green (511/0/4) | the workspace tests: the test build finished at 03:00:12.45 (`Finished` in 45.59 s); the post line at 03:01:00 | whole step 3.54–8.18 (pre, 10 samples, post). While building: 3.76, 3.54, 5.28, 6.67, 8.18. While the tests ran: 7.81, 7.20, 7.07, 6.38, 5.45, so 5.45–7.81. The R10 test passed at 03:00:40.48, between 7.07 and 6.38 |
  | `39002975513`, PR #50 at `6bf10c7`, green (515/0/4) | the workspace tests: the test build finished at 04:34:25.38 (`Finished` in 43.84 s); the post line at 04:35:19 | whole step 1.81–6.95 (pre, 10 samples, post). While building: 1.89, 1.81, 3.14, 5.06, 6.43. While the tests ran: 6.65, 6.95, 6.16, 6.06, 5.36, so 5.36–6.95. The R10 test passed at 04:34:57.28, between 6.16 and 6.06 |
  | `39005571035`, PR #50 at `bfd5397` (its head), green (515/0/4) | the workspace tests: the test build finished at 05:16:39.82 (`Finished` in 47.17 s); the post line at 05:17:35 | whole step 2.38–6.69 (pre, 10 samples, post). While building: 2.38, 2.59, 3.64, 5.47, 6.68. While the tests ran: 6.69, 6.18, 6.27, 5.46, 5.89, 5.16, so 5.16–6.69. The R10 test passed at 05:17:11.93, between 6.27 and 5.46 |
  | `39005941546`, main at `9e6239c` (PR #50's merge), green (515/0/4) | the workspace tests: the test build finished at 05:22:50.24 (`Finished` in 52.41 s); the post line at 05:23:38 | whole step 4.50–**9.67** (pre, 10 samples, post): the pre line read load1 29.00, straight after clippy, the highest yet. While building: 9.67, 8.33, 7.99, 7.86, 7.42, 7.03. While the tests ran: 6.19, 6.07, 5.61, 5.20, then the post line's 4.50, so 4.50–6.19. The R10 test passed at 05:23:17.58, between 6.07 and 5.61 |
  | `39013696039`, PR #51 at `a0b5404`, green (515/0/4) | the workspace tests: the test build finished at 07:28:41.00 (`Finished` in 30.57 s); the post line at 07:29:26 | whole step 3.08–5.43 (pre, 8 samples, post). While building: 3.30, 3.08, 3.49, 4.87. While the tests ran: 4.79, 4.70, 5.06, 5.17, then the post line's 5.43, so 4.70–5.43. The R10 test passed at 07:29:05.60, between 4.70 and 5.06 |
  | `39014021859`, PR #51 at `39db649` (its head), green (515/0/4) | the workspace tests: the test build finished at 07:34:09.36 (`Finished` in 48.54 s); the post line at 07:35:02 | whole step 3.01–5.50 (pre, 11 samples, post). While building: 3.02, 3.01, 3.47, 4.20, 5.50. While the tests ran: 5.25, 5.01, 4.67, 4.54, 4.80, then the post line's 4.17, so 4.17–5.25. The R10 test passed at 07:34:37.49, between 4.67 and 4.54 |
  | `39025809315`, main at `c1fe348` (PR #51's merge), green (515/0/4) | the workspace tests: the test build finished at 10:42:32.22 (`Finished` in 51.53 s); the post line at 10:43:24 | whole step 2.31–6.50 (pre, 11 samples, post). While building: 2.55, 2.31, 3.11, 4.39, 5.56, 6.50. While the tests ran: 6.07, 5.32, 4.53, 3.96, 3.42, then the post line's 3.20, so 3.20–6.07. The R10 test passed at 10:43:02.79, between 4.53 and 3.96 |
  | `39026282575`, PR #52 at `4403643`, green (515/0/4) | the workspace tests: the test build finished at 10:49:53.74 (`Finished` in 45.58 s); the post line at 10:50:42 | whole step 5.07–8.55 (pre, 10 samples, post). While building: 6.28, 5.52, 5.87, 6.83, 8.55. While the tests ran: 8.29, 7.92, 6.73, 6.01, 5.51, then the post line's 5.07, so 5.07–8.29. The R10 test passed at 10:50:20.98, between 6.73 and 6.01. **The 10th green run** |
  | `39031255945`, PR #52 at `cfa251b`, rust green (515/0/4); the dress job failed at step 8 (queue position 5 records it; green on rerun) | the workspace tests: the test build finished at 12:08:21.93 (`Finished` in 41.30 s); the post line at 12:09:15 | whole step 1.67–4.96 (pre, 10 samples, post). While building: 1.67, 2.04, 2.80, 4.16, 4.96. While the tests ran: 4.28, 4.30, 3.84, 3.93, 3.41, then the post line's 3.19, so 3.19–4.30. The R10 test passed at 12:08:52.76, between 3.84 and 3.93. The 11th: the rust test step was green, and the ruling counts that step's green runs |
  | `39032353226`, PR #52 at `fb154ad`, green (515/0/4) | the test build finished at 12:25:20.84; the post line at 12:26:08 | whole step 3.10–6.62. While the tests ran: 6.62, 6.30, 5.94, 5.63, then the post line's 5.10, so 5.10–6.62. The R10 test passed at 12:25:46.90, between 6.30 and 5.94. The 12th |
  | `39032691149`, PR #52 at `75e5ce7`, green (515/0/4) | the test build finished at 12:30:41.94; the post line at 12:31:39 | whole step 3.07–6.65. While the tests ran: 6.65, 6.54, 6.17, 5.32, 5.57, 4.91, then the post line's 4.71, so 4.71–6.65. The R10 test passed at 12:31:15.26, between 5.32 and 5.57. The 13th |
  | `39033038784`, PR #52 at `79d256e` (its head), rust green (515/0/4) on attempt 1; the dress failed R19 there and was green on its job-scoped rerun | the test build finished at 12:35:54.84; the post line at 12:36:47 | whole step 3.74–7.59. While the tests ran: 7.59, 7.07, 7.02, 6.33, 5.94, then the post line's 5.52, so 5.52–7.59. The R10 test passed at 12:36:25.38, between 7.02 and 6.33. The 14th |
  | `39033994454`, main at `df5530d` (PR #52's merge), green (515/0/4) | the test build finished at 12:49:55.83; the post line at 12:50:47 | whole step 2.64–4.80. While the tests ran: 4.53, 4.80, 4.19, 4.65, 4.25, then the post line's 3.87, so 3.87–4.80. The R10 test passed at 12:50:25.72, between 4.19 and 4.65. The 15th |
  | `39034929811`, PR #53 at `18a3858`, green (515/0/4) | the test build finished at 13:04:10.71; the post line at 13:05:00 | whole step 5.78–9.52. While building: 7.45, 6.60, 6.96, 7.85, 9.52, 9.32. While the tests ran: 8.73, 7.99, 6.82, 6.17, then the post line's 5.78, so 5.78–8.73. The R10 test passed at 13:04:38.28, between 7.99 and 6.82. The 16th |
  | `39035305838`, main at `49d29ae` (PR #53's merge), green (515/0/4) | the test build finished at 13:09:23.86; the post line at 13:10:16 | whole step 3.12–3.97. While the tests ran: 3.80, 3.35, 3.45, 3.12, 3.33, then the post line's 3.34, so 3.12–3.80. The R10 test passed at 13:09:54.25, between 3.12 and 3.33. The 17th |
  | `39106598820`, PR #56 at `0431441`, green (515/0/4) | the workspace tests: the test build finished at 07:10:54.59 (`Finished` in 42.62 s); the post line at 07:11:46 | whole step 2.55–4.70 (pre, 10 samples, post). While building: 2.55, 2.68, 3.30, 4.49, 4.70. While the tests ran: 4.62, 4.52, 4.43, 3.90, 4.35, then the post line's 4.35, so 3.90–4.62. The R10 test passed at 07:11:24.55, between 4.43 and 3.90. The 18th |
  | `39108195056`, PR #56 at `f81653f` (its head), green (515/0/4) | the workspace tests: the test build finished at 07:36:35.41 (`Finished` in 50.82 s); the post line at 07:37:27 | whole step 3.46–7.18 (pre, 11 samples, post). While building: 3.51, 3.46, 4.44, 5.59, 6.25, and 7.18, sampled in the second the build finished. While the tests ran: 6.16, 5.61, 4.97, 4.53, 3.91, then the post line's 3.78, so 3.78–6.16. The R10 test passed at 07:37:05.09, in the 4.97 sample's second. The 19th |
  | `39112885332`, main at `5043ccf` (PR #56's merge), green (515/0/4) | the workspace tests: the test build finished at 08:50:45.64 (`Finished` in 59.73 s); the post line at 08:51:44 | whole step 2.80–9.19 (pre, 12 samples, post). While building: 3.03, 2.80, 3.61, 5.29, 7.22, 8.80. While the tests ran: 9.19, sampled 0.4 s after the build finished, then 8.47, 7.35, 7.49, 7.15, 7.01, then the post line's 6.03, so 6.03–9.19. The R10 test passed at 08:51:19.78, between 7.49 and 7.15. The 20th |
  | `39153908984`, PR #57 at `42c06d7`, green (525/0/4) | the workspace tests: the test build finished at 18:24:54.04 (`Finished` in 50.81 s); the post line at 18:26:10 | whole step 3.05–6.29 (pre, 13 samples, post). While building: 3.28, 3.08, 3.05, 4.14, 5.48, 5.73. While the tests ran: 6.25, 5.49, 5.77, 5.82, 6.21, 5.83, 6.29, then the post line's 5.82, so 5.49–6.29. The R10 test passed at 18:25:25.60, between 5.77 and 5.82. The 21st. (`39153017826`, PR #57 at `2497bc8`, is not counted: its rust step failed, finding 10) |
  | `39157683882`, PR #57 at `85f84e1` (its head), green (525/0/4) | the workspace tests: the test build finished at 19:13:41.84 (`Finished` in 41.18 s); the post line at 19:14:48 | whole step 0.95–4.50 (pre, 11 samples, post). While building: 0.95, 0.98, 1.64, 3.47, and 4.50, sampled in the second the build finished. While the tests ran: 3.96, 3.66, 3.47, 3.04, 2.76, 2.64, then the post line's 2.23, so 2.23–3.96. The R10 test passed at 19:14:10.53, between 3.66 and 3.47. The 22nd |
  | `39364285184`, main at `88c8316` (PR #57's merge), green (525/0/4) | the workspace tests: the test build finished at 03:28:23.42 (`Finished` in 1m 02s); the post line at 03:29:41 | whole step 2.54–7.51 (pre, 14 samples, post). While building: 2.82, 2.54, 3.30, 4.23, 6.08, 7.14, 7.51. While the tests ran: 6.93, 6.41, 6.03, 5.43, 4.80, 4.83, 4.54, then the post line's 4.23, so 4.23–6.93. The R10 test passed at 03:28:57.19, between 6.03 and 5.43. The 23rd |
  | `39374921947`, PR #59 at `4b555c8`, green (534/0/4) | the workspace tests: the test build finished at 05:28:23.35 (`Finished` in 48.84 s); the post line at 05:29:38 | whole step 3.48–6.55 (pre, 13 samples, post). While building: 3.84, 3.48, 4.23, 5.00, 5.60. While the tests ran: 6.43, 6.55, 5.84, 5.37, 5.46, 5.46, 5.54, 4.87, then the post line's 4.87, so 4.87–6.55. The R10 test passed at 05:28:53.00, between 5.84 and 5.37. The 24th |
  | `39389183605` attempt 1, PR #59 at `65115ed` (its head), green (535/0/4) | the test build finished at 07:51:32.47 (`Finished` in 30.90 s); the post line at 07:52:33 | whole step 2.18–4.61 (pre, 10 samples, post). While building: 2.18, 2.55, 4.44, and 4.61, sampled in the second before the build finished. While the tests ran: 4.10, 3.89, 3.42, 4.13, 3.54, 3.92, then the post line's 3.92, so 3.42–4.13. The R10 test passed at 07:51:59.21, between 3.89 and 3.42. The 25th. (Its attempt 2's rust job is a carried copy of this one, not a run: §2b's rerun note) |
  | `39401905763` attempt 2, main at `cd3f0c6` (PR #59's merge), green (535/0/4); attempt 1's rust failed R15's sighting 2 and is not counted | the rust job executed alone in attempt 2 (from 09:45:14); the test build finished at 09:46:39.98 (`Finished` in 42.49 s); the post line at 09:47:46 | whole step 4.19–**10.06** (pre, 11 samples, post): the pre line read load1 30.19, the highest yet. While building: 10.06, 8.80, 8.01, 7.44, 7.05. While the tests ran: 6.24, 5.85, 6.02, 5.72, 5.02, 4.79, then the post line's 4.19, so 4.19–6.24. The R10 test passed at 09:47:06.81, between 5.85 and 6.02. The 26th |
  | `39409879554`, PR #60 at `66c6461` (its head), green (535/0/4) | the workspace tests: the test build finished at 10:44:39.50 (`Finished` in 48.91 s); the post line at 10:45:49 | whole step 3.97–7.90 (pre, 12 samples, post). While building: 7.90, 6.89, 6.47, 5.99, 6.26. While the tests ran: 5.92, 5.23, 4.48, 3.97, 4.86, 5.35, 5.26, then the post line's 5.72, so 3.97–5.92. The R10 test passed at 10:45:07.25, between 4.48 and 3.97. The 27th |
  | `39524251208` attempt 1, the R15 A/B's leg `5043ccf` (throwaway PR #62), green (515/0/4) | the workspace tests: the test build finished at 00:57:22.54 (`Finished` in 55.91 s); the post line at 00:58:20 | whole step 2.52–**10.26** (pre, 12 samples, post): load1 30.77 at 00:57:17, while building, above 10.06, so the maximum moves (the pinned rule, below). While building: 2.57, 2.52, 3.65, 6.92, 8.89, 10.26. While the tests ran: 9.52, 9.77, 9.22, 8.19, 7.40, 6.87, then the post line's 6.34, so 6.34–9.77. The R10 test passed at 00:57:55.57, between 9.22 and 8.19. The 28th |
  | `39524813308` attempt 1, the R15 A/B's leg `88c8316` (throwaway PR #63), green (525/0/4) | the workspace tests: the test build finished at 01:02:29.15 (`Finished` in 38.56 s); the post line at 01:03:35 | whole step 2.81–4.97 (pre, 11 samples, post). While building: 2.81, 2.90, 3.25, 3.90. While the tests ran: 3.57, 3.45, 3.71, 3.89, 4.68, 4.97, 4.86, then the post line's 4.86, so 3.45–4.97. The R10 test passed at 01:02:57.06, between 3.71 and 3.89. The 29th |
  | `39524251208` attempt 2, the R15 A/B's leg `5043ccf` (throwaway PR #62), green (515/0/4) | the workspace tests: the test build finished at 01:23:22.07 (`Finished` in 41.99 s); the post line at 01:24:17 | whole step 2.93–4.82 (pre, 10 samples, post). While building: 3.19, 2.93, 3.65, 4.74, 4.53. While the tests ran: 4.82, 4.71, 4.26, 3.76, 3.56, then the post line's 3.33, so 3.33–4.82. The R10 test passed at 01:23:54.10, between 4.26 and 3.76. The 30th |
  | `39524813308` attempt 2, the R15 A/B's leg `88c8316` (throwaway PR #63), green (525/0/4) | the workspace tests: the test build finished at 01:42:28.36 (`Finished` in 54.29 s); the post line at 01:43:40 | whole step 3.51–7.23 (pre, 13 samples, post). While building: 3.66, 3.51, 4.42, 5.35, 6.73, 7.23. While the tests ran: 6.58, 6.07, 5.58, 5.40, 5.26, 5.04, 5.33, then the post line's 4.98, so 4.98–6.58. The R10 test passed at 01:42:58.48, between 5.58 and 5.40. The 31th |
  | `39524251208` attempt 3, the R15 A/B's leg `5043ccf` (throwaway PR #62), green (515/0/4) | the workspace tests: the test build finished at 02:02:08.64 (`Finished` in 37.95 s); the post line at 02:02:57 | whole step 3.44–6.22 (pre, 9 samples, post). While building: 3.88, 3.44, 4.65, 6.03. While the tests ran: 6.22, 5.61, 5.83, 5.62, 4.91, then the post line's 5.27, so 4.91–6.22. The R10 test passed at 02:02:36.81, between 5.83 and 5.62. The 32th |
  | `39524813308` attempt 3, the R15 A/B's leg `88c8316` (throwaway PR #63), green (525/0/4) | the workspace tests: the test build finished at 02:20:51.25 (`Finished` in 57.22 s); the post line at 02:22:06 | whole step 3.03–9.22 (pre, 14 samples, post). While building: 3.74, 3.32, 3.03, 5.38, 7.15, 9.22. While the tests ran: 9.22, 8.72, 8.02, 7.34, 6.74, 7.33, 7.77, 6.72, then the post line's 6.72, so 6.72–9.22. The R10 test passed at 02:21:25.52, between 7.34 and 6.74. The 33th |
  | `39524251208` attempt 4, the R15 A/B's leg `5043ccf` (throwaway PR #62), green (515/0/4) | the workspace tests: the test build finished at 02:39:43.89 (`Finished` in 31.76 s); the post line at 02:40:26 | whole step 4.76–7.50 (pre, 8 samples, post). While building: 7.06, 6.13, 7.07, 7.50. While the tests ran: 7.18, 6.97, 5.95, 5.11, then the post line's 4.76, so 4.76–7.18. The R10 test passed at 02:40:07.44, between 6.97 and 5.95. The 34th |
  | `39524813308` attempt 4, the R15 A/B's leg `88c8316` (throwaway PR #63), green (525/0/4) | the workspace tests: the test build finished at 02:58:26.55 (`Finished` in 51.51 s); the post line at 02:59:37 | whole step 5.21–9.88 (pre, 13 samples, post). While building: 8.63, 7.46, 8.19, 9.15, 9.88, 9.35. While the tests ran: 9.05, 7.71, 7.08, 6.45, 6.23, 5.98, 5.21, then the post line's 5.21, so 5.21–9.05. The R10 test passed at 02:58:55.69, between 7.08 and 6.45. The 35th |
  | `39524251208` attempt 5, the R15 A/B's leg `5043ccf` (throwaway PR #62), green (515/0/4) | the workspace tests: the test build finished at 03:18:39.47 (`Finished` in 56.78 s); the post line at 03:19:33 | whole step 3.13–8.79 (pre, 11 samples, post). While building: 3.49, 3.13, 3.61, 5.25, 6.82, 8.19. While the tests ran: 8.79, 8.54, 7.74, 7.19, 6.71, then the post line's 6.45, so 6.45–8.79. The R10 test passed at 03:19:08.88, between 7.74 and 7.19. The 36th |
  | `39524813308` attempt 5, the R15 A/B's leg `88c8316` (throwaway PR #63), green (525/0/4) | the workspace tests: the test build finished at 03:37:45.37 (`Finished` in 42.75 s); the post line at 03:38:52 | whole step 3.80–9.18 (pre, 11 samples, post). While building: 9.18, 8.14, 7.28, 7.09, 6.87. While the tests ran: 5.94, 5.64, 4.93, 4.59, 4.27, 4.19, then the post line's 3.80, so 3.80–5.94. The R10 test passed at 03:38:12.69, between 5.64 and 4.93. The 37th |
  | `39524251208` attempt 6, the R15 A/B's leg `5043ccf` (throwaway PR #62), green (515/0/4) | the workspace tests: the test build finished at 05:45:04.95 (`Finished` in 41.96 s); the post line at 05:45:56 | whole step 2.01–6.17 (pre, 10 samples, post). While building: 2.01, 2.11, 3.64, 4.35, 5.45. While the tests ran: 6.17, 5.52, 5.36, 5.33, 4.69, then the post line's 4.69, so 4.69–6.17. The R10 test passed at 05:45:34.22, between 5.36 and 5.33. The 38th |
  | `39524813308` attempt 6, the R15 A/B's leg `88c8316` (throwaway PR #63), green (525/0/4) | the workspace tests: the test build finished at 06:04:30.79 (`Finished` in 52.49 s); the post line at 06:05:47 | whole step 2.25–6.11 (pre, 13 samples, post). While building: 2.51, 2.25, 3.08, 4.64, 5.90, 6.11. While the tests ran: 6.00, 5.75, 5.47, 5.23, 5.26, 5.06, 5.40, then the post line's 4.70, so 4.70–6.00. The R10 test passed at 06:05:02.54, between 5.47 and 5.23. The 39th |
  | `39524251208` attempt 7, the R15 A/B's leg `5043ccf` (throwaway PR #62), green (515/0/4) | the workspace tests: the test build finished at 06:23:48.56 (`Finished` in 30.07 s); the post line at 06:24:31 | whole step 3.68–6.83 (pre, 8 samples, post). While building: 6.83, 5.96, 5.83. While the tests ran: 5.74, 5.36, 4.74, 4.59, 3.91, then the post line's 3.68, so 3.68–5.74. The R10 test passed at 06:24:12.68, between 4.74 and 4.59. The 40th |
  | `39524813308` attempt 7, the R15 A/B's leg `88c8316` (throwaway PR #63), green (525/0/4) | the workspace tests: the test build finished at 06:42:11.66 (`Finished` in 40.67 s); the post line at 06:43:20 | whole step 2.20–5.00 (pre, 11 samples, post). While building: 2.35, 2.20, 2.95, 3.47. While the tests ran: 4.37, 5.00, 4.78, 4.60, 4.45, 4.72, 4.69, then the post line's 4.44, so 4.37–5.00. The R10 test passed at 06:42:41.38, between 4.78 and 4.60. The 41th |
  | `39524251208` attempt 8, the R15 A/B's leg `5043ccf` (throwaway PR #62), green (515/0/4) | the workspace tests: the test build finished at 07:01:18.72 (`Finished` in 30.18 s); the post line at 07:02:01 | whole step 3.60–5.59 (pre, 8 samples, post). While building: 4.07, 3.60, 4.70. While the tests ran: 5.59, 5.19, 4.84, 4.33, 4.18, then the post line's 3.87, so 3.87–5.59. The R10 test passed at 07:01:42.21, between 4.84 and 4.33. The 42th. (The A/B's red attempt, `39524813308` attempt 8 on leg `88c8316`, rust job `119092020542`, failed R15 in the GPU/render step and is not a row) |
  | `39412596238` attempt 3, PR #61 at `25c1114` (its head); the rust job executed alone, green (535/0/4) | the workspace tests: the test build finished at 08:11:44.91 (`Finished` in 1m 03s); the post line at 08:13:04 | whole step 2.09–**11.08** (pre, 15 samples, post): load1 33.23 at 08:11:42, while building, the highest yet, so the maximum moves again. While building: 2.29, 2.09, 3.45, 6.06, 8.17, 9.66, 11.08. While the tests ran: 9.45, 9.46, 9.27, 8.91, 9.01, 9.03, 8.94, 8.24, then the post line's 8.24, so 8.24–9.46. The R10 test passed at 08:12:17.60, between 9.27 and 8.91. The 43rd |
  | `39581255559`, main at `056fed0` (PR #60's merge), green (535/0/4) | the workspace tests: the test build finished at 10:00:55.54 (`Finished` in 30.49 s); the post line at 10:01:51 | whole step 3.73–8.35 (pre, 9 samples, post). While building: 8.35, 7.41, 7.20. While the tests ran: 6.65, 5.92, 6.06, 5.39, 4.66, 4.02, then the post line's 3.73, so 3.73–6.65. The R10 test passed at 10:01:17.92, between 6.06 and 5.39. The 44th |
  | `39588856402`, PR #64 at `9385404` (its head), green (537/0/4) | the workspace tests: the test build finished at 11:03:05.59 (`Finished` in 49.63 s); the post line at 11:04:15 | whole step 3.74–7.75 (pre, 12 samples, post). While building: 4.96, 4.64, 5.32, 6.43, 7.15. While the tests ran: 7.75, 6.88, 5.95, 5.11, 4.55, 4.69, 4.27, then the post line's 3.74, so 3.74–7.75. The R10 test passed at 11:03:34.23, between 5.95 and 5.11. The 45th |
  | `39594546676`, main at `2df03a6` (PR #64's merge), green (537/0/4) | the workspace tests: the test build finished at 11:48:33.44 (`Finished` in 33.66 s); the post line at 11:49:32 | whole step 3.84–6.25 (pre, 10 samples, post). While building: 4.58, 4.96, 5.14, 6.25. While the tests ran: 5.42, 4.66, 4.05, 4.06, 4.00, 3.84, then the post line's 3.84, so 3.84–5.42. The R10 test passed at 11:48:56.71, between 4.66 and 4.05. The 46th |
  | `39724387596`, #61's fresh merge ref (`baff7a4`, `25c1114` into `2df03a6`), green (537/0/4) | the workspace tests: the test build finished at 04:49:34.27 (`Finished` in 48.47 s); the post line at 04:50:43 | whole step 4.48–**11.38** (pre, 12 samples, post): load1 34.13 at 04:48:45, the pre line and the first sample, while building, above 11.08, so the maximum moves. While building: 11.38, 9.96, 8.94, 8.12, 7.95. While the tests ran: 7.32, 6.54, 5.61, 5.75, 6.01, 5.56, 4.81, then the post line's 4.48, so 4.48–7.32. The R10 test passed at 04:50:00.92, between 5.61 and 5.75. The 47th |
  | `39750595122`, main at `13d3a7e` (PR #61's merge), green (537/0/4) | the workspace tests: the test build finished at 08:50:18.17 (`Finished` in 32.01 s); the post line at 08:51:14 | whole step 4.21–8.08 (pre, 9 samples, post). While building: 8.08, 7.09, 7.47, 7.56. While the tests ran: 7.43, 6.76, 5.82, 5.00, 4.46, then the post line's 4.21, so 4.21–7.43. The R10 test passed at 08:50:41.10, between 6.76 and 5.82. The 48th |
  | `39765826824`, PR #66 at `7f81237` (merge ref `1348c6a`), green (537/0/4) | the workspace tests: the test build finished at 10:50:34.76 (`Finished` in 1m 01s); the post line at 10:52:00 | whole step 3.90–8.24 (pre, 15 samples, post). While building: 8.24, 7.16, 6.71, 6.58, 6.41, 6.18, 6.01. While the tests ran: 5.26, 4.53, 4.01, 3.90, 4.89, 5.04, 5.08, 5.11, then the post line's 4.84, so 3.90–5.26. The R10 test passed at 10:51:04.28, between 4.01 and 3.90. The 49th |
  | `39772900798`, PR #66 at `f24b393` (its head; merge ref `64845e8`), green (537/0/4) | the workspace tests: the test build finished at 11:46:28.11 (`Finished` in 47.75 s); the post line at 11:47:44 | whole step 3.28–8.81 (pre, 13 samples, post). While building: 8.81, 7.69, 7.55, 6.87, 6.27. While the tests ran: 5.48, 4.74, 4.04, 3.50, 3.28, 3.46, 3.54, 3.39, then the post line's 3.39, so 3.28–5.48. The R10 test passed at 11:46:57.61, between 4.04 and 3.50. The 50th |
  | `39931895990`, main at `8d13d3d` (PR #66's merge), green (537/0/4) | the workspace tests: the test build finished at 09:27:45.21 (`Finished` in 33.68 s); the post line at 09:28:45 | whole step 3.74–7.89 (pre, 10 samples, post). While building: 7.89, 6.89, 6.40, 6.23. While the tests ran: 5.52, 5.27, 4.59, 3.96, 3.74, 4.18, then the post line's 4.18, so 3.74–5.52. The R10 test passed at 09:28:09.25, between 5.27 and 4.59. The 51st |
  | `39933318916`, PR #65 at `39c05fb` (merge ref `161deb1`), green (549/0/4) | the workspace tests: the test build finished at 09:40:31.72 (`Finished` in 52.00 s); the post line at 09:41:45 | whole step 3.75–8.56 (pre, 13 samples, post). While building: 8.56, 7.55, 7.24, 7.39, 6.84, 6.91. While the tests ran: 5.98, 6.05, 6.30, 6.03, 5.29, 4.63, 4.02, then the post line's 3.75, so 3.75–6.30. The R10 test passed at 09:41:01.17, between 6.30 and 6.03. The 52nd |
  | `39934216106`, PR #65 at `f700795` (its head, docs-only; merge ref `7e26b48`); the rust job green (549/0/4), the run red on control-plane alone (R30's sighting) | the workspace tests: the test build finished at 09:47:30.85 (`Finished` in 45.75 s); the post line at 09:48:40 | whole step 4.25–9.79 (pre, 12 samples, post). While building: 9.79, 8.57, 7.77, 7.70, 7.09. While the tests ran: 6.14, 6.13, 5.80, 5.06, 5.14, 4.53, 4.25, then the post line's 4.25, so 4.25–6.14. The R10 test passed at 09:47:59.28, between 5.80 and 5.06. The 53rd |
  | `39946291340`, main at `ccef706` (PR #65's merge), green (549/0/4) | the workspace tests: the test build finished at 11:30:10.45 (`Finished` in 53.78 s); the post line at 11:31:25 | whole step 3.58–10.10 (pre, 13 samples, post). While building: 10.10, 9.00, 7.82, 7.26, 7.07, 6.79. While the tests ran: 6.04, 5.79, 5.00, 5.07, 4.37, 3.92, 3.79, then the post line's 3.58, so 3.58–6.04. The R10 test passed at 11:30:39.64, between 5.00 and 5.07. The 54th |
  | `40190112362`, PR #67 at `ad260e4` (merge ref into `ccef706`); the workspace step green (549/0/4), the job red at the `prompt11_watchdog` floor (the builder's slip) | the workspace tests: the test build finished at 11:45:32.23 (`Finished` in 37.95 s); the post line at 11:46:37 | whole step 2.60–3.50 (pre, 11 samples, post). While building: 2.89, 2.65, 3.01, 3.06. While the tests ran: 2.99, 2.68, 2.88, 2.95, 2.60, 3.50, 3.12, then the post line's 3.12, so 2.60–3.50. The R10 test passed at 11:45:58.93, between 2.88 and 2.95. The 55th, counted by the user's ruling of 2026-10-25 (steps count) |
  | `40190643535`, PR #67 at `e7600aa` (its head; merge ref `6dca3ce`), green (549/0/4) | the workspace tests: the test build finished at 11:53:25.85 (`Finished` in 51.91 s); the post line at 11:54:42 | whole step 4.86–8.43 (pre, 13 samples, post). While building: 6.80, 5.88, 6.39, 7.46, 8.09, 8.43. While the tests ran: 7.31, 6.56, 5.60, 5.91, 6.29, 5.58, 5.17, then the post line's 4.86, so 4.86–7.31. The R10 test passed at 11:53:55.45, between 5.60 and 5.91. The 56th |
  | `40250323983`, main at `3afb73b` (PR #67's merge); the workspace step green (549/0/4), the job red in the `prompt10_rtmp` floor step (R10's sighting 3) | the workspace tests: the test build finished at 00:40:44.65 (`Finished` in 1m 22s); the post line at 00:42:22 | whole step 3.39–**15.31** (pre, 18 samples, post). While building: 3.59, 3.39, 3.79, 6.14, 9.31, 11.43, 13.21, 14.44, **15.31** (load1 45.92 at 00:40:42Z). While the tests ran: 14.54, 13.15, 11.87, 10.70, 9.70, 9.18, 8.48, 7.63, 6.54, then the post line's 5.91, so 5.91–14.54. The R10 test passed at 00:41:23.58, between 10.70 and 9.70. The 57th |
  | `40262177993`, PR #68 at `b838023` (its head), green (549/0/4) | the workspace tests: the test build finished at 03:05:24.25 (`Finished` in 49.89 s); the post line at 03:06:46 | whole step 1.83–5.96 (pre, 14 samples, post). While building: 1.83, 1.90, 3.69, 5.04, 5.96, 5.81. While the tests ran: 5.79, 5.53, 5.21, 5.00, 4.46, 4.49, 4.25, 4.25, then the post line's 4.28, so 4.25–5.79. The R10 test passed at 03:05:58.82, between 5.21 and 5.00. The 58th |
  | `40266562638`, main at `0421933` (PR #68's merge), green (549/0/4) | the workspace tests: the test build finished at 03:58:52.58 (`Finished` in 53.08 s); the post line at 04:00:07 | whole step 4.46–9.68 (pre, 13 samples, post). While building: 9.68 (the pre line, load1 29.05), 8.38, 7.50, 7.42, 7.10, 6.74. While the tests ran: 5.86, 5.24, 4.56, 4.46, 4.87, 4.96, 4.89, then the post line's 5.38, so 4.46–5.86. The R10 test passed at 03:59:22.91, between 4.56 and 4.46. The 59th |
  | `40284114106`, PR #70 at `52371e8` (its head), green (580/0/5) | the workspace tests: the test build finished at 07:15:08.79 (`Finished` in 2m 54s: #70 changed `Cargo.lock`, so rust-cache missed); the post line at 07:16:36 | whole step 2.48–8.08 (pre, 26 samples, post). While building: 2.48, 3.35, 4.42, 4.74, 4.14, 4.41, 3.91, 4.02, 4.19, 3.83, 3.42, 3.05, 3.56, 4.41, 5.19, 6.56, 7.47, 8.02. While the tests ran: 8.08, 7.88, 7.35, 6.46, 6.52, 5.91, 6.01, 5.56, then the post line's 5.57, so 5.56–8.08. The R10 test passed at 07:15:44.95, between 7.35 and 6.46. The 60th |
  | `40293707305`, main at `0132d07` (PR #70's merge); the workspace step green (580/0/5), the run red on control-plane (R32's sighting 2) | the workspace tests: the test build finished at 08:42:23.90 (`Finished` in 1m 49s); the post line at 08:43:23 | whole step 2.68–4.57 (pre, 17 samples, post). While building: 4.57 (the pre line, load1 13.71), 4.51, 3.99, 3.59, 3.23, 3.00, 3.06, 2.95, 2.73, 2.94, 3.91. While the tests ran: 3.96, 3.38, 3.70, 3.29, 3.01, 2.87, then the post line's 2.68, so 2.68–3.96. The R10 test passed at 08:42:49.47, between 3.70 and 3.29. The 61st |
  | `40299126313`, PR #71 at `9965d1e` (its head), green (580/0/5) | the workspace tests: the test build finished at 09:29:22.25 (`Finished` in 33.77 s); the post line at 09:30:23 | whole step 3.91–7.47 (pre, 10 samples, post). While building: 7.47 (the pre line, load1 22.40), 6.52, 6.27, 6.24. While the tests ran: 5.40, 5.45, 5.30, 5.18, 4.59, 3.96, then the post line's 3.91, so 3.91–5.45. The R10 test passed at 09:29:49.60, between 5.30 and 5.18. The 62nd |
  | `40351769240`, PR #71 at `ba68033` (its head), green (580/0/5) | the workspace tests: the test build finished at 15:53:21.02 (`Finished` in 48.54 s); the post line at 15:54:30 | whole step 4.06–7.78 (pre, 12 samples, post). While building: 7.12 (the pre line, load1 21.37), 6.31, 7.04, 7.07, 7.52. While the tests ran: 7.78, 6.69, 5.76, 4.93, 4.30, 4.44, 4.21, then the post line's 4.06, so 4.06–7.78. The R10 test passed at 15:53:51.34, between 5.76 and 4.93. The 63rd |
  | `40441808488`, main at `ab0304a` (PR #71's merge), green (580/0/5) | the workspace tests: the test build finished at 03:03:10.08 (`Finished` in 55.65 s); the post line at 03:04:29 | whole step 4.79–8.12 (pre, 14 samples, post). While building: 7.43 (the pre line, load1 22.29), 6.57, 6.01, 7.34, 7.51, 8.12. While the tests ran: 7.91, 7.14, 6.87, 6.45, 5.90, 5.82, 5.39, 4.79, then the post line's 4.97, so 4.79–7.91. The R10 test passed at 03:03:41.39, between 6.87 and 6.45. The 64th |
  | `40374926189`, PR #73 at `5a65e59` (its head before the rebase), green (585/0/5) | the workspace tests: the test build finished at 18:09:14.20 (`Finished` in 1m 00s); the post line at 18:10:35 | whole step 4.14–7.62 (pre, 14 samples, post). While building: 7.62 (the pre line, load1 22.87), 6.71, 7.01, 6.86, 6.83, 6.76, 6.33. While the tests ran: 5.64, 5.25, 5.34, 5.44, 5.00, 4.74, 4.14, then the post line's 4.22, so 4.14–5.64. The R10 test passed at 18:09:45.36, between 5.34 and 5.44. The 65th |
  | `40442304083`, PR #73 at `7c215c7` (its rebased head), green (585/0/5) | the workspace tests: the test build finished at 03:08:11.70 (`Finished` in 56.41 s); the post line at 03:09:33 | whole step 2.24–7.59 (pre, 14 samples, post). While building: 2.24 (the pre line, load1 6.73), 2.47, 3.27, 4.95, 6.21, 7.09. While the tests ran: 7.59, 6.74, 5.76, 5.49, 5.59, 5.70, 5.14, 4.48, then the post line's 4.25, so 4.25–7.59. The R10 test passed at 03:08:44.52, between 5.76 and 5.49. The 66th |
  | `40448236533`, main at `27368f2` (PR #73's merge); the workspace step green (585/0/5), the run red on dress (R21's sighting 2) | the workspace tests: the test build finished at 04:11:51.72 (`Finished` in 48.04 s); the post line at 04:13:09 | whole step 2.42–5.61 (pre, 13 samples, post). While building: 2.54 (the pre line, load1 7.61), 2.42, 3.77, 5.04, 5.48. While the tests ran: 5.40, 5.24, 5.61, 5.10, 4.69, 5.15, 5.31, 5.06, then the post line's 5.14, so 4.69–5.61. The R10 test passed at 04:12:24.30, between 5.10 and 4.69. The 67th |
  | `40382622804`, PR #74 at `9dfd84b` (its head before the rebase), green (580/0/5) | the workspace tests: the test build finished at 19:01:01.70 (`Finished` in 46.72 s); the post line at 19:02:26 | whole step 3.62–9.28 (pre, 14 samples, post). While building: 9.28 (the pre line, load1 27.85), 8.09, 7.37, 6.97, 6.33. While the tests ran: 5.71, 5.15, 4.67, 4.15, 3.62, 3.87, 4.31, 5.23, 4.77, then the post line's 4.77, so 3.62–5.71. The R10 test passed at 19:01:32.01, between 4.67 and 4.15. The 68th |
  | `40450166227`, PR #74 at `527c065` (an intermediate head of its rebase), green (585/0/5) | the workspace tests: the test build finished at 04:32:48.45 (`Finished` in 52.43 s); the post line at 04:34:07 | whole step 4.42–7.33 (pre, 13 samples, post). While building: 7.20 (the pre line, load1 21.60), 6.30, 5.97, 6.94, 7.16, 7.33. While the tests ran: 7.00, 6.80, 6.47, 6.14, 5.25, 5.29, 4.91, then the post line's 4.42, so 4.42–7.00. The R10 test passed at 04:33:19.18, between 6.47 and 6.14. The 69th |
  | `40451987570`, PR #74 at `ae709db` (its final head), green (585/0/5) | the workspace tests: the test build finished at 04:53:04.13 (`Finished` in 59.52 s); the post line at 04:54:28 | whole step 3.34–5.81 (pre, 15 samples, post). While building: 5.40 (the pre line, load1 16.21), 4.85, 4.97, 5.14, 5.44, 5.81. While the tests ran: 5.64, 5.08, 4.89, 4.19, 3.60, 4.08, 4.26, 3.73, 3.61, then the post line's 3.34, so 3.34–5.64. The R10 test passed at 04:53:38.15, between 4.19 and 3.60. The 70th |
  | `40455238385`, main at `4bef9ed` (PR #74's merge), green (585/0/5) | the workspace tests: the test build finished at 05:26:59.38 (`Finished` in 46.00 s); the post line at 05:28:17 | whole step 1.93–6.32 (pre, 13 samples, post). While building: 2.05 (the pre line, load1 6.15), 1.93, 2.50, 4.77, 6.15. While the tests ran: 6.32, 6.17, 5.43, 4.78, 4.18, 3.71, 3.78, 3.57, then the post line's 3.34, so 3.34–6.32. The R10 test passed at 05:27:34.37, between 4.78 and 4.18. The 71st |
  | `40458035715`, PR #75 at `e048917` (its head), green (585/0/5) | the workspace tests: the test build finished at 05:56:36.43 (`Finished` in 47.45 s); the post line at 05:58:00 | whole step 2.27–4.78 (pre, 13 samples, post). While building: 2.41 (the pre line, load1 7.23), 2.27, 2.76, 3.81, 4.72. While the tests ran: 4.69, 4.02, 4.12, 3.61, 3.70, 3.81, 4.39, 4.78, then the post line's 4.35, so 3.61–4.78. The R10 test passed at 05:57:10.32, between 3.61 and 3.70. The 72nd |
  | `40460725223`, main at `6bb8019` (PR #75's merge), green (585/0/5) | the workspace tests: the test build finished at 06:24:14.41 (`Finished` in 54.48 s); the post line at 06:25:35 | whole step 4.14–8.50 (pre, 14 samples, post). While building: 8.50 (the pre line, load1 25.49), 7.42, 7.06, 6.76, 6.39, 6.32. While the tests ran: 5.95, 5.24, 4.64, 4.75, 4.74, 4.71, 4.47, 4.30, then the post line's 4.14, so 4.14–5.95. The R10 test passed at 06:24:46.40, between 4.64 and 4.75. The 73rd |

  The ruling's ≈2.5–4.5 is the green run from its third sample on
  (02:28:40). The proof run ran no tests, so it is not a green test step;
  its readings sit inside the same band. ~~Every reading to date is under
  the interim ceiling of 5.0.~~ *(§2c, 2026-10-16: true at `68a8388`;
  main's merge run went past it, below.)*

  **The envelope to date (green runs, while the tests ran): 3.74–7.81 per
  CPU across the three green runs (whole steps 1.25–8.18).** The PR head
  run's peak, 4.90 per CPU (load1 14.71 at 02:54:32), sat 0.10 under the
  interim ceiling of 5.0. Main's merge run then went past it: load1 24.53
  (8.18 per CPU) at 03:00:07 while building, and 5.45–7.81 while the tests
  ran, every one of those five samples above 5.0, with the R10 test green
  inside them.
- ~~**Put to the user, not decided here: the green envelope now crosses the
  interim ceiling.**~~ **Ruled: the ceiling moves with the envelope** (the
  user's amended ruling of 2026-10-16, below; §2c). The ruling counts a CI
  sighting when load1/ncpu in its window sits within the observed green
  envelope, and attributes a
  sighting above the 5.0 ceiling to environment. On `ac99d49` a green
  workspace step ran at 5.45–7.81 per CPU, and the R10 test passed at about
  7. Read as written, a sighting at that load would be both inside the
  green envelope and above the ceiling. Whether the interim ceiling moves
  with the envelope is the user's ruling.
- **Approved with the ruling (its items 2 and 3).**
  - **The macOS substitution is approved as recorded.** `sysctl`
    (`vm.loadavg`, `hw.ncpu`, `hw.memsize`, `vm.swapusage`) and `vm_stat`
    stand in place of `/proc/loadavg`, `nproc` and `free -m`: the same
    evidentiary content, platform-correct, and disclosed here and in the PR
    body. The order assumed Linux; that miss is the relay's.
  - **Both recorded-not-changed items are accepted as filed.** Coverage
    matches the sighting history. Job-scope sampling becomes a candidate
    amendment only if a sighting ever lands in the `prompt10_rtmp` floor
    step. The soak's R10 sighting count remains owed and rides the records.
- **Not covered, recorded.** The workspace step is where both sightings
  fell. The rust job's later step "the RTMP transport suite actually ran
  (Prompt 10 WU5)" runs `prompt10_rtmp` again, after the post line; a
  sighting there carries no samples. Other jobs are not covered either.
  Extending the capture is a separate decision (the order). The soak's
  dedicated R10 count (the register row) stays owed: this PR is
  workflow-only. **Accepted as filed** (the user's ruling of 2026-10-16,
  item 3; ~~below~~ above: the ruling's block was inserted ahead of this
  bullet. §2c, 2026-10-16).

**The CI ceiling, amended 2026-10-16: it moves with the envelope** (the
user's amended ruling for Finding R10).
1. **The fixed interim (5.0 per CPU) is struck.** It was calibrated on two
   instrumented runs, and the third run's green envelope (5.45–7.81 per CPU,
   peak 8.18) already exceeds it. Why: ceilings pinned to tiny baselines
   break. The envelope is the baseline.
2. **The ceiling is derived and recomputed at each sighting:** 1.5× the
   maximum per-CPU load observed across instrumented green runs of the
   rust test step. Once the envelope holds 10 or more green runs, the
   margin tightens to 1.25×. A sighting at or below the ceiling counts
   toward reopening; a sighting above it is attributed to environment and
   does not count.
3. **A sighting below the envelope counts.** The ceiling is the only
   exclusion. Low load cannot exculpate; it strengthens the signal.
4. **The justification datum.** Runs `38996601613` (`68a8388`) and
   `38996990687` (`ac99d49`) executed a byte-identical tree (`67249a1`, 0
   diff lines), yet per-CPU load while the tests ran moved from 4.28–4.90
   to 5.45–7.81 (1.59× peak to peak, 1.82× across the two ranges). Runner
   variance alone swings the environment nearly 2× on identical code. An
   absolute ceiling cannot represent "typical", and an envelope-relative
   one does by construction.
5. **Each sighting is recorded with its bracketing samples, the envelope
   to date, and the ceiling that applied.**

**The ceiling as it stands, derived from the tree** *(at 3 green runs;
superseded at 6 by PR #50's merge, below; §2c)*:
- Green instrumented runs to date: **3**. They are `38994994256`
  (`70712f3`), `38996601613` (`68a8388`) and `38996990687` (`ac99d49`).
  The failure-path proof `38994605707` is not green and ran no tests, so
  it is not counted. The margin is 1.5× (fewer than 10 runs).
- The maximum is taken over the whole test step, the reading the ruling's
  own "peak 8.18" uses. It is **8.18 per CPU** (load1 24.53 at
  2026-10-17T03:00:07Z, `ac99d49`).
- **The ceiling is 1.5 × 8.18 = 12.27 per CPU: load1 36.8 on the current
  3-CPU runner.** Had the maximum been read only while the tests ran
  (7.81), it would be 11.72.

**Watch line, recorded, no action.** Green peaks so far run 4.50 → 4.90 →
8.18 (whole step). If the envelope keeps climbing, that drift is a
runner-fleet or image question, not R10. To date the image is constant
(`macos-14-arm64` 20260831.0302.1, provisioner 20260828.587, on all three)
and the runner differs on each (`GitHub Actions 1000003180`, `…3184`,
`…3186`).

**The ceiling at PR #50's merge** (the envelope rows above extend it):
**6 green runs** (`70712f3`, `68a8388`, `ac99d49`, `6bf10c7`, `bfd5397`,
`9e6239c`). The maximum moves to **9.67 per CPU** (load1 29.00 at
2026-10-17T05:21:57Z, main's merge run, the step's pre line). The margin is
still 1.5× (fewer than 10 runs). **The ceiling is 1.5 × 9.67 = 14.50 per CPU,
load1 43.5 on the 3-CPU runner** (it was 12.27). The image is still
`macos-14-arm64` 20260831.0302.1. **Watch line:** whole-step peaks run
4.50 → 4.90 → 8.18 → 6.95 → 6.69 → 9.67, on one image and six different
runners. That is not monotone, but the maximum keeps moving; per the
ruling, drift is a runner-fleet or image question, not R10.

*(Run `39002620888` at `e6a6e29` is not a row: its rust job failed at
`cargo fmt --check` before the tests, so it is not a green instrumented
run.)*

**The ceiling at PR #51's merge** (the three rows above): **9 green runs**.
No new maximum (the peaks were 5.43, 5.50 and 6.50), so the ceiling **holds at
14.50 per CPU**, 1.5 × 9.67, as the user accepted on 2026-10-17. **The next
green instrumented run is the 10th, and the margin tightens to 1.25× there**
(the amended ruling's item 2). With the maximum still 9.67, that ceiling is
**12.09 per CPU**, load1 36.2 on the 3-CPU runner; it is recomputed with that
run's own samples. The image is still `macos-14-arm64` 20260831.0302.1, and
every run had a different runner.

**The ceiling at 10 green runs: 12.09 per CPU** (recorded 2026-10-17). The
10th run, `39026282575`, peaked at 8.55, so the maximum is still 9.67 and the
ceiling is **1.25 × 9.67 = 12.09 per CPU** (load1 36.2). It held at the 11th
(`39031255945`, peak 4.96). In the user's words: "Ceiling 12.09 recorded; the
downward move is the mechanism working."

**The ceiling at PR #52's merge: holds at 12.09 per CPU** (15 green runs;
the peaks since the 11th were 6.62, 6.65, 7.59 and 4.80, all under the 9.67
maximum).

**The ceiling at PR #53's merge: holds at 12.09 per CPU** (17 green runs;
the peaks were 9.52, the nearest yet to the 9.67 maximum, and 3.97).

**The ceiling at PR #56's merge: holds at 12.09 per CPU** (20 green runs;
the peaks were 4.70, 7.18 and 9.19, all under the 9.67 maximum).

**The ceiling at PR #57's merge: holds at 12.09 per CPU** (23 green runs;
the peaks were 6.29, 4.50 and 7.51, all under the 9.67 maximum).

**The ceiling at PR #59's merge: moves to 12.58 per CPU** (26 green runs;
the peaks were 6.55, 4.61 and **10.06**). The maximum moves from 9.67 to
10.06 per CPU (load1 30.19 on 3 CPUs at 2026-10-20T09:45:57Z, the pre line
of main's merge run `39401905763`, attempt 2), and the ceiling is derived
as 1.25 × 10.06 = 12.58. R10 was silent in all three.

**R10's envelope rule, pinned** (the user's ruling of 2026-10-20, item 1,
verbatim): "the green maximum is the maximum per-CPU observed anywhere in the
rust step's window — pre, samples, or post." It matches the reading the
derivation has used since 2026-10-16 ("the maximum is taken over the whole
test step").

**The ceiling through the R15 A/B and #61's rerun: moves to 12.83, then
13.85 per CPU** (43 green runs; rows 27–43, every one read job-exact). The
A/B's green attempts are green instrumented runs of the rust test step, so
they extend the envelope; its red attempt is not a row.
- **12.83:** the maximum moved from 10.06 to **10.26** per CPU, load1 30.77
  on 3 CPUs at 2026-10-21T00:57:17Z, a sample while building (the test build
  `Finished` at 00:57:22.55, in 55.91 s): the A/B's leg `5043ccf`, run
  `39524251208` attempt 1, rust job `118975083661`, green (515/0/4), read
  job-exact by `gh api --allow-escape-sequences
  repos/jonquixote/nbe/actions/jobs/118975083661/logs`. The ceiling was
  1.25 × 10.26 = 12.83 per CPU (load1 38.5).
- **13.85:** the maximum moved again, to **11.08** per CPU (applied under the
  pinned rule and reported to the user on 2026-10-21 before it was
  recorded): load1 33.23 on 3 CPUs at 2026-10-21T08:11:42Z, a sample while
  building, in #61's run `39412596238`, attempt 3, rust job `119112170748`,
  green. **The ceiling is 1.25 × 11.08 = 13.85 per CPU** (load1 41.5 on the
  3-CPU runner).
- None of the A/B's other 15 attempts peaked above 10.26. The image was
  `macos-14-arm64` 20260831.0302.1 throughout.

**The ceiling at PR #60's merge: holds at 13.85 per CPU** (44 green runs;
main's merge run peaked at 8.35, its pre line, under the 11.08 maximum).
R10 was silent in every run from row 27 to row 44.

**The ceiling at PR #64's merge: holds at 13.85 per CPU** (46 green runs;
the peaks were 7.75 and 6.25, both under the 11.08 maximum). R10 was silent
in both.

**The ceiling through #61, #66 and their merges: moves to 14.22 per CPU,
then holds** (51 green runs; rows 47–51, every one read job-exact). The
maximum moved from 11.08 to **11.38** per CPU at row 47: load1 34.13 on 3
CPUs at 2026-10-22T04:48:45Z, the pre line, while building, in #61's
fresh-ref run `39724387596`, attempt 1, rust job `119644866600`, green.
**The ceiling is 1.25 × 11.38 = 14.22 per CPU** (load1 42.7 on the 3-CPU
runner). Rows 48–51 peaked at 8.08, 8.24, 8.81 and 7.89, all under it. R10
was silent in all five. #65's run `39603865448` is red and adds no row. The
image was `macos-14-arm64` 20260831.0302.1 throughout.

**The ceiling at #65's head `39c05fb`: holds at 14.22 per CPU** (52 green
runs; row 52 peaked at 8.56). R10 was silent.

**The ceiling at PR #65's merge: holds at 14.22 per CPU** (54 green runs;
rows 53 and 54 peaked at 9.79, `f700795`'s rust job, and 10.10, main at
`ccef706`, both pre lines, under the 11.38 maximum). R10 was silent in
both. *(Row 53 was missed in #65's bookend report, which counted 53 runs;
the builder's slip, corrected here.)*

**The ceiling through #67 and its merge: moves to 19.13 per CPU** (57 green
runs; the user's ruling 2 of 2026-10-25, "steps count, per the rule's
wording"). A green workspace step counts even when its job is red elsewhere:
- row 55 is `ad260e4`'s (3.50), its job red at the `prompt11_watchdog` floor.
  *(§2c: the builder had excluded it by the job's conclusion, "a red job, so it
  adds no envelope row"; the ruling names that a misapplication, corrected.)*
- row 56 is `e7600aa`'s (8.43);
- **row 57, main at `3afb73b`, moves the maximum from 11.38 to 15.31** (load1
  45.92 on 3 CPUs at 2026-10-26T00:40:42Z, while building; three consecutive
  samples read above the old 14.22). Its job was red on R10's sighting 3, in
  the `prompt10_rtmp` floor step, outside this window. **The ceiling is 1.25 ×
  45.92/3 = 19.13 per CPU** (load1 57.4 on the 3-CPU runner).

**Pinned with it** (the user's words): "a mixed step (green sub-step beside a
failed assertion) is excluded pending its row's ruling; a step that failed to
run removes its own evidence, so today's counts."

**The ceiling at #68's head and its merge: holds at 19.13 per CPU** (59 green
runs; rows 58 and 59 peaked at 5.96 and 9.68, under the 15.31 maximum). R10
was silent in both.

**The ceiling at #70's head and its merge: holds at 19.13 per CPU** (61 green
runs; rows 60 and 61 peaked at 8.08 and 4.57, under the 15.31 maximum). Row
60's build ran 2m 54s on a rust-cache miss (#70 changed `Cargo.lock`); its
peak, 8.08, is the first sample after the build. R10 was silent in both.

**The ceiling through #71 and #73, their heads and merges: holds at 19.13
per CPU** (67 green runs; rows 62–67 peaked at 7.47, 7.78, 8.12, 7.62, 7.59
and 5.61, under the 15.31 maximum). R10 was silent in all six.

**The ceiling through #74, its heads and its merge: holds at 19.13 per CPU**
(71 green runs; rows 68–71 peaked at 9.28, 7.33, 5.81 and 6.32, under the
15.31 maximum). R10 was silent in all four.

**The ceiling through #75 and its merge: holds at 19.13 per CPU** (73 green
runs; rows 72 and 73 peaked at 4.78 and 8.50, under the 15.31 maximum). R10
was silent in both.

**R10's envelope is workspace-step-only** (the user's ruling of 2026-10-21).
"The rust step's window" in the pinned rule is the workspace test step's:
its pre line, samples and post line. The GPU/render step's `gpu-*` lines,
added by #64, are not R10's.

**R15's GPU-step envelope** (the same ruling). The `gpu-*` lines start an
envelope of their own, under the pinned rule:
- the maximum per-CPU anywhere in the GPU/render step's window (`gpu-pre`,
  `gpu-sample`, `gpu-post`);
- green runs only;
- a ceiling of 1.25× that maximum.

A `three_frames…` failure in that step is read against it.

| GPU row | Run | gpu-pre | Whole step | gpu-post |
|---|---|---|---|---|
| G1 | `39588856402` attempt 1, rust job `119184455758`, PR #64 at `9385404`, green (`prompt04` 19) | 3.74 | 3.74–3.76 (pre, 1 sample, post; 11:04:15–11:04:19Z) | 3.76 |
| G2 | `39594546676` attempt 1, rust job `119203495290`, main at `2df03a6` (PR #64's merge), green (`prompt04` 19) | **3.84** | 3.61–3.84 (pre, 1 sample, post; 11:49:32–11:49:36Z) | 3.61 |
| G3 | `39724387596` attempt 1, rust job `119644866600`, #61's fresh ref (`baff7a4`), green (`prompt04` 19) | **4.48** | 4.18–4.48 (pre, 1 sample, post; 04:50:44–04:50:48Z) | 4.18 |
| G4 | `39750595122` attempt 1, rust job `119729900179`, main at `13d3a7e` (PR #61's merge), green (`prompt04` 19) | 4.21 | 3.90–4.21 (pre, 1 sample, post; 08:51:14–08:51:17Z) | 3.90 |
| G5 | `39765826824` attempt 1, rust job `119780898668`, PR #66 at `7f81237`, green (`prompt04` 19) | 4.84 | 4.84–**4.88** (pre, 1 sample, post; 10:52:00–10:52:06Z) | **4.88** |
| G6 | `39772900798` attempt 1, rust job `119804589136`, PR #66 at `f24b393`, green (`prompt04` 19) | 3.39 | 3.39–3.58 (pre, 1 sample, post; 11:47:44–11:47:50Z) | 3.58 |
| G7 | `39931895990` attempt 1, rust job `120341940415`, main at `8d13d3d` (PR #66's merge), green (`prompt04` 19) | 4.18 | 4.01–4.18 (pre, 1 sample, post; 09:28:45–09:28:49Z) | 4.01 |
| G8 | `39933318916` attempt 1, rust job `120346686856`, PR #65 at `39c05fb`, green (`prompt04` 19) | 3.75 | 3.53–3.75 (pre, 1 sample, post; 09:41:45–09:41:50Z) | 3.53 |
| G9 | `39934216106` attempt 1, rust job `120349653140`, PR #65 at `f700795` (the rust job green) | 4.25 | 3.97–4.25 (pre, 1 sample, post; 09:48:40–09:48:45Z) | 3.97 |
| G10 | `39946291340` attempt 1, rust job `120389662330`, main at `ccef706` (PR #65's merge), green (`prompt04` 19) | 3.58 | 3.58–3.66 (pre, 1 sample, post; 11:31:25–11:31:31Z) | 3.66 |
| G11 | `40190112362` attempt 1, rust job `121159101575`, PR #67 at `ad260e4` (the GPU step green, `prompt04` 19; the job red at the `prompt11_watchdog` floor) | 3.12 | 2.90–3.12 (pre, 1 sample, post; 11:46:37–11:46:42Z) | 2.90 |
| G12 | `40190643535` attempt 1, rust job `121160705915`, PR #67 at `e7600aa`, green (`prompt04` 19) | 4.86 | 4.50–4.86 (pre, 1 sample, post; 11:54:42–11:54:47Z) | 4.50 |
| G13 | `40250323983` attempt 1, rust job `121340580334`, main at `3afb73b` (PR #67's merge; the GPU step green, `prompt04` 19; the job red on R10's sighting 3) | 5.91 | 5.91–**5.97** (pre, 1 sample, post; 00:42:22–00:42:29Z) | **5.97** |
| G14 | `40262177993` attempt 1, rust job `121376701187`, PR #68 at `b838023`, green (`prompt04` 19) | 4.28 | 3.97–4.28 (pre, 1 sample, post; 03:06:47–03:06:53Z) | 3.97 |
| G15 | `40266562638` attempt 1, rust job `121390146285`, main at `0421933` (PR #68's merge), green (`prompt04` 19) | 5.32 | 5.32 (pre, 1 sample, post; 04:00:07–04:00:12Z) | 5.32 |
| G16 | `40284114106` attempt 1, rust job `121443674416`, PR #70 at `52371e8`, green (`prompt04` 19) | 5.28 | 5.28–5.39 (pre, 1 sample, post; 07:16:36–07:16:42Z) | 5.39 |
| G17 | `40293707305` attempt 1, rust job `121474548526`, main at `0132d07` (PR #70's merge; the rust job green, `prompt04` 19; the run red on control-plane, R32's sighting 2) | 2.68 | 2.68 (pre, 1 sample, post; 08:43:23–08:43:26Z) | 2.68 |
| G18 | `40299126313` attempt 1, rust job `121492392391`, PR #71 at `9965d1e`, green (`prompt04` 19) | 3.91 | 3.70–3.91 (pre, 1 sample, post; 09:30:23–09:30:27Z) | 3.70 |
| G19 | `40351769240` attempt 1, rust job `121673431674`, PR #71 at `ba68033`, green (`prompt04` 19) | 4.06 | 3.94–4.06 (pre, 1 sample, post; 15:54:30–15:54:34Z) | 3.94 |
| G20 | `40441808488` attempt 1, rust job `121983656691`, main at `ab0304a` (PR #71's merge), green (`prompt04` 19) | 4.97 | 4.60–4.97 (pre, 1 sample, post; 03:04:29–03:04:34Z) | 4.60 |
| G21 | `40374926189` attempt 1, rust job `121755258657`, PR #73 at `5a65e59`, green (`prompt04` 19) | 4.22 | 4.22–4.60 (pre, 1 sample, post; 18:10:35–18:10:40Z) | 4.60 |
| G22 | `40442304083` attempt 1, rust job `121985247669`, PR #73 at `7c215c7`, green (`prompt04` 19) | 4.25 | 3.94–4.25 (pre, 1 sample, post; 03:09:33–03:09:39Z) | 3.94 |
| G23 | `40448236533` attempt 1, rust job `122004538883`, main at `27368f2` (PR #73's merge; the rust job green, `prompt04` 19; the run red on dress, R21's sighting 2) | 5.14 | 4.86–5.14 (pre, 1 sample, post; 04:13:09–04:13:13Z) | 4.86 |
| G24 | `40382622804` attempt 1, rust job `121782578211`, PR #74 at `9dfd84b`, green (`prompt04` 19) | 4.77 | 4.12–4.77 (pre, 1 sample, post; 19:02:26–19:02:33Z) | 4.12 |
| G25 | `40450166227` attempt 1, rust job `122010764931`, PR #74 at `527c065`, green (`prompt04` 19) | 4.42 | 4.42–4.60 (pre, 1 sample, post; 04:34:07–04:34:13Z) | 4.60 |
| G26 | `40451987570` attempt 1, rust job `122016602116`, PR #74 at `ae709db`, green (`prompt04` 19) | 3.34 | 3.34 (pre, 1 sample, post; 04:54:28–04:54:35Z) | 3.34 |
| G27 | `40455238385` attempt 1, rust job `122026909134`, main at `4bef9ed` (PR #74's merge), green (`prompt04` 19) | 3.34 | 3.34–3.36 (pre, 1 sample, post; 05:28:17–05:28:22Z) | 3.36 |
| G28 | `40458035715` attempt 1, rust job `122035836637`, PR #75 at `e048917`, green (`prompt04` 19) | 4.35 | 4.03–4.35 (pre, 1 sample, post; 05:58:00–05:58:06Z) | 4.03 |
| G29 | `40460725223` attempt 1, rust job `122044419319`, main at `6bb8019` (PR #75's merge), green (`prompt04` 19) | 4.14 | 3.86–4.14 (pre, 1 sample, post; 06:25:35–06:25:41Z) | 3.86 |

After G1 the maximum was 3.76 and the ceiling 4.70. **G2 moves the maximum to
3.84** (load1 11.52 on 3 CPUs, the `gpu-pre` line), so **the ceiling is
1.25 × 3.84 = 4.80 per CPU** (load1 14.4), at 2 green runs. This was applied
under the pinned rule and reported to the user before it was recorded.

**G3 moves the maximum to 4.48** (load1 13.45 on 3 CPUs, the `gpu-pre` line
and the sample), inside G2's 4.80 ceiling, so the ceiling became 1.25 × 4.48 =
5.60. **G5 moves it to 4.88** (load1 14.63, the `gpu-post` line), inside
5.60, so **the ceiling is 1.25 × 4.88 = 6.10 per CPU** (load1 18.3). G4, G6
and G7 peaked under it, and `three_frames…` passed the GPU step in all five.
G8 (#65 at `39c05fb`) peaked at 3.75, under it, as did G9 (4.25) and G10
(3.66).

**G11–G15, by the user's ruling of 2026-10-25 (steps count).** G11 (`ad260e4`,
3.12) and G12 (4.86) peaked under 6.10. **G13, main at `3afb73b`, moves the
maximum to 5.97** (load1 17.91 on 3 CPUs, the `gpu-post` line), inside 6.10,
so **the ceiling is 1.25 × 5.97 = 7.46 per CPU** (load1 22.4). G14 (4.28) and
G15 (5.32) peaked under it, and `three_frames…` passed the GPU step in all
five. G16 (#70 at `52371e8`, 5.39) and G17 (main `0132d07`, 2.68) peaked under
it too, and `three_frames…` passed the GPU step in both.
G18–G23 peaked under it too: 3.91 and 4.06 (#71 at `9965d1e` and
`ba68033`), 4.97 (main `ab0304a`), 4.60 and 4.25 (#73 at `5a65e59` and
`7c215c7`), 5.14 (main `27368f2`); `three_frames…` passed the GPU step in
all six.
G24–G27 peaked under it too: 4.77, 4.60 and 3.34 (#74 at `9dfd84b`,
`527c065` and `ae709db`), 3.36 (main `4bef9ed`); `three_frames…` passed the
GPU step in all four.
G28 and G29 peaked under it too: 4.35 (#75 at `e048917`) and 4.14 (main
`6bb8019`); `three_frames…` passed the GPU step in both.

**The floor-bracket envelope** (the user's ruling of 2026-10-26, on #68's
two-key pass; the third envelope). From `0421933`, `floor-pre`, `floor-sample`
and `floor-post` bracket every floor step (named steps 8 and 36, the 27 floor
and count-gate steps between them). Its rule is the pinned one:
- the maximum per-CPU anywhere in the bracket's window (`floor-pre`, the
  samples, `floor-post`);
- green runs only;
- a ceiling of 1.25× that maximum.

A floor-step sighting is read against it. #69's floor window (`0fb9d5e`,
3.47–4.14) is not counted: its run was red by design, as #49's failure-path
proof was, and its bracket held a failed floor step (a mixed step).

| Floor row | Run | floor-pre | Whole bracket | floor-post |
|---|---|---|---|---|
| F1 | `40262177993` attempt 1, rust job `121376701187`, PR #68 at `b838023`, green (33/33 floor lines) | 3.97 | 3.44–**4.72** (pre, 10 samples, post; 03:06:53–03:08:33Z) | 4.24 |
| F2 | `40266562638` attempt 1, rust job `121390146285`, main at `0421933` (PR #68's merge), green (33/33 floor lines) | **5.32** | 3.94–**5.32** (pre, 7 samples, post; 04:00:12–04:01:13Z) | 3.94 |
| F3 | `40284114106` attempt 1, rust job `121443674416`, PR #70 at `52371e8`, green (34/34 floor lines) | 5.39 | 4.93–**8.01** (pre, 15 samples, post; 07:16:43–07:19:15Z) | 5.99 |
| F4 | `40293707305` attempt 1, rust job `121474548526`, main at `0132d07` (PR #70's merge; the rust job green, 34/34 floor lines; the run red on control-plane, R32's sighting 2) | 2.52 | 1.90–3.52 (pre, 10 samples, post; 08:43:26–08:45:01Z) | 3.52 |
| F5 | `40299126313` attempt 1, rust job `121492392391`, PR #71 at `9965d1e`, green (34/34 floor lines) | 3.70 | 1.89–3.70 (pre, 6 samples, post; 09:30:27–09:31:25Z) | 1.89 |
| F6 | `40351769240` attempt 1, rust job `121673431674`, PR #71 at `ba68033`, green (34/34 floor lines) | 3.94 | 3.19–3.94 (pre, 7 samples, post; 15:54:34–15:55:40Z) | 3.21 |
| F7 | `40441808488` attempt 1, rust job `121983656691`, main at `ab0304a` (PR #71's merge), green (34/34 floor lines) | 4.60 | 3.47–4.60 (pre, 7 samples, post; 03:04:34–03:05:44Z) | 3.89 |
| F8 | `40374926189` attempt 1, rust job `121755258657`, PR #73 at `5a65e59`, green (34/34 floor lines) | 4.60 | 2.48–4.60 (pre, 9 samples, post; 18:10:41–18:12:04Z) | 2.48 |
| F9 | `40442304083` attempt 1, rust job `121985247669`, PR #73 at `7c215c7`, green (34/34 floor lines) | 3.94 | 3.53–4.26 (pre, 10 samples, post; 03:09:39–03:11:15Z) | 3.53 |
| F10 | `40448236533` attempt 1, rust job `122004538883`, main at `27368f2` (PR #73's merge; the rust job green, 34/34 floor lines; the run red on dress, R21's sighting 2) | 4.86 | 2.85–4.86 (pre, 8 samples, post; 04:13:14–04:14:34Z) | 3.67 |
| F11 | `40382622804` attempt 1, rust job `121782578211`, PR #74 at `9dfd84b`, green (34/34 floor lines) | 4.12 | 2.99–4.12 (pre, 9 samples, post; 19:02:34–19:04:01Z) | 3.34 |
| F12 | `40450166227` attempt 1, rust job `122010764931`, PR #74 at `527c065`, green (34/34 floor lines) | 4.60 | 2.88–4.60 (pre, 9 samples, post; 04:34:13–04:35:41Z) | 3.01 |
| F13 | `40451987570` attempt 1, rust job `122016602116`, PR #74 at `ae709db`, green (34/34 floor lines) | 3.34 | 3.34–4.29 (pre, 10 samples, post; 04:54:35–04:56:13Z) | 3.38 |
| F14 | `40455238385` attempt 1, rust job `122026909134`, main at `4bef9ed` (PR #74's merge), green (34/34 floor lines) | 3.36 | 2.50–3.71 (pre, 9 samples, post; 05:28:22–05:29:43Z) | 3.71 |
| F15 | `40458035715` attempt 1, rust job `122035836637`, PR #75 at `e048917`, green (34/34 floor lines) | 4.03 | 2.50–4.03 (pre, 9 samples, post; 05:58:06–05:59:32Z) | 2.50 |
| F16 | `40460725223` attempt 1, rust job `122044419319`, main at `6bb8019` (PR #75's merge), green (34/34 floor lines) | 3.86 | 2.56–3.98 (pre, 9 samples, post; 06:25:41–06:27:10Z) | 3.07 |

**F1 seeds it:** the maximum is 4.72 (load1 14.15 at 03:08:14Z), and the
ceiling 1.25 × 14.15/3 = 5.90. **F2 moves the maximum to 5.32** (load1 15.96,
the `floor-pre` line, at the same instant as that run's `gpu-post`: the two
windows meet at one boundary reading), inside 5.90, so **the ceiling is 1.25
× 15.96/3 = 6.65 per CPU** (load1 19.95), at 2 green runs.

**F3 moves the maximum to 8.01** (load1 24.04 at 07:17:34Z, a sample; 24.02
at 07:17:23Z), counted by the user's ruling of 2026-10-26 on #70's two-key
pass, its cause named: the `bindings` floor step compiled `nbe-preflight`
cold inside the bracket (43 s, against about 4 s on main), because #70
changed `Cargo.lock` and rust-cache missed. So **the ceiling is 1.25 ×
24.04/3 = 10.02 per CPU** (load1 30.05), at 4 green runs. **F4 peaked at
3.52**, under it. Main's rust-cache missed again, as predicted (a
restore-key hit, "full match: false"), and `nbe-preflight` compiled in the
bracket for 21.35 s, but the bracket read 2.44 and 2.24 per CPU during that
compile. F4's peak is the window's end (the 08:44:57Z sample and the post
line, load1 10.57), during the `automation_rules` floor step. The user's
ruling of 2026-10-26 accepted the row as read: it says what the log says,
not what was expected.

**F5–F10 peaked under it:** 3.70 and 3.94 (#71 at `9965d1e` and
`ba68033`), 4.60 (main `ab0304a`), 4.60 and 4.26 (#73 at `5a65e59` and
`7c215c7`), 4.86 (main `27368f2`), at 10 green runs.
**F11–F14 peaked under it too:** 4.12, 4.60 and 4.29 (#74 at `9dfd84b`,
`527c065` and `ae709db`), 3.71 (main `4bef9ed`), at 14 green runs.
**F15 and F16 peaked under it too:** 4.03 (#75 at `e048917`) and 3.98
(main `6bb8019`), at 16 green runs.

**The control-plane bracket envelope** (the user's ruling of 2026-10-26 on
R32, (a); the fourth envelope). From `ab0304a`, `cp-pre`, `cp-sample` and
`cp-post` bracket every test step of the control-plane job (named steps 7
and 20, the 12 test steps between them, `npm test` through the
operator-signals end to end), after a 60-second settle (the user's ruling of
2026-10-26 on CP1). Its rule is the pinned one:
- the maximum per-CPU anywhere in the bracket's window (`cp-pre`, the
  samples, `cp-post`);
- green runs only (a green job in a red run counts: steps count);
- a ceiling of 1.25× that maximum.

A control-plane sighting is read against it (R32's included). Not counted:
`9965d1e`'s window (4.49–10.59), whose readings carried the engine build's
load-average tail (the ruling: the settle, not the count), and #72's
(`dc38d76`), red by design, as #69's was.

| CP row | Run | cp-pre | Whole bracket | cp-post |
|---|---|---|---|---|
| CP1 | `40351769240` attempt 1, control-plane job `121673432156`, PR #71 at `ba68033` (the settle head), green (`npm test` 205/205, package-reconcile 4/4) | 4.52 | 2.33–**4.94** (pre, 21 samples, post; 15:43:56–15:47:19Z) | 4.94 |
| CP2 | `40441808488` attempt 1, control-plane job `121983656950`, main at `ab0304a` (PR #71's merge), green (`npm test` 205/205, package-reconcile 4/4) | **6.77** | 3.23–**6.77** (pre, 20 samples, post; 03:08:05–03:11:26Z) | 4.62 |
| CP3 | `40442304083` attempt 1, control-plane job `121985247728`, PR #73 at `7c215c7`, green (`npm test` 205/205, package-reconcile 4/4) | 3.40 | 2.32–4.98 (pre, 20 samples, post; 03:08:45–03:12:06Z) | 4.98 |
| CP4 | `40448236533` attempt 1, control-plane job `122004538970`, main at `27368f2` (PR #73's merge; the control-plane job green, `npm test` 205/205, package-reconcile 4/4; the run red on dress, R21's sighting 2) | 2.39 | 2.15–5.01 (pre, 20 samples, post; 04:12:32–04:15:53Z) | 4.74 |
| CP5 | `40450166227` attempt 1, control-plane job `122010765314`, PR #74 at `527c065`, green (`npm test` 205/205, package-reconcile 4/4) | 3.14 | 3.01–4.56 (pre, 21 samples, post; 04:33:35–04:37:06Z) | 4.56 |
| CP6 | `40451987570` attempt 1, control-plane job `122016602551`, PR #74 at `ae709db`, green (`npm test` 205/205, package-reconcile 4/4) | 3.70 | 2.48–4.62 (pre, 21 samples, post; 04:53:47–04:57:12Z) | 4.37 |
| CP7 | `40455238385` attempt 1, control-plane job `122026909528`, main at `4bef9ed` (PR #74's merge), green (`npm test` 205/205, package-reconcile 4/4) | 4.72 | 3.46–4.72 (pre, 21 samples, post; 05:28:25–05:31:49Z) | 4.69 |
| CP8 | `40458035715` attempt 1, control-plane job `122035836675`, PR #75 at `e048917`, green (`npm test` 205/205, package-reconcile 4/4) | 1.92 | 1.88–5.15 (pre, 21 samples, post; 05:57:45–06:01:14Z) | 4.76 |
| CP9 | `40460725223` attempt 1, control-plane job `122044419247`, main at `6bb8019` (PR #75's merge), green (`npm test` 205/205, package-reconcile 4/4) | 4.50 | 2.76–5.28 (pre, 21 samples, post; 06:25:34–06:29:03Z) | 4.64 |

**CP1 seeds it:** the maximum is 4.94 (load1 14.82, the last sample at
15:47:18Z and `cp-post` at 15:47:19Z, during the operator-signals end to
end: test load), and the ceiling 1.25 × 14.82/3 = 6.18 per CPU. **The
settle's evidence** (the user's ruling of 2026-10-26, on #71's two-key
pass): `cp-pre` read 10.59 per CPU at `9965d1e`, 0.26 s after the engine
build, and 4.52 at `ba68033`, after the settle; the samples then fell to
2.33 (15:45:16Z) before the end-to-end steps raised them.

**CP2 moves the maximum to 6.77**, counted by the user's ruling of
2026-10-26 on #71's bookend (the pinned rule, on F3's precedent), its cause
named: `cp-pre` read load1 20.31 at 03:08:05Z, then 5.83, 5.03 and 4.33 per
CPU over 31 s, the engine build's tail. Main's rust-cache missed for the
control-plane job ("full match: false"; `0132d07`'s control-plane job
failed, R32's sighting 2, and saved nothing), so the engine built for 2m
23s, finishing at 03:07:05Z, and its tail outlasted the 60 s settle (at
`ba68033` the build took 21.70 s). So **the ceiling is 1.25 × 20.31/3 = 8.46
per CPU** (load1 25.39), carried as derived from a cold-build CP2: the next
normal reading shows whether 8.46 was weather, and if it was, the lane
corrects downward per the standing rule. Logged as a row, not gated.
**CP3 (4.98) and CP4 (5.01)**, the next readings, were warm (the engine
built in 14.97 s and 15.55 s, full cache matches; `cp-pre` 3.40 and 2.39),
their peaks at the window's end, test load.

**The ceiling stays 8.46 per CPU** (the user's ruling of 2026-10-26, on
#73's bookend): the pinned number, recorded per the readings; no downward
rule exists. CP3 and CP4 are weather-confirming. **Queued, a proposed
downward rule** (the same ruling; proposed, not in force): after three
consecutive readings under 50% of a ceiling raised by a single
weather-flagged reading, the ceiling re-derives from the highest
non-weather reading.

**CP5–CP7, warm, under 8.46:** 4.56 and 4.62 (#74 at `527c065` and
`ae709db`), 4.72 (main `4bef9ed`), at 7 green runs; none is under the queued
proposal's 50% line (4.23). CP7's peak is `cp-pre` (load1 14.17 at
05:28:25Z), 60 s after a warm 23.04 s engine build; its first readings held
flat (4.72, 4.65, 4.61, 4.67) rather than decaying as CP2's build tail did.

**CP8 and CP9, warm, under 8.46:** 5.15 (#75 at `e048917`, a late sample
during the operator-signals end to end, load1 15.45 at 06:01:07Z) and 5.28
(main `6bb8019`, a mid-window sample, load1 15.83 at 06:27:06Z), the highest
warm readings yet, at 9 green runs; neither is under the queued proposal's
50% line (4.23).

**Merged: PR #49 is `ac99d49`** (the bookend, the standing pattern of §2c),
on the user's word of 2026-10-16 (the CI-ceiling ruling's item 4): a squash at
2026-10-17T02:58:35Z (parent `be7a5d8`), pinned to the reviewed head
`68a8388`. The merge tree `67249a1` is identical to `68a8388`'s. The CI
before the merge, at the head, run `38996601613`, read per test (§2b): rust
511/0/4, 30 of 30 floors; control-plane npm 176/176, every e2e floor held,
the refused start raised at 2008 ms; dress 27/27 with `ok 24`, drops 1,
underruns 196, watchdog trips 0. The ruling commit `68a8388` was handled per
the docs-only precedent: 0 code lines since `70712f3`, CI read at the new
head.

Main's CI at `ac99d49`: run `38996990687` (push), attempt 1, read per test
(§2b). Every job succeeded:
- **rust** (job `117291616981`, runner `GitHub Actions 1000003186`):
  `TOTAL: 511 passed, 0 failed, 4 ignored`, 30 of 30 floor lines at or
  above floor, 0 `test result: FAILED`, and
  `telemetry_tick_wires_the_live_session_counter ... ok`, both in the
  workspace step and in the RTMP floor step.
- **control-plane** (job `117291617130`): `npm test` 176/176; automation
  32/32; effects 2/2. Every e2e floor held: slate-release 4/4, recall 2/2,
  take-duration 3/3, resync-end 3/3, package-reconcile 4/4, resync-request
  3/3; show-stop ran 6, exercised 1, skipped 5; operator-signals ran 4,
  exercised 1, skipped 3, and its refused start raised at 2020 ms.
- **dress rehearsal** (job `117291616878`): 27/27 with `ok 24`.
  DRESS-COUNTS (ci, `ac99d49`): drops 3, underruns 202, fallbackTicks 0,
  watchdogTrips 0, step4RiseMs 1974, step6RiseMs 3024, all inside the
  backstops.
- **R10 did not fire on main.** There is no armed sighting to record. The
  run's samples extend the envelope (the table above).
- **State and bookend accepted** (the user's word with the amended ruling):
  #49 merged as `ac99d49`, with tree identity to the reviewed head; main is
  green per test at run `38996990687`; R10 was silent, and no armed sighting
  is owed.

### ~~Finding R11 — the stream thread's 500 ms teardown wait expired twice, once under the ceiling (recorded 2026-09-25, PR #33)~~ ~~RESOLVED by `bb7d4a2` (2026-09-26, Prompt 11 WU0)~~ ~~**REOPENED 2026-10-09** by post-fix sighting 5 (the user's rule of 2026-10-06)~~ **RESOLVED BY MEASUREMENT 2026-10-11** (the user's ruling (b) on PR #41, merged as `a6fc4d8`)

**Resolution, recorded 2026-09-26 (§2c: the open entry stands below, unedited).**
Both halves of the resolution condition landed together in `bb7d4a2`:
- **The rebound, by event, not wall.** `stop_thread` now awaits the thread's
  exit EVENT — `done` is a `tokio::sync::oneshot` the thread fires as its last
  act — bounded by a deadlock backstop, `STREAM_THREAD_EXIT_BACKSTOP` =
  1100 ms. That plus the transport's 800 ms, waited in sequence, is 1900 ms,
  inside §16.1's 2 s window (a pinned arithmetic test). A slow exit passes; only
  an exit that never comes fails the stop.
- **The capture.** `StreamStats` carries the thread's phase (`spawned →
  openingEncoders → running → exiting → done`). On expiry the `Teardown` error
  and a `tracing::error` read `phase=…, encoder_ready=…, encoder_open_us=…,
  aac_ready=…`, so a sighting names where the thread was.

Guards: `a_slow_exit_inside_the_backstop_is_a_clean_stop` (an 800 ms exit via a
per-session seam passes), `a_hung_exit_is_reported_with_the_thread_phase` (the
capture names `phase=exiting`), and `the_exit_backstop_fits_the_section_16_1_window`.
Falsified at `bb7d4a2`, each restored with `git checkout`:

| Mutation | Signature |
|---|---|
| backstop back to the old 500 ms cap | `a thread that exits inside the backstop is a clean stop, however slowly: Teardown("stream thread did not exit within 500 ms (phase=exiting, encoder_ready=true, encoder_open_us=239059, aac_ready=true)")` |
| capture removed from the error | `the capture names the phase the thread was in, got: E_NETWORK: stream thread did not exit within 1100 ms` |
| the event wait skipped | `the wait must have waited for the exit event (7.642703ms < 800ms)` |

**The first datum the capture produced** is in the first row above: on the first
encoder open in that test process, VideoToolbox took **239 ms**
(`encoder_open_us=239059`); the next open took 38 ms. A cold open alone can
spend half the old 500 ms cap before the thread ever reads `Stop`. That is
evidence for hypothesis 2 (a cold VideoToolbox start), not proof of what the two
sightings were. The backstop does not depend on which hypothesis is true, which
is the point of rebounding by event.

**Found while writing the guards:** `set_force_close_error` is process-global,
so `close_error_seam_fails_loudly_with_the_network_token`'s armed seam made the
new tests' concurrent `stop_and_close` return the injected `Teardown`
("stream teardown failed (injected)"). The three stop tests now serialize on a
module mutex. This was a latent in-process contention of exactly the shape
hypothesis 1 names. It is not shown to be what either sighting was: both
sightings read "did not exit within 500 ms", never "injected".

**Still watched.** A flake that goes quiet is not thereby explained. The soak
row stays, and a sighting now carries its capture.

*Reopen condition amended 2026-10-03 (the user's word), on post-fix sighting
2, which kept its capture: `phase=exiting` at load 37.5. The amended
condition and the sighting are in the register row (`docs/soak-protocol.md`
§5).*

*Tripwire sharpened 2026-10-06 (the user's word), on post-fix sightings 3 and
4: both `phase=exiting`, both under the ceiling (loads 2.15–2.40), found by
PR #37's two-key pass. R11 stays resolved, and a third such sighting reopens
it. Queued with them: measure the exit path's true duration, the run loop's
return and the encoders' drop (VideoToolbox's invalidation is the suspect).
The sightings, their captures and the queue line are in the register row.*

*Reopened 2026-10-09, by the rule above, on post-fix sighting 5. The
package-reconcile PR's gate hit it in a full workspace run at `3e1aa2c`:
`phase=exiting`, `encoder_open_us=324828`, the one-minute load 2.34–2.79
throughout, on AC with no discharge in the window. That is the third
`exiting`-phase sighting under the ceiling. `stream.rs` is unchanged since
`592444a`. The re-run was 508/0/3, and CI at the same head was 508/0/3. The
queued measurement of the exit path is R11's open work, put to the user and
not started. The capture is in the register row.*

### R11's exit-path measurement (2026-10-10, branch `r11-exit-path`; the user's word of 2026-10-10)

A measurement work order. The verdict rule was fixed before any numbers
existed:
- **BOUNDED:** every run completes, and the slowest are a bounded tail (max
  within about 2× p99). The backstop becomes max(max × 2, p99 + 1 s).
- **HEAVY-TAILED:** report the tail and the phase carrying it, and do not
  widen the backstop.
- **UNBOUNDED:** a run hangs. Stop and report.

**The sightings, from the register** (`docs/soak-protocol.md` §5, the R11 row).

| # | When and where | Test | Phase | Load (1m) | `encoder_open_us` | Counted |
|---|---|---|---|---|---|---|
| 1 | 2026-09-27, the executor's machine during WU8 | `a_slow_exit_inside_the_backstop_is_a_clean_stop` | unknown (the message was lost) | 4.90, a minute later | — | unconfirmed |
| 2 | 2026-10-03, PR #35's two-key pass, at `592444a` | the same | `exiting` | 37.5 | 539120 | over the ceiling |
| 3 | 2026-10-05, PR #37's pass, a full workspace run at `9473da3` | the same | `exiting` | 2.40 → 2.32 | 286487 | under, the first |
| 4 | 2026-10-05, run 19 of 20 of the test alone | the same | `exiting` | 2.15 | 340090 | under, the second |
| 5 | 2026-10-09, PR #40's gate, a full workspace run at `3e1aa2c` | the same | `exiting` | 2.34–2.79 | 324828 | under, the third: R11 reopens |
| 6 | 2026-10-10, this measurement's loop, iteration 9 of 100, the test alone | the same | `exiting` | 2.19 | 345845 | under, the first with the teardown timeline |

Sighting 5's raw line (`ws-head.raw.log`):
`Teardown("stream thread did not exit within 1100 ms (phase=exiting, encoder_ready=true, encoder_open_us=324828, aac_ready=true)")`
at `stream.rs:1029`.

**The exit path, read at `01e6de2`.** From the stop request to the join:
1. **`stop_thread` starts the clock.** It drops the frame sender, sends `Stop`
   on the control channel, and starts the backstop:
   `tokio::time::timeout(STREAM_THREAD_EXIT_BACKSTOP, done_rx)`.
2. **The thread may still be opening its encoders.** `StreamThread::open` is
   synchronous: the H.264 `EncodeSession::open` (VideoToolbox), then the AAC
   `AacEncoder::with_bitrate` (AudioToolbox). The thread reads its control
   channel only inside `run`, so a stop that lands during the open waits for
   it. **This phase can block, on VideoToolbox's session creation**, and the
   backstop's clock is already running.
3. **`run` returns** on its next iteration: `control_rx.try_recv` sees `Stop`,
   or `frame_rx.recv_timeout(10 ms)` sees the dropped sender. At most one
   in-progress encode and one AAC drain stand in the way.
4. **`Exiting`.** The test seam's exit delay runs here; it is zero in
   production.
5. **The encoder drop:** `EncodeSession::drop` calls
   `VTCompressionSessionInvalidate`. **This can block, on VideoToolbox.**
6. **The AAC drop:** `AudioConverterDispose`.
7. **`Done`**, and the `done` oneshot fires.
8. **The waiter wakes and joins** the thread, whose body is finished.
9. **Then, outside the backstop, the transport.**
   `PublisherHandle::shutdown_and_wait(800 ms)` is the socket teardown, after
   the thread.

There is no file finalize on this path (that is the record take's).

**Where 1100 ms lives.** `STREAM_THREAD_EXIT_BACKSTOP` in `record/stream.rs`,
chosen in `bb7d4a2` (Prompt 11 WU0). It is §16.1's 2 s `show.stop` window,
less the transport's 800 ms, less 100 ms of acknowledgement headroom (its doc
comment). `the_exit_backstop_fits_the_section_16_1_window` pins the sum.

**The instrumentation (permanent).** `StreamStats::teardown`, a
`TeardownTimeline` of eleven marks in µs since the session opened:
- openStart, openEnd, stopRequested, runReturned;
- exitDelayEnd, encoderDropped, aacDropped;
- doneSent, doneReceived, joined, transportClosed.

The thread sets its own marks, and the waiter (`stop_thread`,
`stop_and_close`) sets the rest. Every stop logs one `info` line ("stream
teardown") with the timeline. The backstop's capture now carries it too, so a
sighting names the mark it stopped at.

Production behaviour is unchanged:
- no sleeps, locks or awaits, and the path's order is the same;
- the two codec drops are spelled out, in the order the fields dropped them.

Why permanent: a sighting in a real show then carries the whole teardown.
The cost is one `Instant::now()` and one atomic store per mark, eleven per
session, plus one log line per stop. Tests and measurements read it through
`StreamStats::teardown`.

**The measurement.** `crates/nbe-engine/tests/r11_exit_path.rs`, an
`#[ignore]`d measurement in `zerocopy_bench`'s convention.
- **One teardown per process,** so VideoToolbox opens cold, as it does in
  each lib-test process.
- **The stream tests' configuration:** `rtmp://127.0.0.1:9/live/k` (nothing
  listens), `select_stream(true)`, 640×360 at 30 fps, and production's exit
  path (no exit delay).
- **`immediate`:** stop straight after open, as the lib suite's stream tests
  do.
- **`running`:** stop once the encoders are open and the thread has run
  100 ms, as a show's `stream.stop` does.

The conditions:
- Two guarded runs on 2026-10-10: 20:33–20:49 and 20:50–21:01.
- AC, 45 W adapter, Low Power Mode on, no sleep or battery event. Power-limited:
  discharge on AC in 47 of 194 samples (to −958 mA) and 12 of 130 (to −267 mA).
- Every run launched under load 2.5; the load read inside the runs was
  1.93–2.98. None voided.
- n = 100 per configuration and build.
- A driver bug (an output path expanded before its variables were set) lost
  three of the first run's four batches; they were re-run in the second.

**`immediate`, instrumented (n = 100, ms, nearest-rank):**

| phase | median | p95 | p99 | max |
|---|---:|---:|---:|---:|
| encoder open | 251.89 | 266.33 | 292.25 | 293.01 |
| open remaining when the stop landed | 251.98 | 266.40 | 292.31 | 293.10 |
| loop reaction after the open | 0.00 | 0.00 | 0.00 | 0.00 |
| encoder drop (VT invalidation) | 3.49 | 3.84 | 5.61 | 6.02 |
| AAC drop | 0.14 | 0.20 | 0.22 | 0.24 |
| `done` to the waiter waking | 0.05 | 0.08 | 0.10 | 0.11 |
| join | 0.04 | 0.06 | 0.08 | 0.09 |
| **stop → `done` (what the backstop bounds)** | **255.72** | **270.77** | **296.07** | **296.96** |
| transport, after the join | 7.60 | 7.70 | 7.76 | 7.79 |
| stop → closed | 262.93 | 278.52 | 302.28 | 303.16 |

**`running`, instrumented (n = 100, ms):**

| phase | median | p95 | p99 | max |
|---|---:|---:|---:|---:|
| loop reaction | 0.07 | 0.09 | 0.10 | 0.10 |
| encoder drop (VT invalidation) | 4.33 | 4.68 | 4.72 | 6.14 |
| AAC drop | 0.16 | 0.20 | 0.24 | 0.25 |
| `done` to the waiter waking | 0.05 | 0.08 | 0.08 | 0.09 |
| join | 0.05 | 0.08 | 0.11 | 0.11 |
| **stop → `done`** | **4.61** | **4.99** | **5.03** | **6.44** |
| transport | 7.19 | 7.73 | 7.75 | 7.76 |
| stop → closed | 11.66 | 12.65 | 12.74 | 14.26 |

**The five slowest, every phase (ms):**
- **`immediate`, open remaining → encoder drop → AAC → stop→`done` →
  transport:**
  - 293.10 → 3.61 → 0.20 → 296.96 → 6.14
  - 292.31 → 3.57 → 0.12 → 296.07 → 6.19
  - 283.47 → 5.61 → 0.21 → 289.35 → 5.17
  - 268.23 → 5.17 → 0.22 → 273.68 → 6.24
  - 266.40 → 6.02 → 0.24 → 272.74 → 7.79
- **`running`, loop reaction → encoder drop → AAC → stop→`done` → transport:**
  - 0.04 → 6.14 → 0.22 → 6.44 → 7.71
  - 0.09 → 4.68 → 0.18 → 5.00 → 6.15
  - 0.08 → 4.71 → 0.15 → 4.99 → 7.67
  - 0.08 → 4.72 → 0.14 → 5.03 → 7.70
  - 0.07 → 4.70 → 0.17 → 4.99 → 6.86

**The instrumentation does not perturb.** `stop_and_close` was timed from
outside, against the uninstrumented `stream.rs` (n = 100 each, ms):

| configuration, build | median | p95 | p99 | max | not ok |
|---|---:|---:|---:|---:|---:|
| `immediate`, uninstrumented | 263.28 | 270.15 | 277.09 | 341.52 | 0 |
| `immediate`, instrumented | 262.93 | 278.52 | 302.28 | 303.16 | 0 |
| `running`, uninstrumented | 12.23 | 12.79 | 12.85 | 12.86 | 0 |
| `running`, instrumented | 11.68 | 12.67 | 12.76 | 14.28 | 0 |

The medians are within 0.6 ms. The tails cross both ways (the uninstrumented
max is the highest). `immediate`'s spread is the encoder open's own
variance, and the marks cost microseconds. The stream suites were green on
both builds: the lib's `record::stream` 12, `prompt10_rtmp` 20 (1 ignored),
`prompt10_stream_cmds` 21, `prompt10_telemetry` 5, `stream_url_precedence`
15.

**`a_slow_exit_inside_the_backstop_is_a_clean_stop` ×100** (instrumented, the
test alone per process): **99 pass, 1 fail**. The failure is sighting 6, at
load 2.19:

`Teardown("stream thread did not exit within 1100 ms (phase=exiting, encoder_ready=true, encoder_open_us=345845, aac_ready=true, teardown: openStart=+0.1ms openEnd=+356.9ms stopRequested=+0.0ms runReturned=+356.9ms)")`

The stop landed before the open began. The thread saw it 356.9 ms later,
then began the test's 800 ms exit delay, so it would have exited about
1157 ms after the stop, past the 1100 ms backstop.

**The sightings, against the measured shape.** The exit path proper (the
thread sees `Stop` → `done`) is a few milliseconds, at most 6.44 here. Every
sighting is the test's own arithmetic:
- the test stops straight after open, so its stop lands during the encoder
  open;
- the backstop's clock includes the rest of that open;
- the test then adds 800 ms;
- so the test's budget for (the open's remainder plus the exit) is 300 ms.

Every counted sighting's open was 286–540 ms, at or past that budget once
spawn and scheduling are added. Cold opens measured standalone here run
252 ms median and 303 ms max. In the lib binary, and under load, they ran
longer (sighting 2's 539 ms at load 37.5). Hypothesis 2 of the filing (a
cold VideoToolbox start) is the mechanism, now measured. It is the open, not
the invalidation: the invalidation is the encoder drop, 6.1 ms at most.

**The verdict: BOUNDED, and blocked by SPEC §16.1.** The basis is stop →
`done` over every instrumented run in the stream tests' configurations
(n = 200): median 6.44 ms, p95 262.59, p99 289.35, max 296.96. Every run
completed, and max/p99 = 1.03, within 2×. The rule's backstop is then
max(296.96 × 2, 289.35 + 1000) = **1289.35 ms**.
- **The collision.** 1289 ms plus the transport's 800 ms is 2089 ms, over
  §16.1's 2 s `show.stop` window, which
  `the_exit_backstop_fits_the_section_16_1_window` pins.
- **Per configuration:** `immediate` alone gives 1296.07 ms, and `running`
  alone (production's `stream.stop`) gives ~~1005.02 ms~~ 1005.025 ms (the raw
  p99 is 5025 µs; §2c, 2026-10-12, the user's word on PR #41's fresh two-key pass), which fits.
- ~~**So nothing moved.** The constant, the test and R11's status are
  unchanged, put to the user rather than chosen here:~~
  - the formula over both configurations, which needs §16.1's budget changed
    (a);
  - or over production's configuration, 1005 ms, with the slow-exit test
    stopping a running thread so its 800 ms delay measures the exit and not
    the open (b).

  ~~R11 stays reopened until that word.~~ *(§2c, 2026-10-11: the user ruled
  (b); R11's closure is below.)*

**Guard, a hang still fails the test.** With the encoder drop made never to
complete, the slow-exit test fails at the current backstop:
`… teardown: openStart=+0.1ms openEnd=+270.4ms stopRequested=+0.0ms runReturned=+270.4ms exitDelayEnd=+1076.0ms`.
The timeline ends at `exitDelayEnd`, so it names the hung phase, the encoder
drop. Restored from the copy (identical), the stream lib tests pass 12/12.

**R11 closes: RESOLVED BY MEASUREMENT** (the user's words of 2026-10-11,
ruling (b) on PR #41). ~~The closure line, with PR #41's merge SHA, is added
after the user's merge.~~ **Closed by `a6fc4d8`**, the merge of PR #41
(recorded 2026-10-12 in the next PR, per §2c's post-merge pattern). Main's
push run `38323483547` at `a6fc4d8`, read per test (§2b):
- **rust:** `TOTAL: 511 passed, 0 failed, 4 ignored`, with all 30 floors.
- **control-plane:** npm `passed=139 failed=0`, and the package-reconcile
  end-to-end 4/4.
- **dress rehearsal:** 27/27, `ok 24` for step 10, with `DRESS-COUNTS` run
  `38323483547`, sha `a6fc4d8…`, within the backstops.

- **The mechanism.**
  - The backstop's clock starts at the stop request, but the thread reads
    `Stop` only inside `run`.
  - So a stop that lands during a cold VideoToolbox open (286–540 ms across
    sightings 1–6) spends the budget before the exit begins.
  - The old test's own 800 ms delay left it 300 ms.
  - Hypothesis 2 of the filing (a cold VideoToolbox start) is confirmed.
  - The invalidation is not the cost: the encoder drop was at most 6.14 ms
    over 200 runs.
- **The ruling, and why.** (a) is rejected:
  - §16.1's 2 s `show.stop` window is an operator-facing guarantee;
  - the 800 ms that would break it is the test's own injected delay, not a
    production cost;
  - loosening the spec to fit a test artifact is backwards.

  The measurement showed that the quantity the timeout spans (the open's
  remainder plus the exit) is not the one the test names (a slow exit). (b)
  aligns all three: the backstop, the test and the name.
- **The backstop: 1005 ms.** The formula is max(max × 2, p99 + 1 s) over
  production's running configuration: ~~max(6.44 × 2, 5.03 + 1000) =
  max(12.88, 1005.03)~~ max(6.44 × 2, 5.025 + 1000) = max(12.88, 1005.025), so
  `STREAM_THREAD_EXIT_BACKSTOP` = 1005 ms. *(§2c, 2026-10-12, the user's word on PR #41's fresh two-key pass: the raw p99 is 5025 µs,
  `scratchpad/r11/inst-running.jsonl`, stop → `doneReceived`. It read 1005.03
  here and 1005.02 above, two roundings of 1005.025.)*
- **The margin analysis:**
  - **A stop during the open.** ~~The worst observed stop → `done` for a stop
    during the open is about 547 ms: the sightings' opens of 286–540 ms plus
    at most 6.5 ms of exit.~~ The worst observed stop → `done` for a stop
    during the open is about 557 ms:
    - the sightings' VideoToolbox opens ran 286–540 ms;
    - the whole open adds about 11 ms to the VideoToolbox open, the AAC
      open (median 10.99 ms, range 10.73–21.92 ms over the 200 measured runs:
      `openEnd − openStart − encoderOpenUs`; sighting 6, 10.95 ms);
    - the exit is at most 6.44 ms.

    The standalone `immediate` max was 296.96 ms. ~~1005 ms covers 547 ms
    about 1.8×.~~ 1005 ms covers about 557 ms about 1.8× (1.80×), on the
    reference machine. *(§2c, 2026-10-12, the user's word on PR #41's fresh two-key pass.)*
  - **The restructured slow-exit test** needs 800 ms plus the exit (at most
    6.44 ms), leaving 199 ms, about 31× the measured exit.
  - ~~**Slower machines.** CI runs about 1.7× the normative rise times. At 1.7×
    the exit is about 11 ms, and sighting 2's 539 ms open (at load 37.5)
    becomes about 916 ms, so a stop during it lands at about 927 ms, still
    inside 1005.~~ *(§2c, 2026-10-12, the user's word on PR #41's fresh two-key pass: the 1.7× had no source in the tree, and the
    tree's own data contradict it. No extrapolation replaces it.)*
  - **The tree's CI-vs-normative rise factor, recorded as context and not
    applied:** at least 1.86×. The normative rises are 974 and 1034 ms (the
    R12/R13 baseline, above), and CI's healthy rises are 1971–2980 ms at step 4
    and 1927–2009 ms at step 6 (the R13 record, above). It is not applied to
    the open: sighting 2 was itself measured at load 37.5, so scaling it again
    would double-count.
  - **CI** never opens VideoToolbox (it has no hardware encoder), so the
    cold-open path does not arise there.
  - **Why not 1100.** §16.1's ceiling allows up to 1100 ms (2000 − 800
    transport − 100 ack). 1005 is kept: it is the ruling's formula, and it
    covers ~~the observed and the 1.7×-scaled worst cases~~ the worst observed
    case about 1.8× (§2c, 2026-10-12, the user's word on PR #41's fresh two-key pass). The 95 ms more would
    only cover opens past about a second, which the backstop exists to
    report.
  - **The sum.** With the transport's 800 ms it is 1805 ms, leaving 195 ms of
    acknowledgement headroom inside §16.1's 2 s (100 ms before).
    `the_exit_backstop_fits_the_section_16_1_window` pins both the value
    (1005 ms) and the sum (1805 ms). §16.1's text is untouched.
    *(Cross-reference, 2026-10-14: since the (C) fix, SPEC v0.4.8 row 6, this
    sum is `show.stop`'s worst case. The control plane sends `show.stop`
    alone, and the engine's quiesce arm runs this stream close beside the
    record's 1500 ms finish, so the stop acknowledges within max(1500, 1005 +
    800) = 1805 ms. Before the fix, a skipped `stream.stop` never reached this
    path. The measured stop→ack is in queue position 2's fix record.)*
- **The test, restructured.** `a_slow_exit_inside_the_backstop_is_a_clean_stop`
  now waits until the thread is `Running` (its encoders open) before it
  stops, so its 800 ms delay measures the exit, as the name says. Its
  assertions stand. The stop-during-open case is now its own test,
  `a_stop_during_the_encoder_open_waits_for_the_open_inside_the_backstop`.
  The timeline proves the stop landed during the open
  (`openEnd > stopRequested`) and that the exit waited for it
  (`doneReceived ≥ openEnd`), inside the backstop.
- **One other test changed, of necessity.**
  `a_hung_exit_is_reported_with_the_thread_phase` asserts the operator-facing
  message, `did not exit within 1100 ms`, which now reads 1005 ms. Only that
  literal changed, with a §2c note.
- **§2c, struck and not erased:**
  - the constant's doc comment keeps its old sizing text struck: ~~"Sized to
    §16.1's 2 s `show.stop` window … 1900 ms ≤ 2000 ms … 100 ms of ack-pump
    headroom"~~, with its provenance (`bb7d4a2`, Prompt 11 WU0);
  - the slow-exit test's doc keeps ~~"The test stopped straight after
    open."~~ with why it failed;
  - the pin's doc notes that it was 1100 ms and 1900 ms.
- **Guards.**
  - **The hang guard, against the restructured test** (2026-10-11
    02:00, on AC, load 2.39). With the encoder drop made to never complete,
    the slow-exit test fails at the new backstop:
    `Teardown("stream thread did not exit within 1005 ms (phase=exiting, encoder_ready=true, encoder_open_us=243118, aac_ready=true, teardown: openStart=-255.0ms openEnd=-0.8ms stopRequested=+0.0ms runReturned=+0.1ms exitDelayEnd=+809.0ms)")`.
    The stop landed on a running thread (`openEnd` before `stopRequested`),
    as restructured. The timeline ends at `exitDelayEnd`, so it names the
    hung phase, the encoder drop. Restored from the copy (identical) and
    rebuilt, it is green: `test result: ok. 1 passed`.
  - **The restructured slow-exit test, run 100 times** (the test alone
    per process, each launched under load 2.5; loads 1.89–2.47): 100 pass,
    0 fail. That was the expected count: 800 ms plus about 6 ms is well under
    1005.
  - **The stop-during-open test, run 100 times** (loads 1.76–2.36):
    100 pass, 0 fail. Each run's timeline checked that the stop landed during
    the open and that the exit waited for it.
  - The stream suites ran in full, green; see the gate.
- **The instrumentation stays permanent.** Sighting 6 is its justification:
  `phase=exiting, … encoder_open_us=345845, … teardown: openStart=+0.1ms
  openEnd=+356.9ms stopRequested=+0.0ms runReturned=+356.9ms`. It is the
  first capture that showed where the time went: the stop landed before the
  open began, and the thread saw it 356.9 ms later. Five earlier sightings
  had only the phase.
- **Sightings 1–6, against the mechanism.** Every one is the old test's
  shape:
  - the stop landed during the encoder open;
  - the open's remainder (286–540 ms cold; sighting 2's 539 ms at load 37.5)
    plus the injected 800 ms crossed 1100 ms, with the thread in `exiting`;
  - sighting 1's message was lost, and it is consistent with the rest.

  None was a slow exit. With the test restructured, its budget covers the
  exit alone.

### Finding R11 (as filed 2026-09-25, kept per §2c) — the stream thread's 500 ms teardown wait expired twice, once under the ceiling

*Numbered after R10; no R11 existed in the tree. This entry supersedes the
"load flake" note filed under § 11's "SPEC v0.4.6" entry in `74dcc24`, which is
kept there, struck, per §2c.*

**Signature.** `record::stream::tests::close_error_seam_fails_loudly_with_the_network_token`
fails with `released seam must close: Teardown("stream thread did not exit within
500 ms")`. The test arms the close-error seam, gets its injected `Teardown`, then
releases the seam and calls `stop_and_close` again. That second call must see the
stream thread exit within `STREAM_THREAD_STOP_TIMEOUT` (500 ms).

**Sightings.**
1. **PR #33's two-key pass, at load 27.3.** Recorded in `74dcc24` with load named
   as the cause.
2. **2026-09-25, at load 2.59 — under the 3.0 quiescence ceiling** — in a full
   `cargo test --workspace` at `263b074`, where the `nbe-engine` lib suite runs
   this test beside 49 others in parallel (`49 passed; 1 failed`). It did not
   reproduce: 0 failures in 10 lib-suite runs at load 5.6–6.3, and 20/20 passes
   run alone (load 4.64 at the end of those).

**The correction.** ~~"at load 27.3 that plausibly took longer … The class is
R9's: a wall-clock bound that measures the machine, not the code."~~ (`74dcc24`)
**Refuted by sighting 2.** Load does not predict this failure: it expired once
at 2.59 and never in thirty tries at load 4.6–6.3. What the two sightings share is
only the shape. The bound is 500 ms of wall clock against a thread's exit, the
failure is the wait expiring, and **what the system and the thread were doing
inside that window is unknown in both.**

**The class correction.** This is **not** R9's class as recorded. R9's bound
*tracked* load: it passed at 2.58 and failed at ~30 on the same binary, which is
what made it a machine measurement. A bound that expires under the ceiling with
its cause unknown is an **open flake**, and this entry files it as one.

**Hypotheses — both UNTESTED, neither evidenced:**
- **In-process contention from the parallel lib suite.** Sighting 2 ran beside
  49 tests in one process; the 30 non-sightings ran either alone or in quieter
  suite runs. The system load average does not measure this.
- **A cold VideoToolbox start.** The stream thread opens its H.264 and AAC
  encoders eagerly, first thing (`run_stream_thread` → `StreamThread::open`),
  before it can read the `Stop` control message. It invalidates the
  VideoToolbox session on its way out. The test stops the thread immediately
  after `StreamSession::open`, so the 500 ms window contains an encoder open, an
  AAC open and an invalidation. A first-in-process session opening slowly would
  spend the window before the thread ever sees `Stop`. This is read from the
  code, not observed.

**THE RESOLUTION CONDITION, written before the next run rather than after it.**
- **The next sighting must capture the window's contents.** At the moment the
  wait expires, record where the thread was: `StreamStats`' `encoder_ready`,
  `encoder_open_us` and `aac_ready` say whether it was still opening encoders,
  and a thread stack says the rest. That is how R7 was resolved: its payload
  dump, added two rounds earlier for exactly this, named the mechanism on the
  fifth sighting. **A third sighting without that capture adds a count and
  settles nothing.**
- **The standing fix direction: rebound by event, not wall** — R9's own
  resolution shape ("rebound by work, not wall"). The tree is closer than the
  phrase suggests: `stop_thread` already waits on the exit *event* (it polls the
  thread's `done` channel). What fails is the 500 ms wall *cap* on that wait,
  after which it detaches the thread unjoined and reports failure. A deadlock and
  a slow exit are told apart by whether the exit *ever* happens, so the wait
  should hang off the event with a generous wall backstop sized for deadlock
  detection. §16.1's 2 s `show.stop` window is the ceiling that backstop has to
  respect, and that is the design question to settle. Queued under § 10's "Still
  owed"; not built here.

**The CI consequence, stated plainly.** This test runs on every `rust` job,
since the unit test needs no encoder to open a session. It can red any of them
with an attempt-1 failure that goes green on re-run. That is rerun culture
arriving through a wall clock. This entry is what stands between a green re-run
and the habit of pressing it: a re-run is not evidence the flake is gone, and a
sighting that is re-run without being recorded here is a lost datum.

**Watch list:** `docs/soak-protocol.md` §5, in R7's and R10's shape.

### Finding R12 — the dress rehearsal's RI-1 gate is red on every CI run, and nobody read it (recorded 2026-10-03, PR #35)

*Numbered after R11, the highest filed. The register row is in
`docs/soak-protocol.md` §5.*

**Signature.** `not ok 12 - [RI-1] gate: no drops, no underruns, no fallback,
and the profile is real` (`packages/control-plane/src/dress-rehearsal.test.ts`,
line 849 onward), in the `dress rehearsal` CI job. The gate's asserts run in
order and stop at the first failure. So each run shows one conjunct only: either
`zero-drop across the whole show` (`N !== 0`) or `no audio underruns across the
show` (`N !== 0`). The other values come from each run's uploaded
`telemetry.jsonl` (`dress-rehearsal-artifacts`), read at the last tick before
`show.stop`. Those readings equal what test 12 asserted.

**Sightings.** ~~Four of the four~~ ~~Five of the five~~ Seven of the seven runs
sampled under the old gate, read per test. The fifth was added 2026-10-03, at
PR #35's ride-along head; the sixth and seventh were added 2026-10-04, at PR
#35's final head and its merge. The last row is the first honest run under the
new gate:

| Run | Commit | Context | Failed on (asserted) | Drops | Underruns | Fallback ticks | Watchdog trips | Other dress failures |
|---|---|---|---|---|---|---|---|---|
| `36214380771` | `619846c` | main, v0.4.6 merge, 2026-09-26 | zero-drop, `2 !== 0` | 2 | 211 | 16 | (no counter before v0.4.7) | — |
| `36567491896` | `de90bbe` | PR #34 head, 2026-09-29 | underruns, `179 !== 0` | 0 | 179 | 0 | 0 | step 4: the clip rise came late |
| `36658852056` | `592444a` | main, PR #34 merge, 2026-09-30 | underruns, `160 !== 0` | 0 | 160 | 0 | 0 | step 6: the sfx rise came late |
| `37171465949` | `8265292` | PR #35 head, 2026-10-04 UTC | zero-drop, `1 !== 0` | 1 | 164 | 1 | 1 | — |
| `37177561864` | `66f9f94` | PR #35 head after the ride-along, 2026-10-04 UTC | zero-drop, `3 !== 0` | 3 | 174 | 2 | 1 | step 4: the clip rise came late (R13) |
| `37178415083` | `f1a8279` | PR #35's final head, 2026-10-04 UTC | zero-drop, `2 !== 0` | 2 | 152 | 1 | 1 | — |
| `37179591329` | `d7b96f7` | main, PR #35's merge, 2026-10-04 UTC | underruns, `170 !== 0` | 0 | 170 | 0 | 0 | step 6: the sfx rise came late (R13) |
| `37181092309` | `83bfb60` | PR #36, **the new gate: counts logged, not failed** | — (27 of 27) | 0 | 176 | 0 | 0 | — (step 6's rise logged at 4971 ms) |
| local | `8265292` | normative machine, quiescent (2.46 → 2.94) | — (16 of 16) | 0 | 0 | 0 | 0 | — |

Sighting 5's whole-run maxima are drops 4 and underruns 267. Its counts are
inside every backstop, but **3 drops at the gate is above the earlier 0–2
pattern**: the trend the work order will log starts being watched here.

On CI, underruns are non-zero from the first tick (frame 0) of every run. Over a
whole run they reach 249–315. The job's own comment in `ci.yml` measured 83 and
120, plus a dropped frame, on 2026-09-11. When the slate went to air on CI, it
was the watchdog (§10.3). The first fallback tick coincides with the first drop
tick, so it is the same late frames, not an independent failure.

**Class.** A hardware-bound gate asserting on non-reference hardware. This is
R9's family (a wall-clock threshold measuring the machine), at CI scale. The
gate's zero-drop and zero-underrun conjuncts are claims about the reference
machine (§0.3), and `ci.yml` already says so. The runner is 3 arm64 cores on a
shared VM.

**The process failure.** The leg is advisory in two layers:
- **Job level.** `dress-rehearsal:` … `continue-on-error: true`
  (`.github/workflows/ci.yml`, line 1027 onward).
- **Inside the step.** A band fails the step only if `passed < 9` or
  `failed > 3`. A run with 15 passing and 1 failing reads green before
  `continue-on-error` is even consulted.

The job's comment names "a gate red on purpose — it teaches reviewers to ignore
red" as a failure mode it rejected. Test 12 has been red on purpose in every
run sampled since. The band also absorbed failures that were not hardware
claims: steps 4 and 6, whose rises arrived late (now Finding R13). Advisory and unread, it stayed
invisible for weeks. Two merge bookends, v0.4.6's and PR #34's, recorded "all
green" from the job's conclusion over failing tests.

The rule that now forbids that reading is `docs/implementation-standards.md`
§2b, "A job's conclusion is not its tests".

**Disposition, decided 2026-10-03 (the user's word): the corrected split.** The
split as first ordered would have gated "no underruns" and "no fallback" on CI.
It would have been red in four runs of four: underruns are non-zero at every CI
gate, and the slate went to air at two of the four gates. The two-key pass
corrected it, and the user decided the corrected form:

- **On CI, the gate keeps the functional conjuncts.** These are: the profile is
  real; a decode session is held; and the slate engages only through a watchdog
  trip, meaning every fallback tick sits inside a watchdog episode. Any operator
  or render-failure slate fails.
- **The zero conjuncts are normative-only.** Drops = 0, underruns = 0, no slate
  at all: these are asserted on the normative machine only, in the local
  rehearsal and in the soak. The dress job adopts ran/exercised accounting
  (rule 8), so a capability-gated step that prints `# SKIP` is counted as ran
  and not as exercised.
- **The four counts are logged on every CI run, each asserted separately, so no
  conjunct hides another.** Each has a gross-regression backstop about an order
  of magnitude above the sightings:

  | Count | At the gate on CI | Whole run on CI | Backstop |
  |---|---|---|---|
  | Drops | 0–2 | max 3 | ≤ 20 |
  | Underruns | 160–211 | max 315 | ≤ 2000 |
  | Watchdog trips | 0–1 | max 2 | ≤ 10 |

  The underrun count is also logged as a trend. It has risen from 83/120 on
  2026-09-11, while the rehearsal grew a stream step, and a 10× backstop alone
  would hide a 5× regression.
- **The band and `continue-on-error` are removed**, and the job gates on
  `failed == 0`. The leg never runs unread-red again.

**~~Queued (a small work order, not built in PR #35)~~ Implemented in PR #36
(2026-10-04): the dress leg's gate, in `ci.yml` and the rehearsal.** The order as
it was queued:

> Split the RI-1 gate test into one assertion per conjunct. On CI, assert that
> the profile is real, that a decode session is held, and that every fallback
> tick lies inside a watchdog episode. At the gate, on every run, log
> `droppedFramesTotal`, `audioUnderrunsTotal`, the fallback ticks and
> `watchdogTripsTotal`, each against its backstop (drops ≤ 20, underruns ≤ 2000,
> trips ≤ 10), and append the underrun count to a trend record. Assert the zero
> conjuncts only on the normative machine (the local rehearsal and the soak).
> Adopt ran/exercised accounting, so that a `# SKIP` capability gate is not
> counted as exercised. Remove the step's band and the job's
> `continue-on-error`, and gate on `failed == 0`.
>
> *Amended 2026-10-03, carrying Finding R13:* also log step 4's and step 6's
> rise time (take or play to the first audible tick) on every run, with its
> trend. On CI, assert the rise is present within a 10 s backstop, and keep the
> 3000 ms bound for the normative machine only.

**As implemented (PR #36, branch `dress-leg-r12-r13`: `76f092f` the test,
`83bfb60` the CI gate, `2582b2a` the head commit on the counts line).**

- **The gate, one test per conjunct (16 tests become 27).**
  - Three functional conjuncts, on every machine: the profile is real; a decode
    session is held; every fallback tick lies inside a watchdog episode
    (`watchdogTripsTotal > watchdogClearsTotal` on that tick).
  - Three backstops, on every machine: drops ≤ 20, underruns ≤ 2000, watchdog
    trips ≤ 10.
  - One `DRESS-COUNTS` line.
  - Three normative zero conjuncts.
  - R13's two normative 3000 ms rise bounds.
  - The gate reads its ticks once, so every gate test judges the same snapshot.
- **The machine (`whichMachine`).** This is the tree's own `CI` convention,
  made fail-closed:
  - With no CI variables set, it is the normative machine, and the zero claims
    are asserted.
  - With `GITHUB_ACTIONS=true`, it is a runner, and each zero claim is skipped
    with a logged `SKIP normative:` reason.
  - Any other combination fails with "refusing to skip".

  It cannot flip silently. A stray `CI=1` on the normative machine fails
  instead of skipping. The runner always sets `GITHUB_ACTIONS`, and if it did
  not, the zero claims would run there and fail red. Step 11's drop asserts,
  which used to skip on CI without a word, now use the same mechanism. *(§2c,
  2026-10-04: the PR #36 pass's order spoke of "steps 11/13's old silent skips".
  Only step 11 carried the silent skip. Step 13's direct `CI` read chose a
  message, never an assertion: both branches assert `hasMoof`.)*
- **The two hiding places, deleted, quoted from `main` at `d7b96f7`.**
  - The job's flag (`ci.yml`, line 1030):
    ```yaml
        continue-on-error: true
    ```
  - The step's band, comment included (`ci.yml`, lines 1121–1137 at
    `d7b96f7`). The order cited "1127–1136", copying my imprecise citation from
    the PR #35 pass, and `83bfb60`'s message repeats it; the tree's lines are
    these:
    ```sh
              # The band stays, and the reason has CHANGED. It used to tolerate R2,
              # R4 and R5 being red by design. Those are closed — 12/12 three times
              # consecutively on the normative machine. It now tolerates the runner:
              # macos-14 gives 3 arm64 cores, and the gate step asserts
              # droppedFramesTotal == 0 and audioUnderrunsTotal == 0, which are
              # performance claims about REFERENCE hardware. Measured on this runner:
              # 83 and 120 underruns in two runs, and a dropped frame, where the
              # normative machine gives 0. No code change makes a shared CI VM hold a
              # 33 ms audio cadence next to wgpu.
              if [ "${passed:-0}" -lt 9 ]; then
                echo "expected at least 9 passing steps, got ${passed:-0}"
                exit 1
              fi
              if [ "${failed:-99}" -gt 3 ]; then
                echo "expected at most 3 failing steps (the hardware-bound gate assertions), got ${failed:-99}"
                exit 1
              fi
    ```
- **The job now gates on `failed == 0`, with the rule-8 floors.**
  - ran ≥ 27.
  - exercised = ran minus the `# SKIP` lines, ≥ 18. The runner takes 9 skips: 4
    capability skips (stream, record, sync and kill, because it has no hardware
    H.264 encoder) and 5 normative ones.
  - The `DRESS-COUNTS` line is required.

**Falsified** on the normative machine. Each rehearsal started under the
ceiling, and each mutation was restored from a saved copy.

| # | Mutation | Load (before → after) | Signature |
|---|---|---|---|
| baseline | none (normative) | 2.47 → 1.84 | 27 of 27, 0 `SKIP` lines, all counts 0, rises 974 and 1034 ms |
| baseline | `CI=true GITHUB_ACTIONS=true` | 1.84 → 1.89 | 27 of 27, 6 `SKIP normative:` lines (locally step 11 runs and logs its own); the CI step's extracted check exits 0 |
| baseline | `CI=1` alone | 1.89 → 2.30 | 6 failures: `refusing to skip "zero drops across the show" on an unidentified machine (CI="1", GITHUB_ACTIONS="")…` |
| F1a | the engine reports no profile | 2.48 → 2.56 | **27 of 27: the conjunct is blind here** (see below) |
| F1a′ | the tick carries an empty profile | 2.47 → 3.36 | `not ok 12 - [RI-1] gate: the profile is real …`: `saw ""` |
| F1b | decode sessions forced to 0 | 2.41 → 2.04 | `not ok 13 - … a decode session is held …`: `saw 0` |
| F1c | an operator slate at step 7 | 2.04 → 2.17 | `not ok 14 - … every fallback tick lies inside a watchdog episode …`: `(tick index): [15]`, and the normative `the fallback slate must never have gone to air`. ~~Collateral: `not ok 13`, `saw 0` (below)~~ *(§2c, 2026-10-04: not collateral, but a stale binary; see below.)* On a rebuilt engine (the PR #36 pass, load 2.27 → 2.01) only `not ok 14` (`[16]`) and the normative slate test fail, and `decodeSessions` holds at 3 through the slate |
| F2 | drops +25, underruns +2500, trips +11 (CI path) | 2.07 → 2.06 | `not ok 15/16/17`, each by name: `droppedFramesTotal at the gate is 25, over the backstop of 20 …`, `… 2500, over … 2000`, `… 11, over … 10`; the CI check: `FAIL: 3 failing test(s)`, exit 1 |
| F3 | every take's clip source silenced | 1.97 → 2.26 | `not ok 5 - … step 4 …`: `waited 10000 ms for busPeakDbfs.clip to rise`. The run took 64 s against about 55, so nothing hangs |
| F4 | droppedFramesTotal +1, both paths | 2.16 → 1.67 · 1.67 → 1.53 | normative: `not ok 19`, `zero-drop across the whole show / 1 !== 0`. CI path: 27 of 27, `"drops":1` logged, the CI check exits 0 |
| F5 | the runner's capability skips reproduced (forced no encoder and no chain), then one more skip | 1.53 → 2.50 · 2.43 → 2.96 | as the runner: `skips=9 exercised=18`, exit 0. With one more: `skips=10 exercised=17`, `FAIL: expected at least 18 tests exercised …`, exit 1 |

**What the gate still cannot see, recorded rather than hidden.**

- **The profile conjunct is blind to a missing engine profile.** The control
  plane's tick fills a missing engine value with the manifest's declared one
  (`telemetry.ts`: `f?.qualityProfile ?? state.qualityProfile`). So an engine
  that reports no profile still passes (F1a). The old gate's comment, "a stub
  would report the manifest's declared value", named this case, and its assert
  never caught it either.

  **Plainly: the tree's fresh-null fallback goes past the spec's text.** §10.1.1
  allows the requested profile only when the engine report is stale or absent,
  with `engineConnected: false` saying why. The tree's `??` also fills a
  **fresh** report's null, while `engineConnected` reads `true`.

  **Hedged in the test** (the PR #36 pass's adjudication F1, adopted
  2026-10-04). The comment and message now say what the gate proves: a
  non-empty effective profile, on a fresh report. They also say what it does
  not prove: that the engine's probe produced it. The test now asserts
  `engineConnected === true` on the gate tick, which closes the sanctioned
  stale-or-absent path.

  **Queued:** ~~carry the profile's provenance on the tick, engine or manifest.
  That is a wire change, so it is the user's word.~~ the fix gets a spec row
  when it comes. The leading candidate is a no-value emission when a fresh
  report carries no profile, in the shape of v0.4.6 row 3's `streamBufferMs`
  sentinel. The alternative is provenance on the tick, a new field with its wire
  audit trio.
- **An operator slate raised during a watchdog episode** cannot be told from
  the watchdog's own slate in telemetry. The episode check passes it.
- **Steps 5 and 6 still assert zero drops on every machine**: a mix drops no
  frames, and a stab drops nothing. Their windows read 0 in all seven CI runs
  sampled. If one goes red on CI, it is R12's class.
- ~~**F1c's collateral `decodeSessions` 0** at the gate, after an inserted
  slate-and-take, is unexplained. The mutation changed the show's sequence, so
  this is an observation, not a finding.~~
- **F1c's "collateral", explained** (§2c, the PR #36 two-key pass of
  2026-10-04). It was a harness defect, not the engine.
  - The batch rebuilt the debug engine only when it mutated engine source.
  - F1b set `decode_sessions: 0`, built, ran, and restored the source
    **without rebuilding**.
  - F1c changed only the test file, so it ran on F1b's binary:
    `decodeSessions` read 0 from the first tick, before the slate went up.

  On a rebuilt engine, the slate mutation fails only its own tests, and
  `decodeSessions` holds at 3 through the slate. **The executor's own batch had
  the same ordering, and that is how "unexplained" entered this finding.** F1a′
  ran on F1a's binary in the same way, harmlessly, because it forces the profile
  empty regardless.

  **The lesson: rebuild after every restore.** A test-only mutation that follows
  a restore otherwise runs on the previous mutation's binary.

**The first honest CI run under the new gate:** `37181092309` at `83bfb60`,
read per test, 27 of 27 (`ran=27 passed=27 failed=0 skips=9 exercised=18`). It
logged `"sha":"26e0e1d…"`, GitHub's test-merge commit, not the head, and
`2582b2a` makes the line name the head. ~~R12 stays OPEN in the register until the
user's word after the two-key pass.~~ *(§2c: superseded by the execution below.)*

**After the merge (the user's word of 2026-10-04):** R12 becomes *resolved;
watch* once main's merge run reads green per test. The trend rule in its
register row keeps its home there.

**Executed 2026-10-05** (a ride-along on PR #37's branch, after its two-key
pass): main's PR #36 merge run, `37188708420` at `dc312a6`, reads green per
test (§2b). Dress 27 of 27, all rust results `ok.`, control-plane 0 `not ok`.
R12 is *resolved; watch* in the register. Its trend rule stays live there, and
read at `b8af9ff` neither leg fires.

### Finding R13 — the dress rehearsal's audio rises arrive after their 3000 ms wait on CI (recorded 2026-10-03, PR #35)

*Numbered after R12. The register row is in `docs/soak-protocol.md` §5.*

**Signature.** Step 4 (`not ok 5 - [RI-1] step 4: a take with audio follow
raises the clip bus on the wire`) or step 6 (`not ok 7 - [RI-1] step 6: a
soundboard stab raises the sfx bus and drops nothing`) fails with `waited 3000
ms for busPeakDbfs.<bus> to rise`. The wait is `RISE_MS + TICK_MS`.

**Sightings.** These are the ~~three~~ four the band absorbed, now read per
test, plus the first under the new gate. Rise times are from each run's
artifacts, or from the `DRESS-COUNTS` line, measured from the take or the play
to the first audible tick:

| Run | Commit | Step | Rise (ms) | At the expiry |
|---|---|---|---|---|
| `36567491896` | `de90bbe` | 4 (clip) | 2981 | the risen value (−20.8 dBFS) in the very tick the wait gave up on |
| `36658852056` | `592444a` | 6 (sfx) | ≈3986 | — |
| `37177561864` | `66f9f94` | 4 (clip) | 3967 | the clip at −120 dBFS. The take was at 11044 ms and the clip became audible at 15011 ms, about 4 s later |
| `37179591329` | `d7b96f7` | 6 (sfx) | ≈3986 | main, PR #35's merge |
| `37181092309` | `83bfb60` | 6 (sfx) | 4971 | **the new gate:** logged inside the 10 s backstop, green |

The runs that passed show the healthy pattern on CI: step 4 at 1971–2980 ms and
step 6 at about 1927–2009 ms. Rises land in steps of about 1 s, set by the 1 Hz
telemetry tick and the 1 s meter window (`audio_driver.rs`). A rise one step
late crosses the 3000 ms wait.

**Class.** R9's: a wall bound measuring the runner. **The behaviour is
correct.** The rise comes in every run, every time. It is late on a loaded
runner, and the normative machine is 16 of 16 (`8265292`). The recall
end-to-end, which uses the same witness in a gating job, already waits 10 s
(`302c0e3`).

**Disposition (the user's word of 2026-10-03): it rides R12's work order.**
- **On CI:** the work order's log-and-backstop list gains each step's rise time,
  logged per run with its trend. The rise is asserted present within a 10 s
  backstop, sized from the sightings: 2.5× the worst (3986 ms), ten telemetry
  ticks, and the same window `recall.e2e.ts` uses for the same witness.
- **On the normative machine:** the 3000 ms bound stays, asserted in the local
  rehearsal and the soak.

**Implemented in PR #36** with R12's work order (`76f092f`, `83bfb60`):
- On every machine, steps 4 and 6 wait up to the 10 s backstop and record their
  rise time on the `DRESS-COUNTS` line.
- `[R13] step 4 (normative)` and `[R13] step 6 (normative)` assert the 3000 ms
  bound through the same machine mechanism.
- F3 above suppresses the rise: the 10 s backstop fails step 4 by name, and the
  rehearsal does not hang.

**What the rise-time measure cannot see** (the PR #36 pass, adopted
2026-10-04):
- **The poll quantum.** The wait polls every 50 ms, so a rise reads up to about
  50 ms late.
- **Tick-plus-window resolution.** A bus level reaches the control plane once
  per 1 s telemetry tick, from a 1 s meter window. A rise time is therefore
  "ticks until detection": it lands in steps of about 1 s, and two rises a few
  hundred ms apart can read the same.
- **The 10 s backstop logs, but passes, a rise between 3 and 10 s.** On CI only
  the log line sees that span. The 3000 ms bound fails it, but only on the
  normative machine.

~~R13 stays OPEN in the register until the user's word after the two-key pass.~~
*(§2c: superseded by the execution below.)*
**After the merge (the user's word of 2026-10-04):** R13 closes outright once
main's merge run reads green per test, because its rises are logged on every
run.

**Executed 2026-10-05** (the same ride-along): `37188708420` at `dc312a6`
reads green per test, with step 4 / step 6 rises of 3025 / 2985 ms logged
inside the 10 s backstop. **R13 is closed** in the register.

### Finding R14 — power: a running show slept on battery, and the adapter fell short on AC (recorded 2026-10-06, PR #37's fix-forward)

*Numbered after R13. The register row is in `docs/soak-protocol.md` §5, and
the evidence is in its §2a.*

**The user's word of 2026-10-06:** power availability matters, and it is
accounted for in the application's performance, not only in the documents.
The budget a show runs on is the adapter's wattage minus what the machine and
its attached devices consume: audio interfaces, Stream Decks, capture cards, a
monitor or teleprompter. The user met this with OBS, where frame-rate and
buffering problems appeared once every device was plugged in.

**What was measured on 2026-10-06:**
- **On battery, a running show slept.** This machine idle-sleeps 1 minute after
  the last input on battery (`pmset`: `sleep 1`). nbe holds no sleep assertion,
  so the slate-release end-to-end slept 491 s mid-test and failed its
  precondition. The run is void; the AC re-run was 4 of 4.
- **On AC, the 45 W adapter fell short.** Under a CPU-bound build the battery
  discharged while plugged in (`InstantAmperage` −1092 mA). Apple rates this
  machine's adapter at 87 W.
- **Low Power Mode is on, on AC too** (`lowpowermode 1`).
- **The battery is at 64% of design capacity**, `Service Recommended`.

**Not measured yet:** any frame-rate or underrun effect of power. The effect
on the thresholds is what the work order measures.

**Queued: the application work order (its own PR; the parts that change the
wire need the user's word).**
1. **The engine holds a sleep assertion while a show runs.** It takes
   `kIOPMAssertPreventUserIdleSystemSleep` while `showState` is RUNNING (and
   prevents display sleep when it drives a display output), and releases it at
   stop, unload and exit. This is the demonstrated failure. No wire change.
   Guard: `pmset -g assertions` names the engine during a running show,
   falsified by removing the assertion.
2. **Power state, reported.**
   - At preflight, in `preflight_report.json`, with warnings: on battery; an
     adapter below the machine's rating; Low Power Mode on.
   - On the telemetry tick: power source, adapter watts, system load, battery
     current, Low Power Mode, and the CPU speed limit (`pmset -g therm`).

   The tick fields are a wire change: a spec row and the audit trio.
3. **Headroom as an operator signal.** A discharge on AC is the deficit, made
   visible before frames drop. `DRESS-COUNTS` carries the power facts, so the
   R12/R13 trend sees them.
4. **The harnesses enforce precondition 6.** `scripts/soak.sh` and the gate
   battery refuse on battery, run under `caffeinate`, and record the power
   samples.
5. **Measure.** The dress rehearsal's draw on the 45 W adapter and on 87 W,
   with and without a representative device load (an audio interface, a
   Stream Deck, a capture card, an external display). Those are the numbers
   the thresholds and the warnings need.

### The preflight bound's constants: provenance and one residual (recorded 2026-09-07)
### The preflight bound's constants: provenance and one residual (recorded 2026-09-07)

The bound `nbe-preflight` runs under is derived from two measured constant
pairs — `MS_PER_FRAME_{RELEASE,DEBUG}` and `MS_PER_MB_{RELEASE,DEBUG}` — plus a
floor and a ceiling. Their provenance is **confirmed against the spec's own
reference target**: the measurements were taken on a 6-core Intel i7 @ 2.6 GHz,
which is the machine `docs/hardware-baseline.txt` records, and §0.3 makes that
machine normative for §12.11's arithmetic. They are not numbers from an
arbitrary laptop.

**Residual, with an owner.** CI runs `macos-14` — Apple Silicon, a different
architecture, where neither pair has been measured. The direction is very likely
favourable (hardware decode), and nothing is at risk today because CI's own
fixtures land on the 60 s floor with better than 17x margin. But it is
unmeasured, and the rehearsal is the job that will care first.

**3d's promotion re-baselines both pairs on the runner** before the rehearsal
becomes a required gate. A real-time gate whose timing constants were measured
on a different architecture is a gate that will flake for a reason nobody looks
at.

### Carried into 07 from the spine's own review pass (recorded 2026-09-07)

Two observations the independent pass over `9bd18f9` reproduced and declined to
file. Both are recorded so they have an owner rather than fading with the pass.

1. **`DEFAULT_METER_WINDOW_MS` and `telemetry_interval_ms` are two constants for
   one number.** R2's fix holds peaks across a meter window and publishes on its
   boundary, and the intent is that the window equals the interval a telemetry
   tick reports. That is true only while both constants read 1000:
   `AudioDriver` is constructed with `(state, sink, house_rate)` and has no
   access to `EngineConfig`, so it cannot derive the window from the interval.
   Not filed because the interval is hardcoded in `main.rs` and in
   `EngineConfig::default()` with no env override, so no user can create the
   divergence — but this is the §12.4 budget defect photographed one step before
   it became real, and that one cost three fix rounds. **Whoever makes the
   telemetry interval configurable owns deriving the window from it.**

2. **The meter window rolls on audio-block count, not wall clock.** If the
   driver's cycle rate drifts, window and tick slide relative to each other, so
   a tick reports the most recently *completed* window rather than the interval
   it nominally covers. Acceptable for a peak meter, and recorded in §10.1's
   implementation notes so nobody later asserts tighter timing on `busPeakDbfs`
   in ignorance of it.

### 07 was split into 07 and 07b (recorded 2026-09-05)

The upgrade pass rewrote 07 in place, and what it wrote is a different document
from what it replaced: `agents/prompts/07-overlay-level.md` is the spine (the
As-Built Ledger, R5, R6, the rehearsal's findings, F1/F2, the inherited hang)
plus the overlay level itself. The graphics mission it overwrote —
templates, the ticker, the breaking banner, the clock — is restored verbatim
from `26bf2f55^` as `agents/prompts/07b-graphics-templates.md`.

**The reason for the order: 07b composites onto the overlay level 07 builds.**
`View = overlay(transition(A, B))` has to exist before there is anywhere for a
ticker to survive a transition, so the level comes first and the furniture
second. 07 answers the text-stack question in writing (§5.2 question 2); 07b
implements it.

Suffix ordering follows the `02b`/`02c` precedent, so the series keeps its
numbering: nothing downstream of 07 renumbers. 07b has **not** had its upgrade
pass and gets one before execution — its scope decisions (packaged fonts, no
per-frame relayout, RSS sanitized at the control plane) stand; its Step 0
inventory predates the engine and does not.

### Step 07b records — the graphics layer as built (recorded 2026-09-11)

**Assumptions made real.**

a. **Fonts are package-resident, and the engine cannot reach a host face.**
   Ratified by the user; it is also what the code already did — preflight
   resolves `templateId` against the package's own `templates` and
   `fontAssetIds` against assets that package declares. `cosmic-text` is
   compiled with `default-features = false` for this reason: its defaults pull
   system font discovery, and `FontBook` is built from *bytes* with no
   constructor that takes a directory. `templates/graphics/README.md` claimed
   the opposite since the founding scaffold and now records that the package
   model won.

a2. **`font_asset_ids` is resolved and preflight-validated, and the renderer
   does not consult it** (recorded 2026-09-11, two-key F1). `scene.rs` resolves
   a template's fonts onto the text layer and preflight refuses a package whose
   `fontAssetIds` do not resolve — but `text_texture` rasterizes against the
   whole `FontBook`, and `TextRaster::rasterize` takes `families.first()`. Two
   consequences, both measured: **per-template font selection does not exist**,
   so a package declaring two faces with a template naming the second gets the
   first; and **a template naming no font still draws**, in the package's first
   declared face.

   That fallback is defensible — it is package-internal, and assumption 11
   forbids only *host* faces. What was not defensible was the test named
   `a_template_naming_no_font_draws_no_text`, which asserted only that the
   resolved list was empty while claiming a behaviour the code lacks. Renamed to
   `a_template_naming_no_font_resolves_an_empty_font_list`, which is what it
   proves.

   **v0.5 agenda item:** the schema field and the renderer now disagree in the
   tree. Either the renderer honours `fontAssetIds` per template, or the field
   is documented as advisory-for-preflight. Picking neither is what produced
   this entry.

b. **Text is a `LayerSource`, not a pipeline.** Shaped text rasterizes to RGBA8
   and is consumed by the *existing* `draw_for`. There is no second draw path
   and no second walk — `drawn_elements` is still the one walk, and
   `LayerSource::Text` answers "no" to the asset-blame question because a glyph
   is not media.

c. **`layer_for` stays pure.** It *names* text the way it already names video;
   the render loop resolves content per frame. That is what lets a clock — whose
   content is a function of the master clock — obey §6.5's no-per-frame-relayout
   rule: the cache is keyed on the rendered string, which changes about twice a
   second with `blinkColon` on, not thirty times.

**Reductions, stated plainly.**

d. **`ClockConfig.format: "locale"` is not implemented** and falls through to
   `HH:mm:ss`. ~~`timezone` and `locale` are read from the manifest and
   unused~~ — **corrected 2026-09-11 (two-key F3): they are not read at all.**
   `ClockSpec` carries `show_elapsed`, `format` and `blink_colon` and nothing
   else; `clock_of` never touches `timezone` or `locale`. "Read and unused"
   implied plumbing that exists and does not, which is a worse error than the
   reduction it was describing. `wall` mode reads the host clock as UTC.
   Localized and zoned formatting is real work with a real dependency, and
   pretending otherwise by aliasing it silently would have been worse than
   saying so.

e. **§16.7 rule 1 — "breaking override items appear first" — is not
   implemented, because the payload cannot express it.** `ticker.override`'s
   item schema is `{ text, language?, priority?, ttlSec? }`: there is no field
   that marks an item as breaking. Rules 2, 3 and 4 are implemented and now
   tested (`ticker.test.ts`, six cases, falsified by making the sort
   non-stable). Rule 1 needs either a schema field or a ruling that "breaking"
   means `priority: 100000` — a **v0.5 question**, not something to invent here.

f. **One font, chosen on coverage.** AC-15 wants English, Spanish-accented and
   Arabic from one packaged face. Measured: Noto Sans (2.0 MB) has no Arabic at
   all; Noto Sans Arabic covers both but is a variable font; **Amiri** (431 KB,
   OFL-1.1, static) covers all four test strings. Amiri is a classical Arabic
   typeface rather than a news-desk sans — an **aesthetic debt**, recorded as
   such. A show that wants a different face packages a different face; that is
   the whole point of package-resident fonts.

g. **Text sizing is proportional to the View, not to the element box.** The
   raster is sized at 6% of target height with glyphs at 72% of that. A template
   cannot yet specify a point size or a colour per field — `fields.color` tints
   the whole element. Per-field typography is 07b's obvious next increment and
   is not in this one.

**Debts.**

h2. **The doubled raster is not pixel-identical across the period** (recorded
   2026-09-11). Writing the test F2 demanded turned this up: glyphs land at
   sub-pixel offsets, so the second copy is phase-shifted a fraction of a pixel
   from the first — for "BREAKING NEWS" only 147 of 485 columns match exactly. A
   column-equality assertion therefore *fails on correct output*. What the design
   does guarantee is that the two halves carry the same content, and
   `the_ticker_raster_carries_the_item_twice_so_the_wrap_shows_no_jump` asserts
   it as column-ink correlation, with the measured separation quoted in the test:
   doubled 0.9538–0.9844, single copy −0.1254–0.2708.

   Worth knowing for the same reason: `ticker_period_px` derives the period from
   the doubled raster's own width, so removing the doubling makes the period
   silently half an item rather than failing. That self-consistency is why the
   original suite did not catch it, and why the new test asserts content rather
   than geometry.

h. **The ticker's raster holds the item twice.** That is how the wrap is
   seamless without a repeating sampler, and it doubles the texture for a long
   item. A 200-character item at 1080p is a few MB, which is nothing against
   §12.4's budgets — but it is a multiplier, and a package with many long ticker
   items has not been measured.

i. **No glyph atlas.** Each distinct rendered string is its own texture, cached
   by content. For a ticker and a clock that is a handful of textures; for a
   lower third edited live it is one per edit until the package reloads. An
   atlas is the standard answer and is not needed yet — but "not needed yet" is
   a measurement someone should repeat before 13's operator shell starts editing
   fields at speed.

j. **The uv window added to `LayerUniform` is used by exactly one caller.**
   Everything else passes `(0, 0, 1, 1)`, which reproduces the previous
   `out.uv = c` exactly — the 192-test suite including every golden frame is the
   proof that nothing moved. It is a general mechanism with one user, which is
   worth knowing before a second one arrives.

### Work order DRESS — R2/R4/R5 closed and the rehearsal promoted (recorded 2026-09-11)

Reproduced first, three runs on the normative machine (i7-9750H, 6p/12l, 16 GiB):

| Step | Finding | Rate | Nature |
|---|---|---:|---|
| #4 step 3 | R5 | **3/3** | Deterministic, not a flake |
| #5 step 4 | consequence of R5 | 3/3 | No clock → no audio → no clip-bus rise |
| #11 gate | R4 | **2/3** | Intermittent |
| #7 step 6 | R2 | **0/3** | Did not fail once |

Against the recorded 7-9 pass / 3-5 fail, the observed range was 9-10 / 2-3 —
and R2's step never failed, because R2 had already been fixed.

**R5 — closed. The clock was never the defect.** `MasterClock` is
`floor(elapsed × rate)` derived on read, so a frozen 0 means `start()` had not
been called. It had not: the engine applies directives in arrival order (§5.9)
and `show.load` decodes every asset — 10.7 s for this package — so `show.start`
sat queued behind it while the control plane had already recorded RUNNING.
Measured: `showState` RUNNING at t=3005 ms, first non-zero `masterClockFrame` at
t=10014 ms, then exactly +30/s. The rehearsal was pressing START while the
package was still loading, which is not a thing an operator does.

The fix is in the system, not the assertion: `waitForGrace` — the §5.9.5
acknowledgement the server has always tracked for `show.stop`'s quiescence
window — is now exposed as `ControlPlaneServer.awaitApplied`, and step 2 waits
on it. *(2026-10-14, SPEC v0.4.8 row 6: `show.stop`'s window has since moved.
It waits after its own directive is sent, for that exact version
(`waitForAck`), and it no longer waits through `waitForGrace`'s "at or
above". `awaitApplied` keeps "at or above", which is what step 2 needs.)* Step 2's own comment had claimed it did this all along. Falsified:
removing the wait returns steps 3 and 4 to failing, 10 pass / 2 fail.

**R4 — closed. The counter was right and the scheduling was wrong.** Underruns
appeared at exactly the tick the clock started and never during the load, which
ruled out the decode contention the original finding suspected. §8.10's table is
explicit that *a missed callback deadline IS an underrun*, so the count was
honest — and `falling_behind_the_cadence_is_an_underrun` pins that branch. The
defect was that `audio_driver::spawn` used `tokio::spawn`, putting a 33 ms audio
cadence on the same worker pool as the wgpu render loop; at `show.start` the
render loop spun up and audio missed its deadline. Audio now owns a dedicated OS
thread, with an `AudioThread` handle carrying a stop flag — a thread with no way
to stop is a leak, which the old `JoinHandle::abort` hid.

**R2 — verified closed, fixed before this pass.** `publish` max-merges each
block's peaks into a window and rolls on the telemetry boundary: peak-hold
across the interval, with the decision written down at the site. So 0/3 is
correct behaviour rather than luck. Falsified to prove the fix is load-bearing:
reverting to publish-then-reset fails
`a_transient_shorter_than_the_window_still_reaches_the_meter`, which is exactly
R2's scenario.

**The promotion was attempted, failed on CI, and was reverted — and that is the
most useful thing this work order produced.**

The criterion I was given and wrote down was "three consecutive green runs on
the normative machine". I met it: 12/12, 12/12, 12/12, each fix falsified. I
removed `continue-on-error`, dropped the scar from the job name, tightened the
band to 12/12, and pushed. **CI went red**, twice:

| | normative machine | macos-14 runner |
|---|---:|---:|
| `audioUnderrunsTotal` | **0** | **83**, then **120** |
| `droppedFramesTotal` | 0 | 1 |
| steps passing | 12/12 | 9-10 of 12 |

The gate step asserts `droppedFramesTotal == 0` and `audioUnderrunsTotal == 0`.
**Those are performance claims about reference hardware**, and the runner is
3 arm64 cores. §8.10 counts a missed 33 ms callback deadline as an underrun, and
a shared CI VM cannot hold that cadence beside wgpu — no code change in this
work order makes it. Requiring the job would block every PR on a hardware fact,
which is a worse version of the state promotion was meant to end.

**The corrected rule, recorded as the rule:** three consecutive green runs on the
normative machine **and** a green run on the runner that will gate. The first
half is what I had written; it was not enough, and writing only half of it is how
a job gets promoted into blocking every PR.

What promotion did buy, permanently: **it exposed a step that had been failing on
CI all along.** Step 1 spawns `target/debug/nbe-preflight` and the dress job
built only the release binary, so `not ok 1` was in every advisory run —
including `34596023038` — invisible behind `continue-on-error` and a band that
tolerated five failures. That build step is now in the job. An advisory job was
reporting a failure nobody could see, which is the argument for promotion in one
line, and the argument for reading advisory logs in the meantime.

The band therefore stays, with its reason changed: it used to tolerate R2, R4 and
R5 being red by design; it now tolerates the runner's capacity, and the header
says so. The artifact upload stays — `engine.log`, `telemetry.jsonl` and
`timings.json` are why a failure is debuggable rather than merely red.

**What would make promotion possible**, for whoever takes it next: split the gate
so the composition claims (no fallback, profile real, every step reached) are
asserted everywhere and the zero-drop/zero-underrun thresholds are asserted only
where they mean something — on the normative machine, or on a self-hosted runner
that is one. That is a change to what the gate claims, not a weakening of it, and
it deserves its own work order rather than being smuggled into this one.

**What the gate proves, and what it cannot.** The runner is `macos-14` — arm64.
The normative machine is Intel with discrete AMD graphics (§0.3). A green run
proves the parts compose and the protocol holds; it proves nothing about the
frame budget or any timing threshold on reference hardware. **Never cite a CI
duration as a performance result.**

**Two observations found on the way, neither fixed here.**

1. `renderNode.clockState` is not the render node's clock state. `server.ts`
   derives it as `state.showState === "RUNNING" ? "RUNNING" : "STOPPED"` — the
   control plane's own opinion, reported as an observation of the engine. That
   is what made R5 read as a clock defect: the wire said the engine's clock was
   RUNNING while it was stopped. Fixing it needs the engine to report its clock
   state, which is a §10.1 wire addition — a spec change this work order does
   not authorise. **v0.5 candidate.**
2. `masterClockFrame` publishes `master_frame().unwrap_or(0)`, so a STOPPED
   clock is indistinguishable on the wire from one that just started. Harmless
   while `masterClockState` carries the distinction, and worth removing when
   observation 1 is addressed.

### ZERO-COPY Phase 3 — blocked on design, memo written (2026-09-19)

Phase 3's executor read the frame path and **stopped before touching production
code**, which was the right call: the migration is three changes across three
crates (device reach, retarget, encode seam), and the only tractable slice would
have published `record_tap_path: zeroCopy` while the frames still went through
readback — the exact report-vs-reality lie its own falsification exists to catch.

`docs/zero-copy-p3-design.md` answers the three questions that stop was waiting
on, argued against the tree, with a go/no-go each and an ordered plan. **No
no-gos.** Two things changed shape under examination and are worth carrying:

1. **A finding against merged code.** §10.1.1 says *"The emitted field shape is
   always complete. A telemetry consumer MUST never see a missing field."*
   Phase 2 shipped `recordTapPath` as `.optional()`, absent until a take selects
   — deliberately, so absence means "no take yet". That conflicts with a
   normative sentence **today**, before any migration. The memo recommends
   stubbing (`"none"`) rather than scoping §10.1.1, and puts the fix first in the
   plan because it is independent of Phase 3.
2. **Backpressure does not transfer.** `RecordMsg::Frame { rgba: Vec<u8> }`
   carries an owned copy per frame, so shedding is free. A shared surface is one
   mutable allocation — shedding a surface already drawn into is a corrupted
   frame, not a skip. Zero-copy therefore needs a **surface pool** and a
   pre-check that asks "is a free surface available?" **before** the draw. That
   is the design, not a complication of it.

Mid-take chain loss is named as **new behaviour** (nothing in the tree answers
it) and decided to loud failure with `E_NO_ZEROCOPY`, on the precedent that
`record.stop`'s finalize failure withholds its ack rather than reporting a
success it cannot vouch for.

One prompt defect recorded: the work order asked for the argument against
"§7.4's render-role isolation sentence". §7.4 is *Element identity and state
model* and no such sentence exists. The real texts are §5.2 (topology) and
§10.1.1, and the latter settles device reach outright — the render node probing
its own hardware and reporting over `engineTelemetry` is exactly what the
effective quality profile already does.

### ZERO-COPY Phase 3b — the migration, executed (2026-09-20)

The memo's seven-step plan, in order, each step its own commit with its
falsification signature. Steps 1-4 shipped no behaviour change to recording;
step 5 is the migration.

`record.start` now probes the chain at the take's geometry, asks the published
table, publishes the selection to the §10.1 tick, and builds the take's surface
pool — so the frame path and telemetry's claim became the same statement at one
point rather than drifting apart. Measured on the reference machine, quiescent:
render + tap at 1080p30 goes **15.866 ms → 1.376 ms** (`docs/09-measurements.md`).
Three consecutive green rehearsals, each naming `zeroCopy (Table)`.

**Three things the memo did not reach**, all found by a test failing rather than
by review, and recorded in the memo under "Corrections found in execution":

1. **Q2's carried obligation named dimensions; FORMAT is a second one.** A
   record surface is `Bgra8Unorm` (VideoToolbox wants 32BGRA) and the composite
   pipeline targets `Rgba8Unorm`; wgpu refuses the mismatch outright. A BGRA
   sibling pipeline is built at init — at init, because compiling one mid-take
   would be work inside the frame path. The consequence the memo also missed:
   `readback_view` promises RGBA8, so it swizzles while the View is BGRA, or
   every golden-frame comparison silently swaps red and blue instead of failing.
2. **The probe's texture needed `COPY_SRC | COPY_DST`.** Q2's GO rests on the
   readback still working across the retarget, and a copy needs the usage flag.
   Free on the Metal side: `MTLTextureUsage` has no blit bit.
3. **Q2's "the take's surface for the take's lifetime" is superseded by Q3's
   pool**, written after it. The retarget is per frame.

**A number the migration found and did not keep.** Paced at 30 fps through the
production seams, a `cpuReadback` take sheds 20 of 40 frames; the zero-copy take
sheds 0. That is the case for the migration in the tree's own terms — and per
the gate split it is a soak number, not a test threshold.

§0.1 assumption 24's rescoped candidate (b) — ~~**remains UNRATIFIED**. This
work makes its mechanism a fact in the tree, not law.~~ **RATIFIED as SPEC
v0.4.4 on 2026-09-21**, together with the two §10.1 fields that report the path,
because the rescope's "MUST report which path is live" has no observable
without them.

**One thing left on the ledger:** `select_with_override` is built and tested and
wired to nothing, so the published table is currently the only voice and an
operator has no lawful way to restrict it. A gap in the escape hatch, not in the
rule. It lands wherever a config surface next appears — Prompt 10 is the likely
place — and `docs/09-measurements.md` ("On the ledger") is where that decision
is owed.

### Prompt 10 upgraded for the tree — 2026-09-21

`agents/prompts/10-streaming.md` was written 2026-09-10, before the ZERO-COPY
arc existed. It asked for streaming "fed by the shared GPU frames per Section
9.7 … never read back to CPU" — an aim with no mechanism at the time, and now
both a mechanism and **ratified law** (v0.4.4). Rewritten so an executor starts
from the tree rather than re-deriving it: what is already true (the stream row
and its refusal semantics, the pool, the encode seam, the telemetry precedent,
the measurements), the §9 quotes that settle the stream shape, and the
disciplines including §2a rules 7 and 8.

**Three decisions turned out to belong to the user, not to an executor**, and
the upgrade presents them rather than smuggling them into the brief:

1. **USER CHOICE C1 — RTMP or SRT first.** §9.1 and §9.4 both say "RTMP or SRT"
   and neither chooses. Recommendation: RTMP first (named first in both
   sections, the platform path, pure-Rust implementations so no FFI), SRT next.
   The FFI point is load-bearing: the workspace denies `unsafe_code` with one
   exemption and CI hard-codes it, so a libsrt binding is a policy decision,
   not an implementation detail — the same finding ZERO-COPY Phase 1 recorded.
2. **Blocker B1 — there is nowhere for the stream endpoint to live.**
   `OutputDefaults.stream` carries `protocol`, `videoBitrateKbps`,
   `audioBitrateKbps` and `additionalProperties: false`; the schema has no
   `url`, `endpoint`, `ingest`, `rtmpUrl` or `streamKey` anywhere. §16.14 gives
   `stream.start` an **optional** `url`. So a stream cannot be started from a
   manifest at all, and the old draft's "otherwise the manifest's
   `outputs.stream` wins" was unachievable. Recommendation: clarify §16.14 so
   the command's `url` is the only source — no schema revision, and a stream key
   does not belong in a file meant to be copied between machines.
3. **Blocker B2 — the owed override cannot be closed by Prompt 10 as the tree
   stands.** The ledger says `select_with_override` "lands wherever a config
   surface next appears" and names this prompt, but both `outputs.*` objects are
   `additionalProperties: false`, nothing in §9 or §16 mentions a tap-path
   override, and the standards forbid a prompt from editing the schema
   (`schemas/*.json` changes are spec revisions). The debt stays owed and the
   prompt is marked BLOCKED on that word for the override only.

Two smaller findings recorded in the prompt: the schema's `stream.protocol`
enum accepts `"whip"` while §9.1 defers WHIP, so a schema-valid unbuildable
package needs a stated refusal point (§17.5's precedent says preflight); and a
lawful refusal on a machine with no zero-copy chain has no honest error code in
§16.14's list.

**The design gate was also reframed.** The old draft implied the stream simply
shares frames. The memo's Q3 analysis transfers but its answer does not — a
stream outlives a take and its backpressure is the network's, so
shed-before-draw is wrong for a consumer that can stall for seconds. And §9.7
("One composite produces one GPU frame … MUST NOT recomposite") rules out a
second pool, because two pools mean two draws or a copy. The shape the spec
describes is one composite into one surface with N consumers holding
references, and the pool's `Arc::strong_count == 1` free list already
generalises to that at no cost.

### SPEC-REV — the streaming unblock, ratified as v0.4.5 (2026-09-21)

The Prompt 10 upgrade found four blockers between the executor and the work. The
user spoke all four the same day, and SPEC-REV landed them as their own change
rather than folded into the feature PR (§4's counter-precedent). **Prompt 10 is
unblocked.**

| Blocker | The user's decision | Landed as |
|---|---|---|
| **C1** transport | **RTMP first.** SRT deferred *pending a policy decision about the single `unsafe_code` exemption* — most SRT stacks are libsrt bindings, so admitting one is a policy change, not an implementation detail | §9.1 narrowed; schema enum `["rtmp"]` |
| **B1** endpoint | **The manifest carries it** — `show.outputs.stream.url`, declarative like `outputs.record.directory`. `stream.start`'s `url` is a per-run override; neither present is `E_BAD_PAYLOAD` | §9.4 (new rule); schema `url` |
| **B2** override field | **`outputs.{record,stream}.tapPath: { enum: ["auto","cpuReadback"], default "auto" }`.** The field is law; **the wiring stays owed to Prompt 10** | schema; changelog row 5 |
| **B3** `whip` manifest | **Refused at schema validation**, before load and before any command | §9.4 (new rule); `ValidationError::RefusedTransport` |
| **B4** refusal code | **`E_NO_ZEROCOPY`, reused not invented** — already the tap's token, already means exactly this | §10.4 registry; §16.14 |

**Four things worth carrying forward from how this landed.**

1. **It is a narrowing, and the record says so.** `protocol` went from
   `["rtmp","srt","whip"]` to `["rtmp"]`, so a v0.3 manifest declaring `srt` or
   `whip` was valid under v0.4 and is not under v0.4.5 — a second exception to
   "a v0.3 manifest that does not use `sequenceRef` is a valid v0.4 manifest".
   Nothing in the tree ever spoke either, and no fixture declares a protocol, so
   it breaks nothing that worked.
2. **The historical schemas were left alone.** `manifest.v0.2.json` and
   `manifest.v0.3.json` still accept `whip`, and **no code loads either** —
   `validate_manifest` embeds the v0.4 schema and validates every accepted
   `manifestVersion` against it, and the TypeScript generator reads v0.4 too.
   Editing a published historical schema would change the record of what v0.2
   meant while changing no behaviour.

   *Corrected 2026-09-22 (§2c). PR #29's body claimed* ~~"the only occurrence of
   either filename anywhere is the `$id` inside v0.2 itself"~~ *— which is
   wrong. The true counts are **11** occurrences of `manifest.v0.2.json` and
   **31** of `manifest.v0.3.json`: the README, `spec.v0.2.5.md`, `spec.v0.3.md`,
   eight files under `agents/prompts/`, `prompt-01-definition-of-done.md`, a
   superpowers plan, and `review-midpoint-report.md`. **All documentation; none
   a load** — a grep over `*.rs`, `*.ts`, `*.mjs`, `*.js`, `*.yml`, `*.toml`
   and `*.sh` returns nothing at all. The decision stands on the claim that
   matters; the phrasing overstated it.*
3. **A commit message that describes intent rather than its diff (§2c).**
   `94c71af`'s message says it *"also drops a stray `#[test]` that registered
   `a_refused_transport_fails_validation_and_says_why` twice"*. **The commit
   carries no such change** — `crates/nbe-core/tests/model.rs` only, +49/−0.
   The branch's history was rewritten (`reset --soft` plus two fresh commits)
   so that `validate.rs` landed already correct in `c942bf6`, which adds
   exactly three `#[test]` lines for three tests; by the time the second commit
   existed there was nothing left to remove. The message describes what
   happened during the work, the diff describes what the tree received, and a
   reader running `git show 94c71af` for that fix finds nothing. History is
   **not** rewritten to fix this — the record carries the discrepancy instead,
   which is what §2c is for. Found by PR #29's two-key pass; §2a rule 6's own
   subject.

4. **The refusal carries a REASON, not just a rejection.** The schema alone says
   `"whip" is not one of "rtmp"`, which tells an operator what was rejected and
   not why — the difference between fixing the manifest and filing a bug. A
   `check_transport` pass runs first and names the reason each transport is
   deferred, and `nbe-preflight` reports it under its own `refusedTransport:`
   prefix.

### The queue after Prompt 09 — decided 2026-09-17, in this order

| # | Work order | Why it sits here |
|---|---|---|
| 1 | **TRANSITIONS** | Ahead of the others because it is the last piece of the compositor's own contract. §16.6's `Animation.easing` permits six families and the overlay path implements **linear only**, ignoring `easing` and `delayFrames` — schema and implementation disagree in the tree today (v0.5 outline rows 4 and 5). Exit-time override is engine-reachable and wire-unreachable. None of that needs recording or streaming to exist, and all of it is in front of anything that composites |
| 2 | **ZERO-COPY** | 10's lead-in, and the resolution path SPEC v0.4.2 names for §0.1 assumption 24. The recording allowance is scoped to the record output on 1080p reference geometry with four expiry trip-wires; streaming inherits none of it, so the general rule has to be satisfied before a second encoder consumer arrives. IOSurface-backed `CVPixelBuffer` → `MTLTexture` → `wgpu::hal` import, so decode, composite and encode share one surface. Measurements already in `docs/09-measurements.md` — this work order spends them rather than re-deriving them |
| 3 | **Prompt 10 — Streaming** *(prompt upgraded 2026-09-21; **UNBLOCKED** — all four blockers decided and landed as SPEC v0.4.5, see the section above)* | Needs 2 first by construction: it is the second consumer of the same frames, and per-consumer readback is what assumption 24 exists to forbid. Inherits the guest-link JWT/`jti` revocation work and the TURN credential derivation rule (`[RI-5]`, §5.1 #11, §9.6.2). WHEP preview (AC-20) is explicitly post-v1 and not 10's scope |

The ordering is a dependency claim, not a preference: TRANSITIONS touches only
the compositor, ZERO-COPY is what makes a second encoder consumer legal, and 10
is that consumer. Reversing 2 and 3 would land streaming on a per-consumer
readback the spec forbids and the v0.4.2 allowance does not cover.

### The gate split — decided 2026-09-17, and DRESS-2 discharged by it

Two review passes converged on the same defect from opposite directions. Work
order DRESS found a job that was **red on purpose** and learned that teaches
reviewers to ignore red. The two-key pass over PR #18 found floors that were
**green about what they could not see** — they counted `passed`, a
capability-gated skip reports `ok`, and forcing the H.264 probe absent satisfied
all ten Prompt 09 floors while exercising nothing. CI run `34953923576` proved it
was not hypothetical: 22 of 60 tests and the rehearsal's record, sync and kill
steps skipped, so **AC-6 had never run in CI.**

The decision, recorded so nobody re-litigates it:

**a. Structure gates in CI, everywhere, today.** Every machine-independent claim
— the rehearsal's steps 1-11 composition and protocol assertions, the Prompt 09
hard floors — asserts and gates on every run. The 09 floors now carry a second
number, `exercised = ran - skipped`, which skipping cannot satisfy; the floors
are `min` of two derivations (ungated-test counts read from source, and the
worst-case no-hardware measurement) so no runner flakes them.

**b. Hardware claims gate on failure, skip loudly on absence.** The threshold
assertions — zero View drops, zero underruns, the record/sync/kill steps, AC-6 —
run wherever the capability exists, and on a run where they executed a failure
fails the job. On a runner without the capability they skip loudly, as they now
do, and the job stays green. The observational step names exactly what went
unexercised on every such run; its exit 0 is not evidence about recording.

**c. The soak is the gate for hardware claims.** `docs/soak-protocol.md` +
`scripts/soak.sh`: weekly, and **required before anything called a release**.
It owns the zero-drop/zero-underrun thresholds, the recording contract
end-to-end, and the v0.5 failover drill when that exists. Artifacts are the
existing `timings.json` + `engine.log` + `telemetry.jsonl` shape plus `soak.json`
and the Prompt 09 pressure counters (`record_tap_ms`, `skipped_record_frames`).
Scheduling mechanics are the operator's choice; the protocol is what must exist
in the tree. Its three outcomes are PASS / FAIL / **VOID**, and the last one is
the point: a run whose preconditions did not hold proves nothing in either
direction, and conflating it with FAIL is how "it passed on rerun" becomes a
habit.

**d. R7 rides the soak's watch list.** Flake-register entries are asserted every
soak with their **counts** recorded, not merely pass/fail, because a flake that
goes quiet may have moved to a machine nobody watches. R7's three sightings are
all load-sensitive and all green on rerun; a soak appearance is the first chance
to catch a payload.

**Two preconditions had to be recalibrated during the first runs, and both
recalibrations are the same lesson as (b).** The script initially refused on any
running `node` — unsatisfiable here, because long-lived MCP servers hold node
processes that never touch the tree — and demanded 20 GiB free when the machine
reports 14. A precondition the normative machine cannot meet is the same defect
as a gate that is always red. Narrowed to build/test contenders in this repo,
and to 8 GiB, with both derivations written down in the protocol.

**Quiescence earned its place empirically.** On 2026-09-17 the rehearsal returned
13/15 when launched immediately after a full `cargo test --workspace` plus
clippy, then 15/15 four consecutive times on an idle machine. The thresholds the
soak owns are exactly the assertions load perturbs, so the protocol measures an
idle machine or it measures nothing.

First soak on the normative machine at `e2e22cc`: **PASS** — preconditions held,
09 suites 0 skips, rehearsal 15/15, R7 0 sightings in 1 iteration.

**What the split does NOT do.** No self-hosted runner: the normative machine is a
daily driver and that decision stays open. No CI gate that pretends to cover
hardware it lacks.

## 08 — Companion mapping (elevated to a normative requirement)

Per the v0.4 outline §6, 08 is no longer "wire up a Stream Deck." It builds an **Input Intent schema** — a mapping layer that is *data, not code* — from physical intents (Companion button, MIDI note, keyboard chord) to semantic §16 commands, with per-device profiles as user-editable documents. The §16 command surface with token auth and audit is already the device-independent core (`docs/portability.md`, known-good boundary 1), so 08 adds a layer above it and must not add a second command surface beside it. The proof of generality is normative: a keyboard-shortcut adapter ships in the same prompt and must work with **zero** changes to the core. The Input Intent schema is a wire-level contract and takes normative spec text at 08's moment. Target hardware: StreamDeck XL via Companion.

**Two-key adjudications (PR #15, recorded 2026-09-12):** non-TAKE defaults unpinned → OUT (nice-to-have; presence proven); deck consumer wiring → OUT (next prompt's scope; the generation contract holds alone). Note that the rest of the PR body's gap list was closed during the fix rounds.

## TRANSITIONS coverage audit (recorded 2026-09-18, read-only)

Work order TRANSITIONS Step 0. Every transition kind/parameter the spec defines × engine × golden × falsification. Known state going in: cut and mix proven; overlay persistence across a 15-frame mix proven (07).

(Line numbers below are as-audited; Step 1 added the underlay — `Transition.underlay`, `FrozenLayer`, freeze/collapse in `on_take`, underlay branch in `scene_for` — so engine sites moved. The audit's claims stand; only the coordinates aged.)

| Kind/param | Spec citation | Engine? | Golden test? | Falsified? | Gap |
|---|---|---|---|---|---|
| cut | §7.9 zero-duration tween; `view.take` default | Yes — `TransitionKind::Cut` (`scene.rs:881-885`); non-`mix` maps to Cut (`directive.rs:430-433`); progress always 1.0 (`scene.rs:910-911`) | Yes — `take_changes_the_view_within_two_frames` (`prompt04.rs:161-212`) | Yes, pixels before/after boundary | — |
| mix | §7.9 opacity tween; §7.9.1 first frame by +1, complete by duration+1 | Yes, linear whole-frame alpha only (`render.rs:417-434`, applied `render.rs:609`) | Yes — `mix_interpolates_across_its_duration_and_never_mid_frame` (`prompt04.rs:214-255`) | Blend+completion yes; first-frame-by-+1 partial (jumps to start+5) | GAP-11 |
| wipe / sting / dve | §7.9 mask tween / alpha+audio cut point / single-element transform; wire admits all six (§16.2) | No — all map to Cut (`directive.rs:430-433`); post-MVP cover exists (§7.9 requires only cut+mix for v1) but wire-accepted kinds render as cut silently | No test sends any of the three | No | GAP-1 |
| move | §7.9 shared-element transforms; AC-23 identity/frame-exact/easing | No — no interpolation (`scene.rs:667-701`); maps to Cut | No | No | GAP-2, GAP-12 |
| durationFrames default 15/0 | §7.9 15 frames; schema defaults | Yes (`directive.rs:434-442`) | Partial — explicit values only, default path never taken | Partial | GAP-6 |
| duration zero / one / max (600/120) | Schema min 0, max 600/120 | Zero collapses to cut (`scene.rs:910-911`); one works by construction; NO clamp anywhere (`scene.rs:888-901`) — a 10000-frame mix is accepted | No | No | GAP-6 |
| easing linear | §7.9 six families; schema defaults `easeInOut` | Yes trivially (progress = elapsed/duration) | Yes, via mix test | Yes | — |
| easing ×5 others | Same citations | No — easing never read engine-wide (overlay path explicitly ignores, `directive.rs:757-760`) | No | No | GAP-3 |
| per-element duration/delay/stagger/path | §7.9 | No — only whole-transition `duration_frames` (`scene.rs:888-901`) | No | No | GAP-4 |
| preset + elementOverrides | §16.2 preset rule; `TransitionPreset` schema | No — engine reads neither; named preset silently plain cut/mix | No | No | GAP-5 |
| back-to-back takes | Spec silent (§17.3 no mid-transition row) | Works by construction (`render.rs:435-440`) | No two-take test | No | GAP-7 |
| cut landing mid-mix | Spec silent | Defined: instant new `view_item` (`scene.rs:910-911` + `render.rs:435-440`); audio ramp-only | No | No | GAP-8 |
| mix landing mid-mix | Spec silent | **[SUPERSEDED by Step 1 — see resolutions note below.]** Was: struct overwritten, blend discarded. Is: freeze-at-take underlay starts the new mix from the displayed blend | Step-1 suite | Falsified | GAP-9 |
| audio crossfade §8.7.5 | Mix crossfades over same duration | Yes, wired (`directive.rs:480-488`; `audio_control.rs:111-141`); curve linear (spec-legal; "equal-power" comment FIXED to linear in Step 3) | Mapping pinned (`prompt06.rs:1407-1427`); no take→master e2e | Partial | GAP-10 |
| audio cut ramp §8.7.6 | Cut + ≥5 ms ramp | Yes (`directive.rs:472-488`; 5 ms floor `audio.rs:25,104-106`); §8.7.7 unwired | Partial, same prompt06 tests | Partial | GAP-10 |
| AC-17 latency | ≤2 frames; mix first frame by next frame | Yes, next-boundary discipline (`directive.rs:443`) | Cut proven (`prompt04.rs:161-212`); mix first-frame unbisected | Partial | GAP-11 |
| AC-24 persistence | Ticker survives complex MOVE untouched | No as written (no move). Honest subset: DSK-above-transition + take-independence for cut/mix (solid stand-ins, hand-built `Transition`, pure-fn unit) | Partial (cut/mix only) | — *(no value recorded; §2c, 2026-10-14: this cell was missing, so GAP-12 rendered under Falsified? and Gap stood empty)* | GAP-12 |
| §17.2 TRANSITIONING | Scene-state publication + §17.3 events | No engine counterpart; resync silently clears in-flight transition (`directive.rs:863-865`) | No | No | GAP-7 |

GAP findings (spec-cited, Step 3 routes each): **GAP-1** wipe/sting/dve degrade to cut silently (v0.5); **GAP-2** move unimplemented, AC-23 unmet (v0.5); **GAP-3** easing families unapplied (v0.5); **GAP-4** per-element timing absent (v0.5); **GAP-5** preset/overrides ignored (v0.5); **GAP-6** duration edges unpinned + uncapped — Step 3 adds goldens for default/zero/one, clamp decision recorded (max accepted today); **GAP-7** back-to-back + TRANSITIONING unreported — Step 3 adds two-take golden; publication is v0.5; **GAP-8** cut-mid-mix abrupt — Step 3 adds golden pinning current behavior; **GAP-9** mix-mid-mix pop — Step 1's row (discover/define/test/falsify); **GAP-10** audio curve comment false + §8.7.7/bed unwired + no e2e — comment fix in Step 3, rest v0.5; **GAP-11** mix first-frame + e2e legs — Step 3 bisects start+1; **GAP-12** AC-24 as-written unproven — v0.5 (needs move).

### TRANSITIONS v0.5 findings (recorded Step 3 — deferred, not dropped)

- **GAP-1** — wipe/sting/dve render as cut with no signal (§7.9 mask tween / alpha+audio cut point / single-element transform; §16.2 admits all six).
- **GAP-2** — move has no interpolation and AC-23 (identity/frame-exact/easing) is unmet (§7.9 shared-element transforms).
- **GAP-3** — the five non-linear easing families are never read engine-wide (§7.9 six families; schema defaults `easeInOut`).
- **GAP-4** — per-element duration/delay/stagger/path absent; only whole-transition `duration_frames` exists (§7.9).
- **GAP-5** — preset + elementOverrides ignored; named presets render as plain cut/mix (§16.2 preset rule).
- **GAP-7 (publication)** — §17.2 TRANSITIONING scene-state publication + §17.3 events have no engine counterpart (§17.2/§17.3).
- **GAP-10 (remainder)** — §8.7.7 unwired, bed-as-clip unwired, no take→master e2e click test (§8.7.7; §8.7.5/§8.7.6 mapping only is pinned).
- **GAP-12** — AC-24 as-written (ticker survives complex MOVE untouched) unproven; needs move (§7.9; AC-24).

### TRANSITIONS resolutions (recorded post-Step-3)

Step-0 rows above stay as-audited per §2c; what changed since: **GAP-9 closed** — mix-mid-mix no longer discards the blend (freeze-at-take underlay; continuity falsified); **GAP-6/GAP-8/GAP-11 closed** — duration/default/zero/one/max goldens, cut-mid-mix pin, start+1 bisection; **GAP-10 comment leg closed** (linear-per-code). Underlay chains capped at 8 layers (drop-oldest-nonbase, human rates never reach it). Stop/resync clear in-flight transitions. Remainder stays v0.5 as routed.

## 09 — Recording

**Owns `marker.add` → recording chapter (§16.11)**, assigned by `[RI-5]` — 09's current doc does not mention it, and its upgrade pass must. Inherits two dormant deferrals that its own benchmark is the trigger for: zero-copy IOSurface→Metal (re-defer *with numbers*, not with prose) and the display surface. §0.1 assumption 14 fixes fragmented MP4 as the crash-safe default. 09 should also carry `[RI-8]`'s pinned residency policy into its own resource accounting: **unload-at-next-load**, so a stop→start recovery does not pay the 46 s reload measured in the report §3.2.

**Fourth sighting, 2026-09-17 (PR #19 run `35314193753`).** Same test, same
signature (`expected 3 directives, got 4`), on a branch whose entire diff was
docs, a shell script and a workflow comment — no TypeScript at all. Local run on
the same tree: 82/82. That makes two of four sightings on docs-only branches,
which is as close to proof as this gets that R7 is timing, not content.

**The register's own gap, closed.** Four sightings produced four counts and no
payloads, because the assertion's message carried only `directives.length`. The
message now dumps each directive's `command`, `seq` and `stateVersion`
(`server.test.ts`, diagnostics only — the assertion is unchanged), verified by
forcing the expected count to 2:

```
received: [{"command":"show.load","seq":1,"stateVersion":1},
           {"command":"preview.set","seq":2,"stateVersion":2},
           {"command":"view.take","seq":3,"stateVersion":3}]
```

So the next sighting names the duplicate instead of adding a tally mark, and the
redelivery-vs-extra-bump question becomes answerable rather than merely open.
The likely mechanism is visible in the test: a fixed `setTimeout(r, 30)` before
the count, which is exactly the window a slow or loaded runner widens.

### P9 Step 0b — frame-path fork decided (recorded 2026-09-14, normative machine)

Full table in `docs/09-measurements.md` (complete pastes, 300 frames/phase, 1080p30, Radeon Pro 555X). Decision: **(A) CPU readback first cut; zero-copy deferred with numbers.** Render+readback p95 19.3 ms / max 24.5 ms vs 33.333 ms budget, 0/300 over budget; GPU-copy proxy p95 4.5 ms. Reload cost re-measured: release preflight on dress_show 0.8–1.7 s (the stale 46 s was debug-under-contention; DRESS's 10.7 s was show.load decode, a different cost — cited correctly here). Answer changes if: dress-content composite + readback p95 crosses ~28 ms; encoder+mux+audio load trips drops; 4K target (≈48 ms readback alone); long-run p99 exceeds budget.

**Spec-correction candidate (visible, not quiet):** landing path (A) while §0.1 assumption 24 forbids CPU readback needs a scoped correction — a recording-first-cut allowance or a v0.5 rewording — recorded here rather than violated silently. Telemetry caveat on record: the tap sits outside `render_frame`'s deadline today, so `dropped_frames_total` is blind to it until 09 accounts the tap inside the deadline or separately.

### Disk pressure episode (recorded 2026-09-14, P9 parked mid-flight)

Arc: 1.1 Gi → 5.8 Gi → ~200 MB free with `target/` at 32 GB on the normative machine, discovered while 09's Fix-A agent was mid-flight. Cause decomposition: incremental compilation cache churning under the commit-mutate-restore falsification discipline (each mutation rebuilds; nothing ever prunes). `cargo clean` removed 23.4 GiB across 152,682 files; free went 593 Mi → 19 Gi.

Durable fix (one commit, at a boundary — no battery straddled it): workspace `[profile.dev]` gains `incremental = false` + `debug = "line-tables-only"` (function names stay in backtraces; full `debug = 0` held as escalation). Full rebuild after: 4m06s. Re-measured `MS_PER_FRAME_DEBUG`: 6 runs, 3.51–6.09 s → worst 6.77 ms/frame → constant 25 → 10, derivation comment updated (the old 13.5 s cold figure dated from the memory-pressured tree; codegen unchanged). Governance: `./scripts/clean-stale.sh` (cargo-sweep -t 14 or incremental fallback) as weekly habit in README; standards §4 sets the 40 GB target/ ceiling and the prune-after-battery duty. 09 resumes at Fix-A review with headroom for its media and rehearsal artifacts.

## 10 — Streaming

Inherits the guest-link JWT / `jti` revocation work (§10.7 #1) assigned by `[RI-5]`, and the TURN credential vending shape (§5.1 #11, §9.6.2) whose response has a schema but no derivation rule. WHEP preview (AC-20) is explicitly **post-v1** and not 10's scope — it waits for a WebRTC stack to exist. The mix-minus guarantee 06 built structurally (§8.6, `render_guest_return` has no path reading a guest's own bus) is 10's to preserve when real guests replace test tones.

### Executed as PR #30 — and repaired before merge (2026-09-23)

PR #30 executed this prompt and its author's own two-key pass called it
**mergeable** at `8733c99`. ~~"Verdict: mergeable. The two-key pass holds."~~
An independent review found it was not (§2c): CI was red, the stream carried no
audio, the encoder ran on the render loop, and the production loop was never
tested. The repair round (`87f93b2`..`251b405`) is recorded here; the numbers
are in `docs/09-measurements.md`, Prompt 10 section.

**What the repair round changed.**

1. **Refusal order is config → chain → encoder**, decided and pinned
   (`stream_start_refusal_order_is_config_then_chain_then_encoder`).
   ~~The chain refusal is the SPEC's claim; the encoder refusal is this
   build's.~~ Corrected by the own-author pass (§2c): §9.2's hardware-only
   encode is spec law too — both refusals come from the spec, and §16.14 states
   no evaluation order. The honest grounds: a configuration refusal is the same
   on every machine, and config → chain → encoder is the only order the CI
   runner (chain, no encoder) can observe — PR #30's encoder-first order failed
   there in run 35878301689. The pinned order is drafted as an UNRATIFIED
   candidate in `docs/v0.5-outline.md` §7.
2. **The stream has its own thread** (`nbe-stream`), owning the VideoToolbox
   session and the AAC converter; the render loop's whole stream cost is one
   bounded `try_send`. PR #30 opened the encoder on the first live tick
   (measured 35.6–39.9 ms, over the 33.3 ms budget in 10 of 11 starts) and
   encoded every frame on the loop.
3. **The stream carries the show's audio** through its own `AudioTap`, AAC at
   the manifest's rate, the codec's own AudioSpecificConfig. MediaMTX logs
   `2 tracks (H264, MPEG-4 Audio)` from the engine's pipeline.
4. **Honest timestamps and parameters**: RTMP timestamps are media time (video
   PTS on the show clock; audio sample count), the extended timestamp repeats
   on Type 3 chunks, and fps / bitrates come from the show and the manifest.
5. **The transport conforms**: it announces its own chunk size instead of
   silently adopting the server's, answers pings, reads the server with a real
   chunk reader, and its static AAC header is 48 kHz (was 44.1).
6. **A merged defect, found here: the zero-copy free rule.** VideoToolbox
   retains the `CVPixelBuffer` after `encode_pixel_buffer` returns (1.6–17.5 ms
   measured); the pool called the surface free on `Arc::strong_count == 1`
   alone. **The record path shipped this first**, in ZERO-COPY Phase 3b; the
   rule now waits for VideoToolbox's release — read after an `Acquire` fence
   (`7c57ccf`), without which the rule was sound on x86-64 but unproven on
   arm64. The precise exposure and "no shipped recording has been audited" are
   in `docs/09-measurements.md`; the guard's soak row makes the soak its home.
7. **The loop is in the library** (`tick::run_tick`, `tick::run_loop`), so
   tests drive the production loop; the dress rehearsal streams (step 9); G1's
   guard runs on real surfaces instead of `SharedPool<()>`.
8. **`streamBufferMs` is `0` with no session again** (law); PR #30's `-1`
   sentinel is an UNRATIFIED candidate in `docs/v0.5-outline.md` §7, beside a
   drafted `streamTransportState`. *(Superseded 2026-09-25, §2c: SPEC v0.4.6
   ratified both. The sentinel is `-1` again, now as law, and
   `streamTransportState` is on the wire — see "SPEC v0.4.6" under § 11. The
   "(law)" above was also an inference: §10.1 stated no no-session value until
   v0.4.6.)*
9. **`stream.start` no longer stalls the directive path**: the encoder probe
   opened a real VideoToolbox session on every call (36–38 ms); a positive
   answer is now cached and warmed at boot. 37.2 ms → 4.3 ms per start.

**First soak with the stream rows: PASS** (2026-09-25, `main` `57dd4b9`, normative machine, load 2.68): 3/3 clean iterations, zero skips; every required stream capture present — `RECONNECT`, `SURVIVAL`, `G1 guard`, `LIVE LOOP`, `BOTH LIVE`, `VT retain guard` (12/12 held, 0 handed out) — and the rehearsal's `STREAM` line each iteration (148–149 video, 233–234 audio messages, 0 drops, 0 underruns); MediaMTX interop `proved`. Artifacts: `target/soak/20260925T061156Z/`.

**Still owed, named rather than hidden.**

- **nginx-rtmp is untested.** The chunk-size and ping fixes stand on RTMP
  §5.4.1 / §7.1.7 and the conforming double; MediaMTX accepted PR #30's
  chunk-size behaviour too (it evidently treats chunk size symmetrically), so
  the real-server proof does not discriminate that bug. A per-direction server
  is the missing witness.
- **The client never sends Acknowledgement messages** (it receives almost
  nothing, so a server's window is never reached); recorded, not built.
- ~~**Audio-tap eviction is per sample.** A consumer stalled past the ring's
  capacity can lose an odd number of samples and swap stereo channels — the
  record tap has the same property. The stream thread keeps drains paired, but
  cannot repair an eviction.~~ Wrong about both paths (own-author pass, §2c).
  The hazard: with the ring full, a drain racing an eviction mid-push returned
  an odd count starting on a right-channel sample (probe: 177 of 907 drains).
  **Stream:** the thread's carry re-paired from the shifted start — L and R
  swapped silently until the next odd drain (new with PR #30). **Record:** the
  writer refused the odd push ("audio must be whole stereo frames") and the
  take ended (pre-existing, merged). **Fixed** (`909f89d`): `AudioTap` stores
  and evicts whole stereo frames, so every drain starts on a left sample and
  holds whole frames; the guard `drains_racing_eviction_stay_stereo_aligned`
  reads 0 odd of 8,833 drains (104 of 598 with the old eviction).
- ~~**Transport state is not on the wire** — see the v0.5 §7 candidate.~~ **Landed 2026-09-25**: SPEC v0.4.6 row 2, `streamTransportState` (§2c).
- **The extended-timestamp fix has no real-ingest witness past 0xFFFFFF.**
  It is guarded against the conforming double
  (`extended_timestamp_repeats_on_every_type3_chunk`, base `0x0100_0000`);
  a 4.66-hour soak leg or a real-ingest marathon is its owed witness.
- **The second `Acquire` fence, deferred deliberately.** `SurfacePool::is_free`
  proves one direction: the retain read cannot see the pre-encode baseline. The
  other half — VideoToolbox's pixel reads ordered before the compositor's next
  writes once the read sees the release — rests today on CoreFoundation's
  internal atomics and on Metal submission acting as a barrier. A second
  `fence(Acquire)` after `encoder_released()` returns true would make it
  explicit, at one `dmb ishld` on arm64 per acquire. Both keys judged the
  current shape sound in practice; the line is owed to the first ARM production
  target or the next quiet moment, whichever comes first.
- ~~**QUEUED (small work order): the stream thread's teardown wait, rebound by
  event (Finding R11).**~~ **DONE in `bb7d4a2` (Prompt 11 WU0, 2026-09-26)** —
  the rebound and the capture, as queued below; R11 is marked resolved. The
  queue line, kept: `stop_thread` already waits on the thread's exit event
  but gives up after 500 ms of wall clock (`STREAM_THREAD_STOP_TIMEOUT`) and
  detaches the thread. The wait's purpose is deadlock detection, and a deadlock
  and a slow exit differ in whether the exit *ever* happens. So bound the wait by
  the exit event, with a generous wall backstop sized to §16.1's 2 s `show.stop`
  window. Also add the capture R11 requires: on expiry, record `StreamStats`
  (`encoder_ready`, `encoder_open_us`, `aac_ready`) and the thread's state, so
  the next sighting names its phase. Not built in PR #33; the merge was waiting.

## 11 — Watchdog

*Upgraded 2026-09-25 UTC — see "Prompt 11 upgraded for the tree" below; ~~blocked on C1 and B1–B5~~ all six decided 2026-09-25 — see "SPEC v0.4.6" below.*

The watchdog itself exists and is gated (pass 4 confirmed deadline accounting and fallback trip both fail correctly when deleted). What 11 must now add is **the automation engine runtime** (§13, AC-25), assigned by `[RI-5]`: triggers, the once-per-frame limit, runtime cycle suppression, and audit logging of every automation action. `automation.hold` exists from Prompt 02; the engine behind it does not. 11 also inherits **F3's fix** as context — the fix round adds a `fail_view` seam, so §10.3's engagement path finally has production coverage that 11's work must keep.

### Prompt 11 upgraded for the tree — 2026-09-25 UTC (locally 2026-09-24 −0700)

`agents/prompts/11-watchdog.md` was written 2026-09-10 as a watchdog prompt: detect, shed, report, restore. By then the watchdog was built and gated, and this entry had already (2026-09-04) assigned the slot the **automation engine runtime**. The draft cited two prompt files that do not exist and forbade watchdog work on the render loop, where the built watchdog lives (AC-7 states a one-frame deadline; that the loop is the only place to meet it is the prompt's inference). The upgrade rewrites it around what the tree has (§0 of the prompt: watchdog on the loop, ladder rung 1 with hysteresis, the automation schema/types/commands with no runtime — ~~and audit kind~~: `AuditRecord.kind` does not include `"automation"` yet, only a comment anticipates it (corrected in PR #32's fix round) — `autoFollow` normative since v0.1 and unimplemented) and names the decisions owed before an executor starts:

| # | Decision | Recommendation in the prompt |
|---|---|---|
| **C1** | Scope: automation runtime + the watchdog remainder with a subject (§10.3 threshold question, ladder rung 2), or split 11a/11b | one prompt, two gated work units; rungs 3–4 and GPU timing out (no subject / Prompt 12) |
| **B1** | `audioLevel` must fire within one frame; bus levels reach the control plane at 1 Hz | engine level-crossing event (wire candidate, UNRATIFIED) |
| **B2** | `streamHealth` needs transport state, which is not on the wire | ratify `streamTransportState` (v0.5 §7) |
| **B3** | `mediaStart` has no engine event | control-plane-side (the take applied) for v1 |
| **B4** | §13.4 transitive cycle rejection needs a command → trigger effect table the spec lacks | draft the table as an UNRATIFIED candidate *(the recommendation as made; the table was drafted so, and ratified 2026-09-29 — SPEC v0.4.7 row 9)* |
| **B5** | AC-25 #2's "pending actions" — rules have no delay | fired-but-not-dispatched within the current frame |

~~**Prompt 11 is BLOCKED on C1 and B1–B5.**~~ *Unblocked 2026-09-25 — the user spoke all six; see the next entry.* They are the user's words, landed the way SPEC-REV landed Prompt 10's four blockers (v0.4.5), before any executor starts. One finding to settle during execution, not assumed: §10.3 says "more than 1 frame", the built watchdog trips when accumulated `ceil(late / budget)` exceeds 2.

### SPEC v0.4.6 — Prompt 11's six decisions, spoken 2026-09-25

The user spoke Prompt 11's six decisions on **2026-09-25** (UTC and local −0700
agree on the date). SPEC-REV-2 landed them the v0.4.5 way — the words predate
the text, so there was no drafting phase, and each ratified row landed with its
mechanism and had its guard run at its landing commit. Branch `spec-rev-v046`.

**Merged** as PR #33 on 2026-09-26 at 03:18 UTC (locally 2026-09-25 20:18
−0700), squashed to `619846c` on `main`. Its tree is identical to the PR's
final head, `6988c04`. **Main's CI at the merge commit: run `36214380771`,
success** — ~~control-plane, dress rehearsal and rust all green~~
control-plane and rust green; the dress rehearsal job, advisory by decision
(`continue-on-error`), concluded success with 15 of 16 passing (`not ok 12 -
[RI-1] gate`). *Corrected 2026-10-03 (§2c): "all green" read the job's
conclusion, not its tests; found while landing the recall leg (§ 13).*
Recorded 2026-09-26 as the first commit of Prompt 11's branch; it was owed
from the merge.

| # | The user's word | Where it landed |
|---|---|---|
| **C1** | One prompt, two gated work units | **Recorded for the executor** — `agents/prompts/11-watchdog.md` §1 and §3 |
| **B1** | The engine level-crossing event; its mechanism is Prompt 11's to build and it ships there as a candidate | **Candidate, home: Prompt 11's feature PR** — marked UNRATIFIED with its guards there, ratified by the user separately. No v0.4.6 row |
| **B2** | Ratify `streamTransportState` | **Landed, v0.4.6 row 2** — §10.1 field, note and §10.1.1 ownership; engine, protocol, control-plane schema and `buildTick`; token, completeness, readability and redial-on-the-wire guards; the mirror fixture samples `"reconnecting"`; the soak captures distinct values. Landing `f15b617`, record `9d1dfed` |
| **B3** | Control-plane-side `mediaStart` | **Recorded for the executor** — the take applied. §13.4.1's `mediaStart` column uses it |
| **B4** | The command → trigger effect table, as a candidate | ~~**Candidate, drafted UNRATIFIED in SPEC §13.4.1**~~ Drafted in SPEC §13.4.1 (v0.4.6 row 4, `69a2b54`): all 55 §16 commands with citations. Its mechanism — WU5's transitive check — ships with Prompt 11's feature PR, ~~and the table is ratified separately~~. **RATIFIED by the user's word of 2026-09-29, riding PR #34's merge** (SPEC v0.4.7 row 9; struck in the §13.4.1 flip, §2c) |
| **B5** | "Pending" = fired-but-not-dispatched within the frame | **Recorded for the executor** — pinned by a test in WU2 |

The same order settled two Prompt 10 candidates from `docs/v0.5-outline.md` §7:
**row 1**, `stream.start`'s refusal order as §16.14 law (`724e35f`, record
`0a39c29`), and **row 3**, the `streamBufferMs` NO-SESSION sentinel, `-1` on the
engine and the control plane (`00d8b46`, record `9a5c3a2`). All three §7 rows now
read RATIFIED.

**Found while landing it, recorded rather than fixed** (the order allowed no
other behaviour):

- ~~**A keyless `rtmp://` endpoint publishes nothing and says live.**
  `resolve_stream_url` checks only the scheme, so `rtmp://host/app` passes. Then
  `parse_rtmp_url` refuses it, the session opens with no publisher, `streamState`
  goes live, and nothing is published. `StreamSession::stream_buffer_ms` reads
  `0.0` for it, not the sentinel. The new field is how it surfaced: the tick reads
  `streamTransportState: "closed"` beside a live stream.~~ **FIXED in PR #33's
  fix round (`40e96e6`, §2c).** The two-key pass judged a ledger line
  insufficient: an operator sees a stream that says live and reaches no one. Now
  `resolve_stream_url` runs the publisher's own `parse_rtmp_url`, so a keyless
  or malformed endpoint is refused `E_BAD_PAYLOAD` at resolve time, before any
  probe or session, naming the parser's reason. The guards are in
  `stream_url_precedence`. The fix also exposed the tests' reliance on the
  defect. Keyless URL literals that the tests treated as valid (15 in
  `stream_url_precedence`, 23 in `prompt10_stream_cmds`, 1 in
  `prompt10_telemetry`) resolved only through it. They gained a `/key`, so the
  hardware-gated stream tests now open real publishers.
- **`item.stop` has no engine effect.** The engine does not route it, so a
  stopped timed item keeps its scheduled `end`, which the control plane's
  `markDone` drops. §13.4.1 records it for WU3/WU5.
- **The control plane's `StreamState` type admits `"reconnecting"`,** and nothing
  sets it. `stream state tokens are stable` pins the token; only tests assign it.
- **§10.1 never stated `streamBufferMs`'s no-session value.** The repair round's
  "§10.1 law: 0" was code-comment inference, so v0.4.6 wrote the sentence rather
  than amending one.
- **`stream_tap_selection`'s doc comment said "not cleared at stop",** and every
  stop path clears it. It is corrected in row 2, with the old text struck.

**~~A load flake, recorded by class~~ A teardown-wait flake — superseded by
Finding R11 (PR #33's two-key pass).**
*Corrected 2026-09-25, a day after it was written (§2c). Its causal claim — that
load explains the failure — is refuted: the same failure recurred at load 2.59,
under the ceiling, and did not recur in 30 tries at load 4.6–6.3. See **Finding
R11** beside R7 and R10 in § 07. The original text stands, struck where it is
wrong:*

`record::stream::tests::close_error_seam_fails_loudly_with_the_network_token`
failed once, at load 27.3, on its teardown-wait bound. Its second
`stop_and_close` must see the stream thread exit within
`STREAM_THREAD_STOP_TIMEOUT` (500 ms). The thread drops its VideoToolbox encoder
on the way out (the constant's own doc says this is short but not instant), ~~and
at load 27.3 that plausibly took longer. That cause is an inference, not a
measurement.~~ The same test passed in this PR's workspace battery at `7346879`,
which started at load 3.85. ~~The class is R9's: a wall-clock bound that measures
the machine, not the code. By doctrine a run above the quiescence ceiling is VOID
for anything timing-shaped, so this is not a finding against the teardown.~~
**No test changes: the bound is not weakened to look green.** The 500 ms, plus
the transport's 800 ms, is what keeps a stream stop inside §16.1's 2 s window
beside record's parallel 1.5 s (`STREAM_THREAD_STOP_TIMEOUT`'s own doc). ~~It
matters on CI only if a loaded runner approaches it. Today's runner has no H.264
encoder, so its stream threads run without one and have no VideoToolbox session
to invalidate on the way out.~~ *(Struck: the test runs on every CI `rust` job
and can red it with an attempt-1 failure — R11's CI consequence.)* One
consequence of the keyless fix widens the ~~class~~ exposure: the hardware-gated
`prompt10_stream_cmds` and `prompt10_telemetry` tests now open real publishers
and stream threads, so their stops are bounded by the same 500 ms. ~~A loaded
local run can flake them the same way, and the same doctrine applies.~~ Any run
can trip that bound, loaded or not, until R11 is resolved.

### Prompt 11 executed — branch `prompt11-watchdog` (from 2026-09-26)

A running record, one line per work unit, completed as each lands. Base: `main`
at `619846c`.

- **Merge record** (`5e9afc9`): PR #33's merge, and main's CI run `36214380771`,
  recorded in the v0.4.6 entry above.
- **WU0** (`bb7d4a2`, record `7fb42ab`): R11's rebound. The exit wait hangs off
  the exit event with an 1100 ms deadlock backstop, and a sighting carries its
  phase capture. R11 is marked resolved (see Finding R11 under § 07).
- **B1** (`6158435`, test fix `6322f90`, record below): the `audioLevelCrossing`
  event. `nbe-core` reads the params, the engine compares measured levels
  per block and emits, preflight refuses malformed rules, and the control plane
  parses the frame and hands it to its consumer. SPEC v0.4.7 carries it, drafted
  here and ratified by the user's merge. Guards and falsifications are in
  v0.4.7's changelog. Found while landing it: crossings queued during an outage
  would have replayed on reconnect, firing rules late. They are dropped at
  connect.
- **WU1** (`2d6a9a4`, guards `2eedec7`): the evaluator core, on the ONE command
  path (`server.ts` `runCommand`). A rule's action runs as `operator`, actor
  `automation:<ruleId>`, audited `kind: "automation"` in the same `AuditLog`.
  The params contract covers every trigger kind (`docs/automation-design.md`
  §2). `nbe-preflight` refuses by it; the control plane's read holds the same
  verdicts over one shared 32-case fixture and also judges action payloads.
  `rssKeyword` is refused at load because no RSS source exists, contradicting
  the draft's "available". `enabled: false` is now honoured. Falsified at
  `2eedec7`, each restored with `git checkout`:

  | Mutation | Guard that failed | Signature |
  |---|---|---|
  | an automation action not audited | `a rule fires through the real command path…`, `a rule disabled… re-arms it` | `one action row: []` — `0 !== 1`; `re-armed, it fires on the next change` — `0 !== 1` |
  | `enabled: false` ignored at load | `a rule disabled in the manifest never fires…` | `true !== false` |
  | the control plane's load-time read ignored | `an action payload preflight cannot judge…` | `'ok' !== 'error'` |
  | a rule acts as `admin` | the audit-row test; `…faces an operator's role check…` | `an automation action faces an operator's preconditions (§13.1)` (+ `'admin'`); `the rule is refused as an operator is` (+ `'E_NOT_FOUND'`) |
  | preflight's rule check removed | `every_trigger_kind_is_read…`, `a_malformed_audio_level_rule…` | the rss rule `must fail preflight` (left 0, right 2) |
  | `"automation"` removed from the audit kind union | the kind pin (compile) | `TS2678: Type '"automation"' is not comparable to type '"command" \| "auth" \| "preflight"'` |

  Also found: with no `nbe-preflight` built, the control-plane suites that
  `t.skip()` on it never exit. `render-channel.test.ts` hangs the same way, so
  this predates this work; CI always builds preflight. The release binary was
  also stale (Sep 24), and the control-plane tests resolve release before
  debug, so they had been running an old preflight. It was rebuilt.
- **WU2** (`e985e40`): the guards for the limiter, hold, and B5's pending queue,
  with two corrections found while writing them.
  - A hold now cancels pending actions whichever command engages it. A
    `snapshot.recall` restoring a held snapshot had been missed.
  - The drain's redundant hold re-check is removed; nothing could have caught
    its absence.

  A test seam, `automationClock`, keeps the limiter's frame deterministic.
  Falsified at `e985e40`:

  | Mutation | Guard that failed | Signature |
  |---|---|---|
  | the limiter removed | `a rule that would fire twice in one frame fires once…` | `fired once in the frame` — `2 !== 1` |
  | hold suppression removed | `a held engine fires nothing…` | `held: nothing dispatched` — `1 !== 0` |
  | pending not cancelled when a hold engages | `a hold cancels every pending action…` | `only the hold itself dispatched` (actual: three actors) |

- **WU3** (`75f7ad4`): the trigger adapters.
  - **Where each is wired.** `mediaStart` (B3) on a take; `mediaEnd` on the
    recorded `PLAYING → DONE`; `timer`; `timeOfDay`; `hotkey` on an
    `intentSource`, whatever the carried command's outcome; `streamHealth` on
    the tick's `streamTransportState`, with a baseline (§5 of the design note
    argues the token mapping); `audioLevel` on the v0.4.7 crossing.
  - **The §13.4.1 `item.stop` claim** is now guarded: a stopped timed item's
    late `end` fires nothing.

  Falsified at `75f7ad4`, one adapter at a time. Each fails exactly its own
  test (17 pass, 1 fail):

  | Mutation | Guard that failed | Signature |
  |---|---|---|
  | mediaStart unwired | `mediaStart fires when a take puts its item on air…` | `only A2's start, once` |
  | mediaEnd fires on any `end` (the `PLAYING → DONE` check removed) | `mediaEnd fires on the engine's end of a PLAYING item — and not for a stopped one` | `a stop is not a completion (§13.4.1's item.stop row)` |
  | timer never scheduled | `timer fires atMs…` | deep-equal: `[]` vs the timer event |
  | timeOfDay never scheduled | `` timeOfDay fires when the local wall clock reaches `at` `` | `at 00:53:03` (deep-equal) |
  | hotkey only on accepted commands | `hotkey fires when its binding fires, whatever…` | deep-equal: `[]` vs the hotkey event |
  | streamHealth without a baseline | `streamHealth fires on a change TO…` | `after live` |
  | audioLevel unwired | `audioLevel fires on the engine's matching crossing…` | deep-equal: `[]` vs the audioLevel event |

- **WU4** (`4d11ba9`): `autoFollow`, normative since v0.1 and unimplemented until
  now.
  - **What it does.** When an item carrying it completes (`PLAYING → DONE`), the
    View advances to the next rundown item with `view.cut`.
  - **How it runs.** Through the evaluator's queue, with actor
    `autoFollow:<itemRef>`. A hold suppresses it, and cancels it while it is
    still pending.
  - ~~**The last item** has nowhere to go; that is audited, not invented.~~
    *Superseded by the user's word of 2026-09-27 (§2c): a completion with no
    next item is a no-op that audits nothing, because nothing was attempted.
    The first landing's `autoFollow.endOfRundown` row is gone (`34fc557`).*

  Falsified at `4d11ba9`:

  | Mutation | Guard that failed | Signature |
  |---|---|---|
  | the advance removed | 4 of the 5 autoFollow tests | `the next item is on air` — `'AF' !== 'AN'` |
  | autoFollow ignores hold | `a hold suppresses autoFollow…` | `held: the View does not advance` — `'AN' !== 'AF'` |
  | the wrong next item (two ahead) | `autoFollow advances to the next rundown item…` | `'AZ' !== 'AN'` |
  | ~~end of rundown not audited~~ (superseded: now the required behaviour) | ~~`autoFollow on the last rundown item goes nowhere, and says so`~~ | ~~deep-equal: `[]` vs `["autoFollow.endOfRundown"]`~~ |

  **The correction, `34fc557`**, per the user's continuation order. The
  once-per-frame limiter now counts `autoFollow`, keyed by its actor; before,
  it bypassed the limiter. A completion with no next item audits nothing.
  Falsified at `34fc557`:

  | Mutation | Guard that failed | Signature |
  |---|---|---|
  | the limiter does not count autoFollow | `autoFollow advances once per completion, never twice, and the limiter counts it` | `no second advance in the frame` — `2 !== 1` |
  | a completion with no next item audited again | `autoFollow on the last rundown item is a no-op that audits nothing` | `nothing attempted, nothing audited` (deep-equal) |

- **WU6** (`d455aa0`): ladder rung 2, built as the user decided on 2026-09-27.
  The §4a stop is resolved; the draft's "buildable" row is struck with the
  tree's correction.
  - **The invariant, as tested.** Eviction applies only to loops not feeding
    the View. With a loop on air and rung 2 reached and held, that loop's ring
    is resident on every frame. The View keeps its cadence: the loop repeats
    every period and moves within it. The previewed loop sheds, and is
    re-acquired on take. Both are recorded and counted, and `degradationRung`
    reports 2.
  - **The ladder is in order.** Rung 1 comes after 2 consecutive late frames;
    rung 2 comes only from rung 1, after 2 more late frames while rung 1 is in
    force.

  Falsified at `d455aa0`:

  | Mutation | Guard that failed | Signature |
  |---|---|---|
  | on-air eviction permitted (the shed ignores what feeds the View) | all 4, the continuity guard among them | `frame 6: the loop feeding the View must stay resident (rung LoopsShed)`; `frame 4: the on-air loop stays resident` |
  | rung 2 straight from nominal | `the_ladder_climbs_in_order_rung_1_before_rung_2` | `rung 1 at 2 late frames, rung 2 at 4 — in that order` — left `[0, 2, 2, 2, 2, 2]` |
  | no re-acquire on take | `an_off_air_loop_sheds_under_rung_2_and_is_reacquired_on_take` | `re-acquired on take, recorded` — left `[]` |
  | shedding disabled | 3 of 4 | `the previewed loop is shed` |

  Three findings from building it, recorded rather than fixed:
  - **The watchdog comes before rung 2.** The watchdog trips on a missed-frame
    sum above 2, which is three consecutive misses. A rung-2 threshold counted
    consecutively would sit behind the fallback slate and never act. Rung 2
    therefore counts late frames since rung 1; the ladder's domain is
    sustained but intermittent pressure. This matters for WU5's §10.3 answer.
  - **The engine never clears `fallback_active`.** It is only ever stored
    `true` (`watchdog.rs`, `render.rs`, `directive.rs`). Once the slate is up,
    nothing in the engine takes it down. For §10.3, and the user's.
  - **The engine ignores `preview.set`.** Its preview item changes only
    through `show.resync`, so the preview bus shows a new preview only after a
    resync. This predates Prompt 11 and is out of scope.
- **WU1's spec bookkeeping** (owed since WU1; ordered 2026-09-27):
  - **The params contract is spec text.** SPEC v0.4.7 row 3: §13.2 gains the
    table preflight enforces, written from `nbe_core::automation::validate_rule`
    arm by arm, ratified by the user's merge. It is guarded by the shared
    32-case fixture, which both readers read, and by preflight's every-kind
    test. The guards' run is in v0.4.7.
  - **`rssKeyword` is refused, and the spec now says so.** §13.2's trigger row,
    §13.4.1's reading and its `ticker.refreshRss` row are amended to the
    refusal, with the old cell struck (§2c). The error came from the draft
    prompt's §0.5 row, "available (per refresh)", written from the command's
    name rather than its handler. That row and the WU3 adapter list are struck
    in `agents/prompts/11-watchdog.md`. The order also named v0.5-outline rows.
    **The tree has none:** `grep -i rss` finds 0 lines in
    `docs/v0.5-outline.md` (and 0 in `docs/v0.4-outline.md`), so nothing
    there is struck, and the draft was the only source of the error.
  - **One more WU1 defect**, found while writing WU7: `audit.ts`'s comment
    named `audit.test.ts` as the kind pin. No such file exists; the pin is in
    `automation.test.ts`. Fixed in `63e392f` (comment only).
- **C1, as it actually reads.** ~~C1 — the take contract~~ was the executor
  order's description (§2c; the continuation of 2026-09-27 corrected it). C1 is
  the **scope decision**, `agents/prompts/11-watchdog.md` §1 and §3, decided
  2026-09-25: **one prompt, two gated work units.**
  - *In:* (1) the automation engine runtime (§13, AC-25), including
    `autoFollow`; (2) the watchdog remainder that has a subject — §10.3's
    threshold question, and ladder rung 2 with its AC-27 item.
  - *Out, recorded:* ladder rungs 3–4 (no effect pipeline, no multiview
    output); GPU timing and the other §10.1 stubs (Prompt 12).
  - **Where the build stands.**
    - Unit 1: WU1–WU4 are landed and recorded, and WU7's latency is landed
      (`fb1ef14`). WU5 (self-trigger suppression, preflight cycle detection)
      runs last.
    - Unit 2: rung 2 is landed (WU6, `d455aa0`). §10.3's answer is WU5's.
    - The out-of-scope items stay out. `Rung` ends at `LoopsShed = 2`
      (`render.rs`), and ~~`telemetry.rs`, where `renderGpuTimeMs` is stubbed
      `0.0`, is unchanged since `619846c`~~ the §10.1 stub lines in
      `telemetry.rs` (`renderGpuTimeMs`, `vramUsedMib`,
      `textureCacheUsedMib`, `masterClockDriftMs`) are unchanged since
      `619846c`. *(§2c, on the two-key pass of 2026-09-29: the file itself
      changed at `4c6e176`, when WU8 added the watchdog counters and the
      `fallback_active()` read; the stubs did not, so the substance holds.)*
    - One prompt, one PR: compliant.
- **WU2's recall path, guarded** (`6b3ab85`; SPEC v0.4.7 row 4). WU2 fixed the
  hold so that any command engaging it cancels pending actions, including a
  `snapshot.recall` restoring a held snapshot. The fix was recorded, but no
  test drove that path; the B5 test engages the hold through
  `automation.hold` only. The guard makes a recall the first of three queued
  actions. Falsified at `6b3ab85` by a cancel keyed on `automation.hold`: it
  fails with `only the recall dispatched`, both pending markers dispatched
  while held, and the B5 test still passes. §13.5 gains the sentence it
  needed: hold is a state, not a command. The automation suite's CI floor
  rises 24 → 25.
- **WU7** (`fb1ef14`, corrections `f03796f`, `fb7619f` and `6ab9c77`): latency,
  measured. The last correction came from the gate run: the landing's test
  helper failed `clippy -D warnings` (`type_complexity`), and a type alias
  fixes it.
  - **The instrument.**
    `packages/control-plane/src/automation-latency.measure.ts`, run with
    `npm run measure:automation` and deliberately not part of `npm test`.
    - Tier cp runs every trigger kind through its real source on the
      production clock.
    - Tier engine runs `audioLevel` end to end through the release binary.
    - Every phase is counted two ways, and loads are printed at each tier's
      start and end. A tier at 3.0 or above is VOID.
    - The last line is `AUTOMATION: PASS|FAIL|VOID`. The soak runs it every
      iteration (`docs/soak-protocol.md` §1).
    - The wire gains `ts` on `audioLevelCrossing` (v0.4.7 row 1, amended),
      and the audit rows gain `queuedAt` and the hold's `heldAt`,
      `cancelledAt` and `latencyMs`.
  - **The numbers** (`docs/09-measurements.md`, WU7; quiescent, loads pasted).
    - Every kind is dispatched well within one frame of observation. The
      worst max is timer's 3.594 ms, which includes the `setTimeout` callback's
      own lateness.
    - The hold cancels at p99 0.009 ms.
    - **`audioLevel` from crossing to queue: p50 0.539, p99 0.761, max
      19.656 ms, 600/600 within one frame.** That is B1's acceptance.
  - **A defect in the instrument, found by its own check.** The first full
    engine-tier run put the control plane on the wall clock as
    `performance.timeOrigin + t`. Thirteen minutes into the process that sum
    was about 3 ms off, so every span came out negative, and the verdict still
    said PASS. `f03796f` recalibrates at a millisecond edge before every play
    and fails a negative span. Both verdicts are falsified: a 40 ms stall
    reads `OVER 1 FRAME`, and a skewed clock reads `DISAGREE: a span is
    negative`, each exiting 1.
  - **Finding for the user:** §10.7's command limiter (10 per burst, 5/s per
    connection per family) binds a rule's actions, since each rule runs on its
    own connection. A rule therefore sustains 5 actions/s in one family, below
    once per frame. It is recorded, not changed.
- **WU5** (`ed64942`; §10.3 pin `5538bdd`): self-triggers, run last.
  - **Preflight refuses a rule cycle by name** (§13.4, AC-25 #3). §13.4.1 is
    now data, `crates/nbe-core/src/automation_effects.json`, and
    `nbe_core::automation_effects` builds rule → rule edges from it.
    - The refusal reads ``automationRule: automation rules form a cycle: `a` →
      `b` → `a` (`a`: `view.take` changes `viewItem`; `b`: `record.start`
      changes `recordState`) — a rule's action can re-trigger itself (SPEC
      §13.4)``.
    - The check over-approximates: conditions are ignored, and disabled rules
      count.
    - **Deferred edges count.** This was WU5's decision, and a looping
      playlist is refused. Aliases resolve before the check.
  - **The runtime suppresses a self-trigger.** It checks whether the rule is in
    the event's cause chain, and audits `automation.suppressedSelfTrigger`.
  - **§13.4.1, row by row.** The runtime check executes all 55 commands
    against watchers. ~~It found **no missing edge**, since no command raised a
    trigger its row does not name.~~ *Struck (§2c): the two-key pass of
    2026-09-29 found two. `scene.arm` writes `previewItem` when the preview is
    empty, and `show.stop` writes `streamState` and `recordState`. The probe had
    taken neither path, and a probe sees only the path it takes. Fixed in PR
    #34's fix round (`6557ae6`), with a static reader of every handler.* 47 commands are accepted, and every row has
    one. The rows the tree contradicted are amended in place with citations
    (SPEC §13.4.1, "Checked against the tree"):
    - nine groups whose "yes" changes no field a rule can name;
    - `show.unload`, whose rules unload before the diff;
    - `audioLevel` and `streamHealth` cells marked deferred;
    - take and cut clearing `previewItem` and `fallbackActive`.
  - **Two tests changed** because their manifests became cycles. The
    recall-hold guard now triggers on A2's `mediaStart`, since its recall
    changes `previewItem`. "An action payload preflight cannot judge…" now
    triggers on a hotkey, because `mediaEnd` → `view.cut` is a deferred
    self-edge.
  - Falsified at `ed64942` (F1–F5) and `5538bdd` (F6), each restored with
    `git checkout`:

    | # | Mutation | Guard that failed | Signature |
    |---|---|---|---|
    | F1 | the runtime suppression removed | `the runtime suppresses a rule its own action re-triggered` | actual `automation.action`, expected `automation.suppressedSelfTrigger` |
    | F2 | preflight's cycle check removed | `a_rule_cycle_fails_preflight_with_the_cycle_named` | `… "preview.set" … must fail preflight; errors []` |
    | F3 | an edge dropped from the data (`preview.set` without `previewItem`) | all three readers: nbe-core's cycle-of-one test, preflight's cycle test, and the runtime row check | the row check: `triggers raised that the command's §13.4.1 row does not name — missing edges` + `'preview.set {"itemRef":"A1"} raised stateChange:previewItem'` |
    | F4 | deferred edges ignored | `a_linear_playlist_is_not_a_cycle_and_a_looping_one_is`; preflight's cycle test | `deferred edges count (WU5): a looping playlist re-triggers itself`; `… "program.cut" … must fail preflight; errors []` |
    | F5 | a command's row deleted (`marker.add`) | all three readers | `§16's 55 commands, each once` (left 54, right 55); `registered commands with no §13.4.1 row`; `marker.add has a §13.4.1 row` |
    | F6 | the watchdog threshold 2 → 1 | both §10.3 pins | `the tree does not slate a single frame missed by 1–2 frames; §10.3's words would`; `the slate engages on the third consecutive miss, not before (frame 1)` |

    Under F6, prompt04's older `a_late_view_frame_counts_and_trips_the_watchdog_via_the_loop`
    still passes. It asserts only that sustained misses trip, which is why the
    pins are needed.
  - **CI found the first pin too tight.** Run `36356479347` at `5a5cc55`
    failed the rust job with `late by more than one frame and at most two:
    293.22125ms`. The runner's first frame took about 143 ms to render on top
    of the 250 ms injected. The bound refused a scenario that had not
    happened, which is its job, but 50 ms of headroom was a wall bound
    tracking the machine (R9's class). `1fa7fe6` fixes it: both pins now
    render an on-time warm-up frame first, and the 1.5-budget case runs 750 ms
    against 300 ms, which leaves 150 ms of headroom. F6 was re-run at
    `1fa7fe6` and fails both pins again, with the same signatures.
  - **§10.3's threshold — the answer.** §10.3: "If the render loop misses a
    deadline by more than 1 frame, the watchdog MUST: 1. log fault, 2.
    increment fault counter, 3. activate fallback slate if the fault affects
    VIEW." The tree works differently (`render.rs`'s deadline arm;
    `watchdog.rs`). A late View frame reports `ceil(late / budget)` missed
    frames. Consecutive reports are summed, and an on-time frame resets the
    sum. A sum above 2 logs and raises the slate. **They disagree in both
    directions**, both pinned through the real render loop (`5538bdd`):
    - *The tree is laxer.* One frame late by 1.5 budgets reports 2, and 2 is
      not above 2, so there is no log and no slate. §10.3's words require
      both, for any single frame late by between 1 and 2 frames.
    - *The tree is stricter.* Three consecutive frames, each late by less than
      one frame, trip the slate, though none "misses a deadline by more than 1
      frame".
    - *"Increment fault counter" has no counter.* `droppedFramesTotal` counts
      every late View frame, whatever its lateness, which is a different
      quantity. The log is written only at the trip.
    - *And the slate never comes down*, as WU6 found: the engine only ever
      stores `fallback_active = true`.

    **A spec-revision question for the user, not a drive-by fix.** There are
    three options:
    - (a) Move the tree to §10.3's words, and slate any frame late by more than
      one frame. With a slate that never clears, one 70 ms hiccup at 30 fps
      would hold the slate for the rest of the show.
    - (b) Move §10.3 to the tree: slate when the lateness of consecutive late
      View frames, in frames rounded up, sums past 2. A single long hiccup
      stays off the slate, and §10.5's ladder acts first on sustained
      pressure.
    - (c) Keep the sum and add a single-frame bound.

    Whichever is chosen, two things are still owed: a watchdog fault counter,
    and a rule for releasing the slate (§10.3 names activation only). The
    executor recommends (b), with both of those.
    **Decided 2026-09-27: (b), plus both mechanisms**; built in WU8, below.
- **WU8** (`4c6e176`): the watchdog's recovery half, on the user's decision
  (b). SPEC v0.4.7 row 5 rewrites §10.3 to the accumulation as built, with
  both directions written and the old text struck (§2c). Its guards and
  falsifications (i)–(iv) are in the v0.4.7 block.
  - **The fault counter** is `watchdogTripsTotal`, and `watchdogClearsTotal`
    sits beside it. Each counts once per episode. Both ride the §10.1 tick,
    because §10.5's precedent for the ladder is the tick (`degradationRung`).
    The audit log (§10.7) belongs to the control plane, and the engine keeps
    none. The tree decided this.
  - **K = 30**, `WATCHDOG_CLEAR_AFTER_ON_TIME`. The trip accumulates over one
    unbroken late run, and recovery is the same accumulation over an unbroken
    on-time run. The clear threshold sits an order of magnitude above the
    trip, which is the hysteresis: the fastest possible cycle is 1 frame to
    trip and 30 to clear, and one on-time frame clears nothing. 30 is also the
    tree's existing "pressure has cleared": the ladder's
    `RESTORE_AFTER_ON_TIME` is 30, so the slate comes down on the frame the
    ladder stands down.
  - **The slate is held by source**: `fallback_sources` and
    `FallbackSource::{Held, Watchdog}`, changed with `fetch_or` and
    `fetch_and`. The recovery releases only the watchdog's own slate. Changing
    the field's type made all twenty stale readers fail to compile, so none
    could keep reading the old flag silently.
  - ~~**Open, for the user (out of WU8's scope, recorded rather than
    fixed).** The `Held` sources are still never released by the engine.
    After an operator's `view.fallback`, a take clears the control plane's
    `fallbackActive` (`state.ts` `take`), but the engine's `on_take` releases
    nothing. The View keeps the slate, and the engine's tick keeps reading
    `fallbackActive: true`. This is WU6's finding, now narrowed to the sources
    the watchdog does not own. The candidate fix is that the engine's take
    releases `Held`, as the control plane's does.~~ **FIXED in PR #34's fix
    round, `5942305`** (the user's word of 2026-09-27: fixed in this PR,
    before the two-key pass). `on_take` releases `Held` for both take and cut,
    and does not release the watchdog's slate. It is guarded by
    `prompt11_slate` (3) and `slate-release.e2e.ts` (2, the real engine
    binary), and falsified (i)–(iii) in SPEC v0.4.7 row 7. WU6's finding is
    now closed for every source: ~~each has exactly one release path~~.
    ~~*Recorded, not fixed: a resync whose snapshot says `fallbackActive: false`
    still does not release `Held`. A take missed during an outage is replayed
    as the snapshot's `viewItem`, but the slate stays until the next take or
    cut. The operator can recover it, and the tick shows it.*~~ **FIXED in PR
    #34's fix round, `a8678d8`**, and wider than recorded. The two-key pass of
    2026-09-29 found the control plane clearing `fallbackActive` in
    `loadPackage` and `unloadPackage` as well as in `take`, while the engine
    followed only the take. So view.fallback → show.stop → show.unload →
    show.load aired the next show under the old show's slate. The invariant is
    now release parity: the engine releases `Held` exactly where the control
    plane clears the flag — take and cut, `show.load`, and a resync reporting
    `false` (both branches pinned). The watchdog's slate keeps its one release,
    the recovery. Guards: `prompt11_slate` 3 → 7 and `slate-release.e2e.ts`
    2 → 4 (the pass's scenario, and a reload). Falsifications are in SPEC
    v0.4.7 row 7.
    - **`show.unload`, as the tree has it** (the tree wins over the order's
      wording on two points). The control plane forwards `show.unload`, but the
      engine has no route for it, so the directive is ignored. After an unload
      with no following load, the engine keeps rendering whatever its View
      last showed: the old show's item, or the slate if one was held. Nothing
      is taken off air. The tick keeps reporting the engine's `fallbackActive`
      while the control plane's state says `false`. With no package loaded
      there is no take to recover with. The next `show.load` releases the
      slate and clears the View, as does a resync reporting `false`.
    - **Queued (not this PR):** route `show.unload` to the engine, clearing its
      package, View, preview and `Held`, so an unloaded show goes off air.
      *(Cross-reference, 2026-10-09: SPEC v0.4.8 row 5's package
      reconciliation leaves the no-package snapshot to this item. A resync
      naming no package reconciles nothing; see the package-reconcile record,
      after the zombie-registration fix's.)*
- **The limiter exemption** (`0e3b69b`; seam fix `3270bc1`), on the user's
  decision of 2026-09-27. A rule's actions, and autoFollow's, dispatch without
  §10.7's per-connection command limiter; sessions keep it unchanged. The
  bounds on automation are once per frame per rule (§13.3 #3), preflight's
  cycle refusal (§13.4), and the runtime's self-trigger suppression. Nothing
  else was needed, because a rule cannot flood through any of the three:
  - it fires at most once per frame;
  - it cannot re-trigger itself in the same dispatch, which the runtime
    suppresses;
  - it cannot loop through deferred effects, which preflight refuses.

  SPEC §13.3 gains the sentence (v0.4.7 row 6), because the text implied the
  opposite: §13.1's "same preconditions as a human operator's commands" reads
  naturally as including the limiter. Falsified: with the limiter re-applied,
  20 of 30 are refused `E_RATE_LIMITED`. The test found a defect in WU2's test
  seam first (`nextFrame`'s float drift), fixed on its own in `3270bc1`.
- **The two-key pass of 2026-09-29, and PR #34's fix round.** Two blockers
  are fixed and recorded above and in SPEC v0.4.7 rows 7 and 8: the missing
  cycle edges (`6557ae6`, record `2d036a8`) and release parity (`a8678d8`,
  `990a22a`, record `1c14b4c`). The ride-alongs:
  - **A-1, floor arithmetic `2d6a9a4` omitted.** WU1's landing introduced two
    gates by name without their numbers: the automation suite at ran ≥ 6 /
    exercised ≥ 6 (WU1's six tests at that commit), and `automation_rules` at
    1/1 (one fixture test). The 6 appears in the next commit's arithmetic,
    `2eedec7`'s "CI floor 6 -> 8". History is not rewritten; this line carries
    the omission.
  - **A-2**, the truncated CI step names: quoted in `5496554`.
  - **B-5**, the §10.3 pins' stale message and comment: `bcaa54c`, which also
    corrects the take-payload comment in `prompt11_slate`.
  - **D**, the `telemetry.rs` sentence under C1: struck above (§2c).
- **The re-pass of 2026-09-29, and PR #34's last round.** Two text-or-guard
  findings, both touching text the merge ratifies:
  - **S-1** (`6bd5a11`): the static scan enumerated `commands/*.ts` instead of
    the dispatch table. It now reads `buildRegistry` and holds scanned ==
    registry == data. The re-pass's `marker.flag`, which passed 2/2 at
    `99586ee`, now fails naming it. v0.4.7 row 8 reads "every registered
    command".
  - **T-1** (this record): §10.3's release-parity paragraph said "exactly
    where the control plane clears", which was false for `unloadPackage`. It
    now says the operator's slate is released wherever the control plane's
    clear reaches the engine, at the engine's three release points (take/cut,
    `show.load`, a resync reporting `false`; a resync is a replay, not a
    clear site). It states the unload gap in the same paragraph and quotes the
    queue line above. v0.4.7 row 7's title and text say the same. Text only;
    no behaviour changed.
- **Flake sightings in this PR, each with a home (the register's rule).**
  - *CI run `36356479347`*: a bound authored in this PR (the §10.3 pin) did not
    survive a cold runner. CI caught it and `1fa7fe6` fixed it (WU5, above).
    It is not a register entry: there was no recurrence, and the cause is
    known.
  - *`a_slow_exit_inside_the_backstop_is_a_clean_stop`*, 2026-09-27: one local
    failure at load about 4.9, over the ceiling, whose message was not kept.
    It is entered in R11's register row (`docs/soak-protocol.md` §5) because
    its cause is not known.
  - The two abandoned WU7 starts are voided in `docs/09-measurements.md`, each
    with its reason: the executor's own CPU-bound `grep` (load 3.39), and a
    start at load 3.25, over the ceiling.

## 12 — Benchmark

**Reframed by H1.** The reference machine is Intel with discrete AMD graphics; ~~the spec declares Apple Silicon the primary target~~ — that was v0.3; SPEC v0.4 §0.1 assumption 2 names the Intel reference machine and says Apple Silicon "is welcome and supported, but MUST NOT be assumed" (corrected 2026-09-25, §2c). Every performance number to date — quality-profile capping, the 8 ms render budget, the degradation ladder's thresholds — is unvalidated on the declared target. 12 must state which architecture each measurement was taken on, and AC-5's 30-minute zero-drop soak must not be reported as met on an architecture the spec does not target. S1 is 12's problem too: `renderGpuTimeMs` is always 0, and it is the ladder's input, so the ladder is currently deciding on a constant. A benchmark prompt that inherits a stubbed GPU timer measures nothing.

## 13 — Operator shell

Inherits the display-surface deferral (04 → 09 → here in practice) and S2: **`showState` is absent from the §10.1 telemetry tick**, so a shell subscribing to telemetry alone cannot say whether the show is running. Either the shell also tracks `stateChange` frames, or v0.4 adds the field — the outline records it as a candidate. 13 is also the first consumer that will notice R2's metering strobe: bus meters sampled once a second from a 33 ms window are unusable in an operator UI, so R2's fix is a prerequisite rather than a nicety.

### The re-plan's P1, its recall leg — `snapshot.recall` reaches the engine (2026-10-03, branch `prompt13-recall-fix`)

**Where P1 is written, said first (the tree wins).** P1 comes from the Prompt 13
re-plan of 2026-10-02 ("Prompt 13 upgraded for the tree"). That document is
not in the tree yet: it was delivered read-only, pending the user's word, and
its branch `prompt13-upgrade` holds only PR #34's merge bookend. So P1's text
cannot be struck where it stands. It is quoted here, struck as fixed, and the
re-plan's own copy is struck when it lands. The user's word of 2026-10-03: the
recall leg is fixed now.

> ~~**P1** (recall leg) — `snapshot.recall` is forwarded (`forward: true`),
> has no engine route (`directive.rs` ignores it), and no resync follows.
> **`snapshot.recall` changes the CP's `viewItem`/overlays while the engine
> keeps airing the old View: an on-air split brain**, against §25.2 #2's "a
> snapshot restores what the audience sees".~~ **FIXED** by `a0e3e6d`
> (mechanism and guards) and `835fcbc` (SPEC v0.4.8 row 1: the §13.4.1 row,
> its two deferred edges in the data, and the preflight pin), with floors in
> `be9d278` and the end-to-end's report window widened on CI evidence in
> `302c0e3`.

**The routing decision.** The fix reuses the existing `snapshot.recall`
directive, now resolved. It adds no new directive and does not use `view.take`.

- The command was already forwarded and is in the command surface, so the
  frame shape is unchanged. What changes is that it carries resolved content
  (§5.9.1) instead of the snapshot's name. No new directive means no wire audit
  trio, and the mirror stays 18/18.
- `view.take` was ruled out for three reasons:
  - The engine's take always releases `Held`, and `recallSnapshot` never clears
    `fallbackActive`. A take directive would have made the recall a release
    site that the control plane does not have (§10.3's release parity).
  - A take cannot say "empty View".
  - A take cannot carry overlays.

The engine applies the recall through the take's own application
(`apply_take`): cut-class, the clip bus's source swapped, and the playback
generation moved on. It does not release `Held`. The control plane's half
changes too: a recalled item that changes the View starts now, as a take's
does, instead of at the snapshot's old start, which a later resync would replay
as a seek. The snapshot no longer saves a start frame.

**The tree against the order's wording, on four points.**

- **§13.4.1's row text.** The row did not read "sets viewItem wholesale, not via
  take". The no-engine-effect caveat was its sources cell, "`directive.rs`
  routing (not routed)", and that cell is now struck.
- **Overlays are fixed too.** The order said "the recall's resolved view". The
  tree's recall restores `visibleOverlays` wholesale, and §25.2 #2 counts
  overlay visibility as View state. Routing only the item would have left the
  same split brain for overlays, so the recall carries them.
- **There is no next-item pointer.** autoFollow advances from the item that ends
  (`order[indexOf(ended) + 1]`). So the autoFollow pin is the ended item, held
  two ways:
  - the engine drops the outgoing item's pending end;
  - the control plane's restored item states leave that item no longer
    PLAYING.
- **The test runner is not vitest.** The order's gate says "control-plane
  vitest". The package runs `node --test`, and that is what ran.

**Falsifications.** Each mutation was made on a saved copy and the file was
restored from that copy afterwards; the tree was clean after each. Loads are
the one-minute figure at each start; all were above the 3.0 ceiling. None of
these outcomes is a timing claim: each is a behaviour that happened or did not
(a pixel, a bit, a command, an event, an exit code). They are disclosed as runs
above the ceiling.

| # | Mutation | Load | What failed, with its signature |
|---|---|---|---|
| F1 | The engine's `"snapshot.recall"` route removed (the pre-fix engine) | 4.27 | `prompt13_recall` 1/6: the split-brain tuple `left: ([255, 0, 0, 255], false, false)` against `right: ([0, 0, 255, 255], false, false)`. The empty-View, media-arm, overlay (`[("ov1", true, Enter)]` against `[("ov2", true, Steady)]`) and parity (`left: None`, `right: Some("A2")`) tests failed with it. `recall.e2e.ts` 0/2: `controlPlaneView: 'C'`, `engineClipAudible: false` (expected `true`), in both tests |
| F2 | The recall releases `Held` (`apply_take(d, true)`) | 4.35 | `prompt13_recall` 5/6: `left: ([0, 0, 255, 255], false, false)` against `right: ([12, 200, 90, 255], true, true)`. `recall.e2e.ts` 1/2: `engineSlate: [false, false]` (expected `[true, true]`) with `controlPlaneSlate: true`, the new split brain the parity rule forbids |
| F3 | No media arm: the recall as a bare View store, resync-style | 3.90 | `prompt13_recall` 5/6: `the recall swaps the clip bus's source to A2 through the take's path, a cut; got []`. The split-brain pixel test **passed** under this mutation. It cannot tell a take from a store, which is why the media-arm test exists |
| F4 | The autoFollow pin's control-plane half: the recall restores the View but not the item states | 4.87 | `automation.test.ts` 29/31: `Q1's late end cannot advance the rundown past the snapshot` / `'Q2' !== 'P1'`. Collateral: the row check's precondition, `precondition: the preview is empty for scene.arm` / `'A1' !== null` |
| F5 | The autoFollow pin's engine half: the recall skips the playback generation | 4.54 | `prompt13_recall` 5/6: `the outgoing item's pending end is superseded by the recall's generation; got [AppliedStateVersion { … state_version: 3 }, ItemEvent { … item_ref: "A1", event: End, … }]` |
| F6 | The control plane forwards the recall raw (the pre-fix handler) | 4.64 | `automation.test.ts` 30/31: the directive's payload was `{ name: 'a1' }` with `target: {}`, against `{ transition: 'cut', audio: { transition: 'follow' }, visibleOverlays: [] }` with `target: { itemRef: 'A1' }` |
| F7 | The data: `snapshot.recall`'s two deferred edges removed | 3.74 | `nbe-preflight` `automation` 5/6: `… "snapshot.recall" … must fail preflight; errors []` / `left: 0` (preflight admitted `mediaEnd → snapshot.recall`) |

The release engine was rebuilt from the restored tree after F1 and F2.

**Found while landing it.** These are recorded, not fixed.

- ~~**The engine reads a take's transition length as the item's duration.**~~
  **FIXED** by `990b8ed` (SPEC v0.4.8 row 2, `57d7490`; branch
  `take-duration-fix`, 2026-10-05); see the take-duration fix's record below.
  As found: `schedule_done`, the only source of `itemEvent end`, runs only when the take
  payload carries `durationFrames`. That field is the transition's
  (`resolveTransition`). Two consequences follow:
  - a cut, which carries none, never schedules an end;
  - a mix take of a timed item schedules its end at the mix's length, so the
    control plane would mark the item DONE, fire `mediaEnd`, and advance
    autoFollow about half a second in.

  The midpoint report's "nothing on the wire moves when a clip is exhausted"
  covered the first half and not the second. ~~A recall's resolved cut carries no
  `durationFrames`, so it inherits only the first.~~ *(§2c, v0.4.8 row 2: a
  recall now carries the recalled item's own `itemDurationFrames`, and the item
  ends at it.)*

  **The intent, recorded (the user's word of 2026-10-03).** The engine fix is
  its own PR, after this merge. That fix is what makes the ratified take/cut
  `mediaEnd` cell ("a timed item's end is scheduled for its duration") true.
  ~~Until it lands, a recall inherits the defect through `apply_take`: its cut
  schedules no end.~~ **Kept (2026-10-05):** the fix landed as its own PR
  (`990b8ed`), and the take/cut cell is now true as written, unamended.
- ~~**§13.4.1's `view.take`/`view.cut` `mediaEnd` cell overclaims.** It says "a
  timed item's end is scheduled for its duration". For a cut, nothing is
  scheduled. The error is in the safe direction for a cycle check. The row is
  ratified, so changing it is the user's word; it is not changed here. The new
  recall cell states the same mechanism exactly.~~ *(§2c, 2026-10-05: it no
  longer overclaims. Since v0.4.8 row 2 the cell is true as written. It was
  the tree, not the cell, that was wrong.)*
- **Audio under the operator's slate.** `view.fallback` changes the picture,
  not the clip bus, so an item's audio continues beneath the slate. That
  predates this fix. A recall under the slate follows it: the recalled item's
  audio plays beneath the slate, and `recall.e2e.ts` uses that as its witness.
  The spec says nothing about audio under the slate.

  **Queued as a spec question (2026-10-03): does a slate mute?** The spec is
  silent. It is the user's word when it matters.
- **PR #34's merge bookend overstated the dress rehearsal.** `0461261`, on
  `prompt13-upgrade`, said "control-plane, dress rehearsal and rust all
  green" for run `36658852056`. The job's conclusion is success, but the job is
  advisory (`continue-on-error`, the gate split of 2026-09-17), and inside it
  14 of 16 passed. These two failed:
  - `not ok 7 - [RI-1] step 6: a soundboard stab raises the sfx bus`;
  - `not ok 12 - [RI-1] gate`.

  The v0.4.6 bookend copied the same phrase for run `36214380771`, which
  passed 15 of 16 (`not ok 12`). That one is corrected in place ~~below~~
  above, in the v0.4.6 entry under § 11 (§2c: it sits above this section, not
  below). PR #34's bookend is corrected on its own branch (`0af6f0f` on
  `prompt13-upgrade`).

  **The process rule is now written** (the user's word of 2026-10-03):
  `docs/implementation-standards.md` §2b, "A job's conclusion is not its
  tests". A green job over failing tests is a red run. The chronic red these
  bookends missed is Finding R12, under § 07.
- **The dress rehearsal's audio-rise steps are intermittent on CI, late
  rather than absent.**
  - Step 4 (the clip bus) failed on PR #34's head run `36567491896`.
  - Step 6 (sfx) failed at the merge, `36658852056`.
  - Step 6 passed in the first run and step 4 in the second.

  The step-4 failure timed out at its 3000 ms wait with the risen value
  (−20.8 dBFS) in the last tick, with 82 underruns. The job is advisory, so
  nothing went red. `recall.e2e.ts`, which uses the same witness in a gating
  job, waits 10 s (`302c0e3`). The steps' 3000 ms bound is the open
  question, and it is the rehearsal's to settle. *Filed 2026-10-03 as Finding
  R13, under § 07. The disposition rides R12's work order.*

**After this record: the toolchain rename, `8265292`.** This is its own row.
It landed in this PR so that the gate's CI could go green.

- **What happened.** On run `37170937500`, CI's `dtolnay/rust-toolchain@stable`
  resolved to rustc 1.99.0 (2026-09-28). That release deprecates
  `AtomicU32::fetch_update` ("renamed to `try_update` for consistency"), so
  clippy's `-D warnings` failed the rust job at `crates/nbe-engine/src/video.rs:71`.
  The site is `SessionLease::drop`, untouched since `74cfd04`.
- **The fix.** `8265292` renames that call, which is the only one in the
  workspace. CI at `8265292`: run `37171465949`.
- **The security-review flag, answered.** A background review flagged
  `try_update` as a single-shot compare-and-swap that would leak the session
  count under contention. It is not. In the installed std source
  (`library/core/src/sync/atomic.rs`, rustc 1.98.1):
  - `fetch_update` is now only a forwarder,
    `self.try_update(set_order, fetch_order, f)`, marked
    `#[deprecated(since = "1.99.0", note = "renamed to `try_update` for consistency")]`;
  - `try_update` is the retry loop:
    `while let Some(next) = f(prev) { match self.compare_exchange_weak(prev, next, set_order, fetch_order) { … Err(next_prev) => prev = next_prev } }`.
    It returns `Err(prev)` only when the closure returns `None`, and it has been
    stable since 1.95.0.

  Same method, same retry loop: behaviour is unchanged.

**The two-key pass (2026-10-03), and what it settled.** The pass had an amended
precondition, the user's word: CI's dress rehearsal is read per test, and the
mission gate's evidence is the local rehearsal.

- **CI.** All 16 tests ran and 15 passed. Test 12 failed in the chronic shape:
  zero-drop, `1 !== 0` (Finding R12).
- **Local, at `8265292`.** The rehearsal ran quiescent (load 2.46 → 2.94) and
  passed 16 of 16, with no `# SKIP`. Across 50 ticks, `droppedFramesTotal`
  stayed at 0, `audioUnderrunsTotal` at 0, and the slate never went to air.
- **The user's words of 2026-10-03:**
  - Keep the refusal consequence: `mediaEnd → snapshot.recall` and
    `audioLevel → snapshot.recall` are refused at load, the `mediaEnd` edge
    over-approximated as its cell says.
  - Close the pass's four record gaps before the merge. This commit does so.

**Queued (from the pass, section D): a static reader for the deferred cells.**
The table's "no missing edge" claim has no mechanism for deferred cells:
- the static scan reads only `stateChange` cells;
- the runtime row check reads only same-dispatch triggers;
- the preflight pins cover only the rows they name.

That is where the `stateChange` cells stood before S-1. The reader goes over the
engine's directive routes (`directive.rs`) and compares each routed command's
reachable mechanisms with its deferred cells:

| Reachable mechanism | Deferred cell |
|---|---|
| a push to `audio_commands`, or the clock starting or stopping | `audioLevel` |
| `playing.begin` or `schedule_done` | `mediaEnd` |
| a stream session | `streamHealth` |
| the clock starting | `timer` |

**Its first case: `show.start` and `show.stop` against `audioLevel`.** The DRESS
record says no clock means no audio, and neither row lists the edge. This is
unconfirmed. For this PR's own row the guard is sufficient: both new edges are
pinned at load and falsified (F7).

**The merge's bounded acceptance (the user's word of 2026-10-03).** CI at this
PR's head is red under §2b ("a green job over failing tests is a red run"). It
is red by two named classes, and the merge accepts exactly those two:

- **R12's gate.** Test 12, `not ok 12 - [RI-1] gate: …`, fails its zero-drop or
  underrun conjunct. Its counts are inside the backstops: drops ≤ 20,
  underruns ≤ 2000, watchdog trips ≤ 10.
- **R13's late rise.** Test 5 or test 7 (step 4 or step 6) finds the clip or
  sfx bus rising after the 3000 ms wait. The rise is inside the 10 s backstop.

Each class is 16 of 16 on the normative machine, run quiescent. At `8265292`,
the last code change, the local rehearsal passed 16 of 16 with 0 drops and 0
underruns, and the slate never went to air (load 2.46 → 2.94). Every commit
since is docs-only.

**What blocks the merge:** a sighting outside its backstops, or a new signature
(any other failing test).

~~**The R12+R13 dress-leg work order is the top item of the queue after this
merge.**~~ *Done: PR #36 (2026-10-04) implements it (Findings R12, R13).* **The
top item of the queue is now the deferred-edge static reader** (the queue line
above). The tree keeps no standing board, so this paragraph is where that order
is recorded.

**The queue keeps P1's other legs for the shell work.** `preview.set` and
`scene.arm` still have no engine route, so the preview bus is stale until a
resync. The snapshot's preview half stays control-plane-only, and the engine
says so in `on_recall`'s doc. These remain prerequisites before any shell work
that shows a preview pane. `preview.set` is the WU6 finding above,
recorded as out of scope then and queued now.

**Merged** as PR #35 on 2026-10-04 at 05:20 UTC, squashed to `d7b96f7` on
`main`. Its tree, `c7153a3`, is identical to the PR's final head, `f1a8279`.

**Main's CI at the merge commit, run `37179591329`, read per test (§2b):**
- rust and control-plane are green;
- the dress rehearsal passed 14 of 16 (`# fail 2`).

The two failures:
- **`not ok 7 - [RI-1] step 6`** is R13's late rise. The sfx bus rose after
  about 3986 ms.
- **`not ok 12 - [RI-1] gate`** is R12's underrun conjunct, `170 !== 0`. At the
  gate the counts were 0 / 170 / 0 / 0; the whole-run maxima were drops 1,
  underruns 259, trips 1.

**The run is red under §2b**, by exactly the two classes the bounded acceptance
names, each inside its backstops. Recorded 2026-10-04 as the bookend of PR #36,
the work order those two classes queued.

### The take-duration fix — a timed item ends at its own duration (2026-10-05, branch `take-duration-fix`)

**The defect** was recorded with PR #35 (above, struck to FIXED).
`schedule_done`, the engine's only source of `itemEvent end`, ran on the take
payload's `durationFrames`, and that field is the transition's length. The
effects:
- A cut never ended a timed item, so autoFollow never advanced after a cut.
- A mix ended a timed item at the mix's length, so autoFollow ran away.
- A recall inherited both.

**The fix.** The user's word of 2026-10-04: the engine fix makes the ratified
take/cut `mediaEnd` cell true, keeping the intent line in PR #35's record.
- The take payload carries the item's duration as its own field,
  `itemDurationFrames`. The transition keeps §16.2's own `durationFrames`.
- In Rust the two are `transition_duration_frames` and `item_duration_frames`
  (`nbe_protocol::TakePayload`), and the engine reads every take through that
  typed model.
- The control plane resolves the item's duration from the package in one
  helper, `takePayload`. It parses the payload through
  `TakeDirectivePayloadSchema` before view.take, view.cut and snapshot.recall
  send it.
- The landing is `990b8ed`, the spec row is `57d7490`, and the CI floors are
  `3159592`.

**The tree, against the order, on three points.**
- **There is no end at end of file.** `schedule_done` is the only
  `ItemEvent::End`, and the engine has no end-of-file signal. So "untimed
  clips still end at EOF" is false: an untimed item never ends. The pin is
  "no end": an untimed item never ends, and a transition's length never ends
  it. A timed item ends exactly once.
- **The take/cut `mediaEnd` cell needed no amendment.** It reads "a timed
  item's `end` is scheduled for its duration", and it never named the field.
  It is verified true as written. Only the `snapshot.recall` row's cell named
  the field, and it is amended (§2c).
- **Tests encoded the defect.** prompt03's four cut takes sent the item's
  duration as `durationFrames`, and so did prompt13_recall's `take()` helper.
  After the fix:
  - `item_end_emitted_after_timed_duration` failed;
  - the stopped-show guard and the superseded-take guard would have passed
    **vacuously**, because nothing was scheduled at all;
  - prompt13_recall's superseded-end checks would have passed vacuously too.

  They are moved to `itemDurationFrames`, with the reason in place (§2c).

**New behaviour, stated.** A take payload that does not read is now refused
(`DirectiveError::Invalid`). It used to be read field by field with silent
fallbacks. The control plane is the only producer, and it parses its own
payloads first.

**Falsified** on the normative machine. Each mutation was restored from a
saved copy **and both engine binaries were rebuilt** (the lesson from PR #36).
Every run started under the ceiling (2.26–2.49).

| # | Mutation | Signature |
|---|---|---|
| M1 | Control plane: the cut's item duration dropped | automation `not ok 32` (the `itemDurationFrames: 60` lines missing). e2e 0 of 2: `autoFollow must advance T1 -> T2 at T1's end; the View is still T1`, and the same for M1 |
| M2 | Engine: the old read (`schedule_done` on the transition's field), the pre-fix signature | `take_duration` 3 of 8. The cut: `a cut to a 6-frame item ends it at 200 ms`, `left: 0`. The mix: `no end at 300 ms …`, `left: 1`. The recall: `left: 0`. Untimed: `left: (0, 1)`. e2e: T1 never advances; `at 1000 ms M1 is still on air … 'M2' !== 'M1'` (ran away at the mix's 200 ms) |
| M3 | Engine: `is_current` removed | `(A's ends, B's ends) … left: (1, 1) right: (0, 1)` |
| M4 | Engine: the end scheduled twice | `exactly one end for one take of a timed item`, `left: 2`, and four more tests at `left: 2` |
| M5 | Engine: the recall's share reverted (schedule only on a take) | `the recalled 6-frame item ends at 200 ms`, `left: 0` |
| M6 | Control plane: the recall's share reverted | automation `not ok 32`: the recall's `itemDurationFrames: 60` missing |
| M7 | The trio, TypeScript: `itemDurationFrames` dropped from the schema | mirror: ``TypeScript's take payload has no `itemDurationFrames` field``; protocol.test: `Unrecognized key(s) in object: 'itemDurationFrames'`; automation 22 of 32, because the producer's own parse now refuses its timed payloads: `'error' !== 'ok'` |
| M8 | The trio, Rust: the item's wire key renamed | mirror 18 of 20 (`left: {…"itemDuration"…}`; the fixture round trip loses the field). `take_duration` 1 of 8: the engine no longer reads the control plane's key |
| M9 | Engine: a payload that does not read is half-applied (default on error) | `a payload whose itemDurationFrames is not a number is refused: ()` |

**Observed, not a finding.** The dress rehearsal's final segment keeps A1, a
150-frame item, on air for about 9 ticks while the record, sync and kill steps
run. A1's end now fires there; before the fix that cut never ended. No step
reads A1's state, and the View holds an ended clip, so the rehearsal is 27 of
27 with no expectation shifted (quiescent, load 2.48 → 2.58).

**The consequence, recorded by the PR #37 two-key pass (2026-10-05): autoFollow
never advances after an untimed item.**
- autoFollow fires only on a `PLAYING → DONE` edge (`server.ts`, the
  `itemEvent` branch). The `end` that makes that edge comes only from
  `schedule_done`.
- An untimed item goes `LIVE`, never `PLAYING` (`state.ts` `take`). It carries
  no `itemDurationFrames`, so nothing schedules its end.
- So `autoFollow: true` on an untimed item is inert, silently. The manifest
  schema accepts it (`Item` in `schemas/manifest.v0.4.json`), and preflight
  says nothing about it.

This is not new with the fix. Before it, only a mix could send an `end` for an
untimed item, and `markDone` ignores a `LIVE` item, so autoFollow never fired
then either. What the fix settles is that no mechanism exists: there is no end
at end of file (above).

It falls short of §3.1's definition of the flag: "Per-item flag to advance
automatically when media ends". An untimed clip's media does end, because a
clip plays once (`video.rs`). Nothing tells the control plane when it has.

**Queued as a spec question (2026-10-05): should an end at end of file
exist?** That is, should the engine emit `itemEvent end` when an untimed
clip's media is exhausted, so that autoFollow and `mediaEnd` rules work for
untimed clips? ~~Or should autoFollow on an untimed item be refused or warned
at preflight? It is the user's word.~~

**Decided in part (the user's word of 2026-10-06, §2c):** preflight refuses
`autoFollow` on an untimed item. It lands as its own small PR, with a spec
note when it is built. **Still queued for the user:** whether the engine emits
an end at end of file.

### PR #37's fix-forward — the take invariant, the strictness completed, the pass's corrections (2026-10-06, branch `take-duration-fix`)

**The defect, P1 of PR #37's two-key pass, introduced by this PR.**
`view.take { transition: "mix", durationFrames: 0 }` is spec-legal (§16.2:
`durationFrames` 0–600). `resolveTransition`'s audio-follows-video rule copied
the 0 into `audio.durationFrames`, whose minimum is 1, and the new take
payload's parse refused it. The parse ran **after** `state.take`. So the
control plane's View moved, the engine received nothing, and the client saw
`E_ENGINE`. That is the split-brain class the recall fix removed.

**The invariant (the user's word of 2026-10-06): no command path mutates state
before its directive validates.** The order is build → validate → mutate →
send. It is written on `takePayload` (`commands/view.ts`), which builds and
validates the directive, and it is applied on all three take-class paths:
- **view.take** builds its directive before `state.take`.
- **view.cut** builds it before the arm, which is a write too, and the take.
- **snapshot.recall** builds it from the snapshot itself (`requireSnapshot`,
  read only) before `recallSnapshot` writes anything. It used to be built from
  the state after the recall.

The tree, said first: these three are the only paths whose directive
validates. `overlay.show`/`overlay.hide` and `show.stop` send payloads that are
empty or already validated as commands, so they have no validation step to
order.

**The 0 made honest.** A 0-frame mix resolves as a cut: no transition length,
so no `audio.durationFrames` to copy. The command schema is unchanged; the
resolution is where the 0 becomes honest.

**The strictness completed.** `TakePayload` gains `deny_unknown_fields`, as
`Envelope` and `DirectiveFrame` already have it. So does `TakeAudio`: the order
named `TakePayload`, and the TypeScript schema's audio object is strict too, so
the mirror is strict on both sides at both levels. The shared fixture gains two
invalid cases: a misspelt item duration (M8's class) and an unknown key inside
`audio`. §5.9.1's note now says an unknown key does not read.

**Guards** (the counts move: `npm test` 125 → 129, `take_duration` 8 → 9, the
take-duration end-to-end 2 → 3, the fixture's invalid cases 3 → 5, the mirror
unchanged at 20):
- `src/take-payload.test.ts` (4, in `npm test`, through dispatch):
  - a 0-frame mix is a cut: accepted, and the control plane and its
    directive agree;
  - the invariant, one test per path. Each forces an unreadable payload with an
    in-test item of duration 0, which the manifest schema forbids, and asserts
    the refusal is the payload parse's own and that every field a take-class
    command writes, plus the bridge, is untouched.
- `take-duration.e2e.ts` +1, on the real engine: a 0-frame mix to a timed
  autoFollow item. The control plane says Z1, and the engine acks the take's
  stateVersion: it acks only what it applied. Z1 then ends at its own 30
  frames, and autoFollow advances once.
- `take_duration.rs` +1: an unknown key is refused on the take, the cut and
  the recall, naming the key, with the View unmoved and nothing scheduled.

**Falsified** on the normative machine. Each mutation was restored from a
saved copy, checked identical, and **both engine binaries rebuilt**. Every run
started at load 2.27–2.46.

| # | Mutation | Signature |
|---|---|---|
| (i) | The whole control-plane fix reverted to `9473da3` (the pre-fix tree) | unit 0 of 4. The 0-frame mix: `outcome: 'refused: [ { "code": "too_small", "minimum": 1 …'`, `viewItem: 'A'`, `a: 'PLAYING'`, `bumps: 0`, `directives: []`, which is the pass's probe signature. Each invariant test: the control plane moved (`viewItem: 'Z'`, `Z: 'PLAYING'`). End to end, test 3: `"status":"error"`, `"code":"E_ENGINE"`, path `audio.durationFrames`, `"controlPlaneView":"Z1"` |
| (ii) | The copy of the 0 restored; parse-before-mutate kept | unit 3 of 4. The 0-frame mix is refused **before any mutation**: `viewItem: null`, `a: 'ARMED'`, `bumps: 0`, `directives: []`. End to end: `E_ENGINE` with `"controlPlaneView":"M2"`, the View it had: the control plane did not move |
| (iii-a) | view.take mutates before it validates | its invariant test only: `viewItem: 'Z'` for `'B'`, `Z: 'PLAYING'` for `'ARMED'`, `B: 'READY'` for `'LIVE'`, `previewItem: null` for `'Z'` |
| (iii-b) | view.cut arms and takes before it validates | its test only: `viewItem: 'Z'` for `'B'`, `Z: 'PLAYING'`, `B: 'READY'` for `'LIVE'` |
| (iii-c) | snapshot.recall recalls before it validates | its test only: `viewItem: 'Z'`, `Z: 'PLAYING'`, `automationHold: true` for `false`, `visibleOverlays: ['bug']` for `[]` |
| (iv-a) | `TakePayload` lenient again | `take_duration` 8 of 9: `a payload with an unknown key is refused: ()`. Mirror 19 of 20: `"an unknown key: a misspelt item duration (the two-key pass's M8 class)": must be refused` |
| (iv-b) | `TakeAudio` lenient again | mirror 19 of 20: `"an unknown key in the audio object": must be refused` |
| (iv-c) | M8 re-run with the strictness in place (the Rust key renamed) | **now loud**: `take_duration` 2 of 9, every timed take refused by name, e.g. ``Invalid("view.take: the take payload does not read: unknown field `itemDurationFrames`, expected one of …")``. At `b8af9ff` the same mutation failed only by its effects |

**The pass's corrections (§2c each).**
- §13.4.1's `snapshot.recall` row: "over-approximates in two ways only" is
  struck. It is three ways, because a recall to an empty View
  (`clear_view`) schedules no end either.
- §5.9.1's note, amended before ratification: the end is counted in wall-clock
  time from the point the engine applies the directive, and the item airs at
  `master+1`, up to one frame later. The dress rehearsal's A1 ended 5.00 s
  after its take.
- `prompt03.rs`: "these four tests" is corrected to three tests (four takes).
- `dress-rehearsal.test.ts`: the 12 fps comment named the transition's field;
  it now says the item's duration.
- `protocol.test.ts`: the comment that called the engine lenient on an unknown
  key is corrected.

**Recorded alongside.** R11's post-fix sightings 3 and 4 and the sharper
tripwire are in the register row (`docs/soak-protocol.md` §5) and under Finding
R11 above. The autoFollow decision is recorded above.

~~**Queued (its own PR, after this one): the resync-end gap.**~~ **FIXED** by
`299038b` (SPEC v0.4.8 row 3, `acd5857`; branch `resync-end-fix`; the record
is below). `on_resync` never
schedules a timed item's remaining end. It sets the View and its start frame,
but it neither begins a playback generation nor calls `schedule_done`, and the
resync snapshot carries no item duration to schedule from (`state.ts`
`resyncSnapshot`). So an engine restart strands a timed item on air: it never
ends, and autoFollow stops there. ~~A reconnect without a restart keeps the
original pending end, because `on_resync` leaves the generation alone.~~ *(§2c,
v0.4.8 row 3: a resync that carries `viewItemEnd` now supersedes the pending
end with its own, at the same time; one that carries none still leaves the
generation alone.)* Found by PR #37's two-key pass; recorded on the user's word
of 2026-10-06.

**The gate, run twice (§2c: the first run is void).**
- **The first run, from 02:00, was partly on battery.** The charger came out
  mid-run (the first battery tag in the power log is 02:21:33), and at 02:27:39
  the machine idle-slept for 491 s. The slate-release end-to-end's test 4 ran
  486 s and failed its precondition. That run is void, and it is kept aside
  with its logs. It opened Finding R14.
- **The re-run, on AC under `caffeinate -i -s`** (02:38:10 to 03:01:32; AC at
  both ends; no sleep or battery event in the window):
  - workspace **489/0/3 across 55 binaries**, which is 488 + 1, the new
    `take_duration` test (main `dc312a6`: 478/0/3 across 54);
  - `npm test` 129/129, mirror 20/20, `take_duration` 9/9, prompt03 10/10,
    prompt13_recall 6/6;
  - the end-to-ends: take-duration 3/3, recall 2/2, slate-release 4/4;
  - the local dress 27/27 at 0 / 0 / 0 / 0;
  - tsc, fmt and clippy clean.

  Every run started under 2.5. `npm test` ended at 3.03, over the 3.0
  ceiling. The run is power-limited (§2a): the adapter is 45 W, and the gauge
  read a discharge in the final build.

**CI at `7fdd0ca`, run `37447312366`, read per test (§2b).**
- **Attempt 1 is red.** Dress 27/27 and control-plane green. The rust job's
  workspace step was all green (55 binaries), but its "GPU/render tests
  actually ran" step re-ran prompt04 and exited 101, **printing nothing**:
  the step's `out=$(cargo test …)` under `bash -e` exited before the echo.
  prompt04 had passed 17/17 in the workspace step two minutes earlier. The
  failing test is unknown, so it is entered as R15
  (`docs/soak-protocol.md` §5).
- **Attempt 2 (the failed job re-run) is green per test.** 142 `test result:`
  lines, all ok, with prompt04 17, `take_duration` 9, mirror 20 and
  prompt13_recall 6.
- **The swallowing is fixed** (the user's word of 2026-10-06, `23dcc9d`). All
  28 capture steps keep the exit code, echo the output, then fail naming the
  suite. Falsified on the extracted steps under `bash -e`:
  - with a forced failing test, the old steps printed 0 lines and the new
    ones printed the failure;
  - the 09 loop records a failing suite and goes on.

**Stated, not changed:** a take payload the control plane's parse refuses still
reaches the client as `E_ENGINE`, because `server.ts` maps any error that is not
a `CpError` to it, though the engine was never contacted. After this fix no
accepted command reaches that path; the invariant tests force it.

### The resync-end fix — a restarted engine still ends the timed item (2026-10-07, branch `resync-end-fix`)

**The defect** was recorded by PR #37's two-key pass and queued by the user's
word of 2026-10-06. `on_resync` re-applied the show's state: the View, the
overlays, and the slate's engage and release. It never scheduled the on-air
timed item's remaining end, and the resync snapshot carried no duration. So
an engine restart stranded a timed item on air: no `itemEvent end` ever fired,
and autoFollow stopped dead mid-rundown. It is the resync half of the family
that PR #35 closed for recall.

**The fix.** The landing is `299038b`, the floors `113a6a1`, the spec row
`acd5857`.
- **The wire.** The `show.resync` snapshot gains `viewItemEnd: { itemRef,
  remainingFrames }`, carried with its audit trio:
  `nbe_protocol::ResyncViewItemEnd` (`deny_unknown_fields`),
  `ResyncViewItemEndSchema` (strict), and the shared fixture
  `resync_view_item_end.json` (2 valid, the clamped zero among them; 6
  invalid).
  - It is present only while the on-air item is timed and `PLAYING`. An item
    that already ended is not ended again, and an untimed one never ends.
- **The remaining time is computed by the control plane** (`state.ts`
  `viewItemEnd`), because the engine has no package and its tick names no
  `viewItem`. It is the item's `itemDurationFrames` (v0.4.8 row 2) less the
  wall-clock time since the control plane's own take (`viewItemTakenAtMs`,
  new, set by a take and by a recall that changes the View), rounded **up** to
  a whole frame, so rounding never ends it early, and clamped to
  `[0, duration]`.
- **Its error bound: good to about a frame.** The control plane's take
  precedes the engine's apply by a local hop. The engine counts the original
  end from its apply, and the item airs at `master+1`, up to one frame after.
  Rounding up adds less than a frame. A wall clock stepped backwards cannot
  lengthen the item past its duration.

  *Refined by the two-key pass (the user's word of 2026-10-08):* **the
  rescheduled end is not strictly late-only.**
  - A healthy engine fires the end at `t_take + h + D`, where `h` is the
    take's local hop, from the control plane's stamp to the engine's apply.
  - The resync's end lands at `t_take + D + h' + q`, where `h'` is the
    resync's hop and `q` is the rounding, `0 ≤ q <` one frame.
  - The difference is `h' − h + q`. Rounding is late-only, but the end as a
    whole can come early by the take's local hop when the resync's hop is
    shorter. Both hops are local, so it stays well under two frames, `master+1`
    included.

  The spec's "good to about a frame" stands accurate.
- **The clamp's semantics.** Ends are counted in wall-clock time from the
  apply, so an item that should have ended during the outage ends **at resync
  time**: its remaining time is zero, the end fires on receipt, and autoFollow
  advances. The end arrives late by the outage, on purpose. The audit says
  what happened: `sendResync` records `resync.viewItemEnd` with the item,
  `remainingFrames`, `durationFrames`, `elapsedMs` and `overdueMs`.
- **The engine.** `on_resync` reads `viewItemEnd` **before** it applies
  anything. One that does not read, or that names another item than
  `viewItem`, refuses the resync, as an unreadable take payload refuses a
  take. After re-applying the View, it schedules the end through the take's
  own machinery: `playing.begin` supersedes any end still pending, so a resync
  on a healthy engine leaves exactly one, at the right time. Without an end,
  the generation is left alone, so a resync that carries none (untimed, done,
  or from a control plane older than this row) cannot cancel a pending end.

**The tree, against the order, on three points.**
- **§13.4.1 needs no amendment.** `show.resync` is directive-only and has no
  row. The end it restores is the `view.take`/`view.cut` row's own deferred
  `mediaEnd` edge ("a timed item's `end` is scheduled for its duration"),
  which an engine restart used to sever. No row's edge set changes. The change
  still rides as v0.4.8 row 3, because §5.9.4's snapshot gains a field and a
  sentence.
- **The reconnect suite has no floor step.** `reconnect.rs` is two wire-level
  tests inside the workspace run, and it is untouched. The engine's pins went
  to a new handler-level suite, `resync_end.rs` (7), with its own floor.
- **The sibling sentence is not yet true.** The order's sentence is "resync's
  application is now complete for everything §13.4.1 says a take does". Its
  siblings are the recall leg (#35), the slate parity (#34), and now the end.
  Read cell by cell against the take/cut row:
  - **stateChange.** The engine's half, releasing the operator's slate, is
    reconciled by a resync since #34.
  - **mediaEnd.** A deferred end, which cancels the previous pending end, is
    re-established by this row.
  - **Other triggers, `audioLevel`, deferred: "the take swaps the clip bus's
    source".** **This cell still distinguishes resync from take.**
    `on_resync` never touches the audio graph, so after an engine restart the
    on-air item's clip bus stays silent.

  So the sentence would be true for the slate and the end, and false for the
  clip bus. It is recorded as false, with the cell named.

~~**Observed, not changed (each the user's word):**~~ **Queued** (the user's
word of 2026-10-08, §2c: these two were recorded as observed). The prompt map
keeps no current queue list (its queue sections, after 07 and after Prompt 09,
are historical), so the order is stated here, absolutely (the user's word of
2026-10-08, §2c: it was relative, and "Behind it" came to point at a struck
item):
- ~~**At the top of the queue after this merge: the zombie registration.**~~
   **FIXED** by `94dec15` (SPEC v0.4.8 row 4, `955b43f`; branch
   `zombie-resync-fix`; the record is below).
   `sendResync` runs before the connection handler installs the socket's
   listeners (`server.ts`: `register`, then `sendResync`, then the `message`
   and `close` handlers). Falsification F3 showed the shape:
   - the strict parse throws inside the connection handler;
   - the engine is registered but never resynced, so §5.9.4 makes it hold and
     apply nothing;
   - the control plane never processes its frames, and never notices the
     engine go.

   The clamp makes it unreachable from the end's computation, but any throw
   while the snapshot is built does the same. **The fix direction:** install
   the listeners before `sendResync`, or deregister the session on a throw.
- ~~**Position 1 after PR #39's merge:**~~ ~~**In flight, PR #40 (SPEC v0.4.8
  row 5): the package-reload gap, a design question.**~~ **FIXED** by
  `b961cc5` (PR #40, SPEC v0.4.8 row 5; the package-reconcile record is
  below). *(It read ~~"Behind
  it:"~~, §2c. §2c, 2026-10-10: it left the numbered queue when PR #40 took it
  up, and it is struck FIXED by the merge SHA after the user's merge. The
  numbered positions below are the user's word of 2026-10-09, ruling on PR
  #40's fresh two-key pass.)* The control
  plane sends `packagePath` in the snapshot, but `on_resync` never reads it;
  only `show.load` does (`directive.rs`), and §5.9.4's field list does not
  name it. A restarted engine rejoins with the View, the overlays and now the
  end, but without the show's media: it cannot render the item it was told
  is on air, which compounds the clip-bus cell above. What a restarted engine
  should reload ("package, assets, overlays") is a resync-semantics decision
  for the user.
- ~~**Position 1: R11's exit-path measurement**~~ (the user's word of 2026-10-09,
  ordering the queue; *the label struck as the earlier strikes are, §2c, 2026-10-14, the user's word on the fresh pass*). ~~*(In flight: PR #41, resolved by measurement under the
  user's ruling (b) of 2026-10-11; it is struck FIXED by PR #41's merge SHA
  after the user's merge.)*~~ **FIXED** by `a6fc4d8` (PR #41; R11 resolved by
  measurement; the record is under Finding R11). R11 reopened on 2026-10-09, on post-fix sighting 5
  (Finding R11 above, and the register row in `docs/soak-protocol.md` §5).
  The work is the one queued on 2026-10-06: measure the stream thread's exit
  path, the run loop's return and the encoders' drop (VideoToolbox's
  invalidation is the suspect), against the 1100 ms backstop.
- ~~**Position 2: `show.stop`'s `stopVersion` across the grace wait**~~
  **FIXED** by `95583c5` (PR #43, the (C) fix; SPEC v0.4.8 row 6; struck in
  the next PR, the standing pattern of §2c). *(moved
  from position 4 and amended, the user's word of 2026-10-09; §2c,
  2026-10-10)* (the user's word of 2026-10-09, from PR #40's two-key pass;
  older than that branch). `show.stop` reads `ctx.stateVersion` as
  `stopVersion`, emits its stop directives at it, then awaits `waitForGrace`
  for up to 2 s before the dispatcher's bump (`commands/show.ts`). A command
  on another connection that bumps during the wait is assigned that same
  version, so two commands' directives carry one `stateVersion`, and the grace
  wait can be met by the other command's acknowledgement. It is the shape of
  the package generation race, which `afterBump` fixed for `show.load` (the
  package-reconcile record's delta, below). *Read while writing this line, not
  run:* with outputs active, `show.stop` already sends `record.stop`,
  `stream.stop` and its own `show.stop` at one `stateVersion`, and the
  engine's gate skips a directive whose version is not above the last it
  applied (`channel.rs` `classify`: `<=`). ~~Whether the second and third are
  skipped is the first thing to establish when this item is built.~~
  *(§2c, 2026-10-10, the user's word on the fresh pass's read, which confirms
  it as a read:)*
  - **What the read says will happen.** With outputs active, the gate always
    skips `show.stop`'s own directive, so the engine's own stop (its clock)
    never runs from it. If `record.stop` applies, `stream.stop` is skipped too.
    If `record.stop` fails (nothing recording), `stream.stop` applies.
  - **Each skip leaves a gap.** A skipped directive's sequence number is never
    consumed (`classify` returns before advancing `expected_seq`), so the next
    directive meets a sequence gap. The engine sends `resyncRequest`, and the
    control plane resyncs it (rule 5).
  - **The resync does not stop outputs.** `on_resync` has no output handling,
    and the snapshot carries no record or stream state. So, read and not run,
    a skipped `stream.stop` leaves the stream running after `show.stop`.
  - **The interaction with row 5.** Every output-active stop therefore ends in
    a resync, when the next directive arrives, and that resync runs the
    package reconciliation.
    - *The tree, on "therefore in a package reload":* the resync reloads only
      when the identity has moved by then. A resync at the held identity loads
      nothing (G3; the two-key pass's same-identity reconnects gave reloads
      `[0, 0, 0, 0, 0]`).
    - The stop-load-start workflow does move it. The next directive is then
      the forwarded `show.load`, which meets the gap and is dropped. So the
      load runs inside the resync's reconciliation instead of as `show.load`:
      about 1.27 s on the dress package (`package-reconcile.measure.ts`). That
      is this PR's latency story, moved onto the stop path.
    - Before row 5, that dropped `show.load` was never applied at all.
  - **Establish first.** The item keeps its establish-first character: its
    first step is the test that shows the skips on the real gate.
  - **The scope is a design choice, the user's word when the item is built:**
    one `stateVersion` per emitted directive (the dispatcher bumping for
    each), or a gate that accepts equal versions from one command.

  **Established, 2026-10-12** (the user's word of 2026-10-12;
  establish-first, NO FIX). The read's "will" is measured fact on all four
  legs. The harness is `packages/control-plane/src/show-stop-version.e2e.ts`:
  the real engine binary on the real gate and real sockets, an RTMP sink
  whose byte count is the on-air observable, and the dress package with a
  record directory. It ran on the normative machine, every leg exercised.

  | Leg | Outputs | At the stop (one `stopVersion`, S) | Until the next directive | The next directive (`view.fallback`) | After the resync |
  |---|---|---|---|---|---|
  | E1 | record + stream | `record.stop` applied (the ack is S). `stream.stop` and `show.stop` skipped: `stale stateVersion; skipping sv=7` twice | the engine's clock runs. The stream stays on air: the sink's bytes grow, and the engine reports `streamTransportState: live`, while the control plane reports `streamState: idle`. The recording stops (its file stops growing) | `seq gap; requesting resync … seq=9`, then `show.resync applied sv=8` | the clock stops. **The stream is still on air** (`live`, bytes growing). No package reload |
  | E2 | stream | `record.stop` fails (`record.stop requires an active recording`). `stream.stop` applied (the ack is S). `show.stop` skipped: one `stale` line | the clock runs; the stream is off air (`closed`) | gap, then resync | the clock stops |
  | E3 | record | `record.stop` applied. `stream.stop` and `show.stop` skipped: two `stale` lines | the clock runs; the recording stops | gap, then resync | the clock stops |
  | E4 (control) | neither | `show.stop` applied (the ack is S); nothing skipped | the clock is stopped | accepted: no gap, no resync | — |

  Each leg was green three times in a row (17:47, 17:49, 17:51) and again
  after the falsifications (18:12). All runs were on AC, power-limited.

  **The mechanics, confirmed:**
  - **A skip needs an earlier directive at the same version to succeed.**
    E2's failed `record.stop` let `stream.stop` through. Mutation C showed
    that any success at the version skips everything after it.
  - **With an output active, `show.stop`'s own directive is always
    skipped.** So the engine's own stop (its clock) runs only at the next
    resync, and that resync comes only when another directive arrives.
  - **The resync touches no output.** E1's stream is still on air after it.
    The operator's `show.stop` leaves the stream live, and the control plane
    reports it idle.
  - **Row 5 on the E1 path.** The resync's package reconciliation ran at an
    unchanged identity and loaded nothing (no `the package was reloaded`
    line). The resync restored the clock (stopped) and the slate (the next
    directive was `view.fallback`), not the outputs.
  - **Not exercised here:** this line's first concern, another connection
    bumping during the 2 s wait. The matrix sends nothing concurrently; it
    stands as read. *(Exercised since, by the (C) fix's concurrent leg and its
    falsification F2, 2026-10-14; the record is below.)*

  *Known limit of the clock observables (§2c, 2026-10-14, the user's word on the fresh pass, no test change):
  `clockStoppedAfterResync`, and E4's `clockRanAfterStop: false`, would also
  pass on a stale engine report, which reads frame 0. E1's `live` comes only
  from a fresh report.*

  **Falsified.** Each mutation was restored from saved copies, checked
  identical, and the engine rebuilt; green after:

  | # | Mutation | Result |
  |---|---|---|
  | A | each emitted directive stamped with its own `stateVersion` (the dispatcher bumps per emission) | E1–E3 fail and E4 passes. E1: `skippedAtStop: 0`, `clockRanAfterStop: false`, no gap, `streamOnAirAfterResync: false`. ~~The stream was still on air in the 2 s after `show.stop`'s response, because the engine applies `record.stop`'s finish before `stream.stop` (its directive loop is sequential)~~ *(§2c, 2026-10-14, the user's word on the fresh pass: NOT MEASURED. The fresh pass's own run under A read `streamOnAirAfterStop: false`. Whether bytes arrive in that window is a race between the control plane's reply and the engine closing the stream. Under A there is no resync, so "after the resync" meant only the window after `view.fallback`. The substance stands: A inverts the skips, with `skippedAtStop: 0`, the clock stopped and no gap.)* |
  | B | the gate accepts an equal version (`classify`: `<` for `<=`, ungrouped) | E1–E3 fail and E4 passes. E1: `skippedAtStop: 0`, the clock stopped, `streamOnAirAfterStop: false`, no gap |
  | C | for the control: a directive that succeeds at `stopVersion` before `show.stop`'s own | all four fail. E4: `skippedAtStop: 1`, `clockRanAfterStop: true`, a gap and a resync |

  E4 passes A and B by design: it is the control, and they change nothing on
  its path. C shows that it catches a skip of the show's own stop.

  **The design question, restated with the confirmed mechanics.** It is the
  user's word; nothing is recommended beyond what the evidence supports.
  - **(A) One `stateVersion` per emitted directive** (the dispatcher bumps
    per emission). It breaks `dispatch.ts`'s "exactly ONE stateVersion bump
    for the accepted command", and the stop's single acknowledged version
    (§5.9.5 step 2; the grace waits on the first emission's version).
    Mutation A made every skip vanish. ~~The engine still applies
    `record.stop`'s finish before `stream.stop`, so the stream outlives the
    record finish.~~ *(§2c, 2026-10-14, the user's word on the fresh pass: not measured, and the pass's run under A
    contradicts it; see the falsification table.)* It also collides with
    §5.9.1: "Every directive emitted for one command carries that command's
    version", beside §5.9.5 step 1. So any fix that re-stamps directives
    answers to two spec mandates, not one. *(§2c, 2026-10-14, the user's word on the fresh pass.)*
  - **(B) A gate that accepts equal versions from one command.** It needs
    grouping on the wire: an ungrouped gate cannot tell one command's
    directives from a replay at the same version. It touches the invariant
    the reconnect family leans on: after a resync at version X, the gate
    drops everything at or below X, because the snapshot already covers it,
    and an equal version would apply what the snapshot already includes.
    Mutation B, ungrouped, made every skip vanish.
  - **(C) What the evidence adds.**
    - The engine's own `show.stop` already quiesces live outputs
      (`on_show_stop_inner`'s `(true, false)` arm: the record finish, the
      stream close, the ack after), so one `show.stop` directive could carry
      the whole stop.
    - But §5.9.5 step 1 mandates separate `record.stop` / `stream.stop`
      directives "at the stop command's `stateVersion`".
    - The gate's stale rule is in no spec text: it is the engine's `classify`.
      So the spec itself mandates several directives at one version, and the
      implementation's gate drops all but the first that succeeds.
    - (C) would change §5.9.5.
    - §16.1, steps 1–2, already says that when `show.stop` is received "the
      engine MUST issue an internal `record.stop`" if recording is active,
      and an internal `stream.stop` if streaming is active. That is the very
      path the gate skips. Letting the engine's own stop quiesce the outputs
      therefore matches §16.1 as written, rather than requiring its
      amendment. *(§2c, 2026-10-14, the user's word on the fresh pass.)* The ruling remains the user's.

  **Merged: PR #42 is `067e709`** (the establishing tests; the bookend, the
  standing pattern of §2c). Main's CI at `067e709`, run `38546704827`, read per
  test (§2b): rust 511 passed, 0 failed, 4 ignored, with all 30 floors green;
  npm 139; the show.stop-version e2e ran 4, exercised 1 (E4, the control) and
  skipped 3 on the encoder refusal; the dress rehearsal 27/27, with
  DRESS-COUNTS (sha `067e709`) at 0 drops and 196 underruns. No queue line
  closes with #42: position 2 stays open until the fix merges.

  **The fix: (C), one directive carries the stop** (the user's ruling of
  2026-10-14). (B) is rejected outright, and (A) is the fallback, fired
  only if a prove-first item below finds a gap that cannot be honestly
  closed.
  - The control plane sends only the forwarded `show.stop`; the
    `record.stop` / `stream.stop` bundle is deleted.
  - The engine's `on_show_stop_inner` quiesce arm is the stop.

  **Prove-first, answered from the tree at `067e709` before the code:**
  - **(a) Quiesce completeness: complete.**
    - *The record half is the standalone `record.stop`.* It runs
      `stop_and_finish(RECORD_STOP_TIMEOUT)` (1500 ms, `record/thread.rs`)
      and sets `Idle`. It clears the markers on `Ok`, keeps them on
      `Timeout` (the take is force-abandoned with its file kept), and clears
      them on other errors.
    - *The marker sidecar is written on this path.* `writer.finish()` is "the
      single place files finalize", and it writes `<stem>.markers.json` from
      `markers::list()` (`writer.rs:919`). `stop_and_finish` awaits that
      finish, and the caller's `markers::clear()` runs only after it.
    - *The stream half is the standalone `stream.stop`.* It is
      `stop_and_close()` (R11's 1005 ms thread backstop, then the transport's
      800 ms), then `Idle` with the selection and tap cleared.
    - *`force` and the `quiesceOutputs=false` refusal are unchanged.* The
      `(_, true)` arm abandons both outputs (no finish, markers cleared, a
      warning). The `(false, false)` arm refuses.
    - *One difference, the test seam only.* A session-less record stop
      clears the markers here and keeps them in standalone `record.stop`.
    - *Worst case per phase.* The record half takes at most 1500 ms on the
      blocking pool. It runs concurrently with the stream half, at most
      1005 + 800 = 1805 ms. Stopping the clock, releasing the sessions and
      clearing the transition take microseconds. So the engine acknowledges
      within about 1805 ms, 195 ms inside §16.1's 2 s.
    - *A stale comment.* The arm's own comment ("1500 ms record + 800 ms
      stream = 2.3 s") predates R11's backstop and undercounts the stream
      half. It is corrected, comment only.
  - **(b) What the grace window waits for under (C).**
    - *The tree has no countdown.* The order named `grace-window.telemetry.ts`;
      no such file, and no telemetry countdown, exists in the tree.
    - *What the window is.* It is `waitForGrace` (`server.ts`), awaited in
      `show.stop`'s handler *before* the dispatcher forwards `show.stop`. It
      resolves on any acknowledgement at or above the version (`noteApplied`).
    - *What it gates under (C).* With the bundle gone, the window gates the
      stop's response and the control plane's belief about the outputs. It
      waits for `show.stop`'s own acknowledgement, so it moves after the
      forward: it cannot wait for a directive not yet sent.
    - *The §16.1 arithmetic.* The engine needs at most about 1805 ms. The
      control plane waits 2000 ms from the forward. The stop is graceful if
      it is acknowledged inside that window.
  - **(c) The version: satisfied by construction for the directive; the wait
    is fixed.**
    - Under (C) the handler sends nothing itself. The only directive is the
      dispatcher's forward, at the bumped version.
    - The wait uses that version through a new `HandlerOutput.afterForward(sv)`,
      which the dispatcher awaits after the forward: bump, send, wait, with no
      await between the bump and the send.
    - The wait is for the *exact* acknowledgement of that version, not "at
      or above". The engine acknowledges each applied directive at its own
      version, in order, so a later command's acknowledgement cannot stand
      in for a failed stop's.
  - **(d) The control plane learns from the acknowledgement** (the design,
    recorded before the code). The handler sets `showState: STOPPED` before
    the bump, as today, and leaves `recordState` / `streamState` alone. Then
    `afterForward` waits up to 2000 ms for the exact acknowledgement.
    - **Acknowledged.** The engine sends it only once every active output has
      quiesced; the arm withholds it on the first failure. The outputs go to
      `idle`.
    - **Not acknowledged** (a timeout, an acknowledgement withheld after a
      failed quiesce, or no render node). The control plane surfaces
      `SHOW_STOP_FORCE_WARNING`, as today.
      - The outputs are **held at their prior state** (`recording` / `live`),
        not `idle`: the control plane cannot know. "Held" names that state.
      - ~~The operator sees the warning, and~~ The tick's `streamState: live`
        sits beside the engine's own `streamTransportState`, which is not
        live once the engine has dropped the session. *(§2c, 2026-10-14, the
        fresh pass on PR #43: the warning reaches `deps.warn` only, which is
        `console.warn` in production; `HandlerOutput.warnings` is never read.
        So only someone watching the control plane's console sees it. The
        operator surface is queued under R17, position 5.)*
      - What heals it: the operator's explicit `record.stop` / `stream.stop`.
        The control plane marks the output `idle` and forwards; the engine
        refuses harmlessly when it is already idle.
      - Reconciling from the engine's report is R17's queued item. A late
        acknowledgement, after the window, is not acted on; that is named and
        queued with R17.
    - **`force`.** The engine abandons the outputs (no finish) and
      acknowledges. The control plane waits for the same exact
      acknowledgement, with the force warning: acknowledged, the outputs go
      `idle`; not acknowledged, they are held.
    - **No active output.** Forward only, no wait, as today.
  - **(e) `on_resync`'s output blindness.**
    - Under (C) the engine never acknowledges a stop whose quiesce failed,
      so the control plane holds the outputs per (d).
    - A later resync carries no output state and changes none. The engine
      has already taken and dropped (abandoned) the failed session, so its
      outputs are idle.
    - The tick shows the control plane's `streamState: live` beside the
      engine's own transport state, which is not live.
    - It heals by the explicit stops, or by R17's reconciliation. Output
      state on the resync is out of scope here and is queued in R17's
      territory.
  - **One finding from the read.** `show.stop` was the only command that
    sent more than one directive at one version. `view.take`, `view.cut`,
    `overlay.show`, `overlay.hide` and `snapshot.recall` are each
    `forward: false` with exactly one extra directive. Under (C) every
    command sends exactly one, so the stale gate cannot drop a sibling
    anywhere in the tree.
  - **Not in the tree.** The order names a "severest-missing-slate"
    `show.stop` leg at 2947–2949; no such test or line exists. ~~Every
    `show.stop` caller found is updated: `render-channel.test.ts`,
    `dispatch.test.ts`, `automation.test.ts`, the e2e files and the dress
    rehearsal.~~ Every `show.stop` caller found was checked. Those whose stop
    has an active output are updated: `render-channel.test.ts`,
    `dispatch.test.ts`, `automation.test.ts` and `show-stop-version.e2e.ts`.
    The rest stop with no output active (`slate-release.e2e.ts`,
    `package-reconcile.e2e.ts`, the dress rehearsal's step 10, after step 9's
    own `stream.stop`) or only read the payload (`protocol.test.ts`,
    `effects-static.test.ts`), so they are unchanged, and their suites are in
    the gate. *(§2c, 2026-10-14: written before the code, it claimed updates
    the code did not need.)*

  **The code (2026-10-14), as (d) designed it.**
  - **`commands/show.ts`.** The bundle is deleted. The handler validates as
    before and sets `showState: STOPPED`. With an active output it returns
    `afterForward(stopVersion)`, which waits on `waitForAck(2000,
    stopVersion)`.
    - Acknowledged: `recordState` and `streamState` go `idle`.
    - Not acknowledged: they are held, and ~~`SHOW_STOP_FORCE_WARNING`~~
      `SHOW_STOP_HELD_WARNING` is logged *(§2c, the PR #43 delta: the one text
      split in two, below)*.
    - `force` logs it either way.
    - With no active output: forward only, no wait.
  - **`dispatch.ts`.** `HandlerOutput.afterForward` is awaited after the
    forward: bump, send, wait. `DispatchDeps.waitForAck` replaces
    `waitForGrace` and `emitDirectivesNow`; both are struck in its doc comment
    (§2c). `show.stop` was `emitDirectivesNow`'s only user, and it was the
    defect's mechanism.
  - **`server.ts`.** An exact waiter. `noteApplied` keeps each render
    session's last 64 acknowledged versions, and `waitForAck` resolves only on
    its own. `awaitApplied` (the dress rehearsal's) keeps "at or above".
    - **Found in review, before the push:** the exact waiter would also have
      taken a `show.resync`'s acknowledgement. During the wait the control
      plane's version is the stop's, so a resync sent then, on a reconnect or
      after a gap on the stop itself, is stamped with the stop's version.
      Its acknowledgement says nothing about the stop, because the resync
      carries no output state. A still-running engine that never applied the
      stop would have been marked idle: the defect's own class.
    - Now each session notes the versions it was sent a resync at, and an
      acknowledgement of one of those never meets an exact wait. Two unit
      guards cover it, a reconnect inside the window and a same-connection
      resync request (F5).
  - **The engine.** No behavior change. The quiesce arm's comment read "1500
    ms record + 800 ms stream = 2.3 s", written before R11's backstop. It now
    reads 1500 + 1805 = 3.3 s in sequence, max(1500, 1805) = 1805 ms
    concurrent, with the old text struck (§2c). The release binary was
    rebuilt for it once, before the first run, and no mutation touched it.
  - **The wire.** Its shape is unchanged. `show.stop`'s acknowledgement is
    `appliedStateVersion { v, kind, stateVersion }`, and its outcome is
    binary: acknowledged means every active output quiesced (or, under
    `force`, was abandoned); withheld means a quiesce failed.
  - **The spec.** v0.4.8 row 6: §5.9.5 rewritten, the old text struck; §5.9.6
    new, the stale-version gate as the reconnect invariant; §5.9.1 and §16.1
    stand as written. No wire, schema or audit shape changed, so no audit trio
    is touched.

  **The evidence, on the normative machine (2026-10-14; on AC, 45 W adapter,
  Low Power Mode on).** `show-stop-version.e2e.ts` now runs the real engine
  behind a tap, a WebSocket relay that records the render channel both ways
  (`show-stop.harness.ts`). The guards read the wire itself: which directives
  the stop put on it, at which versions, and which the engine acknowledged.
  Six legs, 6/6 green, every leg exercised:

  | Leg | On the wire from the stop | Skips / gaps | The stream | The control plane | The take |
  |---|---|---|---|---|---|
  | E1 record + stream | `show.stop` at S, then the next directive at S+1; acks S, S+1 | 0 / 0 | off air at the stop (the sink's bytes stop); the engine reports `closed` | `recording`/`live` when the stop crossed the wire, `idle` after the ack | finalized: the sidecar holds `ssv-open`, `ssv-mid` in frame order; ffprobe reads 2 streams |
  | E2 stream | the same | 0 / 0 | off air, `closed` | `live` at send, `idle` after | — |
  | E3 record | the same | 0 / 0 | — | `recording` at send, `idle` after | finalized, with the sidecar |
  | E4 control | the same | 0 / 0 | — | no wait (no output) | — |
  | concurrent | `show.stop` at S; the second operator's `view.fallback`, sent the moment the stop crossed the wire, at S+1; acks S, S+1 | 0 / 0 | off air, `closed` | `idle` after the ack | finalized, with the sidecar |
  | held | `show.stop` at S fails: `E_DISK: marker sidecar: Permission denied` (the record directory made read-only); no ack of S | 0 / 0 | off air, `closed`: the stream half succeeded | **held** `recording`/`live`, with the warning; the tick shows `streamState: live` beside the engine's `closed`. The operator's `record.stop` / `stream.stop` heal it to `idle`; the engine, already idle, refuses both (`E_FORBIDDEN_STATE`), with no gap | ~~not finalized: no sidecar~~ finalized: ffprobe reads 2 streams, 2.005 s. Only the marker-sidecar write failed (`E_DISK … Permission denied`), so no sidecar *(§2c, 2026-10-14, the fresh pass on PR #43; my gate's held leg reads the same 2.005333 s)* |

  **The guards, each falsified.** Each mutation was restored from a saved
  copy and checked identical with `cmp`. The control plane runs from source
  under `tsx`, so a restore needs no build beyond `tsc --noEmit`, and each
  restore re-ran green.

  | # | Guard | Mutation | The failing half (quoted) | Restored |
  |---|---|---|---|---|
  | F1 | E1–E3 inverted | the bundle back: `record.stop` and `stream.stop` at `ctx.stateVersion` before the forward, three stops at one version | E1 fails: `directivesFromStop: [["record.stop",9],["stream.stop",9],["show.stop",9],["view.fallback",11],["show.resync",11]]`, `skippedFromStop: 2`, `seqGapsFromStop: 1`, `streamOnAirAfterStop: true`, `engineStreamTransportAfterStop: "live"`, `controlPlaneStreamState: "idle"`. E4 stays green | 6/6 |
  | F2 | concurrent | the pre-bump version: `show.stop` sent at `ctx.stateVersion` and waited on before the dispatcher's bump, the pre-(C) shape | fails: `directivesFromStop: [["show.stop",9],["view.fallback",9],["show.stop",10],["show.resync",10]]`, `stale stateVersion; skipping sv=9`, a gap and a resync. **The second operator's slate was dropped** and came back only through the resync | 6/6 |
  | F3 | ack-learning | the outputs marked `idle` at send | unit (`render-channel` + `dispatch`): 7 of 36 fail, every `show.stop` test with an active output (`between the send and the ack the outputs are held, not idle`; `not acknowledged: the control plane cannot know, so it holds`; `held: a later ack is not the stop's outcome`; …). E2e: E1 reads `controlPlaneAtSend: {"record":"idle","stream":"idle"}`; **the held leg reports `idle` under a failed finalize** | unit 36/36; e2e 6/6 |
  | F4 | exact ack | `waitForAck` as "at or above" | 3 of 36 fail: `a later command's acknowledgement does not stand in for the stop's own` (`the stop was never acknowledged: warned`), and both resync guards below | 36/36 |
  | F5 | a resync's ack is not the stop's | `noteApplied`'s `ownAck` forced true (a resync's acknowledgement counts) | 2 of 7 `show.stop` tests fail: `a reconnect's resync at the stop's own version does not stand in…` and `a resync requested on the same connection inside the window does not stand in either` (`the stop itself was never acknowledged`: no warning was logged) | 7/7 |

  F2 also answers this line's first concern, which the establishing matrix
  never exercised: under the pre-(C) shape, a command from another
  connection during the wait takes the stop's version and is skipped.

  **Finding: `stateChange` frames can arrive out of version order.** In the
  concurrent leg the operator socket received `stateChange` versions `[1, …,
  8, 10, 9]`: the second command's frame (S+1) before the stop's (S). The
  stop's response and frame wait for the acknowledgement, and the second
  command does not. Under F2's pre-(C) shape the order was `[…, 9, 10]`.
  - §5.4.1 rule 2 still holds: each frame carries its own command's response
    version, no later than that response.
  - Each frame carries the snapshot as it is sent. So S+1's frame shows the
    outputs still held, and S's, later, shows them idle.
  - A client that discards a frame below the newest version it has seen
    would keep the held outputs until `system.status`.
  - Recorded in §5.9.5 item 3; nothing else is changed. ~~Whether clients need
    a rule is put to the user.~~ **Ruled 2026-10-14 (the user's word on PR
    #43's fresh pass): no new rule. Clients must not assume version-ordered
    frames across connections.** Each frame carries its own command's
    version (§5.4.1 rule 2) and its send-time snapshot. §5.9.5 item 3 now
    states the client side as a MUST NOT.

  **Timing** (`show-stop.measure.ts`, at `a249c51`, 2026-10-14
  10:43–10:52 UTC). n = 20, 0 void, every sample a clean stop, each one E1
  on a fresh control plane and a fresh engine. Each sample started below a
  1-minute load of 2.5 and read under 3.0 at both ends (1.85–2.88). Power: on
  AC, 45 W adapter, Low Power Mode on (power-limited, not void), battery 100%
  and charged at both ends, run under `caffeinate`. The pmset log shows no
  sleep, wake or battery event in the window. The tap's relay hop is in every
  number.

  | Span (ms, nearest rank) | Budget, from the code | Median | p95 | Max | Min |
  |---|---|---:|---:|---:|---:|
  | wire → `stream quiesced` (the engine's log) | 1005 thread backstop + 800 transport = 1805 (R11's constants, `record/stream.rs`) | 41.4 | 53.7 | 55.8 | 33.4 |
  | wire → `recording quiesced` (concurrent) | 1500 (`RECORD_STOP_TIMEOUT`, `record/thread.rs`) | 75.9 | 88.9 | 91.4 | 61.9 |
  | wire → the ack crossing back | max(1500, 1805) = 1805, then the clock stop and release (microseconds) | 82.9 | 94.8 | 98.8 | 68.9 |
  | **stop → ack at the control plane** | §16.1's 2000 window | **84.1** | 95.8 | **100.4** | 70.3 |
  | stop → response | the ack, plus the response | 84.6 | 96.3 | 101.3 | 70.9 |

  The worst stop→ack, 100.4 ms, is inside the 2000 ms window by 1899.6 ms.
  The worst case by construction stays 1805 ms, 195 ms inside, which is
  R11's arithmetic. The record finish is the longer half in every sample;
  the stream close is about half of it. The control plane's own share
  (stop→ack less wire→ack) is about 1.2 ms: its send and the tap's two
  relay hops. A run started before the resync fix (4 samples, at the
  pre-fix tree) was stopped and kept, not counted.

  **CI.** CI is read per test at the pushed head (§2b), and the run is quoted in
  the PR and the report. On CI's runner the engine refuses `record.start` /
  `stream.start` (no hardware H.264 encoder), so the e2e exercises E4 alone
  and skips the other five loudly; its floors are ran ≥ 6, exercised ≥ 1. The
  run at the merge is recorded with the merge bookend in the next PR, per the
  standing pattern.

  **The fresh pass's delta** (the user's ruling of 2026-10-14 on PR #43's
  two-key pass: five corrections and the dispatch pin; docs and one test,
  no engine change). The fresh pass found the mechanism sound: no path to a
  false idle, and F1–F5 reproduced at the committed head. Its corrections:
  1. **§5.9.6 rule 4 forbade what the tree does.** `sendResync` stamps a
     `show.resync` with the current version (`server.ts`), and the render
     node applies a resync ahead of the gate. So `show.stop` at S, then
     `show.resync` at S, on one connection, is the design: unit test 26
     drives it. Rule 4 is struck and rewritten.
     - It now governs commands: a command emits at most one directive.
     - `show.resync` is carved out by name. It is not a command's
       directive; it is the recovery channel, and it runs ahead of the gate.
     - **Two emitters, named.** The command dispatcher sends at most one
       directive per command. The resync path (`server.ts` `sendResync`) sends
       `show.resync` on a connect and on rule 5's request. ~~"Only the
       dispatcher sends directives"~~ (the build report's claim) was false.
  2. **The dispatch pin** (new, code). `dispatch.ts` step 6b counts the
     command's directives, its forward plus its extras, and refuses more
     than one with `E_ENGINE`, before the bump and before any send.
     - The guard is `dispatch.test.ts`: "a command emits at most one
       directive: a forward plus an extra, or two extras, is refused before
       the bump and the send". It registers both bad shapes and asserts the
       refusal, no bump and an empty wire, then that the two legal shapes
       still pass.
     - No handler in the tree trips it. One that did would be a programming
       error: its state mutations stand unbumped, refused loudly rather than
       dropped silently by the render node.
     - **Falsified.** The guard was disabled (`if (false && directiveCount >
       1)`). The pin failed: `Missing expected rejection (refused):
       automation.hold: refused`. Restored from the copy, `cmp`-identical,
       `tsc` clean, green.
  3. **The ordering ruling** is recorded above and in §5.9.5 item 3.
  4. **The held row** is corrected above. The take finalized; only its
     sidecar failed.
  5. **The warning.**
     - *Text.* `SHOW_STOP_FORCE_WARNING` said "graceful output shutdown
       exceeded 2 s; force-stopping outputs". That was Prompt 02c's exact
       string, and since row 6 the control plane holds, not force-stops. One
       text named two outcomes, so each now has its own (`dispatch.ts`):
       - `SHOW_STOP_HELD_WARNING`: not acknowledged within the window, the
         outputs held;
       - `SHOW_STOP_FORCE_WARNING`, its text corrected: `force`,
         acknowledged, the outputs abandoned.
     - It is not wire-visible: `deps.warn` only, not in a frame, response or
       audit row. So no §13.4.1 cell reads it, and §13.4.1 is unchanged. It
       lives in TypeScript; no engine rebuild was needed.
     - *Reach.* Recorded, not fixed: console-only (see (d) above). The
       operator surface is queued under R17 (position 5).
  6. **Provenance of the falsifications.** Which tree each run used:
     - **F1, F2 and the e2e half of F3** (03:31–03:33 PDT): `show.ts`
       identical to `a249c51`'s, with `server.ts` from before the resync fix.
       So was their restored 6/6 at 03:35.
     - **F3 and F4's unit halves and F5** (03:42): files `cmp`-identical to
       `a249c51`'s, minutes before that commit (03:43:30).
     - **The fresh pass re-ran all five at `6f067ca`**, with matching values:
       F1's E1 wire list exactly, F3 7/36, F4 3/36, F5 2/36.
     - **The local gate's load.**
       - The first workspace run ended at 3.14 (over the ceiling).
       - The rerun read 1.72 at the start and 1.91 at the end, with 5 of 22
         samples at or above 3.0 (peak 3.42) as the suite starts its
         parallel tests.
       - No timing claim rests on either. Their pass counts (511/0/4, twice)
         stand.
       - The timing harness voids a sample at 3.0, and starts each below
         2.5. My run's per-sample loads were 1.85–2.88, and the fresh pass's
         timing run peaked at 2.69 over 40 samples.
       - The fresh pass voided its own workspace run as a quiet-machine
         datum (4 of 22 samples at or above 3.0, peak 3.37). Its 511/0/4
         stands.
  **Merged: PR #43 is `95583c5`** (the bookend, the standing pattern of §2c).
  Main's CI at `95583c5`, run `38664008411`, read per test (§2b): every job
  green (rust 40/40 steps, control-plane 27/27, dress 17/17).
  - rust: workspace 511 passed, 0 failed, 4 ignored; every floor green.
  - control-plane: npm 146; the show.stop e2e ran 6, exercised 1 (E4, the
    control), and skipped 5 on the encoder refusal.
  - dress: 27/27, `ok 24` (step 10). DRESS-COUNTS for CI (sha `95583c5`): 2
    drops, 195 underruns and 1 watchdog trip, within the backstops.
  - Position 2 is FIXED by it.
- ~~**Position 3: R16, the dress rehearsal's step 10 samples the transition
  tick**~~ **FIXED** by `b93db99` (PR #44; harness only; struck in the next
  PR, the standing pattern of §2c) (the user's word of 2026-10-12, ruling on PR #41's fresh two-key
  pass). A harness-side fix: step 10 must not sample the transition tick. It
  reads its baseline from a tick that postdates the engine's stop report,
  with no retries and no tolerance. The register row is R16 in
  `docs/soak-protocol.md` §5.

  **The fix (2026-10-14, the work order of 2026-10-14; branch
  `r16-step10-baseline`).** Harness only: no engine or control-plane change.

  **Interplay with PR #43, read at HEAD (`95583c5`).** #43 does not remove
  the race. Step 10's show has no active output: step 9 stops its own stream
  on both paths. So `show.stop` takes the handler's no-output path, forward
  only and no wait (`commands/show.ts`; SPEC §5.9.5 item 3, "With no active
  output, `show.stop` is forwarded and not waited on").
  - The response returns before the engine applies the stop, and the control
    plane has already set `showState: STOPPED`.
  - Its tick, on its own 1000 ms `setInterval` per subscriber (`server.ts`
    `startTelemetry`), carries the last engine report it received (`buildTick`,
    `world.last`).
  - The engine reports on its own 1000 ms deadline schedule (`channel.rs`
    pump).
  - So a tick between the stop's apply and the engine's next report carries
    the pre-stop frame. ~~The phase between the two cadences is fixed per run
    by the connect and subscribe order, which is why some runs never show it.~~
    The phase between the two cadences is not fixed. Node's `setInterval`
    drifts with event-loop lag while the engine's deadline schedule does not,
    and where the stop lands matters too, so whether a run shows the race is
    phase luck. That strengthens the not-by-construction point *(§2c,
    2026-10-14, the fresh pass on PR #44)*.
  - Read on a plain local run: the response took 5 ms, and the ack arrived
    1 ms after it. The first `STOPPED` tick came 915 ms after the ack,
    carrying frame 0: that run's phase hid the race. With the race arranged,
    the knob below, it carried frame 426.
  - The fix lands regardless, correct by construction rather than by phase.

  **The witness, and why it is honest.** The tick carries no applied
  version, no sequence and no engine report time. Its `ts` is the control
  plane's, and its `showState` is set before the forward. What it does carry,
  under the control plane's own rule, is freshness. So the baseline is **the
  first tick with `engineConnected: true` whose `ts` is more than
  `ENGINE_TELEMETRY_TTL_MS` (2000) after the stop's acknowledgement
  reached the control plane**. The proof:
  1. **The acknowledgement's arrival.** `ackedAt` is read in the dress's own
     process when `server.awaitApplied(stopVersion)` resolves, ~~which is in
     the turn that processed the acknowledgement~~ no earlier than the turn
     that processed the acknowledgement. It is read in a later microtask, or
     later still when the acknowledgement had already arrived
     (`waitForGrace`'s already-acknowledged branch). The proof needs only "no
     earlier than the ack", so the step still holds *(§2c, 2026-10-14, the
     fresh pass on PR #44)*.
  2. **The control plane's freshness rule.** `engineConnected` is
     `engineFresh`: `now − receivedAt ≤ 2000`, with the tick's `ts` that same
     `now` (`telemetry.ts` `buildTick`). So a fresh tick with
     `ts > ackedAt + 2000` carries a report received strictly after the
     acknowledgement, on the same socket.
  3. **The engine's ordering.** The engine queues the acknowledgement only
     after `clock.stop()` (`on_show_stop_inner`, then `apply`).
     - Its pump drains queued frames (acks) before it builds a report, and
       both go down one channel and one socket (`channel.rs`).
     - A report that arrives after the acknowledgement was therefore built
       after the stop.

  No retry and no tolerance: that tick is the baseline, and the step asserts
  that the clock does not move across `RISE_MS` from it, as before. If no such
  tick arrives within `ENGINE_TELEMETRY_TTL_MS + TELEMETRY_MS`, the step fails
  and names what it saw.

  The order's cadence fallback was not taken. The "second `STOPPED` tick" is
  sound only if the engine's next report lands before the control plane's
  next tick. The margin for that is the per-run phase gap less any lateness
  in the engine's pump, so it is a timing assumption, not a construction.
  The step prints a `STEP10` line: the transition tick and the baseline tick,
  with index, time from the acknowledgement and frame.

  **The knob** (`NBE_DRESS_STOP_DELAY_MS`, harness-only, off by default): the
  dress delays the `show.stop` directive on its way to the engine. The control
  plane has answered and is ticking `STOPPED` while the engine's clock still
  runs: R16's race, arranged on purpose.

  **Falsified, on the normative machine, knob 1500 ms:**

  | Run | Step 10 | `STEP10` |
  |---|---|---|
  | The pre-fix shape: no wait for the acknowledgement, the baseline from the first tick after the response | **fails**: `a stopped clock must not advance: 419 -> 0`, R16's signature. 26/27, only step 10 | the baseline is the transition tick, idx 23, frame 419 |
  | The fix, restored from the copy (`cmp`-identical, `tsc` clean) | **passes**, 27/27 | the transition tick, idx 23, 590 ms *before* the acknowledgement, carried frame 423; the baseline, idx 26, 2414 ms after it, carried frame 0. That run ended at load 3.06, void as a datum; the batch below supplies the in-ceiling greens |

  **The exercise: 25 consecutive kept runs, every one green** (the dress,
  normative machine, 2026-10-15 01:55–02:55 UTC, knob 1500 ms on every run).
  31 runs were started to keep 25.
  - **Load.** Each run started only below a 1-minute load of 2.5 (~~2.20–2.49~~
    the kept 25 started at 2.20–2.49; voided run 28 started at 2.04 *(§2c,
    2026-10-14, the fresh pass on PR #44)*).
    A run that ended at 3.0 or over is void per the tree's rule, and was kept
    out of the count.
    - Six were voided: runs 1, 14, 15, 20, 22 and 28, which ended at 3.13,
      3.98, 3.88, 3.38, 3.61 and 3.83. All six were green anyway (27/27,
      step 10 `ok`).
    - The kept 25 ended at 2.04–2.96.
    - An `opencode` process outside this work was using about 15% CPU
      during the batch, beside the terminal's 55%.
  - **The kept 25.** Each was 27/27, with `ok 24 - [RI-1] step 10`.
    - **The injection fired 25 of 25.** `stopDelayFired: 1`, and the
      acknowledgement came 1500–1505 ms after the response.
    - **The race was realized 25 of 25.** Every transition tick carried a
      running frame (420–429) and was built 548–608 ms *before* the
      acknowledgement.
    - The pre-fix step would have read each of those as its frozen frame.
      Its falsification above failed on exactly that.
    - Every baseline tick came three ticks after the transition tick,
      2396–2457 ms after the acknowledgement, at frame 0, and `later` was 0.
  - **Power** (precondition 6). 62 samples over the batch, once a minute:
    every one on AC, 45 W adapter, Low Power Mode on, so the whole batch is
    power-limited, not void.
    - The battery was 98% and charging at the start, then 100% and charged.
    - One sample read −28 mA, at 02:41:06 during kept run 24: a discharge
      on AC. `ioreg` printed it as an unsigned wrap, 18446744073709551588.
      It marks that run power-limited, which the adapter already does, not
      void.
    - `pmset -g log` shows no `Sleep`, `Wake` or `Using Batt` in the window.
    - Runs were under `caffeinate -i`, not the protocol's `-i -s`. The pmset
      check above is what rules out a sleep.
  - **A comment-only edit ~~straddles~~ preceded the batch.** At 01:53:37 UTC
    the stale "the tick does NOT carry showState" comment in the dress was
    annotated (§2c). ~~Every kept run started after that, so the kept 25 ran
    the committed file.~~
    - It landed after the falsification pair (01:48–01:52 UTC) and before
      every batch run (run 1 started at 01:55:12). So all 31 batch runs ran
      the committed file.
    - The falsification pair ran a file that differed from it only by that
      comment, so its behaviour was identical.
    - *(§2c, 2026-10-14, the fresh pass on PR #44.)*
  **Merged: PR #44 is `b93db99`** (the bookend, the standing pattern of §2c).
  Main's CI at `b93db99`, run `38685803055`, read per test (§2b): every job
  green (rust 40/40 steps, control-plane 27/27, dress 17/17).
  - rust: workspace 511 passed, 0 failed, 4 ignored; all 30 floor lines green.
  - control-plane: npm 146; the show.stop e2e ran 6, exercised 1 and
    skipped 5.
  - dress: 27/27, with `ok 24 - [RI-1] step 10`. DRESS-COUNTS for CI (sha
    `b93db99`): 1 drop, 188 underruns, 0 watchdog trips, within the
    backstops.
  - Position 3 and the R16 register row are FIXED by it.
- **~~Position 2~~ ~~Position 3~~ ~~Position 4: surface a failed re-resync on the `resyncRequest` path~~**
  **FIXED** by `461a30b` (PR #45; SPEC v0.4.8 row 7; struck in the next PR,
  the standing pattern of §2c). (the
  user's word of 2026-10-08, adopting PR #39's two-key pass). An established
  connection that asks again gets `sendResync` with its result ignored, and a
  throw there is answered only as an `E_ENGINE` reply on the engine's own
  socket, neither audited nor warned.
  - **The ceiling.** The engine's gate does not advance `expected_seq` on a
    `SeqGap` (`channel.rs` `classify`), so after a gap every directive is a
    gap: dropped, each one re-requesting. A persistent failure (a
    deterministic throw while the snapshot is built) therefore leaves a frozen
    engine on an established connection. Every directive is a SeqGap, every
    re-request fails, telemetry still flows, commands are accepted, nothing on
    air changes, and there is no signal.
  - A transient failure (a send refused under backpressure) heals on the next
    directive's re-request.
  - **The item:** audit and warn a failed re-resync, as the handshake now does
    (`resync.handshakeFailed`). Whether to tear down the established session
    on a persistent failure, handing recovery to the now-honest connect path,
    is the user's word when the item is built.

  **The read (2026-10-14, at `b93db99`; the work order of 2026-10-14),
  recorded before the code.**
  - **The `resyncRequest` handler** (`server.ts`):
    `} else if (frame.kind === "resyncRequest") { sendResync(session); }`.
    - Its result is ignored. `sendResync` has returned whether the socket
      accepted the resync since PR #39, and it throws if the snapshot cannot
      be built (`state.resyncSnapshot`: `ResyncPackageSchema.parse`,
      `ResyncViewItemEndSchema.parse`).
    - A throw escapes `handleMessage` and is caught at the session's tail:
      `ws.send(JSON.stringify(errorResponse(randomUUID(), state.stateVersion,
      "E_ENGINE", String(err))))`. It is neither audited nor warned.
  - **What reaches the engine today.**
    - On a throw: that error response. It is not a `DirectiveFrame`, so the
      engine logs `unparseable frame from control plane` (`channel.rs`) and
      carries on, connected.
    - On a refused send: nothing at all.
    - Either way the gate's `expected_seq` does not advance, so every later
      directive is a gap that re-requests. That is the frozen engine.
  - **Row 4's close semantics** (§5.9.4 rule 1, its failure half) cover a
    connect only: "it MUST close that connection (1011, `show.resync handshake
    failed`), hold no registration for it, and surface the failure". Rule 5,
    the re-request, has no failure half.
  - **The handshake's failure shape, the mirror to copy:**
    1. `forget()`.
    2. `audit.record({ kind: "command", outcome: "rejected", role, tokenId,
       command: "show.resync", event: "resync.handshakeFailed",
       stateVersionBefore, stateVersionAfter, detail: { error } })`.
    3. `deps.warn(...)`.
    4. `ws.close(1011, "show.resync handshake failed")`.
  - **`sendDirect`'s failure shapes** (`render-bridge.ts`, read in full). Every
    failure is `false`:
    - no registration (`if (!session) return false`);
    - the sender refuses, either `if (session.closed) return false` or
      `if (ws.bufferedAmount > 256 * 1024) return false; // backpressure`;
    - a sender throw, caught, logged (`render-bridge: direct send failed`)
      and returned as `false`.
  - **The split depends on what that `false` can be. Answer: never a
    persistent failure on a live connection.**
    - **A sender throw is unreachable.** `ws` 8.21.3's `send` throws only
      while `CONNECTING`, which a server-side socket never is once
      connected. On `CLOSING` or `CLOSED` it calls `sendAfterClose`: no
      throw, the frame dropped. And `JSON.stringify` cannot throw on the
      snapshot, which holds only strings, numbers, booleans, arrays and
      objects from Maps.
    - **So a `false` is backpressure** (it drains) **or a connection
      already gone** (`forget()` has run, and keeping it is moot).
    - **A persistent failure, the snapshot build, is an exception, never a
      `false`.** The tree distinguishes the two classes the ruling splits on,
      so the order's stop condition does not fire.
    - One wrinkle, recorded: a socket mid-close reads as *sent*. The sender
      returns true after `sendAfterClose`, and the close that follows tears
      the session down and hands recovery to the connect path.

  **The design** (the user's ruling of 2026-10-14, the split):
  - **Each failed re-resync is audited and warned.** The event is
    `resync.requestFailed`, the handshake's sibling: kind `command`, outcome
    `rejected`, command `show.resync`. Its detail is `{ error, cause:
    "snapshot" | "send", reason, sessionKept }`, where `reason` is the
    engine's `resyncRequest` reason. The audit kinds are unchanged.
  - **A throw while the snapshot is built is the persistent class.** The
    handler runs `forget()`, the audit, the warn, then `ws.close(1011, "show.resync request failed")`, in the handshake's
    order, leaving no registration. Recovery passes to the engine's
    reconnect loop and a clean slate, where rule 1's handshake retries it
    loudly.
  - **A refused send is the transient class.** The handler audits and
    warns, and keeps the session. The next directive's gap re-requests.
  - **The rate.** Every failure is its own audit row: no coalescing, because
    the audit is the record. The steady state under a persistent throw
    follows from the engine's backoff (`channel.rs` `run_forever`: 100 ms,
    doubling to a 10 s cap, reset only by a connection that outlived the
    current delay):
    1. The established connection outlived its delay, so after the 1011 the
       engine reconnects in 100 ms.
    2. Each reconnect's handshake builds the same snapshot, throws, and
       closes within milliseconds, never outliving its delay. So the delays
       run 200, 400, 800, 1600, 3200, 6400, then 10 000 ms.
    3. That gives one `resync.requestFailed` row, then `resync.handshakeFailed`
       rows at about 0.1, 0.3, 0.7, 1.5, 3.1, 6.3, 12.7, 22.7 s and every 10 s
       after: ~~about 11 in the first minute~~ 1 + 11 = 12 rows in the first
       minute (one `resync.requestFailed`, then 11 `resync.handshakeFailed`),
       then 6 a minute, each with its warn. *(§2c, the fresh pass on PR #45:
       the 11 counted only the handshake rows.)*
    4. The rate is paced by the engine's backoff, ~~not by directives~~ once
       the connection is torn down. Before that, each request already in
       flight at the teardown adds its own row, so the extra rows are bounded
       by the requests already in flight. The fresh pass's probe sent three
       requests in flight and got three `snapshot` rows on one connection.
       *(§2c, the fresh pass on PR #45.)*
  - **Under a transient refusal.** One row per refused re-request. Those
    come only as directives reach the engine and meet a gap, so the rate is
    bounded by the directives that get through, and none are sent while the
    socket stays backed up.
  - **Scope.** The warn reaches the console, as `SHOW_STOP_HELD_WARNING`
    does. An operator surface is R17's queued item (position 5).

  **The code and the evidence** (2026-10-14, branch `resync-request-audit`;
  struck FIXED by its merge SHA in the next PR). Row 4's record, the
  zombie-registration fix below, is the handshake this mirrors.
  - **`server.ts` `resyncOnRequest(reason)`** replaces the bare
    `sendResync(session)`. It builds and sends the re-resync inside a guard.
    - A throw runs `forget()`, then audits `resync.requestFailed` with
      `{ error, cause: "snapshot", reason, sessionKept: false }`, warns, and
      calls `ws.close(1011, "show.resync request failed")`.
    - A `false` audits `{ cause: "send", sessionKept }` and warns. It keeps
      the session (`sessionKept` is false only if the connection had already
      closed).
    - `audit.ts` documents the event. The kinds are unchanged.
  - **Guards, ~~on a fake socket~~ on a real server and a real socket, the
    engine scripted by the test** (`resync-handshake.test.ts`, +3; the suite is
    6/6; §2c, the fresh pass on PR #45):
    - **G1:** the snapshot throws on an established connection's request.
      The result: closed `{1011, "show.resync request failed"}`,
      registrations 0, no resync, no error reply, audited `[{rejected,
      show.resync, snapshot, seqGap, sessionKept: false}]`, warned, and the
      next connect's first directive is `show.resync` (registrations 1).
    - **G2:** the first re-request's send is refused. The result: the socket
      open, registrations 1, audited `[{…, send, seqGap, sessionKept:
      true}]`, warned; the next request's resync arrives, and an operator's
      directive flows.
    - **G4:** a re-resync that succeeds audits nothing and warns nothing.
  - **Guards, on the real engine** (`resync-request.e2e.ts`, 3/3, normative
    machine). Each leg drops one directive on its way to the engine, so the
    next one meets a real sequence gap:
    - **snapshot:** `{"audited":[{"cause":"snapshot","reason":"seqGap",
      "sessionKept":false}],"warned":true,"gapRequests":1,"reconnects":1,
      "handshakeFailures":0,"registrations":1}`. The engine reconnected, its
      handshake resync applied, and directives applied again.
    - **send:** `{"audited":[{"cause":"send","reason":"seqGap",
      "sessionKept":true}],"warned":true,"frozenUntilTheNextRequest":true,
      "gapRequests":2,"reconnects":0,"registrations":1}`. The engine stayed
      frozen until the next gap re-requested, and then it resynced and
      directives applied.
    - **persistent:** one `resync.requestFailed`, then `resync.handshakeFailed`
      rows at **104, 309, 712, 1518, 3122, 6327, 12 730 and 22 735 ms** (that
      run's load was not recorded; the in-ceiling measurements are under G3
      below): the
      engine's backoff, as the design computed (0.1, 0.3, 0.7, 1.5, 3.1, 6.3,
      12.7, 22.7 s). ~~That is 9 attempts and 8 rows, one per attempt, each
      warned.~~ The engine logged 9 dropped connections and the audit holds 8
      `resync.handshakeFailed` rows, one per failure, each warned. The first
      drop is the request failure's own 1011, whose row is the
      `resync.requestFailed`; each of the other 8 is a failed handshake.
      *(§2c, the fresh pass on PR #45: this read as attempts trailing rows at
      the cut-off.)* Five directives sent in that time added no rows.
  - **G3, the falsification of the whole.** `resyncOnRequest(frame.reason)`
    was put back to the silent `sendResync(session)`, restored from the copy
    and `cmp`-identical, with `tsc` clean.
    - **Unit:** G1 and G2 fail. G1 read `closed: 'never: the socket stayed
      open'`, `registrations: 1`, `errorReplies: ['E_ENGINE']`, `audited:
      []`, `warned: false`: the frozen ceiling, made observable. G2 failed
      at "the refused send is audited". G4 passes either way, as it should.
      Restored: 6/6.
    - **Real engine:** all three legs fail. snapshot failed at "the failed
      re-resync is audited", and send at "the refused re-resync is audited".
      persistent read `{"requestFailed":[],"attempts":0,
      "handshakeFailedRows":0}`: in 25 s of failures the engine was never
      torn down. Restored from the copy (`cmp`-identical). The gate's run at
      `84a5e4f` is 3/3. ~~Its persistent leg read the backoff again: 107,
      312, 716, 1520, 3125, 6329, 12 736 and 22 742 ms.~~ That run sat in the
      control-plane/e2e window voided below, so no timing is quoted from it
      *(§2c, the fresh pass on PR #45)*. In-ceiling measurements:
      - **Mine, at the delta's tree** (2026-10-15, 01:21:35–01:22:01 UTC):
        105, 309, 716, 1520, 3125, 6329, 12 734 and 22 737 ms. Load was
        sampled every 2 s and read 2.30–2.54, never at or above 3.0, with no
        discharge.
      - **The fresh pass's five runs**, each under the ceiling (peak loads
        2.51, 2.57, 2.56, 2.02 and 2.52). The first read 105, 310, 713, 1517,
        3120, 6324, 12 728 and 22 732 ms.
      - **The engine's constants** (100 ms, doubling to a 10 s cap) give
        100, 300, 700, 1500, 3100, 6300, 12 700 and 22 700 ms. The measured
        excess, about 5–40 ms, is accumulated connect overhead.
      - Two more of my runs crossed 3.0 (peaks 3.04 and 3.36). They are void
        as data, and their passes (3/3 each) stand.
  - **The gate at `84a5e4f`** (normative machine; on AC, 45 W adapter, Low
    Power Mode on, so power-limited; no sleep or battery event):
    - Rust: workspace 511 passed, 0 failed, 4 ignored (twice); `spec_budgets`
      2/2; clippy, fmt and tsc clean.
    - Control plane: npm 149 (146 plus the three new guards); automation
      32; effects 2; handshake suite 6.
    - E2e, every leg exercised: slate-release 4, recall 2, take-duration 3,
      resync-end 3, package-reconcile 4, show-stop 6, resync-request 3.
    - Dress: 27/27, DRESS-COUNTS normative 0 drops and 0 underruns, load
      2.31–2.59.
    - **Voids, named.** Both workspace runs crossed 3.0 as the suite ran:
      10 of 22 samples (peak 3.59), then 13 of 22 (peak 4.23) after starting
      at 2.22. So did the control-plane and e2e window, 28 of 77 samples
      (peak 5.80). Each is void as a quiet-machine datum. Their pass counts
      stand, and no timing claim rests on them.

  **The fresh pass's delta** (the user's ruling of 2026-10-15 on PR #45's
  two-key pass, adopted in full: a guard and the docs, no other code):
  - **D1, the teardown's order, now guarded.** Nothing pinned that the
    registration is released *before* the close. The socket's own
    `close → forget` listener masked a missing `forget()`: the pass removed
    it on either path, and both suites stayed green.
    - **The probe.** `resync-handshake.test.ts` wraps the server-side
      `WebSocket.prototype.close`. At the instant the server initiates its
      1011 close, before any close listener can run, it reads the
      registrations, the audit rows and the warn.
    - **One test per path:**
      - "the order, the request path: forget, audit and warn all precede the
        1011 close";
      - "the order, the handshake path: …".
    - Each asserts `[{ registrations: 0, audited: 1, warned: true }]`. Only
      the first `close(1011)` is read, because `ws` calls `close` again
      internally on the echoed frame. The suite is 8/8.
    - **Falsified, each path.** `forget()` was removed from the request path
      (`resyncOnRequest`'s catch), and then from the handshake path (the
      handshake's catch).
      - Each time only that path's order test failed: `+ registrations: 1,
        - registrations: 0`, 7/8.
      - The end-state tests stayed green, which is the masking the pass
        found.
      - Restored from the copy, `cmp`-identical, `tsc` clean: 8/8.
  - **D4, the order goes into the spec** (the user's decision). Rule 1's
    failure half now states it: release the registration, audit, warn,
    close (1011, the named reason), as the load-bearing order. Rule 1's old
    text is struck (§2c). Row 7's "in rule 1's order" now cites real spec
    text. The alternative, dropping the phrase, was rejected.
  - **D2.** Row 7 and the impact paragraph claimed the 1011 replaced "an
    `E_ENGINE` reply or silence". It replaces only the `E_ENGINE` reply; a
    refused send was silent on the wire and still is. Both are struck and
    corrected, and the PR body is edited, noting the edit.
  - **D3.** The 9-and-8 explanation is corrected above and in the e2e
    comment: 1 + 11 = 12 rows in the first minute. The assertion stands.
  - **D5.** "Never the directive rate" is struck in rule 5 and above: the
    extra rows are bounded by the requests already in flight, as the pass's
    probe showed (3 requests, 3 `snapshot` rows on one connection).
  - **D6.** Rule 5 now states the mid-close drop: it reports as sent, so
    nothing is audited, and nothing is lost either.
  - **D7.** "On a fake socket" is struck above and in the PR body: the unit
    guards run a real server and a real socket. The e2e header's grouping
    is corrected: G3 is the falsification, not a guard.
  - **D8.** The backoff quote from the voided window is struck above and
    requoted from in-ceiling runs.
  - **The audit-sink cases** are recorded and queued, with no code: R18 in
    the register, attached to R17's operator-surface item (position 5).
  **Merged: PR #45 is `461a30b`** (the bookend, the standing pattern of §2c).
  Main's CI at `461a30b`, run `38836482232`, read per test (§2b): every job
  green (rust 40/40 steps, control-plane 28/28, dress 17/17).
  - rust: workspace 511 passed, 0 failed, 4 ignored; all 30 floor lines green.
  - control-plane: npm 151; the show.stop e2e ran 6, exercised 1 and
    skipped 5; the resync-request e2e ran 3 and exercised 3.
  - dress: 27/27, with `ok 24`. DRESS-COUNTS for CI (sha `461a30b`): 1 drop,
    163 underruns, 0 watchdog trips, within the backstops.
  - Position 4 is FIXED by it.
- **Position 5: R17, the control plane reports a refused stream as live**
  (the user's word of 2026-10-12, establish-first). On CI the engine refuses
  `stream.start`, and the control plane's telemetry still reports the stream
  `live` for five ticks, until the dress's own `stream.stop`. The first step
  is to establish what the control plane hears of the refusal and what an
  operator sees. It is cross-referenced with R16, the same observed-state lag
  on another surface. The register row is R17 in `docs/soak-protocol.md` §5.
  - ~~**Queued 2026-10-14 (the user's word on PR #43's fresh pass): a held
    stop on an operator surface.**~~ **LANDED in spine part A** (branch
    `operator-signal-lane`, 2026-10-15): the audit row (`show.stopHeld`, raised
    and cleared) and the lane (`outputs.held`). Promoting `held` into the
    belief enums is part B's. *(§2c.)*
    - When `show.stop` goes unacknowledged, the control plane holds the
      outputs (SPEC v0.4.8 row 6) and logs `SHOW_STOP_HELD_WARNING` to its
      console only.
    - An operator-facing held-outputs state belongs on an operator surface,
      not a console: an audit row at minimum, and the tick. The tick's only
      warnings channel today is `deprecationWarnings`, so it needs a field of
      its own or a general channel.
    - It sits with this item because it is the same gap: the control plane's
      belief about an output, beside the engine's report, with nothing that
      tells the operator they differ. The late-acknowledgement
      reconciliation, queued here with row 6, is its other half.
    - The failed re-resync's warn (queue position 4, SPEC v0.4.8 row 7) is
      console-only too, and belongs on the same operator surface.
    - **R18 attaches here** (the user's word of 2026-10-15, record and queue).
      When the audit sink itself is broken, a failure is invisible on that
      surface by definition. A last-resort surface for audit failure is the
      open design question. The register row is R18 in
      `docs/soak-protocol.md` §5.

  **The spine (the user's design ruling of 2026-10-15).** The five R17
  threads are one defect. The control plane's belief about failure and
  output state reaches an operator inconsistently, and once that belief is
  wrong it has no honest way to be corrected. The work splits by risk:
  - **Spine part A** (this branch, `operator-signal-lane`). An operator-signal
    lane: alerts on the shared `statusSnapshot`; the control plane announces
    stream divergence; R18's fallback. It is control-plane-only and
    **additive**: nothing a client reads today changes meaning, no enum
    moves, no automation rule's inputs move, there is no engine or wire
    change, and no audit trio is touched.
  - **Spine part B.** The behavioural half: the precedence rule (a fresher
    engine report beats the control plane's belief, and an explicit operator
    action beats both), `held` promoted into the belief enums, and R17's
    refusal-lag fix as its proving case.
  - **Work order 2.** Engine-touching: an engine-reported record state, and
    output state on the resync snapshot, as its own spec row.

  **The design record, before the code.**
  - **Three tree corrections to the framing** (the user's map of
    2026-10-15):
    1. There are six surfaces, not three. Besides the audit file, the
       console and the tick, the control plane pushes a `stateChange` frame on
       every accepted command. That frame, `system.status` and
       `GET /status` all carry the full `statusSnapshot`. Command error
       replies make the sixth.
    2. Stream provenance is already present. The tick carries the engine's
       own `streamTransportState` beside the control plane's `streamState`,
       so a refused stream shows `live` beside `none` and a held stop shows
       `live` beside `closed`. The defect is that nothing announces the
       contradiction.
    3. There is no engine-reported record state: only the tap's path and
       reason, and the free space.
  - **The four adjustments, ruled:**
    1. `held` is an alert in part A, not an enum value. Moving the enum is
       part B's, with the automation question: `streamState` and
       `recordState` are rule-nameable, and their values are not checked.
    2. Divergence alerts use PR #44's freshness construction and exclude
       the stale stub.
    3. Alerts are audited on their edges, never per tick.
    4. R18's fallback is the one alert without an audit row.
  - **Refinements.**
    - Audit rows carry severity and reason.
    - Restart semantics are stated (below, once built).
    - B's spec row must answer the automation transition: whether
      live→held should fire `stateChange` rules, and that `to: "idle"` rules
      would stop matching. B also strikes the control plane's dead
      `"reconnecting"` value (~~`state.ts:16`~~ `state.ts:17`, one line
      down since spine A's `Alert` import; §2c). *(The automation
      transition is answered by the user's ruling of 2026-10-16, below.)*
  - **The automation transition, ruled** (the user's ruling of 2026-10-16,
    spine B; it answers the refinement above).
    1. `stateChange` fires on the actual belief transition with the actual
       values, exactly once: live→held fires
       `stateChange{from: "live", to: "held"}`. It does not fire to
       `"idle"`.
    2. Rules written `to: "idle"` not matching a hold is the correct
       semantics, not a regression. Idle is terminal: nothing armed,
       nothing resumes. ~~Held is intentional suspension: resume expected.~~
       *Held is §5.9.5's limbo: a stop commanded, the output's state unknown
       (ratified 2026-10-16; the relay's "intentional suspension" gloss is
       struck, §2c: it described a pause feature the tree does not have).*
       "Stream down" automations must not fire on ~~an operator's hold~~
       *a held stop (§2c, the same gloss)*;
       "left the air" automations must not miss a real stop.
    3. The "left the air" intent is served by from-matching: `from: "live"`
       catches live→idle, live→held, and any future departure. If the
       trigger schema supports only `to:` today, the design record adds
       `from:` as part of this work: the vocabulary ships complete, not as a
       later alias.
    4. No compat mapping, no double-firing, no held-fires-idle alias. The
       compat burden is zero: the automation runtime is unshipped (roadmap
       §1), so no rule depends on a collapse that never existed.
    5. §13 gets the belief vocabulary enumerated (idle, live, held, …) as
       the normative matcher domain: a spec row per the promotion path,
       before any implementing prompt.
    - **The tree, read against the ruling** (the tree wins; recorded, not
      changed):
      - **Item 3 is moot** (the user's word of 2026-10-16, ordering the spec
        row): `from:` already ships. §13.2's params table (normative since
        v0.4.7) gives `stateChange` `{ field, itemRef?, from?, to? }`. Preflight admits the key
        (`crates/nbe-core/src/automation.rs:313`), and the control plane
        parses it (`automation.ts:173`) and matches it by equality on the
        event's `from` (`:353`). So `from: "live"` already catches every
        departure from live, today's live→idle included, and spine B adds
        no key. Its new transitions arrive through the same matcher.
      - **What is missing is item 5's domain.** Neither preflight
        (`automation.rs:312–332`, keys and `field` only) nor the control
        plane (`automation.ts:173–174`, copied as given) checks a `from` or
        `to` value. So a rule naming `"held"` loads today and can never
        fire, and a misspelled value loads the same way. §13.2's own
        principle for `audioLevel` is "a rule that can never fire is not
        accepted and left inert". The enumerated domain is what lets both
        readers refuse such a rule.
      - **The relay's "roadmap §1" citation, corrected** (the user's word of
        2026-10-16): it named an off-tree document. The one roadmap document
        in the tree, `docs/move-parity-and-virtual-set-roadmap.md`, has as
        its §1 "The core finding", on Move-transition parity. The substance
        was confirmed from the tree: the repository has no tag and no
        release, so no automation rule exists outside the tree. The runtime
        is built (`automation.ts`; `automation.test.ts` 32/32 on every CI
        run; AC-25's measurement), but unreleased. Relay misses are recorded,
        not smoothed.
      - **Item 5's vocabulary, per field, in the tree today.**
        - `StreamState` is `idle | live | reconnecting` (`state.ts:17`).
          `reconnecting` is the dead value B strikes.
        - `RecordState` is `idle | recording` (`state.ts:18`). The record
          belief's on-air value is `recording`, not `live`.
        - So the spec row enumerates each field's domain separately.
        - §13.2's `stateChange` fields also include `showState`,
          `viewItem`, `previewItem`, `automationHold`, `fallbackActive` and
          `itemState`. The ruling names the belief vocabulary; whether the
          row enumerates those too is the row's to state. *(The user's scope ruling of 2026-10-16:
          every field is enumerated. The row's table is below.)*
  - **Spine B's spec row: SPEC v0.4.8 row 9, drafted 2026-10-16** (branch
    `spine-b-domain-row`; the user's GO and scope rulings of 2026-10-16; its
    own PR, ratified on merge, before any implementing prompt).
    - **The scope, as ruled.**
      1. Per-field domains in one table. No field inherits another's
         vocabulary. `streamState` gains `held`, and the dead `reconnecting`
         is struck.
      2. Every `stateChange` field is enumerated from tree facts. A domain the
         tree shows unstable, or owned by unlanded work, is named as an
         exception with its owner.
      3. A validation domain, not documentation: preflight and the control
         plane refuse a value outside it at load, with refusal tests on both
         readers.
      4. This design record gains the precedence section, below.
      5. The first commit lands the staged records (`5b8b70d`).
      6. Spec work per Standards §1.4, and new vocabulary in `VOCABULARY.md`
         on the same PR.
    - **The field domains, with the tree's writers** (the spec's table, with
      the lines it rests on; every value has a writer except exception 1's):

      | Field | Domain | Writers in the tree |
      |---|---|---|
      | `showState` | `UNLOADED`, `LOADED`, `RUNNING`, `STOPPED` (`ShowState`, `state.ts:16`) | `UNLOADED`: initial (`state.ts:120`), `unloadPackage` (`:485`, from `show.unload`, `commands/show.ts:183`), crash recovery (`persistence.ts:71`). `LOADED`: `loadPackage` (`state.ts:467`). `RUNNING`: `show.start` (`commands/show.ts:120`). `STOPPED`: `show.stop` (`:140`) |
      | `viewItem` | `null`, or a rundown item id | a take (`state.ts:294`; `view.take`, `view.cut`); `null` initially (`:141`) and at load and unload (`:469`, `:486`); `recallSnapshot` (`:453`) |
      | `previewItem` | `null`, a rundown item id, or a scene id | `preview.set` (`commands/view.ts:32`); **`armScene` writes the scene's id into an empty preview** (`state.ts:414`, from `scene.arm`); `null` initially (`:173`), on a take of the previewed item (`:297`), and at load and unload (`:472`, `:489`); `recallSnapshot` (`:454`) |
      | `streamState` | `idle`, `live`, `held` | `live`: `stream.start` (`commands/output.ts:36`). `idle`: initial (`state.ts:204`), `stream.stop` (`output.ts:45`), `show.stop`'s acknowledgement (`commands/show.ts:164`). `held`: none yet (exception 1). `reconnecting`: written nowhere since Prompt 02's type (`ffea3ab`), now struck from `StreamState` (`state.ts:17`) |
      | `recordState` | `idle`, `recording` (`RecordState`, `state.ts:18`) | `recording`: `record.start` (`output.ts:14`). `idle`: initial (`state.ts:203`), `record.stop` (`output.ts:25`), `show.stop`'s acknowledgement (`show.ts:163`) |
      | `automationHold` | `true`, `false` | initial `false` (`state.ts:197`); `automation.hold` (`commands/state.ts:70`); `recallSnapshot` (`state.ts:460`) |
      | `fallbackActive` | `true`, `false` | `true`: `view.fallback` (`commands/view.ts:84`), and the view item's `markMissing` or `markError` (`state.ts:320`, `:326`, from the engine's item event, `server.ts:908–909`). `false`: initial (`:174`), a take (`:298`), load and unload (`:475`, `:497`) |
      | `itemState` | the seven of `ItemState` (`state.ts:14`) | each written: `ARMED` `:271`; `READY` `:279`, `:291`, `:307`, `:343` and the default read (`automation.ts`'s `readField`); `LIVE` and `PLAYING` `:293`; `DONE` `:314`; `MISSING` `:319`; `ERROR` `:325` |

    - **The exceptions, named with their owners** (the spec's three):
      1. `held` is in `streamState`'s domain with no writer. Owner: spine B's
         implementing prompt. Until it lands, a rule naming `held` loads and
         cannot fire: the one such rule §13.2 admits.
      2. `recordState` has no `held`, by the scope ruling. The tree's §5.9.5
         hold covers both outputs, and the record belief stays `recording`
         through it. The engine reports no record state (the spine's tree
         correction 3, above), so nothing could detect or heal a record hold.
         Owner of a record `held`, if one is ever ruled: work order 2. §10.1.2's
         "Promoting `held` into `streamState` and `recordState`" is narrowed
         (§2c there).
      3. A condition's `equals` is not read against the domain. The ruling's
         scope is `from` and `to`; a condition names the same fields, so an
         `equals` outside its field's domain can never hold. Owner: ~~the
         user's word~~ **this design record** (§2c). **The principle is
         adopted and the scope deferred** (the user's ruling of 2026-10-16;
         #50 does not expand). **The named follow-up row: "the `equals`
         domain row"**: a condition's `equals` read against its field's
         domain, refused at load in the same shape on both readers
         (``condition `equals` <value> is outside <field>'s domain (<words>)``),
         with the shared fixture's `reason` holding the words to one.
    - **Tree findings, recorded, not changed.**
      1. **`previewItem` carries scene ids.** `armScene` writes a scene's id
         into an empty preview (`state.ts:414`), so the field named for items
         holds a scene. The domain follows the tree, and both readers now take
         the package's scene ids (`RuleRefs.scenes`, `RuleContext.scenes`).
      2. **"Held" now has two meanings in the spec.** §13.5 says "held" for
         `automationHold` true; row 9 makes `held` an output value. The ledger
         records both, kept apart.
      3. **The ruling's picture of `held` is not the tree's.** The ruling calls
         held "intentional suspension", "an operator's hold", with "resume
         expected". The tree's only held is §5.9.5's: a `show.stop` the engine
         never acknowledged, the outputs held at their prior state because the
         control plane cannot know them. Nothing in the tree lets an operator
         hold a stream. The spec and the ledger define `held` from §5.9.5, as a
         suspension rather than a terminal state, which both readings share.
         If spine B also means an operator-initiated hold, that is new
         behaviour with an entry of its own: ~~put to the user~~. **Ruled
         2026-10-16:** `held` is §5.9.5's limbo, a stop commanded and the
         output's state unknown; the gloss is struck in the ruling above. An
         operator-initiated hold is new behaviour, on the promotion path with
         its own entry, and not this spine's (§2c).
      4. **The lane's comment undersells its own rule.** `alerts.ts:171` reads
         "The operator's explicit action supersedes the last episode", but
         `onAccepted` hears every accepted output command, automation's
         included (`server.ts:336`, dispatch's `onSent`). The behaviour is the
         precedence section's fact 3, below.
      5. **A stale local preflight.** `preflightBin()` prefers
         `target/release/nbe-preflight`, and the one on the normative machine
         was built 2026-10-03, before #35's preflight change landed
         (`d7b96f7`). So the control plane's local suites ran an old preflight
         until it was rebuilt for this PR (2026-10-16 21:02). CI builds it
         fresh on every run (`ci.yml`, the control-plane job), so no CI
         result is affected. *Accepted (the user's word of 2026-10-16): CI's
         freshness held. A freshness assertion (the test helper refusing a
         preflight binary older than its sources) is recorded as a hardening
         candidate, not acted on.*
      6. **The `telemetry.test.ts` token pin named `reconnecting` a normative
         wire token.** No spec text did, and no handler ever wrote it. It is
         struck with the ruling (§2c in the test's comment), and the test is
         renamed `stream state tokens are stable: idle, live`.
  - **The precedence section: where automation actions rank** (the user's
    scope ruling 4: argue the proposed order from tree facts, or amend it
    with reasons). Proposed: explicit operator action > automation action >
    fresh engine report > stale belief.
    - **The tree facts.**
      1. **An automation action is a command on the one command path, run as
         `operator`** (`server.ts:438–447`). The path's comment at `:345–347`
         reads "same preconditions, same audit, same acknowledgement rules —
         so it must not have a second path". The action reaches the belief
         through the same handler as the operator's identical command
         (`commands/output.ts:36`, `:45`), and it sends the same directive.
      2. **Dispatch does not rank the two.** Commands apply in the order they
         are accepted. A rule's action differs only in its audit kind
         (`automation`) and in its exemption from the session limiter
         (§13.3, v0.4.7 row 6).
      3. **The lane already treats them alike.** `onAccepted` hears every
         accepted output command through dispatch's `onSent`
         (`server.ts:336`). An automation command therefore opens (b)'s
         window and supersedes an episode exactly as an operator's does
         (`alerts.ts:157–178`).
      4. **The operator outranks automation before dispatch.** While
         `automationHold` is true, every trigger is suppressed within a frame
         and every pending action is cancelled (§13.5; AC-25 #2; the
         evaluator's flow in `automation.ts`, "held? suppressed"). No rule can
         fire to release the operator's hold: the hold is the one operator
         action automation cannot answer. §13.1 gives automation the
         operator's preconditions and nothing more.
      5. **A report outranks a command only after the engine has acted on
         it.** Spine A's witnesses hold the belief a command set until its
         acknowledgement, whichever actor sent it: (b)'s window runs to the
         exact acknowledgement, and (a) needs a report that is fresh and
         postdates the anchor (PR #44's construction).
    - **The argument.**
      - *Automation action > fresh engine report: holds.* By facts 1, 3 and 5,
        an accepted automation command stands against a report where an
        operator's does. A report that predates its acknowledgement describes
        the world before the directive; a fresh one that postdates it
        outranks the command's belief, as it would the operator's.
      - *Fresh engine report > stale belief: holds.* It is spine B's rule, on
        spine A's freshness.
      - *Explicit operator action > automation action: holds as authority,
        not as a belief tier.* By fact 4, the operator can stop automation
        from acting, and automation cannot stop the operator. But once an
        automation command is accepted, the belief must follow it, whoever
        sent the command before it, because the engine was sent that
        directive. A belief that ranked an earlier operator command above a
        later accepted automation command would name an output the engine was
        told to leave: the divergence R17 exists to end. Between two accepted
        commands, the later one sets the belief (fact 2).
    - **The order, amended with those reasons** ~~(put to the user)~~
      **RATIFIED 2026-10-16** (the user's word, §2c). It is recorded as argued
      from `server.ts:438` (a rule's action runs on the one command path as
      `operator`) and `:336` (the lane hears every accepted output command):
      the relay's linear order amended by the tree, an improvement and not
      a reversal.
      - **Authority, before dispatch:** explicit operator action > automation
        action. The hold enforces it (§13.5), and §13.1's shared
        preconditions bound it.
      - **Belief, after dispatch:** the latest accepted command, operator's or
        automation's, until its acknowledgement > a fresh engine report that
        postdates that acknowledgement > stale belief.
      - The proposed four-tier order is this pair read as one line. The
        amendment splits it at dispatch, because the tree enforces the first
        comparison only there.
      - **The refused start, as ratified:** it "flows through unchanged:
        refusal is the acknowledgement; the fresh report after it moves belief
        to truth" (the user's word).
        - **The tree differs, recorded and put to the user.** In the tree a
          refusal is never acknowledged: `directive.rs`'s `apply` returns on
          the handler's error (`on_stream_start(d)?`, `:132`) before
          `set_last_applied` and `ack` (`:147–148`), and the channel only logs
          `directive failed` (`channel.rs:265`). This was the spine's witness
          finding 1, above.
        - So, read literally, "until its acknowledgement" never ends for a
          refused start. The case reaches the fresh report only if (b)'s
          window ending unacknowledged (§5.9.5's 2000 ms) stands in for the
          acknowledgement, or if the engine acknowledges its refusals, which
          is an engine and wire change.
        - Which of the two is the implementing prompt's to carry is the
          user's word. The row is unaffected: its domains and the transition
          rule do not depend on it.
          *(Ruled 2026-10-16: (a). The block below; §2c.)*
      - **The refused start: (a), the user's pick of 2026-10-16.** At the end
        of the 2000 ms unacknowledged window, the command's claim on belief
        expires, and belief resolves by the remaining tiers: a fresh report,
        then stale belief. The alert's raise at the same instant is the
        announcement: the control plane admits ignorance, the lane says so,
        and belief stands down together.
        - ~~**The precision clause, verbatim:** "window expiry ends the belief
          claim and does nothing else — it is not an acknowledgement,
          Amendment 4's "the command's own ack" stays exact, and the alert
          never auto-clears at window end. No engine change, no wire change,
          no ratified row amended."~~ *Struck (§2c, the user's amendment 1 of
          2026-10-17): as written it contradicted the pick, because the alert
          is something expiry does.*
        - **The precision clause, amended and signed off 2026-10-17,
          verbatim:**

          > "Window expiry ends the command's claim on belief and raises the
          > expiry alert where §10.1.2's (b) raises one, and does nothing
          > else. The alert announces the fallback; it does not act on the
          > claim. Expiry is not an acknowledgement: Amendment 4's "the
          > command's own ack" stays exact, and the alert never auto-clears
          > at window end. No engine change, no wire change, no ratified row
          > amended."

          - **Where (b) raises one** (`alerts.ts:214–216`). At the window's
            unacknowledged end, (b) raises unless `engine.notReporting` owns
            the case. There the claim still expires under that alert's
            announcement, and with no fresh report to resolve to, the stale
            belief stands.
          - **Outside the clause.** A command acknowledged inside its window,
            by its own acknowledgement or by a later one proving it processed,
            was confirmed under Amendment 4, not expired
            (`alerts.ts:199–201`), so the clause's trigger never fires there.
            *(The user's fix of 2026-10-17: the draft's note had listed this
            case as one where the claim expires without a raise.)* A command
            superseded inside its window by a newer command for the same
            output gave its claim to the newer one at that acceptance (the
            order's first tier), before any expiry. *(The builder's addition,
            from the ratified order, flagged for the user.)*
        - **The late-acknowledgement rule, signed off 2026-10-17 (amendment 2,
          with the user's fix: "logged" scoped to the alert-clear path),
          verbatim:**

          > "An acknowledgement arriving after its window's unacknowledged
          > expiry is inert to belief: logged where the alert-clear writes
          > its audit row, not applied. The dead claim cannot reclaim the
          > belief. It still clears the expiry alert by §10.1.2's ratified
          > late-acknowledgement rule."

          In the cases where (b) raised nothing (`engine.notReporting` owning
          it), there is no alert to clear, so a late acknowledgement there is
          not logged. The scoped wording says so, and no hardening candidate
          is named for it (the user's default).
        - **Read together with the ratified order** (the builder's
          restatement of the two rulings, not new text of the user's): the latest
          accepted command, operator's or automation's, until its
          acknowledgement or its window's unacknowledged end, whichever comes
          first > a fresh engine report that postdates it > stale belief. The
          window's end is the claim's expiry, not an acknowledgement.
        - **Option (b), recorded as a named hardening candidate: "engine
          refusal-acks".** They would converge belief in milliseconds and give
          the audit lane cause attribution, at the price of a wire change, an
          engine change, and a §10.1.2 mapping-table amendment. It needs its
          own spec row, sequenced after work order 2.
        - **Carried into the implementing prompt's brief, verbatim: the pick
          ~~and the precision clause above~~** *and the amended precision
          clause and the late-acknowledgement rule above (§2c, 2026-10-17)*.
          No brief exists yet:
          `agents/prompts/` holds prompts 01–15, and none is spine B's. This
          block is the carry, and it lands verbatim in the brief when the
          brief is written.
    - **SPEC v0.4.8 row 10, drafted 2026-10-17** (branch
      `spine-b-belief-row`, PR #51; ratified on merge, before any
      implementing prompt): §10.1.3, belief precedence. It holds exactly the
      three items: the belief order, the amended clause, and the
      late-acknowledgement rule.
      - **Verified first, not assumed** (the user's order): row 9 and the §13
        enumeration landed whole with #50. On `main` at `9e6239c`, the v0.4.8
        table ends at row 9. §13.2 carries the field-domain block, its three
        exceptions (the `equals` domain row named), the transition rule and
        the `stateChange` clause. §10.1.2's narrowing and row 9's header
        sentence are in place. Both readers' domains, the scene ids, the
        struck `reconnecting`, the 52-case fixture and the ledger entries are
        on `main`. Row 9's "(merged as `9e6239c`)" is post-merge cadence, and
        landed in this PR's `a0b5404`. The spec held no belief-precedence,
        claim or late-acknowledgement text.
      - **Normative ahead of its implementation.** No guard exists yet:
        spine B's implementing prompt owns them, and its brief carries the
        clause and the late-acknowledgement rule verbatim. Until it lands, the
        belief follows accepted commands only, and the lane announces
        contradictions (row 8).
    - **The gate, at `6bf10c7`** (the PR body has it read per step).
      - **The workspace: three windows, all VOID.** Each ended at or above
        3.0 on the run's own load: 2.38 → 4.16, 2.35 → 3.85, and 1.95 → 3.32.
        All three read 515/0/4 over 58 binaries.
      - **Accepted as the evidence of record** (the user's word of
        2026-10-16): the three agreeing voids, the clean power log, and CI's
        per-test read at `6bf10c7` (run `39002975513`).
      - **The structural note:** the workspace battery now self-voids at 3.0
        on this machine; even a start under 2.0 ended at 3.32. A local-ceiling
        ruling is owed when it next matters.
      - **The fmt miss** (CI run `39002620888` at `e6a6e29` failed `cargo fmt
        --check` on two of this PR's tests, committed before fmt ran): the
        gate worked, and `6bf10c7` is the fix.
    - **Guards, each falsified.**
      - The shared fixture `automation_rules.json` grows from 32 to 52 cases.
        It now carries `refs.scenes` and a `reason` that a refusal must
        contain, checked by both readings: the same verdict, and the same
        words.
      - `nbe-core` adds 2 named tests and `nbe-preflight`'s binary suite 2
        (6 → 8). The control plane's `automation.test.ts` adds 3 (32 → 35),
        one of them the real load path through both readers. The CI floors
        move with them.
      - The enum audit is compile-time: `automation.ts` asserts each domain is
        exactly its state type, and `streamState`'s is that type plus `held`.
      - Run on the normative machine, 2026-10-16 21:09–21:25 PDT, under
        `caffeinate`. On AC with the 45 W adapter and Low Power Mode on, so
        power-limited (§2a); the power log shows no sleep, wake or battery
        event in the window. Every run started under 2.5.
      - **F3's first preflight run is VOID**: it ended at load 4.73, above the
        3.0 ceiling. F3 was rerun whole, at 2.44–2.94.
      - Each mutation was restored from its saved copy, read `cmp`-identical,
        and was touched so that the next build rebuilt.

      | # | Guard | Mutation | What failed |
      |---|---|---|---|
      | F1 | preflight refuses a value outside the domain | the Rust check disabled (`if false && !in_domain…`) | `nbe-core`: both named tests (`held is not a recordState value: ()`; `showState from "Running" must be refused`). The fixture: all 11 out-of-domain cases accepted, the first `case34 … streamState … "reconnecting": expected valid=false, got Ok(())`. Preflight: `{ "field": "recordState", "to": "held" } must fail preflight; errors []` |
      | F2 | the control plane refuses the same | the TS check disabled | the fixture: the same 11 cases; `the domains are per field`: the recordState refusal expected, a parsed rule got; `every field …`: `expected 'string'`, `actual 'object'` |
      | F3 | per field, in Rust: no borrowed vocabulary | `recordState` reads `STREAM_STATES` too | both named tests (`held is not a recordState value`; `recordState from "held" must be refused`). Fixture cases 38 and 39 (`recordState` to `held`, from `live`) accepted. Preflight passed `recordState` to `held` (`errors []`) |
      | F4 | per field, in TS | the same borrow | fixture cases 38 and 39; both named tests |
      | F5 | `previewItem` takes a scene id, in Rust | scene ids dropped from `in_domain` | `previewItem from "S1" must read: … is outside previewItem's domain`; fixture case 44 refused; preflight's passing test: `` `armed`: … `to` "SCN" is outside previewItem's domain `` |
      | F6 | the same, in TS | scene ids dropped from `inDomain` | fixture case 44; `every field …`; the load test: `show.load must succeed`, `E_PREFLIGHT_FAILED` |
      | F7 | preflight passes the package's scene ids | `scenes: Default::default()` in `nbe-preflight`'s `main.rs` | preflight's passing test only, refusing `SCN` the same way |
      | F8 | the control plane passes the package's scene ids | `scenes: new Set<string>()` in `package.ts` | the load test only: `E_PREFLIGHT_FAILED` from the control plane's own read, `` `to` "SCN" is outside previewItem's domain ``, while preflight passed the package |
      | F9 | `reconnecting` stays struck, in Rust | added back to `STREAM_STATES` | `streamState from "reconnecting" must be refused`; fixture case 34; preflight passed `from "reconnecting"` (`errors []`) |
      | F10 | the same, in TS, and the enum audit | added back to `STREAM_STATES` | `tsc`: `automation.ts(65,7): error TS2322: Type 'true' is not assignable to type 'false'` (`streamAudit`); fixture case 34; `every field …` |
      | F11 | the enum audit: a domain is exactly its type | `"PAUSED"` added to `SHOW_STATES` | `tsc`: `automation.ts(64,7): error TS2322` (`showAudit`) |
      | F12 | both readers say the same words (`reason`) | Rust's `recordState` words reordered (`recording, idle`) | the fixture: `case38: the refusal must say "…(idle, recording)"; got "…(recording, idle)"`, and case 39; `held_is_a_stream_value_and_not_a_record_value`; preflight's needle |
  - **The witness ruling (the user's ruling of 2026-10-15, on a stop before
    the code).** Three findings from the tree:
    1. A refused or skipped output command is never acknowledged.
       `directive.rs`'s `apply` returns on the handler's `?` before
       `set_last_applied` and `ack`, so "postdates the command's ack" cannot
       fire for the cases the lane exists for.
    2. The ruled "live beside closed after a stop" cannot happen. After any
       stop the control plane reads `idle`: `stream.stop` sets it on accept,
       and `show.stop` sets it on the engine's ack. A fresh `live` beside
       `closed` is therefore either a held stop or a refused restart, since
       after a first stream the engine reports `closed`, not `none`
       (`telemetry.rs:69–80`). The publisher closes only on a stop
       (`rtmp.rs`); otherwise it redials for ever.
    3. A row was missing: `idle` beside `live` or `reconnecting`, fresh,
       after the stop was processed. That is a stop that did not take: PR
       #42's class, and R17's E1 observation.

    **The witness options, evaluated:**
    - (a) alone, any acknowledgement at or above the command's version,
      fails its defining case. In R17's dress run the next directive is the
      operator's own `stream.stop`, which sets `idle`, so the refused start
      would never alert.
    - (b) alone, an acknowledgement window, misses the family of
      contradictions that arise with no command pending, such as an engine
      restart.
    - **(c), both, ruled:**
      - (b) handles **command confirmation**. `stream.start`,
        `stream.stop`, `record.start` and `record.stop` with no exact
        acknowledgement within §5.9.5's 2000 ms window raise
        "`<command>` unacknowledged". `show.stop` is excluded: it has its own
        exact wait, and the held alert is its surface.
      - (a) handles **standing contradictions** with no command pending. It
        uses PR #44's freshness construction, with the anchor generalized to
        the more recent of the relevant command's acknowledgement and the
        connect or resync.
      - Each alert has its own id, and an episode raises once.
  - **The mapping table, as amended** (CP belief × engine report ×
    freshness × pending command; the spec carries it as §10.1.x). Each row
    is checked against the tree as it is built.
  - **The window.** `stream.start`'s acknowledgement latency is measured
    before the window is pinned: n ≥ 20, on the normative machine. The
    numbers are below, with the margin; the window is not moved without the
    user's word.

  **Spine part A, built (2026-10-15, branch `operator-signal-lane`).**
  - **The lane's shape as built.** `ControlPlaneState` carries
    `alerts: Alert[]` and `health: { auditSinkOk }`. `statusSnapshot()` copies
    both, and so the `stateChange` frame, `system.status` and `GET /status`
    carry them; `buildTick` copies the same list onto the tick. An alert is
    `{ id, severity, reason, since, auditEvent }`, for example:
    ```json
    { "id": "command.unacknowledged.stream", "severity": "warning",
      "reason": "stream.stop (stateVersion 5) was not acknowledged within 2000 ms: refused, skipped or lost; the engine's latest report reads the stream live and the control plane believes it idle",
      "since": 1792118803211, "auditEvent": "command.unacknowledged" }
    ```
    The lane lives in `alerts.ts`: `AlertLane` holds the list and writes its
    edges, and `OperatorSignals` evaluates the conditions. The server feeds it
    four ways:
    - each accepted command, through a new `DispatchDeps.onSent`, called once
      the directive is on the wire and before any `afterForward` wait;
    - each acknowledgement, from `noteApplied`, with whether it was a
      resync's;
    - each render registration;
    - a held `show.stop`, when `show.stop` returns with outputs still active.

    The evaluator runs every 250 ms, and (b)'s window is a timer per
    command. The window is §5.9.5's, the server's `showStopGraceMs`.
  - **The witness, reused from PR #44 and adapted.** PR #44's test saw only
    ticks, so it needed a fresh tick whose `ts` was more than the TTL after
    the acknowledgement. The control plane holds each engine report with its
    own receipt time, so (a) compares directly: the report must be fresh (no
    older than the TTL) and received after the anchor.
    - The anchor is the latest of the processing of the latest stream
      command or `show.stop`, the connect (a registration), and a resync's
      acknowledgement.
    - **The connect was added while building.** The restart e2e showed a new
      engine's first reports reading `none` about 1.3 s before its resync is
      acknowledged, because the resync reloads the package (row 5). They
      describe the new engine's outputs, so the connect is the anchor the
      ruling names.
  - **The window, measured before it was pinned** (`stream-start-ack.measure.ts`,
    the normative machine, 2026-10-16 02:27–02:54 UTC).
    - **Method.** n = 20, 0 void. Each sample was a fresh control plane and
      a fresh engine, so the encoder opened cold, with the dress package on
      air and `stream.start` into a local RTMP sink. Every sample started
      below a 1-minute load of 2.5 and ended at or below 2.78.
    - **Power.** On AC, 45 W adapter, Low Power Mode on (power-limited, not
      void); 100% and charged at both ends; under `caffeinate -i -s`.
    - **The tree measured** was `461a30b` with the harness's sink exported.
      The lane was written while it ran. The acknowledgement's latency is the
      engine's apply (the encoder's open), which the lane does not touch.

    | Send → `stream.start` acknowledged | median | p95 | max | min |
    |---|---:|---:|---:|---:|
    | ms (nearest rank) | 136.6 | 161.6 | 169.0 | 4.5 |

    - **The margin.** The worst sample used 169.0 ms of the 2000 ms window
      (8.4%), leaving 1831 ms. That is not thin, so the window stands at
      §5.9.5's 2000 ms. R11's sightings recorded cold opens of 250–540 ms
      under `cargo test`; these samples ran the release engine binary, and
      their acknowledgements came faster. Why is not measured here.
    - **One sample (15) was acknowledged in 4.5 ms.** That is not
      investigated, and is recorded as an unexplained outlier. It does not
      bear on the margin, which the slowest sample sets.
  - **Restart semantics, as the tree does it.** The lane lives on the state,
    and persistence writes only `stateVersion` and the manifest identity, so
    the lane is never persisted. After a control plane restart it starts
    empty:
    - a persisting condition raises again, with a new raise row;
    - an episode that outlived the process keeps its raise row and never
      gets a clear row;
    - (b)'s windows are timers, so a command accepted just before a restart is
      never confirmed or alerted. Its window dies with the process, and (a)
      covers the stream afterwards.
  - **Two readings of the ruling, built and recorded for the two-key pass:**
    1. **(b) inside the window.** A later acknowledgement arriving inside
       the window already proves the command was processed. It was refused
       or skipped, never acknowledged. So no (b) alert raises, and (a) owns
       the contradiction.
       - Raising at the window's end and clearing at once would write a
         zero-length episode.
       - `alerts-server.test.ts` pins the handoff: the probe command's
         acknowledgement clears (b), and (a) raises `stream.divergence`.
    2. **The unacknowledged-stop guard.** The ruling asked for a skip
       injected as "a success at the same version", PR #42's mutation C.
       - The tree cannot make (b) see that. An acknowledgement carries only a
         version, so a success at the stop's own version acknowledges that
         version, and (b) reads it as the stop's.
       - PR #45's dispatch pin makes two directives at one version impossible
         on the control plane's own paths, so this skip cannot recur there.
       - The guard uses the loss the tree can produce instead: a
         `stream.stop` that never reaches the engine, as a dropped connection
         loses it. That leaves the row's exact state, the stream on air while
         the control plane believes it idle, and the alert says so.
  - **Guards, each falsified.**
    - `alerts.test.ts` (21) drives the evaluator.
    - `alerts-server.test.ts` (4) runs a real server and real sockets, with
      the engine scripted.
    - `operator-signals.e2e.ts` (4) runs the real engine. On the normative
      machine three legs run and the refused start skips; on CI the refused
      start runs and the other three skip.
    - Each mutation was restored from a copy and checked `cmp`-identical,
      then green.

    | # | Guard | Mutation | What failed |
    |---|---|---|---|
    | F1 | (b) raises after the window, never before | an output command marked acknowledged at send ("ack immediately") | unit: 6 of 20, among them `(b) an output command unacknowledged within the window raises`; e2e: lost stop, `the alert raises` |
    | F2 | edge auditing: one raise, one clear per episode | the once-per-episode guard removed (a raise row per evaluation) | unit: `edge auditing: an episode is one raise row and one clear row` |
    | F3 | (a) after an engine restart | the resync's acknowledgement pushes the anchor beyond every report | unit: 6 of 20, among them `the restart contradiction` |
    | F3′ | (a) after an engine restart, on the real engine | the connect and the resync both push the anchor beyond every report: after a reconnect, wait for a command | e2e: restart, `the contradiction raises` |
    | F4b | the flicker guard: a stale stub never raises a divergence | (a) compares the tick's stub (`none` when stale) and the not-reporting check is gone | unit: the flicker guard read `+ 'stream.divergence'`, `- 'engine.notReporting'` |
    | F5 | the pre-acknowledgement witness | no postdate check | unit: `a report that predates the anchor never raises` |
    | F6 | the held alert | `held()` raises nothing | unit: `held: live beside closed after an unacknowledged show.stop is the held alert's` |
    | F7 | R18: no row for the sink's own failure | the sink alert's edges audited too | unit: `R18 … attempts no audit row for it` |
    | F8 | R18: the stderr line | the stderr write removed | server: `R18: a sink that really fails …` (`stderr: 0`, not 1) |
    | F9 | the held path, server to engine | the server never reports a held stop | server: `held: a show.stop the engine never acknowledges …`; e2e: held, `the held alert raises` |
    | F10 | (b) clears on a late exact acknowledgement | the exact ack leaves (b) standing | unit: `(b) a late acknowledgement clears the alert`, `+ 'command.unacknowledged.stream'` |
    | F11 | (b) clears on a later acknowledgement, and (a) takes over | only an exact ack proves processing | unit: `(b) a later acknowledgement proves the command was processed`; server: the unacknowledged-start test, `(a) takes the contradiction over` |
    | F12 | a resync's acknowledgement proves nothing | the resync's ack treated as a command's | unit: `(b) a resync's acknowledgement proves nothing`, `- 'command.unacknowledged.stream'` |
    | F13 | (b) clears on the operator's explicit action | the supersede removed | unit: `(b) the operator's explicit action supersedes the episode` |
    | F14 | `show.stop` is excluded from (b) | `show.stop` gets a (b) entry of its own | unit: `(b) show.stop is excluded`, `+ 'command.unacknowledged.stream'` |
    | F15 | (b)'s negative: acknowledged inside the window | the ack ignored, the window timer kept | unit: `(b) negative: acknowledged inside the window raises nothing`, `+ 'command.unacknowledged.record'` |
    | F16 | row: live beside closed | the `closed` clause removed | unit: `(a) live beside closed …`, `- 'stream.divergence'` |
    | F17 | row: idle beside live/reconnecting | the idle clause removed | unit: `(a) idle beside live or reconnecting …`, `- 'stream.divergence'` |
    | F18 | negative: reconnecting beside live | `reconnecting` added to the live clause | unit: `reconnecting beside live is §9.5's survival shape`, `+ 'stream.divergence'` |
    | F19 | negative: the consistent pairs | `closed` added to the idle clause | unit: `(a) the consistent pairs raise nothing` and the held test (after the heal, idle beside `closed` raised) |
    | F20 | negative: a stream command pending | (a) compares while one is pending | unit: `(a) negative: while a stream command is pending`, `+ 'stream.divergence'` |
    | F21 | the not-reporting alert clears when reports resume | its clear removed | unit: `the engine reporting again clears its alert`, `+ 'engine.notReporting'` |
    | F22 | the held alert heals | its clear removed | unit: the held test, `+ 'outputs.held'`; server: the held test, `the heal clears it` |
    | F23 | (b) clears when the engine goes not-reporting | the record (b) clear removed | unit: `(b) the engine going not-reporting clears the alert`, `+ 'command.unacknowledged.record'` |
    | F24 | the `lastError` pin: preflight still writes it | a passing preflight no longer clears it | server: `additive: …`, `+ 'a previous error'`, `- null` |
    | F25 | additive: every field a client reads today | `lastError` dropped from the snapshot | server: `additive: …`, `- 'lastError'`; the existing `render-channel.test.ts`: `GET /status carries all seven Section 10.4 fields`, `status is missing the §10.4 field lastError` |
    | F26 | additive: no belief moves | a (b) raise sets the stream idle | server: the unacknowledged-start test, `part A changes no belief: the control plane still believes live`, `'idle' !== 'live'` |

    - **F10–F26 were added after the gate** (a guard per clearing rule, per
      mapping row and per negative, and for `lastError` and the additive
      invariant), with one new unit test, `(b) the engine going not-reporting
      clears the alert`, the ruling's third clearing rule, which had none.
      F1–F9 were counted against 20 unit tests, F10–F26 against 21. Each of
      F10–F26 failed only the tests named, was restored from its copy, and
      read `cmp`-identical.
    - ~~**The refused-start leg's own falsification was not run.** The leg runs
      only where the engine refuses the encoder, which is CI; locally it
      skips. F1 removes the path it asserts through (`windowElapsed`, the
      same raise the lost-stop leg reads), and F1 failed the lost-stop leg
      and six unit tests. A run of F1 on CI would need a mutated commit
      pushed to a branch, and was not made.~~ **Closed by the two-key pass
      on PR #46: run on CI, run `38888813424`.** *(§2c, 2026-10-16.)*

    - **F3′ shows why waiting for a command after a reconnect is wrong.**
      After a restart the next command may be the operator's own stop, as in
      R17's dress run, and the lost stream would then never be announced.
    - **The flicker guard was strengthened.** Its first version had no report
      before going stale, so there was nothing to compare, and F4 (only the
      not-reporting check removed) proved nothing about flicker. The test
      now reads `live` while fresh and then goes stale, and F4b fails it with
      the flicker itself.
    - **The additive invariant.** `npm test` is 176 (151 existing, plus 25).
      No existing test file was edited, and automation and effects are
      unchanged.
  - **The two-key pass on PR #46** (independent, three sides; relayed by the
    user 2026-10-16). Verdict: **HOLDS on all ten required answers**, confirmed
    without adopting the builder's reasoning. The tree was clean at `4bbf409`;
    PR #46 was OPEN/MERGEABLE, nothing merged.
    - **Anchor and flicker, probed: no defect.** A fresh `none` that postdates
      the connect anchor raises `stream.divergence` inside the resync window:
      raised at +100 ms after the connect; the resync's acknowledgement at
      +1400 ms leaves it raised. `evaluate()` has no fire-and-clear path: the
      post-resync guard (`alerts.ts:276`) returns early and never clears. The
      connect anchor blocks stale pre-connect reports (F5's witness), not
      post-connect ones. The code matches the spec; no spec delta is owed.
    - **The R18 sink-independence probe (amendment 6).** With the sink broken, a
      forced (b) raise still lands in the lane: the lane is computed state and
      does not depend on the sink. Every alert writes exactly one raise row and
      one clear row per episode. `audit.sinkFailed` has `auditEvent: null`, no
      row is attempted for it, and its stderr line is written exactly once per
      episode.
    - **The refused-start leg, falsified on CI** [verified]. The throwaway
      branch `lane-falsify-ci-temp` (commit `8403d9e`, parent `4bbf409`) had
      two changes:
      - `windowElapsed` returned early (`if (this.i.windowMs > 0) return;`).
        The return was conditional so that `tsc --noEmit` stays green and the
        job reaches the e2e step.
      - The `npm test` step was set to `continue-on-error`, on that branch only.

      Results of run `38888813424`:
      - control-plane failed. `npm test` read 176 tests, 169 passed, 7 failed.
      - operator-signals: `not ok 1 - [lane] refused start …`,
        `failureType: 'testCodeFailure'`, `error: 'the alert raises'`. The leg
        killed the mutant through its own assertion, not through
        infrastructure. The other three legs skipped, as at the head.
      - rust and dress rehearsal succeeded.
      - Draft PR #47 was closed unmerged (2026-10-16T10:46:15Z), and the branch
        was deleted (absent locally and on `origin`).

      Annotation, not a dispute: this mutant kills the raise. F1 instead marks
      the command acknowledged at send. Both remove the alert the leg asserts.
    - **Blip and restart semantics, probed.** `reconnecting` beside `live`
      gives `[]`. The restart leg raised a divergence with the belief still
      `live`. The lost-stop leg shows the stream idle beside live, on air.
      Locally the e2e read 4 ran, 3 exercised, 1 skipped: the mirror image of
      CI's 4 ran, 1 exercised, 3 skipped, as designed.
    - **The additive promise, verified on every surface** (the pass's item 3). No `.rs`, `Cargo` or
      schema diff. A U6 sweep found no exact-key or `.strict()` validators on
      the tick or the snapshot, and no handler writes `state.alerts` or
      `state.health`. Automation 32/32 and effects 2/2 are unedited and green.
      The superset pin is green, and F25/F26 reproduced on the real engine.
    - **§10.1.2, clause by clause: it matches.** That covers the ids,
      severities and events; (b) with all four clears; (a)'s
      anchor/fresh/postdate; all 9 mapping rows including the negatives; the
      edge-audit shape; restart semantics; the R18 carve-out; and the additive
      clause.
      - Amendment 4 is verified: the clear is at-or-above (`alerts.ts:199`).
      - `spec.v0.4.md:2518`'s "exact" names the command's own ack, and
        `:2519–2521` generalize it. F11 proves the generalization load-bearing.
      - No spec delta is needed.
    - **The U register: the four labels the pass's report carries, all answered.**
      - **U1** (the pass's item 9): the harness hunk is an added export on
        `startRtmpSink` and nothing else. "No existing test edited" holds in
        substance, and the PR body already disclosed it.
      - **U2** (item 7): the engine binary is built before operator-signals in
        the same job, so `ran=4` is valid.
      - **U3** (item 8): 151 + 25 = 176, closed from three sides: the base
        worktree read 151/151/0, the pass's run 176/176, and CI
        `passed=176 failed=0`.
      - **U6** (item 3, "U6 sweep"): the additive sweep above.
      - ~~**U8:** the gate's shape, below.~~ *(§2c, 2026-10-16. The label "U8"
        had no source. The pass's report carries exactly four register labels:
        U1, U2, U3 and U6. The gate-honesty answer is its item 10, labelled
        "claim 10". The builder's record defines no U register, so it has no
        U8 to map. The token entered through the relay, whose own point 8 was
        the gate-honesty item. The answer stands below under its source label.)*
      - **Whether the register is fully closed cannot be confirmed from the
        builder's side.** Its definition lives in the pass's order, not in this
        record, and the labels run U1, U2, U3, U6, with gaps. Every label the
        pass's report carries is answered.
    - **Gate honesty (claim 10): HONEST** (the pass's item 10; ~~(U8)~~ §2c,
      above).
      - `fdcc071` is VOID, named with its cause.
      - The `4bbf409` rerun disclosed its mid-window sample of 3.02, and its
        scoping is explicit.
      - One minor looseness, no action: the PR body's "touches no source"
        means **no production source**. A unit test was added.
    - **The pass's own voids** (honest, non-blocking):
      1. Its machine ran hot (1-minute load ~3.4–4.0) all session, so it did
         not attempt the local re-measure of the window.
      2. It did not attempt the local dress either. The evidence of record
         stands: CI at the SHA (27/27 with `ok 24`, 511/0/4) and the builder's
         n = 20 measure.
      3. F3′'s e2e-leg variant remains builder-run only. Its mechanism is
         confirmed at unit level, and the real-engine restart leg is green.
    - **Builder's two-key record: concurs on all ten.** Two annotations, no
      disputes: the CI mutant's shape (above), and the "no production source"
      wording, accepted.
  - **A finding, out of scope.** The `system.status` *command* builds its
    snapshot with `statusSnapshot()` and no render-node argument, so it always
    reports `renderNode.connected: false`. The `stateChange` frame and
    `GET /status` pass the real one. The lane is unaffected: it lives on the
    state. Recorded, not changed.
  **Merged: PR #46 is `be7a5d8`** (the bookend, the standing pattern of §2c),
  on the user's word of 2026-10-16: a squash at 2026-10-16T13:41:01Z (parent
  `461a30b`), pinned to the reviewed head `4bbf409`. The merge tree `f85ec76`
  is identical to `4bbf409`'s. Main's CI at `be7a5d8`, run `38909746941`,
  read per test (§2b):
  - **Attempt 1.**
    - control-plane succeeded: `npm test` 176/176; automation 32/32;
      effects 2/2.
    - Every e2e floor held: slate-release 4/4, recall 2/2, take-duration
      3/3, resync-end 3/3, package-reconcile 4/4; show-stop ran 6,
      exercised 1, skipped 5; resync-request 3/3.
    - operator-signals ran 4, exercised 1, skipped 3. Its refused start
      raised at 2006 ms.
    - dress rehearsal succeeded: 27/27 with `ok 24`. DRESS-COUNTS (ci,
      `be7a5d8`): drops 1, underruns 212, fallbackTicks 0, watchdogTrips
      0, step4RiseMs 2981, step6RiseMs 2976, all inside the backstops.
    - **rust FAILED: R10, sighting 2** (Finding R10, its sighting 2 entry;
      `docs/soak-protocol.md` §5). Not the change under review: the tree
      is identical to `4bbf409`'s, whose head run `38844489887` passed
      this job, and PR #46 touched no Rust.
  - **Attempt 2** (the failed job rerun, job `117004945478`, runner
    `GitHub Actions 1000003175`): rust succeeded.
    - Workspace 511 passed, 0 failed, 4 ignored (58 binaries; the step
      echoes its summary twice).
    - `telemetry_tick_wires_the_live_session_counter ... ok`.
    - All 30 floor lines are at or above their floors.
    - The run's conclusion is success.
  - Spine part A is landed by it. Position 5 stays open: spine part B and
    work order 2 remain.
  **Merged: PR #50 is `9e6239c`** (the bookend, the standing pattern of §2c),
  on the user's word of 2026-10-16: a squash at 2026-10-17T05:21:05Z (parent
  `ac99d49`), pinned to the reviewed head `bfd5397`. The merge tree `3867fea` is
  identical to `bfd5397`'s. SPEC v0.4.8 row 9 is ratified by it; the header's
  row 9 sentence gains "(merged as `9e6239c`)", as rows 1–8 carry.

  The CI before the merge, at the head, run `39005571035`, read per test (§2b):
  rust 515/0/4, 30 of 30 floors; control-plane npm 179/179, automation 35/35,
  every e2e floor held, the refused start raised at 2016 ms; dress 27/27 with
  `ok 24`, drops 1, underruns 153, watchdog trips 0, rises 1985 and 1979 ms.

  Main's CI at `9e6239c`: run `39005941546` (push), attempt 1, read per test
  (§2b). Every job succeeded:
  - **rust** (job `117318329879`, runner `GitHub Actions 1000003199`):
    `TOTAL: 515 passed, 0 failed, 4 ignored`; 30 of 30 floor lines at or
    above floor (preflight automation 8/8, `automation_rules` 1/1);
    0 `test result: FAILED`; `telemetry_tick_wires_the_live_session_counter
    ... ok`, in the workspace step and in the RTMP floor step.
  - **control-plane** (job `117318329868`): `npm test` 179/179; automation
    35/35; effects 2/2. Every e2e floor held: slate-release 4/4, recall 2/2,
    take-duration 3/3, resync-end 3/3, package-reconcile 4/4, resync-request
    3/3; show-stop ran 6, exercised 1, skipped 5; operator-signals ran 4,
    exercised 1, skipped 3, and its refused start raised at 2016 ms.
  - **dress rehearsal** (job `117318329730`): 27/27 with `ok 24`.
    DRESS-COUNTS (ci, `9e6239c`): drops 2, underruns 177, fallbackTicks
    1 (0 outside a watchdog episode), watchdogTrips 1, step4RiseMs 2019,
    step6RiseMs 4965, all inside the backstops.
  - **R10 did not fire on main.** No armed sighting is owed. The run extends
    the envelope (Finding R10's table), and the ceiling moves to 14.50 per CPU.
  - **State and bookend accepted** (the user's word of 2026-10-16): the ceiling
    of 14.50 per CPU (six runs, one image; the drift is recorded as the fleet
    watch line); the rulings landed in `bfd5397` before the merge, with CI green
    at that head; the `equals` row named and cross-linked; the vocabulary ledger
    complete; the envelope rows staged.
  - Spine part B's spec row (SPEC v0.4.8 row 9) is landed by it. Position 5
    stays open: spine B's implementing work and work order 2 remain.
  **Merged: PR #51 is `c1fe348`** (the bookend, the standing pattern of §2c),
  on the user's word of 2026-10-17: a squash at 2026-10-17T10:40:38Z (parent
  `9e6239c`), pinned to the reviewed head `39db649`. The merge tree `c5da808`
  is identical to `39db649`'s. SPEC v0.4.8 row 10 is ratified by it, and the
  header's row 10 sentence gains "(merged as `c1fe348`)".
  - **Before the merge, at the head:** run `39014021859`, read per test
    (§2b): rust 515/0/4, 30 of 30 floors; control-plane npm 179/179,
    automation 35/35, every e2e floor held, the refused start raised at
    2009 ms; dress 27/27 with `ok 24`, drops 1, underruns 167, watchdog
    trips 0. The docs gate at `39db649`: `tsc` 0; `spec_budgets` 2/2, the
    protocol mirror 24/24, `prompt03` 10/10, npm 179/179; no voids.
  - **Main's CI at `c1fe348`, the closing record:** run `39025809315` (push),
    attempt 1, read per test (§2b). Every job succeeded.
    - **rust** (job `117376440774`, runner `GitHub Actions 1000003208`):
      `TOTAL: 515 passed, 0 failed, 4 ignored`; 30 of 30 floor lines at or
      above floor; 0 `test result: FAILED`;
      `telemetry_tick_wires_the_live_session_counter ... ok`.
    - **control-plane** (job `117376440574`): `npm test` 179/179; automation
      35/35; effects 2/2. Every e2e floor held, and operator-signals' refused
      start raised at 2018 ms.
    - **dress rehearsal** (job `117376440731`): 27/27 with `ok 24`.
      DRESS-COUNTS (ci, `c1fe348`): drops 0, underruns 150, fallbackTicks 0,
      watchdogTrips 0, step4RiseMs 1986, step6RiseMs 1998, all inside the
      backstops.
    - **R10 did not fire.** The run extends the envelope (Finding R10's
      table), and the ceiling holds at 14.50 per CPU.
  - **The user's notes on #51** (2026-10-17):
    - The pre-drafting verification was "noted with approval": row 10 rests
      on a verified foundation.
    - The docs-only gate (the spec-parsing suites and `tsc`, no voids) was
      the right gate for a docs PR.
    - The two envelope rows ride these records as owed.
  - **The superseded case is KEPT** (the user's word of 2026-10-17). "It's
    behavioral, not pedantic: expiry drops belief through the lower tiers
    before the newer command; a handoff never leaves the truth ordering."
    - **The ruling's composition, read against the tree.** The ruling says
      "it composes with the late-ack rule — the superseded command's late ack
      is logged, never reclaiming belief".
    - *Never reclaiming* holds.
    - *Logged* does not. The lane keeps only the newer command's entry per
      output (`pending`), and an acknowledgement below that entry's version
      is skipped (`alerts.ts:198–199`). No alert is cleared, so no audit row
      is written. A superseded command's late acknowledgement is therefore
      unlogged under the signed-off scoped wording ("logged where the
      alert-clear writes its audit row"). It is the same unlogged class as
      the `engine.notReporting` case.
    - Recorded, not changed. Whether unlogged late acknowledgements are now
      named a hardening candidate (beside engine refusal-acks) is put to the
      user.
  - Spine part B's belief row (SPEC v0.4.8 row 10) is landed by it. Position
    5 stays open: spine B's implementing work (its brief drafted next, held
    for review) and work order 2 remain.
  - **Spine B's implementing brief, drafted 2026-10-17:**
    `agents/prompts/r17-spine-b-belief.md`. It is **held for the user's
    review before dispatch** ("nothing implements unread").
    - It carries §10.1.3's precision clause and late-acknowledgement rule
      verbatim, both checked byte for byte against the signed-off texts and
      the ratified spec.
    - It states what the tree does today, verified at `c1fe348`: the hold
      at the prior state; belief set only by commands; every reader of the
      belief values; the detection-transaction precedent; the tests that
      move.
    - It names six decisions owed to the user before dispatch: D1, the
      report → belief mapping; D2, `held` against a fresh report; D3, `held`
      in the preconditions and readers (`stream.stop` refuses "stream not
      live", the held stop's own heal path); D4, how a correction lands; D5,
      its own spec row for §5.9.5's "prior state"; D6, the dress rehearsal's
      `stream.stop` race after a refused start.
  - **The user's rulings on the brief and D1–D6, 2026-10-17. The brief is
    APPROVED with these answers folded in; dispatch is the user's word.**
    The rulings are quoted in full in the brief's §2.
    - **D1:** the report → belief mapping, RATIFIED as proposed (`live` and
      `reconnecting` → `live`; `none` and `closed` → `idle`). The table lands
      in row 11.
    - **D2:** `held` gives way to a fresh report on the precision clause's
      schedule. The held alert persists until the real acknowledgement.
    - **D3:** `held` is output-active. `outputsActive()` includes it,
      `stream.stop` accepts it, `stream.start` refuses it by name, and every
      consumer is exhaustive.
    - **D4:** a correction lands like `itemEvent` (one bump, one
      `belief.corrected` row, no frame). Automation fires on command-path
      transitions only, never on corrections.
    - **D5:** row 11 is required, its candidate drafted with the design and
      reviewed before dispatch.
    - **D6:** the dress waits out the correction to `idle` (bounded), then
      asserts `stream.stop` refuses: the race becomes a falsification of D3.
  - **Unlogged late acknowledgements are a named hardening candidate**
    (2026-10-17), beside engine refusal-acks: "same class, terminal
    outcomes dropped on the floor". The current behaviour is correct under
    the ratified semantics, and the evidence gap is recorded, not patched.
  - **Read against the tree, with the fold** (recorded, not changed):
    - **D2a, OPEN, owed before dispatch.** D2 and D3 together can strand
      the held alert. If `show.stop`'s acknowledgement never comes (withheld
      after a failed quiesce, or lost), a fresh `closed` corrects
      `held → idle`, and `stream.stop` then refuses `idle`, so no real
      acknowledgement can arrive. Two questions are the user's: which
      acknowledgements count, and what clears the alert when none can come.
      Today the alert heals when the belief reads outputs inactive
      (`alerts.ts:267`, row 8's F22). §10.1.2 states no clearing rule for
      `outputs.held`, so row 11 states the new one.
    - **D3's "#50's compile-time enum audit enforces exhaustiveness" is
      narrower in the tree.** The audit pins each domain to its type; it
      does not make the consumers exhaustive. The brief requires exhaustive
      `switch`es with a `never` default in every consumer, so `tsc`
      enforces both.
    - **D4 departs from the `itemEvent` precedent on automation, by
      design.** `itemEvent` fires automation's `stateChange`s
      (`server.ts:924`); a correction will not. As a consequence, a refused
      start corrected `live → idle` fires no `from: "live"` rule.
    - **A dress-rehearsal sighting on PR #52, recorded 2026-10-17** (not
      the change under review: `cfa251b` touched only the brief and the
      prompt map). CI run `39031255945` attempt 1 failed the dress job:
      `not ok 10 - [RI-1] step 8: preview.set is visible in telemetry`,
      `preview.set must succeed; got {"code":"E_FORBIDDEN_STATE","message":"item A1 is DONE"}`.
      - **The mechanism, read from the tree.** A1 is timed:
        `durationFrames: 150` at 30 fps, so 5 s
        (`tests/fixtures/dress_show/manifest.json`). Step 4 takes A1 and
        waits for the clip bus to rise, and this run's rise took **6025 ms**
        (`step4RiseMs`; the previous high was 4965 ms, at step 6). So A1 ran
        to its end and went `DONE` during step 4. Step 5's take of A2
        resets only a `LIVE` or `PLAYING` item to `READY` (`state.ts:291`),
        so A1 stayed `DONE`, and step 8's `preview.set A1` was refused.
      - **The class is R13's late rise**, cascading into step 8: any step-4
        rise above A1's 5000 ms does it. Step 4's rise is inside its 10 s
        backstop, but step 8 depends on A1 not ending, and nothing asserts
        that.
      - **The rerun** (only the failed job, disclosed): attempt 2's dress job `117394593348` passed, 27/27 with `ok 10`. Its `step4RiseMs` was 2979, under A1's 5000 ms, as the mechanism predicts. Its DRESS-COUNTS (ci, `cfa251b`, attempt 2): drops 0, underruns **231**. That is the first at-gate reading above R12's previous peak of 230. R12's trend rule (a) needs three consecutive bookends above the peak, so it is watched, not opened.**
      - ~~**Put to the user:** whether to file this as a register entry (R19:
        dress step 8 coupled to A1's duration through step 4's rise), and
        whether the fix is the test's (say, step 8 previews an untimed item,
        or resets A1) rather than the product's.~~ **FILED as R19** (the
        user's word of 2026-10-17; §2c), with its row in
        `docs/soak-protocol.md` §5. "Dress step 8 races A1's completion when
        step-4's rise exceeds A1's duration. Test-design flaw, not a product
        defect."
        - **The mechanism, recorded:** both rise numbers, 6025 ms (attempt 1)
          and 2979 ms (attempt 2); A1's `DONE` persisting through step 5's
          take (`state.ts:291`).
        - **No load data:** the dress job carries no load logging, since
          R10's `[ci-load]` capture is the rust job's only. "Why slow" had no
          instrumented answer.
        - **The fix is its own test-only PR:** step 8 previews an untimed
          item, with a comment naming what the step rehearses.
          Falsification: inject a slow step-4 rise and show that the new
          step 8 passes where the old one raced. It lands before the
          implementing prompt's dispatch.
        - **R12: watched, not triggered** (the user's word). The 231 came on
          a rerun: a selected sample, not a fresh draw.
    - **D2a, ANSWERED 2026-10-17.** The `outputs.held` clearing rule, the
      user's text verbatim (folded into the brief's Appendix A as
      §10.1.2's new clearing rule). The brief is now dispatch-ready;
      dispatch is the user's word:

      > D2a, amended — the outputs.held clearing rule (both recommendations adopted):
      >
      > Part 1 — the real ack: the command's own late acknowledgement, or the acknowledgement of a later stop at-or-above — including the operator's stream.stop or record.stop from held. The engine acks a stop only while that output is still live, so either ack witnesses an actual stop.
      >
      > Part 2 — when no ack can come: the next accepted show.load or show.unload clears the alert, with a clear row that names the cause. Until then, it stands. The lane is not persisted, so a control-plane restart also clears it, as it clears everything.
      >
      > The relay's earlier option (iii) — a fresh report clearing the alert — is WITHDRAWN. Corrections still move belief (D2 unchanged); they do not clear alerts. Two failure modes: with the engine quiesced, no report ever comes and the clear never fires; with the engine healthy, a single transient none would correct held → idle and clear the alert while the stream stays live against the operator's intent — the witness dying to a blip. Belief answers "what is true"; the alert answers "is a command outcome unresolved". This rule keeps them separate.
      >
      > Amended clearing set for outputs.held: (i) the command's own ack; (ii) a later stop's ack at-or-above; (iii) an accepted show.load or show.unload, cause named in the clear row; (iv) control-plane restart, as for the whole lane. It stands in every other case — persistence is the witness working.

      Read against the tree (recorded, not changed):
      - The premise holds: the engine refuses `stream.stop` and
        `record.stop` unless that output is live (`directive.rs`).
      - (i) needs the late `show.stop` acknowledgement tracked, since
        `held()` drops it today (`alerts.ts:298`).
      - (iv) writes no clear row (row 8's restart semantics). *Accepted
        per row 8 (the user's word of 2026-10-17).*
      - The withdrawn option (iii) was the builder's.
      - **Path (i)'s tracking is required brief content** (the user's word
        of 2026-10-17). `held()` drops the pending entry at `alerts.ts:298`,
        so the late-acknowledgement test fails without the tracking. That
        is its falsification. The anti-clear set stands as ruled.
  - **The sequence (the user's word of 2026-10-17):**
    1. #52 merges on the user's word.
    2. R19's test-only fix PR follows.
    3. The implementing prompt dispatches after R19 lands.
      - Today's heal on outputs inactive (`alerts.ts:267`) is replaced.
    - **D5's placement, the builder's reading.** Row 11's candidate is
      Appendix A of the brief, reviewed before dispatch. It lands in the
      spec with the implementing PR and is ratified on that merge, not
      ahead of the code: unlike row 10, it amends passages the tree builds
      today (`show.stop`'s prior-state hold, §16's preconditions).
  - **Confirmed by the user, 2026-10-17:**
    - the D2/D4 sequence as built;
    - D3's two-layer enforcement;
    - D4's divergence from `itemEvent`, as intended;
    - row 11 reviewed as Appendix A now, landing and ratifying on the
      implementing PR, with its text as an early commit and
      ratification-on-merge stated in the body.

    "Ceiling 12.09 recorded; the downward move is the mechanism working."
    ~~**D2a is still open.**~~ The user's message asked to "fold the D2a
    clearing rule into Appendix A verbatim", but no D2a rule text had been
    given, so nothing was folded. *(§2c: answered the same day, below.)* The tree fact
    for the answer: the engine refuses `stream.stop` unless its stream is
    live (`directive.rs`, `on_stream_stop`), so once the engine has quiesced
    no output command can produce the real acknowledgement.
  **Merged: PR #52 is `df5530d`** (the bookend, the standing pattern of §2c),
  on the user's word of 2026-10-17: a squash at 2026-10-17T12:48:28Z (parent
  `c1fe348`), pinned to the reviewed head `79d256e`. The merge tree `32a7f0e`
  is identical to `79d256e`'s. The brief (`agents/prompts/r17-spine-b-belief.md`)
  is landed, approved; it dispatches after R19's fix lands (the sequence).
  - **ORDER 1.1, the handling used: the docs-only precedent.** The head's run
    `39033038784` failed the dress at step 8 on attempt 1 (R19's second
    sighting, below). Since `75e5ce7`'s run `39032691149`, green per test
    (rust 515/0/4, 30/30 floors; npm 179/179; dress 27/27), the head carried
    0 code lines (records and the brief only). The dress job was rerun
    job-scoped and explained, and was green on attempt 2 (27/27, `ok 10`,
    `step4RiseMs` 992); rust 515/0/4 and npm 179/179 on attempt 1.
  - **Main's CI at `df5530d`, the closing record:** run `39033994454` (push),
    attempt 1, read per test (§2b). Every job succeeded.
    - **rust:** `TOTAL: 515 passed, 0 failed, 4 ignored`; 30 of 30 floors;
      `telemetry_tick_wires_the_live_session_counter ... ok`.
    - **control-plane:** `npm test` 179/179; automation 35/35; effects 2/2;
      every e2e floor held; the refused start raised at 2022 ms.
    - **dress:** 27/27 with `ok 24`; DRESS-COUNTS (ci, `df5530d`): drops 1,
      underruns 213, watchdog trips 0, rises 1989 and 3005 ms.
    - **R10 did not fire.** The ceiling holds at 12.09 per CPU.
  - **R19's second sighting**, at `79d256e` (docs only, not the change): the
    same signature, `preview.set must succeed; got
    {"code":"E_FORBIDDEN_STATE","message":"item A1 is DONE"}`, with
    `step4RiseMs` **4993**, under A1's 5000 ms. The race is therefore not
    "the rise exceeds 5 s". A1's 5 s end runs on the engine's clock from its
    take, against step 5's take of A2. Step 4's rise (polled on 1 s ticks)
    plus step 5's `preview.set` round trip sits inside that window, so a
    rise just under 5 s also loses. The fix PR's body answers R19's open
    question with it.
  **Merged: PR #53 is `49d29ae`** (the bookend, the standing pattern of §2c),
  on the user's word of 2026-10-17 (the three orders, ORDER 2): a squash at
  2026-10-17T13:07:58Z (parent `df5530d`), pinned to the reviewed head
  `18a3858`. The merge tree `bf245bf` is identical to `18a3858`'s.
  **R19 is fixed:** the dress rehearsal's step 8 stages G, untimed, not A1.
  The fix is test-only.
  - **The falsification** (in the PR body, the normative machine,
    2026-10-17). An injection held step 4 until 5.5 s after its take:
    - the old step 8 failed 26/27 with CI's exact signature;
    - the new step 8 passed 27/27 under the same injection, and 27/27
      without it;
    - there were no voids, every restore was `cmp`-identical, and `tsc`
      exited 0.
  - **R19's open question, answered in the body.** The 5000 ms is no gate
    but A1's fixture duration: step 4 asserts only the 10 s backstop on CI,
    and 3000 ms on the normative machine. The race runs from A1's take to
    step 5's take, and step 8 is the first step whose precondition reads A1.
  - **A finding outside the fix's scope, recorded, not fixed.** Step 11
    carries the same coupling. At `df5530d` it takes A1 (`:1377`), takes A2
    (`:1463`) after the recording starts, a 2 s sleep and a marker, then
    stages A1 again (`:1485`): roughly 4–5 s against A1's 5 s. It is reached
    only where `record.start` is not refused (on CI the encoder refusal
    skips the step first). Whether it gets the same treatment is the
    user's word.
  - **Before the merge, at the head:** run `39034929811`, read per test.
    - rust 515/0/4, 30/30 floors.
    - control-plane npm 179/179, automation 35/35, every e2e floor held.
    - dress 27/27 with `ok 10` (G), underruns **232**. That is a fresh draw
      above R12's peak of 230 (the 231 was a rerun sample). It is watched,
      not triggered: R12's rule (a) wants three consecutive bookends.
  - **Main's CI at `49d29ae`, the closing record:** run `39035305838` (push),
    attempt 1, read per test.
    - rust 515/0/4, 30/30 floors.
    - control-plane npm 179/179, automation 35/35, every e2e floor held, the
      refused start raised at 2019 ms.
    - dress 27/27 with `ok 10` (G), drops 1, underruns 184, rises 995 and
      960 ms.
    - **R10 did not fire.** The ceiling holds at 12.09.
  - **R19 leaves the register,** struck per its own exit condition ("the
    entry leaves the list when that fix lands and is falsified"), citing
    the falsification above.

  **Spine part B's implementation: the design, written first** (ORDER 3,
  the user's word of 2026-10-17: "After ORDER 2 lands, branch the
  implementing PR off main and begin per the brief"; branch
  `r17-spine-b-impl`, off `49d29ae`). The brief is
  `agents/prompts/r17-spine-b-belief.md`; ORDER 3 named it
  `r17-spine-b-brief.md`, a name the tree does not have. Row 11's text
  landed as an early commit (`2264e56`).
  - **`StreamState` gains `held`** (`state.ts:17`), and #50's audit pins the
    domain to the type with no exception (`streamAudit`). `streamActive()`
    (`state.ts`) is the one exhaustive switch over the stream belief that
    `outputsActive()` reads, so `held` is output-active (D3).
  - **The consumer list (D3)**, from a full read of `streamState` in the
    control plane's source:
    - **Value-branching, each an exhaustive `switch` with an `unreachable`
      default:** `streamActive()` (read by `outputsActive()`, and through it
      by `server.ts`'s held raise and `show.stop`'s `outputsWereActive`);
      `stream.start` (refuses `live` and, by name, `held`); `stream.stop`
      (accepts `live` and `held`, refuses `idle` by name); `show.stop`'s
      unacknowledged path (`live` → `held`); the lane's (a) divergence
      (`held` contradicts nothing: the held alert announces it and a fresh
      report corrects it).
    - **Pass-through, not value-branching:** the status snapshot
      (`state.ts:552`), the tick (`telemetry.ts:138`), automation's snapshot
      (`automation.ts:350`), and two reason strings in the lane.
  - **The claim and its expiry (§10.1.3).** A stream claim is live while the
    lane's pending `stream.*` entry is unprocessed and unexpired, or while
    `show.stop` is inside its window. `windowElapsed` marks `expiredAt`
    before any other effect. For the stream, it moves the anchor to the
    expiry, so only a report that postdates it may correct; then (b) raises
    where it does, so the clause holds. `held()`, which runs when
    `show.stop`'s wait ends unacknowledged, moves the anchor the same way.
    A superseded entry and a confirmed one never reach the expiry.
  - **The correction (§10.1.4).** `OperatorSignals.evaluate()` now runs (a)
    as before (`divergence()`, row 8 unchanged), then `correction()`. With
    no live claim and a fresh report postdating the anchor, a mapped token
    that differs from the belief calls the server's `correct`.
    - The server lands it like `itemEvent`: it writes the belief, calls
      `state.bump()` and `persistence.onDirty()`, and writes one audit row.
    - The row is kind `command`, `command: "engine:report"`,
      `event: "belief.corrected"`, with `detail` `{ field, from, to, report,
      reportAgeMs }`.
    - There is no `stateChange` frame and no `automation.fire` (D4).
  - **The held alert's clearing (D2a).**
    - The old heal on outputs inactive is struck (§2c in `alerts.ts`).
    - **(i)** `heldStopVersion` survives the hold, and its exact
      acknowledgement clears the alert.
    - **(ii)** A later stop's own exact acknowledgement (`stream.stop`,
      `record.stop` or `show.stop`) at or above it clears the alert.
    - **(iii)** An accepted `show.load` or `show.unload` clears it in
      `onAccepted`, with the cause named. Dispatch's `onSent` fires for every
      accepted command (`dispatch.ts:400`), with or without a render node.
    - **(iv)** A restart empties the lane.
    - A correction clears nothing.
  - **Findings so far, recorded, not changed:**
    1. **The correction pre-empts (a)'s held announcement.** A contradiction
       with no claim live (an engine restart dropping a live stream, say)
       still raises `stream.divergence` in the evaluation that sees it. The
       correction then moves the belief, and the next evaluation (250 ms
       later) clears the alert as "consistent again". So row 8's critical
       alert becomes a one-evaluation episode, and the trail is its two
       edge rows and `belief.corrected`. Whether a correction against the
       last command's intent should hold an alert until the operator's next
       command is the user's word.
    2. **"Held" has a third meaning in the spec:** the show clock's `HELD`
       ("operator freeze, emergency", ~~§16's~~ §11.4's clock-state table *(§2c, 2026-10-18: §11.4 is the section the tree has; the table has stood since v0.1)*), beside
       §13.5's automation hold and the output's `held`.
    3. **A `stream.stop` from `held` after the engine quiesced is refused
       engine-side** (`directive.rs`, `on_stream_stop`), so (b) raises
       "stream.stop unacknowledged" at its window's end. The belief reads
       `idle` from the accept, and the held alert stands until the show's
       lifecycle, as D2a rules. Consistent, and noisy.

  **Spine part B's implementation: the build, and every guard falsified**
  (ORDER 3, branch `r17-spine-b-impl`).
  - **Commits, in the discipline's order** (green before falsify, falsify
    before push):
    - `3a3ddfc`: the code, the tests, and the belief suite's two CI floors
      (ran ≥ 4, exercised ≥ 4). It went green before any mutant ran.
    - `1e5bfe4`: five unit guards hardened. The battery found them vacuous
      (B17, B18, B20, B21 and B22 below).
    - `98d4ba0`: messages on three bare `assert.ok` calls. B13b found that a
      failing bare call hangs node for 22 minutes (below).
  - **D1–D6 as built:** as the design record above states, with no
    departure. The correction lands through the server's `correct` (D4).
    The held path writes `held` on `show.stop`'s own command path (D3).
    D2a's four paths each have their own guard (B1–B5), and the anti-clear
    set has one per member (B6–B9).
  - **Findings 4–7, recorded, not changed:**
    4. **The brief contradicts D4 on automation.** §5's work item 5 and
       §6's automation bullet say a `from: "live"` rule "fires once on a
       correction live→idle". D4 (ratified), §6's own D4 bullet ("a
       `from: "live"` rule stays silent on a refused start's correction")
       and §7 ("Fire an automation `stateChange` on a correction") say it
       fires nothing. Built per D4. B29a guards it. The brief's two lines
       are the user's to strike.
    5. **Placement.** §6 names `automation.test.ts` for the transition
       tests. They are in the new `belief.test.ts`, on a real server and
       the real command path, with automation's own audit rows as the
       witness, because the rules' `marker.add` actions are refused once
       the show is `STOPPED`. `automation.test.ts` is unchanged, and its
       floor (35) holds.
    6. **Two freshness guards in series.** The correction's own freshness
       check is shadowed by `evaluate()`'s not-reporting return, and the
       reverse also holds. Each mutant alone survives as an equivalent
       mutant (B13: 0 of 43 fail). Both together are killed (B13b). The
       check stays, as defence in depth.
    7. **A bare `assert.ok` hangs on failure.** Under B13b the
       not-reporting test's first, bare `assert.ok` failed. Node then
       rebuilt the expression from tsx's transformed single-line source
       (`alerts.test.ts:5:14011`). That ran for 1 313 s at 100 % CPU, the
       process sampled in `ErrorUtils::Construct` and `RegExpSplit`, before
       it printed `false == true`. It reproduces in isolation: a 60 s
       bound timed out. The run's window was voided on its own load.
       `98d4ba0` gives each new bare call a message. Row 8's bare calls
       are untouched (scope).
  - **D6, falsified on CI** (the PR #47 pattern). The refused start runs
    only where the engine refuses `stream.start`, and CI's macos-14 has no
    hardware encoder, so CI is the honest witness. There were two throwaway
    branches on `3a3ddfc`, as draft PRs. Only the dress job ran: the rust
    and control-plane jobs were `if: false` on those branches only. Each
    mutant was killed by step 9's own assertion, in the refused branch (the
    log reads `SKIP stream step: engine refused stream.start`), with 26
    passed and 1 failed:
    - **#54, `cc65ac6`, run `39101919002`** (dress job `117599769553`):
      the correction lands nothing (`correction()` returns at entry,
      conditionally, so `tsc` stays green). Result: `not ok 11 - [P10] step
      9` / `the belief corrects to idle within 4000 ms of the refused start
      (§10.1.4)` / `'live' !== 'idle'` (`dress-rehearsal.test.ts:931`).
    - **#55, `380ebf9`, run `39101921648`** (dress job `117599776044`):
      `stream.stop` accepts `idle`. The bounded wait passed. Result:
      `not ok 11` / `D3's precondition: from idle there is nothing to stop`,
      actual `['ok', undefined]`, expected
      `['error', { code: 'E_FORBIDDEN_STATE', message: 'stream is idle' }]`.
    - Both PRs were closed unmerged (05:53:15Z and 05:53:18Z). Their
      branches were deleted locally and on `origin`.
  - **The local battery** (normative machine, 2026-10-17 22:52–23:54, on AC
    under `caffeinate`). Each mutant was applied from an exact match,
    `tsc --noEmit` was run on it (exit 0 for all but B23), the named suites
    were run under the load rule, and the file was restored from its saved
    copy, checked `cmp`-identical and `git`-clean. "Unit" is
    `alerts.test.ts` + `alerts-server.test.ts` + `belief.test.ts`, 43
    tests. The first round ran on `3a3ddfc`'s tests. The rows marked † ran
    again on `1e5bfe4` after the hardening, and B13b on `98d4ba0`. The
    hardening only added assertions and moved two reports past an earlier
    anchor.

    | # | Guard | Mutation | What failed |
    |---|---|---|---|
    | B1 | D2a (i): the held stop's own late ack clears it | path (i)'s clear never fires | unit 2/43: `D2a (i): the held stop's own acknowledgement, however late, clears it`; `held: … only a real acknowledgement clears it` |
    | B2 | D2a (i)'s tracking (the brief's required falsification) | `held()` drops the stop's version again (`heldStopVersion = null`, today's `alerts.ts:298` behaviour) | unit 4/43: `D2a (i)`, `held: … only a real acknowledgement`, `D2a (ii)`, and server `held: …` / `a later stop's own acknowledgement clears it (path ii)` (path (ii) needs the tracked version too) |
    | B3 | D2a (ii): a later stop's own ack at or above | path (ii)'s clear never fires | unit 2/43: `D2a (ii)`; server `held: …` (`a later stop's own acknowledgement clears it (path ii)`) |
    | B4 | D2a (iii): an accepted `show.load`/`show.unload` | its clear removed | unit 1/43: `D2a (iii)`; e2e held leg (rerun †, the first window void): `show.unload clears it` |
    | B5 | D2a (iv): a restart empties the lane | the lane persisted (`alerts` in the snapshot, restored) | unit 1/43: server `held, path (iv)` / `the restarted lane is empty` |
    | B6 | anti-clear: a correction | the correction clears `outputs.held` | unit 4/43: the anti-clear test at `a correction moves the belief and clears no alert`; `held gives way …` / `the correction clears no alert`; `held: … only a real acknowledgement`; `§13.2 …` (`and clears no alert`); e2e held leg: `the held alert stands: persistence is the witness working` |
    | B7 | anti-clear: a transient `none` | a fresh `none` clears it | unit 1/43: the anti-clear test at `a transient none does not clear it` |
    | B8 | anti-clear: a refused stop | a stop's acceptance clears it | unit 3/43: the anti-clear test at `a refused stop, never acknowledged, does not clear it`; `D2a (ii)` (`processed is not witnessed`); server `held: …`, its clear row's cause no longer `record.stop acknowledged …: the stop is witnessed` |
    | B9 | path (ii) needs the stop's exact ack | `entry.exact` dropped | unit 2/43: the anti-clear test at `persistence is the witness working`; `D2a (ii)` |
    | B10 | the struck heal stays struck | the heal on outputs inactive restored | unit 2/43: `held: … only a real acknowledgement` (`the belief alone does not clear the held alert`); `§13.2 …` (`and clears no alert`) |
    | B11 | D1's mapping row by row | `reconnecting` maps to `idle` | unit 1/43 (rerun †, the first window void): `D1: …` / `reconnecting beside a idle belief` |
    | B12 | the anchor: a predating report never corrects | the postdate check removed | unit 3/43: `D1: a stale report … predates the anchor` (`a report that does not postdate the anchor`); `claim: window expiry …`; `held gives way …` (`a report that predates the expiry does not rule`) |
    | B13 | freshness (one guard) | the correction reads the raw report | **survived, 0/43: an equivalent mutant** (finding 6) |
    | B13b | freshness (both guards) | that, and `evaluate()`'s not-reporting return removed | unit 4/43 (rerun on `98d4ba0`, the first window void, finding 7): `D1: …` / `a stale report`; `engine.notReporting owning …` / `the engine is not reporting`; row 8's `(b) the engine going not-reporting clears the alert` and `the flicker guard` |
    | B14 | claim end: its own ack | the claim survives its acknowledgement | unit 3/43: `claim: … until its own acknowledgement` (`a report after the acknowledgement rules`); `claim: a later acknowledgement …`; `§13.2 …` |
    | B15 | claim end: a later ack (Amendment 4) | only an exact ack ends it | unit 1/43: `claim: a later acknowledgement ends it too` |
    | B16 | claim end: window expiry | the claim survives its expiry | unit 4/43: `claim: window expiry …` (`the next report rules`); `late-ack rule …`; `D4 …`; server `an unacknowledged stream.start raises …` (`row 11: the expired claim gives way to the fresh report`) |
    | B17 † | the expiry moves the anchor | that move removed | first round **survived** (vacuous: the report shared the registration's millisecond). Hardened: unit 1/43, `claim: window expiry …` / `a report that predates the expiry does not rule` |
    | B18 † | claim end: supersession (the superseded never expires) | the superseded entry keeps its timer, and the guard is gone | first round **survived** (vacuous: the test ended inside the start's window). Hardened: unit 1/43, `claim: a newer command …` / `the superseded start never expires: its window passes and raises nothing` |
    | B19 | the late-ack rule: inert to belief | a late exact ack reclaims the belief | unit 1/43: `late-ack rule …` / `the dead claim cannot reclaim the belief` |
    | B20 † | `engine.notReporting` owning the window's end: the claim still expires | the not-reporting return moved above the expiry | first round **survived** (vacuous: reports never resumed). Hardened: unit 1/43, `engine.notReporting owning …` / `the claim expired under not-reporting: the next fresh report rules` |
    | B21 † | the hold moves the anchor | `held()`'s anchor move removed | first round **survived** (vacuous: as B17). Hardened: unit 1/43, `held gives way …` / `a report that predates the expiry does not rule` |
    | B22 † | (a): `held` contradicts nothing | `held` beside `closed` is divergent | first round **survived** (vacuous: the held alert owns (a) while it stands). Hardened: unit 1/43, `D2a (iii)` / `held is no divergence` |
    | B23 | D3: every `never` default | a fourth `StreamState` value, `"paused"` | `tsc` exit 2, six errors, one per consumer: `alerts.ts(379,21)`, `commands/output.ts(46,30)` and `(66,30)`, `commands/show.ts(187,29)`, `state.ts(40,26)`: `Argument of type '"paused"' is not assignable to parameter of type 'never'`; and #50's audit, `automation.ts(68,7)`: `Type 'true' is not assignable to type 'false'` |
    | B25 | D3: `stream.start` refuses `held` by name | `held` accepted | unit 1/43: `D3: stream.start refuses held by name …` |
    | B26 | D3: `stream.stop` accepts `held` | `held` refused | unit 1/43: `D3 …` / `stream.stop is the operator's way out of a hold` |
    | B27 | D3: `held` is output-active | `streamActive("held")` false | unit 2/43: `D3 …` / `held is output-active`; `§13.2 …` |
    | B28 | §5.9.5: the hold on the command's path | an unacknowledged `show.stop` leaves `live` | unit 2/43: `D3 and §5.9.5 …`; `§13.2 …` (`live→held, once each`); e2e held leg / `§5.9.5's limbo, on the command's path`; show-stop-version held leg (rerun †, the first window void): `the held leg` |
    | B31 | no alias of `held` to `idle` | the hold writes `idle` | unit 2/43: `D3 and §5.9.5 …`; `§13.2 …` (`live→held, once each; to:"idle" does not match a hold`); e2e held leg / `§5.9.5's limbo, on the command's path` |
    | B29a | D4: no automation on a correction | the correction fires `stateChanges` as `itemEvent` does | unit 2/43: `D4 …` (`one bump`: a rule's `marker.add` landed); `§13.2 …` / `the correction held→idle fires nothing` |
    | B29b | D4: no `stateChange` frame | the correction broadcasts one | unit 1/43: `D4 …` / `no stateChange frame: §5.4.1 is one per accepted command` |
    | B30 | D4: one bump | two bumps | unit 1/43: `D4 …` / `one bump` |
    | D6-W | the dress's bounded wait (CI) | the correction lands nothing | #54 above |
    | D6-R | the dress's idle refusal (CI) | `stream.stop` accepts `idle` | #55 above |

    (There is no B24. Its planned mutant, a dropped `case`, is the same
    `tsc` failure as B23's at one consumer.)
  - **Voids, each named with its cause.** The rule is a 1-min load ≥ 3.0
    at either end of a timed window. Each void was followed by a
    job-scoped rerun of that step alone:
    - B4's e2e (22:57:56–22:58:22, 2.39 → 3.88). The rerun ran
      23:40:19–23:40:45 (2.40 → 2.09) and was killed.
    - B11's unit (23:01:36–23:01:39, 2.29 → 3.39). The rerun ran
      23:40:49–23:40:52 (2.08 → 2.08) and was killed.
    - B28's show-stop-version e2e (23:08:30–23:09:44, 2.32 → 3.49). The
      rerun ran 23:41:27–23:42:39 (2.32 → 2.25) and was killed.
    - B13b's unit (23:14:12–23:36:08, 2.39 → 3.63). The mutant's own
      22-minute spin caused this one (finding 7). The rerun on `98d4ba0`
      ran 23:53:25–23:53:28 (2.24 → 2.46) and was killed.
    - The other three were ambient. The machine's top processes were
      iTerm2, WindowServer, CleanMyMac's monitor and `opencode`.
    - The voided runs had already failed the same way their reruns did.
      They are not counted as evidence.
  - **`npm test`: three voids, cause not isolated.**
    - The voided runs, all 197/197:
      - 23:48:13–23:48:21 (2.42 → 5.38);
      - 23:51:16–23:51:23 (2.39 → 5.05), a job-scoped rerun of the first;
      - the gate's, 23:56:43–23:56:50 (2.47 → 5.94).
    - Each started under 2.5, and the 1-min load rose by about 2.6–3.5
      within its own 7–8 s.
    - Earlier tonight the same suite ended at 2.53 (22:45:53) and 2.65
      (23:12:56).
    - A diagnostic followed, declared beforehand as not evidence: it started
      at 2.65, outside the start rule. Sampled each second, it peaked at 16
      `node` processes, the suite's ~29 files in parallel, and the load went
      2.65 → 2.40 with no rise.
    - So the suite's parallelism is real, but it does not by itself make
      the rise. I cannot attribute the rise further. The top processes were
      iTerm2, WindowServer and `opencode`, with no job of mine besides the
      suite.
    - No rerun is counted. The local `npm test` evidence is the clean
      windows of 22:45 and 23:12 (on `3a3ddfc` and `1e5bfe4`'s tests) plus
      these voided-but-green runs, and CI's control-plane job is the
      load-independent read.
  - **The gate** (normative machine, 2026-10-17 23:53 – 2026-10-18 00:06, at
    `98d4ba0`, on AC under `caffeinate`, with no sleep, wake or battery
    event). No Rust source changed: `git diff main...HEAD -- crates
    Cargo.toml Cargo.lock` is empty. The two builds only confirm the
    binaries are current, and the builds and `tsc` are untimed.

    | Step | Window | Load (start → end) | Result |
    |---|---|---|---|
    | build (release, debug); `tsc` | 23:53:46–23:54:28 | untimed | exit 0 |
    | `npm test` | 23:56:43–23:56:50 | 2.47 → 5.94, **VOID** | 197/197 |
    | automation | 00:01:36–00:01:39 | 2.40 → 2.40 | 35/35 |
    | effects | 00:01:39–00:01:40 | 2.40 → 2.45 | 2/2 |
    | belief | 00:01:40–00:01:42 | 2.45 → 2.45 | 4/4 |
    | e2e slate-release | 00:01:42–00:01:55 | 2.45 → 2.27 | 4/4 |
    | e2e recall | 00:01:55–00:02:08 | 2.27 → 2.45 | 2/2 |
    | e2e take-duration | 00:02:08–00:02:13 | 2.45 → 2.42 | 3/3 |
    | e2e resync-end | 00:02:13–00:02:27 | 2.42 → 2.25 | 3/3 |
    | e2e package-reconcile | 00:02:27–00:02:41 | 2.25 → 2.21 | 4/4 |
    | e2e show-stop-version | 00:02:41–00:03:55 | 2.21 → 2.86 | 6/6 |
    | e2e resync-request | 00:04:26–00:04:52 | 2.40 → 2.07 | 3/3 |
    | e2e operator-signals | 00:04:52–00:05:15 | 2.07 → 1.99 | 3 pass, 1 skip (the refused start: this machine has the encoder) |
    | dress | 00:05:15–00:06:10 | 1.99 → 2.29 | 27/27, `PATH step 9: the stream exercised`; drops 0, underruns 0, rises 1028 and 968 ms |

  - **Belief, lane and audit on the real engine, per mapping row** (the
    gate's runs). The refused start's row comes from CI, the one seam
    where the engine refuses. `reconnecting` is unit-only: no leg makes
    the real engine redial.

    | Leg | Report token | Belief | Lane | Audit |
    |---|---|---|---|---|
    | lost stop (`stream.stop` dropped at the socket) | `live` | `idle` → `live` | `command.unacknowledged.stream` stands, its reason "reads the stream live and the control plane believes it idle" | `belief.corrected` idle→live, report age 204 ms |
    | restart (engine killed under a live stream) | `none` | `live` → `idle` | `stream.divergence` raised, then cleared "consistent again: believed idle, reported none" (finding 1) | `belief.corrected` live→idle, 184 ms |
    | held (a real finalize failure) | `closed` | `held` → `idle` | `outputs.held` stands, through a refused `stream.stop` ("stream is idle"), until `show.unload` clears it ("cleared by show.unload (stateVersion 10)"). After that `command.unacknowledged.record` stands, because the engine refused `record.stop` (finding 3's record twin) | `belief.corrected` held→idle, 206 ms |
    | show-stop-version held leg | `closed` | `held` on `show.stop`'s own frame → `idle`; the record stays `recording` until the heal | the held alert | `show.stop` at v9, the correction at v10, `view.fallback` at v11 (acked), `record.stop` at v12 (refused) |
    | refused start (CI) | `none` | `live` → `idle` | (b) stands, raised at 2027 ms | ~~*from the PR's CI run*~~ `belief.corrected` live→idle, report age 143 ms (run `39106598820`; §2c, filled in the next PR) |

  - **The correction's latency** (window end → the next fresh report → the
    belief), from the server-stamped audit: the expiry's alert row to the
    `belief.corrected` row. It is measured, not asserted (R9).
    - The gate's run: lost stop **583 ms**, held **560 ms**.
    - The run before it (22:46): 960 ms and 548 ms.
    - The engine reports every 1000 ms (`main.rs:32`), and the evaluator
      runs every 250 ms (`server.ts:271`), so the latency is bounded by
      about one report interval plus one evaluation.
    - ~~The refused start (CI) is in the PR's CI run.~~ The refused start on
      CI: **279 ms** (run `39106598820` at `0431441`) and **727 ms** (run
      `39108195056` at `f81653f`). *(§2c, filled in the next PR.)*
  - **The user's rulings on PR #56's six findings (2026-10-18).** The
    numbers are the PR body's. The records' numbers are in brackets.
    1. **[4] Both brief lines are struck.** D4 governs: corrections fire no
       automation. The lines are §5's work item 5 ("and once on a
       correction live→idle") and §6's automation bullet ("and once on a
       correction"). They were struck §2c in the brief, with the reason
       recorded in place (this commit).
    2. **[1] One-evaluation episodes are correct.** The row pair is the
       witness: `stream.divergence`'s raise and clear rows, beside
       `belief.corrected`. **A standing lost-intent witness is a named
       candidate for the next design record, not this PR.** That would be an
       alert that holds while the belief stands corrected against the last
       command's intent.
    3. **[3] Consistent, and kept.** Every command's outcome is witnessed.
       Grouping the noise belongs to the operator surface.
    4. **[2] Held (clock) joins the ledger.** Unqualified "held" means the
       output belief's value, and the other two meanings are always
       qualified: Automation hold (§13.5's "held") and Held (clock) (§11.4's
       `HELD`). A collision count of three is recorded as a known hazard.
       In `VOCABULARY.md` (this commit):
       - Held (clock) is added;
       - Held (output) and Automation hold carry the qualification rule;
       - Held (output)'s "no writer" is struck §2c for its writer, row 11;
       - Belief correction (§10.1.4, row 11) is added under rule 5 ("when in
         doubt, add the term").
    5. **[5] The placement is accepted.** A pointer comment in
       `automation.test.ts`'s field-domain section names where the
       transition tests live (`belief.test.ts`) and what each suite tests.
       The deviation from §6's "`automation.test.ts`" is recorded here.
    6. **The relay miss is recorded.** ORDER 3 named
       `r17-spine-b-brief.md`, and the tree's filename,
       `r17-spine-b-belief.md`, governs.
  - **The two-key pass on PR #56** (independent; relayed by the user
    2026-10-18). Verdict: **HOLDS on all eleven answers.** It was derived
    independently, in a fresh clone at `f81653f`, with the pass's own probes
    and its own mutants. Nothing merged, and the merge word was not given.
    - **The compile-time claim, reproduced exactly.** A fourth `StreamState`
      value gave `tsc` exit 2, with six errors at the builder's line numbers
      (B23's: `alerts.ts(379,21)`, `commands/output.ts(46,30)` and
      `(66,30)`, `commands/show.ts(187,29)`, `state.ts(40,26)`,
      `automation.ts(68,7)`).
    - **The identity measurements.** D2a is byte-identical, 1455 chars, at
      spec `:2562` against brief `:329`. The builder re-measured it at
      `f81653f`: 9 lines each, 1455 chars without the blockquote markers,
      identical line for line.
    - **Their mutant runs.** They killed B1, B2, B3, B4, B6 and B10, all red
      on the builder's exact tests, and every restore was `cmp`-identical.
      B2, the brief's named falsification (path (i)'s tracking), now has
      two independent witnesses.
    - **The legs' timestamps.** The pass measured 442 and 471 ms against the
      builder's 560 and 583 ms (lost stop and held, the correction's
      latency). Both lie within one report interval (1000 ms) plus one
      evaluation (250 ms).
    - **#54 and #55, re-verified at the head.** The D6 dress mutants'
      throwaway runs were checked by the pass.
    - **The A1 boundary, accepted as a scope boundary.** The pass declines to
      re-assert A1's authority tier as its own evidence. That tier sits
      outside this PR's diff, standing on the untouched automation suite,
      35/35. Recorded plainly: the authority tier's evidence is
      pre-existing, and the pass says whose it is.
    - **Their voids, honest.** Load ran 3.0–5.4 all session, and their timing
      re-measures were VOID. They relied on load-insensitive legs and CI's
      reads. The builder's gate windows stand as the builder's
      load-disciplined evidence.
    - **Substance beyond confirmation:**
      - the wire-coverage proof (a refusal is the absence of an ack, not a
        token);
      - path (iv)'s static proof (`persistence.ts` carries no `alerts`,
        `held` or `health`, so the in-process simulation is adequate);
      - (a)'s held branch is reachable only after a clear.
    - **Builder's two-key record: concurs on every call.** No disputes, one
      annotation.
      - **The A1 boundary:** concur, as a scope boundary. The diff changes
        `automation.ts` only in #50's compile-time audit (`streamAudit`
        drops `| "held"`) and `automation.test.ts` only by a comment. No
        hold path is touched. The tier is §10.1.3's "Authority, before
        dispatch" (spec `:2599`).
      - **Wire coverage:** concur. `directive.rs` acknowledges at one point,
        `:147–148`, after every handler's `?`. A refusal returns `Err` first,
        and the channel logs `directive failed` (`channel.rs:264–265`).
      - **Path (iv)'s static proof:** concur. `PersistedSnapshot` is
        `{ stateVersion, manifestIdentity, recoveredAt }`, and `restore()`
        sets only `stateVersion`, `showState` and `pkg`.
      - **The legs, the identity, the compile-time claim, their kills and
        (a)'s held branch (B22):** concur.
      - *Annotation, not a dispute:* the pass's voids (3.0–5.4 all session)
        are a second witness to that night's high machine load. They are
        consistent with the builder's three `npm test` voids, and they
        still do not isolate the cause.

  **Merged: PR #56 is `5043ccf`** (the bookend, the standing pattern of
  §2c), on the user's word of 2026-10-18 ("Two orders, in sequence", ORDER 2; the user ran the merge by hand after the auto-mode classifier refused the builder's): a squash at 2026-10-18T08:48:39Z (parent
  `49d29ae`), pinned to the reviewed head `f81653f`. The merge tree
  `1e0be79` is identical to `f81653f`'s. **SPEC v0.4.8 row 11 is
  ratified**: for the first time, spec and code agree on what the system
  believes. §10.1.3's order, §10.1.4's correction and §10.1.2's clearing set
  for `outputs.held` are now what the control plane does. **R17 is fixed and
  leaves the register**, closed by its proving
  case (CI's dress at `0431441`, run `39106598820`). Main's CI at
  `5043ccf`, run `39112885332`, read per test (§2b), green on attempt 1:
  - **rust** (job `117632556116`): `TOTAL: 515 passed, 0 failed, 4 ignored`,
    with all 30 floor lines held. `telemetry_tick_wires_the_live_session_counter
    ... ok`, so R10 is silent. The peak was load1 27.58 on 3 CPUs, 9.19 per
    CPU (envelope row 20). The ceiling holds at 12.09.
  - **control-plane** (job `117632556222`): `passed=197 failed=0`,
    automation 35/35, belief `ran=4 exercised=4`, effects 2/2, and every e2e
    floor held (slate-release 4, recall 2, take-duration 3, resync-end 3,
    package-reconcile 4, show-stop-version 6 ran and 1 exercised,
    resync-request 3, operator-signals 4 ran and 1 exercised). The refused
    start raised (b) at 2014 ms, and `belief.corrected` live→idle followed
    1011 ms after the window, (b) standing. That is within one report
    interval plus one evaluation (1250 ms).
  - **dress** (job `117632555898`): 27/27, skips 9, exercised 18:
    `PATH step 9: refused start; the belief corrected live -> idle 2358 ms
    after stream.start (bound 4000 ms); stream.stop refused ("stream is
    idle")`. Drops 0, underruns 164, watchdog trips 0, and rises of 4018 and
    3994 ms, all inside the backstops.
  - **The machine-load thread, toward the owed local-ceiling ruling** (#50's
    gate: "A local-ceiling ruling is owed when it next matters"). Two nights
    of evidence on the same machine, with the cause unpinned:
    - **Night 1 (2026-10-16, #50's gate at `6bf10c7`).** The workspace
      battery voided three times on its own load: 2.38 → 4.16, 2.35 → 3.85,
      1.95 → 3.32. Even a start under 2.0 ended at 3.32. Those three voids
      are the evidence of record, by the user's word.
    - **Night 2 (2026-10-17/18, PR #56).**
      - The builder's `npm test` voided three times, each 197/197:
        - 2.42 → 5.38 (23:48:13–23:48:21);
        - 2.39 → 5.05 (23:51:16–23:51:23);
        - 2.47 → 5.94 (23:56:43–23:56:50, the gate).
      - Each started under 2.5 and rose 2.6–3.5 inside its own 7–8 s. A
        diagnostic declared as not evidence (it started at 2.65) peaked at
        16 `node` processes, with no rise. The suite's parallelism is real,
        but it does not by itself make the rise.
      - The falsification battery's ambient voids: B4's e2e (2.39 → 3.88),
        B11's unit (2.29 → 3.39), B28's show-stop-version e2e
        (2.32 → 3.49).
      - The second key's session read 3.0–5.4 throughout, and its timing
        re-measures were VOID. It relied on load-insensitive legs and CI.
    - **What the two nights share.** It is one machine. Start loads sit near
      2.5, and the 1-min load rises inside short windows. The top ambient
      processes on night 2 were iTerm2, WindowServer, CleanMyMac's monitor
      and `opencode`. Nothing pins the cause: neither the suites' own
      parallelism, which the diagnostic did not reproduce, nor any one
      process.
    - **What the ruling must settle.** It is owed, not made. Either a local
      ceiling that this machine's suites can meet (envelope-relative, as
      R10's CI ruling did for CI), or a pinned cause that lifts the
      background. Until then the 3.0 rule stands, and every void is named.
- **~~Position 3~~ ~~Position 4~~ Position 6: the deferred-cell reader**, a static reader that compares each
  routed command's reachable mechanisms with its deferred §13.4.1 cells. It
  was queued by PR #35's two-key pass (section D), and its entry stands under
  the recall leg's record above ("a static reader for the deferred cells").
- **~~Position 5~~ Position 7: a failed engine-side `show.load` heals itself** (the user's
  word of 2026-10-09, ruling on PR #40's fresh two-key pass; the
  package-reconcile record's silent-window paragraph, below, is the
  observation). On a `show.load` failure the engine does what a failed resync
  load already does (SPEC v0.4.8 row 5): `forget_package`, the slate, and a
  report. Today the channel logs `directive failed` and nothing else
  (`channel.rs`). It likely arrives with the queued `show.unload` routing
  (Prompt 11's WU8 record, "route `show.unload` to the engine"), which also
  clears the engine's package.
- **~~Position 8: R20, a plain video clip stops drawing after its first 30
  frames~~** **FIXED** by `88c8316` (PR #57; SPEC v0.4.8 row 12; struck in
  the next PR, the standing pattern of §2c). (the user's word of 2026-10-18). Position 8 by number, first in
  order: the queue ruling below. The register row in
  `docs/soak-protocol.md` §5 carries the mechanism, the class, the
  falsification and the struck refutation.
  - **Found while verifying two outside reviews.** The first review had the
    mechanism backwards: "whole-clip RAM decode, OOM on a 10-minute clip".
    The second corrected it ("a clip longer than 1 second cannot play
    today"). The relay then refuted the correction from the dress's `DONE`,
    which is the clock's, not the picture's. The refutation is struck in
    R20's row, with its cause named.
  - **The class, recorded with the defect: no test looks at the picture.**
    Belief, clocks, acks and counts are asserted; a drawn layer never is.
    Spine B made the system's beliefs provable, but a belief about what is
    on air is still not a picture of it.
  - **The falsification comes before any fix** (ratified 2026-10-18, amended
    to both forms; R20's row carries each in full):
    - **the drop-detector, on A1 today:** frame 45 yields a drawn layer, not
      `None`;
    - **the stopgap-police, on a new fixture:** a 46+-frame frame-counter
      clip in house format, where rendered frame 45 must equal the source's
      own frame 45, decoded independently;
    - **the fixture-integrity guard:** all of that fixture's 46+ frames are
      distinct by hash (the stripper-test lesson).
    The first stronger form, "differs from frame 0", is superseded. It could
    not pass on the static A1, and it would pass under the stopgap.
  - **The fix path.**
    - §12.8's read-ahead, with the determinism constraint intact: decode
      ahead with known occupancy, never wall-clock.
    - A hold-last-frame stopgap may precede it only as harm reduction, never
      recorded as the fix.
    - The frozen-frame comment (~~`video.rs:263–265`~~ `video.rs:262–264`; §2c, item 4: the lines on main at `5043ccf`) is made true or struck
      in the same PR.
    - The design record decides the order.
  - **Then the air-readiness queue, in this order, separate PRs per
    discipline:**
    1. ~~residency honesty~~ **FIXED** by `cd3f0c6` (PR #59; SPEC v0.4.8 row
       13; struck in the next PR, the standing pattern of §2c). Residency
       honesty (preflight reports the residency the engine holds,
       until the §12.3 ladder is real; `video.rs:243–257` and spec `:3094`
       already admit the gap). **Dispatched** by the user's order of
       2026-10-19, after #57's bookend: branch `loop-residency-honesty`, off
       `88c8316`, its record below;
    2. device audio (`NullSink` is the only `AudioSink`);
    3. disk headroom (the soak protocol's 8 GiB precondition);
    4. AC-5, the 30-minute zero-drop soak, never yet run green.
    **The queue after #59 (the user's order of 2026-10-20, with #59's bookend),
    in this order:**
    1. **R25's fix (test-only):** operator-signals' refused-start gates on
       load completion, not the 1.5 s wall clock (R19's class). Its first
       commit lands #59's post-merge records, §12.11.1's floor wording, and
       row 13's ratification mark.
    2. **R24's fix:** `scheduleTimeOfDay` re-arms from the instant just fired,
       not from `wallNow()`. Falsification per its row. A small control-plane
       PR.
    3. **Device audio, via a design record first,** put to the user before
       any code:
       - the backend (cpal or coreaudio-rs, proven with a tone probe: file in,
         tone out the port, before any integration; R20's lesson);
       - the clock (A/V sync, with the measured skew bound named and a proving
         case that measures it);
       - resampling (the clip's rate to the house rate);
       - mixing and per-item gain (what the show schema says and what §12/§19
         must say);
       - the meter and monitor;
       - the ALSA story under the compile-time profile;
       - the CI surface (likely normative-machine-only evidence, the dress
         precedent).
    4. **Disk headroom** (small, preflight-side), **then AC-5**, derived from
       real failures only.

    The unprobeable-loop charge rides any preflight-touching PR above, at the
    builder's discretion. The merge word stays the user's throughout.
  - **WO2's `clockState`** (`server.ts:563` derives `renderNode.clockState`
    from the control plane's `showState`, and the engine sends
    `master_frame().unwrap_or(0)`, `telemetry.rs:82`) rides the same
    engine-opening era, as position 5's work order 2, in its own PR.
  - **The queue ruling (the user's word of 2026-10-18, since the record
    asked): R20 goes first.** Nothing else matters if clips don't play.
    Positions 6 and 7 keep their numbers. They may ride alongside at the
    builder's discretion, since they are a different subsystem. But no
    air-readiness claim precedes R20's closure.
  **R20's design record, written before any fix code** (the user's order of
  2026-10-18: "falsification first, then the design record. The fix is NOT
  in this order"; branch `r20-clip-falsify`, off `5043ccf`). It is held for
  review, and no implementation dispatches before the user's word.
  - **The falsification battery, red today** (`911fcdd`,
    `crates/nbe-engine/tests/r20_clip.rs`). Each draw test asks the real draw
    path (`RenderLoop::render_frame`, then `readback_view`), as Prompt 05's
    pixel tests do. Each first passes a control at frame 15, inside the
    preload, so a failure at frame 45 is R20's and not the harness's. Run at
    03:31:29–03:31:53, load 2.10 → 2.08, on AC under `caffeinate`, after the
    first run (03:17:30–03:17:56, 2.37 → 3.28) voided on ambient load and was
    rerun job-scoped. Results:
    - **The weaker form, on the dress's A1: FAILS.** `R20: A1 is 150 frames,
      but at frame 45 the clip's layer was not drawn (centre [0, 0, 0], black
      is the cleared View; A1 drew [23, 56, 94] at frame 15)`.
    - **The stronger form, on `counter_60.mp4`: FAILS.** `R20: rendered frame
      45 must be the source's own frame 45 [121, 232, 67], got [0, 0, 0]
      (black: the layer was dropped; the source's frame 29 [44, 79, 142]: a
      held last frame; frame 44 [86, 152, 75] or 46 [173, 96, 68]: an
      off-by-one)`. The source frames come from `nbe_decode` alone: no ring,
      no preload cap, no draw path.
    - **The fixture-integrity guard: passes on the fixture and kills both
      of its mutants.** With A1, which is static, in the fixture's place:
      `R20 fixture: counter_60.mp4 frame 1 repeats an earlier frame; every
      frame must differ` (03:32:07–03:32:24, 2.07 → 2.25). With `cfr_30.mp4`
      in its place: `R20 fixture: counter_60.mp4 must outlive frame 45,
      decoded 30 frames` (03:32:34–03:32:36, 2.21). Each time the fixture
      was restored from its saved copy and checked `cmp`-identical.
    - **The gate's static half:** `cargo fmt --check` and
      `cargo clippy --workspace --all-targets -- -D warnings` are clean. The
      workspace grows by three tests, two of them red by design, so this
      branch is not pushed until the fix turns them green. The new suite
      gets its two CI floors with the fix PR (rule 8).
  - **The fixture, as commissioned** (`counter_60.mp4`, beside the dress
    fixtures; its recipe is in `scripts/generate-fixtures.sh`):
    - format: house mezzanine per §6.2: 1920×1080, CFR 30/1, H.264 High,
      `yuv420p`, keyframes at 0 and 30 (`-g 30`), AAC 48 kHz stereo at
      −16.0 LUFS integrated, peak −15.1 dBFS; 158,308 bytes;
    - content: 60 frames, 60 distinct by `framemd5`. Each frame is a flat
      colour keyed by its index N (r = 37N mod 256, g = 91N mod 256,
      b = 255 − 4N). This ffmpeg build has no `drawtext`, so the counter is a
      colour, the shape `cfr_30`, `loop_10` and `cadence_12` already use.
      Adjacent frames differ by at least 37 in red, so an off-by-one reads as
      a different frame;
    - preflight (release `nbe-preflight --house-rate 30`), on the dress
      package plus this asset, scene and item: `preflight OK: air-ready`,
      exit 0, `errors []`, `warnings []`. For `counter_clip`: first and last
      frame decode, `cfr: true`, 30 fps, 1920×1080, `durationFrames: 60`;
    - placement: it is not in the dress manifest, so the dress show is
      unchanged. The engine suite loads it directly.
  - **The current path, from the tree.**
    - **What the 30-frame ring is.** `load_video_asset` decodes a clip with
      no declared loop period to `CLIP_PRELOAD_FRAMES` = 30 (`video.rs:29`,
      `:267`) and keeps those frames as RGBA8 in RAM (`VideoAsset.frames`).
      `period_frames` falls back to the count decoded, 30 (`:277`).
      `upload_ring` uploads every one as its own texture (`render.rs:347`).
    - **Where it ends.** The draw path maps the master clock to a source
      index (`clip_source_index`, `nbe-decode/src/lib.rs:743`), bounded by
      `frame_count`, the ring's 30. At index 30 it returns `None`
      (`:755`).
    - **Who drops the layer.** The `?` in `draw_for` (`render.rs:825–833`)
      returns `None`, and the caller skips the layer (`render.rs:709`).
      `texture_slot`'s hold-last fallback (`.or_else(|| ring.textures.last())`,
      `:839`) is never reached for a clip, because the index was already
      refused.
    - **The decode stops at load.** `next_frame` is called only in
      `load_video_asset` (`video.rs:221`, `:271`). The session lease is kept
      (`_session`, `video.rs:162`, `:305`) but never read again.
    - **The audio diverges.** A clip's audio is decoded whole at load
      (`decode_audio`, `nbe-decode/src/lib.rs:571–578`, up to 30 min), so on
      A1 the sound runs 5 s and the picture 1 s. R20 is also an A/V
      divergence.
  - **§12.8's actual text against the comment.**
    - §12.8 is titled **Long-loop streaming** and governs loops only:
      "Long loops MUST use double-buffered read-ahead", a minimum read-ahead
      of `max(2 * GOP length, 60 frames)`, a wrap policy, and "If wrap
      read-ahead fails, the loop element MUST fall back to frozen frame or
      fallback slate if live."
    - **Nothing in §12 governs a plain clip past its preload.** The comment
      at ~~`video.rs:263–265`~~ `video.rs:262–264` (§2c, item 4: the lines on main at `5043ccf`) ("a plain clip preloads a small window and
      streams the rest (SPEC §12.8's frozen-frame rule covers the gap until
      read-ahead lands)") cites a loop rule for a clip, and it is false
      twice: nothing streams, and nothing freezes.
    - **Decided here:** the comment is made true by the fix, not by a
      gloss. In the fix PR it is §2c-struck and replaced with the clip
      read-ahead's actual contract.
  - **The read-ahead design.**
    - **The shape.** A clip on air (or armed, so a take is instant) keeps a
      window of K resident frames, `[i, i + K)`, where i is the frame the
      draw path will read next. A decode worker per playing clip (the
      session lease it already holds) decodes sequentially, with no seek: a
      clip plays once. It hands frames to the render thread, which uploads
      frame j into slot `j mod K`. The load-time preload stays as the head
      of the window, so a take still draws frame 0 at once.
    - **Decode ahead with known occupancy, never on the wall clock.** The
      worker is paced by occupancy alone. It may produce frame j only once
      slot `j mod K` is free, that is once the draw path has passed `j − K`.
      The render loop publishes the frame it has drawn (an atomic the
      worker reads). Which frame shows remains a pure function of the master
      clock (`clip_source_index`, §12.1). The worker decides only whether
      that frame is resident, never which frame shows. No timer or
      `Instant::now` enters the selection.
    - **The named policy when decode falls behind: `readAheadMiss`.** If
      frame i is not resident when frame i is drawn, the draw path holds the
      last frame it presented for that layer. It never draws black for a
      clip that has not ended, and it never blocks the render thread
      (§12.8's wrap rule 3, extended to clips). Each miss is counted, and the
      count rides the §10.1 tick (`readAheadMissesTotal`), so a miss is an
      observable event, never a silent freeze. The hold is the miss policy,
      not the fix: a run with misses is a degraded run, and the dress
      asserts the count.
    - **The clip's true end.** The draw path needs the clip's real length to
      tell "ended" (drop the layer, correct) from "not yet resident" (miss,
      hold). The length comes from the decoder's metadata at load (the frame
      count of the track, as preflight already reports `durationFrames`),
      not from the preload's count. `period_frames` for a clip becomes that
      length, and the ring depth becomes K.
    - **Residency math under rings.** A 1080p RGBA8 frame is 8,294,400
      bytes, 7.91 MiB.
      - Today each clip holds 30 frames, 237 MiB of VRAM plus 237 MiB of RAM,
        outside §12.4's loop budget (the clip path ignores the budget). The
        dress's A1 and A3 alone hold 332 MiB (237 + 95).
      - At K = §12.8's minimum, `max(2 × GOP, 60)` = 60 for house format
        (§6.2: GOP ≤ 30), ~~one playing clip costs 475 MiB of VRAM. A mix
        between two clips costs 949 MiB~~, past §12.4's 512 MiB total.
        *(§2c, R20's two-key pass, item 1, 711.91 MiB confirmed exact: as
        built and as §12.11.1 says, every loaded plain clip of 90 or more
        frames holds its head plus its window, 711.9 MiB, from load, playing
        or not. Two clips mixing hold 1,423.8 MiB, and the dress's worst case
        is about 1.6 GB. The 30 frames the engine logs per clip, the "video
        ring resident" line's `period` (`render.rs:526–534`), are a vestigial
        log artifact, not normative text. The residency honesty PR corrects
        the line.)*
      - NV12 would cost 2.97 MiB a frame, so 178 MiB at K = 60. That path is
        not built ("RGBA path until shader-side YUV lands", `video.rs:237`).
      - **Proposed for the spec row:** a clip's depth is `K = 2 × GOP`
        (30 to 60 at house format), accounted per playing clip in §12.11's
        package demand. It is not a loop's cache, so it is not charged to
        §12.4's loop budget, but it is reported, so the queued
        residency-honesty item measures what the engine holds.
    - **Memory bounds on the floor device** (§0.3, `hardware-baseline.txt`:
      Radeon Pro 555X, 4 GB dedicated VRAM; 16 GB RAM).
      - The worst case the dress can reach is one mix of two clips at K = 60
        RGBA8: ~~949 MiB of VRAM, plus the 237 MiB heads of the other clips
        loaded~~ about 1.6 GB, every loaded plain clip holding its head and
        window from load *(§2c, item 1, above)*. That fits 4 GB with the loop cache (512 MiB) and the View,
        preview and record targets beside it.
      - RAM is bounded the same way: the worker holds at most K decoded
        frames per clip, and the load-time `frames` vector shrinks to the
        head.
      - The design must also refuse rather than overcommit: a take that
        would push resident clips past the device's working set is §12.4's
        "device working set allows allocation" check, applied to clips.
  - **The stopgap decision: straight to read-ahead.** Reasons:
    1. A hold-last-frame stopgap needs the clip's true length to tell an
       ended clip from an exhausted ring. That plumbing is the read-ahead's
       first step anyway.
    2. The stopgap would turn the weaker form green and leave the stronger
       form red, so the tree would carry a green drop-detector over an
       unfixed defect. Against the dress's static fixtures, a frozen frame
       is indistinguishable from playback, so the dress would look fixed.
    3. Harm reduction buys little on air: a frozen picture for four seconds
       reads as a hang, not as a clip.
    4. The read-ahead's miss policy *is* the hold, bounded and counted, so
       the harm the stopgap would reduce is reduced by the fix itself.
    - **If the user rules otherwise:** the stopgap lands as its own commit,
      recorded as harm reduction and never as the fix. The stronger form
      stays red until the read-ahead lands, and the frozen-frame comment is
      made true in that same PR.
  - **The dress's impact.** After the fix, every dress clip renders its full
    length for the first time, so every dress run's load profile changes.
    - On A1's air time, a decode worker runs, and 7.91 MiB is uploaded each
      house frame (237 MiB/s while it plays), on CI's 3 arm64 cores and on
      the normative machine. A3 (12 frames) and the loop A2 are unaffected.
    - **The floor lines to watch, against each run's own history:**
      - drops (CI backstop 20, normative 0);
      - underruns (R12: CI's peak 232, backstop 2000; normative 0);
      - `step4RiseMs` and `step6RiseMs` (normative gate 3000 ms, CI
        backstop 10000);
      - watchdog trips (backstop 10), and fallback ticks outside a watchdog
        episode;
      - the decode-session peak (`MAX_DECODE_SESSIONS` 8);
      - `readAheadMissesTotal`, new: it must read 0 on the normative
        machine, and on CI it is recorded, not gated, until it has an
        envelope;
      - the rust job's test count, 515 → 518, with R10's envelope read as
        always.
    - The dress gains a step that looks at the picture: the counter clip on
      air, with its frame 45 read back. The class's own remedy, in the
      rehearsal.
  - **The spec surface: row 12.** §12.8 extends to plain clips:
    - clip read-ahead with depth K;
    - occupancy pacing, never wall-clock;
    - the `readAheadMiss` policy (hold the last presented frame, count it,
      never block, never black before the clip's end);
    - the clip's end taken from its metadata;
    - the clip's residency in §12.11's demand.

    Its candidate text is reviewed with this record and lands as an early
    commit in the fix PR, ratified on that PR's merge: the row-11 pattern.
    `VOCABULARY.md` gains the read-ahead miss.
  - **The fix's falsification plan: the mutants its guards must die to.**

    | Mutant | What it does | The test that kills it |
    |---|---|---|
    | M1, revert-to-30-ring | the read-ahead disabled: a clip holds its 30-frame preload again | the weaker form (A1 frame 45 black) and the stronger form |
    | M2, hold-last-frame | the worker never advances; the draw path holds the last resident frame | the stronger form only (shows frame 29's colour); the weaker form passes by design, since A1 is static |
    | M3, off-by-one ring | frame j uploaded to slot `(j ± 1) mod K`, or the index read one off | the stronger form (frame 44's or 46's colour) |
    | M4, occupancy miscount | the worker may overwrite slot `j mod K` before the draw path passes `j − K` | a new form with the fix: the stronger form after the window has filled (frame 45 read back once the worker has run K ahead), which reads frame 45 + K's colour |
    | M5, a miss draws black or blocks | the miss policy removed | a new miss test with the fix: a stalled worker yields the last presented frame and a counted miss, within the frame deadline |
    | M6, the end ignored | a clip past its true length keeps holding instead of dropping | a new end test with the fix: frame 60 of `counter_60.mp4` draws nothing |
    | M7, fixture blinded | `counter_60.mp4` made static or short | the fixture-integrity guard (falsified above, both clauses) |
  - **RATIFIED with four amendments** (the user's word of 2026-10-18). The
    design record above stands, amended as follows:
    1. **Row 12 carries a residency clause.** A clip's resident frames (its
       head plus its window) enter §12.11.1's package demand, and preflight
       computes them, so spec and code agree when the row ratifies.
    2. **An audio scope statement.** R20 is a picture defect. A clip's audio
       is decoded whole at `show.load` (§8.4) and is unchanged. The
       read-ahead governs picture only, and after it picture and sound
       agree for the clip's full length.
    3. **The rung-2 interaction is recorded, and a shrink policy is
       deferred.** Rung 2 sheds an off-air clip's head and window as it
       sheds an off-air loop's ring. The read-ahead stops, and a re-acquire
       re-uploads the head and re-primes the window. The View's clip is
       never shed. Shrinking K under pressure is deferred, not in this row.
    4. **K is in frames, and YUV is noted as byte savings only.** K is 2 ×
       GOP frames (60 at house format). A YUV ring would cut each frame's
       bytes (7.91 to 2.97 MiB at 1080p) and changes nothing else. It is not
       a dependency of this fix.
  - **Implementation dispatches under the ruling's seven constraints:**
    1. the falsification tests and the fix land on the fix PR, whose body
       quotes the red-today evidence (`.git/nbe-staged/r20-pr-body-evidence.md`)
       and then the green at its head;
    2. the comment at ~~`video.rs:263–265`~~ `video.rs:262–264` (§2c, item 4: the lines on main at `5043ccf`) is struck or made true per the
       record: row 12 extends §12.8, and the comment cites the extended rule
       or goes;
    3. `readAheadMiss` holds the last frame, is counted on the telemetry
       tick, never blocks and never draws black, and the clip's end comes
       from its metadata;
    4. M1–M7 each die to their mapped test. A hold-last-frame-as-fix mutant
       must fail the stronger form. Restores are `cmp`-verified (and
       rebuilt, for Rust), and CI-only legs follow #47's throwaway pattern;
    5. the dress's clips render full length for the first time, so the named
       floor lines are watched, and any moved floor is disclosed with its
       cause, never silently re-baselined;
    6. row 12 lands as an early commit, and the PR body states that it
       ratifies on this PR's merge;
    7. a two-key independent pass comes before any merge word: "engine work
       gets what spine B got".
  - **Housekeeping:** `.git/nbe-staged/after-pr56-records.md` was consumed on
    landing (`6beb8d5`) and removed, per convention.
  **R20's build: the fix, its guards, and every mutant killed** (branch
  `r20-clip-falsify`, under the ratified record's seven constraints).
  - **The commits, in order:**
    - `6beb8d5`: PR #56's records (the cadence's first commit).
    - `911fcdd`: the falsification battery, red before any fix.
    - `eb52c0d`: the design record.
    - `dae56ee`: its ratification, with the four amendments and the
      constraints.
    - `3f337fe`: row 12, the early commit.
    - `2d1be82`: row 12's candidate text, made to say a miss is counted on
      the View (a Preview miss holds, uncounted, as §10.2 counts no preview
      miss as a dropped frame).
    - `2bde830`: the fix.
    - `dccabad` and `89ff7a5`: the two M4 hardenings below.
    - `69a55d8`: the read-ahead uploads through a staging pool (finding 9:
      the gate on `89ff7a5` failed, and this is its fix).
    - `9b28f01`: the refill guard (M8 survived `show-stop-version`), the
      CI floor 8 → 9, and row 12's test column (M1–M8, the old text
      struck).
    - `2497bc8`: these records, first form.
    - `42c06d7`: the settle seam waits for the staging pool (finding 10: the
      refill guard's precondition failed on CI).
  - **Green at the head,** the ratified battery unchanged in its assertions:
    `r20_clip` ~~8/8~~ 9/9 (§2c: the ninth is finding 9's refill guard,
    `9b28f01`). ~~The run of record is 05:40:04–05:40:36, load
    2.26 → 2.94.~~ (§2c: that was `89ff7a5`.) ~~The run of record is
    09:30:54–09:31:22, load 2.39 → 2.05; the job-scoped re-run of the void
    in finding 9.~~ (§2c: that was `69a55d8`, 8/8.) ~~The run of record is the
    gate's at `9b28f01`, 10:57:26–10:57:55, load 2.43 → 2.58 (the gate
    below).~~ (§2c: superseded by `42c06d7`.) The run of record is the gate's
    at `42c06d7`, 12:01:11–12:01:41, load 2.45 → 2.16. The battery's decode
    helper now streams each frame's centre
    and hash and shares each fixture's decode across the suite; the
    assertions are unchanged.
  - **Findings, recorded:**
    1. **The dev-profile decoder could not keep pace.** A full 1080p decode
       cost 100.7 ms a frame unoptimized (10 fps) and 9.1 ms optimized
       (109 fps). The timestamp-only decode cost 2.5 ms in both, so the cost
       is the pixel copy. The dress and CI run the dev-profile engine, and
       the dress counted **37 read-ahead misses** on the counter clip before
       the remedy. The remedy is `Cargo.toml`'s
       `[profile.dev.package.nbe-decode] opt-level = 3`, after which the
       dress counted **0**. It changes no behaviour, only speed.
    2. **The window prime costs the load boundary.** In the dev-profile
       engine, one long clip's ring (head 30 + window 60) uploads in about
       1.06 s on the render thread, against about 0.37 s for the 30-frame
       head alone. My first dress design staged the counter clip into step
       11's package, adding a second ring to that load. Step 11's `record.start`
       then waited behind about 2.3 s of uploads, and step 11's cut back to
       A1 landed after A1's end ("item A1 is DONE", twice). That is the
       step-11 coupling #53 recorded and left for the user ("step 11 carries
       the same coupling"), exposed by the heavier boundary. **Remedy, in my
       own addition only:** step 12b stages and reloads its own
       counter-carrying copy, so step 11's load gains only A1's own prime.
       ~~Step 11 has passed in every dress since.~~ (§2c: false at the gate.
       Step 11 failed again on `89ff7a5`, "item A1 is DONE". The second
       cause is finding 9, the read-ahead's per-frame uploads, which slowed
       every tick after the boundary as well. The account above names only
       the boundary, and finding 9 qualifies it. Step 11 passed after
       finding 9's fix.) **The coupling itself stands,
       recorded and not fixed:** it is the user's to rule, and the load
       boundary's cost is a named line to watch.
    3. **Recorded video runs on the received-video timeline** (step 13's own
       note), not the master clock. Step 12c's first design read each
       recorded frame's timestamp as its source index, and failed at 31
       frames of drift. The step now reads the frames in order: the
       counter's recorded frames must run in order, pass 45–89 and 120, and
       show no black between the first and the last. The drift is logged
       only. The logged sequence shows the record ladder's shed gaps (index
       3, then 17, one timestamp step apart).
    4. **Step 13 looked for its kill take in step 11's record directory.**
       Step 12b's reload pointed the package's record output elsewhere, so
       12b's copy now shares step 11's directory. Step 13's stale comment
       ("A1 is still the on-air item") is struck (§2c).
    5. **CI's dress floor counts `# SKIP` lines.** Splitting the picture step
       into 12b (the zero-miss gate) and 12c (the read-back) gives each new
       test exactly one CI skip. So `ran` moves 27 → 29 and `exercised` stays
       18: the two new tests and their two skips cancel. Disclosed in
       `ci.yml`, never re-baselined silently.
    6. **M4 survived its mapped test twice** (the battery below), and each
       survival hardened the guard:
       - first, the settle seam called a full-looking window settled. It now
         requires the decoder to be held at the window's end (`dccabad`);
       - second, the test's window at frame 15 was wholly primed, so no
         decoder ran to overwrite it. The test now runs at frame 45, where a
         decoder fills 90..104 and frame 105 belongs in frame 45's own slot
         (`89ff7a5`).
    7. **The restore trap, from memory, hit once.** The harness restored with
       `copy2`, which preserves the old mtime, so cargo kept the mutant's
       build. One suite run (`r20-fix-5`) tested a stale M5b and is
       discarded. The harness now restores with a fresh mtime, the sources
       were touched and rebuilt, and the battery was re-run whole on the
       final tests. Each mutant's own write had always bumped its mtime, so
       every mutant compiled its intended content.
    8. **The suite's own load.** The first form decoded each 150-frame fixture
       whole in every test, which voided windows on its own load (the
       workspace at 2.01 → 4.45, the suite at 2.49 → 3.72). Streaming and
       shared decodes cut the suite from 181 s to 30 s.
    9. **The read-ahead's uploads cost the record its frames: R20's own
       regression, found at the gate and fixed on this branch.** The gate on
       `89ff7a5` failed two local legs that main passes:
       - `show-stop-version.e2e.ts` 4/6: "E3, record only" and "concurrent"
         ended with `E_RECORD_INPUT: no video units` (the take recorded no
         frame);
       - the dress 27/29: step 11 (`item A1 is DONE`) and step 12b (12
         misses).

       **A/B, the same e2e, the same tree, only the engine binary swapped**
       (runs 07:28–07:35):
       - branch: E1 and E3 failed, 3 × `no video units` (07:28:15–07:29:49,
         load 2.47 → 2.41);
       - main `5043ccf`: 6/6 (07:29:49–07:31:04, 2.41 → 2.52);
       - branch: VOID (07:31:34–07:33:03, 2.33 → 3.78);
       - main: 6/6 (07:33:48–07:35:02, 2.48 → 2.19).

       **The cause, measured** (an instrumented scratch build, never
       committed). `Gpu::upload_rgba` is `queue.write_texture`, whose doc
       already said "never in the per-frame path (SPEC §7.13)". It creates a
       staging buffer per call, and costs **10.5 ms** for one 1080p RGBA
       frame (median of 20, 10.3–10.9). R20's pump called it per frame, up to
       four times a tick. When a take lands during the load boundary's
       upload, the View resumes past the primed window and the pump catches
       up at its cap: **four uploads, 47 ms of pump, 52 ms ticks** against a
       33 ms deadline. The record's pre-check skips any frame whose
       `render_frame` overran, so every recorded frame was skipped. The View
       counted no drop, because its drop timer starts after the pump: a
       cost the dress's zero-drop gate could not see.

       **The fix** (`69a55d8`): the read-ahead uploads through
       `gpu::UploadPool`, a bounded pool of mapped staging buffers (4, one
       frame's upload budget, 32 MiB at 1080p, created on first use). Each
       buffer is reused once the GPU has consumed it. The copy costs **0.90
       ms** a frame (median of 20). With every buffer in flight the tick
       uploads nothing more and never waits. The load-time prime uses the
       same pool and may wait, at the boundary only. Measured on the same
       probe: catch-up ticks 4–9 ms in total (2–3 uploads), steady ticks
       about 2 ms (1 upload), **1** record frame skipped (during the
       boundary), and the take finalized. The guard is the refill test
       (`9b28f01`, M8 in the battery below). Green after the fix:
       `show-stop-version` 6/6 (09:24:29–09:25:50, load 2.37 → 1.81) and 6/6
       (09:25:50–09:27:08, 1.81 → 1.89); the dress 29/29 with 0 misses
       (09:27:08–09:28:09, 1.89 → 2.11). One R20 suite run was VOID
       (09:14:25–09:14:58, 2.38 → 12.44): a macOS Contacts sync burst
       (`contactsd`, `AddressBookManager`), external, re-run job-scoped.

       **What the fix leaves, disclosed:** the load boundary. A plain clip
       longer than 30 frames still costs about **0.45 s** more at the
       boundary than on main (A1: 0.82 s against 0.37 s, release). The
       window's 60 textures cost 3.4 ms to create, and the prime 433 ms: its
       copies into fresh textures, GPU-bound (a 60-frame prime costs 130–230
       ms through the pool in isolation). The render loop shares its thread
       with the directive channel (`main.rs`: one `LocalSet`, `run_until`),
       so the boundary delays directives, not the View. That is the class
       #53 recorded for step 11. R20 makes it about 2.2× per clip. A paced
       prime (uploaded by the pump after load) would remove it. It is a
       candidate, not done here.
    10. **CI, read per test: the refill guard's precondition, and a step-5
        line R20 opened.** Two runs of PR #57.

        Run `39153017826`, at `2497bc8`, failed in two places:
        - **`r20_clip`'s refill guard failed on its precondition, not its
          bound:** "settled at 51, drawing 52 uploads frame 111 and nothing
          else", left 0. On CI's paravirtual adapter the settle's last
          uploads were still in flight, so all four staging buffers were
          taken and the frame waited. That is the pool working as designed
          and a hole in the test. **The fix (`42c06d7`):** the settle seam,
          which is test-only, waits for the pool to drain once settled.
        - **The dress, step 5, "a 15-frame mix drops no frames":** 2 drops,
          asserted on every machine. The CI artifact puts a watchdog fault
          (`misses=4`) about 180 ms into the mix. A1, a plain clip taken
          4.0 s earlier (about frame 120), was the outgoing layer and still
          streaming. The fault came before A1 left the View, so before the
          re-prime burst. On main, A1 past frame 30 drew nothing, so step 5
          had never blended two live video layers on CI; main's CI is green
          12 of 12. A second trip came in step 11's region (`misses=12`;
          drops 3; `readAheadMisses` 7, recorded and not gated on CI).

        Run `39153908984`, at `42c06d7`, was **all green**:
        - `r20_clip` ran 9, exercised 9, with a refill median of **1.74 ms**
          on CI's adapter (0.86–2.45);
        - the dress ran 29, passed 29, with 11 skips, so exercised 18;
        - step 5 passed.

        **Step 5 on CI is therefore intermittent under R20: ~~1 failure in 2
        runs~~ 1 of 3 on PR #57's CI** *(§2c, R20's two-key pass, item 5:
        `39153017826` failed; `39153908984` and `39157683882` were green. The
        six attempts of #58's throwaway run `39209726459` (head `0fc45cb`)
        went 6 of 6 green with step 5 clean. They are excluded because they
        ran an instrumented throwaway binary, not because they were local:
        all nine ran on GitHub's runners. The cross-check's stall figures
        from #58 are struck as unverifiable, because each rerun replaced the
        run's artifact: ~~531 ms~~, mandatory by the user's ruling,
        ~~attempts 1–5's 57–133 ms~~ and ~~"50–530 ms"~~. Only attempt 6's
        artifact survives, and its figure stands: a View maximum of 1.02 ms.
        Filed as R21)*. The cause is narrowed to the two-layer blend while a clip
        streams, on the paravirtual adapter. It is not proven further:
        `renderGpuTimeMs` is 0 there, and the tick is one second wide. It is
        disclosed as a line R20 opened (constraint 5), not re-baselined: the
        step stays asserted on every machine. Whether it becomes
        normative-only, or what bounds it on CI, is the user's to rule.

        **R10, both runs:** `telemetry_tick_wires_the_live_session_counter`
        is ok in both. The maximum per-CPU load was 5.97 (`39153017826`,
        load1 17.90 / 3 CPUs, 18:13:08Z) and 6.29 (`39153908984`, 18.88 / 3,
        18:26:05Z). Both are inside the envelope (max 9.67), and the ceiling
        stays 12.09. No sighting.
  - **The falsification, engine suite** (`r20_clip`, 9 tests). ~~The final
    battery on `89ff7a5`'s tests.~~ (§2c: superseded. A whole battery ran on
    `89ff7a5`, and every mutant was killed then. The code changed after it,
    with the staging pool `69a55d8` and the refill guard `9b28f01`, so the
    ~~battery of record is re-run whole on `9b28f01`, below~~ battery was
    re-run whole on `9b28f01`, below.) (§2c, again: `42c06d7` then changed the
    settle seam the mapped tests use, finding 10, so the battery of record is
    the re-run on `42c06d7`, after this table.) Each mutant was
    applied from an exact match, built, run under the load rule, restored
    `cmp`-identical and `git`-clean against HEAD, and rebuilt. Voided windows
    were re-run job-scoped. M5b's anchor moved with `9b28f01` (the pump call
    now binds its upload count), and its mutant was re-anchored with the same
    meaning: the frame path settles the window before it draws.

    | # | Mutant | The mapped test | What failed (window, load) |
    |---|---|---|---|
    | M1 | revert-to-30-ring: the clip arm disabled | the weaker and the stronger forms | 6 of 9; full length `rendered frame 30 … got [0, 0, 0]`, miss `got [0, 0, 0] (black …)` (10:26:43–10:27:12, 2.34 → 2.18) |
    | M2 | hold-last-frame-as-fix: no window counts as resident | **the stronger form** (the stopgap-police) | 6 of 9; stronger `got [36, 88, 199]`, the held frame; **the weaker passes, by design** (10:27:48–10:29:56, 2.35 → 2.13) |
    | M2b | the read-ahead decodes nothing | the full-length test | 4 of 9; `the read-ahead did not fill its window at frame 45` (10:30:18–10:32:06, 2.16 → 1.95) |
    | M3 | the prime uploads frame j into j+1's slot | the stronger form | 4 of 9; stronger `got [86, 152, 75]`, frame 44's colour, an off-by-one (10:32:27–10:32:56, 2.06 → 2.12) |
    | M4 | occupancy miscount: a frame past the window uploaded anyway | the window test | 3 of 9; `at frame 45 the read-ahead must fill its window [45, 105) and hold its decoder at the window's end … it did not settle` (10:33:17–10:34:45, 1.93 → 2.69) |
    | M5 | a miss draws black | the miss test | 1 of 9; `got [0, 0, 0] (black: drawn black …)` (10:35:21–10:35:52, 2.49 → 2.26) |
    | M5b | a miss blocks on the decoder | the miss test | 2 of 9; `got [80, 154, 67]`, frame 120, drawn late: `the draw waited on the decoder` (10:42:58–10:43:29, 2.47 → 2.32) |
    | M6 | the end ignored (length unbounded) | the end test | 1 of 9; `at frame 60 counter_60 has ended … got [127, 224, 11]`, frame 59 held past its end (10:43:51–10:44:21, 2.31 → 2.34) |
    | M6b | the end from the decoded count, not the metadata | the full-length test | 4 of 9; `rendered frame 90 … got [0, 0, 0]` (10:44:57–10:45:25, 2.14 → 2.79) |
    | M7 | a guard fixture blinded | the integrity guards | counter_60 static (A1): `frame 1 repeats an earlier frame`; short (cfr_30): `must outlive frame 45` (`911fcdd`'s runs). counter_150 static: `frame 1 repeats`, short: `must outlive … 90 frames` (the fix's runs). The fixtures and guards are unchanged since |
    | **M8** | the frame path uploads through `write_texture` (finding 9) | the refill test | 1 of 9, that test only; `the median of 10 was 11.62 ms` against 8.33 ms (10:47:01–10:47:32, 2.49 → 2.30). **It survived `show-stop-version`** (6/6, 09:59:56–10:01:18, 2.27 → 2.99): that e2e fails only when a take lands during the load boundary, so the refill test was written for it (`9b28f01`) |

    Voids this battery: dress M2's first run (10:39:07–10:40:08, 1.98 →
    3.06), re-run below. The aborted battery before it (on `69a55d8`) had
    M1 (09:32:21–09:32:48, 2.24 → 3.05) and M2b (09:37:33–09:38:58, 2.41 →
    3.14) VOID, causes not identified after the fact. Its dress M1 was
    aborted unrun at 10:03 because of my own error: I edited `render.rs`
    while that mutant held it. The battery was killed, the stray mutant line
    reverted by hand, `git diff` checked to hold only the intended edits, and
    the tree rebuilt. That whole battery is superseded by this one.

    **The battery of record, re-run whole on `42c06d7`** (the same mutants and
    anchors; every window valid, no void; every restore `cmp`-identical and
    `git`-clean, every rebuild green):

    | # | Mapped test, what failed | Window, load |
    |---|---|---|
    | M1 | weaker and stronger forms, 6 of 9; `rendered frame 30 … got [0, 0, 0]` | 11:30:49–11:31:18, 2.28 → 2.62 |
    | M2 | **the stronger form** `got [36, 88, 199]`, the held frame; the weaker passes, by design; 6 of 9 | 11:32:25–11:34:32, 2.39 → 2.85 |
    | M2b | the full-length test, `did not fill its window at frame 45`; 4 of 9 | 11:35:55–11:37:43, 2.40 → 2.12 |
    | M3 | the stronger form `got [86, 152, 75]`, frame 44; 4 of 9 | 11:38:20–11:38:49, 2.35 → 2.26 |
    | M4 | the window test, `must fill its window [45, 105) … did not settle`; 3 of 9 | 11:40:26–11:41:54, 2.30 → 2.59 |
    | M5 | the miss test alone, `got [0, 0, 0] (black …)` | 11:42:48–11:43:17, 2.13 → 2.18 |
    | M5b | the miss test, `got [80, 154, 67]`, drawn late; 2 of 9 | 11:43:54–11:44:25, 2.36 → 2.33 |
    | M6 | the end test alone, frame 59 `[127, 224, 11]` held past its end | 11:45:02–11:45:32, 2.11 → 1.95 |
    | M6b | the full-length test, `rendered frame 90 … got [0, 0, 0]`; 4 of 9 | 11:46:24–11:46:51, 2.35 |
    | M8 | the refill test alone, median 11.12 ms against 8.33 ms | 11:48:28–11:49:00, 2.26 → 1.91 |
  - **The falsification, the dress** (each mutant built into the dev-profile
    engine the dress spawns, restored, rebuilt; only the R20 steps failed,
    and the other 28 held). ~~The runs on `89ff7a5`: M1 06:44:08–06:45:09,
    M2 06:46:49–06:47:48.~~ (§2c: superseded by the battery of record on
    ~~`9b28f01`~~ `42c06d7`; see below. Both mutants were killed the same way
    then, M2 with 69 misses.)
    - **M1** (10:37:43–10:38:43, 2.27 → 2.39): 12c
      `the picture must pass the clip's head (frames 45–89 recorded)`. Its
      logged sequence is R20 itself on the recorded picture: indices 1–29,
      then black to the take's end. 12b passed (the old path counts no
      misses).
    - **M2** (10:47:53–10:48:52, 2.48 → 2.29; its first run 10:39:07–10:40:08,
      1.98 → 3.06, VOID, killed the same way with 83): 12b
      `the read-ahead missed 110 frame(s) of the counter clip`.
    - **Of record, on `42c06d7`:** M1 (11:49:37–11:50:37, 2.18 → 2.65), 12c,
      the recorded sequence 1–29, then black; M2 (11:51:16–11:52:16,
      2.25 → 2.42), 12b, `the read-ahead missed 117 frame(s)`. Both restored
      `cmp`-identical and `git`-clean, and rebuilt.
  - **No CI-only leg** (constraint 4). Every guard runs on the normative
    machine. The dress's 12b and 12c are normative-only, and on CI 12b logs
    its misses, recorded and not gated. The refill guard (M8) runs on CI as
    well, against the same deadline: if CI's adapter cannot upload a 1080p
    frame within a quarter of a frame, CI fails, and that is the truth about
    that machine, not a flake. `show-stop-version`, which first showed
    finding 9, is local-only for its record legs (CI has no hardware
    encoder).
  - **The gate at `9b28f01`** (scratchpad `r20/gate2/`, 10:49–11:08; AC,
    under `caffeinate`; `pmset`: no sleep, wake or battery event since
    06:40). Untimed: release and test builds, `fmt --check`, workspace
    `clippy -D warnings`, `tsc`, all green.

    | Step | Result | Window | Load |
    |---|---|---|---|
    | workspace | 525 passed, 0 failed, 4 ignored | 10:54:30–10:56:56 | 2.40 → 2.75 |
    | `r20_clip` | 9/9 | 10:57:26–10:57:55 | 2.43 → 2.58 |
    | `npm test` | 198/198 | 10:58:40–10:58:48 | 2.26 → 2.87 |
    | automation | green | 10:59:33–10:59:36 | 2.38 → 2.59 |
    | effects | green | 11:00:21–11:00:22 | 2.45 |
    | belief | green | 11:00:22–11:00:24 | 2.45 → 2.42 |
    | e2e slate-release | green | 11:00:24–11:00:37 | 2.42 → 2.71 |
    | e2e recall | green | 11:01:07–11:01:21 | 2.21 → 2.66 |
    | e2e take-duration | green | 11:03:51–11:03:56 | 2.05 |
    | e2e resync-end | green | 11:03:56–11:04:11 | 2.05 → 2.22 |
    | e2e package-reconcile | green | 11:04:11–11:04:26 | 2.22 → 2.17 |
    | e2e show-stop-version | 6/6, 0 × `no video units` | 11:04:26–11:05:48 | 2.17 → 2.21 |
    | e2e resync-request | green | 11:05:48–11:06:15 | 2.21 → 2.57 |
    | e2e operator-signals | 3 pass + 1 skip (its CI seam, unchanged) | 11:06:45–11:07:11 | 2.36 → 2.45 |
    | the dress | 29/29; 0 read-ahead misses; the picture holds counter frames 1–149, in order | 11:07:11–11:08:14 | 2.45 → 2.66 |

    No void in the gate. The gate before it, on `89ff7a5`, failed, and its
    failures are finding 9 (that gate's workspace and `npm test` windows
    were VOID, 2.09 → 3.37 and 2.26 → 3.70). (§2c: superseded as the gate of
    record by the gate at `42c06d7`, below, after finding 10's seam change.)
  - **The gate at `42c06d7`, of record** (scratchpad `r20/gate3/`,
    11:55–12:11; AC, under `caffeinate`; `pmset`: no sleep, wake or battery
    event). Untimed: release and test builds, `fmt --check`, workspace
    `clippy -D warnings`, `tsc`, all green.

    | Step | Result | Window | Load |
    |---|---|---|---|
    | workspace | 525 passed, 0 failed, 4 ignored | 11:56:30–11:58:56 | 2.17 → 2.73 |
    | `r20_clip` | 9/9 | 12:01:11–12:01:41 | 2.45 → 2.16 |
    | `npm test` | 198/198 (job-scoped re-run of the void below) | 12:10:57–12:11:05 | 2.06 → 2.13 |
    | automation / effects / belief | green | 12:03:34–12:03:40 | 2.28 → 2.42 |
    | e2e slate-release | green | 12:03:40–12:03:56 | 2.42 → 2.85 |
    | e2e recall | green | 12:04:41–12:04:54 | 2.34 → 2.29 |
    | e2e take-duration | green | 12:04:54–12:04:59 | 2.29 → 2.91 |
    | e2e resync-end | green | 12:05:59–12:06:14 | 2.37 → 2.22 |
    | e2e package-reconcile | green | 12:06:14–12:06:30 | 2.22 → 2.53 |
    | e2e show-stop-version | 6/6, 0 × `no video units` | 12:07:30–12:08:49 | 2.47 → 2.24 |
    | e2e resync-request | green | 12:08:49–12:09:16 | 2.24 |
    | e2e operator-signals | 3 pass + 1 skip (its CI seam) | 12:09:16–12:09:43 | 2.24 → 2.40 |
    | the dress | 29/29; 0 misses; 149 recorded counter frames in order, indices 1–148 *(§2c, R20's two-key pass, item 2: `counter_150`'s frame 0 is (0, 0, 0) by its recipe and classifies as `black`, so an ordered recording starts at 1. This recording also had 12 duplicated and 11 missing indices, all on the record side; the View counted 0 misses)* | 12:09:43–12:10:42 | 2.40 → 2.19 |

    One void: `npm test`'s first run (12:01:41–12:01:49, 2.16 → 4.28, 198/198
    in it). Its likely cause is the suite's own eight-second parallel burst,
    as at the first gate; it is not proven. It was re-run job-scoped.

  - **The two-key pass on PR #57** (independent; run in opencode and relayed
    by the user on 2026-10-19). Verdict: **HOLDS on answers 1–11.** The key
    declared no build involvement. It worked from a fresh clone from GitHub
    at `85f84e1`, with its own probes and its own mutants. Nothing merged,
    and the merge word was not given.
    - **The pass-record summary** (the independent key's sixth item, staged
      verbatim, §2c item 6):

      > Second-key pass: verdict holds on 1–11; claims 12, 14 (refined), 15
      > (with the 531 ms unverified), 16, 17 confirmed; 13 disputed as a
      > general claim (counter60 yes; counter150 7/8 own-index, 2/8 ≥2×,
      > frame 149 → 39); the colour mismatch (up to L1 56 centre) is a
      > register candidate, pre-existing.

    - **The builder's cross-check, declared as such.** The builder ran the
      same eleven answers at `85f84e1` on 2026-10-19, from its own fresh
      clone and probes: answers 1–10 held and answer 11 was partly
      supported. It is not independent. The builder wrote the PR and said
      so before starting. The opencode key is the second key of record.
    - **Claims 12–17, resolved by the user's relay:**
      - **12, confirmed:** 711.91 MiB per loaded plain clip, exact.
        *Clarification:* the "30-frame cache plan" is a vestigial log
        artifact, not normative text. It is `render.rs:526–534`'s "video
        ring resident" line. That line logs `period` (the head's 30) and the
        asset's `plan`, and `asset.plan` is read nowhere else.
      - **13, disputed as stated.** The claim was that ffmpeg's
        nearest-frame identity holds with the runner-up at least 2× as far.
        That is true of `counter_60`'s sample and false as a general claim.
        On `counter_150`, 7 of 8 sampled frames are nearest their own
        index, and 2 of 8 have a ≥ 2× margin. Frame 149 ties at L1 40 and
        classifies as 39. There are two causes: the colour shift (R22), and
        the palette near-collision (frames k and k + 76 differ by L1 28).
      - **14, confirmed, with the magnitude refined:** up to L1 56 at frame
        42's centre, while A1 before R20 shows L1 7. It is filed as R22:
        pre-existing, and not this PR's defect.
      - **15, confirmed except two figures:**
        - the 531 ms stall is UNVERIFIED and is struck, because the
          register does not carry unverifiable numbers;
        - "1 in 9" is restated as 1 in 3 on CI alone. The relay labelled
          the 9 as local plus CI; the builder disputes that label below.
      - **16 and 17, confirmed exactly.**
    - **The user's rulings of 2026-10-19:**
      - §2c items 1–6 are accepted as written, with the 531 ms strike
        mandatory.
      - **Step 5's ruling stands as amended.** CI gets a backstop in R12's
        style, and zero stays the rule on the normative machine.
      - **R21** carries verified figures only. Whether the step-5 failure is
        runner capacity or code is recorded as unresolved.
      - **The machine-load thread gains a named cause:** three concurrent
        sessions (load 6.9–15.2, all passing).
    - **The builder's two-key record: it concurs on every ruling and
      disputes one label.**
      - **12:** concur. `asset.plan` is read only at `render.rs:532`. The
        ring's depth comes from `CLIP_PRELOAD_FRAMES` (`video.rs:294`) and
        §12.8.1's window.
      - **13, the dispute of the builder's own claim:** concur.
        - The claim named no clip, but its evidence was `counter_60` alone:
          frames 0, 15, 29, 30, 44, 45, 46 and 59, with margins from 2.00×
          to 8.7×. The two smallest were at 45 (L1 39 against 78) and at 59
          (40 against 82). `counter_150` was never sampled.
        - **An ffmpeg-side check (2026-10-19, 19:57:29):**
          - ffmpeg's decode of `counter_150` sits within L1 8 of the recipe
            on every frame (the maximum is at frame 122);
          - ffmpeg's frames 149 and 39 differ by L1 55;
          - the shift the builder measured on `counter_60`'s frame 59 was
            (−7, −25, −8). Applied to ffmpeg's frame 149, it lands L1 40
            from 149 and L1 41 from 39. That is the tie the key's probe
            found.
        - The engine side was not re-probed by the builder.
      - **14:** concur, and the R22 filing too.
        - The builder's "up to L1 40" was the maximum of its eight-frame
          sample. Frame 42 was not in that sample.
        - It is pre-existing. R20's diff (`5043ccf..85f84e1`) touches no
          colour-conversion code, and ffmpeg's decode of `counter_150` sits
          within L1 8 of the recipe. So the shift arises in the engine's
          decode path, not in the fixture's encode.
        - The builder's own A1 read at the head is L1 7 (rendered
          [23, 56, 94] against ffmpeg's [27, 58, 93]). That matches the
          key's pre-R20 figure.
      - **The 531 ms strike:** concur.
        - **Where the figure came from:** a tracing line from an
          instrumented throwaway binary. It was in the
          `dress-rehearsal-artifacts` of attempt 1 of run `39209726459`
          (draft #58), at frame 70: `view_ms` 530.97, of which `encode_ms`
          was 529.89. It survives only in the builder's scratch.
        - **Why no one else can check it:** a rerun replaces the run's
          artifact of the same name. The one artifact left is
          `12167916060`, created 06:41:33Z, which is attempt 6's. It is
          byte-identical to the builder's copy (`cmp`, 5 of 5 files).
          Attempt 1's artifact cannot be retrieved, and the job logs carry
          no probe lines.
        - *Annotation:* the same holds for attempts 1–5's other stall
          figures (57–133 ms) and for the report's "50–530 ms" range. They
          are struck with it. The one attempt whose figures survive
          (attempt 6) logged a View maximum of 1.02 ms.
        - **The user's ruling (2026-10-19): the added strikes are accepted,
          with emphasis.** Attempts 1–5's stall figures and the
          "50–530 ms" range cannot be verified for the same reason, the
          artifacts' replacement. The surviving attempt 6 figure (a View
          maximum of 1.02 ms) stands. The builder applied the rule to its
          own numbers without being asked, and the record notes it.
      - **"1 in 9" against 1 in 3:** concur that 1 in 3 is the figure of
        record. **Dispute the label "local plus CI"** (*the dispute stands*,
        by the user's ruling of 2026-10-19, and the label is corrected):
        - All nine runs were on GitHub's runners. Three were PR #57's
          (`39153017826` failed at `2497bc8`; `39153908984` and
          `39157683882` were green). Six were the attempts of #58's
          throwaway run `39209726459` (head `0fc45cb`), all green. No local
          run was counted.
        - The six drop out because they ran an instrumented throwaway
          binary, not R20's code as it merges, not because they were local.
        - **As ruled, the record states:** step 5 failed 1 of 3 on PR #57's
          CI (`39153017826` failed; `39153908984` and `39157683882` were
          green). The six #58 attempts are excluded because they ran an
          instrumented throwaway binary, not because they were local. All
          nine ran on GitHub's runners. ~~"local plus CI"~~ (§2c: the
          label is corrected.)
      - **R21:** concur, and with recording the split as unresolved. These
        figures can be checked by anyone:
        - step 5 failed 1 of 3 on PR #57's CI;
        - main's own CI dress trips the View watchdog in 5 of 5 runs, with
          misses 5–26 (`39005941546`, `39025809315`, `39033994454`,
          `39035305838` and `39112885332`; each artifact is present, all
          from attempt 1);
        - #58 went 6 of 6 green with step 5 clean (its job logs).
        - *The user's ruling (2026-10-19):* R21's figures stand as stated.
          They are 1 of 3 on #57's CI, main's watchdog trips in 5 of 5 runs
          with their artifacts retrievable, and #58's 6 of 6.
      - **16 and 17:** concur.
      - **The machine-load cause:** noted. Every window of the builder's
        was under 3.0 at both ends, so its data neither confirms nor
        contradicts the cause. *Annotation:* by the register's rule, runs at
        load 6.9–15.2 are void as timing evidence. Their passes stand as a
        robustness witness.

  **Merged: PR #57 is `88c8316`** (the bookend, the standing pattern of
  §2c), on the user's word of 2026-10-19 ("Ready to merge when you are"; the
  user ran the merge by hand, because the auto-mode classifier refuses the
  builder's): a squash at 2026-10-20T03:26:15Z (parent `5043ccf`), pinned to
  the reviewed head `85f84e1`. Its subject follows the convention, and its
  body equals the staged `r20-merge-body.txt` byte for byte. The merge tree
  `441aa2a` is identical to `85f84e1`'s. **SPEC v0.4.8 row 12 is ratified**:
  a plain clip plays its full length, §12.8.1's read-ahead is what the engine
  does, and §12.11.1 charges each clip's head and window. **R20 is fixed and
  leaves the register**, closed by its proving case (the battery red at
  `911fcdd` and green at the merge). Main's CI at `88c8316`, run
  `39364285184`, read per test (§2b), green on attempt 1:
  - **rust** (job `118437382891`): `TOTAL: 525 passed, 0 failed, 4 ignored`,
    with all 31 floor lines held, `r20_clip` 9 of 9 (the refill median
    0.93 ms on CI's adapter). `telemetry_tick_wires_the_live_session_counter
    ... ok`, so R10 is silent. The peak was load1 22.53 on 3 CPUs, 7.51 per
    CPU (envelope row 23). The ceiling holds at 12.09.
  - **control-plane** (job `118437382934`): `passed=198 failed=0`,
    automation 35/35, belief `ran=4 exercised=4`, effects 2/2, and every e2e
    floor held (slate-release 4, recall 2, take-duration 3, resync-end 3,
    package-reconcile 4, show-stop-version 6 ran and 1 exercised,
    resync-request 3, operator-signals 4 ran and 1 exercised).
  - **dress** (job `118437382772`): 29/29, skips 11, exercised 18. Step 5
    `ok` (R21: a fourth run of R20's code on CI, green). Drops 0, underruns
    195, watchdog trips 0 at the gate, rises of 2010 and 1995 ms, all inside
    the backstops. Step 12b: `R20 READ-AHEAD: 0 miss(es) over the 150-frame
    clip`; 12c's picture is normative-only (no recording on CI).
  - **The evidence file is retired.** `.git/nbe-staged/r20-pr-body-evidence.md`
    (sha256 `0a23b073…be93`) was removed after the bookend. *The builder
    removed it before its verbatim check passed:* the check compared the file
    with the merged body without stripping the body's blockquote markers, and
    read false. Rebuilt from the merged body with the markers stripped, its
    39 lines hash to the same sha256, so the merged body quotes it verbatim.
    The order was the builder's slip, recorded.
  - **PR #57's body** carries §2c items 2 and 5 as an appended note, so the
    reviewed text stays as reviewed.
  - **The machine-load thread gains a named cause** (the user's ruling of
    2026-10-19, on #57's two-key pass; the thread is #56's bookend, above):
    three concurrent sessions on this machine (load 6.9–15.2, all passing).
    The builder's annotation stands in its two-key record: by the 3.0 rule
    those runs are void as timing evidence, and their passes are a
    robustness witness. The local-ceiling ruling stays owed.

  **The residency honesty build: preflight stops certifying what the engine
  cannot air** (branch `loop-residency-honesty`, off `88c8316`; the user's
  order of 2026-10-19, after #57's bookend; SPEC v0.4.8 row 13, its early
  commit `a1ac9dc`). Held for the two-key pass, then the merge word.
  - **The defect, as ordered.** §12.3's ladder is unbuilt, so the engine
    holds every loop as RGBA8, but preflight planned the declared format, and
    an undeclared loop took the NV12 rung. Spec `:3123` (`:3094` in the
    order; row 12's text moved it) and the code's comments said so, and no
    guard failed. §19's "preflight is the proof a package is air-ready" was
    false for the default case.
  - **The order's premise, corrected by measurement.** The order expected the
    spec's example, a 60-frame 1080p loop declaring nothing, to report 474 MiB
    engine-actual. A throwaway engine probe (not committed; 20:54:07–20:54:17,
    2.26 → 2.29, on AC under `caffeinate`) loaded `counter_60.mp4` as a
    60-frame `videoLoop` through `show.load` and read the picture back:
    - at `vramBudgetMib: 512` the engine holds **60 frames, 474.6 MiB**, and
      draws every frame;
    - at §12.4's default 256 MiB it holds **32 frames, 253.1 MiB**, plans the
      loop as streamed, and draws source frame 31 for master frames 32–59 of
      every period (the finding below);
    - a declared `bc7` changes nothing: 60 frames at 512, as RGBA8.
    So 474 MiB is engine-actual only at a budget of 475 MiB or more. Both
    figures are proven, by `loop_residency.rs` on the engine's side, and both
    are what preflight now reports. The probe's first run (20:42:10–20:44:40)
    used the element kind `clip`, which plays an asset once, so its frames past
    60 drew black: the builder's harness error, not the engine's. Its held
    frames (32, 60, 60) agreed with the valid rerun.
  - **Scope beyond the order, and why: frames, not only the format.**
    Preflight charged a loop over its budget nothing, as if streamed (§12.11.1
    read "Σ over VRAM-resident loops"). The engine does not stream loops; it
    holds its budget's frames. Planned at RGBA8 alone, the spec's example at
    the default budget would have moved from 178 MiB to **nothing**, a worse
    lie. So every loop is charged the frames the engine holds
    (`loop_held_frames`), and row 13 amends §12.11.1 with it. This is the
    builder's reading of "engine-actual", put to the user.
  - **The commits, in order:**
    - `c78caf1`: #57's records (the first commit, per the cadence);
    - `a1ac9dc`: row 13, early (§12.3's note, §12.11.1, §19.2.1);
    - `120e135`: the proving case, red before the fix (`v04` +5 and three
      earlier assertions struck §2c; `loop_residency.rs`, 3, new; its CI floor
      of 3/3);
    - `6931c24`: the fix (`HELD_LOOP_FORMAT`, `loop_frame_cap`,
      `loop_held_frames`, `LOOP_RESIDENCY_NOTE`; preflight's plan, charge and
      note; the engine's plan and cap through the shared names, no behaviour
      change; the clip log line split; `model.rs`'s spelling; and
      `loop_residency.rs` rustfmt'd, whitespace and import order only, because
      `120e135` committed it unformatted);
    - this commit: the records.
  - **Red before the fix (`120e135`):** `v04` 7 failed, 14 passed
    (21:22:08–21:22:17, 2.38 → 2.77, clean): the five new tests and the two
    with struck assertions. `loop_residency` 3 of 3, green, because it
    measures what the engine already holds. The run's raw output is in the
    PR body's quote of the staged evidence. The unfixed binary's figures: 201,
    201, 142, 201 with no `loopBudget` error, and 23 MiB; every one exit 0,
    and none with a note.
  - **Green after (`6931c24`):** 498, 276, 498, exit 2 with
    `loopBudget … the budget's 32 at RGBA8`, and 276; every one with the note.
    *(The second key's ruling 5: at `7913bc7` the default-budget and stream
    rows exit 2, `loopFreeze`, R23; the figures stand.)*
    The first green run (21:28:03–21:28:15, 2.32 → 3.11) was VOID on load at
    its end, likely `v04`'s own parallel decoding, not proven; the gate below
    is of record.
  - **The battery** (each mutant applied in place, its mapped suites run,
    restored from a saved copy, `cmp`-identical and `git`-clean, and
    rebuilt green; every window valid, both ends under 3.0):

    | # | Mutation (class) | Killed by | Window, load |
    |---|---|---|---|
    | M1 | **The lie returns, undeclared:** an undeclared loop planned at the NV12 rung (`yuv_sampling: !has_alpha`, `declared_format: None`) | `v04`, 7 of 21: the spec's example (`got 201`), the verdict, the stream loop, the declared format, the auto test, the arithmetic test, and the 4K test (its plan moves to NV12's 21 frames) | 21:30:09–21:30:12, 2.40 → 2.60 |
    | M1b | **The lie returns, declared:** a declared format planned as declared | `a_declared_format_the_engine_does_not_hold_is_planned_at_rgba8` (`got 142 MiB`), and the arithmetic test's `bc7` case (`got 419`) | 21:30:48–21:30:51, 2.40 → 2.40 |
    | M2 | **Undercounting frames:** a loop the plan streams is charged nothing | `v04`, 4 of 21: the spec's example at the default budget (`got 23 MiB`), the stream loop, the auto test, the arithmetic test | 21:30:56–21:30:59, 2.36 → 2.33 |
    | M2b | **Undercounting frames:** a loop declaring `stream` is charged nothing | `a_loop_over_its_budget_is_charged_the_frames_the_engine_holds` (`got 23 MiB`), and the arithmetic test | 21:31:04–21:31:08, 2.23 → 2.61 |
    | M2c | **Undercounting frames:** the shared cap holds one frame short | all three sides: the unit test (`a loop over its budget holds its budget`); the engine's `a_loop_over_its_budget_holds_its_budget_not_a_window_and_not_nothing`; `v04`, 4 of 21 (`got 269 MiB`) | unit 21:32:31–21:32:32, 2.45; engine 21:33:33–21:33:38, 2.14 → 2.85; `v04` 21:34:45–21:34:49, 2.49 → 2.45 |
    | M3 | **The honesty note dropped** from the report | `the_report_says_in_words_that_loops_are_planned_at_rgba8_until_the_ladder` (`"residencyNote":null`) | 21:35:20–21:35:23, 2.49 → 2.37 |
    | M3b | **The note loses its expiry** (no §12.3, no ladder) | the same test (`the note names the assumption (RGBA8) and its expiry`) | 21:35:32–21:35:36, 2.32 → 2.61 |
    | M4 | **The verdict planned at NV12** while the charge stays honest | `the_budget_verdict_follows_the_format_the_engine_holds` (exit 0 where 2 is owed), and the 4K test | 21:36:15–21:36:18, 2.37 → 2.26 |
    | M5 | **The reference side:** the engine's ring ignores the budget | `loop_residency`'s over-budget test, 1 of 3 (`a loop over its budget holds its budget's frames`) | 21:36:36–21:36:41, 2.15 → 2.05 |

  - **The gate at `6931c24`, of record** (scratchpad `rh/gate/`; AC,
    `caffeinate`):

    | Step | Result | Window | Load |
    |---|---|---|---|
    | builds, `fmt`, `clippy -D warnings`, `tsc` | clean | 21:37:09–21:44:03 | untimed |
    | workspace (of record: the second job-scoped rerun) | **534/0/4** | 22:11:47–22:13:41 | 2.36 → 2.23 |
    | `r20_clip` | 9/9 | 21:52:08–21:52:38 | 2.36 → 2.78 |
    | `loop_residency` | 3/3 | 21:53:08–21:53:11 | 2.25 → 2.31 |
    | `v04` | 21/21 | 21:53:11–21:53:14 | 2.31 → 2.31 |
    | `npm test` (of record: its job-scoped rerun) | 198/198 | 22:06:47–22:06:55 | 2.29 → 2.58 |
    | automation / effects / belief | 35/35, 2/2, 4/4 | 21:56:22–21:56:28 | 2.06 → 1.97 |
    | e2e slate-release | 4/4 | 21:56:28–21:56:42 | 1.97 → 2.82 |
    | e2e recall | 2/2 | 21:56:57–21:57:10 | 2.20 → 2.09 |
    | e2e take-duration | 3/3 | 21:57:10–21:57:15 | 2.09 → 2.00 |
    | e2e resync-end | 3/3 | 21:57:15–21:57:30 | 2.00 → 2.22 |
    | e2e package-reconcile | 4/4 | 21:57:30–21:57:45 | 2.22 → 2.23 |
    | e2e show-stop-version | 6/6, 0 × `no video units` | 21:57:45–21:59:05 | 2.23 → 2.28 |
    | e2e resync-request | 3/3 | 21:59:05–21:59:32 | 2.28 → 2.50 |
    | e2e operator-signals | 3 pass + 1 skip (its CI seam) | 22:01:17–22:01:43 | 2.24 → 2.99 |
    | the dress | 29/29; drops 0, underruns 0; R20: 0 misses, 149 recorded counter frames, indices 1–149 | 22:02:43–22:03:45 | 2.31 → 2.50 |

    Two mid-run samples touched 3.0 inside valid windows (slate-release 3.07,
    operator-signals 3.08); both ends were under 3.0, and both steps passed.
    **Three voids, each named:**
    - **The workspace's first run** (21:47:49–21:50:23, 2.49 → 4.32): 534/0/4
      in it. The likely cause is the suite's own parallel load; not proven.
    - **`npm test`'s first run** (21:53:14–21:53:22, 2.31 → 4.17): 198/198 in
      it, the suite's eight-second parallel burst as at #57's gates; not
      proven.
    - **The workspace's first rerun** (22:04:12–22:05:47, 2.33 → 3.16, a peak
      of 4.0 at 22:04:32): **red**, `r20_clip`'s refill guard,
      `r20_a_window_refill_fits_the_frame_deadline`, median **9.93 ms**
      against its 8.33 ms bound (406 passed before cargo stopped). Not this
      PR's code (it touches no refill path), and characterized below as a
      finding; the second rerun, of record, is green.

  - **Counts.** Workspace 525 → 534 (`v04` +5; `loop_residency` +3;
    `loop_cache`'s unit +1; one test renamed, net 0:
    `an_auto_loop_the_budget_cannot_hold_is_streamed_not_charged` →
    `…_is_charged_its_budget_not_its_period`). `npm test` 198, unchanged (the
    dress's comment only). The rust job's floor lines 31 → 32
    (`loop_residency` 3/3, new). The dress 29, exercised 18 on CI, unchanged.
  - **Findings, recorded:**
    1. **A loop over its budget freezes on its last held frame** (a register
       candidate; the user's to file and to rule). At the default budget the
       spec's example draws source frame 31 for master frames 32–59, 28 of
       every 60, and preflight, now honest about the memory, still certifies
       the `auto` loop air-ready (exit 0). §12.8 requires long-loop
       read-ahead, and its rule 4 allows a frozen frame only when read-ahead
       *fails*; this engine has none for loops. ~~Whether preflight warns or
       refuses on a loop the budget cannot hold is a verdict this PR does not
       take.~~ (§2c: the user ruled, below. Filed as R23; preflight refuses it,
       landed in this PR.)
    2. **§12.10's per-loop report is not emitted.** Preflight writes no
       `loops[]` entries at all (`LoopReport` exists in the model and is never
       populated), so a plan's `reason` surfaces nowhere. That text says
       "streaming with N frames read-ahead", which this engine does not do;
       it is latent today and must not ship as it reads when the per-loop
       report is built. Pre-existing; recorded, not fixed. *(Ruled: the
       unbuilt-MUST register's U1.)*
    3. **An unprobed loop is charged on its declared period** ~~(the worst
       case)~~ (§2c, the user's ruling 5: not the worst case but an optimistic
       one; the engine can hold up to its budget's cap, whatever the declared
       period), while an unprobed clip charges nothing (row 12). Both stand
       beside the decode error the report already carries; the asymmetry is
       recorded. *(Ruled, below: an unprobeable loop fails preflight, and is
       never charged optimistically on its declared period; owner, the
       residency era.)*
    4. **The engine side was not changed in behaviour.** `HELD_LOOP_FORMAT` in
       the engine's plan and `loop_frame_cap` for its ring compute what they
       computed before (`yuv_sampling: false` already selected RGBA8, and the
       cap was `max_frames_by_budget.max(1)`). M5 and `loop_residency.rs` pin
       it.
    5. **The log line is unguarded.** The split "clip ring resident" / "loop
       ring resident" lines are corrected text with no test reading logs.
    6. **R20's refill guard is marginal under the workspace step's
       parallelism** (pre-existing; not this PR's code, which touches no
       refill path). The gate's first workspace rerun failed it, median
       9.93 ms against its 8.33 ms bound, at a load peak of 4.0 (void).
       Characterized in valid windows (scratchpad `rh/gate/`): `r20_clip` at
       the default thread count, the workspace step's, gives 7.26–7.91 ms; at
       `--test-threads=1`, CI's dedicated step, 2.11–2.40 ms. An interleaved
       A/B of two kept binaries, head's against one built from `88c8316`'s
       engine sources (distinct by `cmp`; the tree restored `cmp`-identical
       and `git`-clean before the runs), valid runs only: head 7.45, 6.47,
       7.05 ms; main 4.90, 7.39, 7.90 ms. Two runs were void (main's second,
       2.38 → 3.00; head's fourth, 2.49 → 3.26). The distributions overlap,
       and an earlier sequential batch (head 7.26–7.91 at 22:07–22:10, main
       5.20–6.45 at 22:15–22:18) was drift the interleave dissolved. So the
       margin is main's: in the workspace step the guard runs beside eight
       other GPU tests, and its bound sits 0.4–1.1 ms above their contention.
       ~~Isolating it (its own binary, or the suite serialized in the workspace
       step) is the user's to rule; nothing here changes it.~~ (§2c: ruled,
       below; the guard now runs alone in `r20_clip`'s lane, `e0f9fca`.)
    7. **The builder's slips, recorded:** `120e135` committed
       `loop_residency.rs` unformatted (formatted in `6931c24`); and the probe
       harness's `clip` element (above).
  - **The user's rulings on #59's five decisions (2026-10-19), and where
    each landed.**
    1. **The proving case carries both budgets: accepted.** 474.6 MiB at
       `vramBudgetMib: 512`; 253.1 MiB at the default budget. The relay's 474
       figure was budget-unconditional; corrected.
    2. **Frame-charging: ratified.** Frames × format is the engine-actual
       figure; format alone would have produced the 0 MiB lie. Row 13's
       §12.11.1 amendment stands.
    3. **R23 filed:** a loop over its budget freezes (the engine holds N of M
       frames and draws the last resident one for the rest of every period).
       Preflight refuses it, never warns, with the reason naming the freeze and
       the remedies (raise the budget, shorten the loop, await the ladder or
       streaming). To land in #59 if it rides the existing §12.5 verdict path
       cleanly, the builder's judgment from the tree; otherwise a fast-follow.
       **Landed in #59.** The judgment, from the tree: the refusal is the §12.5
       block's own (the same loop, the same plan, the same place); no fixture
       outside `v04` declares a loop the budget cannot hold (the dress's
       `A2_loop` is 10 frames and fits; `valid_show`'s `logo_loop.mov` is not a
       decodable file and fails already); ~~and inside `v04` the only assertion
       it falsifies is the auto test's "streaming is lawful", struck §2c~~
       (§2c, the second key's ruling 1: imprecise. #59 carries four §2c-struck
       assertion changes in `v04`. Three are in `120e135` and correct
       expectations that certified the old lie: the stream case, the `bc7`
       case, and the auto test's "not charged". The fourth is R23's
       lawfulness line, the only one R23 falsifies).
       - `d1dc16b`: the proving case, red (v04 3 failed, 19 passed;
         00:16:23–00:16:28, 2.49 → 2.37). The first attempt at this run was
         killed mid-wait when the machine rebooted (below); its rerun is of
         record.
       - `7913bc7`: the fix. Every loop is planned in the §12.5 block; a loop
         whose held frames fall short of its period (within §12.4's cap, which
         has its own refusal) freezes. A mandatory `vram` loop keeps
         `loopBudget`, which now names the freeze; any other is refused as
         `loopFreeze`, `stream` included. Raising the budget is offered only
         when some budget holds the loop (within §12.4's 512 MiB total).
         Spec: §12.5's implementation note (R23), row 13's text and cells,
         and §12.3's note. Green after: v04 22/22 (00:18:02–00:18:06,
         2.47 → 2.75).
       - Its battery (each mutant restored `cmp`-identical and `git`-clean,
         rebuilt green; every window valid):

         | # | Mutation | Killed by | Window, load |
         |---|---|---|---|
         | M6 | the refusal removed | the R23 test (exit 0 where 2 is owed) and the auto test | 00:27:17–00:27:20, 2.43 → 2.72 |
         | M6b | the refusal downgraded to a warning | the same two | 00:28:10–00:28:13, 2.31 |
         | M6c | a `stream` loop exempted | the R23 test's `stream` half | 00:28:34–00:28:37, 2.15 → 2.22 |
         | M6d | the remedies dropped | three: the R23 test (`vramBudgetMib to 475`), the auto test, the verdict test | 00:28:57–00:29:00, 2.15 → 2.22 |
         | M6e | raising the budget offered when no budget holds the loop | the auto test | 00:31:05–00:31:09, 2.37 → 2.34 |
         | M6f | the mandatory refusal no longer names the freeze | the verdict test | 00:31:29–00:31:33, 2.33 → 2.95 |
    4. **The refill guard isolated:** a serial lane or its own step, the
       8.33 ms bound kept, load discipline inherited; the A/B clearing #59 is
       recorded (finding 6). **Landed in #59 as a lane** (`e0f9fca`): a static
       tokio `RwLock` in `r20_clip.rs`, the guard taking it for writing and
       every other test for reading, so the guard never runs beside a sibling
       in any step. The bound, the suite's count and its CI floors (9/9) are
       unchanged. Measured at the default thread count, the workspace step's
       mode: **2.32 and 2.31 ms** (00:25:07–00:25:23 and 00:25:38–00:25:57,
       valid; a first, 2.68 ms, was void on load at its end, 2.43 → 3.21),
       against 7.26–7.91 ms before the lane. Two probes of the lane:
       - **L1, the isolation removed** (the guard reads the lane like its
         siblings): **7.81 ms**, 00:32:40–00:32:54, 2.41 → 2.77, and no test
         fails. The isolation is measured, not guarded by a test.
       - **M8 under the lane** (the frame path back on `write_texture`):
         killed, median 11.36 ms against 8.33 ms, 00:33:43–00:34:02,
         2.20 → 2.59.
    5. **`loops[]` unwritten: to the unbuilt-MUST register** (opened by this
       ruling, under "Cross-cutting", below; entry U1). **An unprobeable loop:
       preflight fails, never an optimistic charge on the declared period;
       recorded, owner the residency era.** Not built in #59. Preflight already
       fails such a package (its decode error), but the estimator still charges
       the loop on its declared period, and the ruling retires that charge.
       **The slips are accepted as recorded.**
  - **The machine rebooted mid-session (2026-10-20, about 23:00).** It killed
    R23's first red run mid-wait (exit 137, no output) and cleared
    `/private/tmp`, the session's scratch. The raw logs of the gate and the
    battery at `6931c24` went with it. Their figures stand as recorded in
    `4b555c8` and PR #59's body; the staged evidence lived in
    `.git/nbe-staged/` and survived. Every run since is new and named above.
  - **The gate at `e0f9fca`, of record** (scratchpad `rh/out/gate2/`; AC,
    `caffeinate`):

    | Step | Result | Window | Load |
    |---|---|---|---|
    | builds, `fmt`, `clippy -D warnings`, `tsc` | clean | 00:34:33–00:36:02 | untimed |
    | workspace | **535/0/4** (+1, the R23 test), first run | 00:39:02–00:41:36 | 2.18 → 2.31 |
    | `r20_clip` (`--test-threads=1`) | 9/9 | 00:41:36–00:42:05 | 2.31 → 2.18 |
    | `loop_residency` | 3/3 | 00:42:05–00:42:09 | 2.18 → 2.40 |
    | `v04` | 22/22 | 00:42:09–00:42:12 | 2.40 → 2.29 |
    | `npm test` | **197/198, red**: the `timeOfDay` test's rule fired twice (finding 8, below) | 00:42:12–00:42:21 | 2.29 → 2.19 |
    | `npm test`, its job-scoped rerun | 198/198 | 00:49:07–00:49:14 | 1.68 → 1.89 |
    | automation / effects / belief | 35/35, 2/2, 4/4 | 00:42:21–00:42:27 | 2.19 → 2.25 |
    | e2e slate-release | 4/4 | 00:42:27–00:42:40 | 2.25 → 2.20 |
    | e2e recall | 2/2 | 00:42:40–00:42:54 | 2.20 → 2.08 |
    | e2e take-duration | 3/3 | 00:42:54–00:42:59 | 2.08 → 2.16 |
    | e2e resync-end | 3/3 | 00:42:59–00:43:13 | 2.16 → 2.11 |
    | e2e package-reconcile | 4/4 | 00:43:13–00:43:28 | 2.11 → 2.16 |
    | e2e show-stop-version | 6/6, 0 × `no video units` | 00:43:28–00:44:48 | 2.16 → 2.31 |
    | e2e resync-request | 3/3 | 00:44:48–00:45:15 | 2.31 → 1.85 |
    | e2e operator-signals | 3 pass + 1 skip (its CI seam) | 00:45:15–00:45:43 | 1.85 → 1.97 |
    | the dress | 29/29; drops 0, underruns 0; R20: 0 misses, 150 recorded counter frames, indices 1–149 | 00:45:43–00:46:45 | 1.97 → 2.36 |

    No void: every window started and ended under 3.0.
  - **Finding 8: a `timeOfDay` rule can fire twice** (pre-existing; a
    register candidate for the user to file and rule; not this PR's code,
    whose only control-plane change is the dress's comment). The gate's
    `npm test` failed in a valid window: the rule fired twice at its instant
    (`[{ kind: 'timeOfDay', ruleId: 'tod' }, { kind: 'timeOfDay', ruleId:
    'tod' }]`, at 00:42:18), and the automation step three seconds later
    passed 35/35. **The mechanism, by reading** (`automation.ts:747–760`):
    `scheduleTimeOfDay` arms a monotonic `setTimeout` for a whole-second wall
    instant and, when it fires, re-arms from `wallNow()`. If the timer fires
    while the wall clock still reads before the instant, `next.setHours(h, m,
    s, 0)` is still ahead, so the rule re-arms for the same instant and fires
    again about a millisecond later: its action would run twice. **Measured**
    (a throwaway Node probe, not committed; 00:48:43–00:48:53, 1.55):
    **61 of 2000** `setTimeout`s armed for a wall instant fired while `Date`
    read 1 ms before it (3.05%), never more than 1 ms. The test alone, 20
    runs (00:47:43–00:48:23, 1.91 → 1.77): 0 failed, consistent with a few
    per cent per firing. The fix (not built here) re-arms from the instant
    just fired, not from the wall clock read at fire time.
  - **The two-key pass on PR #59** (independent; relayed by the user on
    2026-10-20). Verdict: **HOLDS on 1–11.** It was derived independently:
    its own packages, its own preflight build, its own mutants. M1 and M6 were
    re-killed at the settled head, `65115ed`, with `cmp` restores. Nothing
    merged, and the merge word was not given. It raised five disputes, all
    ruled and recorded (B–G below).
    - **The builder's two-key record: it concurs on all five rulings, with
      three refinements, measured.**
      - **The assertion count:** concur. `v04.rs` carries four struck
        passages: `:293–294` (the stream case), `:314–315` (the `bc7` case),
        `:456` (the auto test's "not charged as resident"), and `:459–460`
        (R23's lawfulness line). Three of them correct expectations that
        certified the old lie. *Annotation:* the record's sentence ("the only
        assertion it falsifies") was scoped to R23's ripple, where it is
        exact. Read in the record, though, it understates #59's four, and the
        correction stands.
      - **The floor:** concur. The code's charge is `min(length,
        max(maxFramesByBudget, 1))`: `loop_frame_cap`, and
        `loop_cache`'s unit test (a 16384×16384 frame, held 1). The spec's
        formula and row 13's prose dropped the floor; "never nothing" said it
        only in words.
      - **R24:** concur. The second key's 65 of 2000 independently confirms
        the builder's 61 of 2000. *Annotation:* `scheduleTimeOfDay` spans
        `automation.ts:747–760`. `:759` closes the timer's callback and `:760`
        the function, so either range cites it.
      - **Finding 9's row:** concur. The 1.5 s window is the owned defect, the
        class R19 was: a test gating on the wall clock instead of the event
        it waits for. The load boundary stays the paced-prime candidate.
      - **The protocol note:** concur, with two refinements, measured at
        02:28:55 on run `39389183605` (attempt 1's control-plane job
        `118516289915`, attempt 2's `118518591887`):
        - **The trap, confirmed.** `gh run view <run> --log --job
          <attempt-1 id>` served attempt 2. Its md5 was `dcc04cd8…`, identical
          to attempt 2's, at 2124 lines with no failure string.
        - **The job-exact route works, with a flag.** `gh api
          …/actions/jobs/<attempt-1 id>/logs` prints a one-line refusal
          ("the response contains terminal escape sequences") unless given
          `--allow-escape-sequences`. With it, the log is attempt 1's: 2162
          lines, with the failure string.
        - **A second attempt-exact route.** `gh run view <run> --attempt 1
          --log` served attempt 1 (2162 lines in the control-plane job, with
          the failure string; md5 `d6543b3c…`). It is the route the builder
          used for #59's attempt-2 read. The attempt-1 read was taken before
          the rerun, when attempt 1 was the latest, so both reads stand.
      - **The proving table:** concur. The `cachePolicy: "vram"` row already
        read exit 2. The default-budget and stream rows now join it.
  - **#59's CI at `65115ed`, read per test (§2b), each read naming its
    route.** Run `39389183605`.
    - **Attempt 1,** read by `gh run view 39389183605 --log` before the
      rerun, when it was the latest attempt; re-verified after it by
      `--attempt 1 --log` and by the job-exact API route.
      - **rust** (job `118516290204`): `TOTAL: 535 passed, 0 failed, 4
        ignored`, 32 of 32 floor lines held, `r20_clip` 9/9 (refill median
        0.75 ms), `loop_residency` 3/3. R10 was silent; the peak was load1
        13.82 on 3 CPUs, 4.61 per CPU (envelope row 25).
      - **dress** (job `118516290258`): 29/29, skips 11, exercised 18, step 5
        `ok` (R21).
      - **control-plane** (job `118516289915`): **red**. `npm test` 198/198
        and every floor through resync-request held, then operator-signals
        failed its one exercised test (R25).
    - **Attempt 2** (the failed job rerun job-scoped), read by `gh run view
      39389183605 --attempt 2 --log`. ~~GitHub reran all three jobs~~
      (§2c, at #59's bookend: only the failed job executed). The control-plane
      job ran (from 07:56:42): `passed=198`, every floor held, operator-signals
      ran 4 and exercised 1. The rust and dress jobs in attempt 2 are carried
      copies of attempt 1's, with new job ids but the same `started_at`
      (07:50:28) and the same logs; their figures are attempt 1's. The PR body
      and #59's squash message called attempt 2 "green per test" with those
      figures. The message on main cannot be edited, so it is recorded here.

  **Merged: PR #59 is `cd3f0c6`** (the bookend, the standing pattern of
  §2c), on the user's word of 2026-10-20 ("#59 is law"): a squash at
  2026-10-20T09:36:21Z (parent `88c8316`), pinned to the reviewed head
  `65115ed`. Its message equals the staged `rh-merge-body.txt`. The merge
  tree `47e2068` is identical to `65115ed`'s. **SPEC v0.4.8 row 13 is
  ratified**: preflight reports the residency the engine holds, and it refuses
  a loop the engine would freeze. Main's CI at `cd3f0c6`, run `39401905763`,
  read per test (§2b), each read naming its route:
  - **Attempt 1** (`gh run view 39401905763 --attempt 1 --log`; each job
    re-read job-exact):
    - **rust** (job `118558393155`): **red.** The workspace step stopped at
      `prompt04`: `three_frames_each_late_by_under_one_budget_trip_the_watchdog`,
      `frame 2 is late by at most one frame: 544.556708ms` (R15's sighting 2,
      with its message; the register row carries it). It printed no TOTAL.
    - **control-plane** (job `118558393218`): `passed=198 failed=0`,
      automation 35/35, belief 4/4, effects 2/2, and every e2e floor held
      (slate-release 4, recall 2, take-duration 3, resync-end 3,
      package-reconcile 4, show-stop-version 6 ran and 1 exercised,
      resync-request 3, operator-signals 4 ran and 1 exercised).
    - **dress** (job `118558392717`): 29/29, skips 11, exercised 18. Step 5
      `ok` (R21). Drops 0, underruns 192, watchdog trips 0 at the gate, rises
      of 1031 and 1940 ms.
  - **Attempt 2** (the failed rust job rerun job-scoped; read job-exact,
    job `118561952287`, which executed from 09:45:14): **rust** `TOTAL: 535
    passed, 0 failed, 4 ignored`, 32 of 32 floor lines held, `prompt04`
    17/17 in both steps, `r20_clip` 9/9, `loop_residency` 3/3.
    `telemetry_tick_wires_the_live_session_counter ... ok`, so R10 is silent.
    The pre line read load1 30.19 on 3 CPUs, 10.06 per CPU, the highest yet
    (envelope row 26), and the ceiling moves to 12.58. Attempt 2's
    control-plane and dress jobs are carried copies of attempt 1's (the same
    `started_at`), not runs.
  - **The evidence file is retired.** `.git/nbe-staged/rh-pr-body-evidence.md`
    (sha256 `a309cd86e6a7…`) was checked against the merged PR body with the
    blockquote markers stripped (identical), then removed. This time the check
    preceded the removal.
  - **The staged records** (`.git/nbe-staged/after-pr59-records.md`, A–I)
    land here: the two-key entry (above); B, G and H (above); C (§12.11.1's
    floor and row 13's wording, in the spec); D and E (R24 and R25, in the
    register); F (§2b's rerun note, now with the third trap); I (this
    bookend).
  - **PR #59's body** carries a §2c note: attempt 2 executed only the
    control-plane job.

  **R25's fix: the lane's commands wait for the load to complete** (branch
  `r25-refused-start-gate`, off `cd3f0c6`; the user's order of 2026-10-20,
  the queue after #59's item 1; test-only).
  - **The commits:**
    - `00cea7a`: #59's post-merge records (above);
    - `2ae5fd6`: the fix. `onAir()` gates on load completion: every video
      asset the engine decoded at the load ("video asset decoded") has its
      ring resident ("clip ring resident" / "loop ring resident"), bounded
      at 30 s;
    - `f1dcfc7`: the harness's engine log filter admits
      `nbe_engine::render=info`. **The builder's slip:** the gate in
      `2ae5fd6` waited for lines the filter dropped (`RUST_LOG=warn,
      nbe_engine::directive=info`), and it failed its own control on the
      stock dress ("decoded 3, resident 0", 02:55:40–02:56:15). The
      render target logs one line per ring per load, plus rung 2's
      re-acquires and a View recovery.
  - **The falsification** (the test file mutated in place and restored
    `cmp`-identical and `git`-clean after every run). The boundary was
    inflated with N extra copies of `A1_clip` in the test's copy of the
    dress. Only the refused-start test runs. On this machine the engine's
    hardware encoder acknowledges `stream.start`, so a pass is that test's
    SKIP, and a fail is the window missed.

    | Variant | Runs | Result |
    |---|---|---|
    | the fix, the stock dress (control) | 1 (03:01:08–03:01:12, 2.42 → 2.46) | pass |
    | **F1, the old construction** (the gate removed), N = 4 | 1 (02:56:15–02:56:27, 2.23 → 2.24) | **fail**: `no ack and no capability refusal`, the engine's last lines still `show.load`'s |
    | F1, N = 4 and N = 5 | 6 (03:01:12–03:21:12, all valid) | pass: it usually wins the race, sending before the render tick starts the uploads |
    | **F2, the fix**, N = 4 and N = 5 | 6 (03:01:52–03:21:59; one void at its end, 2.48 → 3.17) | pass, every run |
    | **F3, the gate released at the first ring**, N = 4 and N = 5 | 3 (03:02:07–03:24:13, valid) | **fail, every run**: `no ack and no capability refusal`, a command sent inside the boundary |

    F3 reproduces the mechanism ~~deterministically~~ *(§2c, the two-key
    pass on #60, ruled 2026-10-21: deterministic while the boundary is still
    running at the gate's first poll)*: a command sent while the rings upload
    waits behind them. F1 reproduces the defect as CI met it,
    intermittently. **Runs at N = 8 are invalid, the harness's own error:**
    11 video assets exceed the engine's decode-session cap of 8
    (`MAX_DECODE_SESSIONS`), and one refused asset, whichever came last in
    hash order, was `A1_clip` itself (`view.cut` refused: "item A1 is ERROR,
    cannot arm"). Those runs are excluded, and the falsification stays at
    N ≤ 5.
  - **The gate at `f1dcfc7`** (AC, `caffeinate`):
    - builds, `fmt`, `clippy` and `tsc` clean;
    - workspace **535/0/4** (03:26:48–03:28:48, 2.29 → 2.67);
    - `r20_clip` 9/9, `loop_residency` 3/3, `v04` 22/22;
    - automation 35/35, effects 2/2, belief 4/4;
    - every e2e green: show-stop-version 6/6 and operator-signals 3 pass + 1
      skip, both under the new filter;
    - the dress 29/29.

    **`npm test` voided three times, 198/198 in each:** 2.31 → 3.49 (gate),
    1.97 → 4.10, and 1.93 → 4.37, the last started under 2.0. Each rose more
    than 1.5 inside its own 7–8 s. The likely cause is the suite's own
    parallel burst, as on #56's night 2; it is not proven. That joins the
    machine-load thread. CI's run of the suite is the next witness.

    **The witness** (§2c, 2026-10-21): run `39409879554` at `66c6461`,
    attempt 1, the only attempt, so every job executed. The control-plane
    job `118584740694` printed `passed=198 failed=0` at 10:44:29Z. It was
    read job-exact by `gh api --allow-escape-sequences
    repos/jonquixote/nbe/actions/jobs/118584740694/logs`. The refused-start
    test was exercised there: `LANE-LEG refused` raised at 2013 ms, and
    operator-signals ran 4 and exercised 1.
  - **For the user:**
    - **The same construction stands in `show-stop.harness.ts`'s `leg()`**,
      with a 5 s window (`OUTPUT_ACK_MS`) and a wider margin. It is not
      changed here; the gate would move there unchanged.
    - **R24's risk, refined by reading** (for R24's own PR):
      `fire()`'s frame limiter audits a same-frame re-fire as
      `automation.rateLimited`, not executed, and `observedAt` for a
      `timeOfDay` fire is its scheduled instant. In production
      (`performance.now()`), an early fire's re-fire lands about 1 ms
      later, almost always in the same 33 ms frame. So it leaves a spurious
      rate-limited row, and the action runs twice only when the re-arm
      crosses a frame boundary. `automation.test.ts` injects a frozen clock,
      which puts the two fires in different frames, and that is why the gate
      saw two executions.

  **The two-key pass on PR #60** (independent; run in opencode, relayed by
  the user on 2026-10-21). Verdict: **HOLDS on 1–10**, with five disputes,
  all ruled. It was taken at the reviewed head `66c6461`, before the merge
  word.
  - **The builder's two-key record: it concurs on all five rulings, with
    refinements read from the tree.**
    - **F2's body row:** concur. The tree's table records F2's six runs with
      "one void at its end, 2.48 → 3.17". The PR body read "6 | pass, every
      run", and it is annotated (§2c): 5 kept passes + 1 void, the tree
      governing. *Refinement:* the void run passed too, so all six passed and
      five count.
    - **F3's "deterministically":** concur, struck in R25's build record
      above. F3 releases at the first ring line. Its 3 of 3 measured the case
      where the other rings were still uploading at the gate's first poll; it
      does not guarantee that case.
    - **R25's row, built and past tense:** concur (the register). At the
      head, the refused-start test is `operator-signals.e2e.ts:145–170`; the
      row's `:120–137` was the base's (`cd3f0c6`). *Refinement:* the row also
      cites the gate, `:107–132` (`awaitResident` at `:107–115`, `onAir` at
      `:118–132`).
    - **The hardening candidate:** concur, by reading. It is recorded on
      R25's row and left open.
      - `show-stop.harness.ts:261–262` splits each stdout chunk on `"\n"`
        with no carry, so a line split across two chunks becomes two entries,
        and `includes(needle)` misses it.
      - A split "video asset decoded" undercounts the decoded side and can
        release the gate early: the defect's direction, and silent.
      - A split "ring resident" undercounts the resident side and fails at
        30 s, which is loud.
      - Separately, the count is not a match: other "ring resident" lines
        can satisfy it early. The render target logs rung 2's re-acquires
        and a View recovery (the slip note above).
      - Neither has been sighted.
    - **The CI witness:** concur, read job-exact (R25's build record above).

  **#60's CI at the reviewed head, read per test (§2b).** Run `39409879554`
  (`pull_request`, head `66c6461`), attempt 1, the only attempt, so every job
  executed. Each was read job-exact by `gh api --allow-escape-sequences
  …/actions/jobs/<id>/logs`.
  - **rust** (job `118584741030`): `TOTAL: 535 passed, 0 failed, 4 ignored`
    and 32 of 32 floor lines held. `three_frames…` was `ok` in both steps
    (10:44:47Z and 10:45:53Z). The peak was the pre line, load1 23.71 on 3
    CPUs, 7.90 per CPU, under the maximum (envelope row 27, Finding R10).
  - **control-plane** (job `118584740694`): `passed=198 failed=0`, and every
    e2e floor held. operator-signals ran 4, exercised 1, failed 0.
  - **dress** (job `118584740998`): 29/29, skips 11, exercised 18. Step 5
    `ok`. Drops 1, underruns 178.

  **Merged: PR #60 is `056fed0`** (the bookend, the standing pattern of
  §2c), on the user's word of 2026-10-21: a squash at 2026-10-21T09:59:48Z
  (parent `cd3f0c6`), pinned to the reviewed head `66c6461`. The merge tree
  `c66865b` is identical to `66c6461`'s, and `git diff 66c6461 056fed0` is
  empty. **R25 is FIXED** (the register), and its hardening candidate stays
  open. Main's CI at `056fed0`, run `39581255559` (`push`), attempt 1, the
  only attempt, so every job executed (each `started_at` between 09:59:57Z
  and 09:59:59Z on this run). Each job was read job-exact by `gh api
  --allow-escape-sequences repos/jonquixote/nbe/actions/jobs/<id>/logs`:
  - **rust** (job `119159151501`, executed): `TOTAL: 535 passed, 0 failed,
    4 ignored`, and 32 of 32 floor lines held (`automation_rules` 1/1,
    `r20_clip` 9/9, `loop_residency` 3/3; `prompt04` 17 ran).
    - **R15:** `three_frames…` `ok` in both steps. The workspace step passed
      it at 10:01:02Z, between the 6.65 (10:00:55) and 5.92 (10:01:05)
      samples. The GPU/render step passed it at 10:01:54Z, after the post
      line (10:01:51Z, 3.73), with no samples inside that step.
    - **R10:** `telemetry_tick_wires_the_live_session_counter` `ok` in both
      steps (10:01:17Z, 10:02:14Z), so it is silent.
    - **Load:** the pre line read load1 25.06 on 3 CPUs, 8.35 per CPU, the
      step's peak, and the post 3.73 (envelope row 44, Finding R10). The
      ceiling holds at 13.85. The image is `macos-14-arm64`
      20260831.0302.1.
  - **control-plane** (job `119159151268`, executed): `passed=198
    failed=0`, and every floor line held: automation 35/35, belief 4/4,
    effects 2/2; slate-release 4/4, recall 2/2, take-duration 3/3,
    resync-end 3/3, package-reconcile 4/4, resync-request 3/3;
    show-stop-version ran 6, exercised 1, skipped 5; operator-signals ran 4,
    exercised 1, skipped 3, the refused start exercised (`LANE-LEG refused`
    raised at 2015 ms).
  - **dress rehearsal** (job `119159151466`, executed): `ran=29 passed=29
    failed=0 skips=11 exercised=18`. Step 5 `ok`. `DRESS-COUNTS` drops 0,
    underruns 211, watchdog trips 0.
  - **#61 untouched:** its head is `25c1114`, local and remote, last updated
    2026-10-20T11:06:28Z, and its latest run is `39412596238` attempt 3
    (below). It is held for its two-key pass.
  - **The staged records land here** (`.git/nbe-staged/after-pr60-records.md`
    A–L and `r24-records.md` A–B, the user's order of 2026-10-21): R15's
    row, R24's row, R25's row (built, FIXED, its sibling and its hardening
    candidate) and R10's row, in the register; the ambient-load note beside
    §6 (`docs/soak-protocol.md`); §2b's rerun note
    (`docs/implementation-standards.md`); Finding R10's pinned rule, its
    ceilings and envelope rows 27–44; and in this map, F3's strike and the
    CI witness in R25's build record, then the blocks above and below. R24's
    FIXED header and its exit wait for #61's merge.

  **R15's A/B** (the user's ruling of 2026-10-20, item 1; closed on
  2026-10-21 with no extension, item 6). The register's R15 row carries the
  outcome and the ruling; the method and its evidence are here.
  - **The legs:** `5043ccf` (before R20) and `88c8316` (R20's merge), as
    throwaway draft PRs #62 and #63 (#47's pattern). Each head carried its
    leg's tree on top of main, so CI's merge ref was that tree exactly.
  - **The runs:** the rust job rerun job-scoped, interleaved between the
    legs across 2026-10-20/21, 8 attempts per leg. Every attempt executed
    (each `started_at` distinct), was read job-exact, and had its R10 load
    lines recorded. The 15 green ones are envelope rows 28–42 (Finding R10).
  - **The decision rule, set before the final read** (the user's ruling of
    2026-10-20, item 3): "0/8 on the 88c8316 leg rejects the ~3-in-8
    R20-regression hypothesis at ~98% confidence — reclassify R15's
    sightings as load events and proceed to #61's rerun path. Any other
    outcome, report before acting." Under the hypothesis a run passes with
    probability 5/8, and eight passes come at (5/8)^8 ≈ 0.023.
  - **The driver's break, recorded:** v1 died at about 20:56 local on
    2026-10-20. `gh run rerun 39524251208 --job 119018005481` (leg
    `5043ccf`, attempt 6) exited 1, and v1 dropped its stderr, so the cause
    is unproven. A retry by hand at 22:43 succeeded. Driver v2 logs stderr,
    retries a failed rerun, reads a pending attempt, and sends the leg with
    fewer attempts next, so the alternation held. The gap before `5043ccf`'s
    attempt 6 was about 2h03m, not 15 min.
  - **Outcome: 0/8 on `5043ccf`, 1/8 on `88c8316`** (its attempt 8, the
    GPU/render step; the register row). The rule's 0/8 was not met, so it
    was reported before acting, and the user ruled on 2026-10-21 (the
    register row).
  - **Closed:** #62 at 08:09:24Z and #63 at 08:09:27Z on 2026-10-21,
    unmerged, each with its leg's result in a closing comment. The branches
    `r15-ab-5043ccf` and `r15-ab-88c8316` were deleted, local and remote.
    The 16 logs, the driver and the recount stay in `.git/nbe-staged/r15-ab/`.

  **#61's rerun path** (the user's ruling B4 of 2026-10-21: #61's engine
  tree is main's, so its red is not its own). Run `39412596238` at
  `25c1114`, attempt 3: the rust job rerun job-scoped (`gh run rerun
  39412596238 --job 118595961153`, 08:09:41Z).
  - **What executed:** only rust (job `119112170748`, from 08:09:50Z). The
    dress (`119112172992`) and control-plane (`119112234085`) jobs are
    carried copies of attempt 1's, with attempt 1's `started_at` (11:08:03Z
    and 11:06:30Z on 2026-10-20). They are not runs, and their figures are
    attempt 1's.
  - **rust, read job-exact** (`gh api --allow-escape-sequences
    repos/jonquixote/nbe/actions/jobs/119112170748/logs`):
    `TOTAL: 535 passed, 0 failed, 4 ignored`, 32 of 32 floor lines held,
    `automation_rules` 1/1, `r20_clip` 9/9, `loop_residency` 3/3.
    `three_frames…` was `ok` in both steps (08:11:54Z and 08:13:09Z), and R10
    was silent. The image was `macos-14-arm64` 20260831.0302.1.
  - **Load:** the whole step ran 2.09–11.08 per CPU (pre, 15 samples,
    post). The peak was load1 33.23 at 08:11:42Z, while building (envelope
    row 43; the maximum moves, Finding R10). The workspace step's
    `three_frames…` passed at 08:11:54Z, between the 9.45 (08:11:52) and
    9.46 (08:12:02) samples: R15's assertion green at the highest load the
    envelope holds.
  - **Held at the head `25c1114`** for its two-key pass, which the user
    dispatches. DO NOT MERGE until the pass lands.

  **R24's fix: a `timeOfDay` rule re-arms from the instant just fired**
  (PR #61, branch `r24-timeofday-rearm`, off `cd3f0c6`; held at `25c1114`
  for its two-key pass). Its records ride this PR: #61 was branched before
  R24's register row existed.
  - **The commits:**
    - `d7a851e`: the falsification, red. The evaluator is driven directly,
      its wall clock steered read by read: 40 ms short at arming, 1 ms short
      at the fire, then running on from the instant. Two monotonic clocks,
      frozen and live, each assert one action and no rate-limited second
      fire. Red at 03:45:00–03:45:03 (load 2.47): frozen, 2 actions in 3 of
      3 runs; live, 2 actions in 1 of 3 runs and 1 action plus a
      rate-limited row in 2.
    - `4534ec7`: the fix. `scheduleTimeOfDay(rule, after)` counts the next
      instant from the later of the instant just fired and the wall clock.
      Green 5 of 5 under both clocks (03:47:33–03:47:37, 2.27 → 2.57).
    - `25c1114`: the automation suite's CI floors move to 37 (R24's two
      tests).
  - **The builder's slip:** the falsification's first steered clock stayed
    frozen at "1 ms short", so the defect re-fired every millisecond (6
    actions on the live clock). That overstated it: a real wall clock moves
    on, and the defect re-fires once. It was corrected before the red
    commit.
  - **The battery** (`automation.ts` mutated in place, restored
    `cmp`-identical and `git`-clean; every window valid):

    | # | Mutation | Killed by | Window, load |
    |---|---|---|---|
    | M1 | the re-arm without `after` (the defect) | both R24 tests (`got 2`) | 03:47:58–03:48:01, 2.46 → 2.35 |
    | M2 | `from` always the wall clock | both (`got 2`; a rate-limited row) | 03:48:01–03:48:04, 2.35 |
    | M3 | the boundary weakened to `<` | both (`got 2`; a rate-limited row) | 03:48:04–03:48:07, 2.35 → 2.24 |

    The server-level test, `timeOfDay fires when the local wall clock reaches
    \`at\``, passed under every mutant. It catches the defect only when a
    real timer fires early, about 3% of the time, which is why the gate met
    it once.
  - **The gate at `4534ec7`** (AC, `caffeinate`):
    - builds, `fmt`, `clippy` and `tsc` clean;
    - `r20_clip` 9/9, `loop_residency` 3/3, `v04` 22/22;
    - **automation 37/37** (+2, the R24 tests; 03:55:20–03:55:24, 2.24,
      valid), effects 2/2, belief 4/4;
    - every e2e green (show-stop-version 6/6, operator-signals 3 + 1 skip);
    - the dress 29/29.

    **Four voids, every one green inside:** the workspace, 535/0/4, twice
    (2.33 → 3.33 and 2.49 → 4.02); `npm test`, 200/200 (198 + 2), twice
    (2.45 → 5.66 and 2.31 → 6.68). The machine was shared. At 04:05 the top
    processes were the builder's own session (41% CPU), iTerm (35%),
    WindowServer (20%), Microsoft To Do (13%) and opencode (8.5%), beside
    each suite's own burst. CI's run is the clean witness.
  - **CI:** run `39412547573` at `4534ec7` and run `39412596238` at
    `25c1114`, attempts 1 and 2, were red on R15 (the register row's
    sightings), and attempt 3 is green (#61's rerun path, above).
  - **The two-key pass on #61** (independent; relayed by the user,
    2026-10-21) HOLDS on 1–10. Its three disputes were ruled:
    1. `npm test` 198 → 200 in the record: the stale figure corrected. The
       body and CI agree.
    2. **R28 FILED** (the register): the automation suite hangs when
       `nbe-preflight` is not built. Its fix rides the test-hygiene PR.
    3. The R24 tests' calendar window: a note on R24's row, and the instant
       made safe in the same test-hygiene PR.
  - **The builder's two-key record:**
    1. **Concur.** #61's attempt-1 control-plane job printed `passed=200
       failed=0`, with R24's two tests `ok` (job `118593763908`, read
       job-exact by `gh api --allow-escape-sequences
       …/actions/jobs/118593763908/logs`).
    2. **Concur, reproduced** (R28's row). The sibling lines check out:
       `prompt04.rs:327` bounds `late > budget && late <= 2 * budget`, and
       `prompt11_watchdog.rs:141` bounds `l <= TRIP_BUDGET`.
    3. **Disputed, and upheld on 2026-10-22: there is no midnight window;
       there is a DST one.**
       - The evaluator reads the wall clock once per arming
         (`automation.ts:758`).
       - The R24 test's body was run with the instant forced, at 23:59:59,
         00:00:00, 00:00:01 and 12:00:00, under both clocks. `25c1114` gave
         1 action and no rate-limited row everywhere. `cd3f0c6` gave 2
         actions on the frozen clock, and 1 action plus a rate-limited row
         on the live one. So there is no midnight window.
       - Under `TZ=America/New_York`, an instant in fall-back's repeated
         hour (2026-11-01 01:30:00, the second pass, 06:30Z) resolves
         through `setHours` to the first pass (05:30Z). That is behind the
         read, so the rule arms for tomorrow, and the test sees 0 actions.
         The first pass, and UTC at both instants, read 1.
       - R24's row carries the DST note.
    - **An observation, not filed:** the same resolution governs production.
      A rule whose `at` falls in the repeated hour, armed during the second
      pass, waits a day.
  - **#61's fresh merge ref** (the user's order of 2026-10-21):
    - #61 was closed and reopened at 04:48Z on 2026-10-22, with its head
      unmoved (`25c1114`).
    - **The run:** `39724387596`, attempt 1. Every job executed (each started
      at 04:48:05Z) and checked out `baff7a4` (`25c1114` merged into
      `2df03a6`). Each job was read job-exact. The image was
      `macos-14-arm64` 20260831.0302.1.
    - **rust** (job `119644866600`): 537/0/4; 32/32 floor lines; the 4 count
      gates (prompt04 19, prompt07_overlay 13, prompt07b_graphics 14,
      bindings 3). R15's `three_frames…` was `ok` in both steps, and R10 was
      silent.
    - **control-plane** (job `119644866509`): `passed=200 failed=0`;
      automation 37/37; every floor held; the refused start raised at
      2020 ms.
    - **dress** (job `119644866418`): 29/29, exercised 18, step 5 `ok`.
      Drops 0, underruns 204, watchdog trips 0.
    - **Envelope row 47 and GPU row G3** (Finding R10): R10's maximum moves
      to 11.38 per CPU and its ceiling to 14.22. The GPU maximum moves to
      4.48 and its ceiling to 5.60.
  - **Merged: PR #61 is `13d3a7e`** (the bookend), on the user's word.
    - **The merge:** a squash at 2026-10-22T08:49:02Z, by the user's command,
      matched to `25c1114`, with the staged subject and body (`diff` clean).
      The user's terminal printed "already merged". The merge record shows
      one squash, with that subject and body. Its one parent is `2df03a6`.
    - **The tree is the tested merge ref's:** `13d3a7e^{tree}` =
      `baff7a4^{tree}` = `ad701e3`. It differs from `25c1114`'s by #60's and
      #64's files.
    - **The branch:** the remote branch outlived gh's "Deleted remote
      branch".
    - **Main's CI:** run `39750595122` (push), attempt 1. Every job executed
      and checked out `13d3a7e`, and each was read job-exact.
      - rust: 537/0/4, 32/32 floor lines; R15 `ok` in both steps; R10
        silent.
      - control-plane: 200/200, R24's tests `ok`, every floor held; the
        refused start raised at 2014 ms.
      - dress: 29/29, step 5 `ok`, drops 0.
    - **Envelope row 48 (8.08) and GPU row G4 (4.21):** both ceilings hold.
    - **R24 is FIXED by `13d3a7e`** (the register). R15's run is 4 of 10.

  **R15's construction: the three-frame pin counts lateness in frames**
  (branch `r15-three-frames-in-frames`, off `056fed0`; the user's order of
  2026-10-21, after #60's merge; test-only + `ci.yml`). R15 stays OPEN until
  the rebuilt test accrues green history (the register row).
  - **The commits:**
    - `aaff4a8`: #60's post-merge records, and every record staged since
      #60's head (above);
    - `507f54c`: the construction. A shared helper, `late_frames`, asserts
      four things for each frame, in order, that hold whatever the wall
      clock did. The loop drew the frame asked for. The frame is late, by
      construction (`sleep` sleeps at least as long as asked, and every
      delay exceeds the budget). It is one more dropped frame (§10.2). Its
      lateness is counted in frames from the loop's own measurement,
      `ceil(late / budget)`, and the slate is up exactly when the frames
      counted so far exceed 2 (§10.3). The warm-up frame has no deadline, so
      it is on time by construction. Three tests use it:
      - `three_frames_each_late_by_under_one_budget_trip_the_watchdog`
        (rebuilt): the slate is up after three late frames, always. The
        one-budget bound (counts `[1, 1, 1]`) is asserted on the normative
        machine only, by the dress rehearsal's fail-closed rule; a GitHub
        Actions runner logs `SKIP normative:`;
      - `a_stalled_late_frame_moves_the_trip_to_where_the_frames_counted_say`
        (new): a stall emulated on the second frame (400 ms more work). It
        counts at least 3, and the trip comes by the second frame;
      - `the_loops_watchdog_sums_frames_missed_and_trips_above_two` (new):
        §10.3's law with no clock. The loop's own watchdog hears counts: a
        double miss does not trip, an on-time frame resets, and three single
        misses trip on the third.
      Each via-the-loop run prints `R15-COUNTS`;
    - `b30402d`: R15's load capture wraps the GPU/render step (`gpu-pre`,
      a detached `gpu-sample` every 10 s, an `if: always()` `gpu-post`; every
      command `|| true`, both steps `continue-on-error`). The prompt04 step
      runs with `--nocapture`, so `R15-COUNTS` and `SKIP normative:` reach
      the log.
  - **The acceptance criterion** (the user's): a stalled runner must fail
    never, and a genuine frame-policy regression must fail always. Off the
    normative machine every assertion is either a lower bound a stall can
    only strengthen, or §10.3's law applied to the loop's own counts. The
    machine rule was exercised all three ways locally: normative asserted,
    `GITHUB_ACTIONS=true` skipped with its line, and `CI=1` alone refused.
  - **The falsification** (times UTC): the engine's and the test's sources were mutated in
    place. Each was restored `cmp`-identical and `git`-clean, with a fresh
    mtime, and rebuilt before the next run. A control on the restored tree
    ran before the battery and after it (10:35:12–10:35:17, 2.42 → 2.62, and
    10:42:48–10:42:52, 2.31 → 2.37), 19/19 each. AC, `caffeinate`, and every
    kept window under 3.0.

    | # | Mutation | Killed by | Window, load |
    |---|---|---|---|
    | M1 | **a dropped frame:** the deadline arm loses the second late frame (not counted, and the watchdog told it was on time) | `three_frames…` (`frame 2: 210ms of work against 200ms is late by construction`), `a_stalled…`, `a_late_view_frame…` | 10:35:48–10:35:52, 2.35 → 2.32 |
    | M2 | a dropped frame, from the watchdog only: the second late frame counted, but reported on time | `three_frames…` (`frame 3, counts [1, 1, 1]`, the slate down), `a_stalled…` (`frame 2, counts [1, 3]`, the slate down) | 10:37:43–10:37:48, 2.49 → 2.49 |
    | M3 | a constant: the watchdog hears 1 per late frame, not the measured count | `a_stalled…` (`frame 2, counts [1, 3]`, the slate down) | 10:43:48–10:43:53, 2.30 → 2.43 |
    | M4 | the threshold: the loop builds its watchdog at 3 | `the_loops_watchdog…` (miss 2, the slate down), `three_frames…` (frame 3), `a_late_view_frame…` | 10:40:14–10:40:19, 2.47 → 2.35 |
    | M5 | no accumulation: the watchdog keeps the last count, not the sum | `the_loops_watchdog…`, `three_frames…`, `a_late_view_frame…` | 10:41:20–10:41:24, 2.49 → 2.94 |
    | W1 | **the wall clock reintroduced:** `late <= budget` asserted on every machine, every frame | `a_stalled…` (`frame 2 is late by at most one frame: 411.747877ms`) | 10:42:29–10:42:34, 2.24 → 2.22 |
    | W2 | **the wall clock reintroduced:** the trip expected on the third frame by index | `a_stalled…` (`frame 2, counts [1, 3]`: the slate up, expected down) | 10:42:40–10:42:44, 2.28 → 2.34 |

    - **M3's first run was void:** 10:39:08–10:39:13, 2.47 → 3.07. It made
      the same kill, and its cause is unproven. It was rerun alone, with its
      own controls (10:43:12–10:43:16 and 10:46:15–10:46:20, 19/19).
    - **W1 and W2 pass the named test alone.** An unstalled run cannot show
      the wall clock's return, which is why the emulated stall exists: it
      makes that return show on every run.
    - **`a_late_view_frame…` (60 ms against 33 ms) killed M4 in this battery
      but not in v1.** Under M4 its outcome depends on whether a frame
      counts 2. That is a mutant's visibility, not a stall failure: its own
      assertions are lower bounds.
    - **The builder's slip (battery v1, 10:30:15–10:32:21):** the restore
      used `shutil.copy2`, which keeps the source's old mtime. Cargo skipped
      the rebuild, so W1, W2 and the control ran on M5's engine library. The
      control failed 15/4 on a `cmp`-identical tree, with M5's and W2's
      signatures. M1–M5 stood there, because each mutation's fresh write
      forced the crate's rebuild, and their kills match v2's. The whole
      battery was rerun as v2 (`copyfile`, `touch`, and a rebuild after
      every restore), and v2 is the record.
  - **One limit, put to the user:** the deadline arm's rounding above one
    budget (`ceil` against `floor().max(1)`) ~~is caught only when a stalled
    frame lands where they differ~~ *(§2c, the two-key pass on #64, ruled
    2026-10-21: it is undetected deterministically in both the named case and
    the emulated stall. `frames_late` mirrors `render.rs:876–877`'s
    expression, so the oracle follows the engine, and a `floor` mutant passed
    19/19. Only a real stall landing a frame between one and two budgets late
    shows it.)*. Pinning it always needs a clock seam in `render.rs`, which is
    engine code, not test-only. **The clock-seam candidate names both cases:**
    with an injected elapsed time, the named case counts exactly 1 and the
    stalled frame exactly 3, and a rounding change fails both.
  - **Floors:** none move. `prompt04` runs 19 (was 17) against its "at
    least one" gate, and the workspace `TOTAL` (no floor) reads 537.
    *(§2c, the two-key pass on #64, ruled 2026-10-21, the convention pinned:*
    - *a **floor line** is a gate line of the form `<suite>: ran=N …
      (floors …)`, 32 in the rust job at `9385404`;*
    - *the `<suite> tests that ran: N` lines are **count gates**, named
      separately: four, `prompt04` ≥ 1 (`ci.yml:159`), `prompt07_overlay`
      ≥ 13 (`:223`), `prompt07b_graphics` ≥ 14 (`:240`) and `bindings` ≥ 3
      (`:257`);*
    - *the workspace `TOTAL` is a summary, not a gate;*
    - *a floor moves when its threshold in `ci.yml` changes. #64 moved none
      of either.)*
  - **The gate** (times UTC unless marked local, −0700): AC, `caffeinate -i -s`, and no sleep or
    battery event in the window:
    - builds, `fmt` and `clippy --workspace --all-targets` clean;
    - `prompt04` 19/19 (10:25:29–10:25:34, 2.21 → 2.59);
    - the machine rule, all three ways;
    - the two capture steps dry-run, including with the line script missing:
      both exit 0, and no sampler is left behind;
    - the workspace **537/0/4** (10:57:19–10:59:15, 2.49 → 2.88).

    **One void:** the first workspace run, 537/0/4 at 10:53:01–10:55:33,
    ended at 3.10. The likely cause is the suite's own parallel burst after
    the builder's clippy and test builds; it is unproven.

    **The ambient load, named:**
    - opencode, the two-key session, was resident throughout (from 22:53
      local on 2026-10-20), at 0.1% CPU when checked;
    - a remote-desktop session was active at 02:05–03:00, 03:46:24–03:47:05
      and 03:56:26–03:56:56 local. Every kept run fell outside those windows.

    The control plane and the dress are untouched by this PR; CI runs them.
  - **For the user:** R15's class stands in two siblings, unchanged here
    because the ruling names `three_frames`:
    - `prompt04.rs:327`'s `late <= 2 * budget`, with 150 ms of headroom;
    - `prompt11_watchdog.rs:141`'s `l <= TRIP_BUDGET`, with 190 ms of
      headroom, in 2 tests.
    The `gpu-*` load lines are named apart from R10's window, so they extend
    R10's envelope only if the user rules that they do.

  **#64's CI at the reviewed head, read per test (§2b).** Run `39588856402`
  (`pull_request`, head `9385404`), attempt 1, the only attempt, so every job
  executed (each `started_at` between 11:01:28Z and 11:01:29Z). Each job was
  read job-exact by `gh api --allow-escape-sequences
  repos/jonquixote/nbe/actions/jobs/<id>/logs`.
  - **rust** (job `119184455758`): `TOTAL: 537 passed, 0 failed, 4 ignored`,
    32 of 32 floor lines held, no `FAIL` line.
    - **The GPU/render step** ran `prompt04` 19 (was 17), and printed
      `R15-COUNTS counts=[1, 3, 1]` (the emulated stall), `R15-COUNTS
      counts=[1, 1, 1]`, and `SKIP normative: each frame late by at most one
      budget (counts [1, 1, 1])`. `three_frames…`, `a_stalled…` and
      `the_loops_watchdog…` were `ok` in both steps.
    - **R15's GPU-step capture, its first reading:** `gpu-pre` 3.74 per CPU
      (11:04:15Z), one `gpu-sample`, `gpu-post` 3.76 (11:04:19Z). The step
      took about 4 s (GPU row G1).
    - **R10:** silent. The workspace step peaked at 7.75 (envelope row 45).
  - **control-plane** (job `119184456154`): `passed=198 failed=0`, every floor
    line held, operator-signals ran 4 and exercised 1, and `LANE-LEG refused`
    raised at 2006 ms.
  - **dress** (job `119184456190`): 29/29, skips 11, exercised 18. Step 5
    `ok`. Drops 0, underruns 185, watchdog trips 0.

  **The two-key pass on PR #64** (independent; relayed by the user on
  2026-10-21). Verdict: **HOLDS on 1–10**, with three disputes, all ruled. It
  was taken at the reviewed head `9385404`, before the merge word.
  - **The builder's two-key record: it concurs on all three, each checked
    against the tree.**
    1. **The body annotation, "no production crate file touched": concur.**
       `git diff --stat 056fed0 9385404` touches five files: `ci.yml`,
       `crates/nbe-engine/tests/prompt04.rs`, and three docs. There is no
       `crates/*/src/` file and no Cargo manifest or lock. The PR body
       carries the annotation (§2c).
    2. **The floor-counting convention, pinned: concur** (above, at
       "Floors", and in `docs/implementation-standards.md` §2b).
    3. **The `ceil` limit, widened to both cases: concur, and measured.**
       - `frames_late` (`prompt04.rs:351–353`) mirrors the deadline arm's
         expression (`render.rs:876–877`), so the oracle moves only with the
         engine.
       - `render.rs:876`'s `.ceil()` was mutated to `.floor()`, and `prompt04`
         passed **19/19** (11:41:52–11:41:57Z, load 2.11 → 2.10). It printed
         the oracle's own `ceil` counts, `[1, 1, 1]` and `[1, 3, 1]`.
       - The file was restored with `copyfile`, a fresh mtime and a rebuild
         (the engine rlib rebuilt at 04:42:05 local and the test binary at
         04:42:08, both after the restore at 04:41:57). It was then
         `cmp`-identical and `git`-clean, and 19/19 again.
       - The build record's limit is struck and widened (above).
  - **Ruled with the pass (the user's word of 2026-10-21):**
    - **R15:** fix landed by #64. It closes after 10 consecutive green rust
      jobs with no `three_frames…` failure, and any failure reopens the
      R20-residual line (the register row).
    - **The envelopes:** R10's stays workspace-step-only. The `gpu-*` lines
      start R15's GPU-step envelope under the pinned rule (Finding R10, the
      GPU-step table).
    - **The siblings** (`prompt04.rs:327`, `prompt11_watchdog.rs:141`) are
      queued as the next small test-only PR.

  **Merged: PR #64 is `2df03a6`** (the bookend, the standing pattern of
  §2c), on the user's word of 2026-10-21: a squash at 2026-10-21T11:47:14Z
  (parent `056fed0`), pinned to the reviewed head `9385404`. The merge tree
  `a8c6069` is identical to `9385404`'s, and `git diff 9385404 2df03a6` is 0
  lines. The squash subject and message equal the staged
  `pr64-merge-subject.txt` and `pr64-merge-body.md`.

  Main's CI at `2df03a6`, run `39594546676` (`push`), attempt 1, the only
  attempt, so every job executed (each `started_at` between 11:47:20Z and
  11:47:21Z). Each job was read job-exact by `gh api
  --allow-escape-sequences repos/jonquixote/nbe/actions/jobs/<id>/logs`:
  - **rust** (job `119203495290`, executed):
    - `TOTAL: 537 passed, 0 failed, 4 ignored`. 32 of 32 floor lines held,
      and the four count gates read `prompt04` 19, `prompt07_overlay` 13,
      `prompt07b_graphics` 14 and `bindings` 3. No `FAIL` line.
    - **R15:** `three_frames…` passed at 11:48:41Z in the workspace step and
      at 11:49:35Z in the GPU/render step, with `a_stalled…` and
      `the_loops_watchdog…` `ok` in both. The GPU step printed both
      `R15-COUNTS` lines and the `SKIP normative:` line. **This is 1 of the
      10** consecutive green rust jobs R15's closing rule counts.
    - **R10:** silent (11:48:56Z, 11:49:56Z). Envelope row 46 peaked at 6.25,
      so the ceiling holds at 13.85.
    - **GPU row G2:** `gpu-pre` 3.84 (11:49:32Z), one `gpu-sample`, `gpu-post`
      3.61 (11:49:36Z). **The GPU-step maximum moves to 3.84 and its ceiling
      to 4.80** (applied under the pinned rule, and reported to the user
      before it was recorded).
    - The image was `macos-14-arm64` 20260831.0302.1.
  - **control-plane** (job `119203495462`, executed): `passed=198
    failed=0`, every floor line held, operator-signals ran 4 and exercised 1,
    and `LANE-LEG refused` raised at 2015 ms.
  - **dress rehearsal** (job `119203495475`, executed): `ran=29 passed=29
    failed=0 skips=11 exercised=18`. Step 5 `ok`. `DRESS-COUNTS` drops 1,
    underruns 147, watchdog trips 0.
  - **#61 untouched:** head `25c1114`, held for its two-key pass.
  - **The staged records land here** (`.git/nbe-staged/after-pr64-records.md`
    A–I): this block; the strike and the floor convention in R15's build
    record; §2b's note; envelope rows 45–46; the GPU-step table; and R10's and
    R15's register rows.

  **R29's fix: the dress waits for the load's rings before `show.start`**
  (PR #66, branch `r29-dress-residency-gate`, off `13d3a7e`; the user's
  ruling of 2026-10-22, option C, on #65's diagnosis; test-only). The
  register row carries the sighting, the 40-run table and its ranges.
  - **The commits:**
    - `cdf635b`: R29's row, filed.
    - `7f81237`: the construction.
      - R25's predicate moves to `load-residency.ts`, shared: the load is
        complete when every video asset the engine decoded at the load has
        its ring resident. operator-signals' `awaitResident` (#60) uses it,
        with its rule unchanged.
      - The dress marks its engine log before each `show.load`. After the
        engine applies the load, it waits (30 s, R25's bound) for the
        render thread's boundary before `show.start`: at step 2, before
        step 3's clock window, at step 11, and at step 12b.
      - The mark is a character offset into the joined log, so a line
        split across stdout chunks is still counted, unlike R25's
        line-split hardening note.
      - No floor moves: the dress still runs 29.
  - **The falsification** (2026-10-22, local times; the normative machine,
    AC, `caffeinate`, head `7f81237`):
    - The dress's test file and the shared predicate were mutated in place
      and restored from `git show HEAD:` after every run, `git`-clean. Only
      steps 1–3 ran.
    - Each window started under load 2.8 (the last under 2.5) and was kept
      only if both ends were under 3.0.
    - Step 2's load was inflated, as R25's was, with 5 extra copies of
      `A1_clip`: 8 decodes, the decode-session cap.

    | Variant | Kept runs | Result | Start to epoch |
    |---|---|---|---|
    | control: the stock dress, gated | 1 (04:10:31–04:10:41, 2.69 → 2.81) | pass | 507 ms |
    | **F1, the old construction** (the gate removed) | 5 | **fail, 1 of 5** (04:28:13–04:28:31, 2.64 → 2.42); pass, 4 | 93–124 ms on the passes |
    | **F2, the fix** | 5 | pass, every run | 505–638 ms |
    | **F3, the gate released at the first ring** | 3 | **fail, every run**: `waited 3000 ms for masterClockFrame to advance`, frame 0 | none (the clock never moved in the window) |

    - **F1's failure is the defect itself.** The engine indexed the load at
      11:28:27.286Z, and the rings landed at 28.169 (`A1_clip`), 28.940,
      29.715, 30.531 and on. `show.start` went out inside that boundary,
      and the clock read 0 at every tick from 28.284 to 31.286.
    - **F1's four passes are the race won.** A `show.start` sent before the
      render tick begins the uploads is applied at once (93–124 ms), as
      R25's F1 found here (1 of 7). CI's base rate is the 40-run table.
    - **F3 reproduces the mechanism deterministically** while the boundary
      is still running at the gate's release.
    - **F2's 505–638 ms is `show.start` after the rings.** #66's CI read
      564 ms, inside the range of trees before R20 (241–1246).
    - **Voids:** 10 attempts (F1 3, F2 5, F3 2), each ending at 3.03–3.62.
      ~~Every one agreed with its variant's kept result.~~ *(§2c, #66's two-key pass: as counts.)* F1's 3 voided attempts passed step 3 (66, 99, 93 ms), F2's 5 passed (557, 541, 541, 560, 516 ms), and F3's 2 failed. Those are the outcomes of each variant's kept runs. A void counts toward no result. The cause was the
      inflated dress's own burst (8 decodes) on an ambient ~2.6: iTerm
      (with `top` open in it; the first void named `/usr/bin/top` at 43.9%),
      WindowServer, `coreaudiod` and this session. One F2 entry voided three
      times and was rerun under a 2.5 gate. It voided once more
      (04:31:43–04:32:04, 2.45 → 3.03) and was then kept (04:33:35–04:33:56,
      2.49 → 2.64).
    - **The builder's slip:** the driver kept each run's artifacts under its
      variant and attempt number, so repeated entries overwrote earlier
      ones. Every run's log line survives, and F1's failing run was copied
      out before anything overwrote it. Fixed for the rerun. The driver,
      its log and F1's failing artifacts are in `.git/nbe-staged/r29-falsify/`.
  - **The gate** (head `7f81237`; main's engine and the release preflight
    built at that head; load-gated as above, voids named):
    - `tsc` clean (04:35:16–04:35:20, 2.26 → 2.40);
    - `npm test` 200/200 (04:40:12–04:40:23, 2.27 → 2.23). Two earlier
      attempts were void, 2.40 → 5.81 and 2.44 → 4.00, each 200/200 inside:
      the suite's own parallel burst;
    - operator-signals: ran 4, passed 3, skipped 1 (04:41:54–04:42:24,
      2.48 → 2.89). The skip is the refused start: this machine's encoder
      acknowledges `stream.start`. The lost-stop, restart and held tests go
      through `onAir()`, so they ran on the shared predicate. *(§2c, #66's two-key pass: both compositions.)* On CI (`f24b393`, job `119804588742`) the composition is the mirror image: ran 4, exercised 1 (the refused start), skipped 3 (lost stop, restart and held: "no hardware H.264 encoder"). The predicate ran on both surfaces, through complementary tests. One void
      (2.23 → 3.40, `coreaudiod` beside iTerm and WindowServer);
    - the full dress, stock and gated, 29/29 (04:42:54–04:44:03, 2.43 →
      2.70). `DRESS-COUNTS`: drops 0, underruns 0, watchdog trips 0, rises
      1993 and 1996 ms. Load 1's start to epoch was 997 ms.
  - **CI at `7f81237`:** run `39765826824`, attempt 1. Every job executed
    and checked out `1348c6a` (`7f81237` into `13d3a7e`). Each job was read
    job-exact by `gh api --allow-escape-sequences …/actions/jobs/<id>/logs`,
    and the dress artifacts were fetched by `gh run download`.
    - rust 537/0/4, with 32/32 floor lines and the 4 count gates. R15's
      `three_frames…` was `ok` in both steps, and R10 was silent.
    - control-plane 200/200, every floor held. operator-signals, now on the
      shared predicate, ran 4 and exercised 1, with the refused start raised
      at 2017 ms.
    - dress 29/29, steps 2, 3 and 5 `ok`, 0 drops. Load 1's `show.start`
      to epoch was 564 ms.
    - Its envelope rows (R10 row 49 at 8.24 per CPU, and the GPU step's G5
      at 4.88, which moves that ceiling to 6.10) are staged with #61's rows
      47–48 and G3–G4. They land with #65's records, which hold rows 45–46
      and G1–G2, so the rows stay in order. R15's run is 5 of 10. *(They
      landed with #65's records: rows 47–51 and G3–G7, Finding R10.)*

  **The two-key pass on PR #66** (independent; relayed by the user on
  2026-10-23) HOLDS on 1–10. It raised four disputes, all ruled, and the
  builder concurs on each:
  1. **Both operator-signals compositions are stated** (the gate above).
  2. **The 40-run accounting is pinned** (R29's row): 22 runs without R20
     (241–1246 ms) and 18 with it (437–3333 ms).
     - Pinning found `0fc45cb` (run `39209726459`, 437 ms, attempt 6's
       artifact) to be an R20 tree: it is #57's second-key temp branch, with
       60 read-ahead mentions in `crates/nbe-engine/src` against 3 at
       `5043ccf`.
     - **The builder's slip:** #65's diagnosis had filed it under trees
       without R20, and R29's row had left it out.
     - "Only trees carrying R20 exceed 1246 ms" holds on the pinned count.
  3. **The wording "the counts agree"** is replaced: the voids above now
     read as counts.
  4. **The 6 ms is this phase's miss**, with the class finding carried in
     R29's row. Step 3's 3-tick window is in effect two ticks, decided by
     tick phase. The test-hygiene PR owns it.
  - **R25's hardening candidate** (the user's ruling, as the builder refined
    it):
    - the split line stays open for the operator-signals lane: the join
      lives in `dress-rehearsal.test.ts`, not in the shared module, and
      `show-stop.harness.ts:263–267` still splits each chunk with no carry;
    - count-vs-identity stays open for both lanes;
    - the user's earlier wording overstated the coverage, and R25's row is
      corrected.

  **#66's CI at the reviewed head `f24b393`** (docs-only since `7f81237`):
  - **The run:** `39772900798`, attempt 1. Every job executed and checked out
    `64845e8`, whose tree equals `f24b393`'s (`d249519`). Each job was read
    job-exact, and the dress artifacts were fetched by `gh run download`.
  - **rust:** 537/0/4, 32/32 floor lines, the 4 count gates. R15 was `ok` in
    both steps, and R10 was silent.
  - **control-plane:** 200/200, every floor held; the refused start raised at
    2018 ms.
  - **dress:** 29/29, drops 0. Load 1's start to epoch was 697 ms.
  - **Envelope row 50 (8.81) and GPU row G6 (3.58):** both ceilings hold.

  **Merged: PR #66 is `8d13d3d`** (the bookend), on the user's word.
  - **The merge:** a squash at 2026-10-23T09:26:35Z, matched to `f24b393`,
    with the staged subject and body (`diff` clean). Its one parent is
    `13d3a7e`.
  - **The tree is identical to the reviewed head's:** `8d13d3d^{tree}` =
    `f24b393^{tree}` = `d249519`.
  - **Main's CI:** run `39931895990` (push), attempt 1. Every job executed and
    checked out `8d13d3d`. Each job was read job-exact (`gh api
    --allow-escape-sequences …/actions/jobs/<id>/logs`). The image was
    `macos-14-arm64` 20260831.0302.1.
    - **rust** (job `120341940415`): 537/0/4, 32/32 floor lines, the 4 count
      gates. R15's `three_frames…` was `ok` in both steps (09:27:52.93Z and
      09:28:49.06Z), and R10 was silent.
    - **control-plane** (job `120341940969`): 200/200, every floor held; the
      refused start raised at 2000 ms.
    - **dress** (job `120341940725`): 29/29, steps 2, 3 and 5 `ok`. Drops 0,
      underruns 226, watchdog trips 0. Load 1's start to epoch was **65 ms**:
      the gate is live on main.
  - **Envelope row 51 (7.89) and GPU row G7 (4.18):** both ceilings hold.
  - **R29 is FIXED by `8d13d3d`** (the register). **R15's run is 7 of 10.**

  **R26's limiter (PR #65, branch `limiter-8-2`): its CI red, diagnosed, and
  its local evidence** (the user's ruled order of 2026-10-22). The build
  record follows the rebased head's CI.
  - **The commits named here are the pre-rebase SHAs,** at which the
    evidence was taken. The rebase onto `8d13d3d` (2026-10-23) left every
    engine, test and `ci.yml` patch identical: in `git range-diff`, commits
    2–7 are `=`, and `crates/` is identical between `3972e8c` and the
    rebased head.
  - **The red run, `39603865448` at `d571f93`** (attempt 1, its only
    attempt; every job read job-exact):
    - **The dress stall at step 3 is not the limiter.** It is R29, filed and
      fixed by #66.
      - The clock started 3333 ms after `show.start` was sent, and telemetry
        never stopped.
      - The limiter runs on the `nbe-audio` thread, which takes the
        `audio_commands` lock only to drain a bounded batch. Its 245 frames
        delay samples, not the clock.
      - Its cost, estimated and not measured: 96 MACs per stereo frame, plus
        an amortised O(1) min-deque. That is about 10 MFLOP/s at 48 kHz,
        under 1% of a core.
      - The pre-limiter `2497bc8` showed the same signature, at 3040 ms.
    - **The midmix 3-vs-2 (`prompt04_midmix.rs:526`) is behavior, not
      alignment.** It was proven before a line changed (2026-10-22, AC,
      `caffeinate`):
      - reproduced at `d571f93` (03:18:08–03:18:22, 2.60 → 2.62), with
        `prompt06` 32/32, `prompt13_recall` 6/6 and `limiter` 9/9 beside
        it;
      - the queue printed `[SetLimiterCeiling { dbtp: -1.5, source: "show"
        }, TakeItem A2 t0 1, TakeItem A3 t0 11]`;
      - with only `show.load`'s ceiling push removed, it read 20/20;
      - the restored tree, rebuilt, read 19/20 again.

      `show.load` publishes the L1 ceiling on the shared queue, and that
      harness runs no audio driver to drain it. The fix (`3972e8c`): the
      test clears the load's intents, as `prompt13_recall.rs:179` does.
    - CI's workspace step stopped at that first failing binary, so CI never
      ran #65's full workspace.
  - **The battery** (`3972e8c`; 2026-10-22, 03:39–04:03; AC, `caffeinate`):
    - Each entry was an exact, count-checked replacement. Each restore wrote
      the file back from `git show HEAD:`, which gives a fresh mtime, and was
      checked `git`-clean.
    - Each window ran `limiter`, `prompt06` and the limiter unit tests with
      `--no-fail-fast`. It started under load 2.8 and was kept only if both
      ends were under 3.0.
    - The driver and its log are in `.git/nbe-staged/limiter-battery/`. The
      first armed battery was lost with the session scratch in the disk
      cleanup, and was rebuilt from the limiter plan.
    - The controls, before and after (03:42:58, 2.68; 04:03:25, 2.44), read
      9/9, 32/32, 2/2.
    - **17 of 17 killed:**

    | # | Mutant | Kept window, load | Killed by |
    |---|---|---|---|
    | M01 | the clamp at full scale (R26 itself) | 03:47:24–03:47:28, 2.71 → 2.71 | F1, F2, F3, F4, F7, F9, the show's-ceiling test, **F10**, F8's precondition, and the moved trigger and voice tests |
    | M02 | a clamp at the ceiling | 03:49:20–03:49:24, 2.75 → 2.61 | F2, F3, F4, F7, F9, **F10**, and the moved trigger and voice tests |
    | M03 | no lookahead (the gain on the undelayed frame) | 03:50:18–03:50:21, 2.65 → 2.52 | F1, F4, F7, F9, the show's-ceiling test, **F10**, and the moved trigger and voice tests |
    | M04 | the gain one sample late | 03:52:56–03:53:00, 2.64 → 2.67 | F1 (the +20 dBFS impulse) |
    | M05 | no release | 03:53:50–03:53:55, 2.53 → 2.57 | F6 |
    | M06 | a 10 ms release, not L4's 100 ms | 03:56:30–03:56:34, 2.46 → 2.34 | F6 |
    | M07 | the right channel unlinked | 03:57:13–03:57:16, 2.70 → 2.56 | F7, F1, the show's-ceiling test, **F10**, and F8's precondition |
    | M08 | the gain below unity on quiet signals | 03:57:50–03:57:55, 2.53 → 2.57 | F4, F6, F9 |
    | M09 | an allocation per frame | 03:58:15–03:58:19, 2.54 → 2.58 | F8, and `rendering_a_block_allocates_nothing` |
    | M10 | `latency_frames()` one high | 03:58:39–03:58:44, 2.78 → 2.64 | F9, F4, and the latency unit test |
    | M11 | sample-peak detection only (L2) | 03:59:19–03:59:23, 2.72 → 2.58 | F2 |
    | M12 | `show.load` ignoring `truePeakDbtp` | 03:59:58–04:00:03, 2.51 → 2.47 | the show's-ceiling test |
    | M13 | the attack stepped | 04:00:22–04:00:27, 2.74 → 2.74 | F5 (9 samples, against 240) |
    | M14 | the ceiling dropped where it is applied (a disclosed extra) | 04:01:02–04:01:06, 2.79 → 2.72 | the show's-ceiling test (its apply half) |
    | RF1 | re-falsification: the trigger starts at full gain | 04:01:26–04:01:31, 2.70 → 2.73 | the moved `a_soundboard_trigger_is_audible_within_20ms` |
    | RF2 | re-falsification: the mute steps | 04:02:06–04:02:10, 2.52 → 2.40 | the moved `muting_a_bus_ramps_instead_of_stepping`, and both guest-mute tests |
    | RF3 | re-falsification: the voice is cut | 04:02:30–04:02:35, 2.74 → 2.60 | the moved `stopping_a_soundboard_voice_ramps_it_out` |

    - **F10, the rebuilt R26 test,** is re-falsified by the clamp (M01) and
      also killed by M02, M03 and M07. So each of the four moved tests still
      catches its original mutant.
    - **Three voids** (CONTROL-before a1, M03 a1, M07 a1; ending at
      3.01–3.23), each rerun and kept. **The cause, from each `ps`:**
      XprotectService (92.8–94.5% CPU) and `syspolicyd`, macOS assessing
      each freshly built test binary. Beside them, once, was CleanMyMac's
      HealthMonitor, and once airportd and WiFiAgent. XProtect joins the
      ambient-load list.
  - **The valid red run, at `b7dcc43`** (04:49:14–04:49:17, 2.32, kept):
    - `limiter.rs` 0/3:
      - F1: "a sample left at 0.00 dBFS, over the -1.50 dBTP ceiling";
      - F2: "the true peak left at -0.04 dBTP … its samples peak at -3.01
        dBFS";
      - F3: "a run of 19 samples at one magnitude".
    - `prompt06` 30/31, with F10 red: "1.000, against 0.841".
  - **The local gate, at `3972e8c`** (04:49–05:21):
    - builds, `fmt` and `clippy -D warnings` clean;
    - **kept and green:**
      - prompt04 19/19;
      - prompt04_midmix 20/20;
      - limiter 9/9;
      - prompt06 32/32;
      - prompt07_overlay 13/13;
      - prompt07b_graphics 14/14;
      - bindings 3/3;
      - the dress 29/29 (05:20:05–05:21:06, 2.39; `DRESS-COUNTS` drops 0,
        underruns 0, watchdog trips 0).
    - **The workspace read 549/0/4 three times, and `npm test` 198/198
      three times. Every window was void, on the suite's own burst** (ends
      3.34–6.71). This is the first full workspace read of #65. CI at the
      rebased head is its clean witness.
    - **The master peak (the plan's named check)** was −15.00 dBFS, the same
      as #66's limiter-free CI dress. The limiter is transparent on the
      dress's program, as F4 requires.
    - The normative machine's dress is not the surface of the CI-class
      stall (R29).

  **R26's fix: the master bus gets a true limiter** (PR #65, rebased onto
  `8d13d3d`; the build record). R26's row carries the defect.
  - **The commits.** They are rebased; the pre-rebase SHAs, at which the
    evidence above was taken, are in brackets. Commits 2–7 are
    patch-identical by `git range-diff`.
    - `4b68e11` (`1a71e43`): the records. #64's, #61's and #66's batches,
      the device-audio design record, R26–R29 and U2–U5, and #65's
      diagnosis and local evidence (above).
    - `0efa4fc` (`1e26772`): SPEC v0.4.8 row 14, early: the master limiter
      as ruled (§8.2's note). U6 and U7 filed.
    - `8d2a9e8` (`b7dcc43`): the falsification, red: F1, F2, F3 and F10
      against the clamp (the valid red run, above). *(§2c, #65's two-key
      pass: `limiter.rs` there holds 3 tests, F1, F2 and F3, all red; F10 sits
      in `prompt06`, 30/31 there. The suite reaches 9 with the limiter,
      `088180c`: F4, F5, F6, F7, F9 and the show's-ceiling test.)*
    - `088180c` (`f1074f4`): the limiter. `MasterLimiter`, in
      `crates/nbe-engine/src/limiter.rs`, replaces the clamp:
      - a 4× true-peak detector (BS.1770-4 Annex 2, 12 taps a phase),
        stereo-linked;
      - a 5 ms lookahead, whose attack ramps over the whole window (L3);
      - a 100 ms exponential release (L4);
      - the ceiling from the show's `truePeakDbtp` at `show.load`,
        −1.5 dBTP by default (L1), logged;
      - `latency_frames()` reports 245: 240, plus the detector's 5.
    - `2025c11` (`aec8116`): prompt06's three latency-aligned tests re-based
      on `latency_frames()` (AC-13's trigger, the mute ramp, the voice-stop
      ramp); F8, a limiting master allocates nothing.
    - `be8fc0f` (`d571f93`): `ci.yml`'s `limiter` floor line, so floor
      lines go 32 → 33.
    - `39c05fb` (`3972e8c`): the midmix take pin clears the load's intents
      (#65's diagnosis, above).
  - **The counts:**
    - the workspace 537 → 549 (`limiter.rs` 9, the limiter unit tests 2,
      prompt06's F8 1);
    - prompt06 31 → 32;
    - floor lines 32 → 33 (the floor convention stands, #65's two-key pass:
      33 `ran=` floor lines, the 4 count gates apart, as pinned at #64);
    - `npm test` 200, unchanged by #65.
  - **CI at `39c05fb`, the clean full-workspace witness:**
    - **The run:** `39933318916`, `pull_request`, attempt 1. Every job
      executed and checked out `161deb1` (`39c05fb` into `8d13d3d`). Each job
      was read job-exact by `gh api --allow-escape-sequences
      …/actions/jobs/<id>/logs`. The image was `macos-14-arm64`
      20260831.0302.1.
    - **rust** (job `120346686856`):
      - **549/0/4**, with no `test result: FAILED`;
      - 33/33 floor lines, among them `limiter` 9/9, `prompt04_midmix` 20/20,
        `r20_clip` 9/9 and `automation_rules` 1/1;
      - the 4 count gates (prompt04 19, prompt07_overlay 13,
        prompt07b_graphics 14, bindings 3);
      - R15's `three_frames…` `ok` in both steps (09:40:41.30Z and
        09:41:50.86Z); R10 silent.
    - **control-plane** (job `120346686890`): 200/200, every floor held; the
      refused start raised at 2011 ms.
    - **dress** (job `120346688475`):
      - 29/29, steps 2, 3 and 5 `ok`; drops 0, underruns 226, watchdog
        trips 0;
      - load 1's start to epoch 737 ms, with R29's gate in place;
      - the engine logged `master limiter ceiling in force ceiling_dbtp=-1.5
        source="show" latency_frames=245` once per load (3 loads). *(Its
        surface, named on #65's two-key pass: `tracing::info!` in
        `audio_control.rs:254–262`, on the `nbe-audio` thread when the driver
        drains the queue (`audio_driver.rs:240`), a cycle after `show.load`
        pushes it (`directive.rs:381`); target `nbe_engine::audio_control`,
        INFO, the engine's stdout. The engine's `tracing-subscriber` 0.3.23 is
        built without `env-filter`, so `fmt::init()` reads `RUST_LOG` as a
        `Targets` filter, INFO when unset (`fmt/mod.rs:1203–1225`): the line
        shows with `RUST_LOG` unset or `info`, and not under `warn` (most e2e
        harnesses) or the lane's filter (`show-stop.harness.ts:258`). It is in
        no telemetry and no audit row. D5's ruled amendment puts the ceiling
        on the tick's audio line.)*
      - the master peak maxed at −15.00 dBFS over 63 ticks, as on the
        limiter-free main: unchanged.
    - **Envelope row 52 (8.56) and GPU row G8 (3.75):** both ceilings hold.
      R15's run is 8 of 10.
  - Held DO NOT MERGE for its two-key pass. At the merge, R26's header is
    struck FIXED by the merge SHA.

  **The two-key pass on PR #65** (independent; relayed by the user,
  2026-10-23) HOLDS on 1–12. Its four disputes were ruled, and the builder
  concurs on each:
  1. **R30's filing lands with the test-hygiene PR.** #65's witness is CI at
     its code head `39c05fb`. `f700795` is docs-only, and its run is red only
     on the sighting below, in a control plane identical to main's.
  2. **The ceiling log's surface is named** (above, in the CI bullet). It is
     in no telemetry and no audit row. The user's ruling on the builder's
     observation: D5's amendment (`docs/device-audio-design.md`) puts the
     ceiling in force, value and source, read-only, on the tick's audio line
     in the device-audio build PR. It is sized small; if it grows, its own
     row.
  3. **The floor convention stands.** CI at `39c05fb` printed 33 `ran=` floor
     lines (the 32, plus `limiter: ran=9`) and the 4 count gates apart:
     prompt04 19, prompt07_overlay 13, prompt07b_graphics 14, bindings 3.
  4. **The red commit's 3-test suite is stated** (the commits, above).

  **A sighting at #65's head `f700795`, owed to R30:**
  - **The run:** `39934216106`, `pull_request`, attempt 1; merge ref
    `7e26b48`. The rust and dress jobs are green. **The control-plane** (job
    `120349653488`, read job-exact) failed in operator-signals' refused
    start: "not before the window: raised after 1994 ms" (`:168`). The alert's
    own reason reads "not acknowledged within 2000 ms".
  - **Not #65's:** `packages/` is identical to main's. It was not rerun.
  - **The mechanism** is in R30's row, filed by the test-hygiene PR with its
    fix.

  **Merged: PR #65 is `ccef706`** (the bookend), on the user's word.
  - **The merge:** a squash at 2026-10-23T11:28:12Z, matched to `f700795`,
    with the staged subject and body (`diff` clean). Its one parent is
    `8d13d3d`.
  - **The tree is identical to the reviewed head's:** `ccef706^{tree}` =
    `f700795^{tree}` = `f08fefc`.
  - **Main's CI:** run `39946291340` (push), attempt 1. Every job executed
    (11:28:19–11:28:21Z), and each checkout reports `ccef7067…`. Each job was
    read job-exact by `gh api --allow-escape-sequences
    …/actions/jobs/<id>/logs`, and the dress artifacts were fetched by `gh run
    download`. The image was `macos-14-arm64` 20260831.0302.1.
    - **rust** (job `120389662330`): 549/0/4, with no `test result: FAILED`;
      33/33 floor lines (`limiter` 9/9); the 4 count gates. R15's
      `three_frames…` was `ok` in both steps (11:30:20.22Z and 11:31:31.27Z),
      and R10 was silent.
    - **control-plane** (job `120389662291`): 200/200, every floor held. The
      refused start raised at 2000 ms, the edge of R30's class.
    - **dress** (job `120389662195`): 29/29, steps 2, 3 and 5 `ok`. Drops 0,
      underruns 207, watchdog trips 0; load 1's start to epoch 727 ms. The
      master peak was −15.01 dBFS over 57 ticks, and the ceiling was logged 3
      times.
  - **R26 is FIXED by `ccef706`.** SPEC v0.4.8 row 14 is ratified by the
    merge (`docs/spec.v0.4.md`'s changelog: "(merged as `ccef706`)").
  - **The envelope rows and R15's count, corrected here.** #65's bookend
    report gave `ccef706` row 53, G9 and R15 at 9 of 10. It missed
    `f700795`'s rust job (run `39934216106`, job `120349653140`, read
    job-exact). That job is green, 549/0/4, with `three_frames…` `ok` in both
    steps (09:47:40.42Z, 09:48:45.51Z) and R10 silent. Only its control-plane
    was red (the sighting).
    - So `f700795` is R10 row 53 (peak 9.79) and GPU row G9 (4.25).
    - `ccef706` is row 54 (peak 10.10) and G10 (3.66).
    - **R15's run is 10 of 10:** `2df03a6`, `d571f93` (ruled not to break
      it), #61's fresh ref, `13d3a7e`, `7f81237`, `f24b393`, `8d13d3d`,
      `39c05fb`, `f700795`, `ccef706`.
    - **R15's closing condition is met** (the user's ruling of 2026-10-21:
      10 consecutive green rust jobs with no `three_frames…` failure). Its
      closure is the user's word.
    - **The builder's slip:** the miss is recorded here and in R15's row.
  - The backup branch `limiter-8-2-pre-rebase` was deleted, the bookend
    green.

  **The test-hygiene PR** (branch `test-hygiene`, off `ccef706`; the user's
  order of 2026-10-23; test-only). Its five items, as ruled and queued. Each
  falsification was mutated in place, restored from `git show HEAD:` (a
  fresh mtime) and checked `git`-clean, on AC under `caffeinate`. Each
  window started under load 2.6 and was kept only if both ends were under
  3.0. The driver (`fx.py`), its plans and every log are in
  `.git/nbe-staged/hygiene/`.
  - **The commits:**
    - `3e9eadd`: #65's records (above).
    - `57977b5`: **R28.**
      - The handle: 31 listening TCP servers, one per skipped test. They
        were found by listing the process's active resources 15 s in.
      - The cause: node:test (v22.17.0) runs `beforeEach` for a test that
        skips itself with a runtime `t.skip()`, and never its `afterEach`.
        A probe in isolation logged `beforeEach`, the body, and no
        `afterEach`.
      - The fix: a shared `test-preflight.ts` exports `NO_PREFLIGHT`, and
        every preflight-gated test skips by option, naming the binary. That
        covers the 47 runtime skips in four suites, and six tests in three
        more that failed without naming it.
      - `ci.yml`'s two "they SKIP" comments are corrected §2c.
    - `45a31a2`: **R15's siblings**, on #64's frame-count construction.
    - `71e76e1`: **R24's DST-safe instant.**
    - `ccb361a`: **R30**, timed from before the send.
    - this record. **Item 5** (step 3's tick phase) is a noted limitation
      on R29's row, by the user's ruling: no fix, the record is the
      deliverable.
  - **R28's falsification** (times local, 2026-10-25). The red was rerun
    for durable logs: the first red's logs (2026-10-23) were lost with the
    session scratchpad, **the builder's slip**.

    | Run | Binary | Result |
    |---|---|---|
    | red at `3e9eadd`: automation, alerts-server, belief, render-channel | missing | **exit 124** at 60 s, each (03:55:05–03:59:51, kept) |
    | red: companion, server, v04 | missing | 2 failed each, unnamed ("'error' !== 'ok'"; "preflight could not run (exit 127): no stderr") |
    | red: dispatch | missing | exit 0 (its `beforeEach` opens nothing) |
    | red: `npm test` | missing | **exit 124** at 180 s (04:00:13–04:03:13, kept) |
    | green at `57977b5`: all seven suites | missing | exit 0, every gated test skipped: "nbe-preflight not built: /nonexistent/nbe-preflight does not exist" (03:34:36–03:36:02, kept) |
    | green: `npm test` | missing | exit 0 in ~11 s, 200 tests, 0 failed, 64 skipped; three windows, all void on its own burst (3.04–4.87) |
    | **M1**: one automation test back to a runtime skip | missing | **exit 124** (03:43:19–03:44:19, kept) |
    | control: all seven suites | built | 37/37, 5/5, 4/4, 24/24, 9/9, 4/4, 15/15 (03:44:19–03:44:52, kept) |

  - **R15's siblings, fail-never and fail-always** (the runner's env is
    `CI=true GITHUB_ACTIONS=true`):

    | Run | Construction | Env | Result |
    |---|---|---|---|
    | W1: sibling 1's work 750 → 1100 ms (a stall) | old, `57977b5` | — | **fail**: "late by more than one frame and at most two: 810.64 ms" |
    | W2: `JUST_LATE` 210 → 450 ms | old | — | **fail**, both tests: "late by at most one budget: 259.87 ms" |
    | controls | new, `45a31a2` | normative and runner | pass (1/1, 4/4) |
    | W1 | new | runner | **pass**: counts `[3]`, the slate up, the law holding; `SKIP normative:` |
    | W1 | new | normative | fail: "…so it counts 2", `[3]` (the bound still asserted here) |
    | W2 | new | runner | **pass** (255 ms late); `SKIP normative:` |
    | W2 | new | normative | fail, both tests (the bound still asserted here) |
    | R1: the watchdog built at 1 | new | runner; `prompt04`'s sibling 1 (`--exact`) | **killed** at `prompt04.rs:388`, the law check in `late_frames`: "the slate is up exactly when the frames counted exceed 2 (§10.3): frame 1, counts [2]"; 0/1 |
    | R2: a late frame keeps the on-time run | new | runner; `prompt11_watchdog`, all 4 | **killed** by `one_on_time_frame_does_not_clear_and_a_late_frame_restarts_the_run` (`:253`): "a late frame restarts the on-time run"; 3/4 |
    | R3: a trip on every late frame | new | runner; `prompt11_watchdog`, all 4 | **killed** by `trips_and_clears_are_counted_once_each_and_ride_the_tick` (`:209`): "one trip per episode, not per frame", (3, 0); 3/4 |

    Every window was kept (04:04:28–04:13:56). **The kill map, per suite**
    (§2c, #67's two-key pass, ruling 3): `prompt04` kills R1, and
    `prompt11_watchdog` kills R2 and R3; W1 is `prompt04`'s fail-never leg and
    W2 `prompt11_watchdog`'s. **Not covered:** each mutant ran against one
    suite; the cross (R1 against `prompt11_watchdog`, R2 and R3 against
    `prompt04`) was not run.
  - **R24's DST-safe instant** (the instant forced to 2026-11-01T06:30:00Z,
    01:30 EST, the repeated hour's second pass):
    - old, `45a31a2`: under New York, **red**, "one action for one instant;
      got 0", both clocks. The controls pass: unforced, the first pass
      (05:30Z) and the second pass under UTC.
    - new, `71e76e1`: under New York, **green**, with every control passing.
      With R24's own defect restored at that instant, **red**: "got 2"
      (frozen), and a rate-limited row (live).
    - Every window was kept (04:15:17–04:19:10).
  - **R30**, the refused start made to exercise on the normative machine by
    `outputs.stream.tapPath: cpuReadback` (`E_NO_ZEROCOPY`, the
    configuration refusal decided before any probe):

    | Run | Construction | `raisedAfterMs` | Result |
    |---|---|---|---|
    | refusal forced, ×3 | old, `71e76e1` | 2022, 2011, 2018 | pass, on the poll's margin |
    | + a 40 ms reply injected | old | 1978 | **red**: "not before the window: raised after 1978 ms" |
    | refusal forced, ×3 | new, `ccb361a` | 2025, 2028, 2023 | pass |
    | + a 40 ms reply injected | new | 2015 | **pass** |
    | the server's window at 1900 ms | new | 1920 | **red**: "not before the window: raised after 1920 ms" (`insideWindow` empty) |
    | the server's window at 1500 ms | new | 1502 | red, caught first by "before the window: nothing" |
    | control, unforced | new | none | 3 pass, 1 skip (this machine's encoder acknowledges) |

    Every window was kept (04:21:28–04:25:24).
  - **The gate at `ccb361a`:**
    - `fmt`, `clippy --workspace --all-targets -D warnings`, `tsc` and the
      builds are clean.
    - **The workspace: 549/0/4, kept** (04:34:20–04:36:17, 2.43 → 2.63),
      after one void (549/0/4 inside).
    - prompt04 19/19 and prompt11_watchdog 4/4 (kept).
    - **`npm test` 200/200, kept** (04:37:38–04:37:49, 2.49 → 2.34).
    - operator-signals 3 + 1 skip (kept).
    - **The dress: 28/29, kept** (04:39:58–04:41:03, 2.59 → 2.95), after a
      void that read 29/29. That is a sighting, below.
  - **A sighting in the gate's dress, not this PR's:** no engine or dress
    file is touched.
    - `[R20] step 12c`: "the counter's recorded frames must run in order; 1
      went backwards, first at {"t":1.433333,"k":41}".
    - The logged sequence reads `1.37:41 1.40:42 1.43:41 1.47:44`. Frame
      43's slot holds 41, and 43 never appears.
    - **Not a misread colour:** frames 41 and 43 differ by L1 270, and R22's
      worst decode shift is L1 56. The recording at t=1.43 holds frame 41's
      picture.
    - The run's telemetry: read-ahead misses 0, dropped frames 0, the record
      tap `zeroCopy`. Its drift was 2.0 frames, against 0.0 in the void
      before it.
    - The artifacts are kept in `.git/nbe-staged/hygiene/gate/dress-sighting/`.
      Not rerun. It is the user's to rule: a stale frame in the zero-copy
      record path, or the View's ring.
      *(§2c, the user's ruling of 2026-10-25 on #67's two-key pass: filed as
      **R31**, with the 12c probe as its disposition; the register.)*
  - **#67's CI, read per test** (each job read job-exact):
    - **Run `40190112362` at `ad260e4`**, attempt 1 (merge ref into
      `ccef706`). rust **red** at the `prompt11_watchdog` floor (`ran=4
      skipped=2 exercised=2`): the floor counted the new `SKIP normative:`
      lines as skipped tests. **The builder's slip:** the suite was run under
      the runner's env during falsification, the floor's own script was not.
      Fixed by `e7600aa`, its script then run locally under the runner's env
      (`ran=4 skipped=0 exercised=4`). Its workspace step was 549/0/4, and
      `three_frames…` `ok` in both steps. The control plane was red on a
      first sighting in package-reconcile, filed as **R32**. The dress was
      green.
    - **Run `40190643535` at `e7600aa`**, attempt 1 (merge ref `6dca3ce`).
      rust 549/0/4, 33/33 floor lines (`prompt11_watchdog` 4/4/4), the 4
      count gates, `three_frames…` `ok` in both steps, R10 silent;
      control-plane 200/200, package-reconcile 4/4, the refused start 2022 ms
      from the send; dress 29/29, 12c `ok`, drops 0, underruns 210, trips 0.

  **The two-key pass on PR #67** (independent; relayed by the user,
  2026-10-25) HOLDS on 1–10. Its four disputes were ruled, and the builder
  concurs on each:
  1. **R31 and R32 are filed** with the device-audio PR's records: R31, the
     dress's 12c stale frame, with its 12c probe (the user's specification
     of 2026-10-26), a second sighting jumping the queue; R32,
     package-reconcile's resync, on watch.
  2. **The floor-count convention joins the register's standing
     conventions** (`docs/soak-protocol.md` §5): 33 floor lines and the 4
     count gates as of #67, a floor line counted by its `ran=N skipped=`
     form (`preflight automation` has a space; a one-word pattern counts 32,
     as the builder's first tally did).
  3. **The fail-always kill map is named per suite** (above).
  4. **R15 is CLOSED** (the user's word of 2026-10-25), and the R20-residual
     line is the permanent watch. *(§2c: at `ad260e4` `three_frames…` was
     `ok` in both steps; the builder's staged note said the workspace step
     only.)*

  **Merged: PR #67 is `3afb73b`** (the bookend), on the user's word.
  - **The merge:** a squash at 2026-10-26T00:38:14Z, matched to `e7600aa`,
    with the staged subject. Its one parent is `ccef706`.
  - **The tree is identical to the reviewed head's:** `3afb73b^{tree}` =
    `e7600aa^{tree}` = `1bd7c04`.
  - **Main's CI:** run `40250323983` (push), attempt 1. Every job executed
    (00:38:20Z), and each was read job-exact.
    - **rust** (job `121340580334`): **red, on R10's sighting 3** (Finding
      R10), not #67's. The workspace step was 549/0/4, and the 4 count gates
      held. 17 of 33 floor lines held; the 18th, `prompt10_rtmp` (named step
      19, UI 25), failed; named steps 20–38 (UI 26–44) were skipped, among
      them `prompt11_watchdog`'s floor, so #67's floor fix has no main
      reading (its witness is `e7600aa`'s run). `three_frames…` `ok` in both
      steps.
    - **control-plane** (job `121340580557`): 200/200, package-reconcile
      4/4; the refused start 2018 ms.
    - **dress** (job `121340580717`): 29/29, 12c `ok`; drops 0, underruns
      187, watchdog trips 0.
  - **R28 and R30 are FIXED by `3afb73b`.** R15 is CLOSED: "closed at 10,
    verified to 13".
  - **The user's rulings of 2026-10-25, on the bookend:**
    1. **R10's sighting 3:** load-blind, not clean, not environment. R10
       moves to hunt pending-active, and the `[ci-load]` extension is a small
       workflow PR ahead of the device-audio build (#68, below).
    2. **Steps count, per the rule's wording:** envelope rows 55–57 and
       G11–G13; R10's ceiling moves to 19.13 and the GPU step's to 7.46
       (Finding R10). The mixed-step rule is pinned there.
    3. The merged branch on `origin`: the builder's deletion was refused by
       the session's permission classifier, and the user deleted it on
       2026-10-26, with #69's.

  **The floor-step capture, PR #68** (branch `r10-floor-capture`, off
  `3afb73b`; workflow-only; the user's ruling 1 of 2026-10-25).
  - **The delta** (`b838023`; `ci.yml`, the rust job only; 31 lines added,
    none removed). Two steps bracket every floor step: named steps 8 and 36
    (UI 14 and 42), with the 27 between them (named 9–35), which print all
    33 floor lines and 3 of the 4 count gates.
    - Before: a `floor-pre` line, then a detached sampler writing a
      `floor-sample` line every 10 s.
    - After, `if: always()`: `floor-post` and every sample.
    - One bracket over the span covers each floor step that carries a
      network or timing assertion, with no list to drift as suites change.
    - The logging cannot fail the job (#49's guards: `|| true`, a detached
      sampler, `continue-on-error`).
  - **Falsified, #49's pattern.** Locally, both step bodies (extracted from
    the YAML) exit 0 under `bash -e` and `bash -eo pipefail`: normally, with
    `RUNNER_TEMP` unset, with it unwritable, and with the post step run
    without the pre step. On CI, throwaway draft PR #69 (`0fb9d5e`) forced
    the `prompt10_rtmp` step (named 20, UI 26) to exit 1 after 25 s: run
    `40262195352`, rust job `121376758059`, read job-exact.
    - `floor-pre` at 03:08:53Z; the forced failure at 03:09:35Z.
    - The `if: always()` step printed `floor-post` and all five samples,
      three of them written during the failing step.
    - The job failed on the forced step alone; its workspace step was
      549/0/4.
    - #69 was closed unmerged. Its branch was deleted locally by the builder
      and on `origin` by the user.
  - **CI at `b838023`:** run `40262177993`, attempt 1, every job read
    job-exact. rust 549/0/4, 33/33 floor lines and the 4 count gates; R10's
    drain tests `ok` in both runs of the suite; `three_frames…` `ok` in both
    steps; `floor-pre`, 10 samples and `floor-post` (3.44–4.72), the
    `prompt10_rtmp` step's tests ending between two samples. control-plane
    200/200, the refused start 2020 ms; dress 29/29, 12c `ok`, drops 0.

  **The two-key pass on PR #68** (independent; relayed by the user,
  2026-10-26) HOLDS on 1–9. Its four disputes were ruled, and the builder
  concurs:
  1. **The third envelope:** a floor-bracket envelope under the pinned rule,
     seeded from #68's floor window (F1: max 4.72 per CPU, ceiling 5.90;
     Finding R10). R10's workspace envelope is unchanged at 19.13. The
     builder's proposal (floor readings read against R10's ceiling, feeding
     no envelope) is struck.
  2. **The branch line corrected:** both branches were deleted by the user on
     2026-10-26 at about 03:45Z.
  3. **The step-numbering convention, pinned:** records count the YAML's
     named steps, and UI numbers appear in parentheses only (the register's
     standing conventions). It was applied §2c to #67's bookend and #68's
     record.

  **Merged: PR #68 is `0421933`** (the bookend), on the user's word.
  - **The merge:** a squash at 2026-10-26T03:57:03Z, matched to `b838023`,
    with the staged subject. Its one parent is `3afb73b`.
  - **The tree is identical to the reviewed head's:** `0421933^{tree}` =
    `b838023^{tree}` = `94ec649`.
  - **Main's CI:** run `40266562638` (push), attempt 1. Every job executed
    (03:57:09Z), and each was read job-exact.
    - **rust** (job `121390146285`): 549/0/4, 33/33 floor lines, the 4 count
      gates; R10's drain tests `ok` in both runs of the suite;
      `three_frames…` `ok` in both steps; the capture printed `floor-pre`
      (04:00:12Z), 7 samples and `floor-post` (04:01:13Z).
    - **control-plane** (job `121390146257`): 200/200, package-reconcile
      4/4; the refused start 2024 ms.
    - **dress** (job `121390146090`): 29/29, 12c `ok`; drops 0, underruns
      209, watchdog trips 0.
  - **#68's capture covers the floor steps from `0421933`** (R10's row).
  - **The three envelopes each gain their row:** R10 row 59 (9.68; 19.13
    holds), G15 (5.32; 7.46 holds), and F2 (5.32, moving the floor maximum:
    the ceiling 6.65).

  **The device-audio build PR** (branch `device-audio`, off `0421933`; the
  user's order of 2026-10-26; the device-audio design record, D1–D8 as ruled
  2026-10-21, and D5's amendment of 2026-10-23). SPEC v0.4.8 row 15, a
  candidate until the merge. The era's final piece.
  - **The commits:**
    - `5862f9c`: #67's and #68's records (above).
    - `5fd9f4b`: **the device sink** (D1, D2, D5, D6, D7; R27). A lock-free
      SPSC ring the cpal callback copies out of; presentation marks (the
      ring frame a callback starts on, and when it is heard: arrival,
      cpal's offset, and the stream latency cpal omits); the `Feeder`,
      which anchors the cursor at the master sample due when a block is
      heard, slips a block one sample short or long past 0.5 ms (4-point
      cubic, end to end), and re-anchors past 4 ms; the rate guard; the
      HAL helper (`nbe-decode`'s `hal`, the change's one `unsafe`); the
      tick's `audioDevice` and `audioLimiterCeiling`; the lane's
      `audio.device` and `audio.underrun`; `NBE_AUDIO_SINK`, every harness
      null by default; SIGTERM/SIGINT close the sink with a device open.
    - `ee311cc`: **D4** (U2, U3): every audible element on its declared bus
      at its `gainDb`, loops included; a take returns music and sfx to the
      operator's level. **Found by reading and fixed:** the soundboard
      voices' own pass advanced the sfx bus's ramps a second time on every
      frame a voice sounded, so an sfx fade ran in half its time.
    - `6e164b0`: **D3**: the 44.1 kHz conversion, held to a measured bound.
    - `a5b19b1`: the hardware cases; **the limiter's lookahead compensated**
      (row 14's owed item: the driver renders 245 frames ahead, so program
      out, the record and the stream carry no 5.1 ms delay; found while
      writing the loopback case, since the device skew would have read 0
      with the audio 5.1 ms late); pre-start silence not an underrun (16 at
      one open, on the normative machine); the test seams; CI's surface.
    - `d974455`: SPEC row 15; the design record's as-built notes; the
      presented sum pinned (M-latency); CI's floor at 20; the dress floor
      restored (the builder's slip, below).
    - this record.
  - **Departures from the rulings' wording, put to the user:**
    1. **D1's close points.** `show.unload` has no engine route (row 7's
       stated gap), so the sink closes on drop and on SIGTERM/SIGINT with a
       device open (exit 143, 130).
    2. **D3's pin.** `AVSampleRateConverterAudioQualityKey` is unsupported by
       `AVAssetReaderTrackOutput` (it raised an exception that aborted the
       process); the algorithm key pinned to Mastering measured -100.9 dB,
       the same as the default. No pin is kept; the guard is the measured
       bound (under -95 dB, at 1 kHz ± 2 Hz and 48 000 ± 100 frames).
    3. **M-latency's guard.** The engine's own skew cannot see the stream
       latency's absence (every mark would carry it alike), and VB-Cable's
       stream latency is 0, so the presented sum is pinned by a unit test.
  - **Proposals for ruling:** the 0.5 ms slip threshold and the 4 ms
    realign; the 2-block ring target (the take head's ~70 ms, stated in
    §8.11); `audio.underrun` on 3 rising reports, cleared by 10 quiet ones,
    and `audio.device` a warning when absent and critical when lost; the
    slip-residual bounds (at the copy-out, 72 slips at +100 ppm: the
    fixed-frequency residual under -25 dB, measured -28.4, the 1/1600
    re-timing itself; the interpolation's own under -80 dB, measured -91.7;
    the floor -153.6; joins within the tone's own step; through the
    loopback, the median window -153.7 dB and every impaired window a
    slip's, worst -46.4 dB).
  - **CI's surface:** `NBE_AUDIO_SINK=null` for every job; the
    `device_audio` floor (20/20), inside the floor bracket, so **the rust job
    prints 34 floor lines** and the bracket spans named steps 8 and 37 (28
    between); an observational step lists the runner's audio devices.
  - **The builder's slips:**
    - In `a5b19b1`, a sed raising the `device_audio` floor 18 -> 19 also
      raised the dress's `exercised` floor 18 -> 19, and the dress
      exercises exactly 18. It was caught before push, by diffing every
      threshold against main, and restored in `d974455`.
    - The first loopback-residual run counted 559 short callbacks, all
      after its drive: the test left the device open, with nobody pushing,
      while it analysed. Named and fixed: the device closes first.
  - **The 30-minute proving run** (D2's proving case; `proving/run30.*`): the
    release test binary at `a5b19b1`, 06:08:11–06:38:43Z, started under 2.6
    (2.50 -> 1.69 at the ends; 31 samples 1.30–3.04, one at 3.04 at
    06:19:11Z whose cause the load-only sampler did not capture), on AC
    (charged, 100%), no sleep or battery event in `pmset -g log`. `d974455`
    changes the device path only by extracting the presented sum, the same
    arithmetic.
    - **Engine-internal, the speakers** (switched 44 100 -> 48 000 and
      restored): presented skew **-0.454 ms at 1 min, -0.478 at 10, -0.483
      at 30**; max |skew| after the first 2 s 0.551 ms. 1059 duplicates, 0
      drops, 1 realign. The skew rides the -0.5 ms threshold, as the slip
      holds it. **1059 slips in ~1800 s is +12.25 ppm**: the speakers' 48 kHz
      family measured for the first time (design record §5's unknown; 44.1
      kHz was +12.76 ppm).
    - **End to end, the VB-Cable loopback**: 180 bursts, each onset
      **+31.11..+31.98 ms** after its master time (+31.20 at 1 min, +31.48 at
      10, +31.60 at 30): a 0.87 ms spread across 30 minutes. VB-Cable's own
      output-to-input path, measured by a raw cpal burst in the tone case, is
      +31.85..+31.89 ms, so the engine lands within about -0.7..+0.1 ms of its
      master time beyond the path.
  - **The gate at `d974455`** (`gate/plan.py`, `hygiene/fx.py`; local
    2026-10-25 23:56 – 2026-10-26 00:07; every window started under 2.6):
    - **the workspace 580/0/5, kept** (23:59:58–00:02:00, 2.23 -> 2.62), after
      one void (2.24 -> 3.54: the top processes another session's opencode and
      Python);
    - **the device cases 5 + 1 ignored, kept** (00:02:15–00:02:48);
    - **`npm test` 205/205, kept** (00:02:48–00:02:59);
    - **the e2e suites 28 + 1 skip, kept** (00:02:59–00:04:43; the skip is
      operator-signals' refused start, which this machine's encoder
      acknowledges);
    - **the dress 29/29, kept** (00:06:20–00:07:27, 2.34 -> 1.85), after one
      void (1.87 -> 3.13: coreaudiod 22.7 %, MediaRemote, sysmond; the
      engine's own log reads `state=null` in that run, so not this engine's
      device). drops 0, underruns 0, watchdog trips 0.
    - **End to end over the real protocol**, from the dress's ticks: all 62
      read `audioDevice` `null` ("NBE_AUDIO_SINK=null"); the ceiling read
      `default` for 5 ticks, then `show` (-1.5 dBTP) from `show.load` on; no
      audio alert raised.
  - **The battery** (`battery/plan.py`, driven by `hygiene/fx.py` at
    `d974455`: each mutation in place, restored from `git show HEAD:`, the
    tree git-clean after; every window kept, 23:42–23:53 local 2026-10-25, load
    1.99–2.60). **23 mutants, 23 killed, each by its named guard and for its
    own reason; 3 controls green:**

    | Mutant | What it does | Killed by | Its signature |
    |---|---|---|---|
    | M-ring | the callback allocates (graph work in the copy-out) | `the_callback_copy_out_allocates_nothing` | 4 allocations |
    | M-slip-sign | the slip's sign reversed | the 30-minute simulation | +12.76 ppm: 4.009 ms |
    | M-slip-off | no slip | the 30-minute simulation | +12.76 ppm: 4.001 ms |
    | M-latency | the stream latency left out of the presented instant | `the_presented_instant_includes_the_stream_latency` | 12 671 000 for 18 400 166 |
    | M-rate | the prior rate not restored at close | `the_speakers_rate_is_switched_and_restored` (hw) | 48 000 after close (the test then restored 44 100) |
    | M-xrun | the device's overload not counted | `a_forced_stall_is_one_counted_underrun_episode` (hw) | no overload counted |
    | M-empty | an empty ring plays stale samples | `a_short_callback_plays_silence_and_counts_one_underrun` | the stale 7.0s |
    | M-gain | `gainDb` ignored | `a_take_plays_every_element_on_its_bus_at_its_gain_db` | music at -12.04, not -24 |
    | M-clip | the limiter replaced by a clamp | `a_clip_and_a_loop_are_both_heard_and_their_sum_is_limited_not_clipped` | 1.000 past the 0.841 ceiling |
    | M-fallback | an unknown sink panics | `a_sink_that_cannot_open_is_silence_with_its_reason` | the panic |
    | M-lookahead | the limiter's lookahead not compensated | `the_driver_compensates_the_limiters_lookahead` | 3445 for 3200 |
    | M-prestart | pre-start silence counted | `silence_before_the_first_block_is_not_an_underrun` | 1 for 0 |
    | M-sfx-twice | the old double ramp advance | `an_sfx_fade_runs_its_whole_ramp_while_a_voice_sounds` | 0.1279 for 0.3141 |
    | M-drift-R27 | the tick's drift not the device's | `the_tick_drift_is_the_devices_presented_skew` | 0.000 then 0.000 |
    | M-underrun-uncounted | device events not counted | `device_short_callbacks_and_overloads_are_underruns` | 0 for 3 |
    | M-lost-ignored | loss ignored | `a_lost_device_falls_back_to_silence_and_says_so` | the device kept |
    | M-ceiling-unpublished | the ceiling not on the tick | `the_limiter_ceiling_in_force_reaches_the_tick` | (-1.5, default) |
    | M-cp-null-alerts | a configured null sink alerts | `alerts.test.ts` #37 | 37/39 |
    | M-cp-sustain-1 | one rising report raises | `alerts.test.ts` #38 | 38/39 |
    | M-loop-silent | a videoLoop's audio dropped | `a_videoloop_plays_its_audio_looped` | the loop missing |
    | M-bus-ignored | every element on clip | `each_element_plays_on_the_bus_it_declares_whatever_its_z` | guest read as clip |
    | M-operator-reset | a take resets music to 0 dB | `a_take_returns_an_element_bus_to_the_operators_level` | -12.04 for -22.04 |
    | M-unconverted | the reader asked for 44.1 kHz | `a_44k1_source_converts_to_48k_at_its_own_pitch_and_length` | 44 100 frames |

    Controls: `device_audio` 20/20, `device_audio_hw` 5 + 1 ignored,
    `alerts.test.ts` 39/39. The speakers read 44 100 Hz after the battery.

  - **CI at `52371e8`:** run `40284114106`, attempt 1. Every job executed
    (07:10:04Z), and each was read job-exact (`gh api
    --allow-escape-sequences …/actions/jobs/<id>/logs`).
    - **rust** (job `121443674416`): 580/0/5; 34/34 floor lines
      (`device_audio` 20/20) and the 4 count gates; R10's drain tests `ok`
      in both runs of the suite; `three_frames…` `ok` in both steps. The
      simulations reproduce the local figures exactly (DRIFT-SIM ±12.76 and
      ±100 ppm, max 0.501 and 0.511 ms; SLIP-RESIDUAL 72 slips, -28.4 /
      -91.7 / -153.6 dB). The device cases print `SKIP normative:`.
    - **The design record's §5 unknown, answered:** a `macos-14` runner has
      output devices, "Apple Virtual Sound Device" (the default, 48 kHz;
      HAL latency 0, safety offset 100, stream 0) and "Null Audio Device"
      (44.1 kHz). D7's explicit null sink is what keeps CI's audio
      deterministic.
    - **control-plane** (job `121443674092`): 205/205, package-reconcile
      4/4; the refused start 2020 ms.
    - **dress** (job `121443674446`): 29/29, 12c `ok`; drops 0, underruns
      176 (the null sink's cadence), watchdog trips 0.
    - The envelope rows: R10's 60, G16 and F3 (Finding R10's tables).

  **The two-key pass on PR #70** (independent; relayed by the user,
  2026-10-26) HOLDS on 1–12. Its four disputes were ruled. Its two voids
  stand as declared: the long proving runs (the 30-minute run and the
  hardware cases were not re-run by the second key) and the D3 premise's
  verification. The builder's record:
  1. **Row 15's "(19" is 20** (§2c, struck in the row): the builder's slip.
     `d974455` added the 20th test (`the_presented_instant_includes_the_stream_latency`,
     M-latency's pin) and wrote row 15 from `a5b19b1`'s count of 19.
  2. **The D3 premise's status line** (§2c, added at the ruling in
     `docs/device-audio-design.md`): the premise did not hold. The SDK
     header says "AVSampleRateConverterAudioQualityKey is not supported"
     (`AVAssetReaderOutput.h`, SDK 26.2, `:356`, `:377`; `:225` and `:260`
     in other words); set anyway, it aborted. The verification's void
     stands: the evidence is the header and the builder's run.
  3. **The pass's own sfx slip** (§2c, the pass's text as relayed): the
     misplaced first mutant, after the output pass, survived; the
     corrected one, a second advance inside the bus loop, was killed,
     0.1279 at frame 714. The guard is sound; the first probe was not.
  4. **F3 counts** (8.01, its cause named: `nbe-preflight` compiled cold in
     the bracket after the `Cargo.lock` change), and the floor ceiling
     moves 6.65 → 10.02.
  5. **R27's strike, disputed on the fact:** the relay read it as landed;
     at `52371e8` R27's header was still open, and every FIXED strike is
     made in the PR after the fix. The bookend's order struck it by the
     merge SHA, here.

  **Merged: PR #70 is `0132d07`** (the bookend), on the user's word.
  - **The merge:** a squash at 2026-10-26T08:38:43Z, matched to `52371e8`,
    with the staged subject. Its one parent is `0421933`.
  - **The tree is identical to the reviewed head's:** `0132d07^{tree}` =
    `52371e8^{tree}` = `9bcedaa`.
  - **Main's CI:** run `40293707305` (push), attempt 1. Every job executed
    (08:38:49Z and 08:39:22Z), and each was read job-exact. **Red on
    control-plane alone.**
    - **rust** (job `121474548526`): 580/0/5 (the step's `TOTAL` line),
      34/34 floor lines, the 4 count gates, ~~9~~ 8 `SKIP normative:` lines
      *(§2c, the user's ruling of 2026-10-26 on #71's bookend: the 9th was
      the `prompt11_watchdog` floor's own `grep -vc '^SKIP normative:'`
      line; the builder's slip)*; R10's
      drain tests `ok` in both runs of the suite; `three_frames…` `ok` in
      both steps.
    - **control-plane** (job `121474548893`): red at named step 15 (UI 23),
      package-reconcile 3 of 4: **R32's sighting 2** (its row). `npm test`
      205/205 and every step before it were green. Named steps 16–18 (UI
      24–26: show.stop, resync-request and operator-signals end to end) did
      not run, so the run holds no evidence for them.
    - **dress** (job `121474548971`): 29/29, 12c `ok`; drops 0, underruns
      176, watchdog trips 0.
  - **The cache, as predicted:** all three jobs restored rust-cache from a
    restore key ("full match: false"), main's cache keyed on the
    `Cargo.lock` before #70.
  - **Row 15 is ratified** (merged as `0132d07`); **R27 is FIXED by
    `0132d07`**; **U2, U3 and U4 leave the unbuilt-MUST register**; the
    design record's head records the merge.
  - **The envelopes** (Finding R10's tables): R10 rows 60 and 61 (19.13
    holds), G16 and G17 (7.46 holds), F3 and F4 (the floor ceiling 10.02).
    **R15's watch** notes both runs (R15's row).

  **The user's rulings of 2026-10-26, on the bookend:**
  1. **R32 escalates: hunt OPEN** (its row): (a) the control-plane job's
     `[ci-load]` bracket, a small workflow PR with its own envelope and its
     own two-key pass; (b) a local reproducer, test 4's scenario with an
     induced reload, N runs under inflated load, a reproduction falsified
     normally and about 50 clean runs demoting R32 to watch; (c) the reload
     path logs when audio sources build. Sighting 2 stands recorded with
     its defenses.
  2. **The bookend's envelope rows are accepted as read**, F4's cause note
     included: the cache missed, and the peak was `automation_rules`' end
     of window. The row says what the log says, not what was expected.
  3. **The sfx-slip slot** carries the pass's text (the two-key pass, 3).
  4. **R31's probe: go.** 30 minutes, 12c's record path, the recording
     kept, swap watched; ahead of or beside the R32 bracket PR, in the
     builder's sequencing.

  The queue: R32's bracket PR, then R31's probe, then the era's ledger
  closes. AC-5 soaks last, per the era's order.

  **R31's 12c probe, its sighting, the hunt, and the fix** (branch
  `r31-zero-copy-fence`, off `0132d07`; the user's specification of
  2026-10-26 for the probe, its go of the same day, and the rulings of
  2026-10-26 on the hunt). Every run is kept beside
  `.git/nbe-staged/r31/sighting2.md`, the hunt's full record.
  - **The probe** (`packages/control-plane/src/r31-12c.probe.ts`, `npm run
    probe:r31`, not in `npm test`): step 12c's record path for 30 minutes,
    the real control plane and the dress's debug engine, counter_150 taken
    and an empty scene between takes (the separator), one recording; its
    evidence kept under `.git/nbe-staged/r31/`, never a temp directory. Read
    back with step 12c's classifier (moved, unchanged, to `r20-counter.ts`),
    and joined through the engine's record trace (`NBE_RECORD_TRACE`) to the
    master frame each recorded frame was drawn at: clause 1 (never
    backwards) and clause 2, exact (each picture the one its tick drew);
    R33's repeats and skips counted beside them.
  - **Sighting 2** (the probe's first run, 10:43–11:14Z, zero-copy, the AMD
    adapter, load 4.3–6.5, power-limited): 9 frames backwards in 277 takes,
    every one S1's shape (k − 1, k, **k − 1**, k + 2), clustered in takes
    181–186; 1943 held frames; the engine counted nothing.
  - **The hunt, in eight steps** (`sighting2.md`):

    | Step | What ran | What it showed |
    |---|---|---|
    | 1 | CPU readback, the same state, 30 min | 0 backwards, 124 held |
    | 2 | a GPU completion wait before the handoff, untraced, 30 min | 0 backwards, 1446 held (§2c: first read as the prediction failing; the totals mixed R33's holds with this class) |
    | 3 | a trace joining each recorded frame to its tick's master frame | the held frames split: R33's cadence (the same master frame twice) and this class (a picture older than its tick drew) |
    | 4 | zero-copy, AMD against Intel, traced, 4 × 10 min interleaved | the class on both (1, 13, 7, 13); the coherency hypothesis falsified; on Intel at clip frame 88 |
    | 5 | CPU readback, both GPUs, traced, 4 × 10 min | 0 in 40 minutes |
    | 6 | Intel, zero-copy, with and without the wait, interleaved | **0 and 0 with it, 20 and 28 without** |
    | 7 | the fix, falsified as ruled | below |
    | 8 | the battery | below |

  - **The mechanism:** the View drew into a pool surface, the draw was
    submitted, and the surface went to VideoToolbox at once; the encoder
    could read it before the GPU drew into it, and recorded the picture the
    surface held before.
  - **The second defect, filed as R33** (its row): the render loop can draw
    the previous master frame again and skip the next; both GPUs, both
    record paths; uncounted; on air.

  **The user's rulings of 2026-10-26, on R31's hunt:**
  1. **R31's fix is shape (a):** the recording thread waits on the GPU
     work's ID before encoding; the View never waits; a late GPU delays the
     encode only. Falsification: the traced probe, both GPUs, 0 stale frames
     in 30 minutes on the normative adapter, the class returning with the
     wait removed. The probe lands in the fix PR as the falsification tool,
     then stays as a guard.
  2. **The render-loop timing defect is FILED as R33** (its row carries the
     measured rate and the falsification plan).
  3. **R31's clause 2 is moot per the fix;** the remaining holds are R33's.
  4. **CP1 on #71: the 60-second settle**, not the build-tail count.
  5. **The experiment branch** (`r31-hunt-gpu-wait`) **stays local and dies
     there;** the trace runs and their joins are kept beside `sighting2.md`.

  Sequencing: R31's fix PR (with the probe) next; R32's reproducer and reload
  log follow; #71's two-key pass dispatch stands ready whenever its settle
  lands.

  **R31's fix PR, the build record.**
  - **The fix** (`c15c4de`, `af83c3c`):
    - render: `render_bus` returns its submission; the View's is kept per
      frame (`view_submission`), cleared at the start of every frame.
    - the tap: after the draw the tick marks the loan
      (`TapLoan::mark_drawn`) with that submission and the master frame; a
      zero-copy surface with no draw to wait on (a failed View, an unmarked
      loan) is never handed off: a counted skip.
    - the record thread: `encode_after_draw` waits on the exact submission
      (`DrawDone`) before `encode_pixel_buffer`, at most `DRAW_WAIT` (1 s);
      an unfinished draw is a counted skip, never an encode.
    - **the wait never blocks inside wgpu** (`af83c3c`): the first form's
      blocking `poll(Wait)` held wgpu-core's snatch lock for reading through
      the GPU wait, which the render thread's per-frame `poll(Poll)`
      (`gpu.rs` `reclaim`) can need for writing; now a zero-timeout
      `poll(Wait)` every `DRAW_POLL` (500 µs).
    - measurement knobs, off by default: `NBE_RECORD_TRACE` (the record
      trace) and wgpu's own `WGPU_POWER_PREF` (the adapter; the default stays
      HighPerformance).
  - **The probe** (`f9fa430`, `485aaa3`, `3e949a7`, `635601b`, `1226a94`):
    built, then `--tap cpu`, the run's `NBE_*`/`WGPU_*` in its meta (never a
    credential-like name: the security review's finding, fixed), and the
    trace's exact clause 2 with R33 counted and `--gpu low`.
  - **Guards:** `record::feed` 2 unit tests (the wait precedes the encode,
    bounded by `DRAW_WAIT`; an unfinished draw is never encoded);
    `zerocopy_tap` +3 (a drawn surface reaches the record thread with its
    draw and its frame; a View that did not draw, after one that did, hands
    on nothing; an unmarked surface is never handed off), its CI floors
    12/4 → 15/7; the GPU tests that drive the tap themselves mark their
    loans as the tick does.
  - **Falsified as ruled** (`.git/nbe-staged/r31/fix/`, every recorded frame
    traced, the draw wait never timed out):

    | Run | commit | adapter | minutes | stale | View drops | R33 repeats / skips |
    |---|---|---|---|---|---|---|
    | amd-fix | `1226a94` (blocking wait) | AMD | 30 | **0** | 0 | 491 / 692 |
    | intel-fix | `1226a94` | Intel | 10 | **0** | 1 | 354 / 422 |
    | intel-nowait (the wait removed) | `1226a94` mutated | Intel | 10 | **34** (one two back: S1's shape) | 0 | 331 / 399 |
    | amd-fix2 | `af83c3c` (non-blocking) | AMD | 30 | **0** | 0 | 643 / 852 |
    | intel-fix2 | `af83c3c` | Intel | 10 | **0** | 0 | 258 / 325 |

    The counter-proof's mutant bypasses the wait call, so it stands for
    `af83c3c`.
  - **The battery** (`battery/plan.py`, `hygiene/fx.py`, at `af83c3c`): 7
    mutants, 7 killed, each by its named guard (M-no-wait, M-timeout-encodes,
    M-zero-timeout by the unit tests; M-never-marked, M-stale-submission,
    M-unfenced-handed-off by `zerocopy_tap`; M-no-mark-in-tick by the dress:
    27/29, steps 11 and 12b); 2 controls green; the engine rebuilt from HEAD.
  - **The gate:** two runs, every window started under 2.6 and was kept with both ends
    under 3.0 (`gate/plan.py`, `hygiene/fx.py`; voids named).
    - **At `af83c3c`** (`gate/run/`): the workspace window read **315
      passed, 1 failed**: `prompt09_feed::loop_wiring_pre_check_before_readback`,
      a source-scan guard looking for `handoff_record_frame` in
      `end_tap_frame`; the fix routes that handoff through
      `send_record_frame`. The ordering it guards is unchanged. **The
      builder's slip:** the touched suites were run before committing, the
      whole workspace was not. Fixed in `89d272a` (§2c). `npm test` 205/205,
      e2e 28 + 1 skip, the dress 29/29 (step 12c: 149 counter frames, 1–149,
      through the zero-copy tap).
    - **At `89d272a`** (`gate/run2/`): **the workspace 585/0/5** (580 + the 2
      unit tests + the 3 GPU tests); **`npm test` 205/205**; **e2e 28 + 1
      skip** (the refused start: this machine's encoder acknowledges); **the
      dress 26/29, a sighting, named and not rerun**
      (`gate/run2/dress-sfx-sighting/README.md`): step 6's soundboard stab
      never reached the sfx bus within 10 s, and its two consequences. Not
      this PR's (the engine byte-identical to `af83c3c`'s 29/29); not R13 (a
      late rise that always came); #70 changed that path. Put to the user to
      file.

  **R32's control-plane capture, PR #71** (branch `r32-cp-capture`, off
  `0132d07`; the user's ruling of 2026-10-26, (a); workflow-only, with #70's
  records in its first commit, `f003fa2`).
  - **The bracket** (`9965d1e`): named steps 7 and 20 of the control-plane
    job, the 12 test steps between them (`npm test` through the
    operator-signals end to end); `cp-pre`, a `cp-sample` every 10 s,
    `cp-post` and every sample, printed even when a step fails; every
    command `|| true`, the sampler detached, both steps continue-on-error.
    The engine's release build (named step 6) moved ahead of the bracket.
  - **The settle** (`ba68033`; the user's ruling of 2026-10-26 on CP1): the
    pre step waits 60 s before `cp-pre`, so the build's load-average tail
    stays out of the window's readings.
  - **Falsified:** locally, both step bodies exit 0 under `bash -e` and
    `bash -eo pipefail` (normally, `RUNNER_TEMP` unset, unwritable, the post
    step without the pre), at `9965d1e` and again at `ba68033` (14 of 14
    exits 0, the sampler stopped). On CI, throwaway draft PR #72
    (`dc38d76`, run `40299155992`) forced package-reconcile (named step 16)
    to exit 1: the job failed on that step alone, and the post step printed
    `cp-post` and all 8 samples after the failure. #72 was closed unmerged;
    its branch, `r32-cp-falsify-ci-temp`, is the user's to delete.
  - **CI at `9965d1e`:** run `40299126313`, attempt 1, every job executed,
    each read job-exact: rust 580/0/5, 34/34 floor lines, the 4 count gates;
    control-plane every step green, `npm test` 205/205, package-reconcile
    4/4; dress 29/29, 0 drops. Rows: R10's 62, G18, F5. Its `cp-*` window
    (4.49–10.59, the build's tail) seeds nothing, by the ruling.
  - **CI at `ba68033`:** run `40351769240`, attempt 1, every job executed,
    each read job-exact: rust 580/0/5, 34/34 floor lines, the 4 count gates,
    R10's drain tests `ok` in both runs of their suite, `three_frames…` `ok`
    in both steps; control-plane every step green, `npm test` 205/205,
    package-reconcile 4/4, the refused start `ok`; `sleep 60`
    15:42:56–15:43:56Z, then `cp-pre`; dress 29/29, 0 drops. Rows: R10's 63,
    G19, F6, and CP1, the fourth envelope's seed.

  **The two-key pass on PR #71** (independent; relayed by the user,
  2026-10-26) HOLDS on 1–9. Its three disputes were ruled. The builder's
  record:
  1. **The prompt04-comment claim is not in the diff** (upheld).
     `0132d07..ba68033` touches no `prompt04` step and no `prompt04`
     comment; its only `prompt04` text is in envelope rows and R15's row.
     Its origin, searched: no #71 artifact the builder wrote carries it
     (the PR body and its three edits, the three commit messages, the diff,
     the staged records, the builder's reports). The only prompt04-comment
     claim the builder has written is `e7600aa`'s message (#67), and that
     diff carries it (`ci.yml:152–154`).
  2. **CP1's row and the 6.18 ceiling** were in the PR body only at the
     reviewed head; they landed at the bookend (Finding R10's control-plane
     table), with the settle's evidence: `cp-pre` read 10.59 per CPU at
     `9965d1e` (load1 31.77 at 09:34:48Z, 0.26 s after the engine build
     finished) and 4.52 at `ba68033` (load1 13.56 at 15:43:56Z, after the
     60 s settle). The throwaway #72 (`dc38d76`), also before the settle,
     read 10.22 at `cp-pre` (load1 30.66 at 09:30:07Z). *(§2c: the ruling's
     relay gave "10.22 → 4.52" for `9965d1e` → `ba68033`; 10.22 is #72's
     `cp-pre`, and `9965d1e`'s is 10.59, both in the kept logs.)*
  3. **The throwaway's branch**, `r32-cp-falsify-ci-temp`, is the user's to
     delete.

  **Merged: PR #71 is `ab0304a`** (the bookend), on the user's word.
  - **The merge:** a squash at 2026-10-27T03:01:22Z, matched to `ba68033`,
    with the staged subject and body (byte-identical). Its one parent is
    `0132d07`.
  - **The tree is identical to the reviewed head's:** `ab0304a^{tree}` =
    `ba68033^{tree}` = `b2a78b6`.
  - **Main's CI:** run `40441808488` (push), attempt 1. Every job executed
    (each started 03:01:27Z), and each was read job-exact (`gh api
    --allow-escape-sequences …/actions/jobs/<id>/logs`). **Green.**
    - **rust** (job `121983656691`): 580/0/5 (the step's `TOTAL` line),
      34/34 floor lines, the 4 count gates, 8 `SKIP normative:` lines; R10's
      drain tests `ok` in both runs of the suite; `three_frames…` `ok` in
      both steps.
    - **control-plane** (job `121983656950`): every step green, `npm test`
      205/205, package-reconcile 4/4, the refused start `ok` (2022 ms); the
      bracket live on main: `sleep 60`, `cp-pre`, 20 samples, `cp-post`.
    - **dress** (job `121983657034`): 29/29, 12c `ok`; drops 0, underruns
      217, watchdog trips 0.
  - **The cache:** the control-plane job restored from a restore key ("full
    match: false"): main's `0132d07` control-plane job failed (R32's
    sighting 2) and saved nothing, so both release builds compiled for
    about 2m 20s.
  - **The envelopes** (Finding R10's tables): R10 row 64 (19.13 holds), G20
    (7.46 holds), F7 (10.02 holds), and CP2, above CP1's ceiling (the
    build's tail outlasted the settle), counted by the ruling below. **R10's
    and R15's watches** note the run (their rows).

  **The user's rulings of 2026-10-26, on #71's bookend:**
  1. **#71's bookend confirmed:** the merge is byte-identical,
     single-parent, tree-correct; all holds.
  2. **CP2's 6.77 > 6.18: COUNTED** per the pinned rule, on F3's precedent.
     The reading lands on the row with its cause (the cold engine build, 2m
     23s, outlasted the settle after `0132d07`'s cache save was lost), and
     the ceiling moves to 8.46, carried as derived from a cold-build CP2.
     The next normal reading shows whether 8.46 was weather; if it was, the
     lane corrects downward per the standing rule. Logged as a row, not
     gated.
  3. **The `SKIP normative:` 9 → 8 §2c strike lands.** The miscount was the
     floor script's own grep-pattern line in the output; the count gate is
     unaffected (it counts tests, not these lines). The reading template
     gains a line (`docs/implementation-standards.md` §2b): when counting a
     pattern's occurrences in a log, exclude the floor script's own echo of
     it. This map's #70 bookend line gets its strike note.

  The blank line the rebase of #73 lost before R31's block (its first
  paragraph ran on from #71's queue paragraph) is restored here, as ruled.

  **#73's rebase** (onto `ab0304a`, #71's merge): commits 1–8 replayed
  patch-identical (`git patch-id --stable`; `git range-diff` marks each
  `=`), and the branch's non-docs change is identical; the records commit
  re-anchored after #71's records, `docs/prompt-map-07-13.md` the only
  conflict (`soak-protocol.md` merged clean), its added and removed lines
  the same bar one blank line (restored above). Old → new: `f9fa430` →
  `d7ce311`, `485aaa3` → `ea8955b`, `3e949a7` → `b272d32`, `635601b` →
  `d16c1a7`, `c15c4de` → `1fa7cf9`, `1226a94` → `f6eb251`, `af83c3c` →
  `f331e9c`, `89d272a` → `4f6731f`; the records `5a65e59` → `7c215c7`.
  *(§2c: R31's falsification, battery and gate, above, ran at the
  pre-rebase SHAs; commits 1–8 replayed patch-identical; CI at `7c215c7` is
  the fresh witness.)*
  - **CI at `5a65e59`:** run `40374926189`, attempt 1, every job executed,
    each read job-exact: rust 585/0/5, 34/34 floor lines (`zerocopy_tap`
    ran 15, exercised 14), the 4 count gates; control-plane every step
    green, `npm test` 205/205, package-reconcile 4/4; dress 29/29, 0 drops,
    step 6's sfx rise 2968 ms. Rows: R10's 65, G21, F8 (no `cp-*` bracket:
    that head predated #71).
  - **CI at `7c215c7`** (the fresh witness): run `40442304083`, attempt 1,
    every job executed, each read job-exact: rust 585/0/5, 34/34 floor
    lines (`zerocopy_tap` 15/14), `record::feed`'s 2 unit tests and the 3
    new `zerocopy_tap` tests `ok`, the 4 count gates, R10's drain tests `ok`
    in both runs of their suite, `three_frames…` `ok` in both steps;
    control-plane every step green, `npm test` 205/205, package-reconcile
    4/4, the refused start `ok` (2027 ms); dress 29/29, 0 drops, step 6's
    sfx rise 2021 ms, step 4's clip rise 5012 ms (the latest logged since
    R13's gate, inside the 10 s backstop; R13 is closed and its rises are
    logged, not gated). Rows: R10's 66, G22, F9, CP3.

  **The two-key pass on PR #73** (independent; relayed by the user,
  2026-10-26) HOLDS at `7c215c7`. The builder concurs with each ruling.
  - **§2c, the dispatch's R35 claim** (the user's ruling of 2026-10-26, on
    #73's bookend): written by the user in the #73 two-key dispatch
    (2026-10-26), nowhere else: ~~"Claims: … R35 (stream's same-shaped
    handoff) same fix, own red/green, both GPUs."~~ The claim was the
    relay's; it converted a pending user ruling into a builder's claim.
  - **The oversight note** (the same ruling): "The #73 two-key dispatch
    described R35 and R34 as claims to test when both were open questions
    awaiting the user's ruling. The pass's disputes 1 and 3 correctly
    identified the mismatch. The rulings of 2026-10-26 resolved both by
    filing."

  **The user's rulings of 2026-10-26, on #73's two-key pass:**
  1. **R35** (the stream's zero-copy handoff, R31's shape): FILED, its own
     PR after #73, the same fix mechanics (a recording-side wait on the GPU
     work ID), its own red/green, both GPUs. #73 merges without it; the
     body's open question is resolved by the filing.
  2. **The dress sfx sighting is FILED as R34:** "the soundboard stab never
     reached the sfx bus". Its row carries the question of why no alert
     fired (the lane doesn't read sfx state) and the candidate line on
     `sfx.overload` in the lane. Falsification plan: the dress's sfx step
     asserts an audible onset, or its absence is explained.
  3. **The body's falsification table keeps the old → new SHA map** with
     the patch-id proof; one §2c note added (above); CI at `7c215c7` is the
     fresh witness.
  4. **The pass's voids stand** (its disk full, load 3.01): the interleave
     re-runs and the mutant spot-kills rest on the builder's figures as
     claims, CI as witness, recorded as such on R31's row.

  **Merged: PR #73 is `27368f2`** (the bookend), on the user's word.
  - **The merge:** a squash at 2026-10-27T04:10:06Z, matched to `7c215c7`,
    with the staged subject and body (byte-identical). Its one parent is
    `ab0304a`.
  - **The tree is identical to the reviewed head's:** `27368f2^{tree}` =
    `7c215c7^{tree}` = `ca14134`.
  - **Main's CI:** run `40448236533` (push), attempt 1. Every job executed
    (each started 04:10:14–04:10:15Z), and each was read job-exact. **Red
    on dress alone.**
    - **rust** (job `122004538883`): 585/0/5 (the step's `TOTAL` line),
      34/34 floor lines (`zerocopy_tap` 15/14), the 4 count gates, 8 `SKIP
      normative:` lines; R10's drain tests `ok` in both runs of the suite;
      `three_frames…` `ok` in both steps.
    - **control-plane** (job `122004538970`): every step green, `npm test`
      205/205, package-reconcile 4/4, the refused start `ok` (2022 ms);
      warm builds (full cache matches), the bracket's window CP4.
    - **dress** (job `122004538655`): **28/29, R21's sighting 2** (its
      row): `not ok 6 - [RI-1] step 5: a 15-frame mix drops no frames`, "a
      mix must not drop frames: 0 -> 1". The run's artifact was downloaded
      before any rerun (`.git/nbe-staged/pr73-bookend/artifact/`). Not
      rerun. Every other step `ok`; step 4's rise 1997 ms, step 6's 2981 ms.
  - **R31 is FIXED by `27368f2`** (its row), the probe the permanent guard;
    **R33's row landed** with the merge; **R34 and R35 are filed** (their
    rows, with these records).
  - **The envelopes** (Finding R10's tables): R10 row 67 (19.13 holds), G23
    (7.46 holds), F10 (10.02 holds), CP4 (8.46 holds). **R15's watch**
    notes the run (its row).

  **The user's rulings of 2026-10-26, on #73's bookend:**
  1. **R31's record completion:** the §2c strike of the dispatch's false R35
     claim and the pass-dispatch-oversight note land here (#73's two-key
     pass, above), with R34's and R35's rows, the voids note, the blank
     line, and both bookends.
  2. **R21's sighting 2:** the row updates as drafted; OPEN struck, LOGGED
     kept; the dress job's missing load capture is the standing question
     (the same blind admission as R32's sighting 2, twice now). The
     artifact in `.git/nbe-staged/pr73-bookend/artifact/` is kept. The
     dress step-5 CI backstop moves from R21's row to the infrastructure
     list (`docs/soak-protocol.md` §5). ~~The ruling also stated a
     cold-runner correlation, with a cold-cache dress variant as the
     falsification plan.~~ *(§2c, ruling 4 below.)*
  3. **The ceiling stays 8.46** as the pinned number: no downward rule
     exists; recorded per the readings, CP3 and CP4 noted as
     weather-confirming. A proposed downward rule joins the queue (Finding
     R10's control-plane table).
  4. **The builder's dispute on ruling 2 is upheld** (the user's ruling of
     2026-10-26). The cold-runner correlation is struck §2c, the builder's
     cache table the refuting evidence: the dress job at `27368f2` was a
     full rust-cache match, its builds as warm as both green runs', on the
     same image; the cold build was CP2's, in the control-plane job, a
     different job and runner.

     | dress job | rust-cache | builds (dev / release / dev) | step 5 |
     |---|---|---|---|
     | main `27368f2` (R21's sighting 2) | full match | 20.16 / 8.38 / 5.17 s | 1 drop |
     | main `ab0304a` | full match | 24.27 / 9.76 / 6.48 s | `ok` |
     | #73 `7c215c7` | full match | 19.47 / 8.15 / 5.16 s | `ok` |

     All three on `macos-14-arm64` 20260831.0302.1. **R21's falsification
     plan:** the logged normative-run attempts stand; add a step-5 mix
     variant under induced load (the R32 reproducer's pattern), run once
     the dress job has load capture. The plan depends on the standing
     question by design, and the row says so until then; the standing
     question itself is unchanged.
  5. **The infrastructure list's row** (the user's rulings of 2026-10-26,
     the builder's check upheld): the item stays R21's, "Wanted by R21
     (step 5 is its home step; both sightings live there)". ~~And R32 (both
     sightings surfaced in dress artifacts by eye; a CI frame-drop backstop
     would have raised them at the time, artifacts attached).~~ *(§2c, the
     relay's error: the row meant was R31, misattributed; R32's sightings
     are control-plane package-reconcile failures, and its wants are
     already met, #71's bracket and #74's reload log.)* The builder's check
     on the corrected entry is upheld: R31's sighting was raised by the
     dress's own step-12c frame-order check ("1 went backwards"), not by
     eye; R21's both failed step 5's zero-drop assertion; the item is an
     R12-style backstop, which relaxes CI from zero to a bound while zero
     stays normative; and R31's class cannot run on a runner with no
     hardware encoder. So the item keeps its name (a frame-integrity rename
     was proposed and dropped), and **R31 is not named**: it has its
     permanent guard in the probe, and a CI check that requires an
     encoder-equipped runner is speculative infrastructure, not a
     row-backed want. If it ever earns a row, it earns the item then.

  **R32's hunt, PR #74** (branch `r32-hunt`, off `0132d07`, rebased onto
  `27368f2`; the user's ruling of 2026-10-26, (b) and (c); its first commit,
  `483fe40`, landed #71's and #73's records).
  - **(c) The reload path's log** (`9de5db8` → `23197b8`): at info under
    `nbe_engine::directive`, which test 4 captures: `show.resync received`;
    "the package identity differs from the load held; reloading", naming
    the path, the snapshot's generation and the one held; the load's
    elapsed ms; `audio sources built` with the audio phase's elapsed ms.
    Test 4's "the engine never applied its resync" names the version waited
    for and the last applied, and carries the engine's last 80 lines.
    Checked by forcing that failure once (a temporary mutation, restored).
  - **(b) The reproducer** (`f6d584c`, `eef690f`, `9dfd84b` → `f4e43fb`,
    `f2adf8a`, `ae709db`; `npm run repro:r32`, not in `npm test`): test 4's
    scenario, its race read from the audit, the reconnect's reload induced,
    timed from the reconnect's registration to the engine's apply against
    test 4's 20 s bound, N busy processes running, every run kept
    (`.git/nbe-staged/r32/repro-50x24/`). The normative machine (12 hardware
    threads, on AC): two voids at 48 busy processes (the engine starved at
    its start; the first preflight's 60 s floor), each fixed and recorded;
    then **50 runs at 24 busy processes, 0 past the bound**, every run
    reloading inside the preflight; the apply 3256–5097 ms (median 3870,
    the kept summary's printed figure, the upper of the two middle values;
    3869, re-derived as the middle pair's mean rounded half up, is #74's
    merge message's), its audio phase 44–209 ms; load1 16.6–48.9.
  - **The gate at `9dfd84b`** (load-gated, voids named): the workspace
    580/0/5, `npm test` 205/205 (three windows voided: lsd; the client; `top`
    and a cleaner's monitor), e2e 28 + 1 skip, the dress 29/29.
  - **The rebase** onto `27368f2`: the records commit `483fe40` first, then
    `9de5db8` → `23197b8`, `eef690f` → `f2adf8a`, `9dfd84b` → `ae709db`
    patch-identical (`git patch-id --stable`); `f6d584c` → `f4e43fb` differs
    in `packages/control-plane/package.json` alone (#73's `probe:r31` beside
    `repro:r32`, both kept). Pushed by the user over the intermediate head
    `527c065` (the records' first form, `d7bdf63`). *(§2c: the gate, the
    reproduction and CI at `9dfd84b` ran at the pre-rebase SHAs; CI at
    `ae709db` is the fresh witness.)*
  - **CI at `9dfd84b`:** run `40382622804`, attempt 1, every job executed,
    each read job-exact: rust 580/0/5, 34/34 floor lines, the 4 count gates;
    control-plane every step green, `npm test` 205/205, package-reconcile
    4/4; dress 29/29, 0 drops. Rows: R10's 68, G24, F11 (no `cp-*` bracket:
    that head predated #71).
  - **CI at `527c065`** (the intermediate head): run `40450166227`, attempt
    1, every job executed, each read job-exact: rust 585/0/5, 34/34 floor
    lines, the 4 count gates; control-plane every step green, `npm test`
    205/205, package-reconcile 4/4, the refused start `ok` (2020 ms); dress
    29/29, 0 drops, step 4's rise 3006 ms. Rows: R10's 69, G25, F12, CP5.
  - **CI at `ae709db`** (the fresh witness): run `40451987570`, attempt 1,
    every job executed, each read job-exact: rust 585/0/5, 34/34 floor
    lines (`zerocopy_tap` 15/14), the 4 count gates, R10's drain tests `ok`
    in both runs of their suite, `three_frames…` `ok` in both steps;
    control-plane every step green, `npm test` 205/205, package-reconcile
    4/4, the refused start `ok` (2019 ms); dress 29/29, 0 drops, steps 5 and
    6 `ok`. Rows: R10's 70, G26, F13, CP6.

  **The two-key pass on PR #74** (independent; relayed by the user,
  2026-10-26) HOLDS at `ae709db`. The builder concurs with each ruling.

  **The user's rulings of 2026-10-26, on #74's two-key pass:**
  1. **The pass's dispute is upheld:** the dispatch overstated R32's
     demotion to watch as done; the register update lands with the merge,
     per the ruling.
  2. **Both nuances accepted:** the 1 ms in the median is convention (the
     reading template gains the convention line); the reload line's
     initial-load firing is noted on R32's instrument note.
  3. **The voids stand as named:** no fresh 50-run control by the pass
     (the kept evidence read; its disk 2.6 GB free, 99% full, load
     2.6–3.5); the no-reload probe functional only (load 3.45, no timing
     read); the count gates not individually re-listed (CI green); plus the
     row's existing two, the engine-start starvation (→ `eef690f`, rebased
     `f2adf8a`) and the preflight starvation (→ `9dfd84b`, rebased
     `ae709db`).
  4. **The median line is ruled** (the user's ruling of 2026-10-26, on the
     bookend): a reading reports the kept summary's printed figure (the
     reproducer prints the upper of the two middle values, 3870);
     re-derived figures are noted as such. The reading template's line says
     so.

  **Merged: PR #74 is `4bef9ed`** (the bookend), on the user's word.
  - **The merge:** a squash at 2026-10-27T05:25:28Z, matched to `ae709db`,
    with the staged subject and body (byte-identical). Its one parent is
    `27368f2`.
  - **The tree is identical to the reviewed head's:** `4bef9ed^{tree}` =
    `ae709db^{tree}` = `eee3fea`.
  - **Main's CI:** run `40455238385` (push), attempt 1. Every job executed
    (each started 05:25:33–05:25:34Z), and each was read job-exact.
    **Green.**
    - **rust** (job `122026909134`): 585/0/5 (the step's `TOTAL` line),
      34/34 floor lines (`zerocopy_tap` 15/14), the 4 count gates, 8 `SKIP
      normative:` lines; R10's drain tests `ok` in both runs of the suite;
      `three_frames…` `ok` in both steps.
    - **control-plane** (job `122026909528`): every step green, `npm test`
      205/205, package-reconcile 4/4, the refused start `ok` (2018 ms);
      warm builds (full cache matches), the bracket's window CP7.
    - **dress** (job `122026909307`): 29/29; steps 5, 6 and 12c `ok`; drops
      0, underruns 213, watchdog trips 0; rises 2025 and 1986 ms.
  - **On main's tree:** R33's, R34's and R35's rows; R31 FIXED by
    `27368f2`; the four §2c strikes (the #73 dispatch's R35 claim with the
    oversight note, R21's cold-runner correlation, the infrastructure
    list's R32 attribution, `SKIP normative:` 9 → 8 with the template line);
    the infrastructure list's R21-only item; the restored blank line.
  - **R32 moves to watch** (its row, with these records), its two standing
    instruments named.
  - **The envelopes** (Finding R10's tables): R10 row 71 (19.13 holds), G27
    (7.46 holds), F14 (10.02 holds), CP7 (8.46 holds). **R15's watch**
    notes the run (its row).

  **The arc's ledger closes** (the user's ruling of 2026-10-26): R31 FIXED
  (`27368f2`), R32 on watch, R33, R34 and R35 filed.

  **#74's records, PR #75** (branch `records-after-74`, off `4bef9ed`;
  docs-only, one commit `e048917`).
  - **CI at `e048917`:** run `40458035715`, attempt 1, every job executed,
    each read job-exact: rust 585/0/5, 34/34 floor lines, the 4 count gates,
    R10's drain tests `ok` in both runs of their suite, `three_frames…` `ok`
    in both steps; control-plane every step green, `npm test` 205/205,
    package-reconcile 4/4, the refused start `ok` (2027 ms); dress 29/29, 0
    drops. Rows: R10's 72, G28, F15, CP8.
  - **The two-key once-over** (independent; relayed by the user, 2026-10-26)
    HOLDS at `e048917`: no disputes, no voids.
  - **Merged: PR #75 is `6bb8019`** (the bookend), on the user's word.
    - **The merge:** a squash at 2026-10-27T06:22:32Z, matched to `e048917`,
      with the staged subject and body (byte-identical). Its one parent is
      `4bef9ed`.
    - **The tree is identical to the reviewed head's:** `6bb8019^{tree}` =
      `e048917^{tree}` = `e654b8b`.
    - **Main's CI:** run `40460725223` (push), attempt 1. Every job executed
      (each started 06:22:37–06:22:38Z), and each was read job-exact.
      **Green.** rust 585/0/5, 34/34 floor lines, the 4 count gates, R10's
      drain tests `ok` in both runs of their suite, `three_frames…` `ok` in
      both steps; control-plane every step green, `npm test` 205/205,
      package-reconcile 4/4, the refused start `ok` (2028 ms); dress 29/29,
      0 drops, steps 5, 6 and 12c `ok` (step 4's rise 4036 ms, logged,
      inside the 10 s backstop).
    - **The envelopes:** R10 row 73 (19.13 holds), G29 (7.46 holds), F16
      (10.02 holds), CP9 (8.46 holds).
  - **The R31–R35 arc is closed** (the user's ruling of 2026-10-26): R31
    FIXED (`27368f2`) with its permanent probe; R32 on watch with two
    instruments (the control-plane bracket, the reload log); R33, R34 and
    R35 filed; R21 LOGGED, its sighting 2's artifact saved; four envelopes
    live (R10's workspace, R15's GPU step, the floor bracket, the
    control-plane bracket).
  - **Standing items after the arc:** R35's fix PR next (its row's
    addresses: `record/stream.rs:818` → `:962`); the ceiling-downward
    proposal already on this map as queued, not in force (it landed with
    `4bef9ed`); the dress step-5 backstop on the infrastructure list, wanted
    by R21; `r32-cp-falsify-ci-temp`'s local branch the user's to delete
    (its remote is gone).

  **The user's rulings of 2026-10-27, on the morning's maintenance and R35's
  plan:**
  1. About 8 GB free, enough for R35's builds and recordings. The 36 local
     branches' prune and a `cargo sweep` of `target/` go on the maintenance
     list; `git branch -D r32-cp-falsify-ci-temp` stays the user's.
  2. **R35's plan is approved as designed.** The red-first gate is mandatory,
     and its clean outcome was ruled in advance: 0 stale frames on main's
     code on both GPUs stops the work before building, the falsification
     void, the fix landing as guarding a latent defect (filed as latent, no
     falsification, permanence by the probe), and the row saying so.
  3. **The counter-proof on Intel only:** one demonstration of the class
     returning, R31's battery convention.

  **The user's rulings of 2026-10-27, on R35's red:**
  1. **Proceed to R35's fix.** The kept 10-minute Intel window is sufficient
     red; no further AMD red is required before implementation.
  2. **The evidence boundaries:** Intel is the kept red. The 30-minute AMD
     window is VOID: its artifacts and observations kept, not counted as
     normative red. The end-sample correction is recorded: the probe's
     actual stream-end sample was 3.61; the runner's later 2.82 was read
     after the analysis. The background load's cause remains unknown, and
     the R31 comparison does not establish that the probe contributed
     nothing. The refill-point clustering is a correlation, not a proven
     cause.
  3. **The fix:** the draw-completion handoff and the stream-thread wait;
     the View never waits, the wait is bounded, a skip is counted.
  4. **The verification:** the fixed code in a kept 30-minute AMD window
     and a kept 10-minute Intel window, targeting zero stale pictures; the
     counter-proof on Intel, the wait removed, the class returning in a kept
     window; the unit and GPU guards and the CI gates.
  5. **The load rule is not relaxed** to obtain a kept result: a void
     window is preserved, its reason named, reported as void.
  6. **The red's exact source state is preserved** before the engine
     changes (main's SHA, the probe and receiver or their snapshot, the
     invocation, the evidence paths). The new receiver and the stream
     classifier are part of the measurement surface, in review with the
     frame-to-tick basis.
  7. **R35's row distinguishes** the kept Intel red, the attempted but void
     AMD red, and the fixed-code and counter-proof results.
  8. The PR is held DO NOT MERGE for its two-key review and the user's word.

  **R35's fix PR** (branch `r35-stream-fence`, off `6bb8019`).
  - **The measurement surface** (`3418075`, the red's source): the 12c
    probe's `--path stream` (stream-only, the View drawing into the stream's
    own pool surface) and `src/rtmp-flv-receiver.ts`, which answers the
    engine's three RTMP commands (ffmpeg's `-listen` refuses its publisher)
    and writes each video message whole, with its timestamp, into the run's
    FLV. **The frame-to-tick basis:** the stream thread's PTS index is the
    master frame less the stream's first; the receiver keeps each message's
    timestamp; the analysis rounds `pts_time × 30`; the constant cancels in
    each take's offset. The red's source is kept in
    `.git/nbe-staged/r35/red-source/` (both files with their sha256, the
    probe's diff, the invocation); the engine was main's `6bb8019`.
  - **The red** (main's code; `red.sh`, `red.log`, `red-reading.md`):

    | window | adapter | frames | stale | the window |
    |---|---|---|---|---|
    | 10 min | Intel UHD 630 | 18,303 | **16**, each the previous frame's; 14 at clip frame 87 | **KEPT** (admitted 2.19; its own ends 2.38 and 2.35) |
    | 30 min | AMD Radeon Pro 555X | 54,144 | 9 observed (8 one back, 1 two back) | **VOID**: its own stream-end sample 3.61; at or above 3.0 in 117 of 184 samples, max 6.31; the cause unknown. *(§2c: the runner's 2.82 was read after the analysis, the builder's slip in where the end was read.)* |

    A 1-minute dry run (not counted) had shown one: slots 88, 89, **89**, 91
    at contiguous timestamps.
  - **The fix** (`7989f5a`): `DrawDone::of_view`, this frame's View draw or
    none, shared by the record loan's `mark_drawn` and the stream; the tick
    hands the stream that draw with whichever surface it takes (the record
    loan's or its own); a surface with no draw to wait on is never handed
    off, a stream drop, counted; the stream thread encodes through
    `encode_when_drawn` (R31's `encode_after_draw`: a zero-timeout poll every
    500 µs up to 1 s), the View never waiting, an unfinished draw a counted
    drop, never an encode.
  - **Guards:** `record::stream` 2 unit tests (the wait precedes the encode,
    bounded by `DRAW_WAIT`; an unfinished draw is a counted drop, never an
    encode); `zerocopy_g1` +2 GPU tests (a drawn stream surface reaches the
    stream thread with its draw and its frame; a View that did not draw,
    after one that did, hands the stream nothing), its CI floors 4/3 → 6/5
    (`d4edfc8`); `prompt10_stream_cmds`' channel-full check offers a real
    draw.
  - **Verified as ruled** (`green.sh`, `green.log`; each window admitted
    under 2.6 and kept on its own first and last samples, both under 3.0):

    | window | code | frames | stale | ends (max) |
    |---|---|---|---|---|
    | AMD (normative), 30 min | `d4edfc8` | 54,323 | **0** | 2.08 / 2.04 (6.55) |
    | Intel, 10 min | `d4edfc8` | 18,324 | **0** | 2.13 / 2.60 (3.32) |
    | Intel, 10 min, the wait removed | `d4edfc8` mutated | 18,283 | **3**, each the previous frame's, at clip frame 87 | 2.13 / 2.05 (3.60) |

    The fixed runs' draw wait never timed out (no "did not finish" line);
    every window read 0 View drops, 0 read-ahead misses, 0 watchdog trips.
    The mutation (`encode_after_draw(drawn, encode)` → `Some(encode())`) is
    kept with its run; the tree was restored and the engine rebuilt from
    HEAD.
  - **The gate** (`gate/plan.py`, `hygiene/fx.py`, at `d4edfc8`): the
    workspace **589/0/5** (585 + the 2 unit tests + the 2 GPU tests), kept on
    its third window (two voids, named: 2.57 → 4.21, WindowServer 5.2%,
    bun, bluetoothd; 2.46 → 3.13, WindowServer, locationd, opendirectoryd);
    `npm test` 205/205; e2e 28 + 1 skip (the refused start: this machine's
    encoder acknowledges); the dress 29/29, 0 drops.

**Guards** (the counts move: `resync_end` 7, new; mirror 20 → 22;
`npm test` 129 → 134; the resync-end end-to-end 3, new):
- **`resync_end.rs`:**
  - a restarted engine ends the item at its remaining time;
  - an overdue one ends on receipt;
  - a redundant resync on a healthy engine leaves exactly one end;
  - an untimed item schedules nothing;
  - a resync with no end leaves a pending one alone;
  - an end that does not read, or names another item, refuses the resync
    before anything applies;
  - a stopped show drops the end.
- **The audit trio:** mirror +2, `protocol.test.ts` +1.
- **`resync-snapshot.test.ts` (4):**
  - the remaining time from the take, rounded up;
  - clamped at both ends;
  - none for an untimed, done or empty View;
  - a recall starts the clock.
- **`resync-end.e2e.ts` (3), a real engine killed and restarted:**
  - an 8 s item restarted at 3 s ends at its own 8 s and advances once;
  - a 2 s item whose duration elapses during the outage ends on receipt, and
    the audit says so;
  - an untimed item across a restart: nothing ends.

**Falsified** on the normative machine. Each mutation was restored from a
saved copy, checked identical, and both engine binaries rebuilt. Every run
started at load 2.19–2.48, on AC under `caffeinate`, with no sleep or battery
event in the window (22:30–22:50). The runs are power-limited (§2a): the
adapter is 45 W, and the battery discharged on AC in 40 of 229 samples, down
to −1201 mA.

| # | Mutation | Signature |
|---|---|---|
| F1 | Engine: `on_resync`'s scheduling reverted (the pre-fix tree) | `resync_end` 5 of 7: `the end arrives at the remaining 300 ms`, `left: 0`; `an overdue item ends on receipt of the resync`, `left: 0`. End to end: `autoFollow must advance R1 -> R2 at R1's end after the restart; the View is still R1 (outage 110 ms)`, and S1 never advances |
| F2 | Control plane: the FULL duration sent instead of the remaining time | unit 1 of 4: `remainingFrames: 90` for `60`, `90` for `1`, `90` for `0`, `90` for `75`. End to end: `R1 must end at its own 8000 ms from the take, not its full duration from the restart: ended 11103 ms after the take (delta 3103 ms; outage 105 ms)`, and S1's overdue end came `1992 ms after the engine returned` |
| F3 | Control plane: the clamp at zero removed | unit: the clamp test fails with `ZodError` (`too_small`, the strict parse refusing a negative). End to end, the misbehaviour: the parse throws inside the connection handler at `sendResync`, before the socket's listeners are installed. The restarted engine is registered but never resynced, so S1 stays on air (`the View is still S1`). When the engine is killed again, `the control plane never noticed the engine go` *(§2c, 2026-10-08: since `94dec15` this zombie can no longer form. The listeners install first, and a failed handshake deregisters the session and closes the connection with 1011, so the same throw is now a loud teardown and a clean reconnect.)* |
| F4 | Engine: `is_current` always true (a superseded end not dropped) | `(ends by 800 ms, ends after): one end, never two`, `left: (2, 0)` |
| F5a | Engine: a resync ends the View item with no end on the snapshot | `untimed items, a fresh resync and a resync after a take: no end`, `left: (0, 1)` |
| F5b | Control plane: an end sent for an untimed item | unit: `untimed: { itemRef: 'U1', remainingFrames: 0 }` for `undefined`. End to end: `resyncEnds: [ { itemRef: 'U1', remainingFrames: 0, durationFrames: null, … } ]` for `[]` |
| F6 | Engine: the item check removed | `another item than the View: ()`: the resync that names T2 while T1 is on air is accepted |

### The zombie-registration fix — a failed resync handshake tears down, loudly (2026-10-08, branch `zombie-resync-fix`)

**The defect, F3's shape, demonstrated at the tree.** The engine's connect
path in `server.ts` ran register → `sendResync` → install listeners.
`sendResync` builds the snapshot inside the connection handler, so any throw
while the snapshot was built skipped the listeners: the strict parse (F3's
unclamped end), or a throw in any builder it calls. The result:
- the session stayed **registered** but was never resynced;
- §5.9.4 rule 2 had the engine hold and apply nothing;
- the control plane never processed its frames, and never noticed it go.

The four pins that held `show.resync` on connect stayed green throughout.
This fix hardens the failure path, not the success path.

**The tree adds a second shape.** `WsRenderBridge.sendDirect` catches a send
throw itself (`console.warn`) and reports `false`, and `sendResync` ignored
the result. So a resync that failed to *send* never threw. The listeners
installed, and the engine stayed registered, heard, and never resynced.

**The handler's new order** (`server.ts`, `wss.on("connection")`):
1. `clients.set(connId, session)`;
2. the telemetry closures, and `forget()`: the client entry, the render
   registration and the telemetry timer released, idempotently;
3. **the listeners**: `message`, then `close` → `forget`, then
   `error` → `forget`;
4. **the render handshake, in one guard**: `wsBridge.register`, then
   `sendResync`, which now returns whether the socket accepted the resync;
   an unsent one throws.

**The teardown path.** On any failure: `forget()`; then the audit log records
`resync.handshakeFailed` (kind `command`, outcome `rejected`, the cause in
`detail.error`); then the server's `warn` hook (`console.warn` unless a
caller passes its own); then `ws.close(1011, "show.resync handshake
failed")`, which the engine sees on the wire. The engine's reconnect loop
retries against a clean slate (`channel.rs`: "control plane connection ...
reconnecting"). The close event that follows calls `forget()` again,
harmlessly.

**The invariant**, written on the handler: *a connect either completes its
resync or leaves NO registration. There is no third state.*

**Wire-visible, so the spec carries it.** The spec held no close semantics
anywhere, and §5.9.4 rule 2 is what turns a silent handshake failure into an
indefinite hold. So the failure half lands as §5.9.4 rule 1's new paragraph,
v0.4.8 row 4 (`955b43f`). The audit event is internal, and no audit kind is
new.

**The tree, against the order.**
- **The adjacent check: no other registration path has the shape.** There is
  one WebSocket server and one connection handler. Every role's client entry
  is set before the listeners, but for a non-render session nothing that can
  throw runs between them: only closures are defined. Now the listeners
  follow at once for every role.
- **The `resyncRequest` path is unchanged.** An established connection that
  asks again still gets `sendResync` with its result ignored, and a throw
  there is answered as an `E_ENGINE` error on the socket, as before. It is
  not a registration path, and it is outside this order.

**Guards.** `src/resync-handshake.test.ts` (3, in `npm test`, 134 → 137), on
a real server and socket, one standalone injector each:
- a state whose `resyncSnapshot` throws (the build failure);
- a bridge whose `sendDirect` refuses once (the send failure);
- an operator's connection beside a failed render handshake, which is
  untouched.

Each failure must close the connection with 1011 and the reason, hold no
registration before or after the engine goes, be audited and warned, and
leave the next connect resynced first. The success path is unchanged and
green: the four connect pins (`render-channel.test.ts` tests 7, 8 and 9,
`server.test.ts`'s in-order pin), `resync-snapshot.test.ts` 4/4, and the
resync-end end-to-end 3/3.

**Falsified** on AC under `caffeinate`, 04:21–04:23, with no sleep or battery
event. Each mutation was restored from a saved copy; TypeScript runs from
source, so there is nothing to rebuild. Every run started at load 2.07–2.36.

| # | Mutation | Signature |
|---|---|---|
| Z1 | The old handler restored | the zombie, all three tests: `closed: 'never: the socket stayed open'`, `registrations: 1`, **`registrationsAfterTheEngineGoes: 1`** (the control plane never notices it go), `audited: []`, `warned: false` |
| Z2 | Listeners first, the failure swallowed | `closed: 'never…'`, `registrations: 1`, `audited: []`, but `registrationsAfterTheEngineGoes: 0`: the close is heard, and the third state lasts until the engine leaves. Each half of the fix carries its own weight |
| Z3 | The send result ignored again | the send-failure test alone: `closed: 'never…'`, `registrations: 1`, `audited: []` |

### The package-reconcile fix — a resync reconciles the package by load identity (2026-10-09, branch `package-reconcile`)

**The order** (the user's words of 2026-10-08): reconcile the package by load
identity on every resync. It is SPEC v0.4.8 row 5 and the queue's position 1
above *(§2c, 2026-10-10: that line is now marked in flight, PR #40, and the
numbered queue begins with R11)*. The strike of that queue line waits for the merge: it lands in a
post-merge records commit on main, by the merge SHA. *(Done, 2026-10-11:
struck FIXED by `b961cc5`, the merge of PR #40. Main's history takes only
pull-request merges, so the strike rides in PR #41's records.)*

**The premises, each read at `7cd9d1a` before anything changed.** All six
hold.
- *The snapshot carries `packagePath`, and the engine ignores it.*
  `state.ts` `resyncSnapshot` sends `packagePath: this.pkg?.packagePath ??
  null`. `directive.rs` reads `packagePath` in one place, `on_show_load`, and
  `on_resync` never touches it.
- *`show.start` only starts the clock.* `on_show_start` is
  `self.state.clock.lock().unwrap().start(); Ok(())`. Everything
  package-shaped is set in `on_show_load`: the index, the packaged slate, the
  media, the audio-level watches and the record target.
- *The control plane refuses `show.load` while RUNNING.* `commands/show.ts`:
  `"cannot load a package while the view is live"` (`E_FORBIDDEN_STATE`). So
  there is no recovery on air, and the manual path was stop, load, start.
- *The resync carries overlay IDs only.* `visibleOverlays:
  Array.from(this.visibleOverlays)`: names, which resolve only against a
  loaded package.
- *`packagePath` has no spec status in §5.9.4.* Rule 1's field list at HEAD:
  "`showState`, `viewItem`, `viewItemStartFrame`, `viewItemEnd`,
  `previewItem`, item states, scene states, visible overlays,
  `automationHold`, and the `stateVersion` it was taken at". The field was on
  the wire with no spec and no schema.
- *The divergent case is rule 4's.* Rule 4: "Directives issued while no render
  node was connected MUST NOT be replayed. The snapshot, not the backlog, is
  the recovery mechanism." A stop, load and start during the outage is that
  backlog, so only the snapshot can carry the new package.

**Two names in the order that the tree does not hold.** The tree wins.
- **"§2e".** `docs/implementation-standards.md` has sections 1, 2, 2a, 2b,
  2c, 3, 4, 4a and 5, and no §2e. The power rule applied is the one the tree
  has: the soak protocol's precondition 6, as the falsification records under
  §2a have applied it. The adapter is 45 W, so every timing run here is
  power-limited. That is recorded, not void.
- **"§2a's read of stateVersion on both sides".** §2a is the standards'
  falsification section, and it names no stateVersion read. It is read as
  this order's item 2(a), the load generation each side holds, below.

**Neither side held a load generation**, so both were added, minimally, in
memory:
- **The control plane:** `ControlPlaneState.packageLoadStateVersion`
  (`state.ts`). ~~`show.load`'s handler sets it to `ctx.stateVersion` right
  after `state.loadPackage` (`commands/show.ts`)~~ *(§2c, 2026-10-09: it now
  records the version the dispatcher assigns, through `afterBump`;
  `ctx.stateVersion` could be overtaken, Defect 1 in the delta below)*, and
  `loadPackage` and `unloadPackage` clear it. The `show.load` directive is
  forwarded at that same `stateVersion`.
- **The engine:** `EngineState.loaded_package` (`state.rs`,
  ~~`LoadedPackage { path, load_state_version, ok }`~~
  `LoadedPackage { path, load_state_version }`; §2c, 2026-10-09: `ok` is gone
  with the no-retry rule, below. *(§2c, 2026-10-10: the strike sat inside the
  code span, where it rendered as literal tildes, so it is restructured to
  render; the text is unchanged.)*) `record_load` writes it,
  from `on_show_load` with the directive's `state_version`, and from
  `reconcile_package`. There is also `package_load_attempts`, a counter the
  guards observe (G3's load count). `package_generation` already existed: it
  is the render and audio side's load boundary, not the identity.
- **Nothing on disk.** The control plane's persistence never restores a
  package (`persistence.ts` `restore`: `this.state.pkg = null`, and an
  explicit `show.load` is required). So a generation cannot outlive its
  package across a restart. ~~The restored `stateVersion` also keeps the next
  load's generation above every earlier one.~~ *(§2c, 2026-10-09: not every.
  `show.load` is not flushed at once: persistence writes after a 250 ms
  debounce (`persistence.ts`), so a crash inside that window restores a
  version below the last load's. A repeated generation also needs the same
  path, that exact version, and an engine that missed the new load.)*

**The identity, as built.** The pair is `(packagePath,
packageLoadStateVersion)`:
- the control plane sends it on every resync (`resyncSnapshot`, through the
  strict `ResyncPackageSchema`);
- the engine holds `loaded_package` and compares the two.

`ResyncPackage::read` (`nbe-protocol`) refuses a generation with no path, and
`identity()` is `Some` only when both are present. So an older control plane,
which sends the path without the generation, reconciles nothing.

**The extraction's shape.** `on_show_load`'s body moved, unchanged, into
`load_package(&self, path)`, which begins by counting the attempt.
`on_show_load` parses the path, calls `load_package`, then `record_load` at
the directive's `state_version`, and returns the load's result. That keeps
its error, so `show.load`'s behaviour is unchanged. `load_package` has two
callers, read at the branch head:
- `on_show_load`, which the dispatcher still runs on the blocking pool;
- `reconcile_package`, which `on_resync` awaits through `spawn_blocking`
  before anything else in the snapshot applies.

The dispatcher's `RESYNC` arm became `self.on_resync(d).await?`. The load
lands first, because the View, the overlays and the end all reference its
content. Content is trusted as-is, as `show.load` trusts it: no
fingerprinting.

**The comparison** (`reconcile_package`):
- **no identity:** nothing;
- **equal path and generation:** nothing~~, and it returns whether that load
  failed~~;
- **different, or none loaded:** `load_package`. On failure it logs at
  `error`, calls `forget_package()`~~ and records the failed identity~~.
  *(§2c, 2026-10-09: a failed load is no longer recorded, so a resync at its
  identity retries; the delta below.)*

`forget_package` clears the index, the packaged slate, the media, the audio
assets and per-item audio, the watches and the record target, and moves
`package_generation`. Nothing of the previous show stays renderable or
audible. ~~Recording the failed identity means a resync at the same identity
does not retry it (no retry loop). The operator's next `show.load` is a new
generation, and that does.~~ *(§2c, 2026-10-09: there was no loop to
prevent. Only a sequence gap makes the engine request a resync, and a failed
load is not one. What the record did was block a heal, and the user's word
reversed it: the delta below.)*

**Failure semantics.** A failed load is not connection-fatal. The channel
applies a resync with `handler.apply(&frame).await?`, so an `Err` drops the
connection and every reconnect would retry the load (F5 below shows the
storm). Instead the rest of the resync applies: the show state, the View,
the preview, the overlays, the slate and the end. Then, if an item is on air,
the engine:
- engages its slate (`FallbackSource::Held`). With no package this is the
  generated slate, `[191, 89, 13, 255]`;
- emits `itemEvent missing` for it, with detail `packageNotLoaded`.

The control plane already handles that event. `server.ts` passes it to
`state.markMissing`, which sets the item MISSING and, for the View item,
`fallbackActive`. It is audited as `engine:missing`. With no item on air,
nothing is reported and the slate stays where the snapshot put it. This
matches `markMissing`, which raises the slate only for the View item.

**Why load identity** (the decision record). The alternatives the user
weighed:
- **Warn-only:** the restarted engine stays dark for the rest of the show,
  and recovery is still stop-load-start on air.
- **Void-only (reload only when the engine holds no package):** it heals a
  restart but not the divergent case. A stop and a load of another package,
  or the same path re-authored, during the outage leaves the engine rendering
  the previous show under the new show's item names. F2 shows both.
- **Connection-fatal on a failed load:** a refused resync drops the
  connection, and the reconnect retries the same load, a storm with no
  operator signal. F5 shows it.
- **Path alone:** it misses a same-path reload with new content (F4).

The generation is the cheapest value both sides already agree on: the
`stateVersion` of the `show.load` the engine applied.

**Scope: the no-package snapshot is out.** A `show.unload` during the outage
leaves a snapshot naming no package, and that reconciles nothing. Routing
`show.unload` to the engine is a separate queued item, in Prompt 11's WU8
record, under PR #34's release-parity fix ("route `show.unload` to the
engine"). It now points back here.

**The latency, measured** (`package-reconcile.measure.ts`, R9's convention:
not a test, not in `npm test`, VOID at load 3.0). The real control plane and
the real release engine. Each sample kills the engine, restarts it, and times
from its registration (the resync sent) to `/status`'s
`lastAppliedStateVersion` reaching the resync's `stateVersion`. The two tiers
are a baseline with no package, and the dress package loaded and running with
A1 on air. The dress package is `tests/fixtures/dress_show`: 136 KB, three
video assets, one image and one audio. The run was 2026-10-09
00:14:10–00:16:27, on AC (adapter 45 W, Low Power Mode on), under
`caffeinate`, with no sleep or battery event and no discharge on AC (0 of 27
samples). It started at load 2.41, after a wait for load under 2.5:

```
baseline (no package named, no load): n=5 ms=[31.1, 3.4, 2.6, 2.4, 3.3] median=3.3 max=31.1 load 2.37 3.10 2.74 -> 2.37 3.10 2.74
reload (the dress package reloaded on resync): n=5 ms=[1273.3, 1270.8, 1268.5, 1264.3, 1275.6] median=1270.8 max=1275.6 load 2.37 3.10 2.74 -> 2.50 3.11 2.75
added by the reconciliation (median reload - median baseline): 1267.5 ms
```

**About 1.27 s added, on the reload path only.** It is the package's own load
time: `show.load`'s video decode, which already runs on the blocking pool. An
equal identity, the blip reconnect, adds no load (G3: one attempt, no
boundary crossed). During that time §5.9.4 rule 2's hold applies, and a
restarted engine has nothing to hold. The number is power-limited and
single-machine. It is a record, not a bound.

**Guards.** The counts move:
- `package_reconcile` 8, new;
- mirror 22 → 24;
- `npm test` 137 → 139;
- the package-reconcile end-to-end 2, new.

**`package_reconcile.rs` (8, GPU; pixels through the real render loop, the
`prompt13_recall` shape):**
- **G1**, a fresh engine resynced mid-item: BLACK before, then A1's red after
  exactly one load;
- **G2**, a divergent package: the new show's green A1, after two loads;
- **G3**, an equal identity: one attempt, no load boundary crossed, still
  red;
- **G4**, a same-path reload with a new generation, the file re-authored to
  green: green, after two loads;
- **G5**, a package that does not load:
  - the resync is `Ok`;
  - `missing` A1 once, the View still A1, running, the slate up, the
    generated slate's pixels, one attempt;
  - ~~a second resync at the same identity makes no new attempt and reports
    `missing` again~~ *(§2c, 2026-10-09: a second resync at the same identity
    retries once and reports `missing` again, and nothing retries between
    resyncs)*;
- **G5b**, a divergent package that fails: the generated slate, then BLACK
  after the next take, never the previous show's red;
- a resync naming no package leaves the engine's package alone;
- a package identity that does not read refuses the resync before anything
  applies.

**The audit trio:**
- `resync_package.json`: 4 valid cases and 6 invalid ones, the invalid ones
  being a generation with no path, −1, 1.5, `"12"`, a numeric path and an
  unknown key;
- `mirror.rs` +2: the fixture read both ways, and the Rust and TypeScript
  field sets compared;
- `protocol.test.ts` +1.

**The rest:**
- `resync-snapshot.test.ts` +1: a loaded package is named with its
  generation, and no package names neither.
- `package-reconcile.e2e.ts` (2), the real engine killed and restarted. The
  witness is one only a loaded manifest produces: the package declares a
  record directory, and the engine's report measures `recordSpaceMib`
  against it (0 with no package).
  - **Test 1:** the snapshot pair equals `show.load`'s path and response
    `stateVersion`. The restarted engine measures the record target again,
    and A1 stays LIVE with no slate.
  - **Test 2:** the package is renamed away during the outage. A1 goes
    MISSING, both slates are up, `engine:missing` is audited, and a 3 s watch
    sees one connection and exactly one new registration.

The success path is unchanged and green at the branch head:
- the resync pins: `prompt13_recall` 6, `resync_end` 7, `reconnect` 2,
  `prompt03` 10, and `resync-handshake.test.ts` 3 in `npm test`;
- `show.load`'s suites: `prompt04` 17, `prompt05` 34, `prompt11_slate` 7,
  `take_duration` 9, `integration` 1, and the engine's lib 54;
- the resync-end end-to-end 3/3.

**Falsified** on the normative machine, 2026-10-09 00:17–00:45. Each mutation
was restored from a saved copy, checked identical, and the release engine
rebuilt (`falsify.sh`). The runs were on AC under `caffeinate`, with no sleep
or battery event, and each run started at load 2.32–2.48. They are
power-limited (§2a): the adapter is 45 W, and the battery discharged on AC in
71 of 331 samples, down to −787 mA.

| # | Mutation | Signature |
|---|---|---|
| F1 | No reconciliation (the pre-fix tree) | 5 of 8 fail. G1: `left: ([0, 0, 0, 255], 0)` for `([255, 0, 0, 255], 1)`. G2 and G4 stay red after one load. G5: `left: (0, Some("A1"), true, false, [0, 0, 0, 255], 0)`: no missing, no slate, black. G5b: red, then red. End to end, both fail: `reloaded: false`, and `a1: 'never missing: LIVE'`, `audited: false`, `controlPlaneSlate: false`, `engineSlate: false` |
| F2 | Void-only: reload only when no package is loaded | G2 and G4: `left: ([255, 0, 0, 255], 1)`, the previous show's A1, one load. G5b: red, then red |
| F3 | Always reload (the equality check removed) | G3: `left: (2, 1, [255, 0, 0, 255])` for `(1, 0, …)`. ~~G5: `(attempts, missing): the same identity is not retried`, `left: (2, 1)`:~~ ~~the retry loop~~ ~~a retry at the same identity, not a loop (§2c, 2026-10-09)~~ *(§2c, 2026-10-10: at `7879e87` F3 no longer fails G5, which now pins one attempt per resync and expects the retry. F3 re-run fresh at `7879e87`, restored from a copy and rebuilt, release binary byte-identical after: G3 alone fails in the engine suite, `left: (2, 1, [255, 0, 0, 255])` for `(1, 0, [255, 0, 0, 255])`, and G5 passes; end to end, test 4 fails, `reloadsOnTheReconnect: 1` for `0`.)* |
| F4 | Path-only identity (the generation ignored) | G4 alone: `left: ([255, 0, 0, 255], 1)`: the re-authored package never reloads |
| F5 | A failed load made connection-fatal (the resync refused) | G5: `a load failure must not refuse the resync (the connection would drop): Err(Invalid("show.resync: the package did not load"))`; G5b panics on the same `Err`. End to end, the storm: `engine never registered`, with `control plane connection dropped; reconnecting err=invalid directive: show.resync: the package did not load` at `delay_ms` 100, 200, 400, 800, 1600, 3200, 6400 and 10000 |
| F6 | No `missing` emitted (the failure swallowed) | G5: `left: (0, Some("A1"), true, true, [191, 89, 13, 255], 1)`: the slate up, nothing reported. End to end: `a1: 'never missing: LIVE'`, `audited: false`, `controlPlaneSlate: false`, while the engine's slate is up: the two sides disagree |
| F7 | A failed load's remains kept (`forget_package` skipped) | G5b alone: `left: ([12, 200, 90, 255], [255, 0, 0, 255])`: the previous show's packaged slate, then its red A1 after the next take |
| F8 | The load lands AFTER the rest of the resync | G1, G2 and G4: `left: ([0, 0, 0, 255], …)`: the load clears the View the resync just set, so a correct load still shows black |

**Adjacent paths, read at the branch head.**
- **The callers of the extracted body:** `on_show_load` and
  `reconcile_package`, and no others.
- **The emitters of `ItemEvent::Missing`:** at `7cd9d1a` there were none in
  the engine, and the variant was wire-only. The one emitter is this
  reconciliation's.
- **Its handlers:** the control plane's `itemEvent` path in `server.ts` (to
  `state.markMissing`), and `protocol.ts`'s `ItemLifecycleFrameSchema` enum. Both are
  unchanged.
- **§13.4.1 is unchanged:** `show.resync` has no row, and `missing` arrives
  on the existing engine-event path.

**Observed, not changed.** After a failed reconciliation, the engine's take
path does not consult the package. So a take renders black and reports
nothing (G5b's second value). The control plane already holds the item
MISSING, so the operator has the signal. The recovery is the next
`show.load`, a new generation, and the control plane allows that only off
air. *(§2c, 2026-10-09: or, under the retry rule, the next resync once the
package is back on disk.)*

**Recorded alongside: R11 reopens.** The gate's full workspace run at
`3e1aa2c` failed one test outside this change,
`a_slow_exit_inside_the_backstop_is_a_clean_stop`: `phase=exiting`, under the
ceiling. That is R11's post-fix sighting 5, and the third under the ceiling,
so by the user's rule of 2026-10-06 R11 reopens (Finding R11 above, and the
register row in `docs/soak-protocol.md` §5). The quiescent re-run was
508/0/3, matching CI. The fix's own suites never touch the stream thread.

**The two-key pass's delta (the user's word of 2026-10-09).** PR #40's
two-key pass, at `2090afa`, found two defects the build had not. It also
falsified the no-retry rule's rationale. The user ruled on its four open
items: fix both defects, reverse the rule, and correct the spec and these
records.

- **Defect 1, the generation race.**
  - **The cause.** `show.load` recorded `ctx.stateVersion`, which the
    dispatcher reads before the handler runs (`stateVersion:
    state.stateVersion + 1`, `dispatch.ts`). The version the command is
    actually assigned comes from the bump after the handler's
    `await loadPackage(…)`.
  - **Why it could differ.** Commands are serialized per connection only
    (`server.ts`, `session.tail`). Another connection's command, an
    automation action, or an engine `itemEvent` (`server.ts` bumps on each)
    could bump while the preflight ran.
  - **Reproduced twice.** The other connection's `automation.hold` took
    `stateVersion=1`, `show.load` returned 2, and the generation read 1. The
    first reconnect after that reloaded the package mid-show: 975 ms.
  - **Fixed** in the tree's own shape for after-handler effects,
    `HandlerOutput` (`extraDirectives` already wait for the bump). The new
    `afterBump(sv)` is called by the dispatcher right after the bump, before
    the forward at `sv`, with no await between. `show.load` records its
    generation there.
  - **A doc corrected.** `HandlerCtx.stateVersion`'s doc claimed "assigned by
    the dispatcher bump". It now says what the field is: the expected version,
    which an async handler can be overtaken on. It points at `afterBump`.
  - **The window left.** Between the handler's return and the bump, the
    control plane holds the new package with no generation. A snapshot built
    in that window names no identity and reconciles nothing, and the
    forwarded `show.load` follows it on the same connection.
- **Defect 2, a failed `show.load` then a resync at its identity.**
  - **The cause.** `on_show_load` recorded the failed identity (`ok: false`).
    Its failure path leaves the previous show's index in place; that path is
    unchanged, as ordered.
  - **The effect.** A resync at that identity took `reconcile_package`'s
    `!current.ok` early return, which skipped `forget_package`. The slate on
    air was the previous show's packaged slate, and the next take aired the
    previous show's A1.
  - **Fixed by the retry rule, not at the early return.** The order placed
    the fix at that return, and the same ruling deleted the return. A failed
    load, by `show.load` or by a resync, is now never recorded. So a resync
    naming it goes through the load, and the load's failure calls
    `forget_package`. No path through `reconcile_package` returns past a
    failure.
  - **`show.load` untouched.** Its load and its error are unchanged. Only its
    recording line changed: it now records a success alone.
- **The retry rule, reversed.**
  - **Why.** The no-retry record's rationale ("no retry loop") was false. Only
    a sequence gap makes the engine request a resync (`channel.rs`, the one
    `ResyncRequest`), and a failed load is not one, so a retry cannot loop.
    What the record did was block a heal.
  - **The rule now.** A resync at an identity whose load failed retries it.
  - **The code.** `LoadedPackage` loses `ok`, and `record_load` records
    successes only. `forget_package` also clears `loaded_package`, so after a
    failed reconciliation the engine holds nothing and the next resync
    retries.
  - **What the user should know.** The retry runs only when a resync arrives,
    and nothing in the tree lets the operator request one. So a package put
    back on disk heals at the next reconnect or sequence gap. Until the
    operator's `item.reset` and a take, the control plane holds the item
    MISSING with the slate up.
- **The markers consequence of any mid-show reload** (recorded, not changed).
  - `load_package` calls `markers::clear()` first: `show.load`'s own
    behaviour, so that a new show never inherits markers.
  - A reload while a recording runs therefore empties the store the
    recorder's sidecar is written from at finalize (`record/writer.rs`,
    `markers::list()`). The in-progress recording's sidecar loses the markers
    added before the reload. The recorder keeps writing; the load does not
    touch the record session.
  - The reloads that can meet a running recording:
    - the retry rule's heal;
    - a divergent or same-path reload, when the engine's recorder ran through
      the outage;
    - the race-born reload fixed above.
- **Queued: `show.stop`'s `stopVersion`** (position ~~4~~ 2 in the queue above;
  §2c, 2026-10-10, moved by the user's word of 2026-10-09). The
  same shape, older than this branch: `ctx.stateVersion` held across a 2 s
  await.
- **Guards.**
  - **The counts move:** `package_reconcile` 8 → 10, the end-to-end 2 → 4,
    and the CI floors with them.
  - **G5** now pins one attempt per resync and none between: two resyncs give
    `(2, 1)`, and after frames and 200 ms the attempts are still 2.
  - **G6** (Defect 2): a failed `show.load`, then a resync at its identity.
    `(missing, slate, after the take)` is `(1, GENERATED_SLATE, BLACK)`.
  - **G7** (the heal), giving `((1, GENERATED_SLATE, 1), (0, PACKAGED_SLATE,
    2), RED)`:
    - a resync at a missing path fails;
    - the package is written;
    - a resync at the same identity, with the slate as the control plane
      holds it, loads it;
    - the take airs it.
  - **End to end.** The engine now connects through a TCP proxy, so a test
    can drop the connection with the engine process still running.
    - **Test 2** adds `loadAttempts: 1`.
    - **Test 3** heals: one reload at the same identity in the same engine
      process, then `item.reset` and a cut. A1 is LIVE and both slates are
      down.
    - **Test 4** lands a second connection's `automation.hold` inside a
      dress-package load's preflight. The landing is proven by the load's
      audit row spanning two versions. A reconnect after that loads nothing.

**Falsified**, the delta's guards. Each mutation was restored from saved
copies, checked identical and rebuilt.

The runs were 2026-10-09 04:06–04:16, on AC under `caffeinate`, with no sleep
or battery event. Each run started at load 1.99–2.42. They are power-limited:
the battery discharged on AC in 29 of 116 samples, down to −178 mA. The
unmutated tree went first: `package_reconcile` 10/10 and the end-to-end 4/4.

| # | Mutation | Signature |
|---|---|---|
| N1 | Defect 1's fix reverted: the generation is `ctx.stateVersion` again | end to end, test 4: `generation: 9` for `10`, `reloadsOnTheReconnect: 1` for `0`. The race landed, and the reconnect reloaded mid-show |
| N2 | `forget_package` removed from the load's failure branch | G6: `left: (1, [12, 200, 90, 255], [255, 0, 0, 255])`, the previous show's slate and then its red A1. G5b fails the same way |
| N3 | The no-retry record kept: a failed identity is recorded as held | G7: the healing resync gives `(0, [191, 89, 13, 255], 1)`, no attempt and the generated slate, then `[0, 0, 0, 255]` after the take. G5: `((1, 0), 1)`. End to end, test 3: `reloadedAtTheSameIdentity: 0`, `reloaded: false` |
| N4 | A failed load re-requests a resync (a loop) | end to end, test 2: `loadAttempts: 4114` for `1`. The engine-only G5 cannot see this, since nothing answers its request, and the end-to-end does. *(Added 2026-10-10, from the fresh pass: test 3 also fails, `reloadedAtTheSameIdentity: 0` while `reloaded: true`. That is consistent with the loop reloading the restored package by itself before test 3's reconnect, which then finds an equal identity. The fresh pass counted `loadAttempts: 4066`; the count varies run to run.)* |

**Recorded 2026-10-10 (the user's word of 2026-10-09, ruling on PR #40's
fresh two-key pass).**

- **The counts.** The delta is six commits, `0392383..7879e87`. The order of
  2026-10-09 said seven; the delta report's table listed six. Its §2c
  corrections to this record are eight strikes in seven places; the order
  said six. Neither count had been written into this record.
- **The silent window after a failed engine-side `show.load`** (docs only, no
  behaviour change).
  - When the engine's own `show.load` fails, the previous show keeps
    rendering under the new show's item names, and nothing is reported. The
    channel logs `directive failed` and drops the error (`channel.rs`), so the
    control plane holds the new package and hears nothing.
  - The failure path leaves the previous show's index, media and audio in
    place, partly overwritten: the attempt is counted, decode sessions are
    released, markers are cleared, `package_path` is replaced, and the new
    slate may be installed.
  - It lasts until the next reconnect or sequence gap. That resync names the
    new identity and reconciles it: a reload, or on failure `forget_package`,
    the slate and `missing` (G6).
  - It is older than this PR and unchanged by it, as ordered. The heal is
    queued at position ~~5~~ 7 in the queue above (renumbered 2026-10-12).
- **Disk headroom.** 7.5 GiB was free at the fresh pass and 7.3 GiB at this
  note, under the soak protocol's 8 GiB precondition (`docs/soak-protocol.md`
  §2, precondition 3). R14's power measurements (its work order's step 5,
  Finding R14 above) and the AC-5 soak behind them stay blocked until
  housekeeping frees space.

---

## Cross-cutting, owned by no single prompt

| Item | Where it lands |
|---|---|
| Preflight resource enforcement (H2) | v0.4 outline §3 — schema/contract question, not prompt work |
| `showState` in telemetry (S2) | v0.4 outline §3 — wire contract |
| `viewItemStartFrame` in the resync snapshot | v0.4 outline §2, already confirmed |
| `sequenceRef` | v0.4 outline §5 — review recommends **retire**; evidence absent |
| §12.6 clamp wiring | Re-deferred; trigger is the first Apple Silicon machine or the first >1 GiB loop budget |
| ~~**The dress-rehearsal CI job is `continue-on-error`**~~ **DISCHARGED 2026-09-17 by the gate split** — superseded, original text standing per §2c | `.github/workflows/ci.yml:322` (the citation read `:248` until 2026-09-11; that line is now `echo "::endgroup::"` — the number rotted under edits in the very commit that wrote it, which is the argument for citing by name as well as line). Note there are **two** `continue-on-error: true` in the file and only one is this row's: `:178` belongs to the control-plane job's preflight-timing diagnostic step and stays deliberately non-failing. The job reports **pass regardless of step failures**, so its green is not evidence — on any PR, including the two that cited it. Three of its twelve steps fail today by design (R4, R5, R2), which is why the flag is there. **Either it gates or it is marked observational**; a check that always reports green teaches reviewers to read it as a result. Raised by the PR #11 two-key pass, 2026-09-10. **Work order DRESS closed R5, R4 and R2 (12/12 three times on the normative machine, each falsified) and attempted the promotion — which failed on CI and was reverted.** The row stays open because the reason changed rather than vanished: the gate asserts zero dropped frames and zero audio underruns, which are reference-hardware claims, and the macos-14 runner measured 83 then 120 underruns on 3 arm64 cores. Promotion now needs the gate split so composition claims run everywhere and threshold claims run where they mean something. See the DRESS entry under 07. |
| **Preflight bound vs measured decode cost [HIGH]** | **Recommended before Prompt 08.** Not 07b's scope; does not gate the 07 merge — the defect predates this branch and `main` carries it today. See the step-5c backlog entry under 07 for the evidence. **[DISCHARGED 2026-09-12 by PR #12 — streaming probe landed (163x less memory, bound re-derived); original text left standing per §2c.]** |

### The unbuilt-MUST register (opened 2026-10-19, the user's ruling 5 on PR #59)

A MUST the spec states and the tree does not build. Each entry names the MUST,
where the tree stops, what stands in for it, and its owner. An entry leaves
when the MUST is built and falsified, or when the spec is amended. The
register was opened with one entry; the spec has not been swept for others.

| # | The MUST | Where the tree stops | Owner |
|---|---|---|---|
| U1 | §12.10: "Preflight MUST report per loop" (`assetId`, `periodFrames`, `selectedTextureFormat`, `frameCostMib`, `effectiveBudgetMib`, `maxFramesByBudget`, `cachePolicySelected`, `vramResident`, `reason`) | `LoopReport` exists in `nbe-core`'s model and is never populated: preflight writes no `loops[]` entries (found on PR #59). A plan's `reason` therefore surfaces nowhere. For a streamed loop it reads "streaming with N frames read-ahead", which this engine does not do (R23), so it must not ship as it reads when the report is built | unassigned |
| U2 | `LayerAudio.gainDb` (`schemas/manifest.v0.4.json:730–734`; SPEC §8.2's per-bus gain, the element's own level) is declared and parsed (`nbe-core/src/manifest.rs:516`) | the engine reads only `audio.bus` and `audio.muted` (`crates/nbe-engine/src/scene.rs:766–777`); a take's gain is unity, or −60 dB for `mute` (`audio_control.rs:111–140`) | ~~device audio, this era~~ **built and falsified by the device-audio build PR (`ee311cc`); the entry leaves at its merge**; **it left at `0132d07`** (PR #70's merge; recorded in the next PR, §2c) (`docs/device-audio-design.md`, D4) |
| U3 | A loop's audio (§12.10 #5: "no audio gap if loop has audio"; §12.11.1 charges it in `audioDemandMib`) | it is decoded and held at `show.load` (`directive.rs:328–345`) and never routed to a bus: `item_audio_asset` takes `clip`-kind elements only (`scene.rs:475`) | ~~device audio, this era~~ **built and falsified by the device-audio build PR (`ee311cc`); the entry leaves at its merge**; **it left at `0132d07`** (PR #70's merge; recorded in the next PR, §2c) (`docs/device-audio-design.md`, D4) |
| U4 | §8.10: sustained underruns and device loss raise `E_AUDIO` | `E_AUDIO` is defined (`crates/nbe-protocol/src/lib.rs:82`, `:146`), and no engine path raises it | ~~device audio, this era~~ **built and falsified by the device-audio build PR (`5fd9f4b`): the lane's `audio.device` and `audio.underrun`, `E_AUDIO` in status (§8.10, §10.4); the entry leaves at its merge**; **it left at `0132d07`** (PR #70's merge; recorded in the next PR, §2c) (`docs/device-audio-design.md`, D5) |
| U5 | §8.10: "Device loss: `E_AUDIO`; attempt reopen with backoff; never block the render loop" (`docs/spec.v0.4.md:2053`) | ~~there is no device sink today.~~ *(§2c, the device-audio build:)* The device sink opens at engine start and does not reopen. A lost device (reported, or 1 s without a callback) leaves the engine silent on the null sink's cadence, the tick reading `lost`, and the lane raises `audio.device`, critical. What stands in: D5's `audio.device` alert, the interim surface (the user's ruling D8 of 2026-10-21) | a later device-audio PR (`docs/device-audio-design.md`, D8) |
| U6 | §8.2 #1: "The master bus MUST have: 1. compressor" | no compressor exists. The master stage is the master gain, then the limiter (SPEC v0.4.8 row 14) | a later audio PR, its parameters ruled first (the user's ruling L6 of 2026-10-21) |
| U7 | §8.2 #3: "loudness-safe output", against `show.audio.loudnessTargetLufs` (−16 LUFS by default), with §8.2 #4's peak metering folded in (the user's ruling L6) | nothing measures or normalises the master's loudness. The master meter (`Bus::observe`) reads sample peak and RMS, not true peak or LUFS. The limiter (row 14) bounds the true peak, which is all of loudness safety the tree builds | a later audio PR (L6) |

### The overlay level's four questions answered (recorded 2026-09-09, step 5)

The 07 prompt's §5.2 questions, answered before the code that implements them:

1. **Directive surface.** §16.6's `overlay.show { overlayId, animation? }` /
   `overlay.hide { overlayId }` needs no schema work; the existing surface is the
   answer. On receipt the engine applies the direction at the next frame boundary
   (`anim_start = master_frame + 1`, the same discipline AC-17 imposes on a take),
   never mid-frame, and a take never interrupts an overlay animation in flight.
2. **Ticker text stack.** Owned by 07b. The overlay level makes no rasterization
   decisions; it resolves overlay elements through the same `layer_for` walk as
   scene elements, so layout and rasterization stay off the frame path exactly as
   they do for scenes (§7.13). Nothing here weakens the packaged-font rule.
3. **Preview semantics.** The spec is silent; **no rule is invented**. Overlays do
   not composite on the Preview bus. Recorded as a v0.4 candidate.
4. **Persistence identity.** An overlay is identified by its manifest `id`; its
   pixels are untouched by a take (composition is `overlay(transition(A, B))`, so
   the overlay level is applied after the transition output every frame).
   `visibleOverlays` in §5.9.4 is a full snapshot: a present array — including an
   **empty** one — replaces the on-air set wholesale; an absent key leaves it
   alone (v0.4 §5.9.4's normative sentence, implemented here ahead of the merge).

**Recorded assumption (spec is silent):** `overlay.show` on an on-air overlay and
`overlay.hide` on a hidden overlay succeed as **idempotent no-ops**, returned with
`data.noop: true` and noted in the change stream; idempotent commands still
dispatch normally (one stateVersion bump, one audit record, one directive),
because acceptance accounting must not depend on the mutation's effect.
**[SUPERSEDED — "one directive" is wrong: a noop forwards nothing. See the
Step 5b records, entry a, and `5164383`.]** *Convention, recorded here: a
superseded entry carries its marker at the point of error, not only a forward
reference from the newer entry. A reader who stops at the stale sentence must
learn it is stale from the sentence itself.*

**Recorded clarification (fallback vs overlays; input to the next spec
revision):** SPEC §7.14 (§6.9 in the prompt's numbering) is silent on whether
overlays survive a fallback cut. Resolved per the FTB-above-DSK semantics the
prompt cites from the (unmerged, absent-in-repo) industry gap analysis: the
fallback slate composites **above** the overlay level — a fallback cut covers
ticker, bug, banner, and clock — and on recovery the pre-fallback on-air set
returns.

**Provenance, updated 2026-09-10:** both referenced research docs are now **in
the repo** — `docs/industry-gap-analysis-and-z-axis.md` and
`docs/move-parity-and-virtual-set-roadmap.md`. The gap analysis §3.4 states the
rule verbatim: "The fallback slate composites above the overlay level. A
fallback cut MUST cover tickers, bugs, and banners." So the clarification above
is **text-derived after all**, and the implementation matches its source rather
than merely agreeing with a sentence quoted in a prompt. The earlier
prompt-derived provenance is left visible above rather than rewritten, because
it was true when written.

Element renderers: this step is the composition level only. Overlay elements
resolve through the same `layer_for` path as scene elements; `ticker`, `clock`,
and `breakingBanner` glyph rasterization is 07b's scope. D1–D7 (rotation, pivot,
path, extended easings beyond the schema enum, scaleMode, z-swap,
`element.animate`) remain out; per-source envelopes and the `sfx` ramp invariant
stay with the audio work and are not deferred into this step's tail.

### Step 5b records — overlay level as built (recorded 2026-09-10)

Three entries, as-built, honest about the delta from the step-5 prompt:

a. **Idempotency assumption:** `overlay.show` on an on-air overlay /
`overlay.hide` on a hidden one is an idempotent success — the command is
accepted, stateVersion bumps once, `data.noop: true` is returned, and NO
directive is forwarded. (The step-5 prompt said "noted in telemetry";
as-built is `data.noop`. The code comment in `commands/state.ts` cites a
"recorded assumption" — this entry makes that citation true. It supersedes
the step-5 entry above, which wrongly stated "one directive": a noop
forwards nothing — `overlay.test.ts` "a noop overlay command forwards no
directive" guards this.)

b. **Fallback clarification, flagged as input to the next spec revision:**
the spec is silent on overlay/fallback interaction; the implementation
follows FTB-above-DSK semantics — fallback covers overlays; recovery
restores the pre-fallback on-air set. (`render.rs` gates the whole
scene+overlay branch under `show_fallback`; runtimes are preserved, alpha
stays a pure function of the master clock, housekeeping drops defer to the
first non-fallback frame.)

c. **Reductions, stated plainly:** overlay animations are linear alpha
ramps, duration-honoured, with declared easing unread and no positional
enter/exit; overlays composite on the View bus only; the snapshot's
`animationState` is command-moment state, not a live clock (a shown overlay
reports "enter" until further notice). (`payload.animation.easing`, when
carried, is ignored — see the `on_overlay` comment pointing here.) The
override is **show-only end-to-end**: `on_overlay` honours `durationFrames` on a
hide too, but `overlay.hide`'s schema is `strict({ overlayId })` and the handler
forwards `payload: {}`, so an exit-time override is engine-reachable and
wire-unreachable. Making one expressible is a §16.6 change for the next spec
revision, not a code fix.

Backlog (row I of the step-5c audit, 2026-09-10): the **debug** `nbe-preflight`
binary does not merely run slowly on `valid_show_v0.3` — it **blocks**. A
180-second bounded run produced no output and exited 124; sampled three times
during a 25-second run the process sat in state `SN` at 0.0% CPU having
accumulated 0:00.02 of CPU time. That is a wait, not work, so the step-5b
report's "hangs in video decode" is confirmed as a hang and the 8x debug/release
ratio recorded above does not explain it. Release is unaffected.

> **[CORRECTED 2026-09-10 by work order PREFLIGHT-BOUND — the paragraph above is
> wrong about the mechanism, and the error was mine.]** The debug binary does
> **not** block. Watched across 220 seconds it is state `RN` at 66–99% CPU
> throughout, RSS climbing 389 MiB → 3.25 GiB, and a 226-sample `sample(1)`
> stack is entirely in `probe_asset → decode_all → next_frame`, dominated by
> bounds-checked `Vec<u8>::index_mut` and `unchecked_add::precondition_check`.
> It is work — the same 1.87-billion-iteration BGRA→RGBA swizzle release runs,
> unoptimized.
>
> The `SN`/0.0%/`0:00.02` reading came from probing with
> `pgrep -f "target/debug/nbe-preflight" | head -1`, which matches **two**
> processes and returns the **`timeout` wrapper**, not the worker. Demonstrated
> side by side: `88387 SN 0.0 0:00.01 timeout` beside
> `88389 RN 98.6 0:11.82 nbe-preflight`. I sampled the wrapper's idleness and
> reported it as the subject's. The 180 s `exit 124` was real; the inference
> about why was not. Root cause and the full cost model are in
> `docs/preflight-bound-memo.md` §3.
>
> The original text is left standing per §2c rather than rewritten, because the
> mistake is the instructive part: a `pgrep | head -1` that silently matches a
> wrapper is now a named hazard alongside §2a rule 3's other restore traps.

Measuring that turned up a second thing, and it is worse. On the normative
baseline machine (i7-9750H, 6-core 2.6 GHz — `docs/hardware-baseline.txt`), the
**release** binary's runtime on that same fixture varies **33.6 / 39.6 / 42.4 /
54.7 / 67.4 / 81.7 / 106.3 seconds** across seven back-to-back samples with
nothing else running. **Those figures are sorted, not chronological** — the
order observed was 81.7, 106.3, 39.6, 33.6, 54.7, 67.4, 42.4. This matters: the
sorted list invites reading a monotonic ramp, which would point at thermal
throttling or progressive degradation, and the step-6 prompt did read it that
way. The real signal is unordered variance with the two slowest runs first, so
warming is not the mechanism and the cause is still unidentified. `preflightBound` derives **67,500 ms** for it (basis
`frames`). `runPreflight` passes that as `timeout` with `killSignal: "SIGKILL"`,
so two of those seven samples would have been killed and returned
`timedOut: true, report: null` — `show.load` failing on a valid package, by
coin flip. One sample landed 0.1 s under the bound. The CI fixture gate does not
catch this because it invokes the binary directly, where no bound exists; the
bound lives only in the control plane. **This is a finding against the bound
machinery, not against step 5** — the terms and the safety factor were derived
from per-frame costs that the reference fixture does not obey. Whoever opens it
owns deciding between a larger safety factor, a bound derived from measured
worst-case rather than mean, and making the decode cost itself less variable;
raising the floor alone does not fix a 3.2x spread.

Backlog line (known debt, not fixed here): the `overlay_show` fixture's
placeholder PNGs (`media/logo.png`, `media/fallback.png`) are stubs. The
render-proof suite therefore uses solid graphic fills for pixel-exact
asserts; `ticker`/`clock` glyph rasterization remains 07b's scope.

## The queue after 07 (recorded 2026-09-10, step 6 close-out)

### The prompt series as found

`agents/prompts/`, everything after 07b, one line each:

| Prompt | Subject | Lines |
|---|---|---:|
| `08-companion-mapping.md` | Companion & Stream Deck command mapping | 68 |
| `09-recording.md` | Recording output (`crates/nbe-engine`) | 86 |
| `10-streaming.md` | Streaming output (`crates/nbe-engine`) | 64 |
| `11-watchdog.md` | Performance watchdog & fallback | 59 |
| `12-benchmark.md` | OBS baseline benchmark harness (`tools/bench`) | 58 |
| `13-operator-shell.md` | Swift operator shell (`apps/nbe-macos`) | 44 |
| `14-packaging.md` | Packaging & release pipeline | 36 |
| `15-contrib-outputs.md` | Contribution outputs (WHIP) | 39 |

### The spec-version question, for the user to ratify

**Every one of those eight prompts targets "SPEC v0.3.2 (`docs/spec.v0.3.md`)"** — not 08 alone. Meanwhile `docs/spec.v0.4.md` is in the tree on `main`, and the code already implements v0.4 sentences: §7.15 house-rate reconciliation, §12.11 resources, §5.9.4's `viewItemStartFrame` and wholesale `visibleOverlays` replacement, §10.1's `showState`, and the retirement of `sequenceRef`. A prompt executed against v0.3.2 would be measured against a document the engine has already moved past.

**Correction (2026-09-10):** the sentence that stood here also claimed §16.4's `sequence.*` rows were "still live text in those headers". That was wrong — no prompt from 08 to 15 references `sequenceRef` or `sequence.*` at all. The retarget pass grepped for them and found nothing.

**What the pass actually found was worse.** The version string was the small half. Four prompts cite sections that do not mean what they say — and did not in v0.3 either, since v0.3 and v0.4 are identically numbered through these ranges:

| Prompt | Cited | Actually is | Correct target |
|---|---|---|---|
| 11 watchdog | §9.6 "GPU oversubscription fallback" | §9.6 is WHIP auth / TURN / NDI / WHEP | §10.3 watchdog, §10.5 degradation ladder, §7.14 fallback slate |
| 11 watchdog | §10.4 "structured logging" | §10.4 is the health endpoint | no standalone logging section; §10.7 item 4 is the audit log |
| 11, 12 | §20.5 "performance acceptance" | §20 is the MVP scope hard ceiling and has no subsections | AC-5, AC-11 |
| 12 benchmark | §12 "12.1 metrics, 12.2 reference manifest, 12.3 artifact publication" | §12 is Deterministic loops | AC-11 is the only home; **the reference manifest and publication rule do not exist in the spec** |
| 13 operator shell | §11 "the operator surface" | §11 is the master clock | §5.8 operator topology, §10.8 failure UI, AC-16 |
| 14 packaging | §22 "build/release requirements" | §22 is Acceptance criteria | **no build/release/packaging/notarization section exists in v0.4 at all** |
| 08 companion | §5.3 "auth/roles" | §5.3 is the WebSocket endpoint | §16.0 command authorization matrix |
| 08 companion | §21 "Companion misconfiguration risk" | §21 is Hardware tiers | §24 Risks and mitigations |

**Traced to v0.1.** In v0.1, §20/§21/§22 were Non-goals / Risks / Open questions; in v0.4 those are §23/§24/§25. The tail sections shifted by three somewhere between v0.1 and v0.2, and these prompts were written against v0.1's numbering and never re-checked through four spec revisions. Two of them cite contracts that were never written: 12's benchmark metrics and 14's build/release requirements are prompt-authored, not spec-derived, and the retargeted headers now say so.

Both are v0.5 candidates: a benchmark section and a build/release section.

**Recommendation: retarget all eight to v0.4 as a single mechanical pass, before 08 executes, rather than one-by-one at execution time.** The reasoning is that the drift is uniform and the failure mode is silent: an agent reading v0.3.2 does not know it is holding a superseded document, and Standards §2c's logic applies to prompts as much as to records. Doing it eight times at eight different moments also invites eight slightly different readings of what v0.4 changed. **This is a recommendation only — the ratification is the user's.**

### The preflight-bound finding's position

Recorded in the deferral ledger above: **recommended before Prompt 08**, not 07b's scope, and it does not gate the 07 merge — the defect predates the branch and `main` carries it today. The step-5c auditor made the same recommendation independently. Both are on record; the sequence is the user's to ratify.

### The research references — CLOSED 2026-09-10

Both documents the step-5 prompt cites are now in the tree at exactly the cited
paths, alongside a third: `docs/industry-gap-analysis-and-z-axis.md` (197 lines),
`docs/move-parity-and-virtual-set-roadmap.md` (208 lines), and
`docs/news-broadcast-features-research.md` (184 lines). The dead-unless-authored
marker is **discharged by authoring**, which was the better of the two options
the backlog offered.

One consequence worth stating: the FTB-above-DSK fallback clarification, recorded
twice as prompt-derived because its source could not be read, turns out to be
text-derived — gap analysis §3.4 states it as a normative recommendation in the
same words the implementation follows. The implementation was right and the
provenance note was conservative; both records now say so.
