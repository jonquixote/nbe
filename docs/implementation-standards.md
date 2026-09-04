# NBE Implementation Standards

The repeatable quality bar for every implementation prompt (02 onward). Prompt 01 is the reference implementation of this standard. `docs/prompt-01-definition-of-done.md` shows the standard applied and accepted.

Normative vocabulary: `View`, `Element`, `Sequence`, `Item` — never Program, Layer, Segment, Subsegment outside migration code and tests.

---

## 1. Schema-driven typed models

Any prompt that introduces or extends a typed representation of a JSON Schema MUST:

1. **Map completely** — every schema `properties` entry, `enum` value, and conditional (`if`/`then`) has a corresponding typed field, variant, or guard. No partial mirrors.
2. **Prove the mapping with a round-trip test using real fixtures**: deserialize fixture → typed model → serialize → re-validate against the schema. This test MUST fail if any field, variant, or optionality drifts. It is mandatory, not optional.
3. **Run an enum audit** when touching enums: compare each Rust/TS enum variant against the schema enum literal-by-literal, including capitalization and punctuation (`"HH:mm:ss"`, `"nv12Alpha"`, numeric enums like `frameRate: [30, 60]`). String-renamed variants are the historical bug class; check them first.
4. **Treat the schema as normative and immutable**: `schemas/*.json` changes are spec revisions, not prompt work. Alignment flows schema → code, never the reverse.

Reference pattern: `crates/nbe-core/src/manifest.rs` + `crates/nbe-core/tests/model.rs`.

## 2. Strict CI contracts

Any prompt that introduces a binary, fixture, or externally observable behaviour MUST add a CI gate that asserts **exact** outcomes:

1. Exit codes by value (e.g., `expected exit 2`, not "non-zero").
2. Key outputs: generated files, required strings in those files (e.g., `airReady` true, `migrationRequired` + `nbe-migrate` in the v0.2 rejection report).
3. Behavioural invariants where measurable (e.g., "recording is playable after SIGKILL", "OBS comparison table populated from measurement").

A gate that passes on any failure mode is not a gate. CI MUST fail loudly and name what it expected.

## 2a. Falsification (standing review step)

A green suite is evidence only if it could have been red. Every prompt's review
includes a **falsification pass**: for each behaviour the prompt claims to
deliver, delete or disable that behaviour in the production code and re-run the
suite. Record which tests failed.

The output is a table, and it belongs in the completion report:

| Behaviour removed | Tests that failed |
|---|---|
| … | … |

Rules:

1. **Every claimed behaviour needs at least one test that fails without it.** A
   behaviour whose removal breaks nothing is untested, whatever the suite's
   pass count says.
2. **Falsify the production path, not the test.** Removing an assertion proves
   nothing; removing the code the assertion is supposed to exercise proves
   everything.
3. **Restore and re-run.** The report states the suite is green again after the
   experiment, and the working tree is clean.
4. **A test that passes with its behaviour deleted is a defect**, and is fixed
   or deleted in the same change — not carried as coverage.

This step exists because it has caught real absence twice: a control-plane
bridge that delivered no directives, and a compositor where deleting the whole
render path left 7 of 8 tests passing.

## 2b. Test counts come from CI

Test totals reported in a completion message MUST be the summary lines the test
runners printed, not a hand-tallied figure. The `rust` and `control-plane` CI
jobs echo their summaries in a collapsed group for exactly this purpose; quote
those lines. Arithmetic across suites has been wrong often enough that it is no
longer an acceptable source.

## 3. Prompt structure

Every implementation prompt uses this skeleton:

1. **Status & assumptions** — what is already true, with references.
2. **Goals** — what this prompt delivers.
3. **Constraints** — what MUST NOT change, explicitly.
4. **Work items** — numbered steps.
5. **Tests & CI expectations** — required new tests, required CI changes.
6. **Reporting obligations** — what the "done" message must contain.

Every prompt MUST also state, in its constraints:

- **Forbidden changes** (e.g., no schema edits, no changes to existing exit codes, no scope growth).
- **New tests required** and **CI changes required**.

## 4. Hygiene vs behaviour

- **Hygiene**: caching, logging, error-type tightening, path cleanup, dependency trimming. Hygiene-only prompts MUST NOT alter runtime behaviour.
- **Proof of hygiene**: all prior tests remain unchanged and green. If a hygiene change requires modifying an existing test, it is a behaviour change — reclassify it and say so.
- **Behaviour**: anything observable by a caller, a test, a CI gate, or an operator. Behaviour changes require their own prompt and acceptance criteria.

## 5. Definition of done (all prompts)

Inherited from SPEC Section 23, extended:

```text
schema-valid
state-safe
telemetry-visible
acceptance-tested
mix-minus-safe
click-free
loop-budget-accounted
sandbox-verified
degradation-ordered
model-round-trip-pinned      (Prompt 01c)
ci-gated-exactly             (Prompt 01c)
```
