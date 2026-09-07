# PR #9 re-pass reports — superseded

**This file no longer holds a report.** It held the third independent pass over
`58988dc`, which returned **FINDINGS ×1 [MEDIUM] — F4**: SPEC §12.4's budget was
answered twice, `crates/nbe-preflight/src/main.rs` holding the table (256 / 512)
while `crates/nbe-engine/src/directive.rs` hardcoded a Prompt 05 placeholder of
1024 / 4096 and never read the manifest's `vramBudgetMib`. Measured: a 100-frame
1080p RGBA8 loop with `cachePolicy: "auto"` and no declared budget was streamed
and charged 0 by preflight while the engine made it VRAM-resident at 791 MiB —
and the package came back `airReady: true`.

**F4 was fixed in `c092f20`.** §12.4's constants moved into
`nbe_core::loop_cache` beside the rule they parameterise,
`CacheBudget::from_manifest` became the one constructor, the engine's
placeholder was deleted rather than adjusted, and the engine now reads
`loop.vramBudgetMib` through `PackageIndex::loop_budget()`.

**The fourth independent pass over `c092f20` returned CLEAN** — sixteen
falsification rows, all biting; every accumulated anchor re-verified (the 4K
gate, the probeable-asset case, the twenty-input `parseHouseRate` mirror, the
overflow boundary set, the CI gate in both directions); no fifth surface. That
verdict is what merged PR #9.

A merged branch must not carry a document asserting an open finding, so the
report is not left standing here. It is preserved in full in git history:

| Report | Commit | Verdict |
|---|---|---|
| Second pass over `d1d0e10` | `f9ebca2` | FINDINGS ×3 (1 HIGH) |
| Third pass over `58988dc` | `a692666` | FINDINGS ×1 (MEDIUM) — the text this file held |
| Fourth pass over `c092f20` | — | CLEAN; merged |

`git show a692666:docs/charter-review-repass.md` reads the superseded report.
The review charter that governs the whole sequence is
`docs/review-midpoint-integration.md`.
