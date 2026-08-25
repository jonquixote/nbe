# WNBE SPEC v0.1  
**Worker News Broadcast Engine**  
Status: draft for implementation handoff  
Intended audience: LLM coding agents, broadcast systems engineers, QA engineers.

No clarifying questions are being asked. The locked decisions are accepted as normative. Where ambiguity remains, it is recorded in **Open Questions** and **Assumptions**.

---

## 0. Assumptions

Because no clarifying questions were asked, the following assumptions are normative for v0.1 unless later changed by spec revision:

1. **Single primary render node for MVP.** The control plane may run on the same machine as the render node.
2. **macOS Apple Silicon is the primary live playout target.** Linux cloud nodes are for guest ingest, distribution, backup, and benchmarking, not the primary local playout path unless explicitly configured.
3. **All live media is pre-normalized.** The live render engine must not transcode, motion-interpolate, or repair media during live playout.
4. **OBS is not a dependency.** OBS is only used as a benchmark baseline and optional Plan B through `obs-websocket`.
5. **Smelter is reference only.** The WNBE Rust/wgpu engine is custom. Smelter informs API shape and benchmarking but is not required at runtime.
6. **Companion is the Stream Deck integration path.** Companion emits WNBE WebSocket commands. No custom Stream Deck plugin is built for v1.
7. **Show packages are self-contained folders.** All local assets are referenced by relative path from the package root.
8. **The operator is a single human.** UX must minimize cognitive load, favor big state-clear controls, and automate recovery where possible.
9. **Internet independence is mandatory.** Local playout must continue if WAN is lost. Remote guests and streaming may fail gracefully.
10. **House rate is 30 fps for MVP.** 60 fps is manifest-supported for future showcase episodes but is not required for v1 acceptance.
11. **Fonts and graphic templates are packaged.** Text rendering must not depend on host-system fonts unless explicitly declared.
12. **Security is local-first.** v1 assumes a trusted local network or VPN. Auth tokens are used, but full multi-tenant RBAC is not a v1 hard requirement.
13. **RSS ticker content is sanitized.** The ticker renderer must treat RSS text as untrusted display text, not markup or code.
14. **Recording container default is fragmented MP4.** Matroska is allowed, but fragmented MP4 is the default crash-safe container.
15. **Hardware encode is mandatory.** If hardware encoder is unavailable, live streaming/recording must refuse to start rather than fall back to CPU x264.

---

# 1. Scope and locked decisions

WNBE is a purpose-built live news broadcast/playout system.

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
ws://127.0.0.1:8462/wnbe/v0.1
```

TLS endpoint for remote/VPC use:

```text
wss://render.local:8463/wnbe/v0.1
```

Connection handshake MUST include:

```http
Authorization: Bearer <token>
X-WNBE-Role: operator|producer|monitor|admin|render
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
| `v` | string | yes | Must be `"0.1"`. |
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

## 7.5 Guest audio

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
GET /wnbe/v0.1/status
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

Audio and video MUST remain within:

```text
±1 frame
```

over a 30-minute show on Tier-1 hardware.

If drift exceeds one frame, the engine MUST log and correct by adjusting audio presentation or dropping/holding non-critical frames. It MUST NOT allow unbounded drift.

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

## 11.3 Short-loop VRAM budget

For MVP, the default short-loop budgets are:

| Budget | Value |
|---|---:|
| maximum loop frame count | 900 frames |
| default per-loop memory budget | 256 MiB |
| total short-loop cache budget | 512 MiB |

A loop is VRAM-resident if:

```text
periodFrames <= 900
decodedTextureBytes(periodFrames) <= perLoopBudget
totalShortLoopCache <= totalBudget
```

If any condition fails, the loop MUST be streamed.

Manifest MAY override `vramBudgetMib`, but engine MAY refuse if budget exceeds safe device limits.

## 11.4 VRAM ring buffer

VRAM-resident loops MUST use a texture ring buffer or texture array.

Frame selection:

```text
textureSlot = sourceIndex mod P
```

There MUST be no decoder restart at loop wrap.

## 11.5 Long-loop streaming

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

## 11.6 Loop preflight

Preflight MUST verify:

1. `expectedDurationFrames == loop.periodFrames` if both present.
2. first frame decodes,
3. last frame decodes,
4. wrap index is valid,
5. no audio gap if loop has audio,
6. no VFR.

---

# 12. Manifest JSON Schema v0.1

The following is the formal manifest schema.

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "https://wnbe.local/schemas/manifest.v0.1.json",
  "title": "WNBE Show Manifest v0.1",
  "type": "object",
  "additionalProperties": false,
  "required": [
    "manifestVersion",
    "network",
    "show",
    "assets",
    "rundown",
    "control"
  ],
  "properties": {
    "manifestVersion": {
      "const": "0.1"
    },
    "network": {
      "$ref": "#/$defs/Network"
    },
    "channel": {
      "$ref": "#/$defs/Channel"
    },
    "show": {
      "$ref": "#/$defs/Show"
    },
    "assets": {
      "type": "array",
      "minItems": 1,
      "items": {
        "$ref": "#/$defs/Asset"
      }
    },
    "templates": {
      "type": "array",
      "items": {
        "$ref": "#/$defs/GraphicTemplate"
      }
    },
    "rundown": {
      "$ref": "#/$defs/Rundown"
    },
    "control": {
      "$ref": "#/$defs/Control"
    }
  },
  "$defs": {
    "Id": {
      "type": "string",
      "pattern": "^[A-Za-z0-9._-]+$",
      "minLength": 1,
      "maxLength": 128
    },
    "Network": {
      "type": "object",
      "additionalProperties": false,
      "required": [
        "id",
        "name"
      ],
      "properties": {
        "id": {
          "$ref": "#/$defs/Id"
        },
        "name": {
          "type": "string",
          "minLength": 1
        },
        "logoAssetId": {
          "$ref": "#/$defs/Id"
        },
        "fallbackAudioAssetId": {
          "$ref": "#/$defs/Id"
        }
      }
    },
    "Channel": {
      "type": "object",
      "additionalProperties": true,
      "required": [
        "id"
      ],
      "properties": {
        "id": {
          "$ref": "#/$defs/Id"
        },
        "name": {
          "type": "string"
        },
        "futureScheduler": {
          "type": "object",
          "additionalProperties": true
        }
      }
    },
    "Show": {
      "type": "object",
      "additionalProperties": false,
      "required": [
        "id",
        "title",
        "video",
        "audio",
        "fallbackAssetId"
      ],
      "properties": {
        "id": {
          "$ref": "#/$defs/Id"
        },
        "title": {
          "type": "string",
          "minLength": 1
        },
        "episodeCode": {
          "type": "string"
        },
        "video": {
          "$ref": "#/$defs/VideoSpec"
        },
        "audio": {
          "$ref": "#/$defs/AudioSpec"
        },
        "transitions": {
          "$ref": "#/$defs/TransitionDefaults"
        },
        "fallbackAssetId": {
          "$ref": "#/$defs/Id"
        },
        "outputs": {
          "$ref": "#/$defs/OutputDefaults"
        }
      }
    },
    "VideoSpec": {
      "type": "object",
      "additionalProperties": false,
      "required": [
        "width",
        "height",
        "frameRate",
        "colorSpace"
      ],
      "properties": {
        "width": {
          "type": "integer",
          "minimum": 640,
          "maximum": 8192
        },
        "height": {
          "type": "integer",
          "minimum": 360,
          "maximum": 8192
        },
        "frameRate": {
          "enum": [
            30,
            60
          ]
        },
        "colorSpace": {
          "enum": [
            "rec709"
          ]
        },
        "aspectRatio": {
          "const": "16:9"
        }
      }
    },
    "AudioSpec": {
      "type": "object",
      "additionalProperties": false,
      "required": [
        "sampleRate",
        "loudnessTargetLufs",
        "truePeakDbtp"
      ],
      "properties": {
        "sampleRate": {
          "const": 48000
        },
        "loudnessTargetLufs": {
          "type": "number",
          "default": -16
        },
        "truePeakDbtp": {
          "type": "number",
          "default": -1.5
        },
        "defaultLanguage": {
          "type": "string"
        }
      }
    },
    "TransitionDefaults": {
      "type": "object",
      "additionalProperties": false,
      "properties": {
        "defaultTake": {
          "enum": [
            "cut",
            "mix"
          ],
          "default": "cut"
        },
        "mixDurationFrames": {
          "type": "integer",
          "minimum": 1,
          "maximum": 120,
          "default": 15
        }
      }
    },
    "OutputDefaults": {
      "type": "object",
      "additionalProperties": false,
      "properties": {
        "record": {
          "type": "object",
          "additionalProperties": false,
          "properties": {
            "container": {
              "enum": [
                "fragmentedMp4",
                "matroska"
              ],
              "default": "fragmentedMp4"
            },
            "directory": {
              "type": "string"
            }
          }
        },
        "stream": {
          "type": "object",
          "additionalProperties": false,
          "properties": {
            "protocol": {
              "enum": [
                "rtmp",
                "srt",
                "whip"
              ]
            },
            "videoBitrateKbps": {
              "type": "integer",
              "minimum": 500,
              "maximum": 50000
            },
            "audioBitrateKbps": {
              "type": "integer",
              "minimum": 96,
              "maximum": 320
            }
          }
        }
      }
    },
    "Asset": {
      "type": "object",
      "additionalProperties": false,
      "required": [
        "id",
        "kind",
        "source"
      ],
      "properties": {
        "id": {
          "$ref": "#/$defs/Id"
        },
        "kind": {
          "enum": [
            "video",
            "alphaVideo",
            "audio",
            "image",
            "font",
            "rss"
          ]
        },
        "source": {
          "type": "string",
          "minLength": 1
        },
        "sha256": {
          "type": "string",
          "pattern": "^[0-9a-fA-F]{64}$"
        },
        "format": {
          "enum": [
            "h264",
            "prores4444",
            "hapAlpha",
            "pngSequence",
            "aac",
            "pcm",
            "wav",
            "png",
            "svg",
            "ttf",
            "otf",
            "rss"
          ]
        },
        "cadence": {
          "enum": [
            "preserve",
            "interpolate"
          ],
          "default": "preserve"
        },
        "sourceFrameRate": {
          "type": "number",
          "exclusiveMinimum": 0
        },
        "expectedDurationFrames": {
          "type": "integer",
          "minimum": 1
        },
        "pulldownPattern": {
          "type": "array",
          "minItems": 1,
          "items": {
            "type": "integer",
            "minimum": 1
          }
        },
        "loop": {
          "$ref": "#/$defs/LoopMetadata"
        },
        "loudness": {
          "$ref": "#/$defs/LoudnessReport"
        }
      }
    },
    "LoopMetadata": {
      "type": "object",
      "additionalProperties": false,
      "required": [
        "periodFrames"
      ],
      "properties": {
        "periodFrames": {
          "type": "integer",
          "minimum": 1
        },
        "t0Frames": {
          "type": "integer",
          "minimum": 0,
          "default": 0
        },
        "seamless": {
          "type": "boolean",
          "default": true
        },
        "cachePolicy": {
          "enum": [
            "auto",
            "vram",
            "stream"
          ],
          "default": "auto"
        },
        "vramBudgetMib": {
          "type": "integer",
          "minimum": 1
        }
      }
    },
    "LoudnessReport": {
      "type": "object",
      "additionalProperties": false,
      "properties": {
        "integratedLufs": {
          "type": "number"
        },
        "truePeakDbtp": {
          "type": "number"
        },
        "loudnessRange": {
          "type": "number"
        },
        "measuredBy": {
          "type": "string"
        }
      }
    },
    "GraphicTemplate": {
      "type": "object",
      "additionalProperties": false,
      "required": [
        "id",
        "kind"
      ],
      "properties": {
        "id": {
          "$ref": "#/$defs/Id"
        },
        "kind": {
          "enum": [
            "lowerThirdHeadline",
            "lowerThirdName",
            "breakingBanner",
            "ticker",
            "generic"
          ]
        },
        "fontAssetIds": {
          "type": "array",
          "items": {
            "$ref": "#/$defs/Id"
          }
        },
        "fields": {
          "type": "array",
          "items": {
            "type": "object",
            "additionalProperties": false,
            "required": [
              "name"
            ],
            "properties": {
              "name": {
                "type": "string"
              },
              "label": {
                "type": "string"
              },
              "multiline": {
                "type": "boolean",
                "default": false
              },
              "direction": {
                "enum": [
                  "auto",
                  "ltr",
                  "rtl"
                ],
                "default": "auto"
              },
              "maxLength": {
                "type": "integer",
                "minimum": 1
              }
            }
          }
        }
      }
    },
    "Rundown": {
      "type": "object",
      "additionalProperties": false,
      "required": [
        "segments"
      ],
      "properties": {
        "segments": {
          "type": "array",
          "minItems": 1,
          "items": {
            "$ref": "#/$defs/Segment"
          }
        }
      }
    },
    "Segment": {
      "type": "object",
      "additionalProperties": false,
      "required": [
        "id",
        "title",
        "subsegments"
      ],
      "properties": {
        "id": {
          "type": "string",
          "pattern": "^[A-K]$"
        },
        "title": {
          "type": "string",
          "minLength": 1
        },
        "layerStack": {
          "$ref": "#/$defs/LayerStack"
        },
        "subsegments": {
          "type": "array",
          "minItems": 1,
          "items": {
            "$ref": "#/$defs/Subsegment"
          }
        }
      }
    },
    "Subsegment": {
      "type": "object",
      "additionalProperties": false,
      "required": [
        "id",
        "title"
      ],
      "properties": {
        "id": {
          "type": "string",
          "pattern": "^[A-K][1-9][0-9]*$"
        },
        "title": {
          "type": "string",
          "minLength": 1
        },
        "assetId": {
          "$ref": "#/$defs/Id"
        },
        "layerStack": {
          "$ref": "#/$defs/LayerStack"
        },
        "autoFollow": {
          "type": "boolean",
          "default": false
        },
        "durationFrames": {
          "type": "integer",
          "minimum": 1
        },
        "audioPolicy": {
          "enum": [
            "clip",
            "bed",
            "mute"
          ],
          "default": "clip"
        }
      }
    },
    "LayerStack": {
      "type": "object",
      "additionalProperties": false,
      "required": [
        "layers"
      ],
      "properties": {
        "mergeMode": {
          "enum": [
            "inherit",
            "replace",
            "merge"
          ],
          "default": "inherit"
        },
        "layers": {
          "type": "array",
          "items": {
            "$ref": "#/$defs/Layer"
          }
        }
      }
    },
    "Layer": {
      "type": "object",
      "additionalProperties": false,
      "required": [
        "id",
        "kind",
        "z"
      ],
      "properties": {
        "id": {
          "$ref": "#/$defs/Id"
        },
        "kind": {
          "enum": [
            "videoLoop",
            "clip",
            "camera",
            "guest",
            "graphic",
            "ticker",
            "clock"
          ]
        },
        "z": {
          "type": "integer",
          "minimum": 0,
          "maximum": 1000
        },
        "visible": {
          "type": "boolean",
          "default": true
        },
        "assetId": {
          "$ref": "#/$defs/Id"
        },
        "cameraId": {
          "type": "string"
        },
        "guestId": {
          "type": "string"
        },
        "templateId": {
          "$ref": "#/$defs/Id"
        },
        "fields": {
          "type": "object",
          "additionalProperties": true
        },
        "loop": {
          "$ref": "#/$defs/LoopMetadata"
        },
        "transform": {
          "$ref": "#/$defs/Transform"
        },
        "opacity": {
          "type": "number",
          "minimum": 0,
          "maximum": 1,
          "default": 1
        },
        "chromaKey": {
          "$ref": "#/$defs/ChromaKey"
        },
        "audio": {
          "$ref": "#/$defs/LayerAudio"
        }
      },
      "allOf": [
        {
          "if": {
            "properties": {
              "kind": {
                "const": "videoLoop"
              }
            }
          },
          "then": {
            "required": [
              "assetId"
            ]
          }
        },
        {
          "if": {
            "properties": {
              "kind": {
                "const": "clip"
              }
            }
          },
          "then": {
            "required": [
              "assetId"
            ]
          }
        },
        {
          "if": {
            "properties": {
              "kind": {
                "const": "camera"
              }
            }
          },
          "then": {
            "required": [
              "cameraId"
            ]
          }
        },
        {
          "if": {
            "properties": {
              "kind": {
                "const": "guest"
              }
            }
          },
          "then": {
            "required": [
              "guestId"
            ]
          }
        },
        {
          "if": {
            "properties": {
              "kind": {
                "const": "graphic"
              }
            }
          },
          "then": {
            "required": [
              "templateId"
            ]
          }
        },
        {
          "if": {
            "properties": {
              "kind": {
                "const": "ticker"
              }
            }
          },
          "then": {
            "required": [
              "templateId"
            ]
          }
        }
      ]
    },
    "Transform": {
      "type": "object",
      "additionalProperties": false,
      "properties": {
        "x": {
          "type": "number",
          "minimum": -2,
          "maximum": 3,
          "default": 0
        },
        "y": {
          "type": "number",
          "minimum": -2,
          "maximum": 3,
          "default": 0
        },
        "w": {
          "type": "number",
          "minimum": 0,
          "maximum": 3,
          "default": 1
        },
        "h": {
          "type": "number",
          "minimum": 0,
          "maximum": 3,
          "default": 1
        },
        "crop": {
          "type": "object",
          "additionalProperties": false,
          "properties": {
            "u": {
              "type": "number",
              "minimum": 0,
              "maximum": 1
            },
            "v": {
              "type": "number",
              "minimum": 0,
              "maximum": 1
            },
            "w": {
              "type": "number",
              "minimum": 0,
              "maximum": 1
            },
            "h": {
              "type": "number",
              "minimum": 0,
              "maximum": 1
            }
          }
        }
      }
    },
    "ChromaKey": {
      "type": "object",
      "additionalProperties": false,
      "required": [
        "enabled"
      ],
      "properties": {
        "enabled": {
          "type": "boolean"
        },
        "color": {
          "enum": [
            "green",
            "blue",
            "custom"
          ],
          "default": "green"
        },
        "customColorHex": {
          "type": "string",
          "pattern": "^#[0-9a-fA-F]{6}$"
        },
        "tolerance": {
          "type": "number",
          "minimum": 0,
          "maximum": 1
        },
        "softness": {
          "type": "number",
          "minimum": 0,
          "maximum": 1
        },
        "spillSuppression": {
          "type": "number",
          "minimum": 0,
          "maximum": 1
        },
        "edgeFeather": {
          "type": "number",
          "minimum": 0,
          "maximum": 1
        },
        "garbageMatte": {
          "$ref": "#/$defs/Transform"
        }
      }
    },
    "LayerAudio": {
      "type": "object",
      "additionalProperties": false,
      "properties": {
        "bus": {
          "enum": [
            "clip",
            "guest",
            "sfx",
            "music",
            "mic"
          ],
          "default": "clip"
        },
        "gainDb": {
          "type": "number",
          "minimum": -60,
          "maximum": 12
        },
        "muted": {
          "type": "boolean",
          "default": false
        }
      }
    },
    "Control": {
      "type": "object",
      "additionalProperties": false,
      "required": [
        "bindings"
      ],
      "properties": {
        "bindings": {
          "type": "array",
          "items": {
            "$ref": "#/$defs/ControlBinding"
          }
        },
        "companion": {
          "type": "object",
          "additionalProperties": true
        }
      }
    },
    "ControlBinding": {
      "type": "object",
      "additionalProperties": false,
      "required": [
        "id",
        "action"
      ],
      "properties": {
        "id": {
          "$ref": "#/$defs/Id"
        },
        "description": {
          "type": "string"
        },
        "trigger": {
          "type": "object",
          "additionalProperties": false,
          "required": [
            "kind"
          ],
          "properties": {
            "kind": {
              "enum": [
                "companionKey",
                "hotkey",
                "midi",
                "webButton",
                "osc"
              ]
            },
            "page": {
              "type": "integer"
            },
            "bank": {
              "type": "integer"
            },
            "key": {
              "type": "string"
            }
          }
        },
        "action": {
          "type": "string",
          "minLength": 1
        },
        "payload": {
          "type": "object",
          "additionalProperties": true
        }
      }
    }
  }
}
```

---

# 13. Command API

All commands use the WebSocket envelope defined earlier.

Common error codes:

| Code | Meaning |
|---|---|
| `E_BAD_PAYLOAD` | payload failed schema validation |
| `E_FORBIDDEN_STATE` | current state does not permit command |
| `E_NOT_FOUND` | referenced item does not exist |
| `E_ASSET_MISSING` | referenced asset is missing |
| `E_DECODE` | decode failure |
| `E_ENGINE` | render engine failure |
| `E_VERSION_CONFLICT` | stale `baseStateVersion` |
| `E_UNSUPPORTED` | feature unsupported in current mode |
| `E_AUTH` | auth/role failure |
| `E_NO_HARDWARE_ENCODER` | hardware encoder unavailable |

## 13.1 Show commands

| Command | Payload schema | Preconditions | State transitions | Failure modes |
|---|---|---|---|---|
| `show.load` | `{ packagePath: string, mode?: "load"|"reload" }` | no live program | show `UNLOADED -> LOADED` | `E_BAD_PAYLOAD`, `E_NOT_FOUND`, `E_ENGINE` |
| `show.preflight` | `{ strict?: boolean }` | show loaded | sets preflight state | `E_PREFLIGHT_FAILED` |
| `show.start` | `{ startClock?: boolean }` | preflight passed | show `LOADED -> RUNNING`, clock `STOPPED -> RUNNING` | `E_FORBIDDEN_STATE` |
| `show.stop` | `{}` | show running | show `RUNNING -> STOPPED` | `E_FORBIDDEN_STATE` |
| `show.unload` | `{}` | not live | show `LOADED/RUNNING -> UNLOADED` | `E_FORBIDDEN_STATE` |

## 13.2 Preview/Program commands

| Command | Payload schema | Preconditions | State transitions | Failure modes |
|---|---|---|---|---|
| `preview.set` | `{ itemRef: string }` | item exists and READY/ARMED | target `READY -> ARMED`; previous preview may return to READY | `E_NOT_FOUND`, `E_ASSET_MISSING` |
| `program.take` | `{ transition?: "cut"|"mix", durationFrames?: integer }` | preview armed | preview item becomes LIVE or PLAYING; previous live becomes READY | `E_FORBIDDEN_STATE` |
| `program.cut` | `{ itemRef: string }` | item exists | immediate program switch to item | `E_NOT_FOUND`, `E_FORBIDDEN_STATE` |
| `program.fallback` | `{ reason?: string }` | always allowed | PROGRAM switches to fallback slate | `E_ENGINE` |

## 13.3 Segment/subsegment commands

| Command | Payload schema | Preconditions | State transitions | Failure modes |
|---|---|---|---|---|
| `segment.arm` | `{ segmentId: "A".."K" }` | segment exists | segment `READY -> ARMED` | `E_NOT_FOUND` |
| `segment.unarm` | `{ segmentId: "A".."K" }` | segment armed | segment `ARMED -> READY` | `E_NOT_FOUND`, `E_FORBIDDEN_STATE` |
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
| `ticker.setSource` | `{ source: "manual"|"rss"|"mixed" }` | ticker layer exists | ticker source changed | `E_NOT_FOUND` |
| `ticker.override` | `{ items: [{ text: string, priority?: integer, ttlSec?: integer }], mode: "replace"|"prepend"|"append" }` | ticker exists | ticker queue changed | `E_BAD_PAYLOAD` |
| `ticker.clearOverride` | `{}` | ticker exists | manual override cleared | `E_NOT_FOUND` |
| `ticker.refreshRss` | `{ feedId?: string }` | RSS configured | RSS cache refreshed | `E_NETWORK`, `E_BAD_PAYLOAD` |

## 13.6 Soundboard/audio commands

| Command | Payload schema | Preconditions | State transitions | Failure modes |
|---|---|---|---|---|
| `soundboard.play` | `{ assetId: string, gainDb?: number }` | asset preloaded | playback started | `E_NOT_FOUND`, `E_AUDIO` |
| `soundboard.stop` | `{ playbackId?: string, assetId?: string }` | playback active or known | playback stopped | `E_NOT_FOUND` |
| `soundboard.stopAll` | `{}` | always | all SFX stopped | none |
| `audio.bus.set` | `{ bus: "mic"|"clip"|"music"|"sfx"|"guest"|"master", gainDb?: number, muted?: boolean }` | bus exists | bus params changed | `E_BAD_PAYLOAD` |
| `audio.duck` | `{ bus: "music", enabled: boolean, depthDb?: number, attackMs?: number, releaseMs?: number }` | duck-capable bus | duck state changed | `E_BAD_PAYLOAD` |
| `guest.mute` | `{ guestId: string, muted: boolean }` | guest exists | guest audio muted/unmuted | `E_NOT_FOUND` |

## 13.7 Guest commands

| Command | Payload schema | Preconditions | State transitions | Failure modes |
|---|---|---|---|---|
| `guest.connect` | `{ guestId: string, whipUrl: string, displayName?: string }` | guest not connected | guest source `READY` | `E_NETWORK`, `E_BAD_PAYLOAD` |
| `guest.disconnect` | `{ guestId: string }` | guest exists | guest source disconnected | `E_NOT_FOUND` |
| `guest.setLayout` | `{ guestId: string, layout: "pip"|"full" }` | guest layer exists | guest transform updated | `E_NOT_FOUND` |
| `guest.placeholder` | `{ guestId: string, assetId?: string }` | guest exists | placeholder set | `E_NOT_FOUND` |

## 13.8 Output commands

| Command | Payload schema | Preconditions | State transitions | Failure modes |
|---|---|---|---|---|
| `record.start` | `{ outputId?: string }` | show running, encoder available | recording active | `E_NO_HARDWARE_ENCODER`, `E_DISK` |
| `record.stop` | `{}` | recording active | recording stopped | `E_FORBIDDEN_STATE` |
| `stream.start` | `{ outputId?: string, url?: string }` | show running, encoder available | stream active | `E_NO_HARDWARE_ENCODER`, `E_NETWORK` |
| `stream.stop` | `{}` | stream active | stream stopped | `E_FORBIDDEN_STATE` |

## 13.9 System commands

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

Example 10 → 30:

```text
source:  S0 S1 S2
output:  S0 S0 S0 S1 S1 S1 S2 S2 S2
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

## 15.4 Unsupported source rates

If source frame rate is not covered by a built-in pattern, the asset MUST provide:

```json
"pulldownPattern": [2, 3, 2, 3]
```

or preflight MUST fail.

## 15.5 Cadence preflight

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
| Role | WebSocket controller, preview monitor |
| Forbidden | live renderer, primary compositor |

---

# 19. Acceptance criteria

Each criterion is independently testable.

## AC-1 — Manifest schema validation

Given a valid show package, `preflight` MUST validate the manifest against the WNBE v0.1 JSON Schema and return exit code 0.

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

Over a 30-minute show, audio/video sync drift MUST remain within ±1 frame on Tier-1 hardware.

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

The same show package MUST be runnable through an OBS baseline adapter. A published comparison MUST report:

1. dropped frames,
2. CPU utilization,
3. GPU utilization,
4. glass-to-glass latency,
5. take latency,
6. recording crash safety.

WNBE MUST meet or exceed OBS baseline for dropped frames and CPU/GPU utilization on Tier-1 hardware.

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

# 22. Open questions

The following are not resolved in v0.1 and should be addressed before v1.0.

1. **25 fps and 29.97 fps cadence policy.** Should v1 support them, or are they forbidden until a later revision?
2. **Smelter compatibility level.** Should WNBE expose a Smelter-compatible scene API subset, or only use Smelter for benchmark concepts?
3. **WHIP authentication.** Token-in-URL, mDNS/ICE credentials, or TURN credential vending?
4. **NDI licensing/distribution.** Is NDI acceptable as a compiled dependency, or should it remain optional?
5. **Fragmented MP4 vs MKV default.** Fragmented MP4 is specified as default, but some players handle MKV more gracefully. Confirm default.
6. **VRAM budget on Apple unified memory.** Should `vramBudgetMib` be treated as a soft unified-memory budget with a separate Metal texture budget?
7. **Channel schema fields.** What minimal fields should be reserved now for future scheduler compatibility?
8. **Multi-language ticker priority.** Should ticker items be grouped by language, or is a single mixed feed acceptable?
9. **Recording ISOs.** Should v1 optionally record isolated clip/audio ISO tracks, or remain master-only?
10. **iPhone preview transport.** Should iPhone preview use WebRTC, low-latency HLS, or MJPEG snapshots for simplest reliability?

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

Every subsystem MUST be testable without requiring all other subsystems to be complete.

The definition of done for any subsystem is:

```text
schema-valid + state-safe + telemetry-visible + acceptance-tested
```
