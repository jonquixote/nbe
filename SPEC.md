# NBE SPEC v0.2  
**Worker News Broadcast Engine**  
Status: handoff-ready revision  
Relationship to SPEC v0.1: this document amends v0.2-relevant defects and rulings. All v0.1 sections not explicitly amended remain normative. Where v0.2 conflicts with v0.1, v0.2 wins.

---

## 0. v0.2 revision summary

### 0.1 Blocking defects fixed

| Defect | v0.2 fix |
|---|---|
| Loop VRAM budget was internally contradictory | Section 11 is replaced. Cache texture formats are now explicit. Frame caps are computed from texture format and budget. NV12, RGBA8, NV12+alpha, and optional BC7 costs are defined. |
| No mix-minus / guest return path | A per-guest `guestReturn` mix-minus bus is now mandatory. An anchor `ifb` monitor bus is defined. WebRTC outbound audio is specified. |
| Guest sources contradicted master-clock drift rule | Master-clock drift is now split into local-source and remote-guest rules. Guest sources are internally synced to their own ingest timeline, not the show master clock. Guest video uses hold-latest-complete-frame. |
| Audio behavior during transitions undefined | Audio transitions are now normative. All cuts use click-free ramps. `mix` transitions crossfade audio by default. `audioPolicy` behavior is defined. |

### 0.2 Should-fix items incorporated

| Item | v0.2 change |
|---|---|
| Segment ID regex too narrow | Segment IDs are now `^[A-Z]{1,2}$`. Subsegments are `^[A-Z]{1,2}[1-9][0-9]*$`. A–K remains convention, not schema law. |
| Missing error codes | `E_NETWORK`, `E_PREFLIGHT_FAILED`, `E_AUDIO`, `E_DISK`, `E_TIMEOUT`, `E_TURN`, `E_ICE`, and `E_UNSUPPORTED_FEATURE` are added. |
| No normative take latency | Take latency MUST be ≤ 2 frames end-to-end on localhost for accepted commands. |
| `show.stop` output behavior unspecified | `show.stop` now quiesces recording/streaming automatically unless explicitly forbidden by payload. |
| `asset.loop` vs `layer.loop` precedence undefined | `layer.loop` overrides `asset.loop`. Preflight validates the effective loop. |
| `clock` layer config missing | A dedicated `clock` object is added to `Layer`. |
| AC-11 CPU/GPU wording ambiguous | AC-11 now says NBE must be “no worse than” OBS baseline. |

### 0.3 Open-question rulings incorporated

| # | Ruling |
|---:|---|
| 1 | 25 fps and 29.97 fps are forbidden by default. They are allowed only with explicit pulldown metadata. 25→30 duplicates every fifth frame. 29.97→30 requires one held frame per approximately 1000 source frames, i.e. about one hold per 33 seconds. |
| 2 | Smelter is benchmark concepts only. NBE MUST NOT chase Smelter API compatibility. |
| 3 | WHIP ingest uses bearer-token authentication in HTTP headers. TURN credentials are vended by the control plane. |
| 4 | NDI is optional and feature-flagged. The core build MUST remain NDI-free unless the feature is explicitly enabled. |
| 5 | Fragmented MP4 remains the default crash-safe recording container. |
| 6 | `vramBudgetMib` is a soft budget and MUST be capped by Metal `recommendedMaxWorkingSetSize` on Apple platforms. |
| 7 | Channel schema fields remain minimal: `id`, `name`, open `futureScheduler`. No additional fields are reserved. |
| 8 | Ticker is a single mixed feed with priority flags. Per-language lanes are future Channel-era features. |
| 9 | Recording is master-only for v1. An `isolation` schema hook is added but not implemented. |
| 10 | iPhone preview uses WHEP. MJPEG is only a dev-mode fallback. |

---

# 1. Amended normative sections

---

## 1.1 Error code registry

The following error codes are normative in v0.2.

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
| `E_TIMEOUT` | Operation timed out. |
| `E_TURN` | TURN credential vending failure. |
| `E_ICE` | WebRTC ICE failure. |

Any command table referencing these codes is now valid.

---

## 1.2 Take latency

A normative take-latency target is added.

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

This applies to the local PROGRAM compositor output, not to downstream stream latency.

---

## 1.3 `show.stop` output quiescence

`show.stop` is amended.

Default behavior:

```text
show.stop automatically quiesces outputs.
```

When `show.stop` is received:

1. If recording is active, the engine MUST issue an internal `record.stop`.
2. If streaming is active, the engine MUST issue an internal `stream.stop`.
3. The engine MUST wait up to 2 seconds for graceful output shutdown.
4. The show clock then transitions to `STOPPED`.
5. If graceful shutdown exceeds 2 seconds, the engine MUST force-stop outputs and log a warning.

Recording remains crash-safe because fragmented MP4 or MKV fragments are already written.

Payload:

```json
{
  "quiesceOutputs": true,
  "force": false
}
```

Behavior:

| `quiesceOutputs` | `force` | Active outputs | Result |
|---:|---:|---|---|
| true | false | yes | graceful automatic stop |
| true | true | yes | immediate stop, warning logged |
| false | false | yes | fail with `E_FORBIDDEN_STATE` |
| false | true | yes | immediate stop |
| any | any | no | stop show |

Default:

```json
{
  "quiesceOutputs": true,
  "force": false
}
```

---

## 1.4 Guest ingest, mix-minus, and IFB

Section 7.5 is expanded.

### 1.4.1 Guest return requirement

For every connected guest, the engine MUST create a return audio mix.

Guest return MUST be mix-minus:

```text
guestReturn(guestId) = programReturnMix - that guest's own inbound audio
```

The guest MUST NOT receive their own voice from the NBE return path.

This applies even if the guest is muted in program.

### 1.4.2 Default guest return mix

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

### 1.4.3 Guest return transport

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

### 1.4.4 Anchor IFB

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

### 1.4.5 Echo prevention

The engine MUST guarantee that a guest’s own audio does not enter their own return path.

If a guest is connected through WHIP/WebRTC:

1. inbound guest audio enters `guestBus(guestId)`,
2. `guestBus(guestId)` may enter program mix,
3. `guestBus(guestId)` MUST NOT enter `guestReturn(guestId)`.

A failure of this rule is an `E_AUDIO` fault.

---

## 1.5 Guest clock carve-out and frame-selection policy

Section 10.5 is amended.

### 1.5.1 Local deterministic sources

For local sources, the original rule remains:

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

### 1.5.2 Remote guest sources

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

### 1.5.3 Guest video frame-selection policy

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

### 1.5.4 Guest audio jitter buffer

Guest audio jitter buffer remains:

| Condition | Target |
|---|---:|
| good network | 200 ms |
| variable network | 300–500 ms |
| hard maximum | 1000 ms |

Guest video SHOULD sync to guest audio presentation time, not to master clock.

---

## 1.6 Audio behavior during transitions

A new Section 7.6 is added.

### 1.6.1 Click-free rule

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

### 1.6.2 `program.take` audio behavior

`program.take` payload now includes an optional `audio` object.

```json
{
  "transition": "mix",
  "durationFrames": 15,
  "audio": {
    "transition": "follow",
    "durationFrames": 15,
    "rampMs": 10
  }
}
```

Fields:

| Field | Default | Meaning |
|---|---:|---|
| `audio.transition` | `follow` | Audio transition policy. |
| `audio.durationFrames` | video `durationFrames` for `mix`; 1 for `cut` | Audio crossfade duration. |
| `audio.rampMs` | 10 | Ramp for hard cuts/stops. |

### 1.6.3 Audio transition modes

| Mode | Behavior |
|---|---|
| `follow` | Follow subsegment `audioPolicy`. |
| `crossfade` | Crossfade outgoing and incoming audio over `audio.durationFrames`. |
| `cut` | Cut audio at boundary, but apply click-free ramp. |
| `mute` | Incoming audio muted; outgoing audio ramped out. |

### 1.6.4 Interaction with `audioPolicy`

When `audio.transition = follow`:

| Subsegment `audioPolicy` | Behavior |
|---|---|
| `clip` | Clip audio is active and crossfaded or ramped according to transition. |
| `bed` | Clip audio is muted; music bed continues. |
| `mute` | Incoming clip audio is muted; outgoing audio ramped out. |

### 1.6.5 Video `mix` default

If video transition is:

```json
{
  "transition": "mix",
  "durationFrames": 15
}
```

and no audio override is given, audio MUST crossfade over the same 15 frames.

Crossfade curve:

```text
equal-power crossfade
```

Linear crossfade is allowed, but equal-power is recommended.

### 1.6.6 Video `cut` default

If video transition is:

```json
{
  "transition": "cut"
}
```

audio MUST follow `audioPolicy` and MUST apply at least a 5 ms ramp at any start/stop boundary.

### 1.6.7 Live camera and guest audio

For live camera and guest sources:

| Transition | Audio behavior |
|---|---|
| `cut` | 10 ms ramp by default |
| `mix` | crossfade over transition duration |
| `mute` | ramp out and mute |

---

## 1.7 Loop cache budget and texture formats

Section 11.3 and 11.4 are replaced.

### 1.7.1 Cache texture formats

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

### 1.7.2 Budgets

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

### 1.7.3 Frame budget formula

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

### 1.7.4 Example caps at 1080p with 256 MiB per loop

| Format | Max frames | Approx duration at 30 fps |
|---|---:|---:|
| NV12 opaque | 86 | 2.87 s |
| RGBA8 alpha | 32 | 1.07 s |
| NV12 + alpha | 51 | 1.70 s |
| BC7 | 129 | 4.30 s |

The old 900-frame allowance is dead code for full-screen RGBA loops and MUST NOT be interpreted as sufficient.

### 1.7.5 Apple unified-memory rule

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

### 1.7.6 Loop metadata precedence

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

### 1.7.7 Loop preflight additions

Preflight MUST report:

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

## 1.8 Cadence extensions for 25 fps and 29.97 fps

Section 15 is amended.

### 1.8.1 Default policy

25 fps and 29.97 fps assets are forbidden by default.

They are allowed only if the asset contains explicit pulldown metadata.

### 1.8.2 25 fps to 30 fps

25→30 uses:

```text
duplicate every fifth source frame
```

Pattern over five source frames:

```text
source:  S0 S1 S2 S3 S4
output:  S0 S1 S2 S3 S4 S4
```

Machine representation:

```json
{
  "pulldown": {
    "mode": "repeatNthSourceFrame",
    "n": 5
  }
}
```

### 1.8.3 29.97 fps to 30 fps

29.97→30 requires adding one held frame per approximately 1000 source frames.

```text
source frames 0..999 produce output frames 0..1000
one source frame is held once per 1000-source-frame cycle
```

Machine representation:

```json
{
  "pulldown": {
    "mode": "repeatOnePerNSourceFrames",
    "n": 1000
  }
}
```

The exact frame to repeat MUST be documented in the asset pipeline and verified by preflight.

### 1.8.4 Explicit pattern fallback

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

---

## 1.9 Ticker language and priority

Ticker behavior is amended.

The ticker is a single mixed feed.

Ticker items MAY include:

```json
{
  "text": "...",
  "language": "en",
  "priority": 100,
  "ttlSec": 600
}
```

Ordering rules:

1. Breaking override items appear first.
2. Higher `priority` appears before lower `priority`.
3. For equal priority, insertion order is preserved.
4. `language` is metadata only in v1.

Per-language ticker lanes are not implemented in v1.

---

## 1.10 WHIP auth, TURN vending, NDI feature flag, WHEP preview

### 1.10.1 WHIP authentication

WHIP ingest endpoints MUST require bearer authentication.

Example:

```http
POST /nbe/v0.1/whip/guest/GUEST_ID
Authorization: Bearer <guest-token>
```

If token is missing or invalid, the endpoint MUST return HTTP 401.

### 1.10.2 TURN credential vending

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

### 1.10.3 NDI feature flag

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

### 1.10.4 WHEP preview for iPhone

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

# 2. Amended command API

The following commands are added or replaced.

---

## 2.1 Amended `program.take`

| Command | Payload schema | Preconditions | State transitions | Failure modes |
|---|---|---|---|---|
| `program.take` | see below | preview armed | preview item becomes LIVE or PLAYING; previous live becomes READY; audio transition executes | `E_FORBIDDEN_STATE`, `E_AUDIO`, `E_ENGINE` |

Payload schema:

```json
{
  "transition": {
    "enum": ["cut", "mix"]
  },
  "durationFrames": {
    "type": "integer",
    "minimum": 0,
    "maximum": 120
  },
  "audio": {
    "type": "object",
    "additionalProperties": false,
    "properties": {
      "transition": {
        "enum": ["follow", "crossfade", "cut", "mute"]
      },
      "durationFrames": {
        "type": "integer",
        "minimum": 1,
        "maximum": 120
      },
      "rampMs": {
        "type": "number",
        "minimum": 5,
        "maximum": 50
      }
    }
  }
}
```

Defaults:

```json
{
  "transition": "cut",
  "audio": {
    "transition": "follow",
    "rampMs": 10
  }
}
```

If `transition == "mix"` and `audio.durationFrames` is absent, audio crossfade duration MUST equal video `durationFrames`.

---

## 2.2 Amended `show.stop`

| Command | Payload schema | Preconditions | State transitions | Failure modes |
|---|---|---|---|---|
| `show.stop` | see below | show running unless force | show `RUNNING -> STOPPED`; outputs quiesced | `E_FORBIDDEN_STATE`, `E_DISK`, `E_NETWORK` |

Payload schema:

```json
{
  "quiesceOutputs": {
    "type": "boolean",
    "default": true
  },
  "force": {
    "type": "boolean",
    "default": false
  }
}
```

Behavior is defined in Section 1.3.

---

## 2.3 New `guest.configureReturn`

| Command | Payload schema | Preconditions | State transitions | Failure modes |
|---|---|---|---|---|
| `guest.configureReturn` | see below | guest exists | guest return bus updated | `E_NOT_FOUND`, `E_AUDIO`, `E_BAD_PAYLOAD` |

Payload schema:

```json
{
  "guestId": {
    "type": "string"
  },
  "mode": {
    "enum": ["programMinusSelf", "producerMix", "mute"]
  },
  "includeOtherGuests": {
    "type": "boolean",
    "default": true
  },
  "gainDb": {
    "type": "number",
    "minimum": -60,
    "maximum": 12
  },
  "muted": {
    "type": "boolean"
  }
}
```

Required:

```json
["guestId"]
```

Default mode:

```json
"programMinusSelf"
```

---

## 2.4 New `guest.getTurn`

| Command | Payload schema | Preconditions | State transitions | Failure modes |
|---|---|---|---|---|
| `guest.getTurn` | see below | control plane TURN vending enabled | none; returns credentials | `E_TURN`, `E_AUTH`, `E_NOT_FOUND` |

Payload schema:

```json
{
  "guestId": {
    "type": "string"
  },
  "ttlSec": {
    "type": "integer",
    "minimum": 30,
    "maximum": 86400,
    "default": 600
  }
}
```

Response data schema:

```json
{
  "uris": {
    "type": "array",
    "items": { "type": "string" }
  },
  "username": {
    "type": "string"
  },
  "credential": {
    "type": "string"
  },
  "ttlSec": {
    "type": "integer"
  }
}
```

---

## 2.5 Amended `audio.bus.set`

| Command | Payload schema | Preconditions | State transitions | Failure modes |
|---|---|---|---|---|
| `audio.bus.set` | see below | bus exists | bus params changed | `E_BAD_PAYLOAD`, `E_AUDIO`, `E_NOT_FOUND` |

Payload schema:

```json
{
  "bus": {
    "enum": [
      "mic",
      "clip",
      "music",
      "sfx",
      "guest",
      "master",
      "guestReturn",
      "ifb"
    ]
  },
  "guestId": {
    "type": "string"
  },
  "gainDb": {
    "type": "number",
    "minimum": -60,
    "maximum": 12
  },
  "muted": {
    "type": "boolean"
  }
}
```

If `bus == "guestReturn"`, `guestId` is REQUIRED.

If `bus != "guestReturn"`, `guestId` MUST be ignored.

---

## 2.6 New `clock.configure`

| Command | Payload schema | Preconditions | State transitions | Failure modes |
|---|---|---|---|---|
| `clock.configure` | see below | clock layer exists | clock layer config updated | `E_NOT_FOUND`, `E_BAD_PAYLOAD` |

Payload schema:

```json
{
  "layerId": {
    "type": "string"
  },
  "clock": {
    "type": "object",
    "additionalProperties": false,
    "properties": {
      "mode": {
        "enum": ["wall", "showElapsed"]
      },
      "timezone": {
        "type": "string"
      },
      "format": {
        "enum": ["HH:mm", "HH:mm:ss", "hh:mm A", "locale"]
      },
      "locale": {
        "type": "string"
      },
      "blinkColon": {
        "type": "boolean"
      }
    }
  }
}
```

Required:

```json
["layerId"]
```

---

## 2.7 Amended `ticker.override`

| Command | Payload schema | Preconditions | State transitions | Failure modes |
|---|---|---|---|---|
| `ticker.override` | see below | ticker exists | ticker queue updated | `E_BAD_PAYLOAD`, `E_NOT_FOUND` |

Payload schema:

```json
{
  "items": {
    "type": "array",
    "items": {
      "type": "object",
      "additionalProperties": false,
      "required": ["text"],
      "properties": {
        "text": {
          "type": "string",
          "minLength": 1
        },
        "language": {
          "type": "string"
        },
        "priority": {
          "type": "integer",
          "minimum": 0,
          "maximum": 100000,
          "default": 0
        },
        "ttlSec": {
          "type": "integer",
          "minimum": 1
        }
      }
    }
  },
  "mode": {
    "enum": ["replace", "prepend", "append"],
    "default": "replace"
  }
}
```

---

# 3. Manifest schema amendments

Apply the following amendments to the v0.1 JSON Schema. These amendments are normative.

---

## 3.1 Segment and subsegment IDs

Replace:

```json
"Segment": {
  "properties": {
    "id": {
      "type": "string",
      "pattern": "^[A-K]$"
    }
  }
}
```

with:

```json
"Segment": {
  "properties": {
    "id": {
      "type": "string",
      "pattern": "^[A-Z]{1,2}$"
    }
  }
}
```

Replace:

```json
"Subsegment": {
  "properties": {
    "id": {
      "type": "string",
      "pattern": "^[A-K][1-9][0-9]*$"
    }
  }
}
```

with:

```json
"Subsegment": {
  "properties": {
    "id": {
      "type": "string",
      "pattern": "^[A-Z]{1,2}[1-9][0-9]*$"
    }
  }
}
```

A–K remains the editorial convention. The schema no longer enforces an 11-segment ceiling.

---

## 3.2 Add `Pulldown` definition

Add:

```json
{
  "Pulldown": {
    "oneOf": [
      {
        "type": "object",
        "additionalProperties": false,
        "required": ["mode", "pattern"],
        "properties": {
          "mode": {
            "const": "pattern"
          },
          "pattern": {
            "type": "array",
            "minItems": 1,
            "items": {
              "type": "integer",
              "minimum": 1
            }
          }
        }
      },
      {
        "type": "object",
        "additionalProperties": false,
        "required": ["mode", "n"],
        "properties": {
          "mode": {
            "const": "repeatNthSourceFrame"
          },
          "n": {
            "type": "integer",
            "minimum": 2
          }
        }
      },
      {
        "type": "object",
        "additionalProperties": false,
        "required": ["mode", "n"],
        "properties": {
          "mode": {
            "const": "repeatOnePerNSourceFrames"
          },
          "n": {
            "type": "integer",
            "minimum": 2
          }
        }
      }
    ]
  }
}
```

Add to `Asset.properties`:

```json
{
  "pulldown": {
    "$ref": "#/$defs/Pulldown"
  }
}
```

`pulldownPattern` from v0.1 remains deprecated but MAY be accepted for compatibility. `pulldown` wins if both are present.

---

## 3.3 Add `ClockConfig` definition

Add:

```json
{
  "ClockConfig": {
    "type": "object",
    "additionalProperties": false,
    "properties": {
      "mode": {
        "enum": ["wall", "showElapsed"],
        "default": "wall"
      },
      "timezone": {
        "type": "string",
        "default": "local"
      },
      "format": {
        "enum": ["HH:mm", "HH:mm:ss", "hh:mm A", "locale"],
        "default": "HH:mm:ss"
      },
      "locale": {
        "type": "string"
      },
      "blinkColon": {
        "type": "boolean",
        "default": false
      }
    }
  }
}
```

Add to `Layer.properties`:

```json
{
  "clock": {
    "$ref": "#/$defs/ClockConfig"
  }
}
```

For `kind: "clock"`, `clock` is RECOMMENDED. If absent, defaults apply.

---

## 3.4 Add loop texture format

Amend `LoopMetadata.properties`:

```json
{
  "textureFormat": {
    "enum": ["auto", "nv12", "rgba8", "nv12Alpha", "bc7"],
    "default": "auto"
  }
}
```

Effective loop resolution order:

```text
layer.loop.textureFormat
asset.loop.textureFormat
engine default
```

---

## 3.5 Add isolation schema hook

Amend `OutputDefaults.record.properties`:

```json
{
  "isolation": {
    "type": "object",
    "additionalProperties": true,
    "properties": {
      "enabled": {
        "type": "boolean",
        "default": false
      },
      "tracks": {
        "type": "array",
        "items": {
          "type": "string"
        }
      }
    }
  }
}
```

v1 MUST NOT implement ISO recording. The hook is reserved.

---

## 3.6 Add preview output hook

Amend `OutputDefaults.properties`:

```json
{
  "preview": {
    "type": "object",
    "additionalProperties": false,
    "properties": {
      "enabled": {
        "type": "boolean",
        "default": true
      },
      "protocol": {
        "enum": ["whep", "mjpeg"],
        "default": "whep"
      },
      "path": {
        "type": "string"
      }
    }
  }
}
```

---

## 3.7 Add top-level feature flags

Amend top-level `properties`:

```json
{
  "features": {
    "type": "object",
    "additionalProperties": false,
    "properties": {
      "ndi": {
        "type": "object",
        "additionalProperties": false,
        "properties": {
          "enabled": {
            "type": "boolean",
            "default": false
          }
        }
      }
    }
  }
}
```

`features` is optional.

---

# 4. Amended acceptance criteria

The following acceptance criteria are added or amended.

---

## AC-8 amended — master-clock drift

For local deterministic sources:

```text
audio/video sync drift MUST remain within ±1 frame of the master show clock over a 30-minute show on Tier-1 hardware.
```

For remote guest sources:

```text
guest audio/video sync MUST remain within ±1 frame relative to the guest ingest timeline.
```

Guest offset relative to master clock is not a failure condition.

---

## AC-11 amended — OBS baseline comparison

The same show package MUST be runnable through an OBS baseline adapter.

A published comparison MUST report:

1. dropped frames,
2. CPU utilization,
3. GPU utilization,
4. glass-to-glass latency,
5. take latency,
6. recording crash safety.

NBE MUST be no worse than OBS baseline for dropped frames, CPU utilization, and GPU utilization on Tier-1 hardware.

---

## AC-17 — normative take latency

On localhost, an accepted `program.take` command MUST change the local PROGRAM output within:

```text
2 frames
```

For `mix`, the first mixed frame MUST appear by the next frame after acceptance.

---

## AC-18 — mix-minus isolation

With a guest source replaced by a -20 dBFS 1 kHz test tone and no other program sources active, the corresponding `guestReturn` bus MUST measure the tone at or below:

```text
-80 dBFS
```

This verifies that the guest does not receive their own audio.

---

## AC-19 — audio transition click-free behavior

During any `program.take`, `subsegment.stop`, `soundboard.stop`, or bus mute/unmute:

1. gain changes MUST have ramps ≥ 5 ms,
2. no hard sample-step cut is allowed,
3. recorded master output MUST contain no click impulse exceeding -60 dBFS in a silent test pass.

---

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

---

## AC-21 — loop budget math

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

---

## AC-22 — 25/29.97 explicit pulldown

Given a 25 fps asset with no pulldown metadata, preflight MUST fail.

Given a 29.97 fps asset with no pulldown metadata, preflight MUST fail.

Given valid explicit pulldown metadata, preflight MUST verify the resulting 30 fps CFR output and pass only if the declared pattern is present.

---

# 5. Resolved open questions

The v0.1 open questions are closed with the following rulings.

| # | Question | v0.2 ruling |
|---:|---|---|
| 1 | 25/29.97 fps policy | Allowed only with explicit pulldown metadata. 25→30 duplicates every fifth frame. 29.97→30 adds one hold per ~1000 source frames. |
| 2 | Smelter API compatibility | Benchmark concepts only. No API compatibility requirement. |
| 3 | WHIP auth | Bearer token in headers. TURN credentials vended by control plane. |
| 4 | NDI dependency | Optional, feature-flagged. Core build remains NDI-free. |
| 5 | fMP4 vs MKV default | Fragmented MP4 confirmed as default. |
| 6 | VRAM on unified memory | `vramBudgetMib` is soft and capped by Metal `recommendedMaxWorkingSetSize`. |
| 7 | Channel schema fields | Existing minimal fields are sufficient. No additional reservation. |
| 8 | Ticker languages | Single mixed feed with priority flags. Language is metadata only in v1. |
| 9 | ISO recording | Master-only in v1. Add `isolation` schema hook now. |
| 10 | iPhone preview transport | WHEP is primary. MJPEG is dev-mode fallback only. |

---

# 6. Implementation handoff notes for v0.2

The v0.1 implementation order remains correct:

1. Manifest schema validator and preflight skeleton.
2. Control-plane WebSocket server and state machine.
3. Render-node command bridge.
4. Basic program/preview compositor.
5. Video decode integration.
6. Audio graph.
7. Ticker and lower-third template renderer.
8. Companion command mapping.
9. Fragmented MP4 recording.
10. RTMP/SRT output with hardware encoder.
11. Telemetry and fallback slate.
12. OBS baseline benchmark harness.

v0.2 adds these implementation requirements to that sequence:

- Implement loop cache format accounting before VRAM caching.
- Implement guest return/mix-minus at the same time as WebRTC guest ingest.
- Implement audio ramps before the first live TAKE test.
- Implement WHEP preview after the WebRTC stack exists.
- Implement TURN vending before remote guest testing.
- Add 25/29.97 pulldown tests to preflight CI.

Definition of done for v0.2 subsystems:

```text
schema-valid
state-safe
telemetry-visible
acceptance-tested
mix-minus-safe
click-free
loop-budget-accounted
```
