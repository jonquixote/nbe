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

**Disposition: Prompt 07, alongside R2 and R5.** Numbering continues the R-series filed in
`docs/review-midpoint-report.md` §3.4–§3.8 and §11.2 (R1–R6); that report is a sealed CLEAN
verdict and is not amended to hold this.

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

## 08 — Companion mapping (elevated to a normative requirement)

Per the v0.4 outline §6, 08 is no longer "wire up a Stream Deck." It builds an **Input Intent schema** — a mapping layer that is *data, not code* — from physical intents (Companion button, MIDI note, keyboard chord) to semantic §16 commands, with per-device profiles as user-editable documents. The §16 command surface with token auth and audit is already the device-independent core (`docs/portability.md`, known-good boundary 1), so 08 adds a layer above it and must not add a second command surface beside it. The proof of generality is normative: a keyboard-shortcut adapter ships in the same prompt and must work with **zero** changes to the core. The Input Intent schema is a wire-level contract and takes normative spec text at 08's moment. Target hardware: StreamDeck XL via Companion.

## 09 — Recording

**Owns `marker.add` → recording chapter (§16.11)**, assigned by `[RI-5]` — 09's current doc does not mention it, and its upgrade pass must. Inherits two dormant deferrals that its own benchmark is the trigger for: zero-copy IOSurface→Metal (re-defer *with numbers*, not with prose) and the display surface. §0.1 assumption 14 fixes fragmented MP4 as the crash-safe default. 09 should also carry `[RI-8]`'s pinned residency policy into its own resource accounting: **unload-at-next-load**, so a stop→start recovery does not pay the 46 s reload measured in the report §3.2.

## 10 — Streaming

Inherits the guest-link JWT / `jti` revocation work (§10.7 #1) assigned by `[RI-5]`, and the TURN credential vending shape (§5.1 #11, §9.6.2) whose response has a schema but no derivation rule. WHEP preview (AC-20) is explicitly **post-v1** and not 10's scope — it waits for a WebRTC stack to exist. The mix-minus guarantee 06 built structurally (§8.6, `render_guest_return` has no path reading a guest's own bus) is 10's to preserve when real guests replace test tones.

## 11 — Watchdog

The watchdog itself exists and is gated (pass 4 confirmed deadline accounting and fallback trip both fail correctly when deleted). What 11 must now add is **the automation engine runtime** (§13, AC-25), assigned by `[RI-5]`: triggers, the once-per-frame limit, runtime cycle suppression, and audit logging of every automation action. `automation.hold` exists from Prompt 02; the engine behind it does not. 11 also inherits **F3's fix** as context — the fix round adds a `fail_view` seam, so §10.3's engagement path finally has production coverage that 11's work must keep.

## 12 — Benchmark

**Reframed by H1.** The reference machine is Intel with discrete AMD graphics; the spec declares Apple Silicon the primary target. Every performance number to date — quality-profile capping, the 8 ms render budget, the degradation ladder's thresholds — is unvalidated on the declared target. 12 must state which architecture each measurement was taken on, and AC-5's 30-minute zero-drop soak must not be reported as met on an architecture the spec does not target. S1 is 12's problem too: `renderGpuTimeMs` is always 0, and it is the ladder's input, so the ladder is currently deciding on a constant. A benchmark prompt that inherits a stubbed GPU timer measures nothing.

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
| **The dress-rehearsal CI job is `continue-on-error`** — still open, for a new reason | `.github/workflows/ci.yml:322` (the citation read `:248` until 2026-09-11; that line is now `echo "::endgroup::"` — the number rotted under edits in the very commit that wrote it, which is the argument for citing by name as well as line). Note there are **two** `continue-on-error: true` in the file and only one is this row's: `:178` belongs to the control-plane job's preflight-timing diagnostic step and stays deliberately non-failing. The job reports **pass regardless of step failures**, so its green is not evidence — on any PR, including the two that cited it. Three of its twelve steps fail today by design (R4, R5, R2), which is why the flag is there. **Either it gates or it is marked observational**; a check that always reports green teaches reviewers to read it as a result. Raised by the PR #11 two-key pass, 2026-09-10. **Work order DRESS closed R5, R4 and R2 (12/12 three times on the normative machine, each falsified) and attempted the promotion — which failed on CI and was reverted.** The row stays open because the reason changed rather than vanished: the gate asserts zero dropped frames and zero audio underruns, which are reference-hardware claims, and the macos-14 runner measured 83 then 120 underruns on 3 arm64 cores. Promotion now needs the gate split so composition claims run everywhere and threshold claims run where they mean something. See the DRESS entry under 07. |
| **Preflight bound vs measured decode cost [HIGH]** | **Recommended before Prompt 08.** Not 07b's scope; does not gate the 07 merge — the defect predates this branch and `main` carries it today. See the step-5c backlog entry under 07 for the evidence. |

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
