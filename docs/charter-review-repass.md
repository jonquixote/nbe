# PR #9 — third re-pass, head `58988dc`

**Verdict: FINDINGS.** One, MEDIUM, reproduced end to end.

All three findings from the second pass are closed, and every accumulated
anchor holds: thirteen of thirteen falsification rows still bite, the
`parseHouseRate` mirror is exact across twenty adversarial inputs, the overflow
boundary set behaves identically under the rewritten estimator arithmetic, and
the CI gate refuses a zero-test run in both synthesized forms.

The one finding is the answer to the question this pass was asked. F1's fix did
close the class **inside preflight**. It did not close it **across the crate
boundary**, which is the boundary `loop_cache` was moved to `nbe-core` to
protect. Three of `plan()`'s inputs are still assembled independently by the two
crates, and one of the three puts the engine outside §12.4's normative table.

- **Head reviewed:** `58988dcd37935fa1bce3fa4d80e02a88e0fc853c`
- **Confirmed equal to** `refs/heads/v0.4-spec` at review time
- **Checkout:** fresh clone, attached branch, tracked tree clean (0 modified)
  after every mutation/restore cycle

---

## 1. CI gate lines, verbatim, first action

### Job `rust`

```
==== STEP: cargo fmt --all -- --check
(clean)
==== STEP: the unsafe exception stays in crates/nbe-decode
unsafe exception confined to crates/nbe-decode/src
==== STEP: cargo clippy --workspace --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.83s
==== STEP: cargo test --workspace (summary echoed for the audit trail)
::group::Rust test summary
test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 15 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 34 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 30 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 15 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
TOTAL: 162 passed, 0 failed, 0 ignored
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
# tests 45
# pass 45
# fail 0
# skipped 0
# todo 0
::endgroup::
passed=45 failed=0
  gate satisfied
```

---

## 2. Finding

### F4 — [MEDIUM] §12.4's budget is answered twice, and the two answers differ by 4× and 8×

**`crates/nbe-preflight/src/main.rs:16-17`** against
**`crates/nbe-engine/src/directive.rs:126-130`.**

```rust
// preflight — §12.4's table verbatim
const DEFAULT_PER_LOOP_MIB: u64 = 256;
const DEFAULT_TOTAL_LOOP_MIB: u32 = 512;

// engine — inline literals, no §12.4 citation, no comment
let budget = crate::loop_cache::CacheBudget {
    per_loop_mib: 1024,
    total_mib: 4096,
    recommended_working_set_mib: None,
};
```

§12.4's table is *"default per-loop budget | 256 MiB"* and *"default total
short-loop budget | 512 MiB"*. Preflight matches it. The engine is 4× and 8×
over it, and reads the manifest's `vramBudgetMib` not at all — a grep for it
across `crates/nbe-engine/` returns nothing but the F3 comment. So the engine's
loop cache plans every loop against a fixed 1024 MiB whatever §12.4 says and
whatever the package declared.

`loop_cache` was moved into `nbe-core` this round with a module header that
states the reason: *"Lives in `nbe-core` because two crates must agree… One
rule, one implementation."* The rule is shared. Three of its inputs are not:

| Input to `plan()` | preflight | engine | recorded? |
|---|---|---|---|
| `CacheBudget` | 256 / 512 (§12.4) | 1024 / 4096, manifest ignored | **no** |
| `yuv_sampling` | `!has_alpha` → NV12 for opaque video | hardcoded `false` → RGBA8 | **no** |
| `declared_format` | the manifest's declaration | `None` | yes — F3's note |

**Reproduction 1 — the budget.** A 100-frame 1080p RGBA8 loop,
`cachePolicy: "auto"`, no declared `vramBudgetMib`:

```
preflight: airReady=True vramDemandMib=23 (loop charged 0)
preflight (main.rs:16-17  = 256 / 512)        budget= 256 MiB maxFrames= 32 policy=Streaming resident=791 MiB
engine    (directive.rs:126-130 = 1024/4096)  budget=1024 MiB maxFrames=129 policy=Vram      resident=791 MiB
```

`vramDemandMib: 23` is the render-target floor alone. Preflight streams the
loop and charges nothing; the engine, through the same shared planner, makes it
VRAM-resident and allocates **791 MiB**. The package is `airReady: true` and
the number an operator plans against is short by 791 MiB.

**Reproduction 2 — the format, with nothing declared.** A `kind: "video"` loop,
60 frames, **no `textureFormat` at all**:

```
preflight: vramDemandMib=201  -> loop charged 178 MiB as NV12
engine:    holds RGBA8                      = 474 MiB
```

Preflight passes `yuv_sampling: !has_alpha` (true for opaque video, so NV12 at
1.5 B/px); `video.rs` hardcodes `yuv_sampling: false` (RGBA8 at 4 B/px). No
declaration is involved, so F3's note — which is scoped to *"a loop declaring
`nv12`, `nv12Alpha` or `bc7`"* — does not cover it. This is the default case:
every loop that does not name a format is under-reported 2.67×.

**Direction.** Preflight's effective budget is always ≤ the engine's (at most
512 against a fixed 1024), so the error is one-directional: preflight streams
loops the engine will hold, and `vramDemandMib` **under-reports** what the
engine allocates. §12.5's mandatory-`vram` gate can therefore only *false-refuse*
— it will never pass a package the engine cannot hold — which is why this is
MEDIUM and not HIGH. No MUST is silenced. What is wrong is the number, and
§12.11.3 #1 makes that number the deliverable: *"a number an operator can read
is the point."*

**Provenance.** `per_loop_mib: 1024` dates to Prompt 05 (`0e9b79c`), where it
was a placeholder with no spec table to answer to. `DEFAULT_PER_LOOP_MIB = 256`
arrived in `a95ddc2` — this PR's first fix round — when preflight was given
§12.4's real numbers. The divergence was created by this PR, in the same commit
that moved the rule into `nbe-core` so the two crates could not disagree.

**Fix shape.** §12.4's defaults are spec constants, so they belong beside the
rule they parameterise: `nbe_core::loop_cache` should own
`DEFAULT_PER_LOOP_MIB` / `DEFAULT_TOTAL_LOOP_MIB` / `ABSOLUTE_LOOP_FRAME_CAP`,
and both crates should construct `CacheBudget` from them — the engine reading
the manifest's `vramBudgetMib` the way preflight does. `yuv_sampling` is a real
capability difference and should stay a caller's answer, but then it belongs in
F3's note alongside `declared_format`, whose wording needs to widen from
"a loop declaring a format" to "any loop whose planned format is not RGBA8."

**Observation, not raised as a finding.** §12.4 states residency as a
conjunction — `periodFrames <= 900` **and** `<= maxFramesByBudget` **and**
`totalShortLoopCache <= totalBudget`. `plan()` implements only the second;
`ABSOLUTE_LOOP_FRAME_CAP` lives in preflight alone. A 2,000-frame 320×180 loop
plans `Vram` through the shared planner (`maxFrames = 3106`) in violation of the
first conjunct. Unreachable in production because preflight refuses the package
before the engine sees it, so it is a note for whoever consolidates the
constants, not a defect today.

---

## 3. Claim-by-claim verification

| Claim | How verified | Result |
|---|---|---|
| **F1(a)** 4K reproduction now refuses | Original fixture: 4K house, unprobed loop, `cachePolicy: "vram"`, 30 RGBA8 frames | **Closed.** `exit=2`, `airReady: false`, `loopBudget: … exceed the budget's 8 (SPEC §12.5)` |
| **F1(b)** probed size beats the house frame | New fixture: 4K house, **probeable** 640×360 image loop | **Holds.** `vramDemandMib: 121` = 94 MiB 4K floor + 26 MiB (30 × 640×360×4). Planned at the probed size; the house frame would have given 949 MiB and streamed |
| **F1(c)** 1080p control unchanged | The re-pass's own `hd` fixture | **Holds.** `261` MiB, identical to the previous pass |
| **F1(d)** no stray per-frame arithmetic | Grepped every `* 4` / `bytes_per_pixel` / `w * h` in `main.rs` | **Holds.** Two sites remain, both §12.11.1 terms that are not loop frames: the render-target + slate floor at house resolution, and `imageDemandMiB = w × h × 4` using `asset_dimensions`. The loop charge is `plan.frame_cost_mib` only |
| **F1** one chain, structurally | Read `asset_dimensions` and both `loop_plan` call sites | **Holds.** `loop_plan(asset, lm, manifest, report)`; no caller passes dimensions; the image branch reads the same chain |
| **F2** §19.3 renumbered | Read §19.3; ran the duplicate scan over all five `docs/spec.v*.md` | **Holds.** 1–18 clean, no duplicates anywhere in the corpus |
| **F2** the check itself | Constructed two adjacent 1-2-3 lists separated by one blank line | **Sound enough — see the read.** It merges them and would flag; but in CommonMark that *is* one list, and any non-blank line (heading, paragraph, fence) resets the run. No false positive exists in the corpus |
| **F3** three places, with direction | Grepped all three | **Holds.** `spec.v0.4.md:2033`, `video.rs:198-202`, `loop_cache.rs:18-21`; each says the smaller number is not the safe one until the ladder lands |
| **F3** worked example | Ran the 200-frame BC7 package at `vramBudgetMib: 512` | **Reproduces exactly.** `vramDemandMib = 419` = 23 floor + **396** resident, no `loopBudget` error. Matches the spec note's number |
| **F3** coverage | Probed the undeclared-format case | **Incomplete — folded into F4.** The note is scoped to loops that *declare* a sub-RGBA8 format; the default case diverges too |
| `parseHouseRate` mirror | Recompiled the engine's expression, 20 inputs both sides | **20/20 identical, 0 mismatches** |
| Overflow boundaries | 900 / 901 / `u64::MAX`; audio `checked_mul` edge both sides; 31-asset saturating fold; both-huge | **Unchanged from the previous pass.** Report written on every refusal, no panic in a debug build. The `frame_cost_mib` rewrite did not disturb saturation |
| CI gate fix | Real run, synthesized `0 passed`, missing target | **Holds both ways.** 15 → passes; `0 passed` → refused; no target → refused |

## 4. Falsification rows — all thirteen, re-run

| # | Behaviour deleted | Named test | Observed |
|---|---|---|---|
| 1 | §12.4 frame-cap refusal | `a_period_past_the_frame_cap_is_named_not_a_panic` | FAILED |
| 2 | `audioDuration` refusal | `an_uncountable_audio_duration_is_named_not_a_panic` | FAILED |
| 3 | Budget consulted for `auto` | `an_auto_loop_the_budget_cannot_hold_is_streamed_not_charged` | FAILED |
| 4 | §12.5 mandatory-`vram` failure | `a_mandatory_vram_loop_that_does_not_fit_fails_preflight` | FAILED |
| 5 | Planner honours `declared_format` | `loop_cache::tests::a_declared_format_overrides_the_derived_one` | FAILED |
| 6 | Image charged its own size | `an_images_cost_is_its_own_size_not_the_house_frame` **+ 3 more** | FAILED ×4 |
| 7 | `/rate` divisor in the audio sum | `audio_demand_is_pinned_at_a_mib_boundary` | FAILED |
| 8 | `parseHouseRate` → `Number(raw ?? 30)` | `§7.15: parseHouseRate mirrors the engine's parse` | not ok 43 |
| 9 | `houseRate: opts.houseRate` in `server.ts` | `§7.15: … reachable through the SERVER` | not ok 44 |
| 10 | `sequenceRef` restored to the schema enum | `validate::tests::a_sequenceref_item_is_refused_at_validation` | FAILED |
| 11 | `manifestIdentity` → constant `"0.3"` | `the recovery record carries the LOADED package's manifest version` | not ok 45 |
| 12 | Shared chain → hardcoded 1920×1080 | `a_loop_is_planned_at_one_resolution_not_two` | FAILED |
| 13 | Seed a duplicate item number | `the_specs_numbered_lists_number_consistently` | FAILED — `spec.v0.4.md:3068: item 14 repeats inside the list starting at line 3054` |

Thirteen of thirteen. No falsifier has stopped failing.

## 5. Self-check

- Reported SHA `58988dcd37935fa1bce3fa4d80e02a88e0fc853c` re-verified equal to
  `refs/heads/v0.4-spec` after all probes.
- Attached to a branch, not detached; tracked tree clean after every
  mutation/restore cycle.
- Three consequential probes re-run against that confirmed SHA after a rebuild:
  F1's 4K case still refuses with `budget's 8`; the budget divergence still
  reports 23 MiB where the engine holds 791; the format divergence still reports
  178 MiB where the engine holds 474.
- Every claim above re-read against the command output that produced it.
