# Independent pass over the F3 fix — `P7-overlay-level @ 9a3016e`

**Verdict: FINDINGS.** One, MEDIUM, reproduced end to end.

F3 is fixed and the fix is better than it claims. The bytes term closes the
undeclared case with margin, `max()` selects correctly across four distinct
bases, the rates re-measure on my own fixtures within 1% run-to-run, and — the
answer to the machine-variance question the round left standing — the constants
were measured on the **spec's normative reference target**, not an arbitrary
laptop: `docs/hardware-baseline.txt` records a 6-core Intel i7 @ 2.6 GHz, which
is the machine these numbers come from.

The finding is the one the round invited me to try to break, and it breaks. The
"loose is fine" trade produces a **wrong outcome, not a slow one**: commands are
strictly serialised per connection, the derived bound has no ceiling, and a
100 MB undeclared package yields a bound of 6.7 hours on release and 20.8 hours
on debug. During that window the operator's connection answers **nothing** —
measured: `show.load` issued, `system.status` two seconds later, thirty seconds
elapsed, zero responses. The flat 600 s constant capped that at ten minutes.

- **Head reviewed:** `9a3016ed8746a2853cc8056722e3083807ae2dc2`
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
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1m 31s
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
    Finished `release` profile [optimized] target(s) in 6m 22s
  [timing] cold (no cache restored): 382 s
    Finished `release` profile [optimized] target(s) in 1.41s
  [timing] warm (cache restored): 1.65 s
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
# tests 49
# pass 49
# fail 0
# skipped 0
# todo 0
::endgroup::
passed=49 failed=0
  gate satisfied
```

The cache's promise holds: **382 s cold, 1.65 s warm.**

---

## 2. Finding

### F4 — [MEDIUM] An unbounded bound plus a serialised connection is a stall, not a delay

**`packages/control-plane/src/package.ts`** (`preflightBound`, no ceiling) with
**`packages/control-plane/src/server.ts:366-371`** (per-connection serialisation).

The round defended the loose bytes term: *"a wedged process never finishes, so a
generous bound still catches it — the unacceptable direction was refusing
legitimate loads."* The first half is true. The second half assumes the only
cost of generosity is waiting, and it is not.

**Commands are strictly serialised per connection.** `server.ts` chains them
deliberately, and says so:

```ts
      // Commands execute strictly in arrival order on this connection;
      // an async handler (show.load's preflight subprocess) must not race
      // the next command.
      session.tail = session.tail.then(() => handleMessage(...));
```

**The bound has no ceiling** — `Math.max(PREFLIGHT_FLOOR_MS, framesMs, bytesMs)`
and nothing above it. So the bytes term, which dominates on large packages,
sets how long that connection is unavailable.

**Reproduction.** A 100 MB package with no declared durations, against a
preflight binary that never answers:

```
  derived bound: 75000000 ms = 20.8 hours (basis=bytes, 100 MB)
  sent show.load (edecd408), then system.status (94a058bf) 2 s later
  after 30 s, responses received: NONE
  system.status answered? NO — the connection is blocked behind show.load
```

On release the same package gives 6.7 hours; on debug, 20.8. A realistic
30-minute bulletin at 8 Mbps is ~1.8 GB, which puts the bound in **days**.

**Why this is a wrong outcome rather than a slow one.** The operator's control
channel accepts nothing — no `show.stop`, no `view.fallback`, no
`system.status` — and emits no signal explaining why, because the outstanding
command simply has not answered. That is the precise condition this spine spent
three rounds abolishing: R6's ack fix, §5.9.5's quiescence handshake, and F1's
"accepted, never resolved" were all about an operator being able to know where
they stand. Recovery exists — a new WebSocket gets a fresh `session.tail` — but
it requires the operator to guess that reconnecting is the remedy.

**And it is a regression against the constant it replaced.** The flat 600 000 ms
capped this stall at ten minutes for every package. The derived bound removed
the cap in the name of never refusing a legitimate load, and for large packages
traded a ten-minute stall for a multi-hour one.

**Fix shape.** A ceiling: `min(ceiling, max(floor, framesTerm, bytesTerm))`,
with the ceiling stated in the same measured style as the rest — long enough to
cover the largest package the reference target can legitimately preflight,
short enough that an operator is not stranded. The override can exceed it for
the genuine outlier, which is what an override is for. Alternatively, take the
`show.load` handler off the connection's serial chain so a stalled load cannot
mute the channel — but that reopens the race the comment was written to close,
so the ceiling is the smaller change.

---

## 3. Claim-by-claim verification

| # | Claim | How verified | Result |
|---|---|---|---|
| 1a | Undeclared package resolves, bound 183 636 ms basis=bytes, margins 10.6×/3.7× | Rebuilt both binaries, ran `loadPackage` end to end | **Holds.** release bound 183 636 → RESOLVED 17 810 ms, exit 0, **10.3×**; debug bound 573 864 → RESOLVED 178 180 ms, exit 0, **3.2×**. (The round measured 10.6×/3.7× on a quieter machine; same numbers, same conclusion) |
| 1b | The lying-downward case is covered | The round's own `liar` fixture declares 800 of 1200 frames, so its frames term alone (480 s) already covered it — I built the sharp version: **one** asset declaring 100, seven declaring nothing, 1200 real frames | **Holds, and the bytes term is load-bearing.** frames-term alone would be 60 000 ms (the floor) and would have killed it; actual `RESOLVED after 155 874 ms, exit 1, 1 warning` — the warning being the duration mismatch, which confirms such a package loads |
| 2 | `max()` — four bases, none undercutting | Four fixtures × two profiles | **Holds.** undeclared → `bytes` (183 636 / 573 864); 50 000 declared frames in a 3 KB file → `frames` (3 750 000 / 30 000 000); nothing at all → `floor` (60 000); override → `override` (777) |
| 3a | Rates re-measure | My own fixtures, three runs each, release | **Holds and is conservative.** A1 22.8 / 22.9 / 22.8; cfr_30 52.8 / 52.4 / 52.4; av_tone 7.5 / 7.6 / 7.7 s/MB. Run-to-run spread under 1%. Worst is 52.8 against a constant of 80 — **1.5× headroom before the ×3 safety** |
| 3b | Is "measured on one laptop" a live risk? | Checked the machine against `docs/hardware-baseline.txt` and §0.3 | **Largely answered.** This machine is an Intel i7-9750H; the baseline records a 6-core Intel i7 @ 2.6 GHz — the same machine. §0.3 makes it *normative* for §12.11's arithmetic. So the constants come from the reference target, not an arbitrary one. **Residual, stated:** CI runs `macos-14` (Apple Silicon), a different architecture where these rates are unmeasured; the direction is very likely favourable (hardware decode), and in practice CI's own fixtures land on the 60 s floor with a >17× margin, so nothing there is at risk today |
| 4 | The loose trade is acceptable | Tried to construct a wrong outcome | **FAILS — see F4.** Generosity plus per-connection serialisation mutes the operator's channel for hours |
| 5a | Cache stores dependencies only, keyed on `Cargo.lock` + rustc | Read the config and the action's contract | **Holds as configured.** `Swatinem/rust-cache@v2` on all three cargo jobs; the comment states the reason in the YAML, not only in the commit |
| 5b | A source change cannot yield a stale binary | Changed `main.rs` with `Cargo.lock` untouched, rebuilt, hashed | **Holds, doubly.** binary `66abe7ce…` → `1696929a…` with the lockfile identical: cargo's own fingerprint rebuilds on source change, so even a mis-scoped cache could not serve a stale binary. The action's workspace-exclusion is belt to that braces |
| 6 | The refined row fails on the halved rate | Ran it | **Holds** — `expected: 250000, actual: 125000`. The assertion pins the rate, not merely its sufficiency |

## 4. Falsification rows

| Row | Observed |
|---|---|
| Bytes term removed | `not ok 49 - the bound covers a package that declares no durations at all` |
| Bytes rate halved | `not ok 49` — `expected: 250000, actual: 125000` |
| `"0"` accepted as override | `not ok 47` **and** `not ok 49` |
| Resolver prefers debug | `not ok 48 - preflightBin prefers the release build` |
| Per-frame factor removed | `not ok 47` |
| `parseHouseRate` → `Number()` | 1 failed |
| `houseRate` wiring in `server.ts` | 1 failed |
| `manifestIdentity` → constant | 1 failed |
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

**21 rows, all bite.** The spine's five engine-side rows (R2, the 900-frame
conjunct, R5, R6, F1) were verified at `1da6139`; `git diff --stat 1da6139
9a3016e -- crates/` is empty, so `crates/` is byte-identical and those results
carry.

## 5. The standing question, final form

> Correctly bounded with no input that can be absent or lie.

**The inputs are now sound; the output is not.** Bytes cannot be absent — a
package that references media has them, `stat` reads them before any decode, and
they cover both the undeclared case and the under-declaring one. That half of
the claim is proven.

What is not bounded is the bound itself. `max()` has a floor and no ceiling, so
the derived value is unbounded above, and per-connection serialisation converts
an unbounded bound into an unbounded stall. The ring this time is not the input
— it is that a derivation with a floor and no ceiling is only half a bound.

## 6. Self-check

- Reported SHA `9a3016ed8746a2853cc8056722e3083807ae2dc2` re-verified equal to
  `refs/heads/P7-overlay-level`; tracked tree clean after every cycle.
- Rebuilt from the clean tree before every measurement.
- **Caught mid-pass: the disk filled**, and `cargo build` truncated
  `target/release/nbe-preflight` to 1 712 bytes while still exiting through my
  pipeline. My first re-probe failed with `exit 127` and, read carelessly, would
  have looked like a finding about the resolver. Freed space, rebuilt both
  binaries, re-ran — the numbers above are from the rebuilt tree. A truncated
  binary is the same class as the stale one §2a already warns about, and it is
  worth knowing that a full disk produces it silently.
- My blocked-connection probe left a wedged `sleep` child when the harness
  outlived its command timeout; killed and verified zero before continuing.
- Three consequential probes (undeclared end-to-end both profiles, the four
  bases, the 100 MB bound) re-run against the confirmed SHA after the rebuild.
