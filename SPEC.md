# NBE SPEC v0.2.5  
**News Broadcasting Engine**  
Status: consolidated specification — self-contained  
Relationship to earlier versions: this document consolidates SPEC v0.1 (`docs/spec.v0.1.md`) and SPEC v0.2 (`docs/spec.v0.2.md`) plus the v0.2.1 errata into a single normative document. Earlier versions remain in `docs/` as history. Where this document differs from them, this document wins.

---

## 0. Assumptions

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
16. **The normative schema lives at `schemas/manifest.v0.2.json`.** The copy embedded in Section 12 is identical; if they ever diverge, the repository file wins and the divergence is a spec bug.

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

# 3. System hierarchy

The canonical data hierarchy is:

```text
Network
  └── Channel         // future 24/7 scheduler; schema-only in v1
       └── Show
            └── Rundown
                 └── Segment
                      └── Subsegment
                           └── LayerStack
                                └── Layer
                                     └── Effects
```

v1 **MUST NOT** implement Channel scheduling, but the manifest schema **MUST NOT** preclude it.

Note (v0.3 preview): this single hierarchy will be split into two orthogonal axes — a time axis (Network → Channel → Show → Rundown → recursive Sequence) and a space axis (Scene → Sub-scene → Element → Effect). v0.2.5 keeps the v0.1/v0.2 model normative; the split is designed but not yet normative.

---

# 4. Subsystem 1 — Show/Rundown control plane

## 4.1 Responsibilities

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

It MUST NOT:

1. Decode video.
2. Composite frames.
3. Mix final audio.
4. Depend on OBS.
5. Require internet connectivity for local playout.

## 4.2 Runtime topology

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

## 4.3 WebSocket endpoint

Default local endpoint:

```text
ws://127.0.0.1:8462/nbe/v0.1
```

TLS endpoint for remote/VPC use:

```text
wss://render.local:8463/nbe/v0.1
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

## 4.4 Message envelope

All client-to-server command messages MUST use this envelope:

```json
{
  "v": "0.1",
  "id": "0d9f5c6a-7b8a-4a61-9b0a-5f5a5c8d99ab",
  "command": "program.take",
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
  "v": "0.1",
  "requestId": "0d9f5c6a-7b8a-4a61-9b0a-5f5a5c8d99ab",
  "status": "ok",
  "stateVersion": 413,
  "data": {}
}
```

Error response:

```json
{
  "v": "0.1",
  "requestId": "0d9f5c6a-7b8a-4a61-9b0a-5f5a5c8d99ab",
  "status": "error",
  "stateVersion": 412,
  "error": {
    "code": "E_FORBIDDEN_STATE",
    "message": "No preview item armed."
  }
}
```

## 4.5 State versioning

The control plane MUST maintain a monotonically increasing integer `stateVersion`.

A command with `baseStateVersion` not equal to current state version MUST fail with:

```text
E_VERSION_CONFLICT
```

Commands without `baseStateVersion` are accepted if otherwise valid.

## 4.6 Preview/Program buses

The engine MUST maintain two logical video buses:

| Bus | Meaning |
|---|---|
| `PREVIEW` | The item prepared to go live. |
| `PROGRAM` | The item currently live to output. |

The preview bus MUST be independently rendered and visible in operator UI.

A TAKE operation MUST promote the preview bus to program using the requested transition.

If no preview is armed, TAKE MUST fail.

Note (v0.3 preview): PROGRAM will be renamed **View** and PREVIEW stays **Preview**, matching operator language. `program.*` commands will remain as deprecated aliases for one schema version.

## 4.7 Item references

Commands use item references:

| Reference | Meaning |
|---|---|
| `A` | Segment A |
| `A1` | Subsegment A1 |
| `layer:A.lowerThird` | Layer with ID `lowerThird` in segment A or active subsegment |
| `guest:remote_1` | Guest source ID |
| `camera:main` | Camera source ID |

Subsegment IDs MUST match the `A1`, `A2`, `B1`, etc. convention.

---

# 5. Subsystem 2 — Media asset pipeline

## 5.1 Responsibilities

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

It MUST NOT:

1. Use variable frame rate output.
2. Use motion interpolation unless asset explicitly declares `cadence: interpolate`.
3. Use VP9/WebM alpha as a live format.
4. Rely on live transcoding during playout.

## 5.2 Normalized video format

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

## 5.3 Alpha loops

Alpha loops MUST use one of:

| Format | Status |
|---|---|
| ProRes 4444 | Preferred on Apple Silicon |
| HAP Alpha | Preferred when supported by engine |
| PNG sequence | Fallback |
| VP9/WebM alpha | Forbidden live format |

Alpha assets MUST contain a real alpha channel. Preflight MUST fail if alpha is absent.

## 5.4 Audio assets

Audio assets MUST be:

| Property | Requirement |
|---|---|
| Sample rate | 48 kHz |
| Bit depth | 16-bit PCM minimum, 24-bit preferred |
| Channels | mono or stereo |
| Loudness | normalized to show target |
| Soundboard assets | fully preloadable into RAM |

## 5.5 Graphics and fonts

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

## 5.6 Preflight

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
21. Effective loop metadata present for every `videoLoop` layer (see Section 11).
22. Loop budget math (see Section 11 and AC-21).

Preflight MUST fail on seeded:

1. Missing asset.
2. VFR clip.
3. Incorrect resolution.
4. Broken loop period.
5. Missing fallback asset.
6. Loudness out of tolerance.
7. Invalid manifest field.

Preflight result MUST be machine-readable.

---

# 6. Subsystem 3 — Playout/compositor engine

## 6.1 Implementation constraints

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

## 6.2 Render model

The compositor MUST use a GPU scene graph.

Per frame:

```text
1. Read master clock frame number.
2. Resolve PROGRAM item and PREVIEW item.
3. Resolve inherited layer stack.
4. For each visible layer:
   a. obtain source texture,
   b. apply transform/crop/opacity/chroma key,
   c. assign z-order.
5. Composite layers from low z to high z.
6. Render PROGRAM target.
7. Render PREVIEW target.
8. Submit frames to display/encoder.
9. Emit telemetry.
```

The compositor MUST be frame-deterministic for a given master-clock frame and show state.

## 6.3 Layer types

The engine MUST support the following layer kinds:

| Kind | Source |
|---|---|
| `videoLoop` | looping normal or alpha video |
| `clip` | one-shot or subsegment video |
| `camera` | local camera/capture device |
| `guest` | WHIP/WebRTC remote guest |
| `graphic` | template-generated graphic |
| `ticker` | scrolling ticker |
| `clock` | wall clock or show clock |

## 6.4 Layer stack inheritance

Layer stacks MUST support inheritance.

Merge modes:

| Mode | Behavior |
|---|---|
| `inherit` | Subsegment layers merge over segment layers. |
| `replace` | Subsegment stack replaces segment stack. |
| `merge` | Explicit merge by layer ID. |

Merge rule for `inherit` and `merge`:

1. Start with segment layers.
2. Replace any segment layer with the same `id` if present in subsegment.
3. Append subsegment-only layers.
4. Sort final stack by ascending `z`.
5. Apply visibility.

## 6.5 Chroma key

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

## 6.6 Transitions

v1 MUST support:

| Transition | Requirement |
|---|---|
| `cut` | immediate switch on next frame boundary |
| `mix` | crossfade over `durationFrames` |

Default crossfade duration:

```text
15 frames at 30 fps = 0.5 seconds
```

Transitions MUST be quantized to master-clock frame boundaries.

Future transitions MAY include wipe, sting, and DVE move, but are not v1 acceptance requirements.

### 6.6.1 Take latency (normative)

For a command accepted on localhost:

```text
takeLatency = firstVisibleProgramChangeFrame - commandAcceptedFrame
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

This applies to the local PROGRAM compositor output, not to downstream stream latency. See AC-17.

## 6.7 DVE and PiP

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

## 6.8 Frame budget

For 1080p30:

```text
frame deadline = 33.333 ms
```

A frame is dropped if PROGRAM output is not submitted by its deadline.

Target budget on Tier-1:

| Stage | Target |
|---|---:|
| state resolution | < 1 ms |
| layer graph eval | < 2 ms |
| GPU render | < 8 ms |
| encode submission | < 2 ms |
| OS/driver slack | remainder |

The engine MUST NOT block the render loop on:

1. control-plane WebSocket I/O,
2. thumbnail generation,
3. RSS fetch,
4. non-critical disk writes,
5. telemetry flush.

## 6.9 Fallback slate

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

# 7. Subsystem 4 — Audio engine

## 7.1 Core requirements

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
| `guestReturn` | per-guest mix-minus return (see 7.6) |
| `ifb` | anchor monitor/talkback (see 7.6) |

## 7.2 Bus controls

Each bus MUST support:

| Control | Range |
|---|---:|
| gain | -60 dB to +12 dB |
| mute | boolean |
| meter | peak + RMS |
| solo | boolean, monitor only |

The master bus MUST have:

1. compressor,
2. limiter,
3. loudness-safe output,
4. peak metering.

## 7.3 Ducking

The music bus MUST support ducking.

Default duck behavior:

| Parameter | Default |
|---|---:|
| depth | -6 dB |
| attack | 10 ms |
| release | 250 ms |
| trigger | manual `audio.duck` or voice-detected mic |

Ducking MUST NOT affect `mic` or `guest` buses unless explicitly configured.

## 7.4 Soundboard

Soundboard assets MUST be preloaded into RAM at show load.

Trigger latency MUST be under:

```text
20 ms
```

on Tier-1 hardware.

Soundboard playback MUST NOT cause dropped video frames.

## 7.5 Guest audio (inbound)

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

If guest audio fails, the guest layer MUST be muted automatically.

## 7.6 Guest return, mix-minus, and IFB

### 7.6.1 Guest return requirement

For every connected guest, the engine MUST create a return audio mix.

Guest return MUST be mix-minus:

```text
guestReturn(guestId) = programReturnMix - that guest's own inbound audio
```

The guest MUST NOT receive their own voice from the NBE return path.

This applies even if the guest is muted in program.

### 7.6.2 Default guest return mix

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

### 7.6.3 Guest return transport

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

### 7.6.4 Anchor IFB

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

### 7.6.5 Echo prevention

The engine MUST guarantee that a guest’s own audio does not enter their own return path.

If a guest is connected through WHIP/WebRTC:

1. inbound guest audio enters `guestBus(guestId)`,
2. `guestBus(guestId)` may enter program mix,
3. `guestBus(guestId)` MUST NOT enter `guestReturn(guestId)`.

A failure of this rule is an `E_AUDIO` fault.

## 7.7 Audio behavior during transitions

### 7.7.1 Click-free rule

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

### 7.7.2 `program.take` audio behavior

`program.take` payload includes an optional `audio` object (see Section 13).

### 7.7.3 Audio transition modes

| Mode | Behavior |
|---|---|
| `follow` | Follow subsegment `audioPolicy`. |
| `crossfade` | Crossfade outgoing and incoming audio over `audio.durationFrames`. |
| `cut` | Cut audio at boundary, but apply click-free ramp. |
| `mute` | Incoming audio muted; outgoing audio ramped out. |

### 7.7.4 Interaction with `audioPolicy`

When `audio.transition = follow`:

| Subsegment `audioPolicy` | Behavior |
|---|---|
| `clip` | Clip audio is active and crossfaded or ramped according to transition. |
| `bed` | Clip audio is muted; music bed continues. |
| `mute` | Incoming clip audio is muted; outgoing audio ramped out. |

### 7.7.5 Video `mix` default

If video transition is `mix` and no audio override is given, audio MUST crossfade over the same duration.

Crossfade curve:

```text
equal-power crossfade
```

Linear crossfade is allowed, but equal-power is recommended.

### 7.7.6 Video `cut` default

If video transition is `cut`, audio MUST follow `audioPolicy` and MUST apply at least a 5 ms ramp at any start/stop boundary.

### 7.7.7 Live camera and guest audio

For live camera and guest sources:

| Transition | Audio behavior |
|---|---|
| `cut` | 10 ms ramp by default |
| `mix` | crossfade over transition duration |
| `mute` | ramp out and mute |

---

# 8. Subsystem 5 — Output/distribution

## 8.1 Outputs

The engine MUST support:

1. Local full-screen program display.
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

## 8.2 Hardware encoding

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

## 8.3 Recording

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

## 8.4 Streaming

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

## 8.5 Local network survivability

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
| guest layer | placeholder or fallback if live |
| RSS ticker | last cached items or manual items |

## 8.6 WHIP auth, TURN vending, NDI feature flag, WHEP preview

### 8.6.1 WHIP authentication

WHIP ingest endpoints MUST require bearer authentication.

Example:

```http
POST /nbe/v0.1/whip/guest/GUEST_ID
Authorization: Bearer <guest-token>
```

If token is missing or invalid, the endpoint MUST return HTTP 401.

### 8.6.2 TURN credential vending

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

### 8.6.3 NDI feature flag

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

### 8.6.4 WHEP preview for iPhone

iPhone preview MUST use WHEP.

Endpoints:

```text
POST /nbe/v0.1/whep/program
POST /nbe/v0.1/whep/preview
```

Authentication:

```http
Authorization: Bearer <controller-token>
```

MJPEG fallback:

```text
GET /nbe/v0.1/mjpeg/program
GET /nbe/v0.1/mjpeg/preview
```

MJPEG MUST be disabled by default and enabled only in dev mode.

---

# 9. Subsystem 6 — Monitoring/reliability

## 9.1 Telemetry

The engine MUST emit telemetry at least once per second.

Telemetry fields:

```json
{
  "ts": 1768000000000,
  "masterClockFrame": 54000,
  "programItem": "B2",
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
  "fallbackActive": false
}
```

## 9.2 Dropped-frame definition

A dropped frame is any PROGRAM frame not presented/submitted by its master-clock deadline.

Preview-only misses are not counted as live dropped frames but MUST be logged as preview misses.

## 9.3 Watchdog

The render node MUST implement a frame watchdog.

If the render loop misses a deadline by more than:

```text
1 frame
```

the watchdog MUST:

1. log fault,
2. increment fault counter,
3. activate fallback slate if the fault affects PROGRAM.

## 9.4 Health endpoint

The control plane MUST expose:

```text
GET /nbe/v0.1/status
```

Response MUST include:

1. show load state,
2. master clock state,
3. render node health,
4. stream health,
5. recording health,
6. preflight state,
7. last error.

## 9.5 Failure UI

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

# 10. Cross-cutting concern — Master clock

## 10.1 Authority

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

## 10.2 Clock source

The master clock MUST be based on a monotonic system clock.

It MUST NOT use wall-clock time as its primary source.

Wall-clock time MAY drive the `clock` layer, but not frame scheduling.

## 10.3 Clock epoch

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

## 10.4 Clock states

| State | Meaning |
|---|---|
| `STOPPED` | no frame advancement |
| `RUNNING` | normal show clock |
| `HELD` | operator freeze, emergency |
| `SLAVE` | optional future sync to external timecode |

v1 MUST implement `STOPPED` and `RUNNING`.

## 10.5 Drift policy

### 10.5.1 Local deterministic sources

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

### 10.5.2 Remote guest sources

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

### 10.5.3 Guest video frame-selection policy

Guest video MUST use:

```text
hold-latest-complete-frame
```

At each PROGRAM frame deadline, the compositor MUST use the most recent completely decoded guest frame available.

If no new complete frame has arrived, the compositor MUST repeat the previous guest frame.

If no guest frame has arrived for more than:

```text
500 ms
```

the guest layer MUST display a placeholder.

If the guest layer is the only meaningful program source and no placeholder is available, the engine MUST activate fallback slate.

Guest video SHOULD sync to guest audio presentation time, not to master clock.

## 10.6 Command timing

Commands take effect at the next safe frame boundary unless the command specifies immediate emergency behavior.

TAKE MUST begin no later than the next frame boundary after acceptance.

---

# 11. Cross-cutting concern — Deterministic loops

## 11.1 Loop function

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

## 11.2 Loop cache policy

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

## 11.3 Cache texture formats

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

## 11.4 Budgets

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

## 11.5 Frame budget formula

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

## 11.6 Apple unified-memory rule

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

1. program/preview render targets,
2. live camera textures,
3. guest textures,
4. fallback slate,
5. encoder interop surfaces.

Loop cache MUST NOT exceed the remaining safe budget.

If `vramBudgetMib` exceeds device safe budget, the engine MUST clamp it and log a warning.

## 11.7 VRAM ring buffer

VRAM-resident loops MUST use a texture ring buffer or texture array.

Frame selection:

```text
textureSlot = sourceIndex mod P
```

There MUST be no decoder restart at loop wrap.

## 11.8 Long-loop streaming

Long loops MUST use double-buffered read-ahead.

Minimum read-ahead:

```text
max(2 * GOP length, 60 frames)
```

Wrap policy:

1. Before loop end, pre-stage decoder/seek for frame 0.
2. Maintain next-window buffer.
3. Wrap MUST NOT block the render thread.
4. If wrap read-ahead fails, the loop layer MUST fall back to frozen frame or fallback slate if live.

## 11.9 Loop metadata precedence

If both `asset.loop` and `layer.loop` exist:

```text
layer.loop overrides asset.loop
```

If `layer.loop` is absent:

```text
asset.loop is used
```

If neither exists and layer kind is `videoLoop`, preflight MUST fail.

Preflight MUST validate the effective loop against the actual asset duration.

Effective loop texture-format resolution order:

```text
layer.loop.textureFormat
asset.loop.textureFormat
engine default
```

## 11.10 Loop preflight

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

# 12. Manifest JSON Schema v0.2

The normative manifest schema lives at `schemas/manifest.v0.2.json` in the repository and is embedded in full below. It consolidates the v0.1 schema with all v0.2 amendments (widened segment/subsegment ID patterns, `Pulldown`, `ClockConfig`, loop `textureFormat`, `isolation` hook, `preview` output hook, top-level `features`) and the v0.2.1 errata (`feedAssetId` on Layer, crop bounds restored to 0–1, NBE rename).

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "https://nbe.local/schemas/manifest.v0.2.json",
  "title": "NBE Show Manifest v0.2",
  "type": "object",
  "additionalProperties": false,
  "required": ["manifestVersion", "network", "show", "assets", "rundown", "control"],
  "properties": {
    "manifestVersion": { "const": "0.2" },
    "network": { "$ref": "#/$defs/Network" },
    "channel": { "$ref": "#/$defs/Channel" },
    "show": { "$ref": "#/$defs/Show" },
    "assets": { "type": "array", "minItems": 1, "items": { "$ref": "#/$defs/Asset" } },
    "templates": { "type": "array", "items": { "$ref": "#/$defs/GraphicTemplate" } },
    "rundown": { "$ref": "#/$defs/Rundown" },
    "control": { "$ref": "#/$defs/Control" },
    "features": { "$ref": "#/$defs/Features" }
  },
  "$defs": "See schemas/manifest.v0.2.json — the embedded copy elides nothing; the repository file is the byte-exact normative artifact."
}
```

Editorial note: earlier drafts embedded the full schema inline in this document. As of v0.2.5 the schema is maintained as a standalone repository file (`schemas/manifest.v0.2.json`), validated in CI, and that file — not any inline copy — is normative. This prevents spec/schema drift by construction.

---

# 13. Command API

All commands use the WebSocket envelope defined earlier.

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

## 13.1 Show commands

| Command | Payload schema | Preconditions | State transitions | Failure modes |
|---|---|---|---|---|
| `show.load` | `{ packagePath: string, mode?: "load"\|"reload" }` | no live program | show `UNLOADED -> LOADED` | `E_BAD_PAYLOAD`, `E_NOT_FOUND`, `E_ENGINE` |
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

## 13.2 Preview/Program commands

| Command | Payload schema | Preconditions | State transitions | Failure modes |
|---|---|---|---|---|
| `preview.set` | `{ itemRef: string }` | item exists and READY/ARMED | target `READY -> ARMED`; previous preview may return to READY | `E_NOT_FOUND`, `E_ASSET_MISSING` |
| `program.take` | see below | preview armed | preview item becomes LIVE or PLAYING; previous live becomes READY; audio transition executes | `E_FORBIDDEN_STATE`, `E_AUDIO`, `E_ENGINE` |
| `program.cut` | `{ itemRef: string }` | item exists | immediate program switch to item | `E_NOT_FOUND`, `E_FORBIDDEN_STATE` |
| `program.fallback` | `{ reason?: string }` | always allowed | PROGRAM switches to fallback slate | `E_ENGINE` |

`program.take` payload schema:

```json
{
  "transition": { "enum": ["cut", "mix"] },
  "durationFrames": { "type": "integer", "minimum": 0, "maximum": 120 },
  "audio": {
    "type": "object",
    "additionalProperties": false,
    "properties": {
      "transition": { "enum": ["follow", "crossfade", "cut", "mute"] },
      "durationFrames": { "type": "integer", "minimum": 1, "maximum": 120 },
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

## 13.3 Segment/subsegment commands

| Command | Payload schema | Preconditions | State transitions | Failure modes |
|---|---|---|---|---|
| `segment.arm` | `{ segmentId: string }` | segment exists | segment `READY -> ARMED` | `E_NOT_FOUND` |
| `segment.unarm` | `{ segmentId: string }` | segment armed | segment `ARMED -> READY` | `E_NOT_FOUND`, `E_FORBIDDEN_STATE` |
| `subsegment.arm` | `{ subsegmentId: "A1" }` | subsegment exists | subsegment `READY -> ARMED` | `E_NOT_FOUND`, `E_ASSET_MISSING` |
| `subsegment.unarm` | `{ subsegmentId: "A1" }` | armed | subsegment `ARMED -> READY` | `E_NOT_FOUND`, `E_FORBIDDEN_STATE` |
| `subsegment.stop` | `{ subsegmentId: "A1" }` | playing | subsegment `PLAYING -> READY` | `E_NOT_FOUND`, `E_FORBIDDEN_STATE` |

## 13.4 Layer/graphic commands

| Command | Payload schema | Preconditions | State transitions | Failure modes |
|---|---|---|---|---|
| `layer.toggle` | `{ layerId: string, scope?: string, visible?: boolean }` | layer exists | layer visible state toggled | `E_NOT_FOUND` |
| `layer.set` | `{ layerId: string, patch: { visible?, opacity?, transform?, chromaKey? } }` | layer exists | layer properties updated | `E_BAD_PAYLOAD`, `E_NOT_FOUND` |
| `graphic.show` | `{ templateId: string, fields: object, layerId?: string, z?: integer }` | template exists | graphic layer becomes visible | `E_NOT_FOUND`, `E_BAD_PAYLOAD` |
| `graphic.hide` | `{ layerId?: string, templateId?: string }` | graphic visible/known | graphic hidden | `E_NOT_FOUND` |
| `graphic.update` | `{ layerId: string, fields: object }` | graphic exists | graphic fields updated | `E_NOT_FOUND`, `E_BAD_PAYLOAD` |
| `breaking.show` | `{ headline: string, subhead?: string }` | breaking template exists | breaking banner visible | `E_NOT_FOUND` |
| `breaking.hide` | `{}` | breaking visible or hidden | breaking banner hidden | none |

## 13.5 Ticker commands

| Command | Payload schema | Preconditions | State transitions | Failure modes |
|---|---|---|---|---|
| `ticker.setSource` | `{ source: "manual"\|"rss"\|"mixed" }` | ticker layer exists | ticker source changed | `E_NOT_FOUND` |
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

## 13.6 Soundboard/audio commands

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

## 13.7 Guest commands

| Command | Payload schema | Preconditions | State transitions | Failure modes |
|---|---|---|---|---|
| `guest.connect` | `{ guestId: string, whipUrl: string, displayName?: string }` | guest not connected | guest source `READY` | `E_NETWORK`, `E_BAD_PAYLOAD` |
| `guest.disconnect` | `{ guestId: string }` | guest exists | guest source disconnected | `E_NOT_FOUND` |
| `guest.setLayout` | `{ guestId: string, layout: "pip"\|"full" }` | guest layer exists | guest transform updated | `E_NOT_FOUND` |
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

## 13.8 Output commands

| Command | Payload schema | Preconditions | State transitions | Failure modes |
|---|---|---|---|---|
| `record.start` | `{ outputId?: string }` | show running, encoder available | recording active | `E_NO_HARDWARE_ENCODER`, `E_DISK` |
| `record.stop` | `{}` | recording active | recording stopped | `E_FORBIDDEN_STATE` |
| `stream.start` | `{ outputId?: string, url?: string }` | show running, encoder available | stream active | `E_NO_HARDWARE_ENCODER`, `E_NETWORK` |
| `stream.stop` | `{}` | stream active | stream stopped | `E_FORBIDDEN_STATE` |

## 13.9 Clock commands

| Command | Payload schema | Preconditions | State transitions | Failure modes |
|---|---|---|---|---|
| `clock.configure` | see below | clock layer exists | clock layer config updated | `E_NOT_FOUND`, `E_BAD_PAYLOAD` |

`clock.configure` payload schema:

```json
{
  "layerId": { "type": "string" },
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

Required: `["layerId"]`.

## 13.10 System commands

| Command | Payload schema | Preconditions | State transitions | Failure modes |
|---|---|---|---|---|
| `system.status` | `{}` | always | none | none |
| `system.telemetry.subscribe` | `{ intervalMs?: integer }` | always | telemetry subscription active | `E_BAD_PAYLOAD` |
| `system.telemetry.unsubscribe` | `{}` | subscribed | telemetry subscription removed | none |

---

# 14. State machine

## 14.1 Item states

An item is a Segment, Subsegment, or playable source item.

States:

| State | Meaning | UI indication |
|---|---|---|
| `READY` | Valid, not armed, not live. | gray |
| `ARMED` | In preview/next, preloaded. | yellow |
| `LIVE` | Live non-timed source on PROGRAM. | red |
| `PLAYING` | Timed media active on PROGRAM. | green with red live border |
| `DONE` | Timed item completed. Optional state. | dim green |
| `MISSING` | Required asset/source missing. | flashing red outline |
| `ERROR` | Runtime failure. | red banner |

## 14.2 Transition table

| Current | Event/command | Guard | Next | Side effects |
|---|---|---|---|---|
| `READY` | `arm` | asset valid | `ARMED` | preload, set preview |
| `READY` | asset missing detected | missing | `MISSING` | alert UI |
| `READY` | decode error | failure | `ERROR` | alert UI |
| `ARMED` | `unarm` | not live | `READY` | release preview |
| `ARMED` | `take` | live source | `LIVE` | program switch |
| `ARMED` | `take` | timed media | `PLAYING` | program switch, start media clock |
| `ARMED` | asset missing detected | missing | `MISSING` | alert UI, fallback if preview required |
| `ARMED` | decode error | failure | `ERROR` | fallback if armed critical |
| `LIVE` | `take` away | another item goes live | `READY` | remove from program |
| `LIVE` | device loss | camera/guest lost | `ERROR` | fallback if program |
| `PLAYING` | end reached | duration complete | `DONE` | mark complete |
| `PLAYING` | `stop` | operator stop | `READY` | stop media |
| `PLAYING` | `take` away | another item goes live | `READY` | remove from program |
| `PLAYING` | decode error | failure | `ERROR` | fallback if program |
| `DONE` | `reset`/`arm` | asset valid | `READY` or `ARMED` | reset counters |
| `MISSING` | asset restored | preflight pass | `READY` | clear alert |
| `MISSING` | unrecoverable | manual reset | `ERROR` | alert |
| `ERROR` | `reset` | recoverable | `READY` | clear fault |
| `ERROR` | unrecoverable | none | remains `ERROR` | require reload |

## 14.3 Text diagram

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

# 15. Cadence rules

## 15.1 House rate

Default house rate:

```text
30 fps
```

All final show media MUST be normalized to house rate before live load.

## 15.2 Cadence preservation

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

## 15.3 Hold patterns

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

## 15.4 25 fps and 29.97 fps

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

## 15.5 Unsupported source rates

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

The `Pulldown` schema definition (one of `pattern`, `repeatNthSourceFrame`, `repeatOnePerNSourceFrames`) is normative in `schemas/manifest.v0.2.json`. The v0.1 `pulldownPattern` field is deprecated; `pulldown` wins if both are present.

## 15.6 Cadence preflight

Preflight MUST verify:

1. output frame rate equals house rate,
2. output duration matches expected duration,
3. duplicate-frame pattern matches declared cadence where feasible,
4. no interpolated intermediate frames are present for `preserve` assets,
5. VFR is absent.

---

# 16. Preflight details

## 16.1 Exit status

`preflight` MUST exit with:

| Code | Meaning |
|---:|---|
| 0 | air-ready |
| 1 | warnings only, not air-ready unless `--allow-warnings` |
| 2 | errors, not air-ready |

CI MUST block load on exit code != 0 unless explicitly overridden.

## 16.2 Report schema

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
  "contactSheet": "contact_sheet.jpg"
}
```

The `loops` entries MUST follow the extended report shape defined in Section 11.10.

## 16.3 Seeded failure tests

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

---

# 17. MVP scope hard ceiling

The MVP MUST NOT exceed the following live complexity:

| Item | Maximum |
|---|---:|
| live camera sources | 1 |
| preloaded clips per segment | 3 |
| background loops | 1 |
| alpha logo loops | 1 |
| simultaneous WHIP guests | 1 |
| lower-third templates | headline + name |
| breaking banner | 1 |
| ticker | manual + RSS |
| clock | 1 |
| transitions | cut + crossfade |
| outputs | local program, preview, recording, one RTMP/SRT |
| resolution | 1920x1080 |
| frame rate | 30 fps |

The MVP MAY support future schema fields, but acceptance is based only on the above.

---

# 18. Hardware tiers

These tiers are normative for 1080p30 house rate unless otherwise stated.

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
| Role | WebSocket controller, preview monitor (WHEP) |
| Forbidden | live renderer, primary compositor |

---

# 19. Acceptance criteria

Each criterion is independently testable.

## AC-1 — Manifest schema validation

Given a valid show package, `preflight` MUST validate the manifest against the normative NBE manifest schema (`schemas/manifest.v0.2.json`) and return exit code 0.

## AC-2 — Missing asset detection

Given a seeded manifest referencing a nonexistent asset, `preflight` MUST fail with a machine-readable error identifying the asset ID and path.

## AC-3 — VFR detection

Given a seeded VFR clip, `preflight` MUST fail and report `cfr: false`.

## AC-4 — Cadence preservation

Given 15, 10, 12, and 24 fps source assets normalized to 30 fps with `cadence: preserve`, preflight MUST verify the declared hold patterns and fail if motion interpolation is detected.

## AC-5 — 30-minute zero-drop live show

On a Tier-1 reference machine, a 30-minute continuous live show at 1080p30, single operator, with MVP maximum layers active, MUST produce zero dropped PROGRAM frames.

Measurement:

```text
droppedFramesTotal == 0
```

over the full show.

## AC-6 — Crash-safe recording

If the render process is killed with `SIGKILL` during recording, the resulting fragmented MP4 or MKV file MUST be playable by `ffprobe` and at least one reference player.

## AC-7 — Fallback slate latency

If a live segment source fails, the engine MUST cut to the fallback slate no later than one frame after the missed deadline.

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

1. local program continues,
2. recording continues,
3. stream enters reconnect/backoff,
4. no PROGRAM frames are dropped due to stream failure.

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

## AC-12 — Companion command path

A Bitfocus Companion button mapped to `program.take` MUST cause a successful take via the WebSocket command bus with no custom Stream Deck plugin.

## AC-13 — Soundboard latency

A soundboard trigger MUST produce audible output within 20 ms on Tier-1 hardware and MUST NOT cause dropped PROGRAM frames.

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
3. arm first segment,
4. start show,
5. take between segments,
6. trigger lower third,
7. trigger breaking banner,
8. play soundboard effect,
9. start/stop recording,
10. start/stop stream,

without using a keyboard-driven debug console.

## AC-17 — Normative take latency

On localhost, an accepted `program.take` command MUST change the local PROGRAM output within:

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

During any `program.take`, `subsegment.stop`, `soundboard.stop`, or bus mute/unmute:

1. gain changes MUST have ramps ≥ 5 ms,
2. no hard sample-step cut is allowed,
3. recorded master output MUST contain no click impulse exceeding -60 dBFS in a silent test pass.

## AC-20 — WHEP preview

A WHEP client MUST be able to fetch both:

```text
/nbe/v0.1/whep/program
/nbe/v0.1/whep/preview
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

---

# 20. Non-goals for v1

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

The manifest schema MUST NOT preclude future Channel scheduling, but v1 MUST NOT implement it.

---

# 21. Risks & mitigations

| Risk | Severity | Mitigation |
|---|---:|---|
| Thermal throttling on laptop render node | High | Use desktop/mini workstation for live; enforce Tier-1 GPU budget; monitor thermal state; reduce loop cache; prefer hardware decode/encode. |
| VideoToolbox decode-session limits | High | Limit simultaneous active decode sources; preload short loops into textures; reuse decode sessions; fail early in preflight if decode budget exceeded. |
| VRAM pressure from loop caches | High | Enforce per-loop and total cache budgets; evict non-live loops; stream long loops; fallback to still frame if texture pressure critical. |
| WebRTC jitter causing guest freeze | Medium | Jitter buffer 200–500 ms; placeholder on loss; automatic fallback if guest is live; separate guest from local playout clock. |
| Single-operator cognitive load | High | Big preview/program UI; color states; armed next segment; one-button TAKE; automatic fallback; minimal menus during live. |
| `wgpu` driver/platform differences | Medium | Conformance suite; Metal-first path; Linux Vulkan secondary path; OBS baseline benchmark; feature flags for backend-specific paths. |
| Audio/video drift | Medium | Master clock authority; audio device clock monitoring; drift correction; acceptance test over 30 minutes. |
| RSS feed malicious or malformed | Medium | Sanitize text; disable markup; cache last known items; manual override; feed timeout. |
| Recording corruption on crash | High | Fragmented MP4 or MKV; 1-second fragments; kill-test in CI. |
| Disk I/O stalls during long loops | Medium | NVMe requirement; double-buffer read-ahead; preflight disk read benchmark; separate media disk on Tier-2+. |
| Companion misconfiguration | Medium | Generate Companion bindings from manifest; preflight validates action names and payload schemas. |
| OBS baseline comparison unfair | Low/Medium | Define fixed test package, hardware, metrics, and capture method in test harness. |

---

# 22. Resolved open questions

The v0.1 open questions are closed with the following rulings (carried from v0.2, normative):

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

v0.2.1 errata (adopted):

1. NV12 loop-cache textures are implemented as two-plane YUV (`R8Unorm` + `Rg8Unorm`) with shader-side BT.709 conversion; literal NV12 texture formats are not assumed in `wgpu`.
2. `mixDurationFrames` minimum is 1; a 0-frame mix is a cut, and the schema enforces it.
3. `E_TIMEOUT` is reserved for async network boundaries (TURN vending, WHIP handshake, RSS fetches) and is wired in the control plane and guest ingest modules.

---

# 23. Implementation handoff notes

For coding agents, the implementation order SHOULD be:

1. Manifest schema validator and preflight skeleton.
2. Control-plane WebSocket server and state machine.
3. Render-node command bridge.
4. Basic program/preview compositor with color sources and image/video layers.
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
```
