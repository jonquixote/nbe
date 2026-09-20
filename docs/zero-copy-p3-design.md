# ZERO-COPY Phase 3 — design memo

Status: **decisions, not code.** Phase 3's executor read the frame path and
stopped before touching production, correctly: the migration is three changes
across three crates, and the only tractable slice would have reported
`record_tap_path: zeroCopy` while the frames still went through readback. This
memo answers the three questions that stop was waiting on, argues each against
the tree, and ends in a go/no-go and an ordered plan Phase 3b can run without
re-arguing anything.

Evidence is quoted from the tree at `8147c10`. Where this memo's own work order
and the tree disagree, the tree wins and the disagreement is named.

---

## A defect in the work order, named first

The order asks that device reach be argued against **"§7.4's render-role
isolation sentence"**. There is no such sentence. §7.4 is **"Element identity and
state model"** (`docs/spec.v0.4.md:942`) and has nothing to do with roles or
isolation. Nothing in the spec carries a "render-role isolation" rule under that
number.

The real text the question has to answer to is **§5.2 Runtime topology** —
*"All control traffic MUST pass through the control plane. Direct
dashboard-to-render-node control is forbidden in v1"* — and **§10.1.1**, which
turns out to settle the question outright. Both are used below. The same
mis-citation family has now appeared in prompts 11, 13, 14 and 09's original
draft; it is cheap to check and worth checking every time.

---

## Q1 — Device reach

### The question

`record.start` runs on the directive path. `EngineState` holds **zero** wgpu
handles. The device lives in `RenderLoop`. The probe needs a `&wgpu::Device`.

### Option (a): publish an `Option<Arc<wgpu::Device>>` from `RenderLoop::new`

The pattern exists and is load-bearing already. `render.rs:150-156`:

```rust
pub async fn new(state: Arc<EngineState>) -> anyhow::Result<Self> {
    let gpu = Gpu::init().await?;
    // The probe result is published here, in production, so telemetry
    // reports it because the engine put it there. Publishing (not
    // assigning) means a re-probe after device loss re-applies the
    // manifest cap instead of dropping it.
    state.set_probed_quality(gpu.quality);
```

That comment is the whole argument in miniature: a GPU-derived fact is probed by
the render node at init, **published into `EngineState`**, and read back out by
telemetry. Re-probe after device loss re-applies rather than drops, because the
publication point is the re-init point.

### Option (b): the probe belongs to the render role, reported at handshake

Superficially cleaner — the engine never holds a device handle outside the
render loop, and the capability arrives alongside §5.9.4's resync. But it fails
on three counts against the tree:

1. **It answers the wrong question at the wrong time.** The capability is not a
   property of the connection; it is a property of *this take's geometry*. The
   probe in `zerocopy::probe` builds a surface **at a width and height** and
   throws it away. A handshake-time probe would have to guess the geometry, and
   a 1080p answer does not generalise to the 4K case that Phase 1 measured
   separately and that the selection table has its own row for.
2. **It puts a render-node capability on the control plane's side of a boundary
   §5.2 draws the other way.** The control plane would learn the answer and the
   engine would then have to be told its own hardware's capability back — for a
   decision (`record.start`) the engine makes locally.
3. **It has no re-probe story.** The quality probe's comment exists because
   device loss must re-apply. A handshake-time fact goes stale silently at
   exactly the moment it matters.

### Decision: (a), and it is not a new pattern

**§10.1.1 already mandates this exact shape for a sibling fact.** Quoted:

> | The render node's startup probe (Section 10.5) | The **effective** profile — what this hardware can actually sustain. |
>
> The engine reports the effective profile in `engineTelemetry`, and the control
> plane emits that value when the engine report is fresh […]

A hardware capability, probed by the render node, published by the engine,
reported over `engineTelemetry`, merged by the control plane. `record_tap_path`
is the same kind of fact as `qualityProfile`, and answering *whose probe
`record_tap_reason` describes* is therefore already answered: **the render
node's**, exactly as the effective quality profile is.

**GO.** No spec candidate is needed for device reach — §10.1.1's precedent covers
it, and §5.2's boundary is respected rather than crossed.

### But §10.1.1 contradicts what Phase 2 shipped, and that IS a spec question

The same section ends:

> The emitted field shape is always complete. A telemetry consumer MUST never
> see a missing field, whatever the engine's state — an absent field and a
> stubbed field are different failures and only one of them is diagnosable.

Phase 2 shipped `recordTapPath` as `.optional()`, **absent until a take selects
a path**, and argued the absence was meaningful ("no take yet" vs "fell back").
That argument is good engineering and is in direct tension with a normative
sentence. §10.1.1 says a consumer must never see a missing field; Phase 2 makes
a consumer see one on every tick before the first take.

Two lawful resolutions, and the memo does not pick between them — this is the
one genuinely spec-facing question in Phase 3:

- **(i) Stub, don't omit.** The field is always present and carries a third
  value — `"none"` / `"unselected"` — before any take. Satisfies §10.1.1 as
  written; costs the absent/stubbed distinction Phase 2 wanted.
- **(ii) Scope §10.1.1.** Its completeness rule was written about *engine-owned
  fields going stale* (the surrounding paragraph is entirely about staleness
  and `engineConnected: false`). A field that is meaningfully absent before its
  subsystem has run is arguably a different case, and the rule could say so.

**Recommendation: (i), and no spec change.** The completeness rule is the older,
broader, and better-tested claim; "never a missing field" is the property that
makes a telemetry consumer writable at all. The absent/stubbed distinction Phase
2 wanted survives intact as `"none"` versus `"cpuReadback"` — a stub value is
not less diagnosable than an absent key, it is more.

**This is a finding against merged code**, not a Phase 3 design choice: the field
as shipped conflicts with §10.1.1 today, before any migration. It should be fixed
whether or not Phase 3b proceeds.

---

## Q2 — Retarget

### What the tree does now

`render.rs:464-467`:

```rust
let target = match bus {
    Bus::View => &self.targets.view,
    Bus::Preview => &self.targets.preview,
};
```

Two targets, allocated once in `RenderLoop::new`:

```rust
let targets = RenderTargets {
    view: gpu.make_texture(VIEW_W, VIEW_H, "view"),
    preview: gpu.make_texture(PREVIEW_W, PREVIEW_H, "preview"),
};
```

And the readback:

```rust
/// Read the View target back as RGBA8. Inspection and test path only.
pub async fn readback_view(&self) -> Vec<u8> {
    self.gpu.readback_rgba(&self.targets.view)
}
```

### The questions answered

**Does the CPU fallback share this target?** Yes — `readback_view` reads
`targets.view`, the same texture the compositor writes. There is no second
target. So the CPU path and the zero-copy path are not two pipelines; they are
one pipeline with two *exits*.

**What does the operator see during a zero-copy take?** Nothing changes, and
this is the answer that makes the retarget safe. §5.6 is explicit that *"the
preview bus MUST be independently rendered and visible in operator UI"* — and
Preview has its own target, untouched by any of this. The View is what the
recording consumes; operator visibility of the *View* is by readback for
inspection and test only, per that doc comment. Nothing in production renders
the View to a display.

**So: does the View texture itself become the surface, and what does readback
read?** Yes, and it reads the surface. The IOSurface-backed texture is a
`wgpu::Texture` like any other — Phase 1 proved `create_texture_from_hal`
produces one that `render_frame` writes into and that `readback_rgba` can read
back. Substituting it for `targets.view` for the take's lifetime leaves every
consumer working, including `readback_view` and therefore every golden-frame
test.

**A preview copy would defeat the point** and is not proposed: the whole gain is
that the encoder reads the allocation the compositor wrote.

### Deadline accounting, untouched by construction

The retarget **replaces** the target rather than adding a step. `render_frame`
resolves `target` once per bus per frame and draws into it; swapping which
texture that reference names adds no pass, no copy, and no await inside the
timed region. The timed region is unchanged — `main.rs:96-97`:

```rust
let render_started = Instant::now();
let _ = render.render_frame(frame, deadline);
let render_elapsed = render_started.elapsed();
```

What *leaves* the frame path is the readback await that currently follows it.
That is a removal from the record counter, not an addition to the render budget.

**GO**, with one carried obligation: `targets.view` is allocated at
`VIEW_W × VIEW_H`, and the substitute surface must match exactly or the
golden-frame suites will read a differently-shaped buffer. Phase 3b asserts the
dimensions at swap time and fails loudly on mismatch.

---

## Q3 — The encode seam, backpressure, and mid-take loss

### The seam

`encode.rs` exposes only `encode_rgba`. `RecordMsg` is bytes
(`thread.rs:59-69`):

```rust
pub enum RecordMsg {
    /// A View readback from the loop. […]
    Frame { rgba: Vec<u8> },
    /// A pre-encoded unit (TEST SEAM ONLY) […] Production sends `Frame` exclusively.
    Unit(EncodedUnit),
}
```

Phase 3b restores Phase 1's spike method as production `encode_pixel_buffer`
(skip the pool fetch and the copy; submit the caller's `CVPixelBuffer` straight
to `VTCompressionSessionEncodeFrame`), and adds a third `RecordMsg` variant
carrying a surface handle rather than bytes.

### Backpressure — and the design problem the tree exposes

The existing discipline, quoted from `feed.rs`:

> `try_send` the readback to the record thread over the bounded channel. A full
> channel **sheds (never blocks)** and reports `sent=false` so the loop counts
> the skip.

with `RECORD_CHANNEL_BOUND: usize = 2` and the rationale at `thread.rs:22-25`:
*"a full channel sheds […] The record path degrades; the View never waits."*

**That discipline does not transfer unchanged, and this is the sharpest finding
in the memo.** `Frame { rgba: Vec<u8> }` carries an *owned copy per frame*, so
shedding is free: drop the `Vec`, the compositor's next frame has nothing to do
with it. A shared surface is **one mutable allocation**. If the encoder is still
reading frame N when the compositor starts frame N+1, "shed" is not available —
the pixels have already been overwritten. Shedding a surface you have already
drawn into is not a skip, it is a corrupted frame.

So zero-copy requires a **surface pool**, not a surface:

- N surfaces, N = `RECORD_CHANNEL_BOUND + 1` (one in flight per channel slot,
  plus the one being drawn), each built by the same `probe`.
- The pre-check gains a second question. Today: *is the View already over
  budget?* Zero-copy adds: *is a free surface available?* **No free surface →
  skip before rendering**, which preserves the discipline's actual promise
  (record degrades, View never waits) at the only point where it can still be
  kept — before the draw, not after it.
- The skip counts the same way, so `skipped_record_frames` keeps its meaning and
  the span counters stay comparable across paths.

This is not a complication of the design; it is the design. A single-surface
implementation would be correct only while the encoder never falls behind, which
is the condition backpressure exists because you cannot assume.

### Mid-take chain loss — new behaviour, named as such

**Nothing in the tree answers this**, because no live take uses the chain. The
honest framing: the probe was right at `record.start`, and the chain dies
mid-take (device loss, surface invalidation).

**Option A — fail the take loudly with `E_NO_ZEROCOPY`.** The recording stops;
the operator knows immediately; the file is whatever fragments were already
written, which §9.3's finalization-free fragment policy makes playable.

**Option B — fall back to readback mid-recording.** The file survives whole. But
the path changes mid-file, and two things then need answers the tree does not
have: what `record_tap_path` reports for a take that was both (one value cannot
be true of the whole take), and whether the encoder tolerates its input
switching from a shared `CVPixelBuffer` to a pool-allocated one mid-stream
without a keyframe boundary.

**Decision: Option A, with a documented preference for revisiting.** Two reasons
from the tree rather than from taste. First, `record_tap_path` is *per take* by
construction — it is written once at selection and read by every tick after;
Option B makes the field a lie for part of every take it applies to, and the
whole point of the field is that a fallback is visible. Second, the existing
loud-failure precedent is strong and recent: `record.stop`'s finalize failure
withholds the ack rather than reporting a success it cannot vouch for
(`directive.rs`, pinned by `record_stop_failure_withholds_ack_and_keeps_file`).
A take that silently changes its own transport is the same class of quiet
substitution that rule refuses.

Option B becomes attractive the day `record_tap_path` can carry a transition
rather than a value. That is a wire change and belongs to whoever wants it.

**Falsification it ships with:** force the surface invalid mid-take (drop the
pool while the take runs) → the take fails with `E_NO_ZEROCOPY`, the telemetry
still reports `zeroCopy` for the take that was, the partial file parses under
`ffprobe`, and **no black frames are written**. Mutation: swallow the surface
error and keep feeding → the black-frame assertion fails.

**GO**, with the pool as a precondition and mid-take loss as new behaviour
written to Option A.

---

## Go / no-go

| Question | Verdict | Condition |
|---|---|---|
| Q1 device reach | **GO** — option (a) | §10.1.1's precedent; no spec change needed for reach |
| Q1′ field completeness | **GO, as a fix before migration** | `recordTapPath` must stub, not omit — it conflicts with §10.1.1 **today** |
| Q2 retarget | **GO** | Dimensions asserted at swap; Preview untouched |
| Q3 encode seam | **GO** | **Surface pool**, not a surface; pre-check gains the free-surface question |
| Q3′ mid-take loss | **GO** — Option A | New behaviour, named; loud failure over silent substitution |

No no-gos. The design survives contact with the tree, and the one thing that
changed shape under examination — backpressure — changed because the tree's own
discipline could not be transferred as written.

---

## Phase 3b execution plan

Ordered so each step is independently falsifiable and the tree is never in a
state where telemetry claims something the frame path does not do.

1. **Fix the completeness conflict first** (Q1′). `recordTapPath` /
   `recordTapReason` become always-present, stubbed `"none"` before selection,
   on both sides of the mirror.
   *Falsification:* omit the field → the §10.1.1 completeness assertion fails.
   *Why first:* it is a defect in merged code and is independent of migration.
2. **Device reach.** `EngineState` gains `Option<Arc<wgpu::Device>>`, published
   in `RenderLoop::new` beside `set_probed_quality`.
   *Falsification:* remove the publication → `record.start` cannot probe and
   selection reports `ProbeUnavailable` on a machine that has a device.
3. **The surface pool + the pixel-buffer encode.** `probe` gains a pool
   constructor; `encode_pixel_buffer` restored as production; `RecordMsg` gains
   its surface variant; the pre-check gains the free-surface question.
   *Falsification:* exhaust the pool → frames skip **before** the draw and
   `skipped_record_frames` counts them; mutate the pre-check to draw anyway →
   a torn-frame assertion fails.
4. **Retarget.** `render_frame`'s View target is the take's surface for the
   take's lifetime, dimensions asserted at swap.
   *Falsification:* swap a mismatched surface → the swap fails loudly rather
   than the golden-frame suites reading a wrong-shaped buffer.
5. **Selection at start, wired end to end.** Only now does `record.start` probe,
   select, store, and run the chosen path — the first point at which telemetry's
   claim and the frame path's behaviour are the same statement.
   *Falsification:* the two the work order named — probe-false → `cpuReadback`
   + reason with the file passing every structure assertion; zero-copy take →
   `zeroCopy`/`Table` with the rehearsal's structure assertions unchanged.
6. **Mid-take loss** (Q3′), to Option A, with the falsification above.
7. **Measurement and rehearsals.** Before/after at 1080p30 and the 4K run,
   quiescent with loads pasted, counts derived two ways; three consecutive green
   rehearsals naming their path; `soak.sh` capture; the soak row to present
   tense **only** once it is true.

Steps 1-4 ship no behaviour change to recording. Step 5 is the migration.

## Corrections found in execution (Phase 3b)

Added during execution, per §2c: the text above is preserved as written and
these are what contact with the tree changed. All three were found by a test
failing, not by review.

1. **Q2's carried obligation named dimensions; format is a second one.** The
   memo argued the retarget is a drop-in because "the IOSurface-backed texture
   is a `wgpu::Texture` like any other". It is — but its format is
   `Bgra8Unorm`, forced by the far end of the chain where VideoToolbox wants
   `kCVPixelFormatType_32BGRA`, and the composite pipeline is built for
   `Rgba8Unorm`. wgpu refuses the mismatch outright:

   > Render pipeline targets are incompatible with render pass … the RenderPass
   > uses textures with formats [Some(Bgra8Unorm)] but the RenderPipeline with
   > 'composite' label uses attachments with formats [Some(Rgba8Unorm)]

   Step 4 builds a BGRA sibling pipeline at init and selects by the target's
   format. The shader needs no variant: a fragment shader writes an
   RGBA-ordered `vec4` and the attachment's format decides how that lands in
   memory.

   A consequence the memo also did not reach: `readback_view` promises RGBA8,
   and a BGRA surface returns blue-first bytes. Handing those through would
   swap red and blue in every golden-frame comparison **rather than fail**, so
   the accessor swizzles while the View is a BGRA surface.

2. **The probe's texture needed `COPY_SRC | COPY_DST`.** Q2's GO rests on
   `readback_view` continuing to work across the retarget, and a copy needs the
   usage flag; without it the first readback during a zero-copy take aborts with
   a wgpu validation error. Free on the Metal side — `MTLTextureUsage` has no
   blit bit.

3. **"The take's surface for the take's lifetime" (Q2) is superseded by the
   pool (Q3).** Q3 was written after Q2 and found that one surface cannot serve
   a take. The retarget is therefore **per frame**, set and cleared around each
   one, which is what the code does.

## What this memo does not decide

The clean-feed outputs model stays unbuilt and unprecluded — a second consumer
of a second surface, which the pool makes more natural rather than less.
Ratification of §0.1 assumption 24's rescoped candidate remains a separate word;
this memo makes its mechanism buildable, not law.
