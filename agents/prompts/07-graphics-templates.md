# Agent Prompt 07 — Graphics: Ticker & Lower-Third Templates (crates/nbe-engine)

**Targets: SPEC v0.3.1 (`docs/spec.v0.3.md`) — Sections 6.5 (graphics and fonts), 7.10 (overlay level), 10.7 (RSS sanitize/rate-limit), 16.5 (graphic commands), 16.6 (overlay commands), 16.7 (ticker commands), 16.13 (clock.configure), AC-15 (RTL/Unicode), AC-24 (overlay persistence). Prerequisites: Agent Prompts 01–06 merged (compositor rendering video on the master clock, audio graph live).**

You are a senior Rust engineer building the `nbe` broadcast engine. This prompt builds the graphics layer: text and templates rendered to GPU textures, the scrolling ticker, lower thirds, the breaking banner, and the clock. This is where the View starts looking like a news network.

Read these first:

- `docs/spec.v0.3.md` — Section 6.5's text rules are the heart of this prompt: GPU texture scroll, no per-frame relayout, Unicode/RTL, packaged fonts.
- `agents/prompts/04-basic-compositor.md` — the render loop and overlay level you are rendering into.
- `VOCABULARY.md` — term ledger.

## Step 0: Scope discipline

Allowed now: `glyphon` (cosmic-text shaping) on `wgpu`, packaged font assets (`kind: font`, ttf/otf) from the show package. Forbidden: host-system font fallback (Assumption 11), per-frame text relayout (Section 6.5), HTML/browser rendering (the rejected path), RSS fetching inside the engine (that is the control plane's job).

## Step 1: The text pipeline

- glyphon/cosmic-text on wgpu, with fonts loaded only from the show package's font assets.
- Shape once per content change; rasterize to cached textures. A glyph atlas or SDF cache handles scaling.
- Unicode and UTF-8 throughout, RTL scripts correct, multilingual fields correct — the shaping engine must prove it, not assume it.

## Step 2: Templates

- The five required template classes from Section 6.5 — `lowerThirdHeadline`, `lowerThirdName`, `breakingBanner`, `ticker`, `clock` — as JSON layouts in `templates/graphics/` with typed fields per the `GraphicTemplate` definition.
- Wire the Section 16.5 commands: `graphic.show`, `graphic.hide`, `graphic.update`. Fields are editable live; the element re-lays out once on update and holds via texture otherwise.

## Step 3: The ticker

The special component, per Sections 6.5 and 7.10:

- Scrolls by texture offset, driven by the master clock: scroll position is a pure function of `(masterFrame, speedPxPerFrame)`. Deterministic, drift-free, and free at frame time.
- Sources: manual items, RSS, scheduled items, breaking override. Ordering per Section 16.7: breaking first, then priority, then insertion order; `language` is metadata.
- Behaviors: scroll, pause, resume, priority insertion, edit live, multilingual.
- It lives on the overlay level — it persists across scene transitions, untouched (AC-24's basic form lands here; move-class persistence is proven when move lands).

## Step 4: The clock

- The `clock` element with `ClockConfig`: `wall` | `showElapsed`, timezone, format, locale, `blinkColon`.
- `showElapsed` reads the master clock — never wall time. `clock.configure` per Section 16.13.

## Step 5: RSS from the control plane

- The control plane fetches RSS asynchronously — never the render loop (Section 7.13) — sanitizes items to plain display text (Assumption 13), rate-limits injection (Section 10.7), and pushes items with `ticker.override`.
- `ticker.refreshRss` refreshes the cache; on feed failure the last cached items or manual items keep scrolling (Section 9.5).

## Step 6: Tests

Headless, on the render loop:

1. **AC-15**: render English LTR, Arabic RTL, Spanish accented text, and emoji (if the packaged font supports it) — all correct; scrolling holds the target frame rate.
2. **Texture discipline**: a content change triggers exactly one relayout; a second of scrolling triggers zero (assert layout call counts).
3. **Ordering**: breaking override first, priority ordering, insertion-order tiebreak.
4. **Clock**: `showElapsed` matches the master clock at known frames; `blinkColon` blinks on the beat.
5. **Overlay persistence**: the ticker survives a scene cut and a scene mix untouched.
6. **Preflight**: a manifest referencing a missing packaged font fails preflight (wire the Section 19 font check).

CI: the existing `rust` job covers this, headless with pixel readback on macos-14. `cargo check --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` all pass.

## Constraints

- No host fonts, no HTML/browser rendering, no RSS in the engine, no per-frame relayout. Text layout happens at content change only.
- `anyhow` for the binary, `thiserror` for library errors.
- Vocabulary discipline: `View`, `Element`, `Sequence`, `Item`.
