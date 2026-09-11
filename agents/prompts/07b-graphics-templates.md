# Agent Prompt 07b — Graphics: Text, Templates, Ticker and Clock (crates/nbe-engine)

> **Upgrade pass, 2026-09-10.** This document has had the pass its own note
> demanded. The graphics mission is unchanged; everything it assumed about the
> engine has been re-measured against the tree, and its spec citations have been
> re-checked against v0.4. `agents/prompts/07b-delta.md` is the evidence behind
> the ledger below and remains readable on its own.
>
> **The scope decisions still stand.** Packaged fonts with no host-system
> fallback (§0.1 assumption 11), no per-frame relayout (§6.5), RSS fetched and
> sanitized at the control plane and never in the engine (§10.7 item 2).
> `docs/portability.md` row 7 records the first two as portability assets;
> weakening them is a spec change, not a prompt change.

**Targets: SPEC v0.4 (`docs/spec.v0.4.md`) — §6.5 (graphics and fonts, line 690), §7.10 (overlay level, line 981), §7.13 (frame budget, line 1039), §10.7 item 2 (RSS sanitize and rate-limit), §16.5 (element/graphic commands), §16.7 (ticker commands), §16.13 (clock commands), AC-15 (ticker RTL/Unicode), AC-24 (DSK persistence). Prerequisites: Agent Prompts 01–07 merged — 07 shipped the overlay level this prompt composites onto (`main` @ `f626c9a`).**

You are a senior Rust engineer building the `nbe` broadcast engine. This prompt builds the graphics layer: text and templates rendered to GPU textures, the scrolling ticker, lower thirds, the breaking banner, and the clock. This is where the View starts looking like a news network.

Read these first:

- `docs/spec.v0.4.md` — §6.5's text rules are the heart of this prompt: GPU texture scroll, no per-frame relayout, Unicode/RTL, packaged fonts.
- `agents/prompts/07b-delta.md` — what the overlay level already provides, with file:line evidence.
- `agents/prompts/07-overlay-level.md` — the level you are compositing onto.
- `crates/nbe-engine/tests/prompt07_overlay.rs` — the golden-frame harness to copy.
- `VOCABULARY.md` — term ledger.

## Quality bar

Complies with the NBE Implementation Standards (`docs/implementation-standards.md`). Specifically:

- **Schema-driven typed models (§1):** the template/clock/TextDirection typed model must be round-trip tested and enum-audited against the `GraphicTemplate`/`TemplateField`/`ClockConfig` schema definitions.
- **Strict CI contracts (§2):** every new observable behaviour gets an exact gate. The `prompt07_overlay` gate floor is **13** today; raise it when you add tests, or the gate stops being a gate.
- **Falsification (§2a):** every claimed behaviour needs a test that fails without it. Commit before falsifying; `git reset --hard HEAD` is the restore; rebuild before re-running; evidence pastes are complete (rule 5); commit messages name what they carry (rule 6).
- **Prompt structure (§3):** Forbidden changes, new tests, and CI changes are listed explicitly below.

---

## Step 0 — The As-Built Ledger

What exists, where it lives, and what this prompt must add. Every row is read from the tree, not from a report.

| Concern | Implemented in | State for 07b |
|---|---|---|
| Composition seam | `nbe-engine/src/scene.rs:460` `resolve_overlay()` → `:532` `layer_for()`; `render.rs:517` `draw_for()` | **Present.** Overlay elements resolve through the *same* walk as scene elements. Text is a new `LayerSource`, not a new pipeline. Do not add a parallel walk — that unification took five attempts on the scene side. |
| Overlay indexing, z-sorted | `scene.rs:101` (`overlays` map), `:285` (`sort_by_key(|e| e.z)`) | **Present**, same shape as scenes (`:265`). |
| Overlay directives §16.6 | `directive.rs:98`, `:410`; `protocol.ts:225-226`; `commands/state.ts:9,32` | **Present end to end.** 07b adds **no** overlay commands. It wires §16.5, §16.7, §16.13. |
| Overlay animation | `render.rs` `overlay_alpha()` — linear ramp, pure function of the master clock | **Present, and deliberately reduced.** Linear alpha only; declared `easing` and `delayFrames` are accepted on the wire and unread. See Constraints. |
| Fallback above overlays | `render.rs` — `show_fallback` gates the whole scene+overlay branch | **Present**, and now text-derived: `docs/industry-gap-analysis-and-z-axis.md` §3.4 states the rule. A ticker is covered by a fallback cut. |
| Preflight overlay checks | `nbe-preflight/src/main.rs:492` (`duplicateOverlay` `:545`, `overlayAsset` `:558`, `overlayTemplate` `:566`) | **Present.** |
| Preflight font check | `nbe-preflight/src/main.rs:514-525` — `fontAssetIds` on a template must resolve | **Present.** The old Step 6 test 6 (missing font fails preflight) is already wired; 07b owes the *positive* case. |
| Resync §5.9.4 | `state.ts:326,375`; `directive.rs:536-548` | **Present.** Resynced overlays land steady, not animating. |
| Golden-frame harness | `tests/prompt07_overlay.rs:450` `render_engine()`, `:474` `px_at()`, `readback_view()` | **Present.** 13 tests: 2 index, 6 directive, 5 render. Copy this pattern. |
| **Glyph rasterization** | **does not exist** | **This prompt's mission.** |
| **Template engine** | **does not exist** | **This prompt's mission.** |
| **Ticker / clock / breakingBanner element kinds** | **do not exist** | **This prompt's mission.** |
| **A packaged font asset** | **does not exist anywhere in the tree** | **Blocking. See Step 0b.** |

### Step 0b — The asset debt, now blocking

07b cannot be tested on the assets the tree has.

- **No font asset exists.** `find tests/fixtures -iname "*.ttf" -o -iname "*.otf"` returns nothing, and no fixture manifest declares `"kind": "font"`. A **license-clean packaged font** must be committed as a real asset. The licence matters because fonts ship *inside* show packages.
- **The `overlay_show` media files are not images.** Both are 57-byte UTF-8 text stubs: `tests/fixtures/overlay_show/media/logo.png` and `media/fallback.png`, and both carry the same string — `PLACEHOLDER — fallback slate image (SPEC section 6.9).` — so `logo.png` is mislabeled as well as absent. The existing render suite passes only because it asserts against solid graphic fills. Glyph asserts have no such escape.
- **Consequence:** replacing the stubs changes pixels the current tests assert on. Expect to re-baseline `fallback_covers_overlays` and any test sampling a region backed by `logo.png`. Do this deliberately, in its own commit, with the re-baselined values quoted.

## Step 1 — The text pipeline

- `glyphon`/cosmic-text on wgpu, fonts loaded **only** from the show package's font assets.
- Shape once per content change; rasterize to cached textures. A glyph atlas or SDF cache handles scaling.
- Unicode and UTF-8 throughout; RTL scripts correct; multilingual fields correct — the shaping engine must **prove** it, not assume it.
- The rasterized texture becomes a `LayerSource` consumed by the existing `draw_for()`. No second draw path.

## Step 2 — Templates

- The five §6.5 template classes — `lowerThirdHeadline`, `lowerThirdName`, `breakingBanner`, `ticker`, `clock` — as typed JSON layouts per the `GraphicTemplate` definition.
- **Open question this prompt must close, not inherit — and it is not the question the delta addendum first stated.** `templates/graphics/` **does exist**, and has since the founding scaffold (`7704eb8`). It holds one tracked file, `README.md`, reading: *"Template JSON layouts plus packaged font assets. Rendered to GPU textures by `nbe-engine` (SDF/Skia-class text rendering). There is no HTML/browser render path in the engine."*

  So the real tension is **scaffold-intent versus package-resident**. The scaffold put template layouts *and packaged fonts* in the repo at `templates/graphics/`. The implemented resolution reads `templateId` from the show package (`main.rs:566`), and `fontAssetIds` resolves against assets the manifest declares (`main.rs:514-525`) — i.e. package-resident. Both readings are live in the tree right now, one as a README and one as code.

  **This bears directly on Step 0b's font-asset requirement:** repo-resident per the scaffold, package-resident per the package model, and the answer decides where the license-clean font is committed and how preflight sees it. Decide it in this prompt's execution — with the user, since the scaffold's intent is theirs — and write the decision down. Do not leave both readings open, and do not resolve it by deleting the README.
- Wire the §16.5 commands: `graphic.show`, `graphic.hide`, `graphic.update`. Fields are editable live; the element re-lays out once on update and holds via texture otherwise.

## Step 3 — The ticker

- Scrolls by texture offset, driven by the master clock: scroll position is a **pure function of `(masterFrame, speedPxPerFrame)`**. Deterministic, drift-free, free at frame time. This mirrors `overlay_alpha`'s discipline exactly — alpha there is a pure function of the master clock, and the same rule governs scroll here.
- Sources: manual items, RSS, scheduled items, breaking override. Ordering per §16.7: breaking first, then priority, then insertion order; `language` is metadata.
- Behaviours: scroll, pause, resume, priority insertion, edit live, multilingual.
- It lives on the overlay level. **The persistence mechanism is already built and falsified** — `overlay_persists_across_take` proves pixel-identity across a 15-frame mix, and `animation_immune_to_take` proves a take does not re-key an animation in flight. AC-24's remaining half is ticker-specific: prove the **scroll offset** is unperturbed by a take. That is a new assert on existing machinery, not new machinery.

## Step 4 — The clock

- The `clock` element with `ClockConfig`: `wall` | `showElapsed`, timezone, format, locale, `blinkColon`.
- `showElapsed` reads the master clock — **never** wall time. `clock.configure` per §16.13.

## Step 5 — RSS from the control plane

- The control plane fetches RSS asynchronously — never the render loop (§7.13) — sanitizes items to plain display text (assumption 13), rate-limits injection (§10.7 item 2), and pushes items with `ticker.override`.
- `ticker.refreshRss` refreshes the cache; on feed failure the last cached items or manual items keep scrolling (§9.5).

## Step 6 — Tests

Headless, on the render loop, in the `prompt07_overlay.rs` pattern (or a sibling `prompt07b_graphics.rs` — if you add a target, add its own anchored CI gate with a floor).

1. **AC-15**: English LTR, Arabic RTL, Spanish accented text, and emoji if the packaged font supports it — all correct; scrolling holds the target frame rate.
2. **Texture discipline**: a content change triggers exactly one relayout; a second of scrolling triggers zero. Assert layout **call counts**, not timings.
3. **Ordering**: breaking override first, priority ordering, insertion-order tiebreak.
4. **Clock**: `showElapsed` matches the master clock at known frames; `blinkColon` blinks on the beat.
5. **Ticker persistence**: the ticker's scroll offset is unperturbed by a cut and by a mix. (The generic overlay-persistence case is already covered — do not duplicate it; a duplicate cannot fail for a new reason.)
6. **Preflight, positive case**: a manifest declaring a real packaged font and a template referencing it passes preflight and the font rasterizes. The negative case (missing font) is already wired at `main.rs:514-525` — assert it still fails, but do not re-implement it.

CI: the existing `rust` job covers this, headless with pixel readback on macos-14. `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` all pass. Raise the `prompt07_overlay` floor, or add a floor for a new target.

---

## Constraints

**Forbidden:**

- No host-system font fallback (assumption 11). No HTML/browser rendering (the rejected path). No RSS fetching inside the engine. No per-frame text relayout (§6.5).
- **Do not widen overlay animation.** Animations remain **linear alpha ramps**, duration-honoured, with declared `easing` and `delayFrames` unread and no positional enter/exit (Step 5b records entry c). The extended easing families proposed in `docs/move-parity-and-virtual-set-roadmap.md` §D2 are a **v0.5 spec change**, not 07b work. A ticker that wants an eased entrance is asking for a spec revision.
- **Do not composite on Preview.** Overlays are View-bus only; Preview semantics are spec-silent and no rule was invented (`render.rs`, the `bus == Bus::View` guard). Do not invent one.
- **Do not add an exit-time animation override.** `overlay.hide` is `strict({ overlayId })` and its handler forwards `payload: {}`; the engine's hide-side duration read is wire-unreachable. Making it expressible is a §16.6 change for the next revision.
- Do not touch the composition seam's single walk, the frame contract, or P1 preflight behaviours.
- D1–D7 from the move-parity roadmap remain out of scope. Per-source envelopes and the `sfx` ramp-advance invariant stay with the audio work.

**Required:**

- `anyhow` for binaries, `thiserror` for library errors.
- Vocabulary discipline: `View`, `Element`, `Sequence`, `Item`.

## Definition of done

Per Standards §5, plus:

- The As-Built Ledger's four "does not exist" rows are implemented, or explicitly re-deferred **in writing with a named owner**.
- The font asset is committed, license-clean, and its licence recorded.
- The placeholder-PNG re-baseline is its own commit with the before/after values quoted.
- The `templates/graphics/` question — scaffold-intent versus package-resident, and therefore where the font asset lives — is closed in this document, not left to the reader.
- Falsification table in the report: behaviour removed → test that failed, complete pastes.
- CI summary lines quoted verbatim from the runners, per Standards §2b.
