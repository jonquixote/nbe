# Independent pass over the 07 spine — `P7-overlay-level @ dbb46d7`

**Verdict: FINDINGS.** One, MEDIUM, measured rather than argued.

F1 is genuinely fixed at the layer that owns the handle, and it survives every
attack in the brief: the child is killed, the suite exits with a summary, the
override parses strictly, the two layers fail independently, and — the sharpest
thing I could think to try — a stale `preflight_report.json` from an earlier
successful run cannot be mistaken for a timed-out run's verdict.

The finding is the constant. The brief asked whether a legitimate package can
exceed 600 s. One can, and not an exotic one: **at a measured 130 ms per 1080p
frame in the debug build the control plane actually resolves, the bound is
reached at 2 minutes 34 seconds of footage.** A package that is merely slow is
killed and told it is wedged.

- **Head reviewed:** `dbb46d7c37af05da53109581182ceed59d0b3879`
- **Confirmed equal to** `refs/heads/P7-overlay-level`
- **Checkout:** fresh clone, attached branch, tracked tree clean after every
  cycle, restored with `git reset --hard HEAD` and rebuilt before every
  measurement (§2a, eighth variant)

---

## 1. CI gate lines, verbatim, first action

### Job `rust`

```
==== STEP: cargo fmt --all -- --check
(clean)
==== STEP: the unsafe exception stays in crates/nbe-decode
unsafe exception confined to crates/nbe-decode/src
==== STEP: cargo clippy --workspace --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1m 24s
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
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.35s
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
# tests 46
# pass 46
# fail 0
# skipped 0
# todo 0
::endgroup::
passed=46 failed=0
  gate satisfied
```

---

## 2. Finding

### F2 — [MEDIUM] The 600 s bound is crossed by a legitimate package

**`packages/control-plane/src/package.ts:44`** (`DEFAULT_PREFLIGHT_TIMEOUT_MS`).

The bound's own doc comment sets the test: *"The bound is for **wedged, not
slow**… Ten minutes is far outside any measured run."* The measured runs it was
sized against are a five-second fixture. Real packages are not five seconds.

**Measurement.** Copies of `tests/fixtures/dress_show/media/A1.mp4` (150 frames,
1080p) in one package, timed with the branch's own debug binary:

| 1080p frames | footage | preflight | per frame |
|---:|---:|---:|---:|
| 300 | 10.0 s | 39.16 s | 131 ms |
| 600 | 20.0 s | 77.95 s | 130 ms |
| 1200 | 40.0 s | 156.35 s | 130 ms |
| 2400 | 80.0 s | 311.43 s | 130 ms |

Four points across an 8× span, no deviation — the cost is linear in frames at
**130 ms/frame**. Re-measured from the confirmed clean tree: 136 and 132 ms.

**Therefore 600 000 ms is reached at ~4 600 frames — 2 minutes 34 seconds of
1080p footage.** A rundown with ten fifteen-second clips crosses it. The
extrapolation is a 2× step beyond the last measured point, on a fit with no
observed curvature.

**What the operator sees.** `show.load` fails with
`E_PREFLIGHT_FAILED: preflight did not answer within 600000 ms … It is wedged,
not slow — raise NBE_PREFLIGHT_TIMEOUT_MS only if a real run legitimately takes
longer`. The package was legitimately taking longer. The message asserts the
opposite of the truth and points the operator away from the actual remedy.

**Why the debug build is the right frame.** `preflightBin()` resolves
`target/debug/nbe-preflight` (`package.ts:27`, `:33`), and CI's control-plane
job runs `cargo build -p nbe-preflight` with no `--release`. The debug binary is
the one on the path today.

**The release build changes the arithmetic by 8×**, measured on the same
1200-frame package: **20.00 s, 16.7 ms/frame**, which puts the bound at roughly
twenty minutes of footage. That is comfortably outside any plausible package,
which makes it the more interesting half of the fix: the As-Built Ledger already
proposed *"run preflight's decode in release even from a debug build"* for the
46 s `show.load` problem, and this is the same root cost with a second
consequence. Fixing the build makes the constant right; raising the constant
alone leaves loads taking ten minutes.

**Fix shape (either, or both).** Resolve a release-built preflight for the
control plane's own invocation, so the bound is sized against 16.7 ms/frame; or
size the default against the debug cost, which needs tens of minutes to cover a
real bulletin and makes "wedged" nearly meaningless. The first is better, and it
also closes the 46 s complaint the rehearsal has carried since the midpoint
review. Whatever is chosen, the doc comment should name the measurement it was
derived from rather than a fixture.

---

## 3. Claim-by-claim verification

| # | Claim | How verified | Result |
|---|---|---|---|
| 1a | The timeout fires and SIGKILL is fatal to the child | Direct `execFile` probe with `timeout: 1500, killSignal: "SIGKILL"` | **Holds** — `rejected after 1506 ms  killed=true signal=SIGKILL code=null`, **0 surviving processes** when the child is the binary itself |
| 1b | No orphaned process survives | Same probe with a shell wrapper that *forks* rather than `exec`s | The grandchild survives — but that is a property of killing any shell wrapper, and production `execFile`s the binary path directly with no shell. **Not a finding**; noted because my own earlier probes leaked eight `sleep` processes this way |
| 1c | `show.load` fails by name, nothing half-loaded, audit terminal | My own probe over a real socket, dumping the audit record | **Holds** — `status=error code=E_PREFLIGHT_FAILED`; `state.pkg=null showState=UNLOADED preflightPassed=false`; audit `{"kind":"command","command":"show.load","outcome":"rejected","errorCode":"E_PREFLIGHT_FAILED","svBefore":0,"svAfter":0}` |
| 1d | Suite level: exit code with a summary, not 124 | Wedged binary against `render-channel.test.ts` and `v04.test.ts` | **Holds** — `exit=1`, `# tests 16 / # pass 13 / # fail 3`. Was `exit=124` with zero summary lines |
| 2 | The override parses strictly | 13 inputs through `preflightTimeoutMs()` | **Holds.** `"600abc"`, `""`, `"-5"`, `"1e4"`, `" 600000"`, `"5_000"`, `"٦٠٠"`, `"9007199254740993"` → default. `"+5000"`→5000, `"0600"`→600 per the documented grammar. **`"0"` → default**, which is the important one: Node reads `timeout: 0` as *no timeout*, so the one value that would silently disable the bound is the one it refuses. Behaviour matches the comment exactly |
| 3 | The two layers fail independently | Ran both mutations myself | **Holds.** No timeout → `exit=124`, 1 named failure, **0 summary lines** (hangs). Timeout kept, mapping deleted → `exit=1`, named test fails on *"the failure must name the bound it exceeded, not just fail"*. Neither substitutes for the other |
| 4 | No legitimate package exceeds 600 s | Measured, four points, then release | **FAILS — see F2.** 2m34s of 1080p footage crosses it in the debug build that ships |
| 5a | Near the bound succeeds; past it fails | Stub preflight sleeping 1 s vs a 3 000 ms bound, then 3 s vs 1 500 ms | **Holds** — `1394 ms, timedOut=false, exit 0, report present` / `1504 ms, timedOut=true, exit 124, report null`. No false positive under the bound |
| 5b | All-or-nothing on the failure path | Ran a **successful** preflight first so an air-ready report sat on disk, then wedged the binary | **Holds, and this is the sharpest guard in the change.** `report=null` on timeout and `loadPackage` throws by name — a stale verdict from an earlier run cannot be adopted as this one's |
| 6 | All accumulated anchors bite | 13 Rust + 3 TypeScript v0.4 rows, plus the spine round's five | **21/21 bite.** r14 at compile time (`E0451`), the rest as test failures |
| 6b | The 40-directive pump burst | `crates/` is byte-identical between `9bd18f9` and `dbb46d7` (`git diff --stat` empty for `crates/`) | Verified at `9bd18f9`: 40/40 acks in order, first at 2.05 ms, 3 ticks in 2.5 s. Unchanged by this commit |

## 4. Falsification rows

| Row | Observed |
|---|---|
| `timeout` + `killSignal` removed | `exit=124`, 1 named failure, **0 summary lines** — hangs |
| Named-error mapping removed (timeout kept) | `exit=1`, `not ok - a wedged preflight fails show.load by name…`, error *"must name the bound it exceeded"* |
| §12.4 frame-cap refusal | 1 FAILED |
| `audioDuration` refusal | 1 FAILED |
| Budget consulted for `auto` | 1 FAILED |
| §12.5 mandatory-`vram` failure | 1 FAILED |
| Planner honours `declared_format` | 1 FAILED |
| Image charged its own size | 4 FAILED |
| `/rate` divisor | 1 FAILED |
| `parseHouseRate` → `Number()` | 1 FAILED |
| `houseRate` wiring in `server.ts` | 1 FAILED |
| `sequenceRef` back in the schema | 1 FAILED |
| `manifestIdentity` → constant | 1 FAILED |
| Shared chain → hardcoded 1080p | 1 FAILED |
| Duplicate spec item number | 1 FAILED |
| Engine builds its own budget | **compile error `E0451`** |
| Engine ignores `vramBudgetMib` | 1 FAILED |
| Constant drifts from §12.4 | 1 FAILED |
| Per-block publish-and-reset (R2) | 1 FAILED |
| §12.4's 900-frame conjunct | 1 FAILED |
| `show.load` back on the async path (R5) | 1 FAILED |
| Pump drains only on the deadline (R6) | 1 FAILED |
| F1's unattributable counter | 1 FAILED |

## 5. The closing question

> After this fix, is there any remaining path where an accepted command can fail
> to reach a terminal state?

**No — in the control plane, subject to F2.** Every `await` on a command path:

- `show.load` → `loadPackage` → `runPreflight` — **bounded** by
  `preflightTimeoutMs()`, terminal either way.
- `show.preflight` → `runPreflight` — the same bound, and the same fix covered
  it without needing a second change.
- `show.stop` → `deps.waitForGrace(graceMs, …)` — bounded by
  `showStopGraceMs`, resolving `false` on expiry into the forced path. Terminal.

A repository-wide grep for `fetch(`, `spawn(`, `execFile` and bare `new Promise`
outside tests returns only those, plus the server's own startup and shutdown
promises, which are not on a command path. There is no HTTP call, no second
subprocess, and no unbounded `new Promise` in any handler.

The qualifier matters, though: F2 means an accepted command *does* reach a
terminal state, just the wrong one. "Bounded" and "correct" are different
properties, and the spine has now achieved the first everywhere and the second
everywhere except this constant.

## 6. Self-check

- Reported SHA `dbb46d7c37af05da53109581182ceed59d0b3879` re-verified equal to
  `refs/heads/P7-overlay-level` after all probes.
- Attached to a branch, not detached; restored with `git reset --hard HEAD`
  throughout — the eighth-variant rule, applied.
- Rebuilt from the clean tree before every measurement.
- Three consequential probes re-run against the confirmed SHA after a rebuild:
  the cost measurement (136 and 132 ms/frame — the finding reproduces), the
  wedged path end to end (`ok 7`, `# pass 7 / # fail 0`), and the no-timeout
  mutation (`exit=124`).
- I left eight orphaned `sleep` processes from earlier rounds' probes on this
  machine and only noticed because the orphan count started at 8 rather than 0.
  Cleaned up. It changed no result, but it is exactly the kind of ambient state
  that makes a later measurement lie.
