# Move-Transition Parity and Virtual-Set Roadmap

Status: research synthesis and spec-revision input. Non-normative until adopted into a spec version.
Audience: spec authors, coding agents, QA.
Reference sources: OBS Move Transition plugin documentation (exeldro), Vizrt documentation (Layer Manager, Transition Logic, Multi-Layer Virtual Studio, AI Keyer), FreeD protocol and lens-calibration references (Unreal Live Link FreeD, Aximmetry, miraxyz, Zero Density), NBE SPEC v0.1/v0.2/v0.3.

---

## 1. The core finding

NBE v0.3's state-diff transition engine (Section 7.3) is architecturally equivalent to — and in one respect stronger than — the OBS Move Transition plugin's model of cross-scene element continuity.

The plugin matches sources across scenes by name heuristics ("contains the other source name", "numbers removed from end", "last word removed"). NBE matches by persistent schema identity: the same element ID in two scenes is the same element, with state comprising transform, crop, opacity, visibility, effect parameters, and audio parameters. The transition is an interpolation between two element-state maps keyed by element ID, precomputable at arm time so the 2-frame take-latency acceptance criterion holds.

Name-matching is a runtime guess. ID-matching is a preflight-verifiable contract. The spec model eliminates an entire class of on-air failure (a renamed source silently breaking a move) that OBS operators manage by convention.

## 2. Parity table: Move plugin vs. NBE v0.3

| # | Move plugin capability | NBE v0.3 mechanism | Verdict |
|---|---|---|---|
| 1 | Cross-scene element continuity via name match | Persistent element ID across scenes (7.2) | HAVE — stronger |
| 2 | Matched items tween position/size between scenes | State-diff interpolation keyed by element ID (7.3) | HAVE |
| 3 | Appearing items: defined origin + optional zoom-in | Enter animation, incoming-only (7.3) | PARTIAL — see D1 |
| 4 | Disappearing items: defined destination + optional zoom-out | Exit animation, outgoing-only (7.3) | PARTIAL — see D1 |
| 5 | Easing: easings.net families (sine, quad, cubic, quart, quint, expo, circ, back, elastic, bounce) x in/out/in-out | Enum: linear, easeIn, easeOut, cubicBezier, spring | PARTIAL — see D2 |
| 6 | Curve: path curvature toward/away from canvas center | "path" named in 7.3 prose; absent from schema | GAP — see D3 |
| 7 | Rotation and alignment/bounding-box matching | Transform = x, y, w, h only | GAP — see D4 |
| 8 | z-order behavior during transition (discrete switch) | Not specified | GAP — see D5 |
| 9 | Per-item transition override; transition scale type (max only / aspect / stretch) | TransitionPreset.elementOverrides | PARTIAL — see D6 |
| 10 | In-place move without scene change (Move Source filter): triggers (hotkey, show/hide, activate), "next move" chaining | Automation triggers fire any command (Section 12); overlay.show/hide have own animations (7.4); no element-level animated-state command | GAP — see D7 |
| 11 | Animate audio volume (Audio Move) and arbitrary filter settings (Move Value) | Element state includes audio parameters and effect parameters; both are tweenable (7.2/7.3) | HAVE |
| 12 | Per-scene/per-source transition override filter | TransitionPreset + elementOverrides binding | HAVE |
| 13 | Face-landmark-driven moves (NVIDIA AR Move filter) | Plugin system (future native effect/element plugin) | FUTURE |

## 3. Required deltas (D1-D7)

All seven are schema-and-shader-local. None alter the architecture. All fold into the existing build order (Prompt 04 basic compositor, Prompt 07 graphics layer) without reordering.

### D1 — Enter-from / exit-to state model

Appearing and disappearing elements need explicit terminal states, not just an animation wrapper:

```json
{
  "enterAnimation": {
    "durationFrames": 12,
    "easing": "spring",
    "from": { "x": 1.2, "y": 0.05, "w": 0.30, "h": 0.30, "opacity": 0.0 },
    "zoom": true
  }
}
```

- `from` (enter) / `to` (exit): partial element-state applied as the animation's terminal. Absent fields default to the element's rest state (i.e., opacity-only fade).
- `zoom: true` reproduces the plugin's scale-from-zero appear / scale-to-zero disappear.

### D2 — Extended easing enum

Extend `Animation.easing` additively:

```
linear, easeIn, easeOut, easeInOut,
sineIn, sineOut, sineInOut,
quadIn, quadOut, quadInOut,
cubicIn, cubicOut, cubicInOut,
quartIn, quartOut, quartInOut,
quintIn, quintOut, quintInOut,
expoIn, expoOut, expoInOut,
circIn, circOut, circInOut,
backIn, backOut, backInOut,
elasticIn, elasticOut, elasticInOut,
bounceIn, bounceOut, bounceInOut,
cubicBezier, spring
```

Easing evaluation is a pure function of normalized time; this is a schema and library change only. `cubicBezier` already covers parametric curves; the named families cover the canonical news-graphics feel (back for overshoot entrances, elastic/bounce for playful elements).

### D3 — Path definition

```json
{ "path": { "curve": 0.35 } }
```

- `curve`: scalar matching the plugin's semantics — 0.00 straight line, positive curves away from canvas center, negative curves toward it.
- Alternative richer form (optional): `"path": { "controlPoints": [[x1,y1],[x2,y2]] }` for an explicit quadratic/cubic bezier path. The scalar form is sufficient for parity.

### D4 — Rotation and pivot on Transform

```json
{
  "transform": {
    "x": 0.66, "y": 0.05, "w": 0.30, "h": 0.30,
    "rotation": 15.0,
    "pivot": { "x": 0.5, "y": 0.5 }
  }
}
```

- `rotation`: degrees, clockwise, default 0. Tweenable in state diffs.
- `pivot`: normalized point within the element's own bounds, default center. Required for rotation and zoom to have meaningful semantics (rotate about center vs. corner produce different moves).
- Renderer cost is one affine matrix; the real work is schema, state-diff interpolation, and preflight bounds checking.

### D5 — z-order change semantics

z is not tweenable. State normatively:

- z changes are discrete and apply per element according to `zPolicy` on the transition preset: `start | midpoint | end` (default `midpoint` for mix/wipe, `start` for cut).
- Rationale: when an element moves from behind another to in front, the crossover frame must be deterministic to preserve frame-determinism of the compositor (Section 6.2). Unspecified z-swap timing is a class of irreproducible visual bug.

### D6 — scaleMode for size interpolation

When start and end bounds have different aspect ratios, define interpolation semantics:

```
scaleMode: "stretch" | "aspect" | "maxOnly"   (default "stretch")
```

Mirrors the plugin's transition scale type. `aspect` letterboxes during the move; `maxOnly` scales only up to the source's native size.

### D7 — element.animate command

Normative command for in-place animated state change without a scene transition:

```json
{
  "command": "element.animate",
  "payload": {
    "elementRef": "A1.pipGuest",
    "to": { "transform": { "x": 0.0, "y": 0.0, "w": 1.0, "h": 1.0 } },
    "animation": { "durationFrames": 12, "easing": "spring" }
  }
}
```

- Works on any element in the active scene or overlay level.
- Chaining (the plugin's "next move") is achieved via an automation rule on `stateChange` (or a future `animationEnd` trigger) firing a follow-up `element.animate`.
- This command is what powers the signature PiP-springs-to-fullscreen move on a hotkey — restoring a capability that v0.1's DVE section reduced to static PiP for MVP.

## 4. Virtual sets: phased upgrade path

### Phase A — 2.5D virtual set, first-class (no schedule impact)

The fixed-camera alpha-layer approach (background plate, keyed talent, desk alpha foreground) is fully expressible in v0.3 today. Make it a documented, preflight-checked convention:

- Reserved z-band convention (from the gap analysis): 0-99 background/plate, 100-199 keyed talent, 200-299 in-scene graphics, 300+ foreground inserts. Preflight warns on band violations.
- A `virtualSet2D` scene template in the template library implementing the sandwich with named element roles (`plate`, `talent`, `deskFg`).
- Add a light-wrap parameter to the chroma key effect (Section 6.5): bleed sampled background color onto talent edges. Cheapest available realism win; every virtual-set engine ships it.
- Optional later: AI keying as a native effect (Vizrt shipped exactly this product — AI Keyer, April 2026 — proving green-screen-free virtual sets are now standard practice). This is the formal home for the "virtual green screen" concept.

### Phase B — Tracked 2.5D (highest leverage per unit effort)

Add a tracking input module:

- FreeD protocol ingest over UDP. FreeD is the broadcast-standard tracking format: 8 axes (pan, tilt, roll, X, Y, Z, zoom, focus). Mainstream PTZ cameras (Canon CR-N300 with current firmware, Telycam FreeD models) emit it natively; Unreal ingests it via Live Link; every major virtual-set engine (Vizrt, Zero Density, Brainstorm, Aximmetry, Pixotope, ClassX) consumes it.
- Tracking samples are timestamped against the master show clock; at each frame boundary the compositor reads the aligned tracking state. This preserves frame-determinism and replaces hardware genlock for green-screen workflows (genlock remains relevant for LED volumes, which are out of scope).
- Tracking-to-video delay calibration is a manifest-declared, operator-measured value (frames or ms). The Aximmetry calibration literature is explicit that delay misalignment is the first thing to check when markers drift.
- Bindings map tracking channels to element transforms with per-z-band parallax multipliers: background moves least, foreground most. Without tracking data, parallax is not achievable at all — this binding is the entire Phase B value proposition.

Schema sketch:

```json
{
  "tracking": {
    "sources": [
      { "id": "ptz1", "protocol": "freeD", "endpoint": "udp://0.0.0.0:40000", "delayMs": 40 }
    ]
  },
  "elements": [
    { "id": "plate", "parallaxBind": { "source": "ptz1", "panTiltGain": 0.15 } },
    { "id": "deskFg", "parallaxBind": { "source": "ptz1", "panTiltGain": 0.85 } }
  ]
}
```

### Phase C — True 3D virtual set (post-AC-5, feature-flagged)

Per the spec's phasing-honesty rule (core playout must reach AC-5 30-minute zero-drop before advanced compositing merges), true 3D lands last. Two routes, not mutually exclusive:

1. Native: a `virtualSet` element kind — a glTF scene with baked lighting, rendered to a texture through a perspective camera driven by FreeD state, composited via the existing sub-scene-to-texture path. Requires:
   - Lens calibration profile asset: converts raw lens encoder values to FOV and carries a distortion map. Lens profiling is a standalone discipline; profiles must be loadable per camera/lens pair.
   - Holdout mattes for real-object occlusion (the physical desk stays in front of virtual content) — Phase A's alpha-foreground trick generalizes to scene-rendered matte outputs.
   - Optional depth of field for realism (GPU blur driven by scene depth).
2. External engine as a source: Unreal (Live Link FreeD) or Zero Density renders the set; NBE ingests it as a video source (NDI feature flag precedent) or GPU texture share. NBE remains the rundown, automation, graphics, and output authority — the same relationship Viz Pilot has to Viz Engine. Lowest build cost, highest external dependency.

Schema hooks to land during Phase A/B so Phase C is additive, not breaking:

- `Asset.kind` extension: `lensProfile`.
- `Element.kind` reserved value: `virtualSet` (preflight rejects until the feature flag ships — same pattern as NDI).
- `Element.parallaxBind` (Phase B) designed so a `virtualSet` element consumes the same tracking source identity.

## 5. Build-order impact

| Delta / phase | Lands in | Cost class |
|---|---|---|
| D1 enter-from/exit-to, D2 easings, D3 path, D6 scaleMode | Prompt 04 (basic compositor) | Schema + pure functions |
| D4 rotation + pivot | Prompt 04 | Schema + one matrix in the element shader |
| D5 z-swap semantics | Prompt 02/04 (state machine + compositor) | Normative rule + test |
| D7 element.animate | Prompt 02 (command surface) + 04 | Command handler + existing tween engine |
| Phase A 2.5D set template + light wrap | Prompt 07 (graphics layer) | Template + one shader parameter |
| Phase B FreeD ingest + parallax bindings | New prompt after Prompt 07 | UDP listener + clock-aligned sampler + bindings |
| Phase C virtualSet element kind | Post-AC-5, feature-flagged | glTF renderer OR external-engine ingest |

## 6. Acceptance-criteria additions

- AC (D1-D6): for every transition kind, a golden-frame test comparing composited output at t = 0, midpoint, and end against reference frames, including rotation, curved path, and z-swap at the declared frame.
- AC (D7): `element.animate` issued on localhost produces first visible change within 2 frames and completes within `durationFrames + 1`, matching the take-latency budget.
- AC (Phase B): with a recorded FreeD stream replayed at a fixed offset, pan values drive bound elements by exactly `panTiltGain x delta` degrees of normalized canvas per frame; tracking loss for over 500 ms holds the last value and logs telemetry (mirrors the guest hold-latest-frame policy).
- AC (Phase C, when flagged): virtual-set render holds frame-determinism — identical tracking streams and show state produce bit-identical frames across runs.
