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

### Finding R10 — a stream drain missed its 5 s bound once on CI (recorded 2026-09-25, PR #33)

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
the workspace test step. That capture is owed and not made here (this entry is
records only).

**THE REFUTATION CONDITION, written before the next run rather than after it.**
Either of these reopens R10 as a defect in the transport's drain or in the test
server, not a load flake:
- **a second sighting with the load recorded and under the 3.0 ceiling** — a
  soak iteration qualifies, because the soak checks quiescence first and runs
  `prompt10_rtmp` whole; a CI sighting qualifies only once the load capture
  above exists;
- **the drain approaching its bound on a quiescent run** — say, any quiescent
  run of this test above 2.5 s, against about 0.4 s today.

Until then it rides the watch list (`docs/soak-protocol.md` §5), in R7's shape.
The re-run's green is not evidence of absence; it is the reason the entry exists.

**The widened class, linked.** PR #33's keyless fix (`40e96e6`) means the
hardware-gated stream tests now open real publishers and stream threads. Their
stops are bounded by the same 500 ms `STREAM_THREAD_STOP_TIMEOUT` as
`close_error_seam_fails_loudly_with_the_network_token`'s flake — **Finding R11**,
below. ~~…'s load flake, recorded under § 11's "SPEC v0.4.6" entry. So the stream
suites now carry two wall-clock bounds in R9's class, this drain and that
teardown, and a loaded run can trip either.~~ *Corrected 2026-09-25, a day after
it was written (§2c): R11's second sighting was under the quiescence ceiling, so
the 500 ms flake does NOT track load and is not R9's class.* What links the two
is narrower and true: **both are wall-clock bounds on another thread's
progress** — this drain on the test server's reader, that teardown on the stream
thread's exit. A run that trips either is a sighting to record, not a load
excuse.

### Finding R11 — the stream thread's 500 ms teardown wait expired twice, once under the ceiling (recorded 2026-09-25, PR #33)

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
on it. Step 2's own comment had claimed it did this all along. Falsified:
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
| AC-24 persistence | Ticker survives complex MOVE untouched | No as written (no move). Honest subset: DSK-above-transition + take-independence for cut/mix (solid stand-ins, hand-built `Transition`, pure-fn unit) | Partial (cut/mix only) | GAP-12 |
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
- **QUEUED (small work order): the stream thread's teardown wait, rebound by
  event (Finding R11).** `stop_thread` already waits on the thread's exit event
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
| **B4** | §13.4 transitive cycle rejection needs a command → trigger effect table the spec lacks | draft the table as an UNRATIFIED candidate |
| **B5** | AC-25 #2's "pending actions" — rules have no delay | fired-but-not-dispatched within the current frame |

~~**Prompt 11 is BLOCKED on C1 and B1–B5.**~~ *Unblocked 2026-09-25 — the user spoke all six; see the next entry.* They are the user's words, landed the way SPEC-REV landed Prompt 10's four blockers (v0.4.5), before any executor starts. One finding to settle during execution, not assumed: §10.3 says "more than 1 frame", the built watchdog trips when accumulated `ceil(late / budget)` exceeds 2.

### SPEC v0.4.6 — Prompt 11's six decisions, spoken 2026-09-25

The user spoke Prompt 11's six decisions on **2026-09-25** (UTC and local −0700
agree on the date). SPEC-REV-2 landed them the v0.4.5 way — the words predate
the text, so there was no drafting phase, and each ratified row landed with its
mechanism and had its guard run at its landing commit. Branch `spec-rev-v046`.

| # | The user's word | Where it landed |
|---|---|---|
| **C1** | One prompt, two gated work units | **Recorded for the executor** — `agents/prompts/11-watchdog.md` §1 and §3 |
| **B1** | The engine level-crossing event; its mechanism is Prompt 11's to build and it ships there as a candidate | **Candidate, home: Prompt 11's feature PR** — marked UNRATIFIED with its guards there, ratified by the user separately. No v0.4.6 row |
| **B2** | Ratify `streamTransportState` | **Landed, v0.4.6 row 2** — §10.1 field, note and §10.1.1 ownership; engine, protocol, control-plane schema and `buildTick`; token, completeness, readability and redial-on-the-wire guards; the mirror fixture samples `"reconnecting"`; the soak captures distinct values. Landing `f15b617`, record `9d1dfed` |
| **B3** | Control-plane-side `mediaStart` | **Recorded for the executor** — the take applied. §13.4.1's `mediaStart` column uses it |
| **B4** | The command → trigger effect table, as a candidate | **Candidate, drafted UNRATIFIED in SPEC §13.4.1** (v0.4.6 row 4, `69a2b54`): all 55 §16 commands with citations. Its mechanism — WU5's transitive check — ships with Prompt 11's feature PR, and the table is ratified separately |
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

## 12 — Benchmark

**Reframed by H1.** The reference machine is Intel with discrete AMD graphics; ~~the spec declares Apple Silicon the primary target~~ — that was v0.3; SPEC v0.4 §0.1 assumption 2 names the Intel reference machine and says Apple Silicon "is welcome and supported, but MUST NOT be assumed" (corrected 2026-09-25, §2c). Every performance number to date — quality-profile capping, the 8 ms render budget, the degradation ladder's thresholds — is unvalidated on the declared target. 12 must state which architecture each measurement was taken on, and AC-5's 30-minute zero-drop soak must not be reported as met on an architecture the spec does not target. S1 is 12's problem too: `renderGpuTimeMs` is always 0, and it is the ladder's input, so the ladder is currently deciding on a constant. A benchmark prompt that inherits a stubbed GPU timer measures nothing.

## 13 — Operator shell

Inherits the display-surface deferral (04 → 09 → here in practice) and S2: **`showState` is absent from the §10.1 telemetry tick**, so a shell subscribing to telemetry alone cannot say whether the show is running. Either the shell also tracks `stateChange` frames, or v0.4 adds the field — the outline records it as a candidate. 13 is also the first consumer that will notice R2's metering strobe: bus meters sampled once a second from a 33 ms window are unusable in an operator UI, so R2's fix is a prerequisite rather than a nicety.

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
