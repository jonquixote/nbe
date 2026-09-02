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
