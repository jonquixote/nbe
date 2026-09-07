# Independent pass over the 07 spine — `P7-overlay-level @ 9bd18f9`

**Verdict: FINDINGS.** One, MEDIUM, reproduced end to end.

Six of the seven claims survive adversarial probing intact, several of them
under attacks the spine's own tests do not make. The seventh — the inherited
hang — is half-fixed: the failure is now *named*, but the suite still does not
exit, so CI still burns its wall clock. The root cause is one line below where
the fix was applied, and it is in production code rather than the test harness.

- **Head reviewed:** `9bd18f9b381dfec2ab11038d476cf18629136996`
- **Confirmed equal to** `refs/heads/P7-overlay-level`; base `76ff9f3` (`main`)
- **Checkout:** fresh clone, attached branch, tracked tree clean (0 modified)
  after every mutation/restore cycle, rebuilt from the clean tree before every
  measurement

---

## 1. CI gate lines, verbatim, first action

### Job `rust`

```
==== STEP: cargo fmt --all -- --check
(clean)
==== STEP: the unsafe exception stays in crates/nbe-decode
unsafe exception confined to crates/nbe-decode/src
==== STEP: cargo clippy --workspace --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1m 25s
==== STEP: cargo test --workspace (summary echoed for the audit trail)
::group::Rust test summary
TOTAL: 173 passed, 0 failed, 0 ignored
::endgroup::
==== STEP: GPU/render tests actually ran (Prompt 04)
prompt04 tests that ran: 15
==== STEP: v0.3 fixture passes
preflight OK: air-ready. report at tests/fixtures/valid_show_v0.3/preflight_report.json
  airReady: true
==== STEP: v0.2 fixture is rejected (migration gate)
preflight FAILED (3 error(s), 0 warning(s)): report at tests/fixtures/valid_show/preflight_report.json
  exit code 2; migrationRequired + nbe-migrate present
```

### Job `control-plane`

```
==== STEP: cargo build -p nbe-preflight   (working-directory: .)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.32s
==== STEP: npm ci
found 0 vulnerabilities
==== STEP: npx tsc --noEmit
(clean)
==== STEP: generated manifest types match the schema
  git diff --exit-code src/generated -> exit 0
==== STEP: vocabulary discipline (program/layer only in the alias table)
  no violations
==== STEP: tests pass and actually run (summary echoed for the audit trail)
::group::Control-plane test summary
# tests 45
# pass 45
# fail 0
# skipped 0
# todo 0
::endgroup::
passed=45 failed=0
  gate satisfied
```

Green CI is the floor, and the floor is met. The finding below is something CI
cannot see — because the condition that triggers it is the one where CI stops
reporting at all.

---

## 2. Finding

### F1 — [MEDIUM] The inherited hang is annotated, not fixed

**`packages/control-plane/src/package.ts:54`**, with the symptom in
`packages/control-plane/src/render-channel.test.ts`.

Step 4c's deliverable is stated twice and identically: *"the inherited hang
fails instead of hanging"* (`07-overlay-level.md`, Definition of Done 3) and
*"Remove the binary → the suite **fails** rather than hangs"* (its falsification
row). The done message claims *"three named failures, process exits."*

**Reproduction.** A preflight binary that exists and never answers
(`#!/bin/sh` / `sleep 100000`), which is the condition the finding describes:

```
$ NBE_PREFLIGHT_BIN=…/hang-pf timeout 90 npx tsx --test src/render-channel.test.ts
  exit=124
  named failures: 3   summary line: 0
```

The three named failures are real and are an improvement — each says
`no response to "show.load" within 15000 ms`. But **`exit=124` means the wall
clock killed it**, and there is no `# pass` / `# fail` summary. The last line of
output is a test completing normally; after that the process emits nothing and
never exits. CI still hits its limit, exactly as before; it just prints more on
the way there.

**Root cause, proven separately.** `send()`'s deadline bounds the *test's* wait.
It does not bound the *subprocess*. `runPreflight` calls
`execFileP(preflightBin(), args, { cwd })` with no `timeout` option, no
`killSignal`, and no `AbortSignal` — a grep for all three across `package.ts`
returns nothing. A single un-timed `execFile` child holds Node's event loop
open on its own:

```js
const execFileP = promisify(execFile);
execFileP(hangingBinary, []).catch(() => {});
console.log("started child; main work done, nothing left to await");
```

```
started child; main work done, nothing left to await
  exit=124 (the child kept the loop alive)
```

Nothing in the test file can close that, because the handle belongs to
production code.

**Why this is more than a harness bug.** `runPreflight` is the path
`show.load` takes. With no timeout, a wedged preflight binary means `show.load`
never returns: the operator's command does not answer, the §16 response never
arrives, and the audit log records an accepted command with no outcome. The
measured 46 s decode already shows this call can be slow; nothing bounds how
slow. The test-harness symptom is the same defect seen from outside.

**Fix shape.** A `timeout` (and `killSignal`) on the `execFileP` options in
`runPreflight`, chosen against the measured worst case rather than guessed, so
a wedged binary becomes a named `E_PREFLIGHT_FAILED` instead of an unanswered
command. The `send()` deadline stays — it is a good guard, and it is what made
this diagnosable — but it is the second line of defence, not the first.

---

## 3. Claim-by-claim verification

| # | Claim | How verified | Result |
|---|---|---|---|
| 1 | Three audio tests re-pointed, no assertion weakened | Diffed `9bd18f9^..9bd18f9` on `prompt06.rs`, filtered for added/removed assertion lines | **Holds.** *No assertion line was added or removed* in any of the three. Only setup changed: a window seam and cycle counts. `loud > -3.0`, `quiet < -60.0`, `music < -30.0`, `(mic − −6.0).abs() < 1.5` all unchanged |
| 1b | `set_meter_window` is a test seam, not a production escape | Grepped every caller | **Holds.** Five callers, all in `prompt06.rs`; the only definition is the method itself. Production takes `DEFAULT_METER_WINDOW_MS = 1000` |
| 1c | The new transient test fails under old publish-and-reset | Restored per-block publish-and-reset, ran it | **FAILED as required** — `the transient was -6 dBFS and the window reported -120.0 dBFS` |
| 2a | The starvation test measures what it claims | Read it; it asserts the load took >20 ms before concluding anything | **Holds.** It refuses to draw a conclusion from a fixture that decodes too fast to detect starvation — the failure message says so and tells you to add clips |
| 2b | `show.load` is on the blocking pool at every call site | Grepped `on_show_load` and `load_video_asset` | **Holds.** One call site, inside `spawn_blocking`; `load_video_asset` is reached only from within it |
| 2c | The clock test passes on unmodified parent code | Extracted the clock test alone, checked out `76ff9f3` sources under it, ran it | **Holds** — `show_start_starts_the_clock_at_once ... ok` against parent sources. The clock was never the defect, as claimed |
| 3a | The R6 test runs at production cadence | Read the constant | **Holds.** `const INTERVAL_MS: u64 = 1000; // production cadence`, used for the config, the deadline, and the assertion bound |
| 3b | The pump loses no ack and starves no tick under burst | My own probe: 40 back-to-back directives at a 1000 ms interval | **Holds under attack.** 40/40 acks, in order, **first ack at 2.05 ms**, 3 telemetry ticks in 2.5 s. No lost wakeup (`Notify`'s stored permit covers a push between drain and select), no tick starvation, no catch-up burst |
| 4 | §12.4's cap inclusive at 900, budget irrelevant, reasons distinguish | Ran `plan()` at 899/900/901 on a 320×180 frame | **Holds.** Budget holds 3106 frames, so the cap alone decides. 899 → Vram, 900 → Vram, 901 → `Streaming … exceeds SPEC §12.4's absolute short-loop cap of 900 frames`. A budget refusal reads `… exceeds the 256 MiB effective budget (86 frames max)` — the two are distinguishable at a glance |
| 5 | The pass-4 bypass no longer compiles | Reintroduced `let mut b = …; b.per_loop_mib = 1024;` in `nbe-engine` | **Holds** — `error[E0616]: field per_loop_mib of struct CacheBudget is private`. The compiler is the gate now |
| 5b | `spec_budgets.rs`'s narrowed role is stated and real | Read the comment; ran anchor r14 | **Holds.** The comment says privacy handles other crates and the grep keeps in-crate proliferation. r14 is now refused at compile time (`E0451`) — a *stronger* outcome than the test failure it used to produce |
| 6a | Unattributable counter moves; attributable does not double-count | The spine tests only the unattributable case, so I wrote the other: a corrupt asset an Item **does** reference | **Holds.** `total=1, unattributable=0, itemEvents=1`. Counted once, not once per affected Item, and the gap counter is not inflated by a failure that has somewhere to go |
| 6b | The hang: three named failures, process exits | Hanging preflight binary | **FAILS — see F1.** Three named failures, but `exit=124` and no summary line |
| 7 | The sixteen accumulated anchors still apply | Ran all sixteen on this head | **16/16 bite** — full table below |

## 4. Falsification rows

| # | Behaviour deleted | Observed |
|---|---|---|
| 1 | §12.4 frame-cap refusal | 1 test FAILED |
| 2 | `audioDuration` refusal | 1 test FAILED |
| 3 | Budget consulted for `auto` | 1 test FAILED |
| 4 | §12.5 mandatory-`vram` failure | 1 test FAILED |
| 5 | Planner honours `declared_format` | 1 test FAILED |
| 6 | Image charged its own size | 4 tests FAILED |
| 7 | `/rate` divisor in the audio sum | 1 test FAILED |
| 8 | `parseHouseRate` → `Number(raw ?? 30)` | 1 test FAILED |
| 9 | `houseRate` wiring in `server.ts` | 1 test FAILED |
| 10 | `sequenceRef` back in the schema enum | 1 test FAILED |
| 11 | `manifestIdentity` → constant `"0.3"` | 1 test FAILED |
| 12 | Shared chain → hardcoded 1920×1080 | 1 test FAILED |
| 13 | Duplicate spec item number | 1 test FAILED |
| 14 | Engine builds its own 1024/4096 budget | **compile error `E0451`** (was a test failure; privacy moved the gate earlier), and the `spec_budgets` lint FAILED |
| 15 | Engine ignores the declared `vramBudgetMib` | 1 test FAILED |
| 16 | `DEFAULT_PER_LOOP_MIB` drifts from §12.4 | 1 test FAILED |
| — | Per-block publish-and-reset restored (this round) | `a_transient_shorter_than_the_window_still_reaches_the_meter` FAILED |
| — | `show.load` back on the async path (this round) | `a_package_load_does_not_starve_the_rest_of_the_engine` FAILED |
| — | Pump drains only on the deadline (this round) | `an_ack_does_not_wait_for_the_telemetry_tick` FAILED |

## 5. Observations — reproduced, deliberately not filed

1. **`DEFAULT_METER_WINDOW_MS` and `telemetry_interval_ms` are two constants for
   one number.** The commit's stated intent is *"one telemetry interval's worth
   of blocks, so a published meter covers exactly the interval a tick reports"* —
   true only while both read 1000. `AudioDriver` is constructed with
   `(state, sink, house_rate)` and has no access to `EngineConfig`, so it cannot
   derive the window from the interval. Not filed because the interval is
   hardcoded to 1000 in `main.rs` and in `EngineConfig::default()` with no env
   override, so no user can create the divergence. It is the same shape as the
   §12.4 budget defect the v0.4 cycle spent three rounds on, one step before it
   becomes real. Whoever makes the telemetry interval configurable owns it.

2. **The meter window rolls on audio-block count, not wall clock.** If the
   driver's cycle rate drifts, window and tick slide relative to each other, so
   a tick reports the most recently *completed* window rather than the interval
   it covers. Acceptable for a peak meter; worth knowing before anyone asserts
   tighter timing on `busPeakDbfs`.

3. **§10.1 still has no decode-failure field.** The spine flagged this rather
   than inventing wire surface, which is the right call under the constraints.
   The counters exist engine-side and are gated; nothing surfaces them yet.

## 6. Self-check

- Reported SHA `9bd18f9b381dfec2ab11038d476cf18629136996` re-verified equal to
  `refs/heads/P7-overlay-level` after all probes.
- Attached to a branch, not detached; tracked tree clean after every cycle.
- **Rebuilt from the clean tree before every measurement** (§2a). One near-miss
  of my own, worth recording alongside the stale-binary rule it rhymes with:
  `git checkout <commit> -- <path>` **stages** what it restores, so a following
  `git checkout -- .` restores *from the index* and silently keeps the other
  commit's sources. `git reset --hard HEAD` is the restore; `git status` said
  `7` modified when I expected `0`, which is the only reason I caught it.
- Two probes of mine were wrong before they were right — the burst probe matched
  `"telemetry"` where the wire says `"engineTelemetry"`, and sent a
  discontiguous `seq` that the connection gate correctly dropped. Both produced
  a plausible-looking "0 acks, 0 ticks" that would have been a false HIGH. Fixed
  and re-run; the corrected result is in claim 3b.
- The three consequential probes (the hang, the `execFile` loop-holding proof,
  claim 1's falsifier) re-run against the confirmed SHA after a rebuild — all
  three reproduce identically.
