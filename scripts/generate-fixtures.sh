#!/usr/bin/env bash
# Generate the decode fixtures Prompt 05 tests against (SPEC §19.3 seeded
# failures, AC-3 VFR detection, AC-4 cadence, AC-9 loop wrap).
#
# The outputs are tiny (a few KiB each) and committed, so CI does not need
# ffmpeg. Re-run this only when a fixture's definition changes.
#
#   ./scripts/generate-fixtures.sh
#
# Clips are 640x360 — the smallest the manifest schema allows for a house
# format — so a fixture manifest built around them is schema-valid and the
# check under test is the only thing that can fail. `wrong_res` is the
# deliberate exception. Frame content is a flat colour per frame index so a decoded frame
# can be identified by its pixels alone.
set -euo pipefail

command -v ffmpeg >/dev/null || { echo "ffmpeg is required"; exit 1; }

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out="$root/tests/fixtures/media"
mkdir -p "$out"

say() { printf '  %s\n' "$*"; }

# ---------------------------------------------------------------------------
# 1. Valid CFR clip: 30 fps, 30 frames, H.264. The reference for frame-exact
#    decode — frame N is a known colour.
# ---------------------------------------------------------------------------
say "cfr_30.mp4 (valid: 640x360, 30 fps CFR, 30 frames)"
ffmpeg -y -loglevel error \
  -f lavfi -i "color=c=black:s=640x360:r=30:d=1" \
  -vf "geq=r='if(lt(N,10),255,0)':g='if(gte(N,10)*lt(N,20),255,0)':b='if(gte(N,20),255,0)'" \
  -c:v libx264 -pix_fmt yuv420p -profile:v high -g 10 -frames:v 30 \
  "$out/cfr_30.mp4"

# ---------------------------------------------------------------------------
# 2. VFR clip: must be rejected by preflight (AC-3).
# ---------------------------------------------------------------------------
say "vfr.mp4 (seeded failure: variable frame rate)"
ffmpeg -y -loglevel error \
  -f lavfi -i "testsrc=s=640x360:r=30:d=1" \
  -vf "select='not(mod(n,3))',setpts='N/(10*TB)+0.03*sin(N)'" \
  -fps_mode vfr -c:v libx264 -pix_fmt yuv420p \
  "$out/vfr.mp4"

# ---------------------------------------------------------------------------
# 3. Wrong resolution: must be rejected against a 128x72 house spec.
# ---------------------------------------------------------------------------
say "wrong_res.mp4 (seeded failure: 320x180 against a 640x360 house format)"
ffmpeg -y -loglevel error \
  -f lavfi -i "color=c=blue:s=320x180:r=30:d=1" \
  -c:v libx264 -pix_fmt yuv420p -frames:v 30 \
  "$out/wrong_res.mp4"

# ---------------------------------------------------------------------------
# 4. 12 fps source for cadence preservation (AC-4): normalized to 30 it must
#    present the 2,3,2,3 hold pattern.
# ---------------------------------------------------------------------------
say "cadence_12.mp4 (640x360, 12 fps source, 12 frames)"
ffmpeg -y -loglevel error \
  -f lavfi -i "color=c=black:s=640x360:r=12:d=1" \
  -vf "geq=r='(N*20)':g='0':b='0'" \
  -c:v libx264 -pix_fmt yuv420p -frames:v 12 \
  "$out/cadence_12.mp4"

# ---------------------------------------------------------------------------
# 5. A short loop for AC-9: 10 frames, each a distinct colour, so a wrap is
#    verifiable by pixels.
# ---------------------------------------------------------------------------
say "loop_10.mp4 (10-frame loop, distinct colour per frame)"
ffmpeg -y -loglevel error \
  -f lavfi -i "color=c=black:s=640x360:r=30:d=1" \
  -vf "geq=r='N*25':g='128':b='255-N*25'" \
  -c:v libx264 -pix_fmt yuv420p -g 5 -frames:v 10 \
  "$out/loop_10.mp4"

# ---------------------------------------------------------------------------
# 6a. Audio+video: a 1 kHz tone alongside picture, for the audio decode path.
# ---------------------------------------------------------------------------
say "av_tone.mp4 (640x360 video + 1 kHz stereo tone, 1 s)"
ffmpeg -y -loglevel error \
  -f lavfi -i "color=c=black:s=640x360:r=30:d=1" \
  -f lavfi -i "sine=frequency=1000:duration=1:sample_rate=48000" \
  -c:v libx264 -pix_fmt yuv420p -c:a aac -b:a 128k -shortest \
  "$out/av_tone.mp4"

# ---------------------------------------------------------------------------
# 6. A corrupt file that claims to be video: decode must fail loudly, and at
#    runtime that failure is a fault (itemEvent: decodeError), not a scope
#    boundary.
# ---------------------------------------------------------------------------
say "corrupt.mp4 (seeded failure: truncated)"
head -c 512 "$out/cfr_30.mp4" > "$out/corrupt.mp4"

# ---------------------------------------------------------------------------
# 7. The v0.3 show fixture's clip. This one is NOT a placeholder: preflight
#    decodes it, so it must be real media matching the fixture manifest's
#    house format (1920x1080, 30 fps) and its declared expectedDurationFrames.
#    Flat colour keeps it a few KiB despite the resolution.
# ---------------------------------------------------------------------------
show="$root/tests/fixtures/valid_show_v0.3/media"
say "valid_show_v0.3/media/A1.mp4 (1920x1080, 30 fps, 900 frames)"
ffmpeg -y -loglevel error \
  -f lavfi -i "color=c=0x102030:s=1920x1080:r=30:d=30" \
  -c:v libx264 -pix_fmt yuv420p -preset veryfast -tune stillimage -g 30 -frames:v 900 \
  "$show/A1.mp4"

# ---------------------------------------------------------------------------
# 8. The dress-rehearsal package ([RI-1]). This one is played end to end by a
#    real control plane and a real engine binary, so every asset must be real
#    media at the declared house format (1920x1080, 30 fps) — a placeholder
#    would fail preflight at step 1 of the show, before the rehearsal starts.
#
#    Flat colour and stillimage tuning keep these a few tens of KiB despite the
#    resolution; the rehearsal asserts on wire telemetry, not on pixels.
# ---------------------------------------------------------------------------
dress="$root/tests/fixtures/dress_show/media"
mkdir -p "$dress"

# A1: the take target. It carries a real AAC track at a healthy level, because
# the rehearsal proves `busPeakDbfs.clip` rises after a take with audio
# "follow" — a silent clip would make that assertion unfalsifiable.
say "dress_show/media/A1.mp4 (1920x1080, 30 fps, 150 frames, H.264 + AAC)"
ffmpeg -y -loglevel error \
  -f lavfi -i "color=c=0x1b3a5c:s=1920x1080:r=30:d=5" \
  -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=5" \
  -c:v libx264 -pix_fmt yuv420p -preset veryfast -tune stillimage -g 30 \
  -c:a aac -b:a 128k -ar 48000 -ac 2 -frames:v 150 -shortest \
  "$dress/A1.mp4"

# A2: the 10-frame loop the rehearsal mixes to. periodFrames is declared in the
# manifest; AC-9 wrap is already covered by the engine suite, so what this
# proves here is that a mix across it drops no frames.
say "dress_show/media/A2.mp4 (1920x1080, 30 fps, 10 frames, videoLoop)"
ffmpeg -y -loglevel error \
  -f lavfi -i "color=c=0x5c1b3a:s=1920x1080:r=30:d=1" \
  -c:v libx264 -pix_fmt yuv420p -preset veryfast -tune stillimage -g 10 -frames:v 10 \
  "$dress/A2.mp4"

# The soundboard stab: short, loud, and its own asset, so `busPeakDbfs.sfx`
# rising is attributable to `soundboard.play` and nothing else.
say "dress_show/media/stab.m4a (48 kHz stereo, 0.4 s)"
ffmpeg -y -loglevel error \
  -f lavfi -i "sine=frequency=880:sample_rate=48000:duration=0.4" \
  -c:a aac -b:a 128k -ar 48000 -ac 2 \
  "$dress/stab.m4a"

# A3: a 12 fps source at a 30 fps house rate. Without a non-house-rate clip in
# this package the rehearsal is blind to cadence forever — AC-4 was reported as
# delivered for six prompts while `draw_for` mapped house frames 1:1 and no
# end-to-end test could see it. 12 source frames must span 30 house frames.
say "dress_show/media/A3_12fps.mp4 (1920x1080, 12 fps source, 12 frames)"
ffmpeg -y -loglevel error \
  -f lavfi -i "color=c=0x3a5c1b:s=1920x1080:r=12:d=1" \
  -c:v libx264 -pix_fmt yuv420p -preset veryfast -tune stillimage -g 12 -frames:v 12 \
  "$dress/A3_12fps.mp4"

# A real PNG fallback slate at house resolution — §10.3's watchdog target.
say "dress_show/media/fallback.png (1920x1080)"
ffmpeg -y -loglevel error \
  -f lavfi -i "color=c=0x800000:s=1920x1080" -frames:v 1 \
  "$dress/fallback.png"

say "done — $(ls -1 "$out" | wc -l | tr -d ' ') fixtures in $out, dress package in $dress"
