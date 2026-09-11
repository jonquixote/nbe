# Overlay Level (DSK) Implementation Plan — P7 Step 5

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:executing-plans (inline, this session). Steps use checkbox (`- [ ]`) syntax.

**Goal:** Implement `View = overlay(transition(sceneA, sceneB))` per SPEC v0.3 §7.10 (prompt numbers it §7.4 — §7.4 is Element identity; the prompt's number is an alias) with `overlay.show`/`overlay.hide`, persistence across takes, master-clock-keyed animations, fallback-above-overlays, preflight checks, golden-frame and control-plane proof.

**Architecture:** Engine: index overlays in `PackageIndex`, track per-overlay on-air + animation timeline in `EngineState`, composite overlay layers after transition output in `render.rs`, fallback slate above everything. Control plane: harden existing `overlay.show`/`hide` handlers (idempotent no-op + `data.noop`), dual-write `visibleOverlays` + `overlays[]{id,onAir,animationState}` in snapshot, keep generic dispatch bump/audit/forward. Preflight (`nbe-preflight`): overlay-ID uniqueness + shared Element reference checks.

**Tech Stack:** Rust (nbe-engine, nbe-core, nbe-preflight, wgpu golden frames), TypeScript control plane (zod, MockRenderBridge, node:test), JSON schema v0.3 (read-only).

## Global Constraints

- No schema edits — `schemas/*.json` untouchable.
- No research deltas: no bounded/named slots, no tieToTransition, no clean feed, no reserved z-bands, none of D1-D7.
- Do not change transition-engine semantics, state-diff precompute path, or 2-frame take-latency (AC-17).
- Do not re-implement/modify ticker/graphic/clock renderers except to host them in overlay level.
- No changes to the F3 stale-report guard, the bound machinery, or the audit writer.
- Normative vocabulary: View/Element/Sequence/Item — never Program/Layer/Segment outside migration code and tests.
- `unsafe` stays in `crates/nbe-decode` (CI-gated).
- Evidence pastes complete — no truncating pipes (§2a rule 5). Commit before falsifying; restore with `git reset --hard HEAD`; rebuild from restored tree before measuring.

## Verified baseline

- Head `d0c696fe5e671b5e10edf6be2186e9f22a0616c0`, branch `P7-overlay-level`, origin in sync, tree clean.
- Schema: `Overlay {id, elements: Element[]}`, top-level `overlays` (`schemas/manifest.v0.3.json:56`, `$defs/Overlay:1135`).
- `Animation {durationFrames?, delayFrames, easing, bezier?}` + `enter/exitAnimation` (`nbe-core/src/manifest.rs:634-677`).
- Render: `Transition{progress,is_complete}`, `render_bus` + `fail_view` seam, `FrameReport` deadline path. `drawn_elements` is the single walk.
- Control plane: `overlay.show {overlayId, animation?: string}` / `overlay.hide {overlayId}` (`protocol.ts:223-224`), operator role (`dispatch.ts:117-118`), plain add/delete handlers (`commands/state.ts:9-27`), `visibleOverlays: Set<string>` (`state.ts:146`), generic bump/audit/forward (`dispatch.ts:317-345`).
- Audit `kind: "command"|"auth"|"preflight"` only (`audit.ts:20`). The `automation` kind exists in a comment, not the type — not used here.
- No element tween / animation-state machinery exists in `nbe-engine/src` — this plan builds the composition-level minimum (start frame + durationFrames + easing → alpha). D1-D7 stay out.
- Ticker/clock/breaking glyph rasterization is 07b scope. Overlay hosts `layer_for` as-is: `graphic → Solid`, `clip/videoLoop → Image/Video`; `ticker`/`clock`/`camera`/`guest`/`sceneRef`/`group`/`plugin` resolve via the same path and draw nothing until 07b. There is no `breaking` element kind; `breakingBanner` is a template kind — the banner enters through `graphic`.
- Both research docs (`industry-gap-analysis-and-z-axis.md`, `move-parity-and-virtual-set-roadmap.md`) were absent from the repo when this plan was written; the §3.3 clarification was authored from the prompt's own FTB-above-DSK sentence and recorded as prompt-derived. **Both landed in `docs/` on 2026-09-10, and gap analysis §3.4 states the rule verbatim — the clarification is text-derived after all.**

## Task 0 — Answers in writing (before code)

Append to `docs/prompt-map-07-13.md`, one entry under Prompt 07:

- Q1 directive surface: §16.6 already defines `overlay.show`/`hide`; engine receipt behavior is: apply at the next frame boundary (`anim_start = master_frame + 1`), idempotent no-op when already in the requested state, no transition interaction.
- Q2 ticker text stack: owned by 07b. The overlay level guarantees only that layout/rasterization happens off the frame path — elements resolve to layers exactly as scene elements do.
- Q3 Preview semantics: spec silent → no rule is invented. Overlays do not composite on Preview. Recorded as a v0.4 candidate.
- Q4 persistence identity: an overlay is identified by its manifest `id`; its pixels are untouched by a take; `visibleOverlays` in §5.9.4 is a full snapshot — an empty array clears all on-air overlays.
- Idempotency assumption (spec silent): show-on-air / hide-on-hidden succeed as idempotent no-ops (`data.noop: true`), noted in telemetry/change log.
- Fallback clarification (SPEC §7.14 — §6.9 in prompt numbering — is silent on overlays): the fallback slate composites ABOVE the overlay level; a fallback cut covers ticker, bug, banner, clock; recovery restores the pre-fallback on-air set. Input to the next spec revision.
- `animation?: string` on `overlay.show` is opaque to the control plane, forwarded to the engine; the engine maps it where possible and otherwise uses the element's declared enter/exit durations. Unknown strings are ignored (best-effort), matching `view.take`'s preset leniency.
- Per-source envelopes and the `sfx` ramp invariant: out of scope for this step; owned by the audio/per-source-envelope work. Recorded as a deferred item.

## Task 1 — Engine: index overlays + overlay runtime state

**Files:**
- Modify: `crates/nbe-engine/src/scene.rs` (index `overlays`, add `overlay_alpha`, export `AnimationSpec`-ish parsing inline)
- Modify: `crates/nbe-engine/src/state.rs` (`overlays: Mutex<BTreeMap<String, OverlayRuntime>>`, extend `FrameSnapshot`)
- Test: `crates/nbe-engine/tests/prompt07_overlay.rs` (new)

**Interfaces:**
- `PackageIndex.overlays: HashMap<String, Vec<ElementSpec>>` (z-sorted), reusing `element_spec` + `layer_for`.
- `OverlayRuntime { on_air: bool, anim_start: u64, duration_frames: u64, direction: OverlayPhase }` where `OverlayPhase = Enter | Exit | Steady`.
- `FrameSnapshot` gains no overlay field yet — `overlay_draws` is computed in `RenderLoop` from `state.overlays` + package in Task 3. Task 1 ships index + state only.

- [ ] Step 1: failing test `package_index_holds_overlays_sorted_by_z` (graphic solids at z 9 and 2 → order [2, 9]).
- [ ] Step 2: run → FAIL (no `overlays` field).
- [ ] Step 3: implement `PackageIndex.overlays` (mirror the `scenes` loop), `OverlayRuntime` + `overlays` map in `EngineState`.
- [ ] Step 4: run → PASS. Step 5: commit.

## Task 2 — Engine: directives + resync clear

**Files:** `crates/nbe-engine/src/directive.rs`; tests in `prompt07_overlay.rs`.

- Match arms `"overlay.show" | "overlay.hide"` before the `RESYNC` arm (currently they fall to the ignore branch at `:101-103`).
- `on_overlay`: unknown overlay id in state → no-op (control plane validated; engine is lenient per its directive role). `overlay.show`: if not on air, set `on_air = true`, `anim_start = master_frame.unwrap_or(0) + 1`, `duration_frames` from the element's `enterAnimation.durationFrames` (else 1, effectively instant-over-one-frame), `direction = Enter`; the plain string `animation` payload is forwarded but maps only to "default" for now. If already on air: no-op. `overlay.hide`: mirror with `exitAnimation`.
- `on_resync`: after the previewItem block, replace the on-air overlay set from `visibleOverlays` — present array (including empty) replaces wholesale per v0.4 §5.9.4; absent key leaves the set alone. Consistency with the v0.4 snapshot is implemented ahead of the normative sentence by mandate of this prompt's §5.9.4 note.
- The take path and `transition` state are untouched by overlay directives.

- [ ] Step 1: failing tests — `overlay_show_keys_off_next_frame_boundary`, `overlay_show_is_idempotent`, `resync_with_empty_visibleOverlays_clears_onair`.
- [ ] Step 2: run → FAIL. Step 3: implement. Step 4: run → PASS. Step 5: commit.

## Task 3 — Render: DSK composite + fallback above overlay

**Files:** `crates/nbe-engine/src/render.rs`; tests in `prompt07_overlay.rs` (headless wgpu + `readback_view`, pattern from `prompt04.rs`).

- In `render_bus` (View path only): after the transition/scene draws, for each on-air overlay (elements already z-sorted) push `draw_for(layer, overlay_alpha, frame, t0 = anim_start)` where `overlay_alpha` is Enter→`min(1, (frame - anim_start + 1) / duration)`, Steady→1.0, Exit→`1 - progress`. Released when exit completes and `direction = Exit` → element drops (state mutation happens at render boundary: completed Exit marks `on_air = false`; mutation is via the state lock, not a raw frame-path write).
- `show_fallback == true` → draw the slate only (covers overlays; §3.3 clarification).
- Preview bus: unchanged (Q3).

Tests (all golden-frame via a small temp-dir package with distinct colors):
- [ ] 1 `overlay_persists_across_take` — overlay region pixel-identical at mix start/mid/end.
- [ ] 2 `overlay_composites_above_transition` — mid-mix, overlap region shows overlay color over both scene colors.
- [ ] 3 `show_animation_timing` — show at frame F: overlay invisible at F, alpha > 0 at F+1 when direction=Enter, Steady (alpha 1) by F+duration.
- [ ] 4 `animation_immune_to_take` — take issued mid-enter; the overlay completes on its original timeline (frame indexes unchanged).
- [ ] 5 `fallback_covers_overlays` — all overlays on air → `view.fallback` → frame equals the slate; clear fallback → overlays back.

- [ ] Step 1: failing tests. Step 2: run → FAIL. Step 3: implement. Step 4: run → PASS. Step 5: commit.

## Task 4 — Control plane: idempotency + snapshot overlays[]

**Files:** `packages/control-plane/src/commands/state.ts`, `state.ts`, `telemetry.ts`; new `src/overlay.test.ts`.

- `overlay.show` on an on-air overlay → `{ data: { noop: true } }`; still dispatched (bump/audit/forward unchanged — accepted commands are commands; the idempotency is in the state mutation, not the accounting). Alias rule: hide on hidden mirrors it.
- `visibleOverlays.add/delete` unchanged; add `overlayAnimation: Map<string, { state: "enter"|"exit"|"steady"; atStateVersion: number }>` updated on non-noop show/hide.
- `resyncSnapshot` gains `overlays: Array<{ id, onAir, animationState }>` while keeping `visibleOverlays` (dual-write, one-version bridge; the empty-array-clears rule reads `visibleOverlays`). `saveSnapshot`/`recallSnapshot` keep working from `visibleOverlays` + copy `overlayAnimation`.
- `statusSnapshot` unchanged (out of scope; §10.4's seven things do not include overlay detail).
- Tests: ENOTFOUND path, idempotent noop data + stateVersion still bumps + audit `kind: "command"` record written, snapshot contents, operator-pass/monitor-deny.

- [ ] Steps: failing tests → implement → pass → commit.

## Task 5 — Preflight: overlay uniqueness + references

**Files:** `crates/nbe-preflight/src/main.rs`; tests `crates/nbe-preflight/tests/overlay.rs` (new).

- Duplicate overlay ids → `duplicateOverlay: "<id>"` error, exit 2 (airReady false).
- Each overlay element's `assetId` must resolve to a declared asset; `templateId` to a declared template; `fontAssetIds` (via template) ⊆ assets; `sceneRef` to a declared scene. Schema-shared Element model — same checks as scene elements, resolved against the already-loaded `manifest_json`.
- Valid overlay fixture → airReady true.

- [ ] Steps: failing tests → implement → pass → commit.

## Task 6 — Fixture + CI + proof + falsification

**Files:** `tests/fixtures/overlay_show/` (new; cloned from `valid_show_v0.3` minimals + a real fallback PNG + 2 scenes + overlays exercising all four slots: `ol_ticker` (graphic band), `ol_bug` (image), `ol_banner` (graphic), `ol_clock` (graphic)); `.github/workflows/ci.yml` additions; `docs/prompt-map-07-13.md` (Task 0 text lives here).

- Rust CI: add a step mirroring the prompt04 gate (anchored sed, fail when zero ran) for `--test prompt07_overlay`. Control-plane job: no new step — the overlay test rides the existing suite gate (which fails on any failure).
- Full proof: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `npm test`, `npx tsc --noEmit`, the two fixture gates (v0.3 airReady true; v0.2 exit 2), release binary size + `--version`.
- Falsification (commit first; `git reset --hard HEAD` to restore; `cargo build` after restore before reruns):
  1. Clear overlays on take → test 1 fails.
  2. Composite overlays before the transition → test 2 fails.
  3. Key overlay animation to the transition clock instead of the master clock → test 4 fails.
  4. Fallback skips the overlay level (draw overlays under/over slate) → test 5 fails.

- [ ] Steps: fixture + CI edits, full gates, falsification battery, commit. **Do not push.**

## Report obligations

Finding → fix → test → falsification table; CI verbatim summary lines; records quoted; head SHA and branch; no commit/push without explicit instruction.
