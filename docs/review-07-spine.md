# Independent pass over the F2 fix — `P7-overlay-level @ 1da6139`

**Verdict: FINDINGS.** One, MEDIUM-HIGH, reproduced end to end.

The fix's diagnosis was right and its measurements reproduce on my own fixtures.
The resolver prefers release, both CI jobs build it, the bound tracks the
resolved binary, the regression pin holds, the floor and the factor behave, and
the override is still strict. My numbers for release are slightly *better* than
the round claimed (14.4 vs 16.7 ms/frame), so the constants have more headroom
than advertised.

The finding is the derivation's input. `expectedDurationFrames` is an
**optional** field — `required` is `['id', 'kind', 'source']` — and a package
that omits it derives zero frames and falls to the 60 s floor **regardless of
how much video it actually contains**. Measured end to end: a schema-valid,
air-ready package that loads in 262.9 s with zero warnings is killed at 60 s and
told its binary was killed. The previous flat 600 s constant would have accepted
it. For packages that do not declare durations, the fix is a 10× regression.

- **Head reviewed:** `1da6139d32b5a9039050e5637639fe08420dc261`
- **Confirmed equal to** `refs/heads/P7-overlay-level`
- **Checkout:** fresh clone, attached branch, `git reset --hard HEAD` restores,
  rebuilt from the clean tree before every measurement

---

## 1. CI gate lines, verbatim, first action

### Job `rust`

```
==== STEP: cargo fmt --all -- --check
(clean)
==== STEP: the unsafe exception stays in crates/nbe-decode
unsafe exception confined to crates/nbe-decode/src
==== STEP: cargo clippy --workspace --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1m 32s
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
==== STEP: cargo build -p nbe-preflight --release   (working-directory: .)
    Finished `release` profile [optimized] target(s) in 7m 17s
  [timing] cold release build: 438 s
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
# tests 48
# pass 48
# fail 0
# skipped 0
# todo 0
::endgroup::
passed=48 failed=0
  gate satisfied
```

---

## 2. Finding

### F3 — [MEDIUM-HIGH] The bound derives from an optional field, so undeclared packages fall to the floor

**`packages/control-plane/src/package.ts`** — `expectedDecodeFrames`, and the
floor it falls back to.

`expectedDecodeFrames` sums `expectedDurationFrames` and `loop.periodFrames`
over video assets. **Both are optional.** The manifest schema requires only
`['id', 'kind', 'source']`, and SPEC §12.10 says
*"`expectedDurationFrames == loop.periodFrames` **if both present**"* — the
spec's own wording assumes either may be absent.

A package that declares neither derives **0 frames**, so
`max(FLOOR, 0 × msPerFrame × 3)` is the 60 s floor no matter how much video it
contains.

**Reproduction.** Eight 150-frame 1080p clips (1 200 frames, 40 s of footage) —
the same package used to validate the fix — with the optional
`expectedDurationFrames` removed and nothing else changed:

```
  release  declaredFrames=0  bound=60000 ms  actual decode ~17280 ms  -> ok
  debug    declaredFrames=0  bound=60000 ms  actual decode ~156240 ms -> KILLED
```

End to end against the real debug binary:

```
with the derived bound:
  THREW after 60299 ms: E_PREFLIGHT_FAILED
    preflight produced no verdict within 60000 ms for …/k8_nodur
    (0 frames at 200 ms/frame x 3 safety, floored at 60000 ms); the binary was killed.

with a generous override:
  RESOLVED after 262899 ms — exit 0, 0 warning(s)
```

**Exit 0 and zero warnings.** The package is not merely valid, it is air-ready.
It is refused because it omits a field the schema does not require.

**This is a regression for that class.** The previous flat 600 000 ms would have
accepted this package (262.9 s < 600 s). The fix made undeclared packages **ten
times tighter** while making declared ones correct.

**Release does not escape it, it only moves the threshold.** At the measured
14.4 ms/frame, the 60 s floor is exhausted by ~4 170 frames — **2 minutes 19
seconds of 1080p footage with no declared durations**. That is the same order as
the original F2 finding's 2 m 34 s. For undeclared packages the original defect
survives essentially unchanged; only its cause moved from a bad constant to a
good derivation reading a field that need not be there.

**Why nothing caught it.** Every fixture in the repository declares a duration
or a loop period — `dress_show` 3 of 3, `valid_show_v0.3` 1 of 1, `valid_show`
2 of 2 — and the round's own tests construct manifests with
`expectedDurationFrames` set. The blind spot is structural, not careless.

**The coder's flagged case is the milder one.** They named "declares 100 frames,
ships 10 000" and judged it "wrong error, right instinct" on the belief that
*"a correct run would have failed validation anyway."* It would not: a declared
/ decoded mismatch is a **warning** (`main.rs`: `report.push_warning("duration:
asset … declares {expected} frames, decoded {}")`), exit 1, which `loadPackage`
explicitly accepts. So even the case they described refuses a loadable package —
and omitting the field entirely is worse, because it needs no lie at all.

**Fix shape.** The bound needs an input that cannot be absent. The obvious one is
already on disk: total bytes of the referenced media, which `expectedDecodeFrames`
could fall back to when no duration is declared — bytes-per-second is far less
stable than frames, but a floor derived from *something about this package* beats
a constant that ignores it. Alternatively keep frames as the primary input and
raise the fallback floor to cover the largest package that can be described
without declarations, stated in writing. Either way the case to test is the
manifest that declares nothing, because that is the one the schema permits and
the fixtures never exercise.

---

## 3. Claim-by-claim verification

| # | Claim | How verified | Result |
|---|---|---|---|
| 1a | `preflightBin()` prefers release, falls back to debug | My own probe over all four states | **Holds.** debug only → debug; both → release; release only → release; neither → debug fallback; `NBE_PREFLIGHT_BIN` overrides all |
| 1b | Both CI jobs that shell preflight build release | Read `ci.yml` | **Holds** — lines 103 and 195, both `--release` |
| 1c | Deleting the preference fails a named test | Ran it | **Holds** — `not ok - preflightBin prefers the release build` |
| 2a | Linearity, 130 ms/frame debug and ~16.7 release | Re-measured on my own fixtures, 300/600/1200 frames | **Holds, and better.** debug 131.4 / 130.5 / 130.2; release 15.2 / 14.6 / **14.4**. The release constant of 25 has 1.7× headroom before the safety factor |
| 2b | The bound tracks the resolved binary | `preflightBound` under each profile, 4 600 frames | **Holds.** release 345 000 ms vs 66 240 ms measured cost — **5.2× margin**; debug 2 760 000 ms vs 598 920 ms — **4.6× margin** |
| 2c | Regression pin: the old 600 s constant refused this package | Same probe | **Holds** — debug bound `2 760 000 > 600 000` |
| 3 | The lying manifest is "wrong error, right instinct" | Read the duration check; then constructed the stronger case | **FAILS — see F3.** The mismatch is a *warning*, not a validation failure, so even the described case refuses a loadable package; and omitting the optional field is worse |
| 4a | Zero declared frames → the 60 s floor | Probe | **Holds** — and that is precisely the defect in F3 |
| 4b | Loop periods count; images do not | Probe | **Holds** — `periodFrames: 900` → 900; `max(declared, period)` → 900; an image declaring 9 999 → 0 |
| 4c | Factor at ⅒ fails its test | Ran it | **Holds** — `a debug binary is measured at 130 ms/frame, rounded up` / `20 !== 200` |
| 4d | Override wins, strict, `"0"` refused | Seven malformed values | **Holds** — no leaks; `"0"`, `"600abc"`, `"-5"`, `"1e4"`, `" 6000"`, `""`, `"5_000"` all take the derived value; `"1234"` wins with `derived=false` |
| 5a | All accumulated anchors bite | 13 v0.4 + 4 spine + 3 this round | **20/20 bite.** r14 at compile time (`E0451`), the rest as test failures |
| 5b | Wedged-binary suite: exit 1 with a summary | Ran it | **Holds** — `exit=1`, 3 named failures, `# tests 16 / # pass 13 / # fail 3` |

## 4. Falsification rows

| Row | Observed |
|---|---|
| Resolver preference deleted | 1 failed — `preflightBin prefers the release build` |
| Per-frame factor removed | 1 failed — every package collapses to the floor |
| Factor at ⅒ of measured | 1 failed — `20 !== 200` |
| §12.4 frame-cap refusal | 1 failed |
| `audioDuration` refusal | 1 failed |
| Budget consulted for `auto` | 1 failed |
| §12.5 mandatory-`vram` | 1 failed |
| Planner honours `declared_format` | 1 failed |
| Image charged its own size | 4 failed |
| `/rate` divisor | 1 failed |
| `sequenceRef` in the schema | 1 failed |
| Shared resolution chain | 1 failed |
| Duplicate spec numbering | 1 failed |
| Engine builds its own budget | compile error `E0451` |
| Engine ignores `vramBudgetMib` | 1 failed |
| Constant drifts from §12.4 | 1 failed |
| Per-block publish-and-reset (R2) | 1 failed |
| §12.4's 900-frame conjunct | 1 failed |
| `show.load` on the async path (R5) | 1 failed |
| Pump drains on the deadline only (R6) | 1 failed |

## 5. The standing question

> Bounded everywhere was last round's verdict; this round's claim is **correctly
> bounded everywhere**.

**Not yet — F3 is the gap, and it is precisely a correctness gap rather than a
boundedness one.** Every command path still reaches a terminal state; one class
of package reaches the wrong one. The three paths are unchanged from last round:
`show.load` and `show.preflight` share the derived bound, `show.stop` is bounded
by `showStopGraceMs`, and nothing else on a handler awaits without a deadline.

What changed is that the bound is now *derived*, and a derivation is only as
good as its input. Reading an optional field made the bound correct for the
packages that fill it in and worse than the constant for the packages that do
not.

## 6. Observation — CI has no cache

`ci.yml` contains no `actions/cache`, no `Swatinem/rust-cache`, nothing. **Every
run is cold.** Measured on this machine, from an empty `target/release`:

| | |
|---|---|
| Cold release build of `nbe-preflight` | **438 s** (the coder measured 302 s) |
| Warm | 0.57 s |
| `dress_show` preflight, debug → release | 30.05 s → 3.45 s |

So the release build costs ~7 minutes on each of the two jobs that shell
preflight, every run, to save ~26 s per package load. On the **rehearsal** job
that is the point — the 46 s load is what makes step 3 flaky. On the
**control-plane** job it is a net loss in wall clock unless a cache is added.
Recorded as an observation; the disposition is not mine.

## 7. Self-check

- Reported SHA `1da6139d32b5a9039050e5637639fe08420dc261` re-verified equal to
  `refs/heads/P7-overlay-level` after all probes.
- `git reset --hard HEAD` throughout; rebuilt from the clean tree before every
  measurement; tracked tree clean at the end.
- **Two self-inflicted errors, both caught and both worth recording.** My
  end-to-end probe deleted `NBE_PREFLIGHT_TIMEOUT_MS` at the top, so my first
  "prove the package is legitimate" run tested the same 60 s bound twice and
  appeared to confirm a hang; re-run correctly it resolved in 262.9 s, which is
  the measurement the finding rests on. And my contention experiment spawned
  12–24 busy loops that outlived the command's timeout, leaving the machine at a
  load average of 23 — I killed them and waited for the load to settle before
  re-baselining (release re-measured at 15.2 ms/frame, matching). I abandoned the
  contention measurement rather than report a number taken from a machine no CI
  runner resembles.
- The three consequential probes (release/debug linearity, the bound under each
  profile, the undeclared-package end-to-end) re-run against the confirmed SHA
  after a rebuild.
