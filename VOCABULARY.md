# NBE Vocabulary Ledger

The canonical, expandable vocabulary of the News Broadcasting Engine. Every term has exactly one definition. If a term is not here, it is not normative. Until the glossary is merged into the spec in v0.3, SPEC.md (v0.2.5) wins on any conflict; after v0.3, the spec glossary is generated from this file — the ledger is the source, the spec is the compilation.

## How to extend this ledger

1. One term, one definition. Synonyms are listed as aliases, never defined twice.
2. Every term has a status: `normative` (in the current spec), `planned` (accepted for a future version), `deprecated` (do not use; see replacement).
3. Every term records the spec version that introduced it.
4. New terms are added by commit with a spec-version reference. Deprecations keep the old term listed with its replacement.
5. When in doubt, add the term — an undefined term in an agent prompt is a bug factory.

## Time axis — editorial: when things play

| Term | Status | Since | Definition | Aliases / notes |
|---|---|---|---|---|
| Network | normative | v0.1 | The top-level identity: branding, fonts, fallback assets, template library. | |
| Channel | normative (hook only) | v0.1 | A 24/7 programmed stream of Shows. Scheduler is post-v1; schema must not preclude it. | |
| Show | normative | v0.1 | A single program/episode definition: video/audio specs, outputs, fallback. | |
| Rundown | normative | v0.1 | The root Sequence of a Show: the editorial order of play. | |
| Sequence | planned | v0.3 | A recursive, ordered container of Items. Sub-sequences are nested Sequences. Reusable across Shows. | generalizes Rundown/Segment/Subsegment |
| Segment | normative | v0.1 | Conventional top level of a Rundown. IDs A–K by convention; schema allows A–ZZ. | |
| Subsegment | normative | v0.1 | Conventional second level of a Rundown (A1, A2…). | |
| Item | planned | v0.3 | Leaf of a Sequence: clip reference, scene reference, live source, generated slate, or nested Sequence. | |
| autoFollow | normative | v0.1 | Per-item flag to advance automatically when media ends. | subsumed by Automation in v0.3 |

## Space axis — visual: what is on screen

| Term | Status | Since | Definition | Aliases / notes |
|---|---|---|---|---|
| Scene | planned | v0.3 | A named, reusable visual composition: element list + per-element state + audio state. | |
| Sub-scene | planned | v0.3 | A Scene referenced as an Element inside another Scene; rendered once to a texture, reusable N times. | pre-comp; scene-as-source |
| Element | planned | v0.3 | The atomic addressable visual unit, with persistent identity across Scenes. | renames Layer |
| Group | planned | v0.3 | A named collection of Elements moved/toggled as one. | |
| Effect | normative | v0.1 | A stateless visual transform applied to an Element (chroma key, luma key, color correction, blur, mask, border, shadow, crop, custom WGSL). | |
| Transition | normative | v0.1 | An interpolation between two element-state maps. v0.1: cut/mix. v0.3: the state-diff engine (move, wipe, sting, DVE as parameterizations). | |
| Overlay (DSK) | planned | v0.3 | A compositing level applied after the transition; its elements persist across scene changes. `View = overlay(transition(A, B))`. | downstream key |
| Template | normative | v0.1 | A parameterized graphic (lower third, banner, ticker) with typed fields. | |
| Ticker | normative | v0.1 | The scrolling text element; scrolls by texture offset, never re-layout per frame. | |

## Buses and outputs

| Term | Status | Since | Definition | Aliases / notes |
|---|---|---|---|---|
| View | planned | v0.3 | The main live/recorded composited output. | renames Program |
| Preview | normative | v0.1 | The staging bus: what transitions into View. | |
| Multiview | planned | v0.3 | Operator grid render target: view, preview, source thumbnails, meters, tally borders. | |
| Fallback slate | normative | v0.1 | The resident emergency image; automatic cut-to target on failure, ≤ 1 frame late. | |

## Control

| Term | Status | Since | Definition | Aliases / notes |
|---|---|---|---|---|
| Command bus | normative | v0.1 | The single WebSocket+JSON channel all control traffic passes through. | |
| Envelope | normative | v0.1 | The command message wrapper: `v`, `id`, `command`, `payload`, `baseStateVersion`. | |
| stateVersion | normative | v0.1 | Monotonic control-plane version for optimistic concurrency. | |
| Binding | normative | v0.1 | A trigger→action mapping (Companion key, hotkey, MIDI, OSC, web button). | |
| Preset | planned | v0.3 | A named reusable configuration: element, effect, transition, scene, or audio. | |
| Snapshot | planned | v0.3 | A named, recallable state of the entire View. | |
| Automation rule | planned | v0.3 | trigger + conditions → command. Runs through the command bus with the same preconditions as a human. | advanced scene switcher |
| Automation hold | planned | v0.3 | Global automation kill switch; takes effect within 1 frame. | |
| Tally | planned | v0.3 | Live-source indication, operator- and talent-facing. | |
| Marker | planned | v0.3 | A rundown bookmark; doubles as a recording chapter. | |
| Role | normative | v0.1 | Permissions class: monitor, operator, producer, admin, render. | |

## Media and assets

| Term | Status | Since | Definition | Aliases / notes |
|---|---|---|---|---|
| Asset | normative | v0.1 | A packaged media file with kind, format, hash, cadence, and loop metadata. | |
| Show package | normative | v0.1 | The self-contained folder: manifest + media + templates + audio. | |
| Mezzanine | normative | v0.1 | The house format every asset is normalized to (CFR, house rate, H.264 High, 48 kHz, −16 LUFS). | normalized format |
| Loop | normative | v0.1 | Deterministic master-clock modulo playback: `frame(t) = (F − t0) mod P`. No restart events. | |
| Cadence | normative | v0.1 | Source frame-rate character, preserved via frame holds. | |
| Pulldown | normative | v0.2 | The declared frame-hold pattern for non-house rates (`pattern`, `repeatNthSourceFrame`, `repeatOnePerNSourceFrames`). | |
| Preflight | normative | v0.1 | The CI-runnable proof that a show package is air-ready. | |
| Plugin package | planned | v0.3 | A WASM element plugin or WGSL effect plugin with manifest, version pin, and permission list (deny by default). | |

## Audio

| Term | Status | Since | Definition | Aliases / notes |
|---|---|---|---|---|
| Bus | normative | v0.1 | A named audio channel group: mic, clip, music, sfx, guest, master, guestReturn, ifb. | |
| Mix-minus | normative | v0.2 | The guest return mix excluding that guest's own audio. Mandatory per guest. | |
| IFB | normative | v0.2 | The anchor monitor bus: program minus anchor mic, plus talkback. | |
| Ducking | normative | v0.1 | Automatic music attenuation under speech. | |
| Soundboard | normative | v0.1 | RAM-resident SFX triggered in under 20 ms. | |
| AFV | planned | v0.3 | Audio-follow-video: the named mode for audio tracking the taken source. | implicit in audioPolicy today |
| PFL | planned | v0.3 | Pre-fade listen; formalizes monitor-only solo. | |

## Operations and reliability

| Term | Status | Since | Definition | Aliases / notes |
|---|---|---|---|---|
| Master clock | normative | v0.1 | The single monotonic show clock everything derives from. | |
| Watchdog | normative | v0.1 | Frame-deadline monitor that triggers fallback. | |
| Telemetry | normative | v0.1 | The per-second engine state stream. | |
| House rate | normative | v0.1 | The output frame rate: 30 fps default, 60 for showcase. | |
| Quality profile | planned | v0.3 | A hardware-probed performance envelope selected at startup. | |
| Degradation ladder | planned | v0.3 | The ordered yield list under load: preview fps, loop caches, effect quality, multiview — View is never degraded. | |
| Captions | planned | v0.3 | Sidecar text output (WebVTT-class) alongside the stream; burn-in later. | |

## Deprecated terms

| Term | Replacement | Since |
|---|---|---|
| Program (bus) | View | v0.3 (planned) |
| Layer | Element | v0.3 (planned) |
| Scene collection | — (OBS-ism; do not use) | — |
| OS-level hotkey | Binding | v0.1 |
