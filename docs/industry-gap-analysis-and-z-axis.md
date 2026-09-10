# Industry Gap Analysis and Z-Axis Architecture

Status: research synthesis for spec revision input. Non-normative until adopted into a spec version.
Audience: spec authors, coding agents, QA.
Sources: vendor documentation and industry references (Ross OverDrive, Grass Valley AMPP/Playout X, Vizrt Viz Engine/Viz Pilot, Blackmagic ATEM, Zero Density Reality, EVS, LiveU/TVU), SMPTE/EBU standards material, FCC Part 79 compliance references. Full citations are preserved in the research report; this document is the actionable distillation.

---

## 1. How major broadcast systems handle the z-axis

### 1.1 The hardware lineage: z is a pipeline position, not a number

Vision mixers (production switchers) do not expose a z-coordinate to operators. Depth order is a fixed position in the signal chain:

```
Inputs -> M/E bank (background buses + upstream keyers)
       -> Transition block
       -> Downstream keyers (DSK 1..N)
       -> Fade-to-black (FTB)
       -> Program output
```

- Upstream keyers (USK) sit inside the M/E bank, before the transition block. Their content participates in cuts, dissolves, and wipes. Chroma-keyed talent, PiPs, and in-scene graphics live here.
- Downstream keyers (DSK) sit after the transition block. They overlay the fully switched program and are unaffected by transitions underneath. Logos, bugs, lower thirds, and end credits live here. ATEM Constellation ships 4 DSKs for exactly this purpose.
- FTB is the topmost operation in the chain. It covers everything, including DSKs.
- DSK TIE links a downstream key to the transition block, so the overlay dissolves on/off with the main transition instead of floating above it.
- Clean feed: switchers provide program-minus-DSK as an aux output, used for archive recording without on-air bugs and tickers.

### 1.2 Fill and key discipline

Every overlay in the hardware world is two synchronized signals: fill (color) and key (matte). The keyer cuts the hole; the fill fills it. In software compositors this collapses to internal alpha, but the discipline survives: alpha is a first-class pipeline citizen. NBE's existing rule (alpha assets MUST contain a real alpha channel; preflight MUST fail if absent) is the correct software expression of this.

### 1.3 Graphics engines: fixed planes plus true 3D

Vizrt's model:

- Three fixed compositing planes: Front (1), Main (2), Back (3). Scenes are authored for a plane.
- Within a scene, Viz Engine is a true 3D renderer compositing video, graphics, and data in a single pass.
- Transition Logic: a master scene (background/design elements) coordinates object scenes (variable content such as lower-third text). Transition Logic layers are explicitly "conceptual, not spatial": they express how many graphics can be independently on air at once (one object scene per layer), not pixel depth.

This separation is instructive: spatial ordering (z) and air-status concurrency (how many things can be live and who controls them) are different axes, and the industry keeps them separate.

### 1.4 Virtual sets and AR: true 3D via camera tracking

For tracked virtual production:

- Camera tracking hardware streams physical camera position, orientation, and lens data to the render engine every frame.
- The virtual camera mirrors the physical camera, producing parallax, correct occlusion, and matching depth of field.
- Keyed talent is sandwiched between background and foreground layers. Viz's multi-layer virtual studio render order: virtual-set graphics -> AR graphics -> keyed talent -> final composite.
- A 2.5D middle ground (stacked alpha layers with parallax multipliers, no tracking) is common and is the closest analog to NBE's compositor model.

---

## 2. NBE's current z-axis model, mapped

| Broadcast concept | NBE v0.3 equivalent | Status |
|---|---|---|
| M/E background + USK compositing | Scene Elements with integer z, sorted ascending; inherit/replace/merge stack rules | Present |
| DSK level | Overlay level: `View = overlay(transition(sceneA, sceneB))`; overlays persist across scene changes with independent show/hide commands | Present, structurally exact |
| Sub-compositions | Sub-scenes rendered once to a texture, recursion cap 4, DAG validation in preflight | Present (2.5D) |
| Fill/key discipline | Alpha asset requirements + preflight alpha presence check | Present |
| Named keyer slots with air-status | Raw integer z; overlays are an unbounded list | Missing |
| DSK TIE | Overlays always ride above transitions; no tie option | Missing |
| Clean feed | Recording master-only; isolation hook declared in v0.2 but not implemented | Hook only |
| FTB above all | Fallback slate exists; whether overlays survive a fallback cut is unspecified | Underspecified |
| AR sandwich (bg/talent/fg) | Expressible via z integers; no reserved band conventions | Ad hoc |
| True 3D / camera tracking | Out of scope | Correctly out of scope |

---

## 3. Proposed schema and spec delta (z-axis)

Suggested for a future spec revision. None of this blocks the current implementation order.

### 3.1 Bounded, named overlay slots

Replace the unbounded overlay list with named slots carrying explicit air-status:

```json
{
  "overlays": [
    { "id": "bug",     "slot": "persistent", "elements": [ ... ] },
    { "id": "ticker",  "slot": "persistent", "elements": [ ... ] },
    { "id": "banner",  "slot": "interrupt",  "elements": [ ... ] },
    { "id": "clock",   "slot": "persistent", "elements": [ ... ] }
  ]
}
```

- Slot count is bounded (recommend 4-8, mirroring hardware DSK counts).
- Each slot reports `onAir: boolean` in telemetry; occupancy of an interrupt slot by a new overlay evicts or queues per a declared policy.
- Preflight validates that exactly the declared slots exist and that overlay element IDs are unique within a slot.

### 3.2 TIE semantics

Add per-overlay:

```json
{ "id": "lowerThird", "tieToTransition": false }
```

- `false` (default): overlay rides above scene transitions (current v0.3 behavior, DSK default).
- `true`: the overlay participates in the active transition's dissolve/wipe as if upstream (DSK TIE).

### 3.3 Clean feed output

Promote the v0.2 isolation hook to a defined output variant:

```json
{ "outputs": { "record": { "source": "clean" } } }
```

- `source: "program"` (default) composites overlays; `source: "clean"` composites the scene level only.
- Given render-to-texture sub-scenes and GPU texture sharing to encoder sessions, this is an additional encoder tap, not a re-composite.

### 3.4 Fallback and FTB at the top of the stack

State normatively:

- The fallback slate composites above the overlay level. A fallback cut MUST cover tickers, bugs, and banners.
- A future `view.ftb` command fades the entire View (scene + overlays) to black, mirroring hardware FTB.

### 3.5 Reserved z-band conventions

Document (non-normative convention, validated by preflight warning only):

- 0-99: scene background and plate content
- 100-199: keyed talent / primary video
- 200-299: in-scene graphics and PiPs
- 300+: foreground inserts (future AR hooks)

This reserves headroom for a tracked virtual-set mode without renumbering existing shows.

---

## 4. Consolidated capability gap list

Crediting v0.3 Section 10.3 for what it has already absorbed (Snapshots, Tally borders/labels, WebVTT caption sidecar, Multiview), the remaining gaps versus major broadcast operations, ranked by leverage:

### 4.1 Caption compliance depth (clearest hard gap)

- v0.3 provides a WebVTT-class sidecar. US broadcast carriage requires CEA-608/708 embedding in the encoded output (FCC 47 CFR 79.1), with approximately 99% character accuracy, plus/minus 2-frame synchronization, 100% cue completeness, and title-safe placement.
- Live captioning must reach viewers within roughly 2 seconds of speech; top-25-market news may not use purely automated captioning to satisfy the requirement.
- Placement interacts directly with NBE's own lower-third and ticker layers: captions must not collide with burned-in graphics.
- Recommended: add preflight checks for caption completeness and placement (machine-checkable, consistent with the existing 20-gate philosophy), and an encode-side CEA-608/708 SEI insertion path for streaming/record outputs. The WebVTT sidecar remains the OTT delivery form.

### 4.2 MOS-style status round-trip

- The industry's editorial/device backbone (MOS protocol) is bidirectional: commands flow down, and item status (ready / armed / missing / playing) continuously flows back up to the authoring layer.
- NBE has telemetry and monotonic stateVersion. What is not explicit is the author-facing item-state loop: the rundown/editor UI SHOULD reflect per-item readiness without polling internal state.
- Recommended: define a normative `itemStatus` event stream (ready, armed, live, missing, error) emitted on every state transition affecting a rundown item.

### 4.3 Guest-facing tally and return video

- v0.3 tally is operator-side (borders/labels). REMI platforms (LiveU, TVU) deliver tally and program return to the field over the same IP link as the guest's outbound feed.
- NBE already models Views as WHEP-servable buses and mandates mix-minus audio per guest.
- Recommended: define a guest return View (program or multiview, WHEP-served, JWT-scoped like guest links) with an embedded tally indicator tied to the guest element's on-air state.

### 4.4 Z-axis mechanics

See Section 3: bounded overlay slots, TIE semantics, clean feed, FTB-above-all, reserved z-bands.

### 4.5 Failover as tested discipline

- Industry pattern: automated failover with hard time budgets (detection under 5 s, remediation under 3 s), plus scheduled failure-injection drills ("chaos days"). Untested redundancy is not redundancy.
- NBE has the watchdog, degradation ladder, and fallback slate. What is missing is the drill as a named acceptance test.
- Recommended: add an acceptance criterion of the form "kill the render process during a live show; fallback slate MUST appear within N frames and recovery MUST restore the prior scene state," run in CI with the loopback bridge, and repeated on a schedule against real hardware.

### 4.6 Bonded / multi-path streaming uplink

- REMI ecosystems aggregate multiple network paths for the outbound feed. NBE's stream output is single-path RTMP/SRT with automatic reconnect.
- Recommended (post-v1): SRT bonding or dual-destination streaming as the single-node analog of dual-network redundancy. Not an MVP item.

---

## 5. What not to chase

- True 3D virtual sets / AR occlusion: requires camera tracking hardware and a 3D scene pipeline. This is a product line, not a feature. NBE's 2.5D model (flat layers, z-order, DVE transforms, sub-scene textures) matches how most channels actually run day to day.
- MOS protocol interop with ENPS/iNEWS: NBE is a vertically integrated island by design. The MOS *pattern* (status round-trip) is worth adopting; the protocol itself is not, for a network of one.
- Cloud-native elastic channel lifecycle (Grass Valley AMPP model): the industry direction, correctly deferred by keeping Channel schema-only in v1. Premature here.
- ST 2022-7 dual-network fabrics: meaningless for a single render node. The achievable analogs are 4.5 (tested failover) and 4.6 (uplink bonding).

---

## 6. Summary table

| Gap | Industry reference | NBE today | Proposed action | Priority |
|---|---|---|---|---|
| Caption embedding + compliance gates | FCC Part 79, CEA-608/708 | WebVTT sidecar only | Encode-side SEI insertion + preflight placement/completeness checks | High |
| Author-facing status round-trip | MOS protocol | Telemetry exists, item-status loop not normative | Normative itemStatus event stream | High |
| Bounded overlay slots + air-status | Hardware DSK slots | Unbounded overlay list | Named slots, telemetry onAir, eviction policy | Medium |
| DSK TIE semantics | ATEM/broadcast DSK TIE | Overlays always float | tieToTransition flag | Medium |
| Clean feed recording | Switcher clean-feed aux | Master-only + hook | outputs.record.source = clean | Medium |
| FTB / fallback above overlays | FTB covers DSKs | Unspecified | Normative statement + future view.ftb | Medium |
| Guest tally + return View | LiveU/TVU REMI | Operator-side tally only | Guest return View with embedded tally | Medium |
| Failover drills as acceptance tests | Chaos-day discipline | Watchdog + fallback exist | Named AC with kill-mid-show test | High |
| Uplink bonding | LiveU bonded cellular | Single-path RTMP/SRT | Post-v1 SRT bonding / dual-destination | Low |
