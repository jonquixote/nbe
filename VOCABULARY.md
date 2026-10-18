# NBE Vocabulary Ledger

The canonical, expandable vocabulary of the News Broadcasting Engine. Every term has exactly one definition. If a term is not here, it is not normative. Until the glossary is merged into the spec in v0.3, SPEC.md (v0.2.5) wins on any conflict; after v0.3, the spec glossary is generated from this file — the ledger is the source, the spec is the compilation.

## How to extend this ledger

1. One term, one definition. Synonyms are listed as aliases, never defined twice.
2. Every term has a status: `normative` (in the current spec), `planned` (accepted for a future version), `deprecated` (do not use; see replacement).
3. Every term records the spec version that introduced it.
4. New terms are added by commit with a spec-version reference. Deprecations keep the old term listed with its replacement.
5. When in doubt, add the term — an undefined term in an agent prompt is a bug factory.
6. Every new term cites the spec section that owns it. A definition with no
   owning section is prose, and prose drifts.

## Time axis — editorial: when things play

| Term | Status | Since | Definition | Aliases / notes |
|---|---|---|---|---|
| Network | normative | v0.1 | The top-level identity: branding, fonts, fallback assets, template library. | |
| Channel | normative (hook only) | v0.1 | A 24/7 programmed stream of Shows. Scheduler is post-v1; schema must not preclude it. | |
| Show | normative | v0.1 | A single program/episode **definition**: video/audio specs, outputs, fallback. A static declaration in the manifest — the thing a Show package contains, not the thing that runs. What runs is **Show state**. | |
| Rundown | normative | v0.1 | The root Sequence of a Show: the editorial order of play. | |
| Sequence | normative | v0.3 | A flat, ordered container of Items (SPEC §16.4). `rundown` is the only Sequence; nesting was retired with `sequenceRef` in v0.4 — the hook sat unresolvable for four patch levels and was withdrawn rather than resolved. | generalizes Rundown/Segment |
| Segment | normative | v0.1 | Conventional top level of a Rundown. IDs A–K by convention; schema allows A–ZZ. | |
| Subsegment | normative | v0.1 | Conventional second level of a Rundown (A1, A2…). | |
| Item | normative | v0.3 | Leaf of a Sequence: clip reference, scene reference, live source, or generated slate (SPEC §16.4). Nesting was retired with `sequenceRef` in v0.4. | |
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
| View | planned | v0.3 | The main live/recorded composited output (SPEC §5.6). What the audience sees; never degraded by the ladder. | renames Program |
| Preview | normative | v0.1 | The staging bus: what is prepared to go live, and what transitions into View (SPEC §5.6). Never on air. | |
| Multiview | planned | v0.3 | Operator grid render target: view, preview, source thumbnails, meters, tally borders. | |
| Fallback slate | normative | v0.1 | The resident emergency image; automatic cut-to target on failure, ≤ 1 frame late. | on air: `fallbackActive` true (SPEC §13.2's domain `true`, `false`) |
| Output belief | normative | v0.4.8 | The control plane's own state for an output, as commanded: `streamState` (`idle`, `live`, `held`) and `recordState` (`idle`, `recording`), each with its own vocabulary (SPEC §10.1.2, §13.2). Set by the commands that drive the output, not by the render node, whose own account is its report (`streamTransportState`). | named by row 8's §10.1.2, ledgered with row 9 |
| Idle (output) | normative | v0.4.8 | The output belief's value for an output commanded off: `streamState` and `recordState` both take it, set by `stream.stop` / `record.stop` and by `show.stop`'s acknowledgement (SPEC §13.2, row 9). Terminal: nothing armed, nothing resumes. | the values predate row 9 on the §10.1 tick; ledgered with it |
| Live (output) | normative | v0.4.8 | `streamState`'s value for a stream commanded on air (`stream.start`; SPEC §13.2, row 9). It stays `live` through a transport redial: the redial is `streamTransportState`'s `reconnecting` (§10.1), not the belief's. | not the item state `LIVE`; ledgered with row 9 |
| Recording (output) | normative | v0.4.8 | `recordState`'s value for a recording commanded on (`record.start`; SPEC §13.2, row 9). The record's on-air word: no field borrows another's, so it is never `live`. | ledgered with row 9 |
| Belief claim | normative | v0.4.8 | An accepted output command's hold on the output belief: it holds until the command's acknowledgement (its own, or a later one proving it processed), or until §5.9.5's window ends without one, whichever comes first. Expiry ends the claim and is not an acknowledgement. A late acknowledgement cannot reclaim a dead claim (SPEC §10.1.3, row 10). | |
| Belief precedence | normative | v0.4.8 | The order the output belief follows (SPEC §10.1.3, row 10). Authority, before dispatch: an explicit operator action outranks automation, by the hold. Belief, after dispatch: the latest accepted command's claim, then a fresh engine report that postdates its acknowledgement, then the stale belief. | built by spine B's implementing prompt |
| Belief correction | normative | v0.4.8 | A fresh engine report moving the stream's output belief when no command holds a claim on it: a report that postdates the anchor and maps (`live`/`reconnecting` → `live`, `none`/`closed` → `idle`) to a value the belief does not hold. It lands as a detection transaction: one version bump and one `belief.corrected` audit row, with no `stateChange` frame and no automation trigger. It clears no alert (SPEC §10.1.4, row 11). | built by spine B's implementing PR (#56) |
| Clip head | normative | v0.4.8 | A plain clip's first 30 frames, decoded at `show.load` and resident for the life of the load, so a take draws frame 0 at once and a re-take never waits on a decoder (SPEC §12.8.1, row 12). | |
| Read-ahead window | normative | v0.4.8 | The K = 2 × GOP frames past a plain clip's head (K = 60 at house format), primed at `show.load` and refilled as the clip plays, by a decoder paced by occupancy, never by the wall clock (SPEC §12.8.1, row 12). K is counted in frames. | not §12.8's loop wrap read-ahead, though the same family |
| Read-ahead miss | normative | v0.4.8 | A clip frame not yet resident when its layer is drawn: the layer holds the highest-indexed resident frame below it, the render thread never blocks, nothing is drawn black before the clip's end, and the miss is counted as `readAheadMissesTotal` on the §10.1 tick (SPEC §12.8.1, §10.1, row 12). The hold is the miss policy, not playback. | `readAheadMiss` |
| Held (output) | normative | v0.4.8 | `streamState`'s value for a stream whose stop the render node never acknowledged: the control plane cannot know the output, so it holds it (SPEC §5.9.5; §13.2, row 9). ~~A suspension~~ *§5.9.5's limbo: a stop commanded, the output's state unknown (ratified 2026-10-16; the "suspension" gloss struck, §2c)*, not terminal as `idle` is: a rule written `to: "idle"` does not match it, and `from: "live"` does. An operator-initiated hold would be new behaviour with an entry of its own. ~~No writer until spine B's implementing work (§13.2 exception 1);~~ *Its writer is `show.stop`'s unacknowledged path, which turns a `live` stream `held` on the command's own path, and the next fresh engine report that postdates the stop's expiry corrects it (SPEC §10.1.4, row 11; §2c, 2026-10-18)*; `recordState` has no `held` (exception 2). | not §13.5's "held", which means `automationHold` is true. **Unqualified "held" means this value**; the other two meanings are always qualified: Automation hold (§13.5's "held") and Held (clock) (§11.4's `HELD`). Three meanings of one word: a known hazard (the user's ruling of 2026-10-18) |

## Processes — who runs what

The two components the system is made of. Neither was named in this ledger
before [RI-0]; every architectural statement in the spec and in the review
depends on the boundary between them.

| Term | Status | Since | Definition | Aliases / notes |
|---|---|---|---|---|
| Control plane | normative | v0.1 | The authoritative show-state owner (SPEC §5.1). The Node/TypeScript process that loads and validates Show packages, owns the rundown state machine, exposes the §16 command API, maintains `stateVersion`, and keeps the audit log. It MUST NOT decode video, composite frames, or mix audio (§5.1). | |
| Render node | normative | v0.1 | The Rust/wgpu process that decodes, composites, and mixes (SPEC §5.2, §5.9). It holds no authority over show state: it applies directives and reports `appliedStateVersion`. All control traffic reaches it through the control plane; direct dashboard-to-render-node control is forbidden (§5.2). | **engine**, `nbe-engine`, `render` role (§5.3) |
| Render channel | normative | v0.3 | The control-plane→render-node protocol: directive frames, engine frames, per-connection `seq`, and reconnect resync (SPEC §5.9). | |

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
| Automation hold | planned | v0.3 | Global automation kill switch; takes effect within 1 frame. | §13.5 calls it "held" (`automationHold` true); not Held (output), nor Held (clock). Always written qualified (the user's ruling of 2026-10-18) |
| Field domain | normative | v0.4.8 | The values one `stateChange` field can take, enumerated per field, and so the only values a rule's `from` or `to` may name. A value outside it refuses the rule at load, by name, in preflight and the control plane alike (SPEC §13.2, row 9). No field borrows another's vocabulary. | `stateChange` domain |
| Tally | planned | v0.3 | Live-source indication, operator- and talent-facing. | |
| Marker | planned | v0.3 | A rundown bookmark; doubles as a recording chapter. | |
| Role | normative | v0.1 | Permissions class: monitor, operator, producer, admin, render. | |
| Item state | normative | v0.3 | The runtime state of a rundown Item (SPEC §17): `READY`, `ARMED`, `LIVE` (an untimed item on the View), `PLAYING` (a timed one), `DONE`, `MISSING`, `ERROR`. An item never written reads `READY`. `itemState`'s domain (§13.2, row 9). | `LIVE` here is not the stream's `live`; ledgered with row 9 |
| Show state | normative | v0.3 | The **runtime** state machine of a loaded Show: `UNLOADED → LOADED → RUNNING → STOPPED` (SPEC §16.1 transitions, §17 item/scene states, §5.9.4 resync snapshot). Owned by the control plane and mirrored to the render node by `show.resync`; it is the `showState` field on the wire. Not to be confused with **Show** (a definition) or **Show package** (a folder). | `showState` |

## Media and assets

| Term | Status | Since | Definition | Aliases / notes |
|---|---|---|---|---|
| Asset | normative | v0.1 | A packaged media file with kind, format, hash, cadence, and loop metadata. | |
| Show package | normative | v0.1 | The self-contained folder on disk: manifest + media + templates + audio (SPEC §6). A **static artifact** — it is loaded, validated and preflighted; it has no runtime state of its own. The runtime is **Show state**. | package |
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
| Held (clock) | normative | v0.1 | The master clock's `HELD` state: an operator freeze, for an emergency (SPEC §11.4). v1 must implement only `STOPPED` and `RUNNING`. Always written qualified, "Held (clock)" or the clock's `HELD`, never a bare "held". | not Held (output), nor Automation hold: three meanings of one word, a known hazard (the user's ruling of 2026-10-18) |
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
