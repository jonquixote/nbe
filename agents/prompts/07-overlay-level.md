# Agent Prompt 07 — The Overlay Level, and the Spine Beneath It

**Targets SPEC v0.4** (`docs/spec.v0.4.md`). Written after the midpoint
integration review (`docs/review-midpoint-report.md`, PR #8) and the heritage
pass, against the engine **as it actually is** — not as the pre-review version
of this document imagined it.

**Execution order is not negotiable.** The carried P0s come first. An overlay
composited over a show whose clock starts late and whose acknowledgements
freeze is an overlay on a broken spine. Steps 1–4 are the spine; step 5 is this
prompt's own mission.

---

## Step 0 — The As-Built Ledger

What exists, where it lives, and where it diverges from the spec. This replaces
the old Step 0's inventory-by-imagination. Every "divergence" column entry is a
measured finding, not a guess.

### Crates and modules

| Spec section / concern | Implemented in | Actual behaviour / divergence |
|---|---|---|
| §6 manifest model, §6.7 versioning | `nbe-core/src/{manifest,validate}.rs` | Schema is the source of truth; `manifestVersion` accepts `"0.3"` and `"0.4"` (v0.4 removed only the `sequenceRef` hook). Round-trip proven against the schema. |
| §19 preflight | `nbe-preflight/src/main.rs`, `nbe-core/src/preflight.rs` | Decodes real media. **v0.4 additions:** §17.5 contradictory-item check, §12.11 resource demand (always reported), §7.15 house-rate warning (only with `--house-rate`). **Divergence: takes 46 s on a 5-second 1080p package** in a debug build, and exceeded 180 s under CPU contention. This is the rehearsal's flakiness source. |
| §5.4/§5.9 wire protocol | `nbe-protocol/src/lib.rs`, `control-plane/src/protocol.ts` | Two mirrors of one surface. `nbe-protocol/tests/mirror.rs` parses the §16 tables **out of the spec at test time**, so spec/code drift fails `cargo test`. Both mirrors must move together. |
| §5.1–§5.9 control plane | `control-plane/src/{server,dispatch,state,package,telemetry}.ts` | Owns show state, `stateVersion`, the audit log, the §16 surface. **v0.4:** `show.load` rejects a house-rate mismatch; the snapshot carries `viewItemStartFrame`; the tick carries `showState`. |
| §5.9.4 resync | `nbe-engine/src/directive.rs::on_resync`, `control-plane/src/state.ts::resyncSnapshot` | Full snapshot, both buses, empty `visibleOverlays` clears. **v0.4:** consumes `viewItemStartFrame` instead of guessing `now`. |
| §7 compositor | `nbe-engine/src/render.rs`, `scene.rs`, `gpu.rs` | wgpu 30 textured quads. `drawn_elements` is the **single** walk: `resolve` (picture), `item_audio_asset` (audio) and `items_using_asset` (fault blame) all derive from it. That unification took five attempts; do not re-introduce a parallel walk. |
| §7.10 **overlay level** | **does not exist** | Deferred by P4 in writing, naming 07 as owner. `View = overlay(transition(A, B))` is unimplemented. **This prompt's mission.** |
| §8 audio graph | `nbe-engine/src/{audio,audio_control,audio_driver}.rs` | Real graph, real driver, allocation-free `render`, structural mix-minus. Single source slot per bus — **no per-source envelopes**, which is why §8.7.5's true crossfade is still open. |
| §10.1 telemetry | `nbe-engine/src/telemetry.rs`, `control-plane/src/telemetry.ts` | Merged tick, engine-authoritative fields cached with a TTL. **Divergence: `renderGpuTimeMs`, `vramUsedMib`, `textureCacheUsedMib`, `masterClockDriftMs` are always 0** — stubs. `renderGpuTimeMs` is the degradation ladder's input, so the ladder decides on a constant. |
| §10.3 watchdog / fallback | `nbe-engine/src/watchdog.rs`, `render.rs` | Trips on consecutive late frames; engagement path now covered by the `fail_view` seam. |
| §12 loops and VRAM | `nbe-engine/src/loop_cache.rs`, `video.rs` | Budget arithmetic and streaming fallback exist. **Divergence: the §12.6 unified-memory clamp has no production caller** (`recommended_working_set_mib` is `None` everywhere) — dormant by decision, since the reference target is discrete (§0.3). |
| §18 cadence | `nbe-decode/src/lib.rs::source_index_at` | **Now real.** `draw_for` maps show time to source time; a 12 fps source spans 30 house frames, gated by a pixel readback. Was absent for six prompts while AC-4 was reported delivered. |
| §12.1 master clock | `nbe-engine/src/clock.rs` | `(F − t0)` arithmetic, no wall-clock. **Divergence: does not advance within 2 ticks of `show.start`** — finding R5, step 1 below. |
| §5.9.5 quiescence | `control-plane/src/server.ts` | `show.stop` waits for `appliedStateVersion`. **Divergence: the engine stops sending it after the initial resync** — finding R6, step 2 below. |
| §16 command surface | `control-plane/src/commands/*.ts` | 54 commands after v0.4 retired `sequence.arm`/`unarm`. Role matrix in `dispatch.ts`. |
| **The dress rehearsal** | `control-plane/src/dress-rehearsal.test.ts`, `tests/fixtures/dress_show/` | Real control plane + real engine binary + real protocol + real 1080p media. **Non-blocking CI job**, measured over six runs at pass 7–9 / fail 3–5. It is also the **only** gate proving the engine binary starts its audio driver. |

### Standing invariants — break these and the review's findings return

1. **One walk.** `drawn_elements` decides what an item shows. Picture, audio and
   fault-blame all derive from it. Audio can only name an asset a drawn layer
   names.
2. **Read the manifest.** Where a field states the answer (`audio.bus`,
   `audio.muted`, `audioPolicy`, `visible`, `opacity`), read it. Inference from
   structure is a guess with good manners, and it was wrong five times.
3. **Gates observe effects, not text.** Source text and log text are free to
   forge; three wiring gates died proving it. A gate must observe something only
   the behaviour under test can produce.
4. **The side that knows both facts refuses** (§7.15, §12.11.3). Preflight
   warns; the control plane rejects.
5. **A test count that only goes up cannot detect deletion.** Reconcile
   added/deleted by name when restructuring tests.

---

## Step 1 (P0) — R5: the clock does not start promptly

**Finding.** `masterClockFrame` stays at 0 for several telemetry ticks after
`show.start` returns `ok`, then jumps. It does run — 0 → 150 → 240 — but not
within the charter's 2-tick window. Rehearsal step 3 fails on this, stably, in
every run.

**Why it is first.** Every timed behaviour downstream is measured against this
clock. The audio path is *not* slow — once the clock runs, audio arrives within
about one tick. The clock's start is the defect.

**Deliverable.** `show.start` → the engine's clock advancing, observable in
telemetry within 2 ticks. Root-cause it; do not widen a window to hide it.

**Falsification row.** Reinstate the delay → rehearsal step 3 fails.

## Step 2 (P0) — R6: `appliedStateVersion` freezes after resync

**Finding.** `renderNode.lastAppliedStateVersion` reads 1 on every
`stateChange` through `show.start` (sv 3) and both takes (sv 5, 7). The engine
*applies* directives — the clock starts, takes land, audio plays — and stops
**reporting** that it applied them. The engine log's last acknowledgement is
`show.resync applied sv=1`.

**Consequence.** §5.9.5's quiescence handshake can never fire, so `show.stop`
always takes the forced path; and the charter's "gapless `appliedStateVersion`
per connection" gate cannot be satisfied by construction. Rehearsal step 10
passes today only because it measures elapsed time, and the forced path also
completes inside the window.

**Deliverable.** The engine emits `appliedStateVersion` for every directive it
applies, not only the resync.

**Falsification row.** Suppress the emission → a new test asserting a gapless
per-connection sequence fails, and `show.stop` is observed taking the forced
path.

## Step 3 — The rehearsal's own findings

### 3a. R2 — bus meters are a 33 ms window sampled at 1 Hz

`AudioDriver::publish` writes `bus_peaks` then calls `reset_meters()`, every
audio block (~33 ms). Telemetry samples once per second. Each tick therefore
reports the peak of one 33 ms block — about **3%** of the interval — and a
0.4 s soundboard stab is a coin toss (rehearsal step 6: red in 4 of 6 runs).

An operator watching `busPeakDbfs` sees a strobe, not a level. Decide the
window in writing: a peak-hold across the telemetry interval, or an explicit
decay. Either is defensible; the current behaviour is not a decision.

**Falsification row.** Restore per-block reset → step 6 becomes intermittent
again, and a unit test asserting a full-interval peak fails.

### 3b. R4 — an underrun on the happy path

`audioUnderrunsTotal` reaches 1 during a nominal show. Both production sources
are now tested (sink refusal and cadence overrun); this is about *why* one
occurs when nothing is wrong. Likely the 46 s preflight decode competing for
six cores — which ties it to step 3d.

**Falsification row.** The rehearsal's gate assertion (`audioUnderrunsTotal ==
0`) goes green and stays green across six consecutive runs.

### 3c. Step 4's redesign — measure the right interval

Rehearsal step 4 asserts "the clip bus rises within 2 ticks of the take", but
issues its take **while the clock is still stalled** (R5). A clock-relative
window measured across a clock stall is not a measurement of the audio path at
all — **step 4 would fail against a perfect audio implementation.**

Redesign it to wait for the clock to be observably running before opening its
window.

> **Explicitly forbidden: widening the timeout.** That hides R5 instead of
> measuring it, and the point of the step is to measure.

**Falsification row.** Reintroduce the clock stall with the audio path intact →
the redesigned step still passes (it measures audio, not the clock); reintroduce
an audio fault → it fails.

### 3d. Promote the rehearsal to a required gate

Only once 3a–3c and steps 1–2 close, **and** `show.load` stops taking 46 s
under contention. The fix is not a bigger timeout: run preflight's decode in
release even from a debug build, cache the report by package hash, or narrow the
fixture's decode surface. Then flip `continue-on-error: true` off and require it.

**Gate condition.** Green on `macos-14` **twice consecutively** (charter
definition of done). A real-time gate that flakes is not a gate.

## Step 4 — F1, F2, and the inherited hang

### 4a. F1 — the unattributable-decode branch is ungated

`directive.rs`'s `affected.is_empty()` branch logs
`"decode failure affects no rundown item; nothing to report"`. Deleting that
`warn!` — or the `error!` above it — leaves the suite green. It is the single
place where "logged, therefore not swallowed" rests on an ungated line. Pass 10
wrote the probe; adopt it.

### 4b. F2 — `VideoLibrary::failures` is write-only

Written at `directive.rs`, read nowhere. Its doc comment ("the caller reports
each as `itemEvent: decodeError`") is true only for *attributable* assets. Post
pass-9, an unattributable fault correctly blames no item — which makes this the
natural home for surfacing it to the operator as a **telemetry counter**, not
an `itemEvent`.

### 4c. The inherited hang

`control-plane/src/render-channel.test.ts` **hangs indefinitely** when the
`nbe-preflight` binary is not resolvable: every test reports `ok` and the
process never exits, so CI hits a wall-clock limit rather than reporting a
failure. Pre-existing on `main` (this file is untouched by the review branch).
`send()` in that file has no timeout, unlike its `until()` helper.

**Falsification row.** Remove the binary → the suite **fails** rather than
hangs.

---

## Step 5 — The mission: the overlay level (§7.10)

Everything above is the spine. This is the prompt.

### 5.1 What it is

`View = overlay(transition(sceneA, sceneB))`. Overlay elements — ticker, logo
bug, breaking banner, clock — composite **after** the transition and **persist
across** it. They have independent `overlay.show` / `overlay.hide` commands with
their own enter/exit animations.

**Acceptance case, from P4's recorded deferral:** an overlay is **pixel-identical
across a 15-frame mix crossfading beneath it**, measured at the pixel level.
AC-24 states the same requirement as "a ticker MUST survive a complex scene move
transition untouched and without recomposition artifacts."

### 5.2 The four questions — answer them in the document, before code

**1. The directive surface.** §16.6 gives `overlay.show {overlayId, animation?}`
and `overlay.hide {overlayId}`, both failing `E_NOT_FOUND` for an undeclared
overlay. The control plane already implements both and validates against
`PackageIndex.overlays`. **This question is answered by the existing command
surface — no spec work needed.** State that, and state what the engine must do
on receipt.

**2. The ticker text stack.** §6.5 is unusually specific: Unicode, UTF-8, RTL,
multilingual, **packaged fonts** (§0.1 assumption 11 — host fonts are
forbidden), scroll by **texture offset**, and **per-frame full relayout is
forbidden except on content change**. Choose one stack, justify it against the
dependency rules, and say **where layout and rasterization happen off the frame
path** (§7.13). The packaged-font rule is a portability asset
(`docs/portability.md` row 7) — do not weaken it. **Design here, build in 07b:**
`agents/prompts/07b-graphics-templates.md` carries the templates, the ticker,
the breaking banner, and the clock, and composites onto the level this prompt
builds.

**3. Preview semantics.** Does an armed overlay composite on Preview? §7.10 says
overlays composite after the transition and persist across it, and says nothing
about the Preview bus; §5.6 says Preview is "what is prepared to go live."
**Answer from the spec, not from convenience.** If the spec genuinely does not
decide it, say so and raise it as a v0.4 candidate rather than inventing a rule
in code.

**4. Persistence identity.** What identifies "the same overlay" across a take,
and what happens to its pixels while a mix runs beneath it? Include the
**resync semantics**: §5.9.4's snapshot carries `visibleOverlays`, and an
**empty array MUST clear all visible overlays** — it is a full snapshot, not a
patch. That rule is now normative in v0.4; implement it before the bug exists.

### 5.3 The carried audio items

**Per-source envelopes, designed once.** §8.7.5's true crossfade needs them: the
graph has one source slot per bus, so a take dips through silence rather than
crossfading. 07's overlay work needs the same mechanism for opacity. **Design
one envelope mechanism serving both**, or record explicitly why the audio side
waits.

**The `sfx` ramp-advance invariant — decide in writing.** Today `sfx` ramps
advance only while voices are active; `advance_silent` covers source-less
program buses but not this one. Either extend it or write the invariant where
`PendingSwap` lives. **Decide, don't drift.**

---

## Constraints

- **No schema changes** without stopping and flagging. v0.4 is freshly written;
  a schema need discovered here is a v0.4 amendment, not a prompt decision.
- **`unsafe` stays confined to `crates/nbe-decode`** (CI-enforced).
- **Prompt 03 channel semantics, the P4 frame contract and P1's locked preflight
  behaviours are not yours to touch.**
- Both wire mirrors move together, or `cargo test` fails — by design.
- Every new behaviour lands with a test that fails when it is removed
  (Standards §2a), and the falsification table is part of the deliverable.
- Verbatim CI summary lines for **both** jobs, plus the rehearsal job's line.

## Definition of done

1. Steps 1–2 (R5, R6) closed, each falsified.
2. Step 3's R2/R4 closed; step 4 redesigned to measure the right interval;
   the rehearsal promoted to **required** and green twice consecutively.
3. Step 4's F1/F2 gated; the inherited hang fails instead of hanging.
4. The overlay level implemented, with an overlay **pixel-identical across a
   15-frame mix** measured at the pixel level.
5. The four questions answered **in this document** before the code that
   answers them.
6. Per-source envelopes designed once for audio and overlay opacity, or the
   deferral recorded with a trigger.
7. The `sfx` ramp invariant decided in writing.
