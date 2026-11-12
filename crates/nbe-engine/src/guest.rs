//! WHIP guests through a media server (sprint PR D, on the spike's decision
//! of 2026-11-12: a separate process, MediaMTX, no WebRTC library).
//!
//! The guest's browser publishes over WHIP to the media server. The engine
//! never speaks WebRTC: for each guest it runs three `ffmpeg` children
//! against the media server's RTSP side, each owned by one thread here.
//!
//! 1. **Video in.** Pulls `rtsp://<server>/<path>`, decodes whatever the
//!    browser sent (H.264 or VP8), scales to 1280x720 BGRA at 30 fps, and
//!    writes raw frames to its stdout. The thread reads whole frames into the
//!    guest's [`FrameMailbox`]: the camera's live layer, frame sync and loss
//!    handling (`live.rs`) are the guest's too.
//! 2. **Audio in.** Pulls the same path's audio as 48 kHz stereo float into
//!    the guest's [`LiveAudio`], which the audio graph reads on the guest's
//!    own bus (SPEC §8.6).
//! 3. **Return out.** The graph renders the guest's mix-minus — the program
//!    without that guest, structurally — after every program block into a
//!    lock-free ring; this thread drains it into an encoder child that
//!    publishes Opus to `rtsp://<server>/<path>-return`, which the guest's
//!    browser plays over WHEP.
//!
//! A child that exits (the guest has not published yet, or left, or the
//! server restarted) is restarted after [`RESPAWN_DELAY`]. A guest with no
//! frames on the View is a lost live source: the fallback slate, as for a
//! camera. Nothing here runs on the render thread or the audio thread.

use crate::audio_ring::{RingConsumer, RingProducer};
use crate::live::{FrameMailbox, LiveAudio, LiveFrame};
use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How long a dead child waits before it is started again.
pub const RESPAWN_DELAY: Duration = Duration::from_millis(500);
/// The guest picture the engine works with.
pub const GUEST_W: u32 = 1280;
pub const GUEST_H: u32 = 720;

/// Where a guest's streams live on the media server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuestRoutes {
    /// The media server path the guest publishes to (from its WHIP URL).
    pub path: String,
    pub rtsp_in: String,
    pub rtsp_return: String,
    /// What the guest's browser plays its return from.
    pub whep_return: String,
}

/// The media server path in a WHIP URL: `http(s)://host[:port]/<path>/whip`.
pub fn whip_path(whip_url: &str) -> Option<String> {
    let rest = whip_url.split_once("://")?.1;
    let (_, path) = rest.split_once('/')?;
    let path = path.trim_end_matches('/');
    let path = if path == "whip" {
        ""
    } else {
        path.strip_suffix("/whip").unwrap_or(path)
    };
    (!path.is_empty() && !path.contains("..")).then(|| path.to_string())
}

/// The routes for a guest: the media server is `NBE_MEDIA_SERVER`
/// (default `127.0.0.1`), RTSP on `NBE_MEDIA_RTSP_PORT` (8554); the WHEP
/// URL keeps the WHIP URL's own scheme, host and port.
pub fn routes(whip_url: &str) -> Option<GuestRoutes> {
    let path = whip_path(whip_url)?;
    let server = std::env::var("NBE_MEDIA_SERVER").unwrap_or_else(|_| "127.0.0.1".into());
    let port = std::env::var("NBE_MEDIA_RTSP_PORT").unwrap_or_else(|_| "8554".into());
    let (scheme, rest) = whip_url.split_once("://")?;
    let authority = rest.split('/').next()?;
    Some(GuestRoutes {
        rtsp_in: format!("rtsp://{server}:{port}/{path}"),
        rtsp_return: format!("rtsp://{server}:{port}/{path}-return"),
        whep_return: format!("{scheme}://{authority}/{path}-return/whep"),
        path,
    })
}

fn ffmpeg() -> String {
    std::env::var("NBE_FFMPEG").unwrap_or_else(|_| "ffmpeg".into())
}

/// Counters a session keeps, for the log and the tests.
#[derive(Debug, Default)]
pub struct GuestStats {
    pub video_spawns: AtomicU64,
    pub audio_spawns: AtomicU64,
    pub return_spawns: AtomicU64,
    pub frames_in: AtomicU64,
    pub return_frames_out: AtomicU64,
}

/// One connected guest's three children and their threads. Dropping it
/// stops them.
pub struct GuestSession {
    pub guest_id: String,
    pub routes: GuestRoutes,
    pub stats: Arc<GuestStats>,
    stop: Arc<AtomicBool>,
    children: Vec<Arc<Mutex<Option<Child>>>>,
    threads: Vec<std::thread::JoinHandle<()>>,
}

impl std::fmt::Debug for GuestSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GuestSession")
            .field("guest_id", &self.guest_id)
            .field("routes", &self.routes)
            .finish_non_exhaustive()
    }
}

impl GuestSession {
    pub fn start(
        guest_id: &str,
        routes: GuestRoutes,
        mailbox: Arc<FrameMailbox>,
        audio_in: (Arc<LiveAudio>, RingProducer),
        return_out: RingConsumer,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stats = Arc::new(GuestStats::default());
        let mut children = Vec::new();
        let mut threads = Vec::new();

        // 1. Video in.
        {
            let slot: Arc<Mutex<Option<Child>>> = Arc::default();
            children.push(slot.clone());
            let (stop, stats, url) = (stop.clone(), stats.clone(), routes.rtsp_in.clone());
            let gid = guest_id.to_string();
            threads.push(spawn(&format!("nbe-guest-video-{gid}"), move || {
                let frame_bytes = (GUEST_W * GUEST_H * 4) as usize;
                let mut seq = 0u64;
                while !stop.load(Ordering::SeqCst) {
                    let args = [
                        "-v",
                        "error",
                        "-rtsp_transport",
                        "tcp",
                        "-fflags",
                        "nobuffer",
                        "-flags",
                        "low_delay",
                        "-i",
                        &url,
                        "-an",
                        "-vf",
                        &format!("scale={GUEST_W}:{GUEST_H},fps=30"),
                        "-pix_fmt",
                        "bgra",
                        "-f",
                        "rawvideo",
                        "pipe:1",
                    ];
                    let Some(mut out) = start_child(&slot, &args, true, false) else {
                        std::thread::sleep(RESPAWN_DELAY);
                        continue;
                    };
                    stats.video_spawns.fetch_add(1, Ordering::Relaxed);
                    let mut buf = vec![0u8; frame_bytes];
                    while out.read_exact(&mut buf).is_ok() {
                        seq += 1;
                        stats.frames_in.fetch_add(1, Ordering::Relaxed);
                        mailbox.publish(LiveFrame {
                            width: GUEST_W,
                            height: GUEST_H,
                            bgra: std::mem::replace(&mut buf, vec![0u8; frame_bytes]),
                            seq,
                        });
                    }
                    reap(&slot);
                    if !stop.load(Ordering::SeqCst) {
                        tracing::info!(guest = %gid, "guest video ended; restarting");
                        std::thread::sleep(RESPAWN_DELAY);
                    }
                }
            }));
        }

        // 2. Audio in.
        {
            let slot: Arc<Mutex<Option<Child>>> = Arc::default();
            children.push(slot.clone());
            let (stop, stats, url) = (stop.clone(), stats.clone(), routes.rtsp_in.clone());
            let (input, mut producer) = audio_in;
            let gid = guest_id.to_string();
            threads.push(spawn(&format!("nbe-guest-audio-{gid}"), move || {
                // 10 ms of 48 kHz stereo float.
                let mut bytes = vec![0u8; 480 * 2 * 4];
                let mut samples = vec![0f32; 480 * 2];
                while !stop.load(Ordering::SeqCst) {
                    let args = [
                        "-v",
                        "error",
                        "-rtsp_transport",
                        "tcp",
                        "-fflags",
                        "nobuffer",
                        "-flags",
                        "low_delay",
                        "-i",
                        &url,
                        "-vn",
                        "-ac",
                        "2",
                        "-ar",
                        "48000",
                        "-f",
                        "f32le",
                        "pipe:1",
                    ];
                    let Some(mut out) = start_child(&slot, &args, true, false) else {
                        std::thread::sleep(RESPAWN_DELAY);
                        continue;
                    };
                    stats.audio_spawns.fetch_add(1, Ordering::Relaxed);
                    while out.read_exact(&mut bytes).is_ok() {
                        for (s, b) in samples.iter_mut().zip(bytes.as_chunks::<4>().0) {
                            *s = f32::from_le_bytes(*b);
                        }
                        let pushed = producer.push(&samples);
                        input
                            .frames_in
                            .fetch_add((pushed / 2) as u64, Ordering::Relaxed);
                    }
                    reap(&slot);
                    if !stop.load(Ordering::SeqCst) {
                        tracing::info!(guest = %gid, "guest audio ended; restarting");
                        std::thread::sleep(RESPAWN_DELAY);
                    }
                }
            }));
        }

        // 3. Return out.
        {
            let slot: Arc<Mutex<Option<Child>>> = Arc::default();
            children.push(slot.clone());
            let (stop, stats, url) = (stop.clone(), stats.clone(), routes.rtsp_return.clone());
            let mut consumer = return_out;
            let gid = guest_id.to_string();
            threads.push(spawn(&format!("nbe-guest-return-{gid}"), move || {
                let mut samples = vec![0f32; 960 * 2];
                let mut bytes: Vec<u8> = Vec::with_capacity(960 * 2 * 4);
                while !stop.load(Ordering::SeqCst) {
                    let args = [
                        "-v",
                        "error",
                        "-f",
                        "f32le",
                        "-ar",
                        "48000",
                        "-ac",
                        "2",
                        "-i",
                        "pipe:0",
                        "-c:a",
                        "libopus",
                        "-b:a",
                        "96k",
                        "-application",
                        "lowdelay",
                        "-f",
                        "rtsp",
                        "-rtsp_transport",
                        "tcp",
                        &url,
                    ];
                    let Some(mut stdin) = start_writer(&slot, &args) else {
                        std::thread::sleep(RESPAWN_DELAY);
                        continue;
                    };
                    stats.return_spawns.fetch_add(1, Ordering::Relaxed);
                    // What queued while no encoder ran is stale: start fresh.
                    let stale = consumer.fill_frames();
                    consumer.skip_frames(stale);
                    loop {
                        if stop.load(Ordering::SeqCst) {
                            break;
                        }
                        let n = consumer.pop_into(&mut samples);
                        if n == 0 {
                            std::thread::sleep(Duration::from_millis(5));
                            continue;
                        }
                        bytes.clear();
                        for s in &samples[..n] {
                            bytes.extend_from_slice(&s.to_le_bytes());
                        }
                        if stdin.write_all(&bytes).is_err() {
                            break;
                        }
                        stats
                            .return_frames_out
                            .fetch_add((n / 2) as u64, Ordering::Relaxed);
                    }
                    drop(stdin);
                    reap(&slot);
                    if !stop.load(Ordering::SeqCst) {
                        tracing::info!(guest = %gid, "guest return encoder ended; restarting");
                        std::thread::sleep(RESPAWN_DELAY);
                    }
                }
            }));
        }

        tracing::info!(guest = %guest_id, rtsp_in = %routes.rtsp_in, rtsp_return = %routes.rtsp_return,
            whep_return = %routes.whep_return, "guest session started");
        Self {
            guest_id: guest_id.to_string(),
            routes,
            stats,
            stop,
            children,
            threads,
        }
    }
}

impl crate::live::CaptureHandle for GuestSession {}

impl Drop for GuestSession {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        for slot in &self.children {
            if let Some(child) = slot.lock().unwrap().as_mut() {
                let _ = child.kill();
            }
        }
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
        tracing::info!(guest = %self.guest_id, "guest session stopped");
    }
}

fn spawn(name: &str, f: impl FnOnce() + Send + 'static) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name(name.into())
        .spawn(f)
        .expect("spawn a guest thread")
}

/// Start a reading child, keep it in `slot`, and hand back its stdout.
fn start_child(
    slot: &Arc<Mutex<Option<Child>>>,
    args: &[&str],
    stdout: bool,
    stdin: bool,
) -> Option<std::process::ChildStdout> {
    let mut cmd = Command::new(ffmpeg());
    cmd.args(args)
        .stdin(if stdin { Stdio::piped() } else { Stdio::null() })
        .stdout(if stdout {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stderr(Stdio::null());
    let mut child = cmd
        .spawn()
        .map_err(|e| tracing::warn!(error = %e, "ffmpeg would not start"))
        .ok()?;
    let out = child.stdout.take();
    *slot.lock().unwrap() = Some(child);
    out
}

/// Start a writing child (the return encoder) and hand back its stdin.
fn start_writer(
    slot: &Arc<Mutex<Option<Child>>>,
    args: &[&str],
) -> Option<std::process::ChildStdin> {
    let mut child = Command::new(ffmpeg())
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| tracing::warn!(error = %e, "ffmpeg would not start"))
        .ok()?;
    let input = child.stdin.take();
    *slot.lock().unwrap() = Some(child);
    input
}

fn reap(slot: &Arc<Mutex<Option<Child>>>) {
    if let Some(mut child) = slot.lock().unwrap().take() {
        let _ = child.kill();
        let _ = child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_whip_url_names_its_media_server_path() {
        assert_eq!(
            whip_path("http://127.0.0.1:8889/guest1/whip").as_deref(),
            Some("guest1")
        );
        assert_eq!(
            whip_path("https://media.example:8889/show/g2/whip").as_deref(),
            Some("show/g2")
        );
        assert_eq!(whip_path("http://h:8889/whip"), None);
        assert_eq!(whip_path("not a url"), None);
        let r = routes("http://10.0.0.5:8889/guest1/whip").unwrap();
        assert!(r.rtsp_return.ends_with("/guest1-return"));
        assert_eq!(r.whep_return, "http://10.0.0.5:8889/guest1-return/whep");
    }
}
