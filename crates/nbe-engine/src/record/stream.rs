//! Stream session glue (Prompt 10 WU4+WU5, SPEC §16.14).
//!
//! Lifecycle owner between the directive path and the (WU5) publisher:
//! `stream.start` opens a [`StreamSession`], `stream.stop` / `show.stop`
//! quiescence closes it BEFORE `apply()` emits the ack (SPEC §5.9.5: the ack
//! is honest only after the effect is real) — the `record.stop`
//! `stop_and_finish` shape, mirrored.
//!
//! ## Live feed (WU5 FIX round)
//!
//! The render loop feeds the session via [`feed_stream_surface`]: Surface
//! path, zero-copy, NEVER readback/rgba on this path. G1 discipline: the
//! View draws regardless; the stream takes the drawn surface (`Arc` clone,
//! shared with record when both are live) or drops it (counts
//! `skipped_stream_frames`, never `skipped_record_frames` nor View drops).
//! Encoding is `encode_pixel_buffer` (the zero-copy seam); units become FLV
//! video tags via [`flv_video_tag`] then `publish_video` (bounded try_send,
//! shed counted in the publisher AND the stream counter).
//!
//! ## Scope (WU4)
//!
//! Session bookkeeping ONLY — no transport, no publisher, no encoder session.
//! The live streaming objects arrive in WU5; this side holds only the publish
//! target + the published selection, all `Send`, so engine state can hold it.
//! The stream holds no surface pool: pool ownership (one pool or two, who
//! sizes it) is WU5's G1 decision, and a pool built here with nothing to draw
//! into it would be ~25 MiB of VRAM held for no take.
//!
//! ## Defined behavior
//!
//! * [`set_force_no_chain`] forces the chain-less path exactly as a machine
//!   with no zero-copy chain behaves — the mirror of record's
//!   `set_force_no_encoder`. There is deliberately no force-*available* seam:
//!   a test that needs a chain uses a machine with one (or skips loudly).
//! * [`set_force_close_error`] injects a teardown failure so the
//!   stop-withholds-ack path is testable without a real transport (record's
//!   equivalent sabotages the sidecar on disk; a stub session has no disk
//!   surface to sabotage).
//! * A second `stream.start` while `Live` is refused upstream with
//!   `E_FORBIDDEN_STATE` and preserves the live session (§9.1: exactly one
//!   live stream).
//! * Closing with the force seam armed fails loudly (`E_NETWORK`) and still
//!   ends the live state — the record shape (no pipeline remains to continue
//!   with) — but the ack is withheld: `apply()` only acks on `Ok`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::rtmp::{parse_rtmp_url, spawn_publisher, PublisherHandle, PublisherState};
use crate::record::tap_path::Selection;

/// Forced-unavailable seam (tests only): when set, [`chain_available`]
/// reports no chain without touching the GPU — the mirror of record's
/// `FORCE_NO_ENCODER`.
static FORCE_NO_CHAIN: AtomicBool = AtomicBool::new(false);

/// Force (or release) the chain-less path. Test seam only.
pub fn set_force_no_chain(force: bool) {
    FORCE_NO_CHAIN.store(force, Ordering::SeqCst);
}

/// The seam state: true while forced chain-less.
pub fn force_no_chain() -> bool {
    FORCE_NO_CHAIN.load(Ordering::SeqCst)
}

/// Forced-teardown-failure seam (tests only): when set, [`StreamSession`]'s
/// close reports [`StreamError::Teardown`] instead of closing.
static FORCE_CLOSE_ERROR: AtomicBool = AtomicBool::new(false);

/// Force (or release) the teardown-failure path. Test seam only.
pub fn set_force_close_error(force: bool) {
    FORCE_CLOSE_ERROR.store(force, Ordering::SeqCst);
}

/// Whether this machine has a lawful streaming chain (SPEC §0.1 assumption 24
/// as rescoped by v0.4.2: recording alone holds the readback allowance, so a
/// streaming consumer with no zero-copy chain has no lawful path and
/// [`crate::record::tap_path::select_stream`] answers `None`).
///
/// The probe is honest: it builds the take geometry's pool against the device
/// the render loop published and keeps nothing (WU5 owns the take's pool —
/// see the module docs). A `None` device — a headless engine, or a build
/// where the render loop has not run — is a machine with no chain.
pub fn chain_available(device: &Option<Arc<wgpu::Device>>) -> bool {
    if FORCE_NO_CHAIN.load(Ordering::SeqCst) {
        return false;
    }
    let Some(d) = device else {
        return false;
    };
    crate::record::zerocopy_pool(d, crate::render::VIEW_W, crate::render::VIEW_H).is_ok()
}

/// Opening or closing a stream fails loudly, with stable tokens.
#[derive(Debug, thiserror::Error)]
pub enum StreamError {
    /// No zero-copy chain (seam-forced or genuinely absent): `E_NO_ZEROCOPY`.
    #[error("E_NO_ZEROCOPY: {0}")]
    NoChain(String),
    /// Teardown failed (seam-injected until the WU5 transport lands):
    /// `E_NETWORK`.
    #[error("E_NETWORK: {0}")]
    Teardown(String),
}

/// A live stream: the publish target and the published frame-path selection.
/// Deliberately `Send` (plain data + a `Send` publisher handle) so engine
/// state can hold it; the `!Send` half (when WU5 lands it) lives on the
/// publisher task.
pub struct StreamSession {
    endpoint: String,
    selection: Selection,
    closed: bool,
    /// The WU5 RTMP publisher: spawned at open (background dial, never
    /// blocking the directive path), fed off-thread over a bounded channel.
    /// `None` when there is no runtime (sync unit tests) or the endpoint is
    /// not an RTMP publish target — the WU4 bookkeeping shape is unchanged.
    publisher: Option<PublisherHandle>,
}

impl StreamSession {
    /// Open a session on `endpoint` with the probed `selection`. No I/O on the
    /// caller: the publisher task dials in the background (WU5); WU4 owns the
    /// bookkeeping the ack must wait for.
    pub fn open(endpoint: impl Into<String>, selection: Selection) -> Self {
        let endpoint = endpoint.into();
        let publisher = maybe_spawn_publisher(&endpoint);
        Self {
            endpoint,
            selection,
            closed: false,
            publisher,
        }
    }

    /// The publish target this session was opened on.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// The frame-path selection probed at start.
    pub fn selection(&self) -> Selection {
        self.selection
    }

    /// True once the engine closed the session (graceful close ran, or the
    /// force path dropped it).
    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// True while the session owns a live publisher task.
    pub fn has_publisher(&self) -> bool {
        self.publisher.is_some()
    }

    /// Queue H.264 bytes for publish. Never blocks (bounded `try_send` — the
    /// render loop can never wait on the socket); false = shed or no
    /// transport. A dead transport sheds; local playout never follows it.
    pub fn publish_video(&self, payload: Vec<u8>) -> bool {
        self.publisher
            .as_ref()
            .map(|p| p.try_publish_video(payload))
            .unwrap_or(false)
    }

    /// Queue AAC bytes for publish. Never blocks; false = shed or no
    /// transport.
    pub fn publish_audio(&self, payload: Vec<u8>) -> bool {
        self.publisher
            .as_ref()
            .map(|p| p.try_publish_audio(payload))
            .unwrap_or(false)
    }

    /// Bytes the transport currently holds (accepted, unwritten) — the live
    /// counter behind [`StreamSession::stream_buffer_ms`].
    pub fn buffered_bytes(&self) -> usize {
        self.publisher
            .as_ref()
            .map(|p| p.buffered_bytes())
            .unwrap_or(0)
    }

    /// `streamBufferMs`, honest: the transport's actual buffered bytes through
    /// the §9.4 envelope bitrate. Moves with load by construction (it divides
    /// the live counter); 0.0 with no transport.
    pub fn stream_buffer_ms(&self) -> f64 {
        self.publisher
            .as_ref()
            .map(|p| p.buffer_ms())
            .unwrap_or(0.0)
    }

    /// Transport liveness (`Live` / `Reconnecting` / `Closed`). The engine's
    /// `StreamState` stays `Live` while this reads `Reconnecting` — that
    /// split IS the survival guarantee (§9.5). `Closed` with no transport.
    pub fn publisher_state(&self) -> PublisherState {
        self.publisher
            .as_ref()
            .map(|p| p.publisher_state())
            .unwrap_or(PublisherState::Closed)
    }

    /// Close the session gracefully: the transport is gone BEFORE this returns
    /// (WU5) — so the ack that follows is honest. With the close-error seam
    /// armed this reports [`StreamError::Teardown`] and closes nothing.
    ///
    /// Async by construction: awaits the publisher's `shutdown_and_wait`
    /// (tokio sleep, never blocking) so the directive path never stalls the
    /// executor — `stream.stop` carries a bounded-wait assertion proving it.
    pub async fn stop_and_close(&mut self) -> Result<(), StreamError> {
        if FORCE_CLOSE_ERROR.load(Ordering::SeqCst) {
            return Err(StreamError::Teardown(
                "stream teardown failed (injected): transport did not confirm shutdown".into(),
            ));
        }
        if let Some(publisher) = self.publisher.take() {
            // Bounded wait for the socket to close (the record
            // `stop_and_finish` shape): past it the publisher is abandoned
            // as-is but the ack is still withheld — a window with no graceful
            // shutdown behind it must not ack.
            if !publisher
                .shutdown_and_wait(PublisherHandle::stop_timeout())
                .await
            {
                self.closed = true;
                return Err(StreamError::Teardown(
                    "E_NETWORK: stream teardown timed out: transport did not confirm shutdown"
                        .into(),
                ));
            }
        }
        self.closed = true;
        Ok(())
    }

    /// Drop the session WITHOUT a graceful close (`force=true` immediate
    /// stop): the transport is abandoned as-is, the session over.
    pub fn abandon(&mut self) {
        if let Some(publisher) = self.publisher.take() {
            // Fire-and-forget: signal only, never wait — the task exits on
            // its own; the show stops now (and never blocks the executor).
            publisher.shutdown_signal();
        }
        self.closed = true;
    }
}

/// Spawn the WU5 publisher for `endpoint`, if one can exist here: a tokio
/// runtime must be running (the directive path always has one; sync unit
/// tests do not) and the endpoint must parse as an RTMP publish target
/// (anything else keeps the WU4 transport-less shape).
fn maybe_spawn_publisher(endpoint: &str) -> Option<PublisherHandle> {
    if tokio::runtime::Handle::try_current().is_err() {
        return None;
    }
    let url = parse_rtmp_url(endpoint).ok()?;
    Some(spawn_publisher(url))
}

// ---------------------------------------------------------------------------
// Live feed (WU5 FIX round 2): lock-free encode + bounded publish.
// ---------------------------------------------------------------------------
//
// Finding 3 (session lock held across encode): the loop must never hold the
// `stream_session` Mutex across `encode_pixel_buffer` — `stream.stop` takes
// that same lock, so a slow encode stalls the ack. The split below is the
// fix: [`encode_stream_frame`] touches NO session state (surface +
// loop-owned encoder only — provably lock-free: it takes no lock at all, so
// calling it while holding `stream_session` cannot block), and
// [`publish_stream_frame`] holds the session only across bounded `try_send`s.
// [`feed_stream_surface`] is the same two phases back to back (identical
// counting) for the stream-only path and existing tests.

/// One encoded stream frame, ready to publish (no session touched).
pub struct EncodedStreamPayload {
    /// FLV-ready access units (still length-prefixed; [`flv_video_tag`] wraps).
    pub units: Vec<nbe_decode::encode::EncodedUnit>,
    /// AVC sequence header, `Some` only when the caller still needs one
    /// (`need_seq`) AND the encoder has parameter sets. `None` keeps the
    /// caller's `seq_sent` false so the next frame retries (no keyframe yet).
    pub seq_header: Option<Vec<u8>>,
}

/// Encode one drawn Surface WITHOUT touching the session: opens the
/// loop-owned encoder lazily (surface geometry), encodes zero-copy, snapshots
/// the parameter sets. Returns `(encode_ms, payload)` — `payload=None` is the
/// G1 drop (open/encode failure counts ONE stream drop, never record nor
/// View). Lock-free by construction: no session, no lock, no await.
pub fn encode_stream_frame(
    surface: &std::sync::Arc<nbe_decode::zerocopy::SharedSurface>,
    encoder: &mut Option<nbe_decode::encode::EncodeSession>,
    need_seq: bool,
    stream_drops: &std::sync::atomic::AtomicU64,
) -> (f64, Option<EncodedStreamPayload>) {
    use std::sync::atomic::Ordering;
    let started = std::time::Instant::now();
    let ms = || started.elapsed().as_secs_f64() * 1000.0;
    if encoder.is_none() {
        let (w, h) = surface.dimensions();
        match nbe_decode::encode::EncodeSession::open(w, h, 30, 8_000_000) {
            Ok(enc) => *encoder = Some(enc),
            Err(_) => {
                stream_drops.fetch_add(1, Ordering::SeqCst);
                return (0.0, None);
            }
        }
    }
    let enc = encoder.as_mut().expect("encoder opened above");
    let units = match enc.encode_pixel_buffer(surface.pixel_buffer()) {
        Ok(u) => u,
        Err(_) => {
            stream_drops.fetch_add(1, Ordering::SeqCst);
            return (ms(), None);
        }
    };
    let seq_header = if need_seq {
        match enc.parameter_sets() {
            Some((sps, pps)) => avc_sequence_header(&sps, &pps),
            None => None,
        }
    } else {
        None
    };
    (ms(), Some(EncodedStreamPayload { units, seq_header }))
}

/// Publish one encoded frame through the session. Holds NOTHING but the
/// caller's `&StreamSession` across bounded `try_send`s — never encodes, never
/// blocks. A sequence-header shed counts ONE stream drop like any shed unit;
/// attempting it marks `seq_sent` either way (the transport's send-time cache
/// replays the header after every redial without feeder help). Per-unit sheds
/// count per unit. Returns the publish cost in ms for `stream_tap_ms`.
pub fn publish_stream_frame(
    session: &StreamSession,
    seq_header: Option<Vec<u8>>,
    seq_sent: &mut bool,
    units: &[nbe_decode::encode::EncodedUnit],
    stream_drops: &std::sync::atomic::AtomicU64,
) -> f64 {
    use std::sync::atomic::Ordering;
    let started = std::time::Instant::now();
    if let Some(seq) = seq_header {
        if !session.publish_video(seq) {
            stream_drops.fetch_add(1, Ordering::SeqCst);
        }
        *seq_sent = true;
    }
    let mut shed = 0usize;
    for u in units {
        if !session.publish_video(flv_video_tag(u)) {
            shed += 1;
        }
    }
    if shed > 0 {
        stream_drops.fetch_add(shed as u64, Ordering::SeqCst);
    }
    started.elapsed().as_secs_f64() * 1000.0
}

/// Convert one encoder access unit (AVCC length-prefixed NALs) to an FLV
/// video tag payload: `[frame|codec, avc-type, cts×3] + NALs`. Keyframes
/// (`is_keyframe`) ride `0x17`, inter frames `0x27`; `avc-type` is always
/// `0x01` (NALU, never sequence — sequence headers ride separately at
/// connect). CTS is zero: the transport timestamps at the RTMP layer, so the
/// FLV composition offset stays neutral rather than double-counting.
pub fn flv_video_tag(unit: &nbe_decode::encode::EncodedUnit) -> Vec<u8> {
    let mut out = Vec::with_capacity(5 + unit.data.len());
    out.push(if unit.is_keyframe { 0x17 } else { 0x27 });
    out.push(0x01);
    out.extend_from_slice(&[0x00, 0x00, 0x00]);
    out.extend_from_slice(&unit.data);
    out
}

/// Publish already-encoded units through the session (bounded try_send —
/// never blocks). Returns `(admitted, shed)`: shed frames were dropped by
/// the transport (channel full / no transport) and the caller counts them
/// on the stream counter. Pure publish path — no Surface, no encode — shared
/// by the Surface feed below and by deterministic synthetic e2e coverage.
pub fn publish_units(
    session: &StreamSession,
    units: &[nbe_decode::encode::EncodedUnit],
) -> (usize, usize) {
    let mut admitted = 0usize;
    let mut shed = 0usize;
    for u in units {
        if session.publish_video(flv_video_tag(u)) {
            admitted += 1;
        } else {
            shed += 1;
        }
    }
    (admitted, shed)
}

/// Build an FLV AVC sequence header (`0x17 0x00` + avcC) from REAL parameter
/// sets (the encoder's own SPS/PPS via [`nbe_decode::encode::EncodeSession::parameter_sets`]).
/// Profile/compat/level are copied from the SPS (`sps[1..4]`), never
/// hardcoded — a hardcoded triple that disagrees with the sets fails a real
/// ingest's avcC parse. Refuses empty or mistyped sets loudly (`None`): a
/// sequence header with no parameters is worse than none (it poisons the
/// peer's track setup). The transport's static
/// [`super::rtmp::video_sequence_header`] covers synthetically-fed sessions;
/// THIS covers the live encoder.
pub fn avc_sequence_header(sps: &[u8], pps: &[u8]) -> Option<Vec<u8>> {
    if sps.len() < 4 || pps.is_empty() {
        return None;
    }
    // NAL validation by type, not by exact header byte: real VideoToolbox
    // SPS NALs arrive with nal_ref_idc != 3 (0x27 observed on hardware — a
    // strict `== 0x67` check rejects them and no live stream ever emits its
    // sequence header). forbidden_zero_bit must be 0, type must be 7 (SPS).
    if sps[0] & 0x80 != 0 || sps[0] & 0x1F != 7 {
        return None;
    }
    // PPS likewise by type (the capture already selects type 8; this keeps
    // the constructor honest on its own).
    if pps[0] & 0x80 != 0 || pps[0] & 0x1F != 8 {
        return None;
    }
    let mut out = vec![
        0x17, 0x00, 0x00, 0x00, 0x00, // FLV video tag header: keyframe, AVC, sequence
        0x01, sps[1], sps[2], sps[3], // avcC version 1 + profile/compat/level from the SPS
        0xFF, 0xE1, // 4-byte NALU lengths, one SPS
    ];
    out.extend_from_slice(&(sps.len() as u16).to_be_bytes());
    out.extend_from_slice(sps);
    out.push(0x01); // one PPS
    out.extend_from_slice(&(pps.len() as u16).to_be_bytes());
    out.extend_from_slice(pps);
    Some(out)
}

/// Feed one drawn Surface to the stream: encode zero-copy, publish, measure.
///
/// Same two phases as [`encode_stream_frame`] + [`publish_stream_frame`] back
/// to back (identical counting) — the deterministic hook tests call this; the
/// loop calls the phases separately so the session lock spans only the publish.
///
/// * `surface=None` is the G1 drop: the View drew regardless (to built-in),
///   the stream takes nothing — counts ONE stream drop, returns `0.0`, never
///   touches `skipped_record_frames` nor View drops, never blocks.
/// * `Some` encodes via `encode_pixel_buffer` (no readback/rgba anywhere on
///   this path — the signature cannot express one), converts to FLV, and
///   publishes bounded. Encode failure counts ONE stream drop (the frame, not
///   the take); per-unit publish sheds count per unit.
/// * The FIRST frame whose encoder has parameter sets emits the AVC sequence
///   header ahead of the media (a real ingest establishes the H.264 track
///   from avcC — without it the video is unparseable). `seq_sent` tracks
///   that; a seq-header shed counts ONE stream drop like any shed unit. The
///   header is the session's first video payload, so the transport's
///   send-time cache replays it after every redial without feeder help.
/// * Returns the feed cost in ms (encode + publish) for the loop's
///   `stream_tap_ms` — kept OFF the render budget by construction (measured
///   here, only ever added to the stream counter).
/// * `encoder` is `Option` so the loop owns it lazily (opened at View
///   geometry on first Live frame): `None` opens, open failure counts ONE
///   drop rather than failing the take.
pub fn feed_stream_surface(
    surface: Option<std::sync::Arc<nbe_decode::zerocopy::SharedSurface>>,
    encoder: &mut Option<nbe_decode::encode::EncodeSession>,
    seq_sent: &mut bool,
    session: &StreamSession,
    stream_drops: &std::sync::atomic::AtomicU64,
) -> f64 {
    use std::sync::atomic::Ordering;
    let Some(surface) = surface else {
        stream_drops.fetch_add(1, Ordering::SeqCst);
        return 0.0;
    };
    let (encode_ms, payload) = encode_stream_frame(&surface, encoder, !*seq_sent, stream_drops);
    let Some(payload) = payload else {
        return encode_ms;
    };
    encode_ms
        + publish_stream_frame(
            session,
            payload.seq_header,
            seq_sent,
            &payload.units,
            stream_drops,
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn force_seam_reports_chain_less_without_touching_hardware() {
        set_force_no_chain(true);
        assert!(!chain_available(&None));
        set_force_no_chain(false);
    }

    #[tokio::test]
    async fn close_error_seam_fails_loudly_with_the_network_token() {
        let sel = crate::record::tap_path::select_stream(true).unwrap();
        let mut s = StreamSession::open("rtmp://example/live", sel);
        set_force_close_error(true);
        let err = s.stop_and_close().await.expect_err("armed seam must fail");
        assert!(err.to_string().contains("E_NETWORK"));
        assert!(!s.is_closed(), "failed close closes nothing");
        set_force_close_error(false);
        s.stop_and_close().await.expect("released seam must close");
        assert!(s.is_closed());
    }

    #[test]
    fn publish_attempts_seq_first_and_counts_every_shed() {
        // Transport-less session (non-RTMP endpoint → no publisher): every
        // publish sheds. Deterministic, no hardware: pins that the seq header
        // is attempted (marks sent) and every shed unit — header + media —
        // counts exactly one stream drop each.
        use std::sync::atomic::AtomicU64;
        let sel = crate::record::tap_path::select_stream(true).unwrap();
        let s = StreamSession::open("not-a-publish-target", sel);
        assert!(!s.has_publisher());
        let drops = AtomicU64::new(0);
        let mut seq_sent = false;
        let units = vec![
            nbe_decode::encode::EncodedUnit {
                data: vec![0x65, 0x01],
                is_keyframe: true,
                pts_seconds: 0.0,
            },
            nbe_decode::encode::EncodedUnit {
                data: vec![0x41, 0x02],
                is_keyframe: false,
                pts_seconds: 1.0 / 30.0,
            },
        ];
        let ms = publish_stream_frame(
            &s,
            Some(vec![0x17, 0x00, 0x00, 0x00, 0x00]),
            &mut seq_sent,
            &units,
            &drops,
        );
        assert!(
            seq_sent,
            "attempting the header marks it sent (shed or not)"
        );
        assert_eq!(
            drops.load(Ordering::SeqCst),
            3,
            "header shed + 2 unit sheds count one drop each"
        );
        assert!(ms >= 0.0);
    }

    #[test]
    fn feed_with_no_surface_is_one_stream_drop_not_a_record_skip() {
        use std::sync::atomic::AtomicU64;
        let sel = crate::record::tap_path::select_stream(true).unwrap();
        let s = StreamSession::open("not-a-publish-target", sel);
        let drops = AtomicU64::new(0);
        let mut encoder = None;
        let mut seq_sent = false;
        let ms = feed_stream_surface(None, &mut encoder, &mut seq_sent, &s, &drops);
        assert_eq!(ms, 0.0);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert!(!seq_sent, "a dropped frame sends nothing");
        assert!(encoder.is_none(), "a dropped frame opens nothing");
    }

    #[test]
    fn avc_sequence_header_refuses_empty_or_mistyped_sets() {
        assert!(avc_sequence_header(&[], &[]).is_none());
        assert!(avc_sequence_header(&[0x67, 0x64], &[0x68]).is_none());
        assert!(avc_sequence_header(&[0x65, 0x64, 0x00, 0x1F], &[0x68]).is_none());
        // PPS mistyped (type 7 in the PPS slot) is refused too.
        assert!(avc_sequence_header(&[0x67, 0x64, 0x00, 0x1F], &[0x67]).is_none());
        let sps = vec![0x67, 0x64, 0x00, 0x1F, 0xAA, 0xBB];
        let pps = vec![0x68, 0xCC];
        let seq = avc_sequence_header(&sps, &pps).expect("valid sets build");
        assert!(seq.starts_with(&[0x17, 0x00, 0x00, 0x00, 0x00]));
        // Profile/compat/level copied from the SPS, never hardcoded.
        assert_eq!(&seq[5..10], &[0x01, 0x64, 0x00, 0x1F, 0xFF]);
        assert!(seq.windows(sps.len()).any(|w| w == sps.as_slice()));
        assert!(seq.windows(pps.len()).any(|w| w == pps.as_slice()));
    }

    #[test]
    fn avc_sequence_header_accepts_real_hardware_sps_shapes() {
        // Observed on hardware: VideoToolbox emits the SPS with nal_ref_idc
        // 1 (0x27), not 3 (0x67). A strict exact-byte check rejects every
        // real stream's header — validation is by NAL type instead.
        let sps = vec![0x27, 0x64, 0x00, 0x28, 0xAC, 0x13];
        let pps = vec![0x28, 0xEE, 0x1F, 0x2C];
        let seq = avc_sequence_header(&sps, &pps).expect("hardware sets build");
        assert!(seq.starts_with(&[0x17, 0x00, 0x00, 0x00, 0x00]));
        assert_eq!(&seq[5..10], &[0x01, 0x64, 0x00, 0x28, 0xFF]);
    }

    #[test]
    fn flv_video_tag_marks_keyframes_and_never_sequence() {
        let key = nbe_decode::encode::EncodedUnit {
            data: vec![0x01, 0x02],
            is_keyframe: true,
            pts_seconds: 0.0,
        };
        let inter = nbe_decode::encode::EncodedUnit {
            data: vec![0x03],
            is_keyframe: false,
            pts_seconds: 1.0,
        };
        let kt = flv_video_tag(&key);
        let it = flv_video_tag(&inter);
        assert_eq!(&kt[..5], &[0x17, 0x01, 0x00, 0x00, 0x00]);
        assert_eq!(&it[..5], &[0x27, 0x01, 0x00, 0x00, 0x00]);
        assert_eq!(&kt[5..], &[0x01, 0x02]);
    }
}
