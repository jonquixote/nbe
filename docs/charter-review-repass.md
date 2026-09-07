# PR #9 — fresh re-pass on the post-fix head

**Verdict: FINDINGS.** Three, all reproduced from a clean clone. None of them
is in the fix round's own subject matter: the eleven falsification rows the fix
agent reported all reproduce, `parseHouseRate` is an exact mirror across twenty
probes, and the boundary behaviour holds at 900/901/`u64::MAX`. Two findings
are in the seams the round opened, and one is the record the round was asked to
leave and did not.

- **Head reviewed:** `a83dacbf4a884485103ad6cf669e24a2b4655ae1`
- **Confirmed equal to** `refs/heads/v0.4-spec` at review time
- **Checkout:** fresh clone, attached to a local branch at that SHA, tracked
  tree clean (0 modified files) after every mutation/restore cycle below

---

## 1. CI gate lines, verbatim, first action

Both jobs, run exactly as `.github/workflows/ci.yml` runs them.

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
test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 15 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
TOTAL: 160 passed, 0 failed, 0 ignored
::endgroup::
==== STEP: GPU/render tests actually ran (Prompt 04)
prompt04 tests that ran: 15
==== STEP: v0.3 fixture passes
preflight OK: air-ready. report at tests/fixtures/valid_show_v0.3/preflight_report.json
  grep -q "airReady": true  -> exit 0
==== STEP: v0.2 fixture is rejected (migration gate)
preflight FAILED (3 error(s), 0 warning(s)): report at tests/fixtures/valid_show/preflight_report.json
  exit code 2; migrationRequired + nbe-migrate present
```

### Job `control-plane`

```
==== STEP: cargo build -p nbe-preflight   (working-directory: .)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.33s
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

**Green CI is the floor, and the floor is met.** The findings below are all
things CI cannot see.

---

## 2. Findings

### F1 — [HIGH] Preflight contradicts itself about a loop's dimensions, silencing a §12.5 MUST-fail

**`crates/nbe-preflight/src/main.rs:415-416`** (the §12.5 gate) against
**`:519` / `:523`** (the estimator).

Both call the shared `loop_plan()` — the round's headline fix, whose own doc
comment says the two callers "must answer from the same plan, or preflight
contradicts itself inside a single run." They pass different arguments. When an
asset's dimensions were not probed, the §12.5 gate falls back to a hardcoded
`1920` / `1080`; the estimator falls back to the manifest's declared
`house_w` / `house_h`. One rule, two inputs.

**Reproduction.** A schema-legal 4K package (`show.video: 3840×2160`) with an
asset whose header will not parse, so no dimensions are probed:

```json
{ "id": "L", "kind": "image", "source": "media/loop.png", "format": "png",
  "loop": { "periodFrames": 30, "textureFormat": "rgba8", "cachePolicy": "vram" } }
```

```
exit=0
airReady: True
errors: []
resources: {'vramDemandMib': 94, 'audioDemandMib': 0, 'declaredHouseRate': 30}
```

The §12.5 gate planned at 1920×1080 — 7.91 MiB/frame, `maxFrames = 32`, so 30
frames "fit" and no error was raised. The estimator planned at 3840×2160 —
33.18 MiB/frame, `maxFrames = 7`, so the loop "does not fit", was treated as
streamed, and contributed **nothing**. `vramDemandMib: 94` is the 4K floor
alone.

**Control.** The identical manifest with `show.video` set to 1920×1080 reports
`vramDemandMib: 261` — the loop counted, both halves agreeing. The only
variable is the house resolution.

**Consequence.** §12.5: "Otherwise it MUST be streamed, unless `cachePolicy:
vram` is mandatory, in which case preflight MUST fail." For any package whose
show resolution is not 1080p and whose loop asset dimensions cannot be probed —
a video whose decode fails, an image whose header will not parse, any asset kind
that is never probed — that MUST is silently not enforced, and the operator is
handed a demand number with the loop missing from it. This is the same defect
class the round was convened to fix (a resource number an operator plans
against being wrong), reintroduced by the fix for it.

**Fix shape.** One fallback, not two. The estimator's is the correct one: an
asset's own dimensions where measured, the house frame otherwise. Pass
`house_w` / `house_h` at `:415-416`, or hoist the house resolution so both
callers read the same values.

**Observation, same fixture, not raised as a separate finding.** An `image`
asset carrying a `loop` block is charged through the loop branch only, so when
that loop streams it is charged zero — while §12.11.1's `imageDemandMiB` sum
has no exception for images that also declare a loop. A degenerate combination
the schema permits; worth a sentence when F1 is fixed.

### F2 — [LOW-MED] §19.3's normative checklist now has two item 12s and two item 13s

**`docs/spec.v0.4.md:3063-3066`.**

§19.3 ("The preflight test suite MUST include:") is a single numbered list. The
fix round inserted its two new rows at 12 and 13 without renumbering the four
that followed:

```
11. 29.97 fps asset without pulldown metadata.
12. **A contradictory Item** — e.g. `{"kind": "slate", "sceneRef": …}` (Section 17.5). …
13. **A loop period beyond Section 12.4's absolute cap** — …
12. VRAM-residency request that exceeds `maxFramesByBudget`.
13. Circular sub-scene reference.
14. Self-triggering automation rule.
15. Plugin failing sandbox validation.
16. Unresolvable `sceneRef`, `pluginId`, or group `children` entry.
```

Eighteen entries numbered to sixteen. Introduced by this round: `d1d0e10^`
numbers the list 1–16 cleanly. A scan of every ordered list in the document
finds exactly one collision, so §6's parallel additions (items 27–29) landed
correctly and this is a single slip.

Nothing cites §19.3 by item number today, so nothing is currently broken — but
the document routinely cites checklist rows by number elsewhere (`§12.11.3 #1`,
`§7.15 #2`), and this is normative text. Renumber 12–16 to 14–18.

### F3 — [LOW] The recorded divergence records only the engine's half

**`crates/nbe-engine/src/video.rs:195` and `:225`.**

The comment explains why the engine passes `declared_format: None` — "passing a
declared format here would make the plan describe a residency the engine does
not hold." Correct, and it is the engine's side of the story. Nothing anywhere
in the repository states the consequence on the other side: that until the
format ladder is real, **preflight's declaration-based number can sit below
what the engine will actually hold, and can disagree with it about residency
policy.** A grep for any under-reporting statement returns nothing.

**Reproduction.** A 200-frame 1080p loop declaring `textureFormat: "bc7"` and
`vramBudgetMib: 512`:

```
preflight (declared_format=Bc7)    format=Bc7   frameCost=1.978 MiB maxFrames=258 policy=Vram      residentTotal=396 MiB  readAhead=60 (119 MiB)
engine    (declared_format=None)   format=Rgba8 frameCost=7.910 MiB maxFrames=64  policy=Streaming residentTotal=1582 MiB readAhead=60 (475 MiB)
```

and the binary agrees: `preflight vramDemandMib = 419` (23 MiB floor + a 396
MiB loop), no `loopBudget` error — the loop is reported VRAM-resident. The
engine, through the same shared planner, will stream it and hold roughly 475
MiB of read-ahead. Preflight both mis-states the policy and under-reports the
residency.

The divergence itself is deliberate and correctly deferred to the format
ladder. Only the record is incomplete. One sentence beside the existing comment
(or in the `loop_cache` module header, where the "one rule, one implementation"
claim is made) closes it.

---

## 3. Fix mapping → verification

| # | The round's claim | How it was verified here | Result |
|---|---|---|---|
| 1 | `u64` overflow: saturating arithmetic + §12.4's 900-frame cap, refused by name; same class fixed in the audio `sum()` | Boundary probes at 900 / 901 / `u64::MAX`; audio at the exact `checked_mul` edge (48038396025285 vs …286); 31 assets at the edge to overflow the fold; one package with both | **Holds.** 900 passes, 901 named, `u64::MAX` named, audio edge named one frame past. Debug build throughout — no panic anywhere. Report written on every refusal (P1) |
| 2 | `parseHouseRate` mirrors `u32::from_str` exactly | Compiled the engine's expression verbatim and ran 20 inputs through both sides | **Holds, 20/20 identical** — including `"0"`→0 both, `"+50"`→50 both, `"4294967295"` accepted both, `"4294967296"`→30 both, `"60abc"`/`" 30"`/`"1e2"`/`"5_0"`/Arabic-Indic `"٥٠"`→30 both |
| 3 | Budget decision moved to `nbe-core::loop_cache`; preflight's ladder copy deleted; both consumers on one path | Diffed the module across the move; counted the engine's original tests; reproduced 2396→23 | **Partly.** Move is additive-only, all six original tests intact, 2396→23 reproduces exactly. But the two consumers pass **different dimensions** — see **F1** |
| 4 | `manifestVersion` pinned; real consumer reads the fixed path | Traced every producer and consumer in `packages/control-plane/src` | **Holds.** One path only: `package.ts:136` → `PackageInfo` → `state.ts:444`. No second copy. (`/status` does not carry the field at all; the persisted identity record is the consumer, and that is what the test pins) |
| 5–8 | Retirement scrub, wire-version sentence, §17.5 in both checklists, charter committed | Repo-wide grep for `sequenceRef` / `sequence.arm` / `SequenceRef`; located §5.4's wire paragraph; read §6 #27 and §19.3; resolved every `[RI-n]` | **Holds.** Every surviving mention is a retirement *record*. Wire sentence at §5.4. §17.5 in both checklists. All nine `[RI-n]` in the report resolve against the charter. (§19.3's numbering is **F2**) |
| 9–12 | Exact floor; 82/81 MiB audio bracket; server-level house rate; `sequenceRef` at validation with the variant deleted | Ran every falsification row; recomputed the bracket independently; read `ItemKind` | **Holds.** Bracket arithmetic confirmed (81→0 MiB, 82→1 MiB, 8200→100 MiB). `ItemKind` has four variants |
| CI | The prompt04 gate's zero-tests branch was unreachable | Ran the fixed gate against a real run, a synthesized `0 passed`, and a missing target; ran the **old** pattern against the same zero-test input | **Holds.** New: 15 → passes; `0 passed` → refused; no target → refused. Old pattern against `0 passed` → **passed**, confirming the defect was real |

## 4. Falsification rows, re-run

Every row: delete the behaviour, run the named test, restore. All eleven fail
as claimed — no false falsifiers.

| Behaviour deleted | Named test | Observed here |
|---|---|---|
| §12.4 frame-cap refusal | `a_period_past_the_frame_cap_is_named_not_a_panic` | FAILED |
| `audioDuration` refusal | `an_uncountable_audio_duration_is_named_not_a_panic` | FAILED — `left: 0` (exit 0, air-ready) |
| Budget consulted for `auto` | `an_auto_loop_the_budget_cannot_hold_is_streamed_not_charged` | FAILED — **`left: 2396`** vs 23 |
| §12.5 mandatory-`vram` failure | `a_mandatory_vram_loop_that_does_not_fit_fails_preflight` | FAILED |
| Planner honours `declared_format` | `loop_cache::tests::a_declared_format_overrides_the_derived_one` | FAILED — `left: Nv12`, right `Bc7` |
| Image charged its own size | `an_images_cost_is_its_own_size_not_the_house_frame` **+ 3 more** | FAILED ×4 — `left: 31` vs 23; the tightened floor catches what `>= 20` admitted |
| `/rate` divisor in the audio sum | `audio_demand_is_pinned_at_a_mib_boundary` | FAILED — `left: 29` vs 0 |
| `parseHouseRate` → `Number(raw ?? 30)` | `§7.15: parseHouseRate mirrors the engine's parse` | not ok 43 |
| `houseRate: opts.houseRate` in `server.ts` | `§7.15: … reachable through the SERVER` | not ok 44 — dispatcher test stayed green, which is the point |
| `sequenceRef` restored to the schema enum | `validate::tests::a_sequenceref_item_is_refused_at_validation` | FAILED — "a retired hook must not validate" |
| `manifestIdentity` → constant `"0.3"` | `the recovery record carries the LOADED package's manifest version` | not ok 45 |

## 5. Self-check

- Reported SHA `a83dacbf4a884485103ad6cf669e24a2b4655ae1` re-verified equal to
  `refs/heads/v0.4-spec` after all probes.
- Checkout attached to a branch, not detached; tracked tree clean (0 modified)
  after every mutation/restore cycle.
- The three consequential probes (F1's 4K package, F2's list scan, F3's plan
  comparison) re-run against that confirmed SHA after a rebuild — all three
  reproduce identically.
- Every claim above re-read against the command output that produced it.
