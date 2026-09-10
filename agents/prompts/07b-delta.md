# Agent Prompt 07b — Delta Addendum

Addendum to `agents/prompts/07b-graphics-templates.md`, written after Prompt 07 shipped the overlay level. 07b was recovered verbatim from `26bf2f55^`, before the level existed; this document records what the tree now provides, what 07b says that is no longer true, and what 07b still has to build.

**It changes nothing in 07b itself.** 07b keeps its scope decisions and its constraints. This addendum is the upgrade pass's input, not the upgrade pass.

**Targets: SPEC v0.4 (`docs/spec.v0.4.md`) — §6.5 (graphics and fonts, line 690), §7.10 (overlay level, line 981), §7.13 (frame budget, line 1039), §16.5 (element/graphic commands, line 2558), §16.6 (overlay commands, line 2570), §16.7 (ticker commands, line 2577), §16.13 (clock commands, line 2706), AC-15, AC-24.**

Read alongside:

- `agents/prompts/07b-graphics-templates.md` — the base prompt this amends.
- `agents/prompts/07-overlay-level.md` — the level 07b composites onto.
- `docs/prompt-map-07-13.md` — the Step 5 and Step 5b records; entry c and Q2 are load-bearing here.
- `docs/implementation-standards.md` — the Standards, including §2a rule 6.

---

## 1. Already built — 07b inherits, and must not rebuild

Every row is evidence from the tree, not a summary of a report.

### 1.1 The composition seam

Overlay elements resolve through **the same `layer_for` walk as scene elements**. There is no parallel overlay resolver to keep in step.

- `crates/nbe-engine/src/scene.rs:460` — `resolve_overlay()` maps the overlay's `ElementSpec`s through `self.layer_for(e)`.
- `crates/nbe-engine/src/scene.rs:532` — `layer_for()`, the one walk, shared with `resolve()` at `:472`.
- `crates/nbe-engine/src/render.rs:517` — `draw_for()`, the single draw path both take.

**What this means for 07b:** a ticker, clock, or breaking banner hosted on the overlay level becomes drawable by adding an `ElementSpec` kind that `layer_for` understands. Text rendering is a new `LayerSource`, not a new pipeline.

### 1.2 Z-sorted overlay indexing

- `crates/nbe-engine/src/scene.rs:101` — `overlays: HashMap<String, Vec<ElementSpec>>`, documented "already sorted low-z to high-z (§7.10). Same `ElementSpec` shape as scenes."
- `crates/nbe-engine/src/scene.rs:285` — `elements.sort_by_key(|e| e.z)` at overlay index build, the same call the scene index makes at `:265`.

### 1.3 The directive surface (§16.6)

Built end to end; 07b adds no overlay commands.

- `crates/nbe-engine/src/directive.rs:98` — `"overlay.show" | "overlay.hide" => self.on_overlay(d)?`.
- `crates/nbe-engine/src/directive.rs:410` onward — next-frame-boundary application (`anim_start = master_frame + 1`), package-declared enter/exit bounds, `payload.animation.durationFrames` override.
- `packages/control-plane/src/protocol.ts:225-226` — the wire schemas.
- `packages/control-plane/src/commands/state.ts:9,32` — handlers, idempotent no-ops, `extraDirectives`.

### 1.4 Preflight overlay and font checks

- `crates/nbe-preflight/src/main.rs:492` — the §7.10 block: overlay ID uniqueness (`duplicateOverlay`, `:545`), element asset references (`overlayAsset`, `:558`), template references (`overlayTemplate`, `:566`).
- `crates/nbe-preflight/src/main.rs:514-525` — **`fontAssetIds` on a template must resolve to a declared asset.** 07b's Step 6 test 6 ("a manifest referencing a missing packaged font fails preflight") is therefore already wired; 07b's remaining work is the *positive* case — a declared font that actually rasterizes.

### 1.5 The v0.4 §5.9.4 resync semantics

- `packages/control-plane/src/state.ts:326-327` — the snapshot carries `visibleOverlays` and the derived `overlays[]`.
- `packages/control-plane/src/state.ts:375` — a present array replaces the on-air set wholesale.
- `crates/nbe-engine/src/directive.rs:536-548` — the engine clears and re-inserts; resynced overlays land **steady**, not animating ("authoritative state, not an animation").

### 1.6 The golden-frame harness pattern

`crates/nbe-engine/tests/prompt07_overlay.rs` is the pattern to copy, not to reinvent.

- `:450` — `render_engine()` builds a real `RenderLoop` headless on wgpu.
- `:474` — `px_at(bytes, fx, fy)` samples a fractional coordinate.
- `:521` etc. — `render.readback_view().await` reads the View back for pixel-exact asserts.

Thirteen tests: 2 index, 6 directive, 5 render. The CI gate has a **floor of 13** (`.github/workflows/ci.yml`), so 07b must raise the floor when it adds tests, or the gate stops being a gate.

---

## 2. Stale in 07b as written

| 07b says | Correction |
|---|---|
| **"Targets: SPEC v0.3.2 (`docs/spec.v0.3.md`)"** (header) | Target **SPEC v0.4** (`docs/spec.v0.4.md`). v0.3 remains in `docs/` as history only. This is the single largest correction: v0.4 changed §7.15, §12.11, §5.9.4, §10.1 and retired `sequenceRef`. |
| **"Prerequisites: Agent Prompts 01–06 merged"** | Add **07** (the overlay level). 07b composites onto the level 07 builds; without it there is no host for a ticker. |
| **"`agents/prompts/04-basic-compositor.md` — the render loop and overlay level you are rendering into"** | 04 built the render loop; it **deferred** the overlay level to 07, which built it. Read `07-overlay-level.md` and `prompt07_overlay.rs` for the level. |
| **Step 0's inventory** | Predates the engine. Re-take against the tree. 07b's own note already says this; it is repeated here because Step 0 is the step most likely to be skimmed. |
| **§16.6 overlay commands listed among 07b's wiring** | Already built (§1.3 above). 07b wires §16.5 (`graphic.*`), §16.7 (`ticker.*`) and §16.13 (`clock.configure`) only. |
| **Step 3: "It lives on the overlay level — it persists across scene transitions, untouched (AC-24's basic form lands here)"** | The **persistence mechanism** is built and falsified: `overlay_persists_across_take` proves pixel-identity across a 15-frame mix, and `animation_immune_to_take` proves a take does not re-key an animation in flight. AC-24's remaining half is ticker-specific — that a *scrolling* element's scroll position is unperturbed — which is a new assert on existing machinery, not new machinery. |
| **Step 6 test 5: "the ticker survives a scene cut and a scene mix untouched"** | Narrow it to the ticker's own scroll offset. The generic overlay case is covered; duplicating it adds a test that cannot fail for a new reason. |
| **Step 6 test 6: missing packaged font fails preflight** | Already wired (`main.rs:514-525`). Replace with the positive case. |
| **Step 2: "as JSON layouts in `templates/graphics/`"** | **Correction (2026-09-10): this row was wrong.** It said no such directory exists. It does, and has since the founding scaffold `7704eb8` — `templates/graphics/README.md`, tracked, reading *"Template JSON layouts plus packaged font assets."* The real tension is scaffold-intent (templates **and fonts** repo-resident) versus the implemented resolution (`templateId` from the show package, `main.rs:566`; `fontAssetIds` against declared assets, `main.rs:514-525` — package-resident). It bears on the font-asset requirement, since the two readings put the font in different places. Decide in the upgrade pass with the user; do not resolve it by deleting the README. |

**Two claims in the step-6 close-out prompt that the tree does not support**, recorded here so they are not carried forward:

- There is **no section-numbering drift for the overlay level.** The close-out prompt described "the prompt-era §7.4 vs the spec's actual overlay section". 07b contains no `§7.4` reference at all, and its `7.10 (overlay level)` citation matches v0.4's `## 7.10 Overlay level (DSK)` (`docs/spec.v0.4.md:981`) exactly. v0.4's §7.4 is "Element identity and state model", unrelated. Nothing to correct.
- **No scope was absorbed away from 07b by the overlay level.** The level took §16.6 and the composition seam, both of which 07b only ever assumed as substrate. Glyph rasterization, templates, the ticker, the clock and RSS were 07b's before and remain 07b's.

---

## 3. What 07b must actually build

Unchanged in substance from 07b Steps 1–5; restated against the level that now exists.

1. **Glyph rasterization**, hosted **on the overlay level** — `ticker`, `clock`, `breakingBanner` as overlay element kinds, resolved through `layer_for` (§1.1). The step-5 records answer Q2 explicitly: the overlay level makes no rasterization decisions and 07b owns the text stack.
2. **The template engine** — the five §6.5 classes with typed fields, round-trip tested and enum-audited per Standards §1.
3. **The text stack** — `glyphon`/cosmic-text on wgpu, shaping once per content change, cached textures, Unicode and RTL correct (AC-15).
4. **Layout and rasterization strictly off the frame path** (§7.13, `docs/spec.v0.4.md:1039`). The ticker's scroll must stay a pure function of `(masterFrame, speedPxPerFrame)` — the same discipline `overlay_alpha` follows, where alpha is a pure function of the master clock.
5. **Within the packaged-font rule.** `docs/portability.md` row 7: §0.1 assumption 11 forbids host-system fonts; fonts are packaged. Row 7 calls this "a portability asset, not a risk" and says 07 must not weaken it. **Do not weaken it.** Weakening is a spec change, not a prompt change.

---

## 4. 07b's fixtures need assets that do not exist

This is the recorded placeholder-PNG debt becoming blocking. Glyph asserts cannot run on placeholder pixels.

- **No font asset exists anywhere in the tree.** `find tests/fixtures -iname "*.ttf" -o -iname "*.otf"` returns nothing, and no fixture manifest declares `"kind": "font"`. 07b needs a **license-clean packaged font** committed as a real asset — the licence matters because the font ships inside show packages.
- **The `overlay_show` media files are not images.** Both are 57-byte UTF-8 text stubs:
  - `tests/fixtures/overlay_show/media/logo.png` — `PLACEHOLDER — fallback slate image (SPEC section 6.9).`
  - `tests/fixtures/overlay_show/media/fallback.png` — the same string.

  Note the second defect: `logo.png` carries the *fallback slate* text, so the placeholder is mislabeled as well as absent. The render-proof suite works around both by asserting against solid graphic fills, which is why 13 tests pass on stub assets today. Text rendering has no such escape.
- **Consequence for the CI floor:** replacing the stubs changes pixels the existing render tests assert on. Expect to re-baseline `fallback_covers_overlays` and any test sampling a region backed by `logo.png`.

---

## 5. Constraints 07b inherits

- **D1–D7 remain out of scope.** Nothing in 07b reopens them.
- **Per-source envelopes and the `sfx` ramp-advance invariant stay with the audio work** — §8.7.5's true crossfade is not 07b's, even though 07 inherited it as a promoted deferral.
- **Overlay animations remain linear alpha ramps.** Step 5b records entry c: duration-honoured, declared `easing` and `delayFrames` accepted on the wire and unread, no positional enter/exit. A ticker that wants an eased entrance is asking for a spec change.
- **Overlays composite on the View bus only.** Preview semantics are spec-silent and no rule was invented (`render.rs` — the `bus == Bus::View` guard). 07b must not invent one either.
- **The animation override is show-only end-to-end** (records entry c): `overlay.hide` is `strict({ overlayId })` and its handler forwards `payload: {}`. An exit-time override is a §16.6 change for the next spec revision.
- **No host fonts, no HTML/browser rendering, no RSS in the engine, no per-frame relayout** — 07b's own constraints, unchanged and unweakened.
