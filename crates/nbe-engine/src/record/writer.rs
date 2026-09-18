//! Crash-safe fragmented-MP4 writer (Prompt 09 WU34, SPEC §9.3).
//!
//! Shape of the file, in write order:
//! ```text
//! ftyp | moov (empty sample tables + mvex) | moof+mdat | moof+mdat | ...
//! ```
//! `ftyp`+`moov` go out BEFORE any sample (init-segment safe), and every
//! fragment is `write() + flush()`ed the moment it is complete. A `SIGKILL`
//! therefore costs at most the in-progress tail: everything before it is a
//! parseable prefix (`moov` describes the tracks, each `moof` is
//! self-describing). There is deliberately NO finalization step that the file
//! depends on — `finish` only flushes the tail and patches duration fields.
//! Dropping a [`RecordingWriter`] without `finish` IS the crash shape, and the
//! test proves the prefix still parses (AC-6 at unit level).
//!
//! Fragment policy (§9.3 table): fragments hold ≤ 1 s of video each, audio is
//! interleaved per fragment (one `traf` per track in every `moof`, samples
//! for both tracks in the following `mdat`), `moov` is upfront, and no
//! finalization is required.
//!
//! Video samples are H.264 access units in AVCC form (WU2 `EncodedUnit`);
//! `avcC` is built from the first keyframe's SPS/PPS. Recording opens at the
//! first IDR: pre-IDR units are undecodable without parameter sets and are
//! dropped (counted in debug logs, never silently kept).
//!
//! Audio samples are raw AAC-LC packets from `super::aac` (1024 frames each,
//! 48 kHz stereo). No PCM-in-MP4 path exists anywhere in this file.
//!
//! AAC priming trim (mux-time, no edit list): the encoder carries the
//! standard 2112-sample (~44 ms) delay, and the writer drops the first
//! [`AAC_PRIMING_TRIM_PACKETS`] packets (2048 samples) at mux time. The
//! 64-sample residual (1.33 ms) is the measured A/V offset, inside the 20 ms
//! sync requirement. Packet-drop suffices — no edit list — because the
//! residual is an order of magnitude under the bound; sample-exact trimming
//! would need a partial-packet edit for 1.33 ms nobody can hear.
//!
//! Zero new dependencies: every box here is hand-rolled bytes. The decoder
//! (`ffprobe`) is the conformance check.

use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::encode::EncodedUnit;

use super::aac::{AacEncoder, FRAMES_PER_PACKET};
use super::{RecordError, RecordParams};

pub const VIDEO_TRACK_ID: u32 = 1;
pub const AUDIO_TRACK_ID: u32 = 2;
pub const VIDEO_TIMESCALE: u32 = 90_000;
pub const AUDIO_TIMESCALE: u32 = 48_000;
pub const MVHD_TIMESCALE: u32 = 1_000;
/// Standard AAC encoder priming delay, in samples (@48 kHz): ~44 ms the
/// decoder replays before the first real input sample.
pub const AAC_PRIMING_TRIM_SAMPLES: u64 = 2112;
/// Packets dropped at mux time to cover the priming delay: floor(2112/1024)
/// = 2 packets (2048 samples), leaving a 64-sample / 1.33 ms residual inside
/// the 20 ms sync requirement. Ceil (3 packets) would overshoot to −960
/// samples (−20 ms, at the bound); floor keeps +64 samples (+1.33 ms).
pub const AAC_PRIMING_TRIM_PACKETS: u64 = 2;
/// Fragment ceiling in video ticks: exactly 1 s (§9.3 "≤ 1 second").
const FRAGMENT_TICKS: u64 = VIDEO_TIMESCALE as u64;
/// Pre-IDR buffer cap: ~20 s at 30 fps. A stream with no IDR in that span is
/// broken input, not a slow start.
const MAX_PRE_IDR_UNITS: usize = 600;
/// Bound on video samples buffered between fragment windows (~6 s at 30 fps).
/// Windows normally complete every second, so this only engages when emission
/// stalls (e.g. audio has not caught up): excess sheds oldest-first and counts,
/// mirroring [`super::AudioTap`]'s drop-oldest-counted policy. Shed content is
/// lost but the timeline stays continuous (the next emit's `tfdt` base only
/// counts emitted ticks), exactly like a shed record frame upstream.
pub const MAX_BUFFERED_VIDEO_FRAMES: usize = 180;
/// Bound on AAC packets buffered between windows (~6 s: 46 packets/s).
/// Same drop-oldest-counted shed as video above.
pub const MAX_BUFFERED_AUDIO_PACKETS: usize = 280;
/// Backstop on buffered AAC payload bytes (packets vary in size; the packet
/// cap is the primary bound).
pub const MAX_BUFFERED_AUDIO_BYTES: usize = 4 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Box helpers
// ---------------------------------------------------------------------------

fn be16(buf: &mut Vec<u8>, v: u16) {
    buf.extend_from_slice(&v.to_be_bytes());
}

fn be32(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_be_bytes());
}

fn be64(buf: &mut Vec<u8>, v: u64) {
    buf.extend_from_slice(&v.to_be_bytes());
}

/// Open a box: reserves the size field, writes the type. Returns the offset
/// of the size field for [`end_box`].
fn start_box(buf: &mut Vec<u8>, typ: &[u8; 4]) -> usize {
    let off = buf.len();
    be32(buf, 0);
    buf.extend_from_slice(typ);
    off
}

fn end_box(buf: &mut [u8], off: usize) {
    let len = (buf.len() - off) as u32;
    buf[off..off + 4].copy_from_slice(&len.to_be_bytes());
}

fn full(buf: &mut Vec<u8>, version: u8, flags: u32) {
    buf.push(version);
    buf.extend_from_slice(&flags.to_be_bytes()[1..4]);
}

// ---------------------------------------------------------------------------
// SPS/PPS + avcC + esds
// ---------------------------------------------------------------------------

/// Validate caller-supplied parameter sets (from `EncodeSession`): present,
/// plausible length, correct NAL types. The writer cannot parse these out of
/// the stream — VideoToolbox emits slices only — so it validates what it is
/// given instead of trusting it.
fn validate_parameter_sets(sps: &[u8], pps: &[u8]) -> Result<(), RecordError> {
    if sps.len() < 4 {
        return Err(RecordError::Input(
            "SPS missing or shorter than 4 bytes; cannot build avcC".into(),
        ));
    }
    if sps[0] & 0x1F != 7 {
        return Err(RecordError::Input(
            "first parameter set is not an SPS (NAL type 7)".into(),
        ));
    }
    if pps.is_empty() || pps[0] & 0x1F != 8 {
        return Err(RecordError::Input(
            "second parameter set is not a PPS (NAL type 8)".into(),
        ));
    }
    Ok(())
}

fn avcc_box(sps: &[u8], pps: &[u8], params: &RecordParams) -> Result<Vec<u8>, RecordError> {
    if sps.len() < 4 {
        return Err(RecordError::Input("SPS shorter than 4 bytes".into()));
    }
    let mut b = Vec::new();
    let off = start_box(&mut b, b"avc1");
    b.extend_from_slice(&[0u8; 6]); // reserved
    be16(&mut b, 1); // data_reference_index
    b.extend_from_slice(&[0u8; 16]); // pre_defined + reserved
    be16(&mut b, params.width.min(0xFFFF) as u16);
    be16(&mut b, params.height.min(0xFFFF) as u16);
    be32(&mut b, 0x0048_0000); // horizresolution 72 dpi
    be32(&mut b, 0x0048_0000); // vertresolution
    be32(&mut b, 0); // reserved
    be16(&mut b, 1); // frame_count
    b.extend_from_slice(&[0u8; 32]); // compressorname
    be16(&mut b, 0x0018); // depth
    be16(&mut b, 0xFFFF); // pre_defined (-1)
                          // avcC child
    let coff = start_box(&mut b, b"avcC");
    b.push(1); // configurationVersion
    b.push(sps[1]); // AVCProfileIndication
    b.push(sps[2]); // profile_compat
    b.push(sps[3]); // AVCLevelIndication
    b.push(0xFF); // lengthSizeMinusOne (4-byte NAL lengths)
    b.push(0xE1); // numOfSequenceParameterSets (1)
    be16(&mut b, sps.len() as u16);
    b.extend_from_slice(sps);
    b.push(1); // numOfPictureParameterSets
    be16(&mut b, pps.len() as u16);
    b.extend_from_slice(pps);
    end_box(&mut b, coff);
    end_box(&mut b, off);
    Ok(b)
}

/// `esds`: version/flags + the codec's magic cookie verbatim. The cookie IS
/// the ES_Descriptor hierarchy (see `AacEncoder::esds_content`) — nesting it
/// inside a second hand-built hierarchy is what produced "Audio object type
/// 0" once, so this box builds nothing beyond the header.
fn esds_box(esds_content: &[u8]) -> Vec<u8> {
    let mut b = Vec::new();
    let off = start_box(&mut b, b"esds");
    full(&mut b, 0, 0);
    b.extend_from_slice(esds_content);
    end_box(&mut b, off);
    b
}

fn mp4a_box(esds_content: &[u8]) -> Vec<u8> {
    let mut b = Vec::new();
    let off = start_box(&mut b, b"mp4a");
    b.extend_from_slice(&[0u8; 6]);
    be16(&mut b, 1);
    b.extend_from_slice(&[0u8; 8]); // reserved
    be16(&mut b, 2); // channelcount
    be16(&mut b, 16); // samplesize
    be16(&mut b, 0); // pre_defined
    be16(&mut b, 0); // reserved
    be32(&mut b, AUDIO_TIMESCALE << 16); // samplerate 16.16
    b.extend_from_slice(&esds_box(esds_content));
    end_box(&mut b, off);
    b
}

// ---------------------------------------------------------------------------
// ftyp + moov
// ---------------------------------------------------------------------------

fn ftyp_box() -> Vec<u8> {
    let mut b = Vec::new();
    let off = start_box(&mut b, b"ftyp");
    b.extend_from_slice(b"isom");
    be32(&mut b, 0x200);
    b.extend_from_slice(b"isomiso2avc1mp41");
    end_box(&mut b, off);
    b
}

/// Offsets (absolute, from file start) of duration fields patched at `finish`.
#[derive(Debug, Default)]
struct DurationPatches {
    mvhd: usize,
    tkhd_video: usize,
    tkhd_audio: usize,
    mdhd_video: usize,
    mdhd_audio: usize,
}

fn mvhd_box(buf: &mut Vec<u8>) -> usize {
    let off = start_box(buf, b"mvhd");
    full(buf, 0, 0);
    be32(buf, 0); // creation
    be32(buf, 0); // modification
    be32(buf, MVHD_TIMESCALE);
    let dur_off = buf.len(); // absolute once moov base is added by caller
    be32(buf, 0); // duration (patched at finish)
    be32(buf, 0x0001_0000); // rate
    be16(buf, 0x0100); // volume
    be16(buf, 0); // reserved
    buf.extend_from_slice(&[0u8; 8]); // reserved
                                      // identity matrix
    for v in [0x0001_0000u32, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000] {
        be32(buf, v);
    }
    buf.extend_from_slice(&[0u8; 24]); // pre_defined
    be32(buf, 3); // next_track_ID
    end_box(buf, off);
    dur_off
}

fn tkhd_box(buf: &mut Vec<u8>, track_id: u32, is_video: bool, w: u16, h: u16) -> usize {
    let off = start_box(buf, b"tkhd");
    full(buf, 0, 7); // enabled | in_movie | in_preview
    be32(buf, 0);
    be32(buf, 0);
    be32(buf, track_id);
    be32(buf, 0); // reserved
    let dur_off = buf.len();
    be32(buf, 0); // duration (patched at finish)
    buf.extend_from_slice(&[0u8; 8]); // reserved
    be16(buf, 0); // layer
    be16(buf, 0); // alternate_group
    be16(buf, if is_video { 0 } else { 0x0100 }); // volume
    be16(buf, 0); // reserved
    for v in [0x0001_0000u32, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000] {
        be32(buf, v);
    }
    be32(buf, if is_video { (w as u32) << 16 } else { 0 });
    be32(buf, if is_video { (h as u32) << 16 } else { 0 });
    end_box(buf, off);
    dur_off
}

fn mdhd_box(buf: &mut Vec<u8>, timescale: u32) -> usize {
    let off = start_box(buf, b"mdhd");
    full(buf, 0, 0);
    be32(buf, 0);
    be32(buf, 0);
    be32(buf, timescale);
    let dur_off = buf.len();
    be32(buf, 0); // duration (patched at finish)
    be16(buf, 0x55C4); // language 'und'
    be16(buf, 0); // pre_defined
    end_box(buf, off);
    dur_off
}

fn hdlr_box(buf: &mut Vec<u8>, handler: &[u8; 4], name: &[u8]) {
    let off = start_box(buf, b"hdlr");
    full(buf, 0, 0);
    be32(buf, 0); // pre_defined
    buf.extend_from_slice(handler);
    buf.extend_from_slice(&[0u8; 12]); // reserved
    buf.extend_from_slice(name);
    buf.push(0);
    end_box(buf, off);
}

fn dinf_box(buf: &mut Vec<u8>) {
    let off = start_box(buf, b"dinf");
    let doff = start_box(buf, b"dref");
    full(buf, 0, 0);
    be32(buf, 1); // entry_count
    let uoff = start_box(buf, b"url ");
    full(buf, 0, 1); // self-contained
    end_box(buf, uoff);
    end_box(buf, doff);
    end_box(buf, off);
}

fn empty_table(buf: &mut Vec<u8>, typ: &[u8; 4], entry_bytes: usize) {
    let off = start_box(buf, typ);
    full(buf, 0, 0);
    be32(buf, 0); // entry_count
    debug_assert_eq!(entry_bytes, 0);
    end_box(buf, off);
}

/// moov with EMPTY sample tables + mvex: the init segment. Durations are 0
/// until `finish` patches them (offsets returned for that).
fn moov_box(
    sps: &[u8],
    pps: &[u8],
    asc: &[u8],
    params: &RecordParams,
) -> Result<(Vec<u8>, DurationPatches), RecordError> {
    let mut moov = Vec::new();
    let moff = start_box(&mut moov, b"moov");
    let file_base: usize = ftyp_box().len();
    let mut patches = DurationPatches::default();

    // All offsets below are moov-relative here; made absolute at the end.
    let mvhd_rel = mvhd_box(&mut moov);
    patches.mvhd = file_base + moff + mvhd_rel;

    for (track_id, is_video) in [(VIDEO_TRACK_ID, true), (AUDIO_TRACK_ID, false)] {
        let toff = start_box(&mut moov, b"trak");
        let _ = toff;
        let tkhd_rel = tkhd_box(
            &mut moov,
            track_id,
            is_video,
            params.width.min(0xFFFF) as u16,
            params.height.min(0xFFFF) as u16,
        );
        let mdia = start_box(&mut moov, b"mdia");
        let mdhd_rel = mdhd_box(
            &mut moov,
            if is_video {
                VIDEO_TIMESCALE
            } else {
                AUDIO_TIMESCALE
            },
        );
        if is_video {
            patches.tkhd_video = file_base + moff + tkhd_rel;
            patches.mdhd_video = file_base + moff + mdhd_rel;
            hdlr_box(&mut moov, b"vide", b"VideoHandler");
        } else {
            patches.tkhd_audio = file_base + moff + tkhd_rel;
            patches.mdhd_audio = file_base + moff + mdhd_rel;
            hdlr_box(&mut moov, b"soun", b"SoundHandler");
        }
        let minf = start_box(&mut moov, b"minf");
        if is_video {
            let voff = start_box(&mut moov, b"vmhd");
            full(&mut moov, 0, 1);
            be16(&mut moov, 0);
            moov.extend_from_slice(&[0u8; 6]);
            end_box(&mut moov, voff);
        } else {
            let soff = start_box(&mut moov, b"smhd");
            full(&mut moov, 0, 0);
            be16(&mut moov, 0); // balance
            be16(&mut moov, 0); // reserved
            end_box(&mut moov, soff);
        }
        dinf_box(&mut moov);
        let stbl = start_box(&mut moov, b"stbl");
        let stsd = start_box(&mut moov, b"stsd");
        full(&mut moov, 0, 0);
        be32(&mut moov, 1); // entry_count
        if is_video {
            moov.extend_from_slice(&avcc_box(sps, pps, params)?);
        } else {
            moov.extend_from_slice(&mp4a_box(asc));
        }
        end_box(&mut moov, stsd);
        empty_table(&mut moov, b"stts", 0);
        empty_table(&mut moov, b"stsc", 0);
        // stsz has two extra u32s (sample_size, sample_count).
        let szoff = start_box(&mut moov, b"stsz");
        full(&mut moov, 0, 0);
        be32(&mut moov, 0);
        be32(&mut moov, 0);
        end_box(&mut moov, szoff);
        empty_table(&mut moov, b"stco", 0);
        end_box(&mut moov, stbl);
        end_box(&mut moov, minf);
        end_box(&mut moov, mdia);
        end_box(&mut moov, toff);
    }

    let mvex = start_box(&mut moov, b"mvex");
    for track_id in [VIDEO_TRACK_ID, AUDIO_TRACK_ID] {
        let troff = start_box(&mut moov, b"trex");
        full(&mut moov, 0, 0);
        be32(&mut moov, track_id);
        be32(&mut moov, 1); // default_sample_description_index
        be32(&mut moov, 0); // default_sample_duration
        be32(&mut moov, 0); // default_sample_size
        be32(&mut moov, 0); // default_sample_flags
        end_box(&mut moov, troff);
    }
    end_box(&mut moov, mvex);
    end_box(&mut moov, moff);
    Ok((moov, patches))
}

// ---------------------------------------------------------------------------
// Fragments
// ---------------------------------------------------------------------------

struct OutSample {
    duration: u32,
    size: u32,
    flags: u32,
}

/// trun with per-sample duration/size/flags/cts (flags 0x0F01 — the layout
/// the box-walk test reads back). Returns the box plus the offset of the
/// data_offset field relative to the trun's own start.
fn trun_box(samples: &[OutSample]) -> (Vec<u8>, usize) {
    let mut b = Vec::new();
    let off = start_box(&mut b, b"trun");
    // flags 0x0F01: data_offset + duration + size + flags + cts per sample.
    full(&mut b, 0, 0x0F01);
    be32(&mut b, samples.len() as u32);
    let rel = b.len() - off;
    be32(&mut b, 0); // data_offset placeholder
    for s in samples {
        be32(&mut b, s.duration);
        be32(&mut b, s.size);
        be32(&mut b, s.flags);
        be32(&mut b, 0); // cts_offset (no reordering: decode == display)
    }
    end_box(&mut b, off);
    (b, rel)
}

/// One track's contribution to a fragment.
struct FragTrack<'a> {
    id: u32,
    base_ticks: u64,
    samples: &'a [OutSample],
    data: &'a [u8],
}

fn write_fragment(
    file: &mut File,
    seq: u32,
    video: &FragTrack<'_>,
    audio: &FragTrack<'_>,
) -> Result<(), RecordError> {
    // Assemble moof with placeholder data_offsets, fix them up once moof
    // length is known, then write moof + mdat + flush (crash-safe point).
    let (vtrun, vrel) = trun_box(video.samples);
    let (atrun, arel) = trun_box(audio.samples);

    let mut moof = Vec::new();
    let moff = start_box(&mut moof, b"moof");
    let mhoff = start_box(&mut moof, b"mfhd");
    full(&mut moof, 0, 0);
    be32(&mut moof, seq);
    end_box(&mut moof, mhoff);

    let mut v_field = 0usize;
    let mut a_field = 0usize;
    for track in [video, audio] {
        let troff = start_box(&mut moof, b"traf");
        let thoff = start_box(&mut moof, b"tfhd");
        full(&mut moof, 0, 0x020000); // default-base-is-moof
        be32(&mut moof, track.id);
        end_box(&mut moof, thoff);
        let tdoff = start_box(&mut moof, b"tfdt");
        full(&mut moof, 1, 0); // 64-bit decode time
        be64(&mut moof, track.base_ticks);
        end_box(&mut moof, tdoff);
        let field = moof.len()
            + if track.id == VIDEO_TRACK_ID {
                vrel
            } else {
                arel
            };
        moof.extend_from_slice(if track.id == VIDEO_TRACK_ID {
            &vtrun
        } else {
            &atrun
        });
        end_box(&mut moof, troff);
        if track.id == VIDEO_TRACK_ID {
            v_field = field;
        } else {
            a_field = field;
        }
    }
    end_box(&mut moof, moff);

    // data_offset is moof-relative: video starts after moof + mdat header,
    // audio after the video payload.
    let v_data_off = (moof.len() + 8) as u32;
    let a_data_off = v_data_off + video.data.len() as u32;
    moof[v_field..v_field + 4].copy_from_slice(&v_data_off.to_be_bytes());
    moof[a_field..a_field + 4].copy_from_slice(&a_data_off.to_be_bytes());

    file.write_all(&moof).map_err(disk)?;
    let mdat_len = (8 + video.data.len() + audio.data.len()) as u32;
    let mut mdat_head = Vec::with_capacity(8);
    be32(&mut mdat_head, mdat_len);
    mdat_head.extend_from_slice(b"mdat");
    file.write_all(&mdat_head).map_err(disk)?;
    file.write_all(video.data).map_err(disk)?;
    file.write_all(audio.data).map_err(disk)?;
    file.flush().map_err(disk)?;
    Ok(())
}

fn disk(e: std::io::Error) -> RecordError {
    RecordError::Disk(e.to_string())
}

// ---------------------------------------------------------------------------
// Writer
// ---------------------------------------------------------------------------

struct BufferedVideo {
    data: Vec<u8>,
    /// Ticks since recording start (@90 kHz). Duration is assigned when the
    /// NEXT unit arrives (diff of PTS), or at `finish` for the tail.
    start_ticks: u64,
    duration_ticks: Option<u32>,
    keyframe: bool,
}

pub struct RecordingWriter {
    file: File,
    path: PathBuf,
    params: RecordParams,
    aac: Option<AacEncoder>,
    asc: Vec<u8>,
    header_written: bool,
    patches: DurationPatches,
    pre_idr: Vec<EncodedUnit>,
    pre_idr_dropped_logged: bool,
    /// Unconsumed video, in order. All but possibly the last have fixed
    /// durations (assigned on the next unit's arrival).
    video: Vec<BufferedVideo>,
    pts_base: Option<f64>,
    prev_pts: Option<f64>,
    /// Unconsumed AAC payload bytes + per-packet lengths in order.
    audio_data: Vec<u8>,
    audio_packet_lens: Vec<usize>,
    video_ticks_emitted: u64,
    audio_packets_emitted: u64,
    /// Buffered-but-unshed samples dropped under the window caps above
    /// (drop-oldest, counted — telemetry, never errors).
    dropped_video: u64,
    dropped_audio_packets: u64,
    /// Priming packets still to drop at mux time (counts down from
    /// [`AAC_PRIMING_TRIM_PACKETS`]). Dropped packets never reach the
    /// fragment timeline, so the first retained packet starts at audio time
    /// 0 with only the 64-sample residual.
    priming_to_skip: u64,
    seq: u32,
}

impl RecordingWriter {
    pub fn create(params: &RecordParams) -> Result<Self, RecordError> {
        std::fs::create_dir_all(&params.directory).map_err(disk)?;
        let path = params.directory.join(super::recording_filename(params));
        let file = File::create(&path).map_err(disk)?;
        // Fail loudly BEFORE any sample is accepted: no AAC means no
        // recording, never a video-only file masquerading as complete.
        if !super::aac::is_available() {
            let _ = std::fs::remove_file(&path);
            return Err(RecordError::Aac(
                "AudioToolbox AAC refused the probe; refusing to record".into(),
            ));
        }
        let aac = AacEncoder::new().map_err(|e| RecordError::Aac(e.to_string()))?;
        let asc = aac.esds_content();
        Ok(Self {
            file,
            path,
            params: params.clone(),
            aac: Some(aac),
            asc,
            header_written: false,
            patches: DurationPatches::default(),
            pre_idr: Vec::new(),
            pre_idr_dropped_logged: false,
            video: Vec::new(),
            pts_base: None,
            prev_pts: None,
            audio_data: Vec::new(),
            audio_packet_lens: Vec::new(),
            video_ticks_emitted: 0,
            audio_packets_emitted: 0,
            dropped_video: 0,
            dropped_audio_packets: 0,
            priming_to_skip: AAC_PRIMING_TRIM_PACKETS,
            seq: 1,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Buffered-but-unemitted video samples currently held between windows.
    pub fn buffered_video_len(&self) -> usize {
        self.video.len()
    }

    /// Buffered-but-unemitted AAC packets currently held between windows.
    pub fn buffered_audio_packets(&self) -> usize {
        self.audio_packet_lens.len()
    }

    /// Window-cap sheds so far: video samples dropped oldest-first.
    pub fn dropped_video(&self) -> u64 {
        self.dropped_video
    }

    /// Window-cap sheds so far: AAC packets dropped oldest-first.
    pub fn dropped_audio_packets(&self) -> u64 {
        self.dropped_audio_packets
    }

    /// Install the stream's real parameter sets (WU-pipe: captured by the
    /// record thread from the live encoder's first keyframe, or supplied by
    /// the test seam). Must precede the first IDR push — the header writes on
    /// that push using these sets — and is refused after the header exists.
    pub fn set_parameter_sets(&mut self, sps: Vec<u8>, pps: Vec<u8>) -> Result<(), RecordError> {
        if self.header_written {
            return Err(RecordError::Input(
                "parameter sets arrived after the header was written".into(),
            ));
        }
        self.params.sps = sps;
        self.params.pps = pps;
        Ok(())
    }

    pub fn push_video(&mut self, unit: &EncodedUnit) -> Result<(), RecordError> {
        if !self.header_written {
            if !unit.is_keyframe {
                if self.pre_idr.len() >= MAX_PRE_IDR_UNITS {
                    return Err(RecordError::Input(format!(
                        "{} pre-IDR units with no keyframe; refusing broken stream",
                        self.pre_idr.len()
                    )));
                }
                self.pre_idr.push(EncodedUnit {
                    data: unit.data.clone(),
                    pts_seconds: unit.pts_seconds,
                    is_keyframe: unit.is_keyframe,
                });
                return Ok(());
            }
            // First IDR: parameter sets → header, pre-IDR tail dropped.
            if !self.pre_idr.is_empty() && !self.pre_idr_dropped_logged {
                tracing::debug!(
                    "recording opens at first IDR; dropping {} pre-IDR units",
                    self.pre_idr.len()
                );
                self.pre_idr_dropped_logged = true;
            }
            self.pre_idr.clear();
            validate_parameter_sets(&self.params.sps, &self.params.pps)?;
            self.file.write_all(&ftyp_box()).map_err(disk)?;
            let (moov, patches) =
                moov_box(&self.params.sps, &self.params.pps, &self.asc, &self.params)?;
            self.file.write_all(&moov).map_err(disk)?;
            self.file.flush().map_err(disk)?;
            self.patches = patches;
            self.header_written = true;
        }
        let base = *self.pts_base.get_or_insert(unit.pts_seconds);
        let rel_ticks = ((unit.pts_seconds - base) * VIDEO_TIMESCALE as f64)
            .round()
            .max(0.0) as u64;
        // Fix the previous sample's duration from this arrival's PTS.
        if let (Some(prev_pts), Some(prev)) = (self.prev_pts, self.video.last_mut()) {
            let d = ((unit.pts_seconds - prev_pts) * VIDEO_TIMESCALE as f64).round() as i64;
            prev.duration_ticks = Some(d.max(1) as u32);
        }
        self.prev_pts = Some(unit.pts_seconds);
        self.video.push(BufferedVideo {
            data: unit.data.clone(),
            start_ticks: rel_ticks,
            duration_ticks: None,
            keyframe: unit.is_keyframe,
        });
        self.try_emit(false)?;
        self.enforce_buffer_caps();
        Ok(())
    }

    pub fn push_audio(&mut self, pcm_f32: &[f32]) -> Result<(), RecordError> {
        if pcm_f32.is_empty() {
            return Ok(());
        }
        if !pcm_f32.len().is_multiple_of(2) {
            return Err(RecordError::Input(
                "audio must be whole stereo frames".into(),
            ));
        }
        let aac = self.aac.as_mut().expect("encoder lives until finish");
        let frames = aac
            .encode_interleaved_f32(pcm_f32)
            .map_err(|e| RecordError::Aac(e.to_string()))?;
        self.ingest_aac_frames(frames);
        self.try_emit(false)?;
        self.enforce_buffer_caps();
        Ok(())
    }

    /// Store AAC packets after the mux-time priming trim: the first
    /// [`AAC_PRIMING_TRIM_PACKETS`] packets are dropped (never counted toward
    /// `audio_packets_emitted`, so the retained timeline starts at 0 with the
    /// 64-sample residual). Dropped packets are priming delay, not content.
    fn ingest_aac_frames(&mut self, frames: Vec<super::aac::AacFrame>) {
        for f in frames {
            if self.priming_to_skip > 0 {
                self.priming_to_skip -= 1;
                continue;
            }
            self.audio_packet_lens.push(f.data.len());
            self.audio_data.extend_from_slice(&f.data);
        }
    }

    /// Shed buffered windows down to the caps (drop-oldest, counted). Runs
    /// after every streaming push, so steady-state emission (which drains
    /// below the caps) never sheds and only a stalled window does.
    fn enforce_buffer_caps(&mut self) {
        if self.video.len() > MAX_BUFFERED_VIDEO_FRAMES {
            let excess = self.video.len() - MAX_BUFFERED_VIDEO_FRAMES;
            self.video.drain(..excess);
            self.dropped_video += excess as u64;
        }
        while self.audio_packet_lens.len() > MAX_BUFFERED_AUDIO_PACKETS
            || self.audio_data.len() > MAX_BUFFERED_AUDIO_BYTES
        {
            let Some(len) = self.audio_packet_lens.first().copied() else {
                break;
            };
            self.audio_packet_lens.remove(0);
            self.audio_data.drain(..len.min(self.audio_data.len()));
            self.dropped_audio_packets += 1;
        }
    }

    /// Emit every complete 1 s fragment available from buffered heads.
    /// With `finishing`, emit partial tails too (still cut at 1 s windows so
    /// no fragment exceeds the ceiling).
    fn try_emit(&mut self, finishing: bool) -> Result<(), RecordError> {
        loop {
            let fixed = if finishing {
                self.video.len()
            } else {
                self.video.len().saturating_sub(1)
            };
            if fixed == 0 && !(finishing && !self.audio_packet_lens.is_empty()) {
                break;
            }
            let fstart = self.video_ticks_emitted;
            let fend = fstart + FRAGMENT_TICKS;
            // Count fixed samples starting inside the window.
            let mut m = 0usize;
            let mut t = fstart;
            while m < fixed {
                let s = &self.video[m];
                if s.start_ticks >= fend {
                    break;
                }
                t += s.duration_ticks.unwrap_or(0) as u64;
                m += 1;
            }
            if m == 0 {
                // No video for this window: at finish with leftover audio,
                // drain it in ≤1 s chunks (empty video trun, legal); while
                // streaming, wait for video.
                if finishing && !self.audio_packet_lens.is_empty() {
                    // 46 packets ≈ 0.98 s of AAC.
                    let take = self.audio_packet_lens.len().min(46);
                    self.emit_fragment(0, take)?;
                    continue;
                }
                break;
            }
            if !finishing && t < fend {
                // Window not yet covered by fixed samples — wait for more
                // pushes. (The unfixed tail's duration is still unknown, so
                // `t` only counts fixed samples; a straddler extends ≤1
                // frame past the boundary, inside the test's bound.)
                break;
            }
            // Audio packets starting before the video span's end join this
            // fragment (interleaved per §9.3).
            let vend_secs = t as f64 / VIDEO_TIMESCALE as f64;
            let mut want = 0usize;
            while want < self.audio_packet_lens.len() {
                let start = (self.audio_packets_emitted + want as u64) as f64
                    * FRAMES_PER_PACKET as f64
                    / AUDIO_TIMESCALE as f64;
                if start >= vend_secs {
                    break;
                }
                want += 1;
            }
            if !finishing {
                let audio_end = (self.audio_packets_emitted + want as u64) as f64
                    * FRAMES_PER_PACKET as f64
                    / AUDIO_TIMESCALE as f64;
                if audio_end < vend_secs {
                    // Audio hasn't caught up to this window yet.
                    break;
                }
            }
            self.emit_fragment(m, want)?;
        }
        Ok(())
    }

    fn emit_fragment(&mut self, n_video: usize, n_audio: usize) -> Result<(), RecordError> {
        if n_video == 0 && n_audio == 0 {
            return Ok(());
        }
        let mut vsamples = Vec::with_capacity(n_video);
        let mut vdata = Vec::new();
        let mut consumed_ticks = 0u64;
        for s in self.video.drain(..n_video) {
            let duration = s
                .duration_ticks
                .expect("emitted samples have fixed durations");
            vsamples.push(OutSample {
                duration,
                size: s.data.len() as u32,
                flags: if s.keyframe { 0x0200_0000 } else { 0x0101_0000 },
            });
            consumed_ticks += duration as u64;
            vdata.extend_from_slice(&s.data);
        }
        let mut asamples = Vec::with_capacity(n_audio);
        let mut adata = Vec::new();
        for len in self.audio_packet_lens.drain(..n_audio) {
            asamples.push(OutSample {
                duration: FRAMES_PER_PACKET as u32,
                size: len as u32,
                flags: 0x0200_0000,
            });
            adata.extend_from_slice(&self.audio_data[..len]);
            self.audio_data.drain(..len);
        }
        write_fragment(
            &mut self.file,
            self.seq,
            &FragTrack {
                id: VIDEO_TRACK_ID,
                base_ticks: self.video_ticks_emitted,
                samples: &vsamples,
                data: &vdata,
            },
            &FragTrack {
                id: AUDIO_TRACK_ID,
                base_ticks: self.audio_packets_emitted * FRAMES_PER_PACKET as u64,
                samples: &asamples,
                data: &adata,
            },
        )?;
        self.seq += 1;
        self.video_ticks_emitted += consumed_ticks;
        self.audio_packets_emitted += n_audio as u64;
        Ok(())
    }

    pub fn finish(mut self) -> Result<PathBuf, RecordError> {
        // Fix the tail sample, flush the encoder, emit everything left.
        let tail_duration = match self.video.last() {
            Some(last) if last.duration_ticks.is_none() => Some(
                self.video
                    .iter()
                    .rev()
                    .nth(1)
                    .and_then(|s| s.duration_ticks)
                    .unwrap_or(VIDEO_TIMESCALE / self.params.fps.max(1))
                    .max(1),
            ),
            _ => None,
        };
        if let (Some(d), Some(last)) = (tail_duration, self.video.last_mut()) {
            last.duration_ticks = Some(d);
        }
        let aac = self.aac.as_mut().expect("encoder lives until finish");
        let tail = aac.flush().map_err(|e| RecordError::Aac(e.to_string()))?;
        self.ingest_aac_frames(tail);
        self.try_emit(true)?;
        self.patch_durations()?;
        self.file.flush().map_err(disk)?;
        // WU5 [RI-5]: the always-sidecar. fMP4 carries chapters poorly, so
        // chapters ride `<stem>.markers.json` beside the recording for every
        // container. This is the single place files finalize — a sidecar
        // write failure is E_DISK like any other finalize failure.
        super::markers::write_sidecar(&self.path, &super::markers::list())?;
        Ok(self.path.clone())
    }

    fn patch_durations(&mut self) -> Result<(), RecordError> {
        let video_ticks = self.video_ticks_emitted;
        let audio_ticks = self.audio_packets_emitted * FRAMES_PER_PACKET as u64;
        let video_ms = (video_ticks * MVHD_TIMESCALE as u64 / VIDEO_TIMESCALE as u64) as u32;
        let audio_ms = (audio_ticks * MVHD_TIMESCALE as u64 / AUDIO_TIMESCALE as u64) as u32;
        let mvhd_ms = video_ms.max(audio_ms);
        let patches = [
            (self.patches.mvhd, mvhd_ms),
            (self.patches.tkhd_video, video_ms),
            (self.patches.tkhd_audio, audio_ms),
            (
                self.patches.mdhd_video,
                video_ticks.min(u32::MAX as u64) as u32,
            ),
            (
                self.patches.mdhd_audio,
                audio_ticks.min(u32::MAX as u64) as u32,
            ),
        ];
        for (off, val) in patches {
            self.file.seek(SeekFrom::Start(off as u64)).map_err(disk)?;
            self.file.write_all(&val.to_be_bytes()).map_err(disk)?;
        }
        Ok(())
    }
}

/// One-shot recording: push everything, finish, return the file path.
pub fn write_recording(
    video_units: &[EncodedUnit],
    audio_pcm_f32: &[f32],
    params: &RecordParams,
) -> Result<PathBuf, RecordError> {
    if video_units.is_empty() {
        return Err(RecordError::Input("no video units".into()));
    }
    let mut w = RecordingWriter::create(params)?;
    for u in video_units {
        w.push_video(u)?;
    }
    // Feed audio in bounded chunks so a long recording never holds two
    // copies of the whole mix at once (encoder copy + pending).
    const CHUNK_FRAMES: usize = 8192;
    let mut off = 0usize;
    while off < audio_pcm_f32.len() {
        let end = (off + CHUNK_FRAMES * 2).min(audio_pcm_f32.len());
        w.push_audio(&audio_pcm_f32[off..end])?;
        off = end;
    }
    w.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parameter_set_validation_rejects_mistyped_nals() {
        let sps = vec![0x67, 0x64, 0x00, 0x1E, 0xAA];
        let pps = vec![0x68, 0x11, 0x22];
        validate_parameter_sets(&sps, &pps).unwrap();
        assert!(validate_parameter_sets(&[], &pps).is_err());
        assert!(validate_parameter_sets(&[0x65, 0x11, 0x22, 0x33], &pps).is_err());
        assert!(validate_parameter_sets(&sps, &[]).is_err());
    }
}
