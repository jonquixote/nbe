# Device audio — the design record (air-readiness queue, item 2)

~~**Status: DRAFT, held for the user's rulings on D1–D8 (§6). Nothing here is
committed, and no build dispatches before the rulings land.**~~ **Status:
RULED 2026-10-21, D1–D8 approved with additions (§6), and the register
filings made (§7)** (§2c). **The build sequence:**
1. **The §8.2 limiter PR first** (D4), whose first commit lands this record;
2. **then the device-audio build** per this record.

Each is DO NOT MERGE until its two-key pass and the user's word. The record
itself is docs only: no production code, no dependency added. Drafted 2026-10-21 by the builder, on
the user's order of 2026-10-21 ("the device-audio design record"), in the
shape of R20's design record (`docs/prompt-map-07-13.md`, "R20's design
record") and spine B's brief (`agents/prompts/r17-spine-b-belief.md`).

- **Verified at:** `main` `056fed0`. Every tree claim carries a `file:line`
  read at that commit. **Re-verified at `2df03a6`** (#64's merge, the landing
  base): no source, schema, spec or agent prompt changed between the two (0
  diff lines). `.github/workflows/ci.yml`'s cited lines were read at #64's
  tree, which `2df03a6` equals, and the one prompt-map line that moved is
  updated (§0.2).
- **Lives in:** `docs/device-audio-design.md`, landed in the limiter PR's
  first commit (the user's ruling of 2026-10-21). The probe's source and
  small run outputs are beside it in `docs/device-audio/probe/`. The 4.1 MB of
  raw CSVs stay in `.git/nbe-staged/device-audio/probe/out/`, referenced by
  sha256 (§8): the evidence-reference pattern, not 4 MB of git history.
- **Targets:** SPEC v0.4.8 §8.1, §8.2, §8.9, §8.10, §10.1, §11.5.1, §12.10,
  §12.11.1, §19.2; the deferral recorded in `agents/prompts/06-audio-graph.md`.

---

## 0. The ruled frame, and two tree facts in tension with it

**The frame, as given:** device audio on macOS, through the compile-time
profile the air-readiness order defined; the silence-only fallback stays for
Linux CI. Air-readiness queue item 2, in the era after residency honesty.

The tree disagrees with two of its premises. The tree wins, so they are
stated here, not assumed:

1. **There is no Linux CI.** All three jobs run on `macos-14`: `rust`
   (`.github/workflows/ci.yml:10`), `control-plane` (`:994`) and
   `dress-rehearsal` (`:1485`). The "silence-only fallback" therefore stands
   today on a macOS runner, not a Linux one. Whether a `macos-14` runner has an
   output device at all is unknown (§5).
2. **No compile-time profile exists yet.** `git grep 'cfg(target_os'` over
   `crates/` at `056fed0` finds nothing. `nbe-decode` depends on the Apple
   frameworks unconditionally (`crates/nbe-decode/Cargo.toml`: `objc2-av-foundation`,
   `objc2-video-toolbox`, `objc2-audio-toolbox` and the rest, none
   target-gated), so the workspace does not build off macOS at all today. The
   phrase "the ALSA story under the compile-time profile" appears only in the
   order itself (`docs/prompt-map-07-13.md:6265` at `2df03a6`; `:6217` at `056fed0`). This record therefore
   proposes the profile (D7), and does not inherit one.

## 1. What the tree does today (verified at `main` `056fed0`)

### 1.1 Decode: what is resident, at what rate and format

- At `show.load` every asset's audio track is decoded and made resident by
  asset id (`crates/nbe-engine/src/directive.rs:325–345`). The comment cites
  §8.4: "a take that decodes cannot meet §7.13".
- The decoder is `nbe_decode::decode_audio` (`crates/nbe-decode/src/lib.rs:593–597`).
  It asks AVFoundation's `AVAssetReader` for 48 kHz, 2-channel, interleaved
  float32 linear PCM (`audio_output_settings`, `:691–730`), so **AVFoundation
  converts both the rate and the channel layout at load**. The constants are
  `ENGINE_SAMPLE_RATE = 48_000` and `ENGINE_CHANNELS = 2` (`:585–586`), with a
  30-minute ceiling (`:591`). No sample-rate converter quality key is set, so
  AVFoundation's default applies (unmeasured; §5).
- **The dress's `A1_clip`:** `audio asset resident … frames=240000` is the log
  line at `directive.rs:332–336`. 240000 frames at 48 kHz is 5.000 s, and
  `tests/fixtures/dress_show/media/A1.mp4` is AAC, 48 kHz, stereo, 5.000 s
  (ffprobe).
- **Every audio fixture in the tree is already 48 kHz** (ffprobe: `A1.mp4`
  and `stab.m4a` stereo, `av_tone.mp4` mono). The load-time conversion has
  never been exercised on a 44.1 kHz or 96 kHz source.
- Resident samples live in `EngineState::audio_assets` (`directive.rs:337`,
  `:346`),
  and the driver copies the map into its library on a new package generation
  (`crates/nbe-engine/src/audio_driver.rs:178–190`).

### 1.2 What reaches a bus

- A take becomes `AudioCommand::TakeItem`, and the item's samples become one
  `Source::Pcm { t0, looping: false }` on the **clip bus**, swapped in
  through silence (`crates/nbe-engine/src/audio_control.rs:207–245`). The
  clip bus holds one item's audio at a time.
- **Which element's audio is the item's:** the first `clip`-kind element on
  the item's scene whose `audio.bus` is `"clip"` and which is not muted
  (`crates/nbe-engine/src/scene.rs:463–483`).
  - A loop element never contributes, because of the kind filter (`:475`).
  - `audioPolicy: "bed"` is warned and treated as `"clip"` (`:466–470`).
  - An element whose `audio.bus` names `music`, `sfx`, `guest` or `mic` is
    never played.
- **`audio.gainDb` is declared and ignored.** It is in the schema
  (`schemas/manifest.v0.4.json:730–734`) and parsed by `nbe-core`
  (`crates/nbe-core/src/manifest.rs:512–519`). The engine's element reads only
  `bus` and `muted` (`scene.rs:766–777`). A take's gain is unity, or −60 dB for
  `mute` (`audio_control.rs:111–140`).
- **Loop assets' audio is decoded and never heard.** The load walk decodes
  every asset (`directive.rs:328–345`), loops included. RAM is spent, and
  §12.11.1's `audioDemandMib` counts it (`docs/spec.v0.4.md:3357–3367`), but
  no path routes a loop's audio to a bus.

### 1.3 The graph

- `AudioGraph::render` (`crates/nbe-engine/src/audio.rs:561–700`) claims to be
  allocation-free and lock-free in the steady state (`:15–16`, `:561–563`).
- It reads every source at `(F − t0) · 48000 / houseRate`: the block's base is
  `sample_for_master_frame` (`:536–539`), and the per-source offset is at
  `:818–819`.
- **The block is anchored to an integer master frame.** `render(out,
  master_frame)` computes `base = master_frame · 48000 / houseRate`
  (`:570`). A buffer can only begin on a frame boundary.
- **The master bus's "limiter" is a hard clipper.** The comment reads "A
  limiter, not a clipper: hard clipping is a click generator"; the code is
  `(out[idx] * g).clamp(-1.0, 1.0)` (`audio.rs:676–677`). §8.2 requires a
  compressor, a limiter and loudness-safe output on the master bus
  (`docs/spec.v0.4.md:1779–1782`). None is built.

### 1.4 The sink abstraction, and the silence fallback

- **The seam:** `AudioSink` (`audio_driver.rs:38–44`). It is *push*:
  `write(&mut self, block: &[f32]) -> bool` and `block_frames()`, and
  `false` counts an underrun (§8.10).
- **The only implementation is `NullSink`** (`:48–81`). It counts blocks and
  frames, with a test seam (`fail_next`) that reaches the underrun path.
- **The production wiring:** `main.rs:47` calls `spawn_with_events`. That
  reaches `spawn_driver` (`audio_driver.rs:418–455`), which builds a
  `NullSink` of `48000 / houseRate` frames, 1600 at 30 fps (`:423–426`), on
  a dedicated OS thread (`:448`). Finding R4 (`:432–445`) explains why it is
  not a tokio task.
- **The cadence:** `run_blocking` (`:459–488`) sleeps to the next block by
  `Instant`, reads `state.master_frame()` (`:472`), renders, and counts a late
  cycle as an underrun (`:484`).
- **`AudioDriver::cycle` is not callback-safe; only `render` is.** It locks
  `record_tap` (`:200`), `stream_tap` (`:213`), `audio_commands` (`:235`),
  `bus_peaks` (`:294`) and `audio_level_watches` (`:309`). §8.9 forbids locks
  in the callback (`docs/spec.v0.4.md:2034–2036`).
- **The deferral this record discharges:** "The `cpal` device sink … opening a
  `cpal` output stream, matching its block size, and handing the callback
  `AudioGraph::render`" (`agents/prompts/06-audio-graph.md:121–136`; the
  module header, `audio_driver.rs:10–12`). Prompt 06 allowed exactly that
  crate: "Allowed: `cpal` for device I/O (CoreAudio underneath on macOS)"
  (`06-audio-graph.md:55`).
- **What today's compile switches:** nothing. There is no `cfg` or feature
  around audio (§0.2). The sink is chosen by construction, always `NullSink`.

### 1.5 The master clock against an audio timestamp

- **The View's clock:** `masterFrame = floor(elapsedSeconds × houseFrameRate)`,
  where `elapsed` is measured from an `Instant` epoch taken at `start()`
  (`crates/nbe-engine/src/clock.rs:2`, `:37–41`, `:50–56`). That is the
  host's monotonic clock, `mach_absolute_time`'s timebase on macOS. It is
  read through `state.master_frame()` (`crates/nbe-engine/src/state.rs:444–446`).
- **The audio side's agreement is by construction, not by measurement.** The
  audio thread reads the master frame once per 33 ms block (`:472`), and
  renders a block whose samples begin at that frame (§1.3). Nothing maps a
  sample to the instant it is *heard*. A `NullSink` has no such instant.
- **`audioDriftMs` measures the render cadence, not a device.** `drift_ms`
  is the rendered samples less `master_frame × 48000 / houseRate`
  (`audio.rs:773–780`), published by `publish` (`audio_driver.rs:273–275`).
  §8.9 defines drift as "the difference between the samples the device has
  consumed and the samples the master clock implies"
  (`docs/spec.v0.4.md:2023–2025`). With no device, the field is the cadence
  loop's slip.
- **The record path is a separate consumer of the same graph:** the
  master-bus record and stream taps (`audio.rs:684–693`). It is not touched
  here.

### 1.6 What the show-file schema says about audio

- **`show.audio`** is `AudioSpec` (`schemas/manifest.v0.4.json:165–166`,
  `:215–239`): `sampleRate` **`const 48000`**, `loudnessTargetLufs`
  (default −16), `truePeakDbtp` (default −1.5) and `defaultLanguage`.
- **An element's `audio`** is `LayerAudio` (`:929–930`, `:716–738`): `bus` (one
  of `clip`, `guest`, `sfx`, `music`, `mic`; default `clip`), **`gainDb`
  (−60 to +12)** and `muted`.
- **An item's `audioPolicy`** is `clip`, `bed` or `mute`, default `clip`
  (`:1344–1350`).
- **A scene's `audio`** is an open object (`additionalProperties: true`,
  `:1148–1151`) that nothing reads.
- **An asset's `loudness`** is a `LoudnessReport` (`:425–427`).
- **No field declares a channel layout.** Layout is forced to stereo at
  decode (§1.1).

### 1.7 What §8, §11.5.1, §12 and §19 promise

- **§8.1:** 48 kHz, float32 internal (`docs/spec.v0.4.md:1744–1750`).
- **§8.2:** a master compressor, limiter and loudness-safe output (`:1779–1782`).
  The tree has a clamp (§1.3).
- **§8.9:**
  - one clock; the callback is a cadence;
  - drift is measured as device-consumed against master-implied, and
    corrected by §11.5.1;
  - the callback MUST NOT allocate, lock, block or do I/O (`:1999–2040`).
- **§8.10:**
  - an underrun is counted, with the first of each episode logged;
  - sustained underruns raise `E_AUDIO` in status;
  - **device loss: `E_AUDIO`, reopen with backoff, never block the render
    loop**;
  - an audio fault MUST NOT raise the video slate (`:2042–2058`).
- **§11.5.1:** A/V within **±1 frame of the master show clock over a 30-minute
  show**, corrected "by adjusting audio presentation or dropping/holding
  non-critical frames"; unbounded drift is forbidden (`:2991–3010`).
- **§12:**
  - §12.8.1 item 6: "Audio is unchanged", decoded whole at load (`:3278–3280`);
  - §12.10 #5: preflight MUST verify "no audio gap if loop has audio" (`:3319`),
    a check for loop audio that the engine never plays (§1.2);
  - §12.11.1: `audioDemandMib` (`:3357–3367`).
- **§19:** `audioDemandMib` in the report (`:4514`, `:4571`); seeded failure
  #8, out-of-range loudness (`:4588`).
- **Nothing in §12 or §19 speaks of an output device**, and §7.15 places
  machine facts outside preflight (§1.9).

### 1.8 The operator's surface today

- The §10.1 tick carries `audioUnderrunsTotal`, `audioDriftMs` and
  `busPeakDbfs` (`crates/nbe-engine/src/telemetry.rs:114–124`). The control
  plane parses them (`packages/control-plane/src/protocol.ts:424–428`) and
  carries them on its own tick (`telemetry.ts:44–46`, `:148–150`).
- The status snapshot (`state.statusSnapshot`, served at `server.ts:550` and
  `:624`) carries none of them.
- `E_AUDIO` is defined (`crates/nbe-protocol/src/lib.rs:82`, `:146`), and no
  engine path raises it.
- The operator-signal lane (`packages/control-plane/src/alerts.ts`) has no
  audio alert.

### 1.9 Preflight's contract, and where machine facts live

- Preflight "validates a package in isolation and cannot know the target
  machine, so it reports and warns" (`crates/nbe-core/src/preflight.rs:34–36`;
  `crates/nbe-preflight/src/main.rs:983–985`).
- §7.15: "**the side that knows both facts is the side that refuses**"
  (`docs/spec.v0.4.md:1738`); §12.11.3 #4 repeats it for resources (`:3385`).

### 1.10 Where FFI may live

The workspace denies `unsafe_code` (`Cargo.toml:31–34`), and CI confines
`allow(unsafe_code)` to `crates/nbe-decode` (`.github/workflows/ci.yml:29–53`).
`nbe-decode` already hosts AudioToolbox FFI for AAC encode, with its
justification written beside the dependency (`crates/nbe-decode/Cargo.toml`).

---

## 2. The probe (throwaway, disclosed; its source lands beside this record, §8)

- **What:** a scratch cargo project outside the repo, `audio_probe`. Its
  source and every raw output are kept in `.git/nbe-staged/device-audio/probe/`.
  It depends on:
  - **cpal 0.18.2** (the current release; `cargo search`, 2026-10-21);
  - **objc2-core-audio 0.3.2**, for the HAL property reads cpal does not
    expose;
  - **`nbe-decode` by path**, so the file goes through the engine's own
    decoder.

  It adds no dependency to the repo, and the repo stayed `git status`-clean
  while it ran. *(§2c, 2026-10-21: its source and small outputs land beside
  this record, as evidence, not as a build target, §8.)*
- **The machine:** MacBookPro15,1, Intel i7-9750H, the normative machine
  (§0.3's floor device, `docs/spec.v0.4.md:4633`). Two output devices were
  used:
  - the default output, **MacBook Pro Speakers**, at a nominal 44.1 kHz;
  - **VB-Cable** (virtual, 48 kHz), for silent loopback.
- **The dependency surface it measured:**
  - cpal 0.18.2 on macOS is **coreaudio-rs 0.14.2** over **objc2-core-audio
    0.3.2** and **objc2-audio-toolbox 0.3.2** (`cargo tree`). That is the
    `objc2` 0.6 / `objc2-*` 0.3.2 family `nbe-decode` already pins
    (`objc2 = "0.6.4"`, `objc2-audio-toolbox = "0.3.2"`).
  - So "cpal or coreaudio-rs" is not either/or: **cpal is a layer over
    coreaudio-rs here.**
  - The new crates would be `cpal`, `coreaudio-rs`, `objc2-core-audio`,
    `dasp_sample` and `mach2`. Their full list is confirmed in the build PR
    by `cargo tree` against the workspace lock.

### 2.1 Device facts (`probe info`, 2026-10-21)

| Device | Nominal | Buffer | Device latency (out) | Safety offset (out) | Stream latency | Sum |
|---|---|---|---|---|---|---|
| MacBook Pro Speakers @ 44.1 kHz | 44100 | 512 | 19 | 77 | 258 | 866 frames = **19.6 ms** |
| MacBook Pro Speakers @ 48 kHz (switched by cpal) | 48000 | 512 | 19 | 77 | 275 | 883 frames = **18.4 ms** |
| VB-Cable | 48000 | 512 | 0 | 0 | 0 | 512 frames = 10.7 ms |

cpal's supported configurations for the speakers are 44.1, 48, 88.2 and
96 kHz, float32, with buffers from 14 to 4096 frames.

### 2.2 A file in, a tone out the default device (`probe tone`)

**The file:** `tests/fixtures/media/av_tone.mp4` (AAC, 48 kHz, mono, 1.000 s).
`nbe_decode::decode_audio` returned 48000 Hz, 2 channels, 48000 frames, at a
peak of −17.8 dBFS. It was played at −12 dB (about −30 dBFS) on the default
device, with the MacBook Pro Microphone listening.

**Run 2** (11:28:58Z, load 2.62 → 2.57) is the record; run 1 (11:27:01Z,
load 7.30) agrees:
- **Callbacks:** 132, at 512 frames each.
- **cpal's latency estimate:** `playback − callback` was 12.67 ms on every
  callback. That is (512 + 19 + 77) / 48000. **cpal leaves out the stream
  latency** (`cpal-0.18.2/src/host/coreaudio/macos/device.rs`,
  `get_device_extra_latency_frames` sums device latency and safety offset
  only), which is 275 frames, 5.7 ms, at 48 kHz.
- **Start-up:** the first callback came 52.6 ms after `play()` (55.4 ms in
  run 1). That includes a rate switch.
- **The tone was heard.** A Goertzel filter at the file's own frequency
  (999.0 Hz, from its zero crossings) put the mic capture's peak 45.8 dB over
  the pre-roll median.
  - The first 10 ms window above +20 dB came **23.8 ms after cpal's
    predicted first audible frame**.
  - That figure holds three things this probe cannot separate: the 5.7 ms
    cpal omits, the microphone's own input path (unmeasured), and the 10 ms
    window. It bounds the error of cpal's estimate from above; it does not
    measure the output path alone.
- **cpal changed the system's device rate, and left it changed.** Asked for
  48 kHz on a 44.1 kHz device, it moved the speakers' nominal rate to 48000
  while the stream was open, and **it stayed at 48000 after the stream
  closed**.
  - This was seen in both tone runs and in the speaker underrun run.
  - It is cpal's own behaviour: `build_output_stream` calls
    `set_physical_format`, falling back to `set_sample_rate`, which writes
    `kAudioDevicePropertyNominalSampleRate`. Nothing restores it.
  - The probe restored 44.1 kHz itself each time (status 0, confirmed by
    `system_profiler`). **Disclosed:** after the speaker underrun run, the
    speakers sat at 48 kHz from about 11:29:40Z until restored by hand at
    about 11:31Z.

### 2.3 Underrun behaviour, named (`probe underrun`)

One callback was deliberately held 40 ms (3.75 buffer periods), with a 1 kHz
sine at −20 dBFS.
- **VB-Cable, with loopback capture** (11:29:12Z):
  - **cpal's error callback fired once:** `A buffer underrun or overrun
    occurred.` That is `StreamError::Xrun`, which cpal raises from a
    `kAudioDeviceProcessorOverload` listener
    (`cpal-0.18.2/src/host/coreaudio/macos/mod.rs:100–110`, `:183–185`).
  - The HAL's next callback came **51.08 ms (4.79 periods) later**. The
    missed cycles were skipped, not stretched: device time moved on.
  - The loopback captured **one 512-frame run of digital silence** at the
    stall, and no other.
  - The loopback input also reported an overrun.
- **The control, no stall** (11:29:34Z): no error callback, a step of 1.00
  period, and no silent run.
- **The speakers, timestamps only** (no capture; same stall): one Xrun error
  callback, and a HAL step of 52.00 ms (4.88 periods). This run also switched
  the speakers to 48 kHz (§2.2's disclosure).
- **So, on this machine:**
  - a late callback is heard as a hole of silence, about one buffer;
  - the device timeline jumps forward;
  - the stream continues with no reopen;
  - cpal reports it through the error callback;
  - nothing in the samples written reports it.

### 2.4 Drift: the device's clock against the host's (`probe drift`)

Two 30-minute runs of silence ran concurrently, 11:30:08–12:00:15Z, on AC
power. Each device ran at its own nominal rate with no rate change; the
speakers read 44.1 kHz afterwards (`system_profiler`). Load was 2.00 at the
start, decaying from the builder's probe build. The HAL fit uses host
timestamps and is insensitive to load; the callback-arrival fit
cross-checks it.

| Device | Callbacks | Frames | Rate vs HAL host time | vs callback arrival | `ActualSampleRate` (HAL, 1 Hz polls) | Uncorrected skew at 1 / 10 / 30 min |
|---|---|---|---|---|---|---|
| MacBook Pro Speakers @ 44.1 kHz | 155084 | 79403008 | 44100.5627 Hz, **+12.76 ppm** (residual rms 2.2 frames) | +12.75 ppm (2.8) | mean +12.68 ppm (44099.63–44100.99, 1792 polls) | **+0.77 / +7.66 / +22.97 ms** |
| VB-Cable @ 48 kHz (control) | 168809 | 86430208 | 48000.0000 Hz, −0.00 ppm (0.0) | −0.01 ppm (2.2) | +0.00 ppm | 0.00 / 0.00 / 0.00 ms |

- **The speakers' crystal runs 12.76 ppm fast against the host clock.**
  Three independent methods agree within 0.08 ppm.
- **The sign matters:** the device consumes faster than the master clock
  implies. So contiguous playback runs *ahead* of the picture: +23 ms after
  30 minutes.
- **Against §11.5.1:**
  - 23 ms is inside ±1 frame at 30 fps (33.3 ms);
  - it is outside ±1 frame at 60 fps (16.7 ms), §1.1 #3's showcase rate;
  - it passes 33.3 ms at about 43.5 minutes at 30 fps, and it is unbounded
    beyond (about 92 ms at two hours).

  Correction is therefore required, not optional.
- **VB-Cable is clocked from the host** (0.00 ppm, a zero HAL residual). It
  validates the method's resolution. It also means **a VB-Cable loopback
  cannot measure device drift:** the proving case's drift leg must run on a
  hardware device.
- **Disclosed:** during the window the builder also ran a `prompt04` mutant
  (11:41Z) and #64's bookend reads. The HAL fit's residual stayed at 2.2
  frames.

---

## 3. The design questions

Each question gives the options, a recommendation, and the evidence (a tree
citation or a probe measurement). The rulings themselves are §6.

### D1. The backend

- **Options:**
  1. **cpal 0.18.2** (over coreaudio-rs 0.14.2 on macOS).
  2. **coreaudio-rs 0.14.2 directly** (AUHAL).
  3. raw `objc2-core-audio` / AudioToolbox, the way `nbe-decode` does
     encode.
- **The evidence:**
  - Prompt 06 named cpal (`06-audio-graph.md:55`).
  - cpal's API is safe, so the sink lives in `nbe-engine` with no exception
    to the unsafe rule (§1.10).
  - Both crates share `nbe-decode`'s `objc2` family (§2).
  - The probe proved cpal end to end on the normative machine: a file through
    the engine's own decoder, out the default device, heard (§2.2). Its
    underrun is visible as `Xrun` (§2.3), and its latency is estimable (§2.2).
  - **cpal's two costs, both measured:**
    - it rewrites the device's nominal rate and leaves it (§2.2);
    - its presentation estimate omits stream latency, 5.7 ms here (§2.2).

    coreaudio-rs exposes the full `AudioTimeStamp` (`mSampleTime`,
    `mRateScalar`) and lets the AUHAL's converter bridge 48 kHz to any device
    rate without touching the device. But its property reads need `unsafe`,
    so they would have to live in `nbe-decode`, and it is macOS-only by
    construction.
- **Recommendation: cpal 0.18.2**, with two guards in the sink:
  - **(a) the rate is the engine's explicit act, not cpal's side effect.**
    At open, the sink reads the device's nominal rate. If it is 48 kHz it
    opens at 48 kHz, and nothing changes. If not, it sets 48 kHz itself
    before opening, logs both rates, and restores the prior rate at close.
    A device that cannot do 48 kHz is refused (`E_AUDIO`, §8.10), and the
    sink falls back to silence (D2/D3).
  - **(b) presentation time includes stream latency.** It is read once at
    open, through a small property helper in `nbe-decode`, which is where the
    `unsafe` lives, as for encode. That helper is the one `unsafe` addition
    this design needs.
- **Ruled 2026-10-21: approved.** Both guards are required, falsifiable
  behaviours. The unsafe helper is the change's only `unsafe`, documented with
  its Apple API contract.
  - **That contract:** `AudioObjectGetPropertyData` with a valid
    `AudioObjectPropertyAddress`, a data size equal to the property's type,
    and a pointer valid for that size. The `OSStatus` is checked, never
    assumed.
  - **The crash-path limit, named:** the rate restore runs on the sink's
    close: drop, `show.unload`, or orderly shutdown. **A crash, a `SIGKILL`,
    or a panic that aborts leaves the device at 48 kHz**, because no restore
    can run. The device stays at 48 kHz until the next engine start, or until
    someone changes it in Audio MIDI Setup. The open's log line names the
    prior rate, so an operator can restore it by hand.
  - Restoring a crashed run's prior rate at the next start (a small file
    holding it) is a candidate, not part of this ruling.

### D2. The clock: A/V skew

The View holds master (§1.5); the device runs its own crystal; they drift
(§2.4).

**The architectural fork:** the device *pulls* (a callback), and the seam
*pushes* (§1.4).
- **Option A, push to a ring (recommended).**
  - The existing audio thread keeps rendering 33 ms blocks on the
    master-clock cadence. That is the one clock, unchanged, and every lock in
    `cycle` stays off the real-time thread.
  - `AudioSink::write` becomes a push into a lock-free single-producer
    single-consumer ring of interleaved float32, preallocated at open.
  - The device callback only copies out of the ring. It does not touch the
    graph, so it is real-time-safe by construction (§8.9).
- **Option B, pull the graph in the callback.**
  - The callback calls the graph at the master sample position implied by
    the buffer's presentation time. That is the cleaner clock mapping, and
    the lower latency.
  - It needs every lock in `cycle` moved to lock-free transport (§1.4):
    commands, taps, meters and watches. It also needs `render` taught
    sub-frame starts (§1.3).
  - It is a larger change to a working subsystem, and it is the natural
    sequel, not this round.

**Measuring drift under option A** (§8.9's definition, made true):
- The callback counts the samples the device consumed (an atomic).
- The audio thread compares that count with the samples the master clock
  implies at the buffer's presentation time: the callback's host time, plus
  the HAL latency including stream latency (D1 b).
- The ring's fill is the same quantity, seen from the other side.
- `audioDriftMs` is redefined to that device-referenced figure. With a null
  sink it stays the render-cadence figure it is today (§1.5).

**The correction strategy:**
- **Recommended: a bounded slip.** When the device-referenced skew exceeds a
  threshold, the audio thread renders one block one sample short or long (a
  drop or a duplicate). The 1600 master samples are re-spaced across 1599 or 1601
  output samples by 4-point cubic interpolation (a few lines, no crate), so
  the slip is spread over 33 ms with no step in the waveform, and linear
  interpolation's high-frequency dip is avoided.
  - No resampler crate. Nothing runs on the real-time thread.
  - Its cost is one re-spaced block per correction.
- **Considered and not recommended this round:**
  - **Continuous drift resampling** (a variable-ratio sinc resampler, rubato
    5.0.1 being the current crate): smooth, but CPU and latency on every
    block, and an **unprobed dependency** (the R20 lesson).
  - **Slaving the master clock to the device:** it violates §8.9's one clock
    and the ruled frame ("the View holds master").

**The measured drift rate:**
- **+12.76 ppm**: the speakers at 44.1 kHz over 30 minutes, three methods
  within 0.08 ppm (§2.4).
- **Uncorrected skew:** +0.77 ms at 1 minute, +7.66 ms at 10, +22.97 ms
  at 30.
- **What the slip would do at that rate:** fire about every 1.6 s at 48 kHz
  (12.76 × 10⁻⁶ × 48000 = 0.61 samples per second). Each slip is a 1600 → 1601
  re-spacing of one block: a momentary 625 ppm shift, 0.011 semitones, for
  33 ms.
- **Not measured:** the 48 kHz family's rate on this device (§5).

**The skew bound the build guarantees** (proposed for ruling):
- The device-referenced skew against the master clock is held within
  **±1 ms**: the slip threshold, plus at most one sample of re-spacing.
- The output latency is compensated using the HAL's full figure, so the
  residual is the HAL's own estimate error. §2.2's acoustic run bounds that
  from above at 23.8 ms, which includes an unmeasured input path; the build's
  proving case measures it properly.
- Against §11.5.1's ±1 frame (33.3 ms at 30 fps) that leaves the margin for
  the View's own presentation latency, which is unmeasured (§5).

**The proving case** (the build PR's, normative machine), to the user's
specification:
- a sync-tone/flash clip on air for 30 minutes;
- skew in ms at 1, 10 and 30 minutes, measured two ways:
  - **(i) inside the engine:** the presentation host time of the tone-onset
    sample (the callback's host time plus its offset and the HAL latency)
    against the master time of the flash frame;
  - **(ii) end to end:** VB-Cable loopback (the probe's method, silent) for
    the audio onset, against the frame's presentation time;
- each bounded by the guarantee above;
- an optional acoustic/optical check by phone camera, recorded as advisory.

**Ruled 2026-10-21: approved.**
- The architecture is master-clock render → lock-free buffer → device
  copy-out.
- The correction is ±1 sample per 33 ms block past the threshold.
- The guarantee is ±1 ms.
- The proof is at 1, 10 and 30 minutes, both engine-internal and on the real
  device.

**The ruling's addition: the proving case carries a correction-audibility
measure.** The 999 Hz tone (`av_tone.mp4`, decoded, looped) is played
through correction events, and the discontinuity is bounded and named:
- **The quantity:** the residual of the device-side signal against a
  least-squares fitted 999 Hz sine, over each re-spaced block and over its
  neighbours, reported in dB relative to the tone.
- **Where it is measured:** at the copy-out, which is deterministic; and on
  the normative machine through VB-Cable loopback, where the device is
  host-clocked, so the corrections are forced by an injected ppm.
- **The bound** is proposed in the build PR from the measurement, against
  the unaltered blocks' residual as the floor. The name for what is left is
  "the slip residual".

### D3. Resampling

**Where the house rate comes from:** fixed, not configured. `AudioSpec.sampleRate`
is `const 48000` (`manifest.v0.4.json:224–226`); §8.1 says 48 kHz MUST;
the graph is at `audio.rs:20` and the decoder at `nbe-decode/src/lib.rs:585`.

**The clip's rate to the house rate** already happens at load, by
AVFoundation's converter (§1.1). It is unmeasured in quality and unexercised
on a non-48 kHz source.
- **Recommendation:**
  - keep it at load, where §8.9 #4 puts every expensive operation;
  - **pin its quality explicitly** (`AVSampleRateConverterAudioQualityKey` in
    the output settings, in `nbe-decode`), rather than inherit a default;
  - **add a 44.1 kHz fixture**, so the conversion is exercised: a falsifiable
    pitch and length check (a 1 kHz tone stays 1 kHz, and its duration in
    frames scales by 48000/44100).

**The house rate to the device's rate:** no resampler on the air path. The
device runs at 48 kHz by the sink's explicit act (D1 a). The speakers
support 48 kHz (§2.1), and cpal switched them anyway (§2.2). The only
real-time rate change is D2's ±1-sample slip.
- **Considered:**
  - a real-time 48 → 44.1 kHz converter (rubato 5.0.1, unprobed), whose
    quality/latency trade is about 1–3 ms of filter delay for sinc quality,
    on every block, on the real-time path or just before it;
  - the AUHAL's own converter via coreaudio-rs (D1's option 2): free, but it
    means leaving cpal.

  Neither is recommended while 48 kHz on the device is available.

### D4. Mixing and gain

**Today:** the clip bus holds one item's audio. The first unmuted `clip`
element with `bus: "clip"` plays. Loops are silent, other buses' elements are
silent, and `gainDb` is ignored (§1.2). The master clamp is a hard clipper
(§1.3).

**What happens when two audible items overlap** (a clip under a loop with
audio): today only the clip is heard. The loop's audio is decoded, held in
RAM, charged in `audioDemandMib`, and checked by §12.10 #5 for gaps, but it is
never played (§1.2, §1.7).

**Options:**
1. **Status quo, made explicit.** One audio source per take. §8 says so in
   words, `gainDb` is honoured on that one element, and §12.10 #5 and
   `audioDemandMib` stop charging loop audio no path plays.
2. **Per-element sources (recommended for the schema's sake, scoped).**
   - Every unmuted, audio-bearing element of the on-air scene feeds the bus
     its `audio.bus` names, at its `gainDb`.
   - Loops loop their audio, which `Source::Pcm { looping: true }` already
     supports (`audio.rs:226–230`, `:826–827`).
   - Overlap sums on the bus.
   - This makes `LayerAudio` mean what the schema says. It also makes the
     clipper audible sooner: two sources at −6 dBFS peak already sum to 0.

**Recommendation:** option 2, **gated on the master limiter**, so a true
limiter (§8.2) lands in the same build or before it. Option 1 is the fallback
if the limiter is ruled out of this era (D8).

**Ruled 2026-10-21: approved, and the limiter goes first.**
- **The true limiter is built first, as its own PR within this item.**
  §8.2's MUST is faked today by `.clamp` at `audio.rs:676–677`: a live lie
  (R26, §7).
- **Then** every audible element plays its declared bus at its `gainDb`,
  loops included (U2, U3).

**What the schema must gain:** nothing. `gainDb`, `muted` and `bus` exist.
**What the SPEC must say:**
- §8: a "what is heard" paragraph: which elements feed which bus, at what
  gain, and a loop's audio wrap;
- §12.10 #5 stays (it becomes live, not dead);
- §19: no change. No device field belongs in the report (D6).

### D5. The operator's surface this round

**Options:**
1. **Nothing new.** The counters already reach the tick (§1.8).
2. **Levels on the status snapshot.** `busPeakDbfs` copied from the
   telemetry cache, read-only.
3. **An alert in the lane's style:** `audio.underrun`, raised on §8.10's
   "sustained underruns" (a stated count in a stated window), cleared by a
   quiet window, with `E_AUDIO` in status. A sibling, `audio.device`, covers
   device absent or lost (§8.10's third row).

**Recommendation:** 3, plus a device line on the tick (open or absent or lost;
name; rate) so an operator and a log reader can see which device carried
air. Levels on the snapshot (2) wait for the operator shell, which owns
display.
- The alert's thresholds are proposed in the build PR and ruled there.
- Everything stays out of the video watchdog (§8.10's last paragraph).

### D6. Air-readiness: is device presence preflight's business?

**No, by the tree's own contract.**
- Preflight validates a package in isolation and cannot know the machine
  (§1.9).
- §7.15's principle, "the side that knows both facts is the side that
  refuses", puts a device check with the engine, which knows the device, and
  the control plane, which knows the show.

**Recommendation:**
- the engine probes the default output device at start, as §10.5 probes the
  adapter;
- it reports the result on the tick (D5);
- at `show.load`, the control plane raises `audio.device` when no device is
  open. It does not refuse the load: a show can go to air silent, and
  refusing on audio would trade a small fault for a large one, §8.10's
  reasoning.

A refusal policy ("audio required") is a later, separate ruling, if ever.

### D7. The CI surface and the compile-time profile

**What CI can honestly exercise** (unit level, no device, which is the dress
precedent and Prompt 06's no-hardware policy, `06-audio-graph.md:115`):
- the ring: underrun on empty, counting, no allocation on the pop path (a
  counting allocator around the copy-out);
- the slip: a simulated device clock at ±N ppm drives the correction, and
  skew stays inside the bound for a simulated 30 minutes, run faster than
  real time;
- the device-referenced drift figure;
- the sink-selection fallback;
- the rate guard's decision table: already 48 kHz, set and restore, refused.

**What is normative-machine-only:**
- opening a real device;
- the tone heard (VB-Cable loopback, silent);
- the Xrun path (a forced stall, as §2.3);
- the 30-minute skew (D2's proving case);
- the rate restore.

Each is SKIPped on `GITHUB_ACTIONS=true` with a `SKIP normative:` line, by
the dress's fail-closed rule.

**The compile-time profile (proposed; none exists, §0.2):**
- in `nbe-engine`,
  `[target.'cfg(target_os = "macos")'.dependencies] cpal = "0.18.2"`,
  with the device sink behind `#[cfg(target_os = "macos")]`;
- everywhere else, the `NullSink` silence fallback;
- **no ALSA:** cpal on Linux would build `alsa-sys` and need
  `libasound2-dev`, and with the target gate a Linux build never sees it.
  The probe's own `Cargo.lock` shows the surface: it resolves `alsa` and
  `alsa-sys` (cpal's Linux backend) for every platform
  (`docs/device-audio/probe/Cargo.lock`). `cargo metadata --offline` on it
  fails until they are fetched, unless filtered to macOS.

**Runtime selection on macOS:**
- `NBE_AUDIO_SINK=device|null`, default `device`;
- a failed open falls back to `null`, with a logged reason and `audio.device`
  (D6);
- **CI sets `null` explicitly** for the dress and the suites, so CI's audio
  stays deterministic whether or not a runner happens to have a device. One
  observational CI step lists the runner's devices, to answer §5's unknown.

### D8. Scope of this era

**In (proposed):**
- program audio (the master bus) to the **default output device** on macOS,
  at 48 kHz;
- the ring (D2 A);
- the device-referenced drift figure and the bounded slip (D2);
- underruns from Xrun and from an empty ring;
- the rate guard and the stream-latency read (D1);
- the device line and the two alerts (D5, D6);
- D4's per-element sources **if** the limiter is ruled in;
- the 44.1 kHz fixture and the pinned converter quality (D3);
- this record landed as `docs/device-audio-design.md`, and a SPEC row
  carrying §8's "what is heard", §8.9's drift definition made true, and
  §8.10's device rows as built.

**Out (said explicitly):**
- monitoring, PFL and solo to a second device;
- IFB and guest-return devices (§8.6);
- mic input and voice-detected ducking;
- per-item fades beyond the transition machinery (§8.7);
- device selection by name;
- hot-plug follow beyond §8.10's reopen with backoff;
- option B's pull graph (D2);
- loudness normalisation against `loudnessTargetLufs`;
- Linux.

**The judgement calls:**
- **§8.10's "reopen with backoff" is a MUST.** If it is ruled out of this era,
  it must be filed in the unbuilt-MUST register
  (`docs/prompt-map-07-13.md`, "The unbuilt-MUST register"), not dropped.
- **The limiter is a MUST** (§8.2). Shipping device audio over a hard clipper
  is shipping a known click generator to air.

**Ruled 2026-10-21: approved.**
- Default-device program audio, this era.
- **§8.10's device-loss reopen goes to the unbuilt-MUST register (U5, §7).**
  D5's `audio.device` alert is the interim surface.
- **§8.2's limiter is built now**, first (D4).

---

## 4. The build PR, sketched (for the rulings, not for dispatch)

**Its first commit lands this record**, plus whatever post-merge records are
staged at that time.

**Proving cases:**
- **D1:** the tone heard on the normative machine. A VB-Cable loopback test
  plays the decoded `av_tone.mp4` through the device sink and finds its
  999 Hz onset in the capture.
- **D1 (a):** the device rate is restored after close, observed through the
  HAL.
- **D2:** the 1/10/30-minute skew, both ways (§3, D2), and the simulated-ppm
  unit test on CI.
- **§8.10:** a forced stall counts exactly one underrun episode and logs once.
  Device loss counts and reopens, if ruled in.
- **D3:** the 44.1 kHz fixture's pitch and length.
- **D4:** a clip under a looping element. Both are heard, each at its
  `gainDb`, and the sum is limited, not clipped. Peaks are read on the bus.

**Falsifications** (§2a), each against its guard:
- the callback made to allocate: the counting allocator catches it;
- the slip disabled: the skew test exceeds its bound at 30 simulated minutes;
- the stream latency dropped from the presentation time: the in-engine skew
  test is off by the 5.7 ms the probe measured;
- the rate guard removed: the restore test sees cpal's left-behind 48 kHz;
- the Xrun counting removed: the stall test counts zero.

**Mutant classes:**

| Mutant | What it does | The test that kills it |
|---|---|---|
| M-ring | the callback renders the graph instead of copying out (locks reach the RT thread) | the no-lock/no-alloc RT guard |
| M-slip-sign | the correction slips the wrong way | the simulated-ppm skew test (skew grows) |
| M-slip-off | no correction | the simulated-ppm skew test (a 30-minute bound) |
| M-latency | the stream latency is omitted | the in-engine skew test (normative) |
| M-rate | the prior rate is not restored at close | the rate-restore test (normative) |
| M-xrun | Xrun is not counted | the forced-stall test (normative) |
| M-empty | an empty ring yields stale samples, not silence | the ring unit test (silence and a count) |
| M-gain | `gainDb` is ignored (today's tree) | the D4 bus-peak test |
| M-clip | the limiter is replaced by `clamp` (today's tree) | the D4 sum test (the peak exceeds the limiter ceiling with no overshoot recovery; or a THD bound) |
| M-fallback | a failed open panics, not silence | the selection-fallback unit test |

---

## 5. What this record does NOT know

- **The View's presentation latency.** The skew budget's other half, from
  render submit to photons, is unmeasured. The D2 proving case is where it
  is first measured.
- **The HAL's estimate error, isolated.** §2.2's 23.8 ms bound includes the
  microphone's input path. Only a loopback or optical measurement separates
  them.
- **AVFoundation's default conversion quality**, and its behaviour on a
  non-48 kHz source (§1.1).
- **Whether a GitHub `macos-14` runner has an output device**, and what cpal
  does there (D7's observational step answers it).
- **The speakers' drift at 48 kHz.** The 48 and 44.1 kHz families may be
  derived differently. Only 44.1 kHz was measured (§2.4).
- **Drift on any device but these two, on any day but this one.** A
  different interface, another day's temperature, or AC versus battery can
  each move the crystal (§2.4 is one 30-minute window).
- **rubato 5.0.1's real-time safety and cost**, because it is unprobed. It
  is not recommended here for that reason.
- **The cost of switching the device rate mid-session for other
  applications** (for example, a browser playing at 44.1 kHz). It is
  observed only as "the switch succeeded".
- **A Bluetooth or USB device's latency figures**, or behaviour on unplug.
  Only the built-in speakers and a virtual device were probed.

---

## 6. ~~Rulings owed (the user's, before any code)~~ Rulings of 2026-10-21 (the user's word; §2c)

The proposals in §3 stand as the analysis. Where a ruling adds to them, the
ruling stands. Each ruling is quoted, followed by what it requires.

- **D1: approved.** "cpal 0.18.2; both guards are required falsifiable
  behaviors (rate restore on close, with the crash-path limit named in the
  record; latency includes stream latency). The unsafe helper is the change's
  only unsafe, documented with its Apple API contract."
  - *Requires:* the rate guard and the stream-latency read, each with a test
    that fails without it. The crash-path limit is named in D1. The helper in
    `nbe-decode` documents `AudioObjectGetPropertyData`'s contract.
- **D2: approved.** "master-clock render → lock-free buffer → device
  copy-out; ±1 sample per 33 ms block past threshold; guarantee ±1 ms; proof
  at 1/10/30 min, engine-internal and real-device. Addition: the proving case
  carries a correction-audibility measure (the 999 Hz tone through correction
  events, discontinuity bounded and named)."
  - *Requires:* D2's architecture and slip; the 1/10/30-minute proof, both
    ways; the slip residual measured and bounded (D2's addition).
- **D3: approved.** "no live-path resampler; pin the load-time converter's
  quality; add the 44.1 kHz fixture."
- **D4: approved.** "the true limiter is built FIRST, as its own PR within
  this item (§8.2's MUST is currently faked by .clamp at audio.rs:676–677 — a
  live lie); then every audible element plays its declared bus at its gainDb,
  loops included."
- **D5: approved.** "audio.underrun and audio.device alerts, E_AUDIO raised,
  device line on the tick."
- **D6: approved.** "preflight stays package-only; engine probes and reports;
  control plane alerts at load, never refuses."
- **D7: approved.** "cpal macOS-only, silent sink elsewhere, no ALSA,
  NBE_AUDIO_SINK=null; CI unit-level, device tests normative-only."
- **D8: scope approved.** "default-device program audio this era. §8.10's
  device-loss reopen → unbuilt-MUST register, D5's alert the interim surface.
  §8.2's limiter → built now."

<details><summary>The rulings list as it was put to the user (superseded, kept for the record)</summary>


**D1, the backend.** cpal 0.18.2 (over coreaudio-rs 0.14.2), with two guards:
(a) the sink sets 48 kHz itself and restores the prior rate, or refuses a
device without 48 kHz; (b) the presentation time includes stream latency, read
through one small property helper in `nbe-decode`, which is the one `unsafe`
addition. Or coreaudio-rs directly. Or another backend.

**D2, the clock.**
- (i) The architecture: option A (push to a lock-free ring; the callback
  copies out) this era, with option B (pull) as the sequel.
- (ii) The correction: a bounded slip, one sample re-spaced across a 33 ms
  block, with no resampler.
- (iii) The guarantee: device-referenced skew within ±1 ms of the master
  clock. The 1/10/30-minute proving case measures it in the engine and by
  loopback.
- (iv) `audioDriftMs` is redefined as device-referenced when a device is open.

**D3, resampling.** No real-time resampler. The clip's rate converts at load,
by AVFoundation, with its quality key pinned and a 44.1 kHz fixture added.
The device runs at 48 kHz by D1 (a). Or a real-time converter (crate to be
probed first).

**D4, mixing and gain.** Option 2: per-element sources on their declared
buses at `gainDb`, loops looping, gated on a true master limiter. Or option 1:
one source per take, made explicit, `gainDb` honoured, loop audio struck from
the demand. And the SPEC text: §8's "what is heard".

**D5, the operator's surface.** `audio.underrun` and `audio.device` alerts in
the lane's style, with `E_AUDIO` in status, and a device line on the tick.
Levels on the snapshot wait. Or nothing new this round.

**D6, air-readiness.** Not preflight's. The engine probes and reports; the
control plane alerts at `show.load` and does not refuse.

**D7, the CI surface and the profile.**
- (i) The CI surface: unit-level ring, slip, drift, fallback and rate-guard
  tests on CI; the device, tone, Xrun, skew and rate-restore tests
  normative-only (`SKIP normative:`).
- (ii) The profile: cpal as a macOS target dependency, `NullSink` elsewhere,
  no ALSA.
- (iii) Selection: `NBE_AUDIO_SINK`, `null` set explicitly on CI, plus one
  observational device-listing step.

**D8, scope.** In and out as §3 D8 lists. And two judgement calls:
- whether §8.10's reopen with backoff is built now or registered as an
  unbuilt MUST;
- whether the §8.2 limiter is built in this era, which D4's option 2
  depends on.

Also: where this record lands (`docs/device-audio-design.md` is proposed).

</details>

## 7. The register filings (the user's ruling of 2026-10-21)

The next free numbers were R26–R27 (the flake register,
`docs/soak-protocol.md` §5) and U2–U5 (the unbuilt-MUST register,
`docs/prompt-map-07-13.md`), as the user numbered them. U5 is §8.10's reopen
(D8).

- **R26:** the master "limiter" is a hard clipper. `.clamp(-1.0, 1.0)` at
  `audio.rs:676–677` sits beside its comment ("A limiter, not a clipper") and
  §8.2's MUST.
  - Its test, `the_master_stage_limits_instead_of_clipping`
    (`crates/nbe-engine/tests/prompt06.rs:1091–1140`), asserts only `peak <=
    1.0`, which the clamp satisfies, so the test's name repeats the lie.
  - *Disposition:* fixed by the limiter PR.
- **R27:** `audioDriftMs` measures the render loop's timing, not device
  consumption (`audio.rs:773–780`, against §8.9's definition at
  `docs/spec.v0.4.md:2023–2025`).
  - *Disposition:* fixed under D2's architecture.
- **U2:** `LayerAudio.gainDb` is in the schema (`manifest.v0.4.json:730–734`)
  and parsed (`nbe-core/src/manifest.rs:516`), and the engine ignores it
  (`scene.rs:766–777`).
  - *Disposition:* built this era (D4).
- **U3:** loop audio is decoded, held, and charged (`directive.rs:328–345`;
  §12.11.1's `audioDemandMib`; §12.10 #5), and never played (`scene.rs:475`).
  - *Disposition:* built this era (D4).
- **U4:** `E_AUDIO` is defined (`nbe-protocol/src/lib.rs:82`, `:146`) and never
  raised.
  - *Disposition:* built this era (D5).
- **U5:** §8.10's "Device loss: `E_AUDIO`; attempt reopen with backoff; never
  block the render loop" (`docs/spec.v0.4.md:2053`). There is no device sink
  to lose today; the device build opens one and does not reopen it.
  - *What stands in:* D5's `audio.device` alert, the interim surface.
  - *Owner:* a later device-audio PR, by the user's ruling D8.

## 8. The evidence, by reference

**In the tree** (`docs/device-audio/probe/`, beside this record):
- the probe's `src/main.rs`, `Cargo.toml` and `Cargo.lock`;
- every run's text output (`out/*.txt`).

The `Cargo.toml` is made self-contained for the tree: it gains an empty
`[workspace]` table, so it is not taken for a member of the repo's workspace,
and its `nbe-decode` path becomes relative. The probe ran with the absolute
path `/Users/johnny/Code/nbe/crates/nbe-decode`, the same crate. It is not a
workspace member and not built by CI.

`src/main.rs` is the probe's **final** revision. Two runs used earlier
revisions of the same file:
- tone run 1 (§2.2) predates the Goertzel check;
- the underrun runs (§2.3) predate the `setrate` subcommand.

Nothing else changed between revisions. Tone run 2 and the drift runs used
this revision.

**Out of the tree** (`.git/nbe-staged/device-audio/probe/out/`, repo-local),
by sha256:

| File | Bytes | sha256 (gzip) | sha256 (decompressed) | Lines |
|---|---|---|---|---|
| `drift-speakers.csv.gz` | 1864618 | `e74c8f701e5f267498769db54b692790e322518b621e2a1fb56d9181adaaa453` | `c062e0c429f502278bd9fd06af46a736a5fe209f6b90615de6ea5a5e317af9eb` | 155085 |
| `drift-vbcable.csv.gz` | 1547280 | `6954f286957673a871a92666f8e8da9e66d26faf56db28791755192dfe39b3a2` | `30a11cdafa39a0280b984c8dd62e73d79ecec319d99ccb3c4182b9c8dff0c03f` | 168810 |
| `mic-capture.csv.gz` | 732900 | `3ac11b662f764d5e5e5f696da7d76de911e8b72469ec57fcfbecc45557e71da9` | `df815ddec72ef2acd9151afe1647694e7ef97e0a2764f2e5bce4f951000f3706` | 78849 |

**The columns:**
- the drift CSVs: `arrive_ns,hal_ns,frames`, one row per callback;
- the mic capture: `t_ns,x`, from §2.2's run 2.
