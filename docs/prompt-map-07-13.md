# Prompt map 07–13, re-scoped — `[RI-6]`

Prompts 07–13 measured against what the engine **actually is** after P1–P6, not against what the original build order assumed. One paragraph each: what it must now contain, and what it inherits from the deferral ledger.

Produced by the midpoint integration review. Ledger states are in `docs/review-midpoint-report.md` §6; findings referenced as R*n* (rehearsal), F*n* (pass 4), H*n* (hardware), S*n* (`/status`).

Note on numbering: the build order names 14 (packaging) and 15 (contrib outputs) beyond this range. They are untouched by this review and remain as written.

---

## 07 — Graphics and templates (overlays, ticker, clock)

**Inherits three promoted deferrals** — the overlay level itself (§7.10, deferred by P4 naming 07 as owner), §8.7.5's true crossfade via per-source envelopes, and the `sfx` ramp-advance invariant. It also inherits **R1 as P0**: the item→asset audio miss must be at the front of 07's fix list, because a show with no sound is not a foundation to build overlays on. The scope that P4 deferred is unchanged — `View = overlay(transition(A, B))`, overlays composite after the transition and persist across it, and AC-24 (a ticker survives a complex move transition untouched) is the acceptance case. What has changed is the surrounding evidence: 07 now knows the audio graph works in isolation and does not reach a real take, and that per-source envelopes are load-bearing on **both** sides — video overlays and audio crossfade — so they should be designed once. §6.5's forbidding of per-frame relayout and its packaged-font rule are portability assets (`docs/portability.md` row 7) and must not be weakened. R2, R3 and R5 are assigned here too: they are metering and timing work that 07's audio changes will touch anyway.

### Carried into 07 from the v0.4 review passes (recorded 2026-09-06)

Three observations the fourth adversarial pass over `c092f20` reproduced and
deliberately did not file as findings. They are hardening and conformance work,
not defects in what merged, and they are recorded here so they have an owner
rather than fading with the pass that found them.

1. **The `CacheBudget` struct-literal lint has a bypass.**
   `nbe-core/tests/spec_budgets.rs` refuses a `CacheBudget { … }` literal outside
   `loop_cache.rs`, which is how 1024/4096 came to exist beside 256/512. It does
   not refuse `let mut b = index.loop_budget(id); b.per_loop_mib = 1024;` — the
   fields are `pub` on a `Copy` struct, so a divergent budget can exist with no
   literal for the grep to see. The pass wrote that code and it compiled and
   passed. `#[non_exhaustive]` closes cross-crate literals at compile time;
   private fields with accessors close the mutation too. **Hardening, not a
   defect** — no such site exists, and the primary guarantee is structural (two
   consumers, one constructor each).

2. **§12.4's 900-frame conjunct is enforced only in preflight.** §12.4 states
   residency as a conjunction — `periodFrames <= 900` **and**
   `<= maxFramesByBudget` **and** `totalShortLoopCache <= totalBudget`.
   `nbe_core::loop_cache::plan()` implements the second only;
   `ABSOLUTE_LOOP_FRAME_CAP` now lives in that module but is applied by preflight
   alone. An engine run directly on a package with a >900-frame small-resolution
   loop — the dress rehearsal path, which does not gate on preflight — holds it
   VRAM-resident against the spec. Unreachable through the sanctioned path
   (preflight refuses the package by name first), which is why it did not block
   the merge. **07 owns applying the conjunct inside `plan()`,** where the
   constant already sits.

3. **`has_alpha` is derived differently on the two sides — no action.**
   Preflight infers it from `kind == "alphaVideo"`; the engine detects it from
   decoded pixels. It cannot change the engine's selected format while
   `yuv_sampling: false` (both alpha states select RGBA8), so the difference has
   no consequence today. Documented here so a future reader does not rediscover
   it as a bug. It becomes live the moment shader-side YUV lands — the same
   moment §12.3's format ladder does, and the two should be closed together.

### The 46 s `show.load` and the preflight bound shared one root (recorded 2026-09-07)

Step 3d's gate for promoting the dress rehearsal to a required job includes
*"`show.load` stops taking 46 s under contention"*. The spine's second review
pass found, separately, that the control plane's preflight timeout was crossed
by an ordinary package. **They are the same cost seen twice.**

Measured on this repo's `dress_show` fixture, same machine, same run:

| build | `dress_show` preflight | per 1080p frame |
|---|---:|---:|
| debug | 30.05 s | 130 ms |
| release | 3.45 s | 16.7 ms |

The control plane was resolving `target/debug/nbe-preflight`, and CI built it
without `--release`. It now prefers `target/release`, and both jobs that shell
preflight build it that way.

**Disposition: this shrinks 3d, it does not close it.** The 46 s line can now
cite the release binary — a ~9x reduction on the fixture, which takes the cost
out of the flakiness budget entirely. What remains of 3d is unchanged: 3a-3c and
steps 1-2 closing, and green on `macos-14` **twice consecutively**. When 3d is
next revisited, its "46 s addressed" criterion should be rewritten to cite the
release binary rather than an outstanding fix, and the rehearsal's own timing
assumptions re-measured against it — a step that used to wait 46 s for a load
may now be measuring something else entirely.

### Carried into 07 from the spine's own review pass (recorded 2026-09-07)

Two observations the independent pass over `9bd18f9` reproduced and declined to
file. Both are recorded so they have an owner rather than fading with the pass.

1. **`DEFAULT_METER_WINDOW_MS` and `telemetry_interval_ms` are two constants for
   one number.** R2's fix holds peaks across a meter window and publishes on its
   boundary, and the intent is that the window equals the interval a telemetry
   tick reports. That is true only while both constants read 1000:
   `AudioDriver` is constructed with `(state, sink, house_rate)` and has no
   access to `EngineConfig`, so it cannot derive the window from the interval.
   Not filed because the interval is hardcoded in `main.rs` and in
   `EngineConfig::default()` with no env override, so no user can create the
   divergence — but this is the §12.4 budget defect photographed one step before
   it became real, and that one cost three fix rounds. **Whoever makes the
   telemetry interval configurable owns deriving the window from it.**

2. **The meter window rolls on audio-block count, not wall clock.** If the
   driver's cycle rate drifts, window and tick slide relative to each other, so
   a tick reports the most recently *completed* window rather than the interval
   it nominally covers. Acceptable for a peak meter, and recorded in §10.1's
   implementation notes so nobody later asserts tighter timing on `busPeakDbfs`
   in ignorance of it.

### 07 was split into 07 and 07b (recorded 2026-09-05)

The upgrade pass rewrote 07 in place, and what it wrote is a different document
from what it replaced: `agents/prompts/07-overlay-level.md` is the spine (the
As-Built Ledger, R5, R6, the rehearsal's findings, F1/F2, the inherited hang)
plus the overlay level itself. The graphics mission it overwrote —
templates, the ticker, the breaking banner, the clock — is restored verbatim
from `26bf2f55^` as `agents/prompts/07b-graphics-templates.md`.

**The reason for the order: 07b composites onto the overlay level 07 builds.**
`View = overlay(transition(A, B))` has to exist before there is anywhere for a
ticker to survive a transition, so the level comes first and the furniture
second. 07 answers the text-stack question in writing (§5.2 question 2); 07b
implements it.

Suffix ordering follows the `02b`/`02c` precedent, so the series keeps its
numbering: nothing downstream of 07 renumbers. 07b has **not** had its upgrade
pass and gets one before execution — its scope decisions (packaged fonts, no
per-frame relayout, RSS sanitized at the control plane) stand; its Step 0
inventory predates the engine and does not.

## 08 — Companion mapping (elevated to a normative requirement)

Per the v0.4 outline §6, 08 is no longer "wire up a Stream Deck." It builds an **Input Intent schema** — a mapping layer that is *data, not code* — from physical intents (Companion button, MIDI note, keyboard chord) to semantic §16 commands, with per-device profiles as user-editable documents. The §16 command surface with token auth and audit is already the device-independent core (`docs/portability.md`, known-good boundary 1), so 08 adds a layer above it and must not add a second command surface beside it. The proof of generality is normative: a keyboard-shortcut adapter ships in the same prompt and must work with **zero** changes to the core. The Input Intent schema is a wire-level contract and takes normative spec text at 08's moment. Target hardware: StreamDeck XL via Companion.

## 09 — Recording

**Owns `marker.add` → recording chapter (§16.11)**, assigned by `[RI-5]` — 09's current doc does not mention it, and its upgrade pass must. Inherits two dormant deferrals that its own benchmark is the trigger for: zero-copy IOSurface→Metal (re-defer *with numbers*, not with prose) and the display surface. §0.1 assumption 14 fixes fragmented MP4 as the crash-safe default. 09 should also carry `[RI-8]`'s pinned residency policy into its own resource accounting: **unload-at-next-load**, so a stop→start recovery does not pay the 46 s reload measured in the report §3.2.

## 10 — Streaming

Inherits the guest-link JWT / `jti` revocation work (§10.7 #1) assigned by `[RI-5]`, and the TURN credential vending shape (§5.1 #11, §9.6.2) whose response has a schema but no derivation rule. WHEP preview (AC-20) is explicitly **post-v1** and not 10's scope — it waits for a WebRTC stack to exist. The mix-minus guarantee 06 built structurally (§8.6, `render_guest_return` has no path reading a guest's own bus) is 10's to preserve when real guests replace test tones.

## 11 — Watchdog

The watchdog itself exists and is gated (pass 4 confirmed deadline accounting and fallback trip both fail correctly when deleted). What 11 must now add is **the automation engine runtime** (§13, AC-25), assigned by `[RI-5]`: triggers, the once-per-frame limit, runtime cycle suppression, and audit logging of every automation action. `automation.hold` exists from Prompt 02; the engine behind it does not. 11 also inherits **F3's fix** as context — the fix round adds a `fail_view` seam, so §10.3's engagement path finally has production coverage that 11's work must keep.

## 12 — Benchmark

**Reframed by H1.** The reference machine is Intel with discrete AMD graphics; the spec declares Apple Silicon the primary target. Every performance number to date — quality-profile capping, the 8 ms render budget, the degradation ladder's thresholds — is unvalidated on the declared target. 12 must state which architecture each measurement was taken on, and AC-5's 30-minute zero-drop soak must not be reported as met on an architecture the spec does not target. S1 is 12's problem too: `renderGpuTimeMs` is always 0, and it is the ladder's input, so the ladder is currently deciding on a constant. A benchmark prompt that inherits a stubbed GPU timer measures nothing.

## 13 — Operator shell

Inherits the display-surface deferral (04 → 09 → here in practice) and S2: **`showState` is absent from the §10.1 telemetry tick**, so a shell subscribing to telemetry alone cannot say whether the show is running. Either the shell also tracks `stateChange` frames, or v0.4 adds the field — the outline records it as a candidate. 13 is also the first consumer that will notice R2's metering strobe: bus meters sampled once a second from a 33 ms window are unusable in an operator UI, so R2's fix is a prerequisite rather than a nicety.

---

## Cross-cutting, owned by no single prompt

| Item | Where it lands |
|---|---|
| Preflight resource enforcement (H2) | v0.4 outline §3 — schema/contract question, not prompt work |
| `showState` in telemetry (S2) | v0.4 outline §3 — wire contract |
| `viewItemStartFrame` in the resync snapshot | v0.4 outline §2, already confirmed |
| `sequenceRef` | v0.4 outline §5 — review recommends **retire**; evidence absent |
| §12.6 clamp wiring | Re-deferred; trigger is the first Apple Silicon machine or the first >1 GiB loop budget |
