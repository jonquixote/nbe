# NBE SPEC v0.3  
**News Broadcasting Engine**  
Status: normative specification — self-contained  
Relationship to earlier versions: this document supersedes SPEC v0.2.5. It consolidates SPEC v0.1, SPEC v0.2, and the v0.2.1 errata (via the v0.2.5 consolidation) and introduces the v0.3 composable broadcast language: the two-axis model, element identity, the state-diff transition engine, overlays, sub-scenes, automation, plugins, quality profiles, and the abuse model. Where this document differs from prior versions, this document wins. Prior versions remain in `docs/` as history.

---

## 0. Assumptions and phasing honesty

### 0.1 Assumptions

The following assumptions are normative unless changed by spec revision:

1. **Single primary render node for MVP.** The control plane may run on the same machine as the render node.
2. **macOS Apple Silicon is the primary live playout target.** Linux cloud nodes are for guest ingest, distribution, backup, and benchmarking, not the primary local playout path unless explicitly configured.
3. **All live media is pre-normalized.** The live render engine must not transcode, motion-interpolate, or repair media during live playout.
4. **OBS is not a dependency.** OBS is only used as a benchmark baseline and optional Plan B through `obs-websocket`.
5. **Smelter is reference only.** The NBE Rust/wgpu engine is custom. Smelter informs API shape and benchmarking but is not required at runtime.
6. **Companion is the Stream Deck integration path.** Companion emits NBE WebSocket commands. No custom Stream Deck plugin is built for v1.
7. **Show packages are self-contained folders.** All local assets are referenced by relative path from the package root.
8. **The operator is a single human.** UX must minimize cognitive load, favor big state-clear controls, and automate recovery where possible.
9. **Internet independence is mandatory.** Local playout must continue if WAN is lost. Remote guests and streaming may fail gracefully.
10. **House rate is 30 fps for MVP.** 60 fps is manifest-supported for future showcase episodes but is not required for v1 acceptance.
11. **Fonts and graphic templates are packaged.** Text rendering must not depend on host-system fonts unless explicitly declared.
12. **Security is local-first.** v1 assumes a trusted local network or VPN. Auth tokens are used, but full multi-tenant RBAC is not a v1 hard requirement.
13. **RSS ticker content is sanitized.** The ticker renderer must treat RSS text as untrusted display text, not markup or code.
14. **Recording container default is fragmented MP4.** Matroska is allowed, but fragmented MP4 is the default crash-safe container.
15. **Hardware encode is mandatory.** If hardware encoder is unavailable, live streaming/recording must refuse to start rather than fall back to CPU x264.
16. **The normative schema lives at `schemas/manifest.v0.3.json`.** That repository file is the byte-exact normative artifact; any copy embedded in this document is informational. If they ever diverge, the repository file wins and the divergence is a spec bug.
17. **Deprecation aliases.** `program.*` commands are accepted by the control plane for one spec version and map 1:1 to `view.*` commands, emitting a deprecation warning in telemetry. The schema accepts `layer` as an alias for `element` during migration only.
18. **Migration tooling.** A CLI tool `nbe-migrate` converts a v0.2 show package to a v0.3 package. Preflight in a v0.3 engine rejects v0.2 manifests.
19. **WASM sandbox.** Element plugins run in Wasmtime/Wasmer-class runtimes with strict WASI capabilities: no network, no disk writes outside designated temp mounts, no ambient authority.
20. **WGSL sandbox.** Effect plugins are strictly fragment shaders operating on bound textures, validated via `naga`. They cannot execute arbitrary compute shaders that bypass the render graph.
21. **Cloud cost profile.** Cloud is TURN relay, guest ingest, and distribution/backup only, at an estimated envelope of $0.10–$0.50 per broadcast-hour for managed TURN/relay egress. Self-hosted TURN reduces marginal cost toward zero.
22. **Floor device baseline.** The 2019 dual-GPU Intel/Radeon MacBook Pro is the reference floor. The degradation ladder MUST engage gracefully on it.
23. **Sequence recursion.** Time-axis Sequence nesting is capped at 8 levels; preflight warns beyond 4. This is distinct from the space-axis sub-scene depth cap of 4.
24. **Multi-output frame sharing.** Outputs share rendered frames with hardware encoders via GPU texture sharing (Metal `IOSurface` / Vulkan external memory) without CPU readback.

### 0.2 Phasing honesty note

v0.3 is a superset. The MVP hard ceiling (Section 20) and the implementation order (Section 26) still govern what gets built first. v0.3 features (plugins, automation, advanced state-diff transitions) MUST NOT delay the first broadcast. The core playout engine must reach AC-5 (30-minute zero-drop) before advanced compositing features are merged.

---

# 1. Scope and locked decisions

NBE is a purpose-built live news broadcast/playout system.

It replaces an OBS-based prototype with a deterministic, manifest-driven broadcast engine.

## 1.1 Locked decisions

| # | Decision | Normative ruling |
|---|---|---|
| 1 | Engine | Custom Rust engine using `wgpu`. Smelter is reference for API shape and benchmarking. OBS is benchmark/Plan B only. |
| 2 | Control plane | TypeScript/Node rundown engine. WebSocket + JSON command API is the single control bus. Bitfocus Companion drives Stream Deck XL. |
| 3 | House rate | 30 fps default. 60 fps per-show only for showcase episodes. |
| 4 | Platform | macOS-first render node with Apple Silicon VideoToolbox. Linux cloud render node with NVENC for guests/distribution/backup. Windows/browser future preserved via `wgpu`. |
| 5 | Show package | Folder + `manifest.json`. Preflight validator must prove package air-ready before load. |
| 6 | Guest ingest | WHIP/WebRTC remote guests. NDI or HDMI/SDI capture for local sources. Screen/app mirroring forbidden. |
| 7 | Network | 10–20 Mbps up minimum, 100 Mbps aspirational. Local playout survives total internet loss. |
| 8 | Schedule | No dates or timelines. Phases exit by acceptance tests only. |

---

# 2. Normative language

The keywords **MUST**, **MUST NOT**, **REQUIRED**, **SHALL**, **SHALL NOT**, **SHOULD**, **SHOULD NOT**, **RECOMMENDED**, **MAY**, and **OPTIONAL** are to be interpreted as described in RFC 2119.

---

# 3. Normative glossary

Generated from the canonical `VOCABULARY.md` ledger. One term, one definition. If a term is not here or in the ledger, it is not normative.

## 3.1 Time axis — editorial: when things play

| Term | Status | Since | Definition | Aliases / notes |
|---|---|---|---|---|
| Network | normative | v0.1 | The top-level identity: branding, fonts, fallback assets, template library. | |
| Channel | normative (hook only) | v0.1 | A 24/7 programmed stream of Shows. Scheduler is post-v1; schema must not preclude it. | |
| Show | normative | v0.1 | A single program/episode definition: video/audio specs, outputs, fallback. | |
| Rundown | normative | v0.1 | The root Sequence of a Show: the editorial order of play. | |
| Sequence | normative | v0.3 | A recursive, ordered container of Items. Sub-sequences are nested Sequences. Reusable across Shows. | generalizes Rundown/Segment/Subsegment |
| Segment | normative | v0.1 | Conventional top level of a Rundown. IDs A–K by convention; schema allows A–ZZ. | |
| Subsegment | normative | v0.1 | Conventional second level of a Rundown (A1, A2…). | |
| Item | normative | v0.3 | Leaf of a Sequence: scene reference, sequence reference, clip reference, live source reference, or generated slate. | |
| autoFollow | normative | v0.1 | Per-item flag to advance automatically when media ends. | subsumed by Automation |

## 3.2 Space axis — visual: what is on screen

| Term | Status | Since | Definition | Aliases / notes |
|---|---|---|---|---|
| Scene | normative | v0.3 | A named, reusable visual composition: element list + per-element state + audio state. | |
| Sub-scene | normative | v0.3 | A Scene referenced as an Element inside another Scene; rendered once to a texture, reusable N times. | pre-comp; scene-as-source |
| Element | normative | v0.3 | The atomic addressable visual unit, with persistent identity across Scenes. | renames Layer |
| Group | normative | v0.3 | A named collection of Elements moved/toggled as one. | |
| Effect | normative | v0.1 | A stateless visual transform applied to an Element (chroma key, luma key, color correction, blur, mask, border, shadow, crop, custom WGSL). | |
| Transition | normative | v0.1 | An interpolation between two element-state maps. v0.1: cut/mix. v0.3: the state-diff engine (move, wipe, sting, DVE as parameterizations). | |
| Overlay (DSK) | normative | v0.3 | A compositing level applied after the transition; its elements persist across scene changes. `View = overlay(transition(A, B))`. | downstream key |
| Template | normative | v0.1 | A parameterized graphic (lower third, banner, ticker) with typed fields. | |
| Ticker | normative | v0.1 | The scrolling text element; scrolls by texture offset, never re-layout per frame. | |

## 3.3 Buses and outputs

| Term | Status | Since | Definition | Aliases / notes |
|---|---|---|---|---|
| View | normative | v0.3 | The main live/recorded composited output. | renames Program |
| Preview | normative | v0.1 | The staging bus: what transitions into View. | |
| Multiview | normative | v0.3 | Operator grid render target: view, preview, source thumbnails, meters, tally borders. | |
| Fallback slate | normative | v0.1 | The resident emergency image; automatic cut-to target on failure, ≤ 1 frame late. | |

## 3.4 Control

| Term | Status | Since | Definition | Aliases / notes |
|---|---|---|---|---|
| Command bus | normative | v0.1 | The single WebSocket+JSON channel all control traffic passes through. | |
| Envelope | normative | v0.1 | The command message wrapper: `v`, `id`, `command`, `payload`, `baseStateVersion`. | |
| stateVersion | normative | v0.1 | Monotonic control-plane version for optimistic concurrency. | |
| Binding | normative | v0.1 | A trigger→action mapping (Companion key, hotkey, MIDI, OSC, web button). | |
| Preset | normative | v0.3 | A named reusable configuration: element, effect, transition, scene, or audio. | |
| Snapshot | normative | v0.3 | A named, recallable state of the entire View, including overlay visibility. | |
| Automation rule | normative | v0.3 | trigger + conditions → command. Runs through the command bus with the same preconditions as a human. | advanced scene switcher |
| Automation hold | normative | v0.3 | Global automation kill switch; takes effect within 1 frame. | |
| Tally | normative | v0.3 | Live-source indication, operator- and talent-facing. | |
| Marker | normative | v0.3 | A rundown bookmark; doubles as a recording chapter. | |
| Role | normative | v0.1 | Permissions class: monitor, operator, producer, admin, render. | |

## 3.5 Media and assets

| Term | Status | Since | Definition | Aliases / notes |
|---|---|---|---|---|
| Asset | normative | v0.1 | A packaged media file with kind, format, hash, cadence, and loop metadata. | |
| Show package | normative | v0.1 | The self-contained folder: manifest + media + templates + audio. | |
| Mezzanine | normative | v0.1 | The house format every asset is normalized to (CFR, house rate, H.264 High, 48 kHz, −16 LUFS). | normalized format |
| Loop | normative | v0.1 | Deterministic master-clock modulo playback: `frame(t) = (F − t0) mod P`. No restart events. | |
| Cadence | normative | v0.1 | Source frame-rate character, preserved via frame holds. | |
| Pulldown | normative | v0.2 | The declared frame-hold pattern for non-house rates (`pattern`, `repeatNthSourceFrame`, `repeatOnePerNSourceFrames`). | |
| Preflight | normative | v0.1 | The CI-runnable proof that a show package is air-ready. | |
| Plugin package | normative | v0.3 | A WASM element plugin or WGSL effect plugin with manifest, version pin, and permission list (deny by default). | |

## 3.6 Audio

| Term | Status | Since | Definition | Aliases / notes |
|---|---|---|---|---|
| Bus | normative | v0.1 | A named audio channel group: mic, clip, music, sfx, guest, master, guestReturn, ifb. | |
| Mix-minus | normative | v0.2 | The guest return mix excluding that guest's own audio. Mandatory per guest. | |
| IFB | normative | v0.2 | The anchor monitor bus: program minus anchor mic, plus talkback. | |
| Ducking | normative | v0.1 | Automatic music attenuation under speech. | |
| Soundboard | normative | v0.1 | RAM-resident SFX triggered in under 20 ms. | |
| AFV | normative | v0.3 | Audio-follow-video: the named mode for audio tracking the taken source. | implicit in audioPolicy previously |
| PFL | normative | v0.3 | Pre-fade listen; formalizes monitor-only solo. | |

## 3.7 Operations and reliability

| Term | Status | Since | Definition | Aliases / notes |
|---|---|---|---|---|
| Master clock | normative | v0.1 | The single monotonic show clock everything derives from. | |
| Watchdog | normative | v0.1 | Frame-deadline monitor that triggers fallback. | |
| Telemetry | normative | v0.1 | The per-second engine state stream. | |
| House rate | normative | v0.1 | The output frame rate: 30 fps default, 60 for showcase. | |
| Quality profile | normative | v0.3 | A hardware-probed performance envelope selected at startup: `potato`, `consumer`, `pro`, `reference`. | |
| Degradation ladder | normative | v0.3 | The ordered yield list under load: preview fps, loop caches, effect quality, multiview — View is never degraded. | |
| Captions | normative | v0.3 | Sidecar text output (WebVTT-class) alongside the stream; burn-in later via an Overlay element. | |

## 3.8 Deprecated terms

| Deprecated term | Replacement | Since | Alias compatibility |
|---|---|---|---|
| Program (bus) | View | v0.3 | `program.*` commands map 1:1 to `view.*` for one version |
| Layer | Element | v0.3 | schema accepts `layer` as alias for `element` during migration only |
| Scene collection | — | — | OBS-ism; forbidden |
| OS-level hotkey | Binding | v0.1 | — |

---

# 4. Two-axis system hierarchy

v0.3 splits the single v0.1 hierarchy into two orthogonal axes.

## 4.1 Time axis (editorial: when things play)

```text
Network
  └── Channel         // future 24/7 scheduler; schema-only in v1
       └── Show
            └── Rundown (root Sequence)
                 └── Sequence (recursive, cap 8; preflight warns beyond 4)
                      └── Item (references a Scene)
```

## 4.2 Space axis (visual: what is on screen)

```text
Scene
  └── Sub-scene (Scene as Element, depth cap 4, DAG)
       └── Element (persistent ID)
            └── Effect / Plugin
```

## 4.3 The bridge

An `Item` on the time axis references a `Scene` on the space axis via `sceneRef`. When an Item becomes active, its referenced Scene is instantiated on the View or Preview bus.

## 4.4 Migration

v0.2 packages migrate mechanically via `nbe-migrate` (Section 6.7). The v0.1/v0.2 model (Segment → Subsegment → LayerStack → Layer) maps onto Sequence → Item → Scene → Element.

---

# 5. Subsystem 1 — Show/Rundown control plane

## 5.1 Responsibilities

The control plane is the authoritative show-state owner.

It MUST:

1. Load and validate show packages.
2. Own the rundown state machine.
3. Expose the WebSocket JSON command API.
4. Validate all commands against schema and current state.
5. Emit state-change events to all connected clients.
6. Translate operator commands into render-node directives.
7. Maintain monotonic `stateVersion`.
8. Enforce preconditions before allowing live transitions.
9. Persist last known show state locally for crash recovery.
10. Provide a snapshot API for dashboards and iPhone clients.
11. Vend time-limited TURN credentials.
12. Maintain the append-only audit log of control-plane actions and auth events.

It MUST NOT:

1. Decode video.
2. Composite frames.
3. Mix final audio.
4. Depend on OBS.
5. Require internet connectivity for local playout.

## 5.2 Runtime topology

```text
+-------------------+       WebSocket JSON       +-------------------+
| Web dashboard     | <------------------------> |                   |
+-------------------+                            |                   |
                                                 |   Control Plane   |
+-------------------+       WebSocket JSON       |   Node/TypeScript |
| iPhone controller | <------------------------> |                   |
+-------------------+                            |                   |
                                                 |                   |
+-------------------+       WebSocket JSON       |                   |
| Companion bridge  | <------------------------> |                   |
+-------------------+                            +---------+---------+
                                                           |
                                                  WebSocket JSON
                                                           |
                                                 +---------v---------+
                                                 | Rust Render Node  |
                                                 | wgpu compositor   |
                                                 +-------------------+
```

All control traffic MUST pass through the control plane. Direct dashboard-to-render-node control is forbidden in v1.

## 5.3 WebSocket endpoint

Default local endpoint:

```text
ws://127.0.0.1:8462/nbe/v0.3
```

TLS endpoint for remote/VPC use:

```text
wss://render.local:8463/nbe/v0.3
```

Connection handshake MUST include:

```http
Authorization: Bearer <token>
X-NBE-Role: operator|producer|monitor|admin|render
```

Roles:

| Role | Permissions |
|---|---|
| `monitor` | read state/telemetry only |
| `operator` | live commands, take, graphics, audio, record/stream |
| `producer` | load/preflight/edit rundown/ticker |
| `admin` | all commands, config, auth |
| `render` | internal render-node directive channel |

## 5.4 Message envelope

All client-to-server command messages MUST use this envelope:

```json
{
  "v": "0.3",
  "id": "0d9f5c6a-7b8a-4a61-9b0a-5f5a5c8d99ab",
  "command": "view.take",
  "payload": {},
  "baseStateVersion": 412
}
```

Fields:

| Field | Type | Required | Description |
|---|---:|---:|---|
| `v` | string | yes | Protocol version. |
| `id` | UUID | yes | Client-generated request ID. |
| `command` | string | yes | Command name. |
| `payload` | object | yes | Command-specific payload. May be `{}`. |
| `baseStateVersion` | integer | no | If present, command is rejected on version conflict. |

Server response:

```json
{
  "v": "0.3",
  "requestId": "0d9f5c6a-7b8a-4a61-9b0a-5f5a5c8d99ab",
  "status": "ok",
  "stateVersion": 413,
  "data": {}
}
```

Error response:

```json
{
  "v": "0.3",
  "requestId": "0d9f5c6a-7b8a-4a61-9b0a-5f5a5c8d99ab",
  "status": "error",
  "stateVersion": 412,
  "error": {
    "code": "E_FORBIDDEN_STATE",
    "message": "No preview item armed."
  }
}
```

## 5.5 State versioning

The control plane MUST maintain a monotonically increasing integer `stateVersion`.

A command with `baseStateVersion` not equal to current state version MUST fail with:

```text
E_VERSION_CONFLICT
```

Commands without `baseStateVersion` are accepted if otherwise valid.

## 5.6 View and Preview buses

The engine MUST maintain two logical video buses:

| Bus | Meaning |
|---|---|
| `PREVIEW` | The staging environment: what is prepared to go live. |
| `VIEW` | The main live/recorded composited output. |

The preview bus MUST be independently rendered and visible in operator UI.

A TAKE operation MUST promote the preview bus to view using the requested transition.

If no preview is armed, TAKE MUST fail.

The v0.1/v0.2 name for View is Program; `program.*` commands remain as deprecated aliases for one spec version (Assumption 17).

## 5.7 Item references

Commands use item references:

| Reference | Meaning |
|---|---|
| `A` | Sequence/Segment A |
| `A1` | Item/Subsegment A1 |
| `element:A.lowerThird` | Element with ID `lowerThird` in the active scene for A |
| `scene:SCN_A1` | Scene ID |
| `overlay:ticker` | Overlay ID |
| `guest:remote_1` | Guest source ID |
| `camera:main` | Camera source ID |

Subsegment IDs SHOULD match the `A1`, `A2`, `B1`, etc. convention.

## 5.8 Operator topology (normative)

The anchor drives. A producer or second operator MAY join remotely.

The View is a served endpoint: any authorized client watches it over WHEP (Section 9.6) with hardware decode, in a browser or on a phone. Virtual-camera patterns are forbidden: the engine MUST NOT do GPU-readback detours to make the View visible to a remote operator.

Guest onboarding is one link, no install, browser capture — obs.ninja-class UX — with the mix-minus return and TURN vending built in (Sections 8.6, 9.6).

---

# 6. Subsystem 2 — Media asset pipeline

## 6.1 Responsibilities

The asset pipeline prepares show packages before live load.

It MUST provide:

1. Ingest from source media.
2. Transcode/normalize to house format.
3. Cadence preservation.
4. Loudness normalization.
5. Thumbnail generation.
6. Contact-sheet generation.
7. Alpha/loop validation.
8. Preflight report generation.
9. Asset hashing.
10. Missing-asset detection.
11. Plugin package validation (Section 14).

It MUST NOT:

1. Use variable frame rate output.
2. Use motion interpolation unless asset explicitly declares `cadence: interpolate`.
3. Use VP9/WebM alpha as a live format.
4. Rely on live transcoding during playout.

## 6.2 Normalized video format

All normal video assets MUST be:

| Property | Requirement |
|---|---|
| Resolution | Show resolution, default `1920x1080` |
| Frame rate | House rate, default `30 fps` |
| Frame-rate mode | CFR only |
| Codec | H.264 High Profile |
| Pixel format | `yuv420p` |
| Keyframe interval | ≤ 1 second, i.e. ≤ 30 frames at 30 fps |
| Audio | AAC or PCM, 48 kHz |
| Loudness target | `-16 LUFS` integrated |
| True peak | `-1.5 dBTP` max |
| Container | MP4 or MOV |

Reference FFmpeg shape:

```bash
ffmpeg -i input.mov \
  -vf "scale=1920:1080:force_original_aspect_ratio=decrease,pad=1920:1080:(ow-iw)/2:(oh-ih)/2,fps=30,format=yuv420p" \
  -c:v libx264 -profile:v high -preset slow -crf 18 \
  -g 30 -keyint_min 30 -sc_threshold 0 \
  -c:a aac -b:a 192k -ar 48000 \
  -af loudnorm=I=-16:TP=-1.5:LRA=11 \
  output.mp4
```

The live engine MAY use hardware decode for H.264. The asset pipeline MAY use CPU encoding; the live path MUST NOT.

## 6.3 Alpha loops

Alpha loops MUST use one of:

| Format | Status |
|---|---|
| ProRes 4444 | Preferred on Apple Silicon |
| HAP Alpha | Preferred when supported by engine |
| PNG sequence | Fallback |
| VP9/WebM alpha | Forbidden live format |

Alpha assets MUST contain a real alpha channel. Preflight MUST fail if alpha is absent.

## 6.4 Audio assets

Audio assets MUST be:

| Property | Requirement |
|---|---|
| Sample rate | 48 kHz |
| Bit depth | 16-bit PCM minimum, 24-bit preferred |
| Channels | mono or stereo |
| Loudness | normalized to show target |
| Soundboard assets | fully preloadable into RAM |

## 6.5 Graphics and fonts

Graphics MUST be template-driven.

Required template classes:

1. Lower-third headline.
2. Lower-third name/location.
3. Breaking banner.
4. Ticker.
5. Clock.

Text requirements:

| Requirement | Mandatory |
|---|---|
| Unicode | yes |
| UTF-8 | yes |
| RTL scripts | yes |
| Multilingual fields | yes |
| Font packaging | yes |
| Ticker GPU texture scroll | yes |
| Per-frame full text relayout | forbidden except content change |

Ticker rendering MUST scroll by texture offset or equivalent GPU method. It MUST NOT re-layout the whole ticker string every frame unless ticker content changes.

## 6.6 Preflight

`preflight(show_package)` MUST be runnable locally and in CI.

It MUST produce:

```text
preflight_report.json
contact_sheet.jpg
thumbnails/*.jpg
```

It MUST validate:

1. Manifest JSON schema validity.
2. Manifest semantic validity.
3. Existence of every referenced asset.
4. SHA-256 match when provided.
5. First and last frame decode for every video/alpha asset.
6. Loop duration matches `loop.periodFrames`.
7. Constant frame rate.
8. Correct resolution.
9. Correct house frame rate.
10. Correct cadence pattern.
11. Audio sample rate.
12. Integrated loudness.
13. True peak.
14. Alpha presence for alpha assets.
15. Font availability for templates.
16. Template field completeness.
17. Fallback asset existence and decodability.
18. No forbidden formats.
19. No VFR assets.
20. No missing control binding command.
21. Effective loop metadata present for every `videoLoop` element (see Section 12).
22. Loop budget math (see Section 12 and AC-21).
23. Scene-reference integrity: every `sceneRef` resolves to a declared scene; every `pluginId` resolves to a declared plugin; every group `children` entry resolves within the scene.
24. Scene graph is a DAG: no circular sub-scene references.
25. Automation rules pass cycle detection (Section 13).
26. Plugin sandbox validation (Section 14).

Preflight MUST fail on seeded:

1. Missing asset.
2. VFR clip.
3. Incorrect resolution.
4. Broken loop period.
5. Missing fallback asset.
6. Loudness out of tolerance.
7. Invalid manifest field.
8. Circular sub-scene reference.
9. Self-triggering automation rule.
10. Plugin failing sandbox validation.

Preflight result MUST be machine-readable.

## 6.7 v0.2 → v0.3 migration rules

The migration from v0.2 to v0.3 MUST be mechanical and preflight-verified, performed by the `nbe-migrate` CLI:

1. A v0.2 `Segment` becomes a v0.3 `Sequence` (e.g., `SEQ_A`).
2. A v0.2 `Subsegment` becomes a v0.3 `Item` referencing a generated `Scene`.
3. A v0.2 `LayerStack` becomes a v0.3 `Scene` (e.g., `SCN_A1`).
4. A v0.2 `Layer` becomes a v0.3 `Element`, preserving the `id`.
5. Preflight MUST verify that a v0.2 package run through `nbe-migrate` produces a valid v0.3 package with identical visual and audio output.

Preflight in a v0.3 engine rejects v0.2 manifests (Assumption 18).

---

# 7. Subsystem 3 — Playout/compositor engine

## 7.1 Implementation constraints

The render node MUST be implemented in Rust.

It MUST use `wgpu` for GPU compositing.

Backend targets:

| Platform | Backend |
|---|---|
| macOS | Metal via `wgpu` |
| Linux | Vulkan via `wgpu` |
| Windows | DX12 via `wgpu`, future |
| Browser | WebGPU, future |

The render node MUST NOT:

1. Use OBS as the core compositor.
2. Use CPU x264 for live encoding.
3. Use screen/app mirroring as an ingest method.
4. Depend on the public internet for local playout.

## 7.2 Render model

The compositor MUST use a GPU scene graph.

Per frame:

```text
1. Read master clock frame number.
2. Resolve VIEW item/scene and PREVIEW item/scene.
3. Resolve scene elements (including extensions and sub-scenes).
4. For each visible element:
   a. obtain source texture,
   b. apply transform/crop/opacity/effects,
   c. assign z-order.
5. Composite elements from low z to high z (M/E level).
6. Apply transition interpolation if a transition is running.
7. Composite overlay (DSK) level.
8. Render VIEW target.
9. Render PREVIEW target.
10. Submit frames to display/encoder.
11. Emit telemetry.
```

The compositor MUST be frame-deterministic for a given master-clock frame and show state.

## 7.3 Element kinds

The engine MUST support the following element kinds:

| Kind | Source |
|---|---|
| `videoLoop` | looping normal or alpha video |
| `clip` | one-shot video |
| `camera` | local camera/capture device |
| `guest` | WHIP/WebRTC remote guest |
| `graphic` | template-generated graphic |
| `ticker` | scrolling ticker |
| `clock` | wall clock or show clock |
| `sceneRef` | sub-scene reference (Section 7.7) |
| `group` | named collection of elements (Section 7.6) |
| `plugin` | WASM/WGSL plugin element (Section 14) |

## 7.4 Element identity and state model

Elements have persistent identity across scenes. The same element `id` in two scenes is the same element.

Element state comprises:

```text
transform (x, y, w, h, crop)
opacity
visibility
effect parameters
audio parameters (bus, gainDb, muted)
```

Persistent identity is what makes Move-class transitions possible (Section 7.9).

## 7.5 Scenes

A scene is a named, reusable visual composition: an element list plus per-element state plus optional audio state.

Scenes are declared top-level in the manifest and referenced by rundown items. A scene is self-contained.

## 7.6 Groups

An element of kind `group` names a collection of sibling elements (`children`). Group operations (move, toggle, opacity) apply to all children as one. Groups are scene-local.

## 7.7 Sub-scenes

A scene may be referenced as an element inside another scene (`kind: "sceneRef"`), rendered once to a texture and reused N times.

Rules:

1. Recursion depth cap: 4.
2. Scenes form a directed acyclic graph. Preflight MUST reject circular references.
3. A sub-scene renders to its texture at the show resolution unless the element transform declares otherwise.

## 7.8 Scene extension

The v0.2 layer-stack merge capability is preserved in scene terms. A scene MAY declare a base scene:

```json
{
  "id": "SCN_B2",
  "base": "SCN_B",
  "mergeMode": "inherit"
}
```

Merge modes:

| Mode | Behavior |
|---|---|
| `inherit` | Extending scene's elements merge over the base scene's elements. |
| `replace` | Extending scene's element list replaces the base scene's. |
| `merge` | Explicit merge by element ID. |

Merge rule for `inherit` and `merge`:

1. Start with base scene elements.
2. Replace any base element with the same `id` if present in the extending scene.
3. Append extending-only elements.
4. Sort final list by ascending `z`.
5. Apply visibility.

## 7.9 Transitions and the state-diff engine

A transition is an interpolation between two element-state maps keyed by element ID.

Given the outgoing scene state and the incoming scene state:

- Elements present in both: tween their properties (position, size, opacity, effect parameters).
- Elements only incoming: enter animation.
- Elements only outgoing: exit animation.

Named transition kinds are parameterizations of this engine:

| Kind | Definition |
|---|---|
| `cut` | zero-duration tween |
| `mix` | whole-frame opacity tween over `durationFrames` |
| `wipe` | mask tween |
| `sting` | alpha overlay + audio with a defined cut point |
| `move` | transform tweens on shared elements |
| `dve` | transform tween on a single featured element (e.g., PiP spring-to-fullscreen) |

Easings: `linear`, `easeIn`, `easeOut`, `easeInOut`, `cubicBezier`, `spring`. Per-element duration, delay, stagger, and path.

v1 MUST support `cut` and `mix` (Section 20). The remaining kinds are schema-supported and post-MVP.

The state diff MUST be precomputable at arm time, so the take-latency guarantee holds for arbitrarily complex moves.

Transitions MUST be quantized to master-clock frame boundaries.

Default crossfade duration:

```text
15 frames at 30 fps = 0.5 seconds
```

Transition presets are named, reusable transition configurations bindable to hotkeys (manifest `transitions` array).

### 7.9.1 Take latency (normative)

For a command accepted on localhost:

```text
takeLatency = firstVisibleViewChangeFrame - commandAcceptedFrame
```

For `cut`:

```text
takeLatency <= 2 frames
```

For `mix`:

```text
first mixed frame MUST appear by commandAcceptedFrame + 1
full mix completion MUST occur by durationFrames + 1
```

This applies to the local VIEW compositor output, not to downstream stream latency. See AC-17.

## 7.10 Overlay level (DSK)

Composition order:

```text
View = overlay(transition(sceneA, sceneB))
```

Overlay elements (ticker, logo bug, breaking banner, clock) live on the overlay level, composited after the transition. They persist across scene transitions.

Overlays have independent `overlay.show` / `overlay.hide` commands with their own enter/exit animations.

## 7.11 Chroma key

The chroma key effect MUST be GPU-shader-based.

Parameters:

| Parameter | Range | Default |
|---|---:|---:|
| `enabled` | bool | true |
| `color` | green/blue/custom | green |
| `customColorHex` | `#RRGGBB` | n/a |
| `tolerance` | 0.0–1.0 | 0.30 |
| `softness` | 0.0–1.0 | 0.20 |
| `spillSuppression` | 0.0–1.0 | 0.50 |
| `edgeFeather` | 0.0–1.0 | 0.10 |

The keyer MUST support a garbage matte.

Chroma key MUST run in real time at 1080p30 on Tier-1 hardware.

## 7.12 DVE and PiP

The architecture MUST support DVE transforms.

v1 MVP only requires static PiP guest placement and cut between PiP/full layouts.

Normalized transform space:

```text
x: 0.0 = left
y: 0.0 = top
w: 1.0 = full canvas width
h: 1.0 = full canvas height
```

Example PiP:

```json
{
  "x": 0.66,
  "y": 0.05,
  "w": 0.30,
  "h": 0.30
}
```

## 7.13 Frame budget

For 1080p30:

```text
frame deadline = 33.333 ms
```

A frame is dropped if VIEW output is not submitted by its deadline.

Target budget on Tier-1:

| Stage | Target |
|---|---:|
| state resolution | < 1 ms |
| element graph eval | < 2 ms |
| GPU render | < 8 ms |
| encode submission | < 2 ms |
| OS/driver slack | remainder |

The engine MUST NOT block the render loop on:

1. control-plane WebSocket I/O,
2. thumbnail generation,
3. RSS fetch,
4. non-critical disk writes,
5. telemetry flush.

## 7.14 Fallback slate

The show manifest MUST define `fallbackAssetId`.

The fallback asset MUST be resident in memory/VRAM after show load.

On segment failure, the engine MUST cut to fallback slate no later than one frame after the failure deadline.

Fallback triggers:

1. Missing live asset.
2. Decode failure.
3. Camera device loss.
4. Guest source loss while live.
5. GPU render fault.
6. Watchdog miss.
7. Unrecoverable state error.

Fallback MUST be automatic and MUST NOT require operator action.

---

# 8. Subsystem 4 — Audio engine

## 8.1 Core requirements

The audio engine MUST run at:

```text
48 kHz
float32 internal processing
```

Audio MUST be synchronized to the master show clock.

The audio graph MUST include these buses:

| Bus | Purpose |
|---|---|
| `mic` | anchor microphone |
| `clip` | clip/subsegment audio |
| `music` | music bed |
| `sfx` | soundboard effects |
| `guest` | remote guest audio |
| `master` | final mix |
| `guestReturn` | per-guest mix-minus return (see 8.6) |
| `ifb` | anchor monitor/talkback (see 8.6) |

## 8.2 Bus controls

Each bus MUST support:

| Control | Range |
|---|---:|
| gain | -60 dB to +12 dB |
| mute | boolean |
| meter | peak + RMS |
| solo | boolean, monitor only (PFL) |

The master bus MUST have:

1. compressor,
2. limiter,
3. loudness-safe output,
4. peak metering.

## 8.3 Ducking

The music bus MUST support ducking.

Default duck behavior:

| Parameter | Default |
|---|---:|
| depth | -6 dB |
| attack | 10 ms |
| release | 250 ms |
| trigger | manual `audio.duck` or voice-detected mic |

Ducking MUST NOT affect `mic` or `guest` buses unless explicitly configured.

## 8.4 Soundboard

Soundboard assets MUST be preloaded into RAM at show load.

Trigger latency MUST be under:

```text
20 ms
```

on Tier-1 hardware.

Soundboard playback MUST NOT cause dropped video frames.

## 8.5 Guest audio (inbound)

Guest audio from WHIP/WebRTC MUST pass through:

1. jitter buffer,
2. echo cancellation,
3. noise suppression,
4. automatic gain control,
5. loudness normalization toward house target.

Default jitter buffer:

| Condition | Target |
|---|---:|
| good network | 200 ms |
| variable network | 300–500 ms |
| hard maximum | 1000 ms |

If guest audio fails, the guest element MUST be muted automatically.

## 8.6 Guest return, mix-minus, and IFB

### 8.6.1 Guest return requirement

For every connected guest, the engine MUST create a return audio mix.

Guest return MUST be mix-minus:

```text
guestReturn(guestId) = programReturnMix - that guest's own inbound audio
```

The guest MUST NOT receive their own voice from the NBE return path.

This applies even if the guest is muted in program.

### 8.6.2 Default guest return mix

For guest `G`:

```text
guestReturn(G) =
    mic
  + clip
  + music
  + sfx
  + all guest buses except G
  + master insert effects where safe
```

It MUST exclude:

```text
guestBus(G)
```

It SHOULD exclude any effect return that contains `guestBus(G)`.

### 8.6.3 Guest return transport

The guest return mix MUST be sent as the outbound audio track of the guest’s WebRTC session.

Guest return MUST be independent from the master program mix.

Guest return MUST support:

| Control | Requirement |
|---|---|
| gain | -60 dB to +12 dB |
| mute | boolean |
| metering | peak/RMS |
| mode | `programMinusSelf`, `producerMix`, `mute` |

Default mode:

```text
programMinusSelf
```

### 8.6.4 Anchor IFB

An `ifb` bus is defined.

Default anchor IFB mix:

```text
ifb = program mix - anchor mic + talkback
```

If no talkback source exists:

```text
ifb = program mix - anchor mic
```

IFB is intended for anchor monitoring and producer interruption. It is not required in the public program output.

### 8.6.5 Echo prevention

The engine MUST guarantee that a guest’s own audio does not enter their own return path.

If a guest is connected through WHIP/WebRTC:

1. inbound guest audio enters `guestBus(guestId)`,
2. `guestBus(guestId)` may enter program mix,
3. `guestBus(guestId)` MUST NOT enter `guestReturn(guestId)`.

A failure of this rule is an `E_AUDIO` fault.

## 8.7 Audio behavior during transitions

### 8.7.1 Click-free rule

All audio gain changes MUST be click-free.

Minimum ramp:

```text
5 ms
```

Default ramp:

```text
10 ms
```

Maximum default ramp:

```text
50 ms
```

Hard sample-step cuts are forbidden.

### 8.7.2 `view.take` audio behavior

`view.take` payload includes an optional `audio` object (see Section 16).

### 8.7.3 Audio transition modes

| Mode | Behavior |
|---|---|
| `follow` | Follow the item's `audioPolicy` (AFV). |
| `crossfade` | Crossfade outgoing and incoming audio over `audio.durationFrames`. |
| `cut` | Cut audio at boundary, but apply click-free ramp. |
| `mute` | Incoming audio muted; outgoing audio ramped out. |

### 8.7.4 Interaction with `audioPolicy`

When `audio.transition = follow`:

| Item `audioPolicy` | Behavior |
|---|---|
| `clip` | Clip audio is active and crossfaded or ramped according to transition. |
| `bed` | Clip audio is muted; music bed continues. |
| `mute` | Incoming clip audio is muted; outgoing audio ramped out. |

### 8.7.5 Video `mix` default

If video transition is `mix` and no audio override is given, audio MUST crossfade over the same duration.

Crossfade curve:

```text
equal-power crossfade
```

Linear crossfade is allowed, but equal-power is recommended.

### 8.7.6 Video `cut` default

If video transition is `cut`, audio MUST follow `audioPolicy` and MUST apply at least a 5 ms ramp at any start/stop boundary.

### 8.7.7 Live camera and guest audio

For live camera and guest sources:

| Transition | Audio behavior |
|---|---|
| `cut` | 10 ms ramp by default |
| `mix` | crossfade over transition duration |
| `mute` | ramp out and mute |

---

# 9. Subsystem 5 — Output/distribution

## 9.1 Outputs

The engine MUST support:

1. Local full-screen view display.
2. Local preview display.
3. Local crash-safe recording.
4. One live streaming output in MVP: RTMP or SRT.
5. WHIP output as future/contribution output.

MVP hard ceiling:

```text
1 local display output
1 preview output
1 recording output
1 RTMP or SRT output
```

## 9.2 Hardware encoding

Live encoding MUST use hardware encoders only.

| Platform | Encoder |
|---|---|
| Apple Silicon | VideoToolbox H.264 or HEVC |
| Linux/NVIDIA | NVENC H.264 or HEVC |

CPU x264 in the live path is forbidden.

If no hardware encoder is available, output start MUST fail with:

```text
E_NO_HARDWARE_ENCODER
```

## 9.3 Recording

Recording MUST be crash-safe.

Default container:

```text
fragmented MP4
```

Allowed alternative:

```text
Matroska
```

Recording MUST remain playable if the process is killed with `SIGKILL`.

Fragment policy:

| Property | Requirement |
|---|---:|
| fragment interval | ≤ 1 second |
| moov placement | fragmented/init segment safe |
| audio interleaving | yes |
| finalization required | no |

Recording MUST include:

1. program video,
2. master audio,
3. timecode metadata if available.

Markers (Section 10.6) SHOULD be written as recording chapters where the container supports them.

## 9.4 Streaming

Default MVP stream:

| Property | Value |
|---|---|
| Protocol | RTMP or SRT |
| Video | H.264 High |
| Resolution | 1920x1080 |
| Frame rate | 30 fps |
| Video bitrate | 6–12 Mbps recommended |
| Audio | AAC 48 kHz |
| Audio bitrate | 192 kbps |
| Keyframe interval | 1 second |

Stream failure MUST NOT stop local playout.

Stream reconnect MUST be automatic.

## 9.5 Local network survivability

Local playout MUST continue if:

1. WAN is lost,
2. RTMP endpoint is unreachable,
3. WHIP guest connection drops,
4. RSS ticker feed fails.

In those cases:

| Component | Behavior |
|---|---|
| local playout | continue |
| recording | continue |
| stream | retry/backoff |
| guest element | placeholder or fallback if live |
| RSS ticker | last cached items or manual items |

## 9.6 WHIP auth, TURN vending, NDI feature flag, WHEP preview

### 9.6.1 WHIP authentication

WHIP ingest endpoints MUST require bearer authentication.

Example:

```http
POST /nbe/v0.3/whip/guest/GUEST_ID
Authorization: Bearer <guest-token>
```

If token is missing or invalid, the endpoint MUST return HTTP 401.

Guest links are JWT-signed, expiring, limited-use, and revocable by `jti` (Section 10.7).

### 9.6.2 TURN credential vending

The control plane MUST vend time-limited TURN credentials.

Credentials MAY be returned by:

```text
guest.connect
guest.getTurn
```

Credential response shape:

```json
{
  "uris": [
    "turn:turn.nbe.local:3478?transport=udp"
  ],
  "username": "1768000000:guest1",
  "credential": "redacted",
  "ttlSec": 600
}
```

TURN credentials MUST expire.

Failure to vend credentials MUST return `E_TURN`.

ICE failure MUST return `E_ICE`.

Self-hosted TURN (e.g., coturn via environment configuration) is the default posture. Managed TURN provider integrations are a later option (Section 25).

### 9.6.3 NDI feature flag

NDI is optional.

The core build MUST be NDI-free unless explicitly enabled.

Manifest or node config MAY declare:

```json
{
  "features": {
    "ndi": {
      "enabled": false
    }
  }
}
```

If NDI is disabled:

1. NDI camera sources MUST NOT initialize.
2. Preflight MUST fail or warn according to target profile.
3. Runtime commands referencing NDI sources MUST fail with `E_UNSUPPORTED_FEATURE`.

### 9.6.4 WHEP preview for iPhone and remote operators

Preview and View MUST be served over WHEP.

Endpoints:

```text
POST /nbe/v0.3/whep/program
POST /nbe/v0.3/whep/preview
```

Authentication:

```http
Authorization: Bearer <controller-token>
```

MJPEG fallback:

```text
GET /nbe/v0.3/mjpeg/program
GET /nbe/v0.3/mjpeg/preview
```

MJPEG MUST be disabled by default and enabled only in dev mode.

This is the remote-operator path (Section 5.8): the producer sees what the audience sees, hardware-decoded, with no virtual-camera detour.

## 9.7 Multi-output unification (normative)

One composite produces one GPU frame. Display, recording, streaming, and preview outputs are hardware-encoder sessions sharing those rendered frames via GPU texture sharing (Metal `IOSurface`, Vulkan external memory).

Running record + stream concurrently MUST NOT add CPU load beyond encoder-session overhead, and MUST NOT recomposite.

---

# 10. Subsystem 6 — Monitoring, reliability, abuse, degradation

## 10.1 Telemetry

The engine MUST emit telemetry at least once per second.

Telemetry fields:

```json
{
  "ts": 1768000000000,
  "masterClockFrame": 54000,
  "viewItem": "B2",
  "previewItem": "B3",
  "droppedFramesTotal": 0,
  "renderGpuTimeMs": 4.7,
  "decodeSessions": 4,
  "vramUsedMib": 1830,
  "textureCacheUsedMib": 512,
  "streamState": "live",
  "streamBufferMs": 210,
  "recordState": "recording",
  "recordSpaceMib": 512000,
  "masterClockDriftMs": 0.2,
  "fallbackActive": false,
  "qualityProfile": "consumer",
  "degradationRung": 0,
  "automationHold": false
}
```

## 10.2 Dropped-frame definition

A dropped frame is any VIEW frame not presented/submitted by its master-clock deadline.

Preview-only misses are not counted as live dropped frames but MUST be logged as preview misses.

## 10.3 Watchdog

The render node MUST implement a frame watchdog.

If the render loop misses a deadline by more than:

```text
1 frame
```

the watchdog MUST:

1. log fault,
2. increment fault counter,
3. activate fallback slate if the fault affects VIEW.

## 10.4 Health endpoint

The control plane MUST expose:

```text
GET /nbe/v0.3/status
```

Response MUST include:

1. show load state,
2. master clock state,
3. render node health,
4. stream health,
5. recording health,
6. preflight state,
7. last error.

## 10.5 Quality profiles and the degradation ladder

The engine probes hardware at startup and selects a named quality profile:

```text
potato | consumer | pro | reference
```

Normative yield order under sustained load:

1. Preview frame rate.
2. Loop caches evict to streaming.
3. Effect quality.
4. Multiview tiles.

The View MUST NOT be degraded. Telemetry MUST expose the current ladder rung as `degradationRung`.

## 10.6 Coverage additions

- **Multiview**: operator grid render target composited from existing textures: view, preview, source thumbnails, meters, tally borders.
- **Snapshots**: named, recallable state of the entire View, including overlay visibility.
- **Markers**: rundown bookmarks that double as recording chapters.
- **Tally**: live-source indication (borders/labels), operator- and talent-facing.
- **Captions**: WebVTT-class sidecar output alongside the stream. Later burn-in uses a dedicated Overlay element on the DSK level (Section 25).

## 10.7 Abuse and moderation model (first-class)

This is a worker-network broadcast system; assume hostile attention.

1. **Guest links**: JWT-signed, expiring, limited-use, revocable by `jti`. Revocation takes effect on the control plane immediately.
2. **Ticker**: rate limiting on RSS and manual injection (flood protection). RSS text remains sanitized display text, never markup (Assumption 13).
3. **Call-ins**: guest admission is always producer-gated; there is no anonymous path to air.
4. **Audit log**: append-only structured JSON log of all control-plane actions and auth events, retained locally.

## 10.8 Failure UI

Operator UI MUST show:

| State | Color/indication |
|---|---|
| READY | neutral/gray |
| ARMED | yellow |
| LIVE | red |
| PLAYING | green |
| MISSING | flashing red outline |
| ERROR | red banner |
| FALLBACK | full-screen warning |

---

# 11. Cross-cutting concern — Master clock

## 11.1 Authority

There MUST be one master show clock.

All of the following MUST derive timing from it:

1. video playout,
2. audio playout,
3. graphics animation,
4. ticker scroll,
5. transitions,
6. recording timestamps,
7. telemetry frame numbers,
8. loop phase.

## 11.2 Clock source

The master clock MUST be based on a monotonic system clock.

It MUST NOT use wall-clock time as its primary source.

Wall-clock time MAY drive the `clock` element, but not frame scheduling.

## 11.3 Clock epoch

The master clock epoch is set by:

```text
show.start
```

After start:

```text
masterTimeSeconds = monotonicNow - epoch
masterFrame = floor(masterTimeSeconds * houseFrameRate)
```

For 30 fps:

```text
masterFrame = floor(masterTimeSeconds * 30)
```

## 11.4 Clock states

| State | Meaning |
|---|---|
| `STOPPED` | no frame advancement |
| `RUNNING` | normal show clock |
| `HELD` | operator freeze, emergency |
| `SLAVE` | optional future sync to external timecode |

v1 MUST implement `STOPPED` and `RUNNING`.

## 11.5 Drift policy

### 11.5.1 Local deterministic sources

For local sources:

```text
audio/video sync MUST remain within ±1 frame of the master show clock over a 30-minute show.
```

Local sources include:

1. preloaded clips,
2. background loops,
3. alpha loops,
4. graphics,
5. local camera capture,
6. soundboard audio,
7. music bed,
8. recording output.

If drift exceeds one frame, the engine MUST log and correct by adjusting audio presentation or dropping/holding non-critical frames. It MUST NOT allow unbounded drift.

### 11.5.2 Remote guest sources

Remote guest sources are asynchronous.

For guest sources:

```text
guest audio and guest video MUST be internally synchronized to the guest ingest timeline within ±1 frame.
```

But:

```text
guest stream offset relative to the NBE master clock MAY be arbitrary.
```

Guest sources MUST NOT be forced to the master clock in a way that breaks guest A/V lip-sync.

### 11.5.3 Guest video frame-selection policy

Guest video MUST use:

```text
hold-latest-complete-frame
```

At each VIEW frame deadline, the compositor MUST use the most recent completely decoded guest frame available.

If no new complete frame has arrived, the compositor MUST repeat the previous guest frame.

If no guest frame has arrived for more than:

```text
500 ms
```

the guest element MUST display a placeholder.

If the guest element is the only meaningful view source and no placeholder is available, the engine MUST activate fallback slate.

Guest video SHOULD sync to guest audio presentation time, not to master clock.

## 11.6 Command timing

Commands take effect at the next safe frame boundary unless the command specifies immediate emergency behavior.

TAKE MUST begin no later than the next frame boundary after acceptance.

---

# 12. Cross-cutting concern — Deterministic loops

## 12.1 Loop function

All loops MUST be pure functions of the master clock.

For a loop with:

```text
periodFrames = P
t0Frames = t0
```

the source frame index for master frame `F` is:

```text
sourceIndex = (F - t0) mod P
```

If negative in pre-roll, use mathematical modulo producing non-negative index.

No loop restart event is permitted.

No loop boundary may be distinguishable from any other frame unless the source content itself differs.

## 12.2 Loop cache policy

Each loop asset MAY define:

```json
{
  "periodFrames": 300,
  "t0Frames": 0,
  "cachePolicy": "auto",
  "vramBudgetMib": 128
}
```

Cache policies:

| Policy | Behavior |
|---|---|
| `auto` | engine decides |
| `vram` | attempt full VRAM residency |
| `stream` | stream from disk/decoder |

## 12.3 Cache texture formats

The loop cache MUST declare a texture format.

Default formats:

| Content | Default format | Bytes per 1080p frame | Approx MiB/frame |
|---|---|---:|---:|
| opaque video loop | NV12 / equivalent 4:2:0 | 3,110,400 | 2.97 |
| alpha loop conservative | RGBA8 | 8,294,400 | 7.91 |
| alpha loop planar | NV12 + alpha | 5,184,000 | 4.94 |
| optional compressed | BC7 | 2,073,600 | 1.98 |

Alpha loops require an alpha plane. Relative to opaque NV12, planar alpha roughly doubles memory cost. RGBA8 is the conservative default.

BC7 is optional and MUST only be used if:

1. the asset was precompressed offline,
2. the GPU supports BC7 sampling,
3. the engine validates quality acceptable for broadcast.

Implementation note (v0.2.1): literal NV12 texture support in `wgpu` is limited. Implementations SHOULD use two-plane YUV — `R8Unorm` for Y and `Rg8Unorm` for UV — with a shader-side BT.709 conversion matrix. "NV12" in this document means that or an equivalent 4:2:0 planar representation.

## 12.4 Budgets

Default MVP budgets:

| Budget | Value |
|---|---:|
| absolute short-loop frame cap | 900 frames |
| default per-loop budget | 256 MiB |
| default total short-loop budget | 512 MiB |

The 900-frame cap is necessary but not sufficient.

A loop is VRAM-resident only if all of the following are true:

```text
periodFrames <= 900
periodFrames <= maxFramesByBudget
totalShortLoopCache <= totalBudget
device working set allows allocation
```

## 12.5 Frame budget formula

For a selected texture format:

```text
frameCostBytes = textureBytesPerFrame
frameCostMiB  = frameCostBytes / (1024 * 1024)

maxFramesByBudget = floor(effectivePerLoopBudgetMiB / frameCostMiB)
```

A loop is VRAM-resident only if:

```text
periodFrames <= maxFramesByBudget
```

Otherwise it MUST be streamed, unless `cachePolicy: vram` is mandatory, in which case preflight MUST fail.

Example caps at 1080p with 256 MiB per loop:

| Format | Max frames | Approx duration at 30 fps |
|---|---:|---:|
| NV12 opaque | 86 | 2.87 s |
| RGBA8 alpha | 32 | 1.07 s |
| NV12 + alpha | 51 | 1.70 s |
| BC7 | 129 | 4.30 s |

The 900-frame allowance is dead code for full-screen RGBA loops and MUST NOT be interpreted as sufficient.

## 12.6 Apple unified-memory rule

On Metal/Apple Silicon:

```text
effectivePerLoopBudgetMiB =
    min(
      manifest perLoopBudgetMib,
      engine default perLoopBudgetMib,
      deviceSafeBudgetMib
    )
```

`deviceSafeBudgetMib` MUST be derived from:

```text
MTLDevice.recommendedMaxWorkingSetSize
```

The engine MUST reserve working set for:

1. view/preview render targets,
2. live camera textures,
3. guest textures,
4. fallback slate,
5. encoder interop surfaces.

Loop cache MUST NOT exceed the remaining safe budget.

If `vramBudgetMib` exceeds device safe budget, the engine MUST clamp it and log a warning.

## 12.7 VRAM ring buffer

VRAM-resident loops MUST use a texture ring buffer or texture array.

Frame selection:

```text
textureSlot = sourceIndex mod P
```

There MUST be no decoder restart at loop wrap.

## 12.8 Long-loop streaming

Long loops MUST use double-buffered read-ahead.

Minimum read-ahead:

```text
max(2 * GOP length, 60 frames)
```

Wrap policy:

1. Before loop end, pre-stage decoder/seek for frame 0.
2. Maintain next-window buffer.
3. Wrap MUST NOT block the render thread.
4. If wrap read-ahead fails, the loop element MUST fall back to frozen frame or fallback slate if live.

## 12.9 Loop metadata precedence

If both `asset.loop` and `element.loop` exist:

```text
element.loop overrides asset.loop
```

If `element.loop` is absent:

```text
asset.loop is used
```

If neither exists and element kind is `videoLoop`, preflight MUST fail.

Preflight MUST validate the effective loop against the actual asset duration.

Effective loop texture-format resolution order:

```text
element.loop.textureFormat
asset.loop.textureFormat
engine default
```

## 12.10 Loop preflight

Preflight MUST verify:

1. `expectedDurationFrames == loop.periodFrames` if both present.
2. first frame decodes,
3. last frame decodes,
4. wrap index is valid,
5. no audio gap if loop has audio,
6. no VFR.

Preflight MUST report per loop:

```json
{
  "assetId": "globe_loop",
  "periodFrames": 300,
  "selectedTextureFormat": "bc7",
  "frameCostMib": 1.98,
  "effectiveBudgetMib": 256,
  "maxFramesByBudget": 129,
  "cachePolicySelected": "stream",
  "vramResident": false,
  "reason": "periodFrames exceeds maxFramesByBudget"
}
```

---

# 13. Cross-cutting concern — Automation engine

## 13.1 Rule model

An automation rule is:

```text
trigger + conditions → command
```

The command is any command-bus command. Automation actions face the same preconditions as a human operator's commands.

## 13.2 Triggers

| Trigger | Fires when |
|---|---|
| `mediaEnd` | a timed item completes |
| `mediaStart` | a timed item starts |
| `timer` | a show-clock elapsed time is reached |
| `timeOfDay` | a wall-clock time is reached |
| `audioLevel` | a bus crosses a level threshold |
| `hotkey` | a binding fires |
| `rssKeyword` | an RSS item matches a keyword rule |
| `streamHealth` | stream state changes (e.g., reconnecting) |
| `stateChange` | a specified state transition occurs |

## 13.3 Execution semantics

1. Rules evaluate against state changes, not per frame.
2. Every automation action is written to the audit log (Section 10.7).
3. Automation actions are rate-limited; a rule MUST NOT fire more than once per frame.

## 13.4 Cycle detection

Preflight MUST statically reject rules whose action can re-trigger themselves directly or transitively. The runtime MUST also suppress a rule that fires itself.

## 13.5 Automation hold

`automation.hold` is the global kill switch. When held:

1. all automation triggers are suppressed within 1 frame,
2. `autoFollow` is suppressed,
3. telemetry reports `automationHold: true`.

---

# 14. Cross-cutting concern — Plugin system

## 14.1 Plugin kinds

Two sandboxed plugin kinds:

1. **Effect plugins**: WGSL fragment shaders with declared uniform parameters.
2. **Element plugins**: WASM modules that produce frames or data.

## 14.2 Sandboxing

- Effect plugins are strictly fragment shaders operating on bound textures, validated via `naga`. They MUST NOT execute arbitrary compute shaders that bypass the render graph.
- Element plugins run in a Wasmtime/Wasmer-class runtime with strict WASI capabilities: no network, no disk writes outside designated temp mounts, no ambient authority.

## 14.3 Manifest declaration and permissions

Both kinds are manifest-declared (`plugins` array), version-pinned, and permission-listed:

```json
{
  "id": "lowerthird_anim",
  "kind": "element",
  "source": "plugins/lowerthird_anim.wasm",
  "version": "0.1.0",
  "permissions": []
}
```

Permissions are deny-by-default. An empty list means no network, no disk, no camera, no microphone.

## 14.4 Preflight validation

Preflight MUST verify, for every declared plugin:

1. the package exists and its hash matches,
2. WGSL shaders compile via `naga`,
3. WASM modules load and declare only their manifest permissions,
4. declared permissions are enforceable by the runtime.

## 14.5 Frame format (v1)

Element plugins output RGBA8 into an engine-provided texture. NV12 plane output is a later optimization, not v1 (Section 25).

## 14.6 Plugin API

The plugin API is versioned. Plugins declare the API version they were built against; the engine refuses to load plugins built against an incompatible API version.

---

# 15. Manifest JSON Schema v0.3

The normative manifest schema lives at `schemas/manifest.v0.3.json` in the repository. That file is the byte-exact normative artifact; nothing embedded in this document overrides it.

What is new in v0.3:

1. `manifestVersion` const `"0.3"`.
2. Top-level `scenes` (required), `overlays`, `transitions`, `automation`, `plugins`, `qualityProfile`.
3. `rundown` is now a recursive `Sequence` of `Item`s; Segment/Subsegment remain as conventional levels.
4. `Layer` is replaced by `Element`, retaining every v0.2 property and adding `sceneRef`, `pluginId`, `children`, `enterAnimation`, `exitAnimation`, and the new element kinds `sceneRef`, `group`, `plugin` with their conditional requirements.
5. `Item` kinds: `sceneRef`, `sequenceRef`, `clipRef`, `liveRef`, `slate`.
6. New definitions: `Scene`, `Overlay`, `Element`, `Animation`, `TransitionPreset`, `AutomationRule`, `Sequence`, `Item`, `Plugin`.
7. `Asset.kind` gains `wasm` and `wgsl`.

Migration from v0.2 is mechanical via `nbe-migrate` (Section 6.7). A v0.2 package is rejected by a v0.3 preflight until migrated; this is verified (AC-28).

---

# 16. Command API

All commands use the WebSocket envelope defined in Section 5.4.

`program.*` commands remain as deprecated aliases mapping 1:1 to `view.*` for one spec version, emitting a deprecation warning in telemetry (Assumption 17). `layer.*` commands map to `element.*` likewise.

Error code registry (normative):

| Code | Meaning |
|---|---|
| `E_BAD_PAYLOAD` | Payload failed schema validation. |
| `E_FORBIDDEN_STATE` | Current state does not permit the command. |
| `E_NOT_FOUND` | Referenced entity does not exist. |
| `E_ASSET_MISSING` | Referenced asset is missing. |
| `E_DECODE` | Decode failure. |
| `E_ENGINE` | Render engine failure. |
| `E_VERSION_CONFLICT` | Stale `baseStateVersion`. |
| `E_UNSUPPORTED` | Feature unsupported in current runtime mode. |
| `E_UNSUPPORTED_FEATURE` | Optional feature is disabled, e.g. NDI. |
| `E_AUTH` | Authentication or role failure. |
| `E_NO_HARDWARE_ENCODER` | No compliant hardware encoder available. |
| `E_NETWORK` | Generic network failure. |
| `E_PREFLIGHT_FAILED` | Preflight did not pass. |
| `E_AUDIO` | Audio graph or device failure. |
| `E_DISK` | Recording or media disk failure. |
| `E_TIMEOUT` | Operation timed out (reserved for async network boundaries: TURN vending, WHIP handshake, RSS fetches). |
| `E_TURN` | TURN credential vending failure. |
| `E_ICE` | WebRTC ICE failure. |

## 16.1 Show commands

| Command | Payload schema | Preconditions | State transitions | Failure modes |
|---|---|---|---|---|
| `show.load` | `{ packagePath: string, mode?: "load"\|"reload" }` | no live view | show `UNLOADED -> LOADED` | `E_BAD_PAYLOAD`, `E_NOT_FOUND`, `E_ENGINE` |
| `show.preflight` | `{ strict?: boolean }` | show loaded | sets preflight state | `E_PREFLIGHT_FAILED` |
| `show.start` | `{ startClock?: boolean }` | preflight passed | show `LOADED -> RUNNING`, clock `STOPPED -> RUNNING` | `E_FORBIDDEN_STATE` |
| `show.stop` | see below | show running unless force | show `RUNNING -> STOPPED`; outputs quiesced | `E_FORBIDDEN_STATE`, `E_DISK`, `E_NETWORK` |
| `show.unload` | `{}` | not live | show `LOADED/RUNNING -> UNLOADED` | `E_FORBIDDEN_STATE` |

`show.stop` payload and behavior:

```json
{
  "quiesceOutputs": true,
  "force": false
}
```

When `show.stop` is received:

1. If recording is active, the engine MUST issue an internal `record.stop`.
2. If streaming is active, the engine MUST issue an internal `stream.stop`.
3. The engine MUST wait up to 2 seconds for graceful output shutdown.
4. The show clock then transitions to `STOPPED`.
5. If graceful shutdown exceeds 2 seconds, the engine MUST force-stop outputs and log a warning.

| `quiesceOutputs` | `force` | Active outputs | Result |
|---:|---:|---|---|
| true | false | yes | graceful automatic stop |
| true | true | yes | immediate stop, warning logged |
| false | false | yes | fail with `E_FORBIDDEN_STATE` |
| false | true | yes | immediate stop |
| any | any | no | stop show |

Recording remains crash-safe because fragmented MP4 or MKV fragments are already written.

## 16.2 View/Preview commands

| Command | Payload schema | Preconditions | State transitions | Failure modes |
|---|---|---|---|---|
| `preview.set` | `{ itemRef: string }` | item exists and READY/ARMED | target `READY -> ARMED`; previous preview may return to READY | `E_NOT_FOUND`, `E_ASSET_MISSING` |
| `view.take` | see below | preview armed | preview item becomes LIVE or PLAYING; previous live becomes READY; audio transition executes | `E_FORBIDDEN_STATE`, `E_AUDIO`, `E_ENGINE` |
| `view.cut` | `{ itemRef: string }` | item exists | immediate view switch to item | `E_NOT_FOUND`, `E_FORBIDDEN_STATE` |
| `view.fallback` | `{ reason?: string }` | always allowed | VIEW switches to fallback slate | `E_ENGINE` |

`view.take` payload schema:

```json
{
  "transition": { "enum": ["cut", "mix", "wipe", "sting", "move", "dve"] },
  "preset": { "type": "string" },
  "durationFrames": { "type": "integer", "minimum": 0, "maximum": 600 },
  "audio": {
    "type": "object",
    "additionalProperties": false,
    "properties": {
      "transition": { "enum": ["follow", "crossfade", "cut", "mute"] },
      "durationFrames": { "type": "integer", "minimum": 1, "maximum": 600 },
      "rampMs": { "type": "number", "minimum": 5, "maximum": 50 }
    }
  }
}
```

Defaults:

```json
{
  "transition": "cut",
  "audio": { "transition": "follow", "rampMs": 10 }
}
```

If `transition == "mix"` and `audio.durationFrames` is absent, audio crossfade duration MUST equal video `durationFrames`.

If `preset` is present, the named transition preset supplies kind, duration, easing, and per-element overrides; explicit fields in the payload override the preset.

## 16.3 Scene commands

| Command | Payload schema | Preconditions | State transitions | Failure modes |
|---|---|---|---|---|
| `scene.arm` | `{ sceneId: string }` | scene exists | scene ARMED in preview | `E_NOT_FOUND`, `E_ASSET_MISSING` |
| `scene.apply` | `{ sceneId: string, target: "view"\|"preview" }` | scene exists | scene applied to target bus | `E_NOT_FOUND`, `E_FORBIDDEN_STATE` |

## 16.4 Sequence/item commands

| Command | Payload schema | Preconditions | State transitions | Failure modes |
|---|---|---|---|---|
| `sequence.arm` | `{ sequenceId: string }` | sequence exists | sequence `READY -> ARMED` | `E_NOT_FOUND` |
| `sequence.unarm` | `{ sequenceId: string }` | sequence armed | sequence `ARMED -> READY` | `E_NOT_FOUND`, `E_FORBIDDEN_STATE` |
| `item.arm` | `{ itemId: string }` | item exists | item `READY -> ARMED` | `E_NOT_FOUND`, `E_ASSET_MISSING` |
| `item.unarm` | `{ itemId: string }` | armed | item `ARMED -> READY` | `E_NOT_FOUND`, `E_FORBIDDEN_STATE` |
| `item.stop` | `{ itemId: string }` | playing | item `PLAYING -> READY` | `E_NOT_FOUND`, `E_FORBIDDEN_STATE` |

## 16.5 Element/graphic commands

| Command | Payload schema | Preconditions | State transitions | Failure modes |
|---|---|---|---|---|
| `element.toggle` | `{ elementId: string, scope?: string, visible?: boolean }` | element exists | element visibility toggled | `E_NOT_FOUND` |
| `element.set` | `{ elementId: string, patch: { visible?, opacity?, transform?, chromaKey? } }` | element exists | element properties updated | `E_BAD_PAYLOAD`, `E_NOT_FOUND` |
| `graphic.show` | `{ templateId: string, fields: object, elementId?: string, z?: integer }` | template exists | graphic element becomes visible | `E_NOT_FOUND`, `E_BAD_PAYLOAD` |
| `graphic.hide` | `{ elementId?: string, templateId?: string }` | graphic visible/known | graphic hidden | `E_NOT_FOUND` |
| `graphic.update` | `{ elementId: string, fields: object }` | graphic exists | graphic fields updated | `E_NOT_FOUND`, `E_BAD_PAYLOAD` |
| `breaking.show` | `{ headline: string, subhead?: string }` | breaking template exists | breaking banner visible | `E_NOT_FOUND` |
| `breaking.hide` | `{}` | breaking visible or hidden | breaking banner hidden | none |

## 16.6 Overlay commands

| Command | Payload schema | Preconditions | State transitions | Failure modes |
|---|---|---|---|---|
| `overlay.show` | `{ overlayId: string, animation?: string }` | overlay exists | overlay visible with its enter animation | `E_NOT_FOUND` |
| `overlay.hide` | `{ overlayId: string }` | overlay visible | overlay hidden with its exit animation | `E_NOT_FOUND` |

## 16.7 Ticker commands

| Command | Payload schema | Preconditions | State transitions | Failure modes |
|---|---|---|---|---|
| `ticker.setSource` | `{ source: "manual"\|"rss"\|"mixed" }` | ticker element exists | ticker source changed | `E_NOT_FOUND` |
| `ticker.override` | see below | ticker exists | ticker queue updated | `E_BAD_PAYLOAD`, `E_NOT_FOUND` |
| `ticker.clearOverride` | `{}` | ticker exists | manual override cleared | `E_NOT_FOUND` |
| `ticker.refreshRss` | `{ feedId?: string }` | RSS configured | RSS cache refreshed | `E_NETWORK`, `E_BAD_PAYLOAD` |

`ticker.override` payload schema:

```json
{
  "items": {
    "type": "array",
    "items": {
      "type": "object",
      "additionalProperties": false,
      "required": ["text"],
      "properties": {
        "text": { "type": "string", "minLength": 1 },
        "language": { "type": "string" },
        "priority": { "type": "integer", "minimum": 0, "maximum": 100000, "default": 0 },
        "ttlSec": { "type": "integer", "minimum": 1 }
      }
    }
  },
  "mode": { "enum": ["replace", "prepend", "append"], "default": "replace" }
}
```

Ticker ordering rules:

1. Breaking override items appear first.
2. Higher `priority` appears before lower `priority`.
3. For equal priority, insertion order is preserved.
4. `language` is metadata only in v1.

## 16.8 Soundboard/audio commands

| Command | Payload schema | Preconditions | State transitions | Failure modes |
|---|---|---|---|---|
| `soundboard.play` | `{ assetId: string, gainDb?: number }` | asset preloaded | playback started | `E_NOT_FOUND`, `E_AUDIO` |
| `soundboard.stop` | `{ playbackId?: string, assetId?: string }` | playback active or known | playback stopped | `E_NOT_FOUND` |
| `soundboard.stopAll` | `{}` | always | all SFX stopped | none |
| `audio.bus.set` | see below | bus exists | bus params changed | `E_BAD_PAYLOAD`, `E_AUDIO`, `E_NOT_FOUND` |
| `audio.duck` | `{ bus: "music", enabled: boolean, depthDb?: number, attackMs?: number, releaseMs?: number }` | duck-capable bus | duck state changed | `E_BAD_PAYLOAD` |
| `guest.mute` | `{ guestId: string, muted: boolean }` | guest exists | guest audio muted/unmuted | `E_NOT_FOUND` |

`audio.bus.set` payload schema:

```json
{
  "bus": { "enum": ["mic", "clip", "music", "sfx", "guest", "master", "guestReturn", "ifb"] },
  "guestId": { "type": "string" },
  "gainDb": { "type": "number", "minimum": -60, "maximum": 12 },
  "muted": { "type": "boolean" }
}
```

If `bus == "guestReturn"`, `guestId` is REQUIRED. If `bus != "guestReturn"`, `guestId` MUST be ignored.

## 16.9 Guest commands

| Command | Payload schema | Preconditions | State transitions | Failure modes |
|---|---|---|---|---|
| `guest.connect` | `{ guestId: string, whipUrl: string, displayName?: string }` | guest not connected | guest source `READY` | `E_NETWORK`, `E_BAD_PAYLOAD` |
| `guest.disconnect` | `{ guestId: string }` | guest exists | guest source disconnected | `E_NOT_FOUND` |
| `guest.setLayout` | `{ guestId: string, layout: "pip"\|"full" }` | guest element exists | guest transform updated | `E_NOT_FOUND` |
| `guest.placeholder` | `{ guestId: string, assetId?: string }` | guest exists | placeholder set | `E_NOT_FOUND` |
| `guest.configureReturn` | see below | guest exists | guest return bus updated | `E_NOT_FOUND`, `E_AUDIO`, `E_BAD_PAYLOAD` |
| `guest.getTurn` | see below | control plane TURN vending enabled | none; returns credentials | `E_TURN`, `E_AUTH`, `E_NOT_FOUND` |

`guest.configureReturn` payload schema:

```json
{
  "guestId": { "type": "string" },
  "mode": { "enum": ["programMinusSelf", "producerMix", "mute"] },
  "includeOtherGuests": { "type": "boolean", "default": true },
  "gainDb": { "type": "number", "minimum": -60, "maximum": 12 },
  "muted": { "type": "boolean" }
}
```

Required: `["guestId"]`. Default mode: `"programMinusSelf"`.

`guest.getTurn` payload schema:

```json
{
  "guestId": { "type": "string" },
  "ttlSec": { "type": "integer", "minimum": 30, "maximum": 86400, "default": 600 }
}
```

Response data schema:

```json
{
  "uris": { "type": "array", "items": { "type": "string" } },
  "username": { "type": "string" },
  "credential": { "type": "string" },
  "ttlSec": { "type": "integer" }
}
```

## 16.10 Automation commands

| Command | Payload schema | Preconditions | State transitions | Failure modes |
|---|---|---|---|---|
| `automation.enable` | `{ ruleId: string }` | rule exists | rule enabled | `E_NOT_FOUND` |
| `automation.disable` | `{ ruleId: string }` | rule exists | rule disabled | `E_NOT_FOUND` |
| `automation.hold` | `{ hold: boolean }` | always | global hold state set within 1 frame | none |

## 16.11 Snapshot and marker commands

| Command | Payload schema | Preconditions | State transitions | Failure modes |
|---|---|---|---|---|
| `snapshot.save` | `{ name: string }` | always | snapshot saved | `E_DISK` |
| `snapshot.recall` | `{ name: string }` | snapshot exists | view state restored | `E_NOT_FOUND` |
| `marker.add` | `{ name: string, timecode?: string }` | show running | marker added; recording chapter written if container supports it | `E_DISK` |

## 16.12 Plugin commands

| Command | Payload schema | Preconditions | State transitions | Failure modes |
|---|---|---|---|---|
| `plugin.reload` | `{ pluginId: string }` | plugin exists | plugin reloaded | `E_ENGINE`, `E_AUTH` |

## 16.13 Clock commands

| Command | Payload schema | Preconditions | State transitions | Failure modes |
|---|---|---|---|---|
| `clock.configure` | see below | clock element exists | clock config updated | `E_NOT_FOUND`, `E_BAD_PAYLOAD` |

`clock.configure` payload schema:

```json
{
  "elementId": { "type": "string" },
  "clock": {
    "type": "object",
    "additionalProperties": false,
    "properties": {
      "mode": { "enum": ["wall", "showElapsed"] },
      "timezone": { "type": "string" },
      "format": { "enum": ["HH:mm", "HH:mm:ss", "hh:mm A", "locale"] },
      "locale": { "type": "string" },
      "blinkColon": { "type": "boolean" }
    }
  }
}
```

Required: `["elementId"]`.

## 16.14 Output commands

| Command | Payload schema | Preconditions | State transitions | Failure modes |
|---|---|---|---|---|
| `record.start` | `{ outputId?: string }` | show running, encoder available | recording active | `E_NO_HARDWARE_ENCODER`, `E_DISK` |
| `record.stop` | `{}` | recording active | recording stopped | `E_FORBIDDEN_STATE` |
| `stream.start` | `{ outputId?: string, url?: string }` | show running, encoder available | stream active | `E_NO_HARDWARE_ENCODER`, `E_NETWORK` |
| `stream.stop` | `{}` | stream active | stream stopped | `E_FORBIDDEN_STATE` |

## 16.15 System commands

| Command | Payload schema | Preconditions | State transitions | Failure modes |
|---|---|---|---|---|
| `system.status` | `{}` | always | none | none |
| `system.telemetry.subscribe` | `{ intervalMs?: integer }` | always | telemetry subscription active | `E_BAD_PAYLOAD` |
| `system.telemetry.unsubscribe` | `{}` | subscribed | telemetry subscription removed | none |

---

# 17. State machine

## 17.1 Item states

An item is a Sequence, Item, or playable source item.

States:

| State | Meaning | UI indication |
|---|---|---|
| `READY` | Valid, not armed, not live. | gray |
| `ARMED` | In preview/next, preloaded. | yellow |
| `LIVE` | Live non-timed source on VIEW. | red |
| `PLAYING` | Timed media active on VIEW. | green with red live border |
| `DONE` | Timed item completed. Optional state. | dim green |
| `MISSING` | Required asset/source missing. | flashing red outline |
| `ERROR` | Runtime failure. | red banner |

## 17.2 Scene states

| State | Meaning |
|---|---|
| `IDLE` | Declared, not in use. |
| `ARMED` | Instantiated on the preview bus. |
| `VIEW` | Instantiated on the view bus. |
| `TRANSITIONING` | In an active transition between buses. |

(These were named to avoid collision with the Preview bus; see the v0.3 red-team notes in git history.)

## 17.3 Transition table

| Current | Event/command | Guard | Next | Side effects |
|---|---|---|---|---|
| `READY` | `arm` | asset valid | `ARMED` | preload, set preview |
| `READY` | asset missing detected | missing | `MISSING` | alert UI |
| `READY` | decode error | failure | `ERROR` | alert UI |
| `ARMED` | `unarm` | not live | `READY` | release preview |
| `ARMED` | `take` | live source | `LIVE` | view switch |
| `ARMED` | `take` | timed media | `PLAYING` | view switch, start media clock |
| `ARMED` | asset missing detected | missing | `MISSING` | alert UI, fallback if preview required |
| `ARMED` | decode error | failure | `ERROR` | fallback if armed critical |
| `LIVE` | `take` away | another item goes live | `READY` | remove from view |
| `LIVE` | device loss | camera/guest lost | `ERROR` | fallback if view |
| `PLAYING` | end reached | duration complete | `DONE` | mark complete |
| `PLAYING` | `stop` | operator stop | `READY` | stop media |
| `PLAYING` | `take` away | another item goes live | `READY` | remove from view |
| `PLAYING` | decode error | failure | `ERROR` | fallback if view |
| `DONE` | `reset`/`arm` | asset valid | `READY` or `ARMED` | reset counters |
| `MISSING` | asset restored | preflight pass | `READY` | clear alert |
| `MISSING` | unrecoverable | manual reset | `ERROR` | alert |
| `ERROR` | `reset` | recoverable | `READY` | clear fault |
| `ERROR` | unrecoverable | none | remains `ERROR` | require reload |

## 17.4 Automation hold interaction

If `automation.hold` is active, all automation triggers and `autoFollow` are suppressed within 1 frame (Section 13.5).

## 17.5 Text diagram

```text
                    asset restored
              +-----------------------------+
              |                             |
              v                             |
          +--------+     arm      +--------+
   +----->| READY  |------------->| ARMED  |
   |      +--------+              +--------+
   |         ^                      |    |
   |         | unarm/reset          |    | take(live source)
   |         +----------------------+    |
   |         |                           v
   |         |                        +------+
   |         |                        | LIVE |
   |         |                        +------+
   |         |                           |
   |         | take away/stop/reset      |
   +-------------------------------------+
   |
   |         take(timed media)
   |      +--------------------------+
   |      |                          v
   |   +---------+  end reached   +------+
   |   | PLAYING |--------------->| DONE |
   |   +---------+                +------+
   |      |                          |
   |      | stop/take away/reset     | reset/arm
   +---------------------------------+
   |
   |      asset missing / decode failure / device loss
   +--------------------------------------------------> MISSING / ERROR
```

---

# 18. Cadence rules

## 18.1 House rate

Default house rate:

```text
30 fps
```

All final show media MUST be normalized to house rate before live load.

## 18.2 Cadence preservation

If asset manifest says:

```json
"cadence": "preserve"
```

the pipeline MUST use frame holds only.

Motion interpolation is forbidden unless:

```json
"cadence": "interpolate"
```

and the pipeline explicitly supports it.

v1 MAY reject `interpolate` assets as unsupported.

## 18.3 Hold patterns

For 30 fps house rate:

| Source fps | Ratio | Hold pattern | Notes |
|---:|---:|---|---|
| 15 | 2.0 | `2` | every source frame held 2 output frames |
| 10 | 3.0 | `3` | every source frame held 3 output frames |
| 12 | 2.5 | `2,3,2,3` | alternating pulldown |
| 24 | 1.25 | duplicate every fourth source frame | 4 source frames → 5 output frames |

Example 15 → 30:

```text
source:  S0 S1 S2 S3
output:  S0 S0 S1 S1 S2 S2 S3 S3
```

Example 12 → 30:

```text
source:  S0 S1 S2 S3
output:  S0 S0 S1 S1 S1 S2 S2 S3 S3 S3
```

Example 24 → 30:

```text
source:  S0 S1 S2 S3 S4
output:  S0 S1 S2 S3 S3 S4
```

## 18.4 25 fps and 29.97 fps

25 fps and 29.97 fps assets are forbidden by default. They are allowed only if the asset contains explicit pulldown metadata.

25→30 uses `repeatNthSourceFrame` with `n = 5` (duplicate every fifth source frame):

```text
source:  S0 S1 S2 S3 S4
output:  S0 S1 S2 S3 S4 S4
```

29.97→30 uses `repeatOnePerNSourceFrames` with `n = 1000` (one held frame per ~1000 source frames, about one per 33 seconds):

```text
source frames 0..999 produce output frames 0..1000
```

The exact frame to repeat MUST be documented in the asset pipeline and verified by preflight.

## 18.5 Unsupported source rates

Any custom cadence MAY use explicit pattern mode:

```json
{
  "pulldown": {
    "mode": "pattern",
    "pattern": [2, 3, 2, 3]
  }
}
```

If an unsupported source frame rate has no valid pulldown metadata, preflight MUST fail.

The `Pulldown` schema definition (one of `pattern`, `repeatNthSourceFrame`, `repeatOnePerNSourceFrames`) is normative in `schemas/manifest.v0.3.json`. The v0.1 `pulldownPattern` field is deprecated; `pulldown` wins if both are present.

## 18.6 Cadence preflight

Preflight MUST verify:

1. output frame rate equals house rate,
2. output duration matches expected duration,
3. duplicate-frame pattern matches declared cadence where feasible,
4. no interpolated intermediate frames are present for `preserve` assets,
5. VFR is absent.

---

# 19. Preflight details

## 19.1 Exit status

`preflight` MUST exit with:

| Code | Meaning |
|---:|---|
| 0 | air-ready |
| 1 | warnings only, not air-ready unless `--allow-warnings` |
| 2 | errors, not air-ready |

CI MUST block load on exit code != 0 unless explicitly overridden.

## 19.2 Report schema

`preflight_report.json` MUST include:

```json
{
  "manifestValid": true,
  "airReady": true,
  "errors": [],
  "warnings": [],
  "assets": [
    {
      "id": "A1",
      "kind": "video",
      "exists": true,
      "sha256Ok": true,
      "decodeFirstFrameOk": true,
      "decodeLastFrameOk": true,
      "cfr": true,
      "frameRate": 30,
      "width": 1920,
      "height": 1080,
      "durationFrames": 900,
      "cadenceOk": true,
      "loudness": {
        "integratedLufs": -16.1,
        "truePeakDbtp": -1.7
      }
    }
  ],
  "loops": [
    {
      "assetId": "globe_loop",
      "periodFrames": 300,
      "seamless": true,
      "cachePolicySelected": "vram"
    }
  ],
  "scenes": [
    {
      "sceneId": "SCN_A1",
      "referencesOk": true,
      "dagOk": true
    }
  ],
  "plugins": [
    {
      "pluginId": "lowerthird_anim",
      "sandboxOk": true
    }
  ],
  "contactSheet": "contact_sheet.jpg"
}
```

The `loops` entries MUST follow the extended report shape defined in Section 12.10.

## 19.3 Seeded failure tests

The preflight test suite MUST include:

1. Missing asset reference.
2. VFR clip.
3. 25 fps clip in 30 fps show without pattern.
4. Alpha video with no alpha channel.
5. Loop period mismatch.
6. Broken SHA-256.
7. Missing fallback asset.
8. Out-of-range loudness.
9. Invalid hotkey action.
10. Missing template field.
11. 29.97 fps asset without pulldown metadata.
12. VRAM-residency request that exceeds `maxFramesByBudget`.
13. Circular sub-scene reference.
14. Self-triggering automation rule.
15. Plugin failing sandbox validation.
16. Unresolvable `sceneRef`, `pluginId`, or group `children` entry.

---

# 20. MVP scope hard ceiling

The MVP MUST NOT exceed the following live complexity:

| Item | Maximum |
|---|---:|
| live camera sources | 1 |
| preloaded clips per sequence | 3 |
| background loops | 1 |
| alpha logo loops | 1 |
| simultaneous WHIP guests | 1 |
| lower-third templates | headline + name |
| breaking banner | 1 |
| ticker | manual + RSS |
| clock | 1 |
| transitions | cut + crossfade |
| overlays | 2 (ticker + logo bug) |
| outputs | local view, preview, recording, one RTMP/SRT |
| resolution | 1920x1080 |
| frame rate | 30 fps |

Advanced automation, plugins, sub-scenes, and move-class transitions are post-MVP but schema-supported. The MVP MAY support future schema fields, but acceptance is based only on the above.

---

# 21. Hardware tiers

These tiers are normative for 1080p30 house rate unless otherwise stated.

## Floor device — reference minimum

The 2019 dual-GPU Intel/Radeon MacBook Pro (i7, 16 GB) is the named reference floor device.

1. The OBS baseline comparison (AC-11) MUST also run on the floor device.
2. Quality profiles MUST keep the floor device viable via the degradation ladder (Section 10.5).
3. The floor device is a benchmark and CI target, not the recommended show machine.

## Tier 0 — prototype

Purpose: workflow testing only.

| Component | Minimum |
|---|---|
| CPU | 4-core modern CPU |
| RAM | 16 GB |
| GPU | modern iGPU |
| Storage | NVMe or SATA SSD |
| Encode | not guaranteed |
| Use | not trusted live |

## Tier 1 — trusted live 1080p30

Purpose: MVP acceptance target.

| Component | Requirement |
|---|---|
| CPU | Apple M-series Pro or Ryzen 7 / Intel i7 modern |
| GPU | Apple GPU or RTX 3060/4060-class |
| RAM | 32 GB |
| Storage | NVMe |
| Decode | hardware H.264/HEVC/ProRes where applicable |
| Encode | VideoToolbox or NVENC |
| Network | 1 GbE |
| Internet up | 10–20 Mbps minimum |

## Tier 2 — comfortable production

| Component | Requirement |
|---|---|
| CPU | Apple M-series Max or Ryzen 9 / Intel i9 |
| GPU | RTX 4070-class or Apple Max |
| RAM | 64 GB |
| Storage | separate media and recording SSDs |
| Network | 2.5 GbE |

## Tier 3 — 4K/showcase 60 fps

| Component | Requirement |
|---|---|
| CPU | Apple Ultra-class or high-end workstation |
| GPU | RTX 4080/4090-class |
| RAM | 64–128 GB |
| Storage | high-throughput NVMe |
| Network | 10 GbE |

## Cloud node

Purpose: guests, distribution, backup, benchmarking. Not primary local render path.

| Component | Requirement |
|---|---|
| GPU | NVIDIA L4/A10-class |
| vCPU | 8–16 |
| RAM | 16–32 GB |
| Encode | NVENC |
| Network | 1 Gbps |

## iPhone

Role: controller/monitor only.

| Requirement | Minimum |
|---|---|
| Device | iPhone 12 or newer |
| Wi-Fi | 5 GHz |
| Role | WebSocket controller, preview/view monitor (WHEP) |
| Forbidden | live renderer, primary compositor |

---

# 22. Acceptance criteria

Each criterion is independently testable.

## AC-1 — Manifest schema validation

Given a valid show package, `preflight` MUST validate the manifest against the normative NBE manifest schema (`schemas/manifest.v0.3.json`) and return exit code 0.

## AC-2 — Missing asset detection

Given a seeded manifest referencing a nonexistent asset, `preflight` MUST fail with a machine-readable error identifying the asset ID and path.

## AC-3 — VFR detection

Given a seeded VFR clip, `preflight` MUST fail and report `cfr: false`.

## AC-4 — Cadence preservation

Given 15, 10, 12, and 24 fps source assets normalized to 30 fps with `cadence: preserve`, preflight MUST verify the declared hold patterns and fail if motion interpolation is detected.

## AC-5 — 30-minute zero-drop live show

On a Tier-1 reference machine, a 30-minute continuous live show at 1080p30, single operator, with MVP maximum elements active, MUST produce zero dropped VIEW frames.

Measurement:

```text
droppedFramesTotal == 0
```

over the full show.

## AC-6 — Crash-safe recording

If the render process is killed with `SIGKILL` during recording, the resulting fragmented MP4 or MKV file MUST be playable by `ffprobe` and at least one reference player.

## AC-7 — Fallback slate latency

If a live item source fails, the engine MUST cut to the fallback slate no later than one frame after the missed deadline.

Measurement:

```text
fallbackVisibleFrame <= failureFrame + 1
```

## AC-8 — Master clock drift

For local deterministic sources:

```text
audio/video sync drift MUST remain within ±1 frame of the master show clock over a 30-minute show on Tier-1 hardware.
```

For remote guest sources:

```text
guest audio/video sync MUST remain within ±1 frame relative to the guest ingest timeline.
```

Guest offset relative to master clock is not a failure condition.

## AC-9 — Deterministic loop wrap

For a VRAM-resident loop, ten consecutive loop wraps MUST occur with:

1. zero dropped frames,
2. no decoder restart event,
3. no visible hitch in frame presentation,
4. frame index computed by modulo.

## AC-10 — Internet loss survivability

If WAN is disconnected during live local playout:

1. local view continues,
2. recording continues,
3. stream enters reconnect/backoff,
4. no VIEW frames are dropped due to stream failure.

## AC-11 — OBS baseline comparison

The same show package MUST be runnable through an OBS baseline adapter.

A published comparison MUST report:

1. dropped frames,
2. CPU utilization,
3. GPU utilization,
4. glass-to-glass latency,
5. take latency,
6. recording crash safety.

NBE MUST be no worse than OBS baseline for dropped frames, CPU utilization, and GPU utilization on Tier-1 hardware.

The comparison MUST also run on the floor device (Section 21), with results published separately.

## AC-12 — Companion command path

A Bitfocus Companion button mapped to `view.take` MUST cause a successful take via the WebSocket command bus with no custom Stream Deck plugin.

## AC-13 — Soundboard latency

A soundboard trigger MUST produce audible output within 20 ms on Tier-1 hardware and MUST NOT cause dropped VIEW frames.

## AC-14 — Loudness compliance

All preflighted audio assets MUST be within:

```text
-16 LUFS ±0.5 LUFS
true peak <= -1.5 dBTP +0.2 dB allowance
```

or preflight MUST fail.

## AC-15 — Ticker RTL/Unicode

The ticker MUST correctly render at least:

1. English LTR,
2. Arabic RTL,
3. Spanish accented text,
4. emoji or symbol if packaged font supports it.

Scrolling MUST remain at target frame rate.

## AC-16 — Single-operator usability

A single operator MUST be able to:

1. load a package,
2. run preflight,
3. arm first sequence,
4. start show,
5. take between items,
6. trigger lower third,
7. trigger breaking banner,
8. play soundboard effect,
9. start/stop recording,
10. start/stop stream,

without using a keyboard-driven debug console.

## AC-17 — Normative take latency

On localhost, an accepted `view.take` command MUST change the local VIEW output within:

```text
2 frames
```

For `mix`, the first mixed frame MUST appear by the next frame after acceptance.

## AC-18 — Mix-minus isolation

With a guest source replaced by a -20 dBFS 1 kHz test tone and no other program sources active, the corresponding `guestReturn` bus MUST measure the tone at or below:

```text
-80 dBFS
```

This verifies that the guest does not receive their own audio.

## AC-19 — Audio transition click-free behavior

During any `view.take`, `item.stop`, `soundboard.stop`, or bus mute/unmute:

1. gain changes MUST have ramps ≥ 5 ms,
2. no hard sample-step cut is allowed,
3. recorded master output MUST contain no click impulse exceeding -60 dBFS in a silent test pass.

## AC-20 — WHEP preview

A WHEP client MUST be able to fetch both:

```text
/nbe/v0.3/whep/program
/nbe/v0.3/whep/preview
```

using bearer auth.

Preview startup on a local network SHOULD occur within:

```text
2 seconds
```

MJPEG endpoints MUST remain disabled unless dev mode is enabled.

## AC-21 — Loop budget math

Given a 1080p opaque NV12 loop with:

```json
{
  "periodFrames": 90,
  "cachePolicy": "vram",
  "vramBudgetMib": 256
}
```

preflight MUST reject VRAM residency or force streaming because 90 frames exceeds the 86-frame NV12 budget.

Given a 1080p BC7 loop with:

```json
{
  "periodFrames": 120,
  "cachePolicy": "vram",
  "vramBudgetMib": 256
}
```

preflight MAY accept VRAM residency if BC7 is supported and total cache budget allows it.

## AC-22 — 25/29.97 explicit pulldown

Given a 25 fps asset with no pulldown metadata, preflight MUST fail.

Given a 29.97 fps asset with no pulldown metadata, preflight MUST fail.

Given valid explicit pulldown metadata, preflight MUST verify the resulting 30 fps CFR output and pass only if the declared pattern is present.

## AC-23 — Move (state-diff transition)

Given two scenes sharing an element ID at different transforms, a `move` transition MUST:

1. match elements by persistent identity,
2. complete the tween frame-exact within `durationFrames`,
3. drop zero frames during the move,
4. honor the declared easing within one frame of phase error.

## AC-24 — DSK persistence

A ticker on the overlay level MUST survive a complex scene move transition untouched and without recomposition artifacts.

## AC-25 — Automation

1. A rule MUST fire within 1 frame of its trigger condition becoming true.
2. `automation.hold` MUST cancel pending actions within 1 frame.
3. Cycle detection MUST reject self-triggering rules at preflight.
4. Every automation action MUST appear in the audit log.

## AC-26 — Plugin sandbox

A permissionless WASM element plugin MUST provably be unable to touch network or disk, verified via WASI capability enforcement.

## AC-27 — Degradation order

Under simulated GPU overload:

1. preview frame rate degrades first,
2. loop caches evict to streaming,
3. effect quality steps down,
4. multiview tiles drop,
5. VIEW drops zero frames throughout.

## AC-28 — Migration

A v0.2 show package run through `nbe-migrate` MUST produce a valid v0.3 package that passes preflight with identical visual and audio output. A v0.2 manifest presented directly to a v0.3 preflight MUST be rejected.

## AC-29 — Sub-scenes

1. Recursion depth cap (4) MUST be enforced.
2. Circular sub-scene references MUST be rejected at preflight.

---

# 23. Non-goals for v1

The following are explicitly out of scope for v1:

1. 24/7 Channel scheduler implementation.
2. AI background replacement.
3. Unreal/Unity virtual set.
4. Screen/app mirroring ingest.
5. CPU x264 live encoding.
6. Custom Stream Deck plugin.
7. Timeline dates or calendar scheduling.
8. Multi-machine synchronized channel playout.
9. HDR output.
10. Full motion-interpolated frame-rate conversion.
11. Multi-operator conflict resolution beyond basic state versioning.
12. Browser-based live renderer.
13. iPhone as render node.
14. Virtual-camera patterns (forbidden by the operator topology, Section 5.8).
15. Managed-TURN-only dependency (self-hosted first, Section 9.6.2).
16. Compute-shader plugins (fragment shaders only, Section 14.2).

The manifest schema MUST NOT preclude future Channel scheduling, but v1 MUST NOT implement it.

---

# 24. Risks and mitigations

| Risk | Severity | Mitigation |
|---|---:|---|
| Thermal throttling on laptop render node | High | Use desktop/mini workstation for live; enforce Tier-1 GPU budget; monitor thermal state; reduce loop cache; prefer hardware decode/encode; degradation ladder. |
| VideoToolbox decode-session limits | High | Limit simultaneous active decode sources; preload short loops into textures; reuse decode sessions; fail early in preflight if decode budget exceeded. |
| VRAM pressure from loop caches | High | Enforce per-loop and total cache budgets; evict non-live loops; stream long loops; fallback to still frame if texture pressure critical. |
| WebRTC jitter causing guest freeze | Medium | Jitter buffer 200–500 ms; placeholder on loss; automatic fallback if guest is live; separate guest from local playout clock. |
| Single-operator cognitive load | High | Big preview/view UI; color states; armed next item; one-button TAKE; automatic fallback; minimal menus during live. |
| `wgpu` driver/platform differences | Medium | Conformance suite; Metal-first path; Linux Vulkan secondary path; OBS baseline benchmark; feature flags for backend-specific paths. |
| Audio/video drift | Medium | Master clock authority; audio device clock monitoring; drift correction; acceptance test over 30 minutes. |
| RSS feed malicious or malformed | Medium | Sanitize text; disable markup; cache last known items; manual override; feed timeout; rate limiting. |
| Recording corruption on crash | High | Fragmented MP4 or MKV; 1-second fragments; kill-test in CI. |
| Disk I/O stalls during long loops | Medium | NVMe requirement; double-buffer read-ahead; preflight disk read benchmark; separate media disk on Tier-2+. |
| Companion misconfiguration | Medium | Generate Companion bindings from manifest; preflight validates action names and payload schemas. |
| OBS baseline comparison unfair | Low/Medium | Define fixed test package, hardware, metrics, and capture method in test harness. |
| State-diff transition complexity | High | Precompute diffs at arm time; cap sub-scene recursion; enforce DAG; frame-quantized tweens. |
| WASM/WGSL plugin exploits | High | Strict WASI capabilities; `naga` validation; deny-by-default permissions; AC-26. |
| Floor device thermal throttling | High | Degradation ladder; preview fps drop first; loop eviction; View never degraded. |
| Automation infinite loops | Medium | Static cycle detection at preflight; runtime self-trigger suppression; global `automation.hold`. |
| Abuse via guest links/RSS/call-ins | Medium | Signed expiring revocable JWT links; rate limiting; producer-gated admission; audit log. |

---

# 25. Resolved open questions

## 25.1 Carried from v0.2 (normative)

| # | Question | Ruling |
|---:|---|---|
| 1 | 25/29.97 fps policy | Allowed only with explicit pulldown metadata. 25→30 duplicates every fifth frame. 29.97→30 adds one hold per ~1000 source frames. |
| 2 | Smelter API compatibility | Benchmark concepts only. No API compatibility requirement. |
| 3 | WHIP auth | Bearer token in headers. TURN credentials vended by control plane. |
| 4 | NDI dependency | Optional, feature-flagged. Core build remains NDI-free. |
| 5 | fMP4 vs MKV default | Fragmented MP4 confirmed as default. |
| 6 | VRAM on unified memory | `vramBudgetMib` is soft and capped by Metal `recommendedMaxWorkingSetSize`. |
| 7 | Channel schema fields | Existing minimal fields are sufficient. No additional reservation. |
| 8 | Ticker languages | Single mixed feed with priority flags. Language is metadata only in v1. |
| 9 | ISO recording | Master-only in v1. `isolation` schema hook reserved. |
| 10 | iPhone preview transport | WHEP is primary. MJPEG is dev-mode fallback only. |

## 25.2 Resolved in v0.3 (normative)

| # | Question | Ruling |
|---:|---|---|
| 1 | WASM element plugin frame format | RGBA8 into an engine-provided texture for v1. NV12 plane output is a later optimization. |
| 2 | Snapshot scope | The entire View state, including overlay visibility. A snapshot restores what the audience sees. |
| 3 | TURN providers | Self-hosted coturn via environment configuration first; managed-provider integrations later. |
| 4 | Caption burn-in (later) | A dedicated Overlay element on the DSK level — not a post-composite shader pass. |
| 5 | Sequence depth limit | 8 levels normative; preflight warns beyond 4. |

v0.2.1 errata (adopted):

1. NV12 loop-cache textures are implemented as two-plane YUV (`R8Unorm` + `Rg8Unorm`) with shader-side BT.709 conversion; literal NV12 texture formats are not assumed in `wgpu`.
2. `mixDurationFrames` minimum is 1; a 0-frame mix is a cut, and the schema enforces it.
3. `E_TIMEOUT` is reserved for async network boundaries (TURN vending, WHIP handshake, RSS fetches) and is wired in the control plane and guest ingest modules.

---

# 26. Implementation handoff notes

For coding agents, the implementation order SHOULD be:

1. Manifest schema validator and preflight skeleton.
2. Control-plane WebSocket server and state machine.
3. Render-node command bridge.
4. Basic view/preview compositor with color sources and image/video elements.
5. Video decode integration with hardware decoder.
6. Audio graph with master bus.
7. Ticker and lower-third template renderer.
8. Stream Deck/Companion command mapping.
9. Recording output with fragmented MP4.
10. RTMP/SRT output with hardware encoder.
11. Telemetry and fallback slate.
12. OBS baseline benchmark harness.

Additional sequencing requirements:

- Implement loop cache format accounting before VRAM caching.
- Implement guest return/mix-minus at the same time as WebRTC guest ingest.
- Implement audio ramps before the first live TAKE test.
- Implement WHEP preview after the WebRTC stack exists.
- Implement TURN vending before remote guest testing.
- Add 25/29.97 pulldown tests to preflight CI.
- Implement `nbe-migrate` when the v0.3 schema lands, with the v0.2 fixture as its test input.
- Implement the scene/element model on top of the core compositor; automation after the command bus is stable; plugins last.

Every subsystem MUST be testable without requiring all other subsystems to be complete.

The definition of done for any subsystem is:

```text
schema-valid
state-safe
telemetry-visible
acceptance-tested
mix-minus-safe
click-free
loop-budget-accounted
sandbox-verified
degradation-ordered
```
