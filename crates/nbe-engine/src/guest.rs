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
//! server restarted) is restarted after [`RESPAWN_DELAY`], except a video
//! reader that ended without a picture: it waits for the guest's audio
//! reader to restart ([`wait_for_audio_reader_restart`]). A guest with no
//! frames on the View is a lost live source: the fallback
//! slate, as for a camera. Nothing here runs on the render thread or the
//! audio thread.

use crate::audio_ring::{RingConsumer, RingProducer};
use crate::live::{FrameMailbox, LiveAudio, LiveFrame};
use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

/// How long a dead child waits before it is started again.
pub const RESPAWN_DELAY: Duration = Duration::from_millis(500);
/// A reader's RTSP socket timeout, in microseconds (ffmpeg's `-timeout`).
const READER_TIMEOUT_US: &str = "5000000";
/// How long a killed child gets to exit before it is given up on.
const REAP_DEADLINE: Duration = Duration::from_secs(2);
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
    /// 10 ms blocks the audio reader has read (pushed to the graph or not):
    /// for the log and the tests.
    pub audio_blocks_in: AtomicU64,
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
                        // A reader whose input goes quiet for 5 s ends (and
                        // is restarted while the session runs). It is also
                        // how a reader left behind by a killed engine ends
                        // when no data flows to fail its write (item 7c).
                        "-timeout",
                        READER_TIMEOUT_US,
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
                    let Some(mut out) = start_child(&slot, &stop, &args, true, false) else {
                        if stop.load(Ordering::SeqCst) {
                            break;
                        }
                        std::thread::sleep(RESPAWN_DELAY);
                        continue;
                    };
                    stats.video_spawns.fetch_add(1, Ordering::Relaxed);
                    let mut buf = vec![0u8; frame_bytes];
                    let mut pictured = false;
                    while out.read_exact(&mut buf).is_ok() {
                        pictured = true;
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
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    if pictured {
                        tracing::info!(guest = %gid, "guest video ended; restarting");
                        std::thread::sleep(RESPAWN_DELAY);
                    } else {
                        wait_for_audio_reader_restart(&gid, &stop, &stats);
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
                        // A reader whose input goes quiet for 5 s ends (and
                        // is restarted while the session runs). It is also
                        // how a reader left behind by a killed engine ends
                        // when no data flows to fail its write (item 7c).
                        "-timeout",
                        READER_TIMEOUT_US,
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
                    let Some(mut out) = start_child(&slot, &stop, &args, true, false) else {
                        if stop.load(Ordering::SeqCst) {
                            break;
                        }
                        std::thread::sleep(RESPAWN_DELAY);
                        continue;
                    };
                    stats.audio_spawns.fetch_add(1, Ordering::Relaxed);
                    while out.read_exact(&mut bytes).is_ok() {
                        stats.audio_blocks_in.fetch_add(1, Ordering::Relaxed);
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
                    let Some(mut stdin) = start_writer(&slot, &stop, &args) else {
                        if stop.load(Ordering::SeqCst) {
                            break;
                        }
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
    /// Stops every child and reaps it (the second key's item 7a). The stop
    /// flag is set before the slots are locked, and a thread stores a child
    /// it spawned only under its slot's lock after checking the flag
    /// ([`start_child`]): so a child is either in its slot when it is killed
    /// here, or killed and reaped by the thread that spawned it. Each thread
    /// reaps the child it ran when its read or write ends; any child still in
    /// a slot after the joins is reaped here. None is left running or
    /// defunct.
    ///
    /// A poisoned slot lock (a panic while a thread held it) is taken
    /// anyway: its `Option<Child>` is whole either way, and a panic here
    /// would abort the engine and skip the kills (item 4b of the user's
    /// ruling of 2026-11-15 20:55).
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        for slot in &self.children {
            if let Some(child) = lock(slot).as_mut() {
                // A signal, not a wait: the lock is held only for the send.
                let _ = Reap::kill(child);
            }
        }
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
        for slot in &self.children {
            reap(slot);
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

/// What reaping needs of a child: a kill, and a wait that does not block.
/// `Child` is the one the engine uses; the tests stand in children that
/// exit late or never.
trait Reap {
    fn kill(&mut self) -> std::io::Result<()>;
    /// Whether the child has exited (and was reaped), without blocking.
    fn exited(&mut self) -> std::io::Result<bool>;
}

impl Reap for Child {
    fn kill(&mut self) -> std::io::Result<()> {
        Child::kill(self)
    }
    fn exited(&mut self) -> std::io::Result<bool> {
        Ok(self.try_wait()?.is_some())
    }
}

/// A slot's lock, poisoned or not (item 4b of the user's ruling of
/// 2026-11-15 20:55): the slot holds only an `Option`, whole either way, and
/// a guest's teardown must not panic.
fn lock<C>(slot: &Mutex<Option<C>>) -> MutexGuard<'_, Option<C>> {
    slot.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Kill a child and wait for it, at most `deadline` (item 4a of the user's
/// ruling of 2026-11-15 20:55). Never called with a slot lock held. A child that a kill cannot end
/// (one stuck in the kernel) is given up on, logged, and left unreaped: the
/// guest's teardown goes on without it.
fn kill_and_wait<C: Reap>(child: &mut C, deadline: Duration) -> bool {
    let _ = child.kill();
    let started = Instant::now();
    loop {
        match child.exited() {
            Ok(true) => return true,
            Ok(false) if started.elapsed() < deadline => {
                std::thread::sleep(Duration::from_millis(5));
            }
            Ok(false) => {
                tracing::warn!(
                    ?deadline,
                    "an ffmpeg child did not exit after its kill; left unreaped"
                );
                return false;
            }
            Err(e) => {
                tracing::warn!(error = %e, "an ffmpeg child could not be waited for");
                return false;
            }
        }
    }
}

/// Keep a child in its slot, unless the session is stopping: then it is
/// killed and reaped here, never stored, so a session's drop cannot miss it.
/// The stop check and the store are under the slot's lock; the kill and the
/// wait are not.
fn keep<C: Reap>(slot: &Mutex<Option<C>>, stop: &AtomicBool, mut child: C) -> bool {
    {
        let mut held = lock(slot);
        if !stop.load(Ordering::SeqCst) {
            *held = Some(child);
            return true;
        }
    }
    kill_and_wait(&mut child, REAP_DEADLINE);
    false
}

/// Start a reading child, keep it in `slot`, and hand back its stdout.
fn start_child(
    slot: &Arc<Mutex<Option<Child>>>,
    stop: &AtomicBool,
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
    keep(slot, stop, child).then_some(out).flatten()
}

/// Start a writing child (the return encoder) and hand back its stdin.
fn start_writer(
    slot: &Arc<Mutex<Option<Child>>>,
    stop: &AtomicBool,
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
    keep(slot, stop, child).then_some(input).flatten()
}

/// Take a slot's child out under its lock, then kill and wait for it with
/// the lock released, so the session's drop (or a test) can take the lock
/// meanwhile.
fn reap<C: Reap>(slot: &Mutex<Option<C>>) {
    let child = lock(slot).take();
    if let Some(mut child) = child {
        kill_and_wait(&mut child, REAP_DEADLINE);
    }
}

/// A video reader that ended without a picture met a stream with no video:
/// an audio-only guest (ffmpeg ends at once, "Output file does not contain
/// any stream"), a video track that carries no packets (ffmpeg probes it for
/// 5 s, then ends the same way), or a quiet input (its 5 s timeout).
/// Restarting it at once would end the same way, every ~0.6 s or ~5.5 s
/// (item 4c of the user's ruling of 2026-11-15 20:55). So the video thread
/// waits for the guest's audio reader to restart: that reader ends when no
/// packet has come on its track for 5 s (the same timeout), when the guest
/// leaves, or when it publishes again (perhaps with video). While the
/// guest's audio arrives, the video reader stays down. The audio reader's
/// first start is not a restart.
fn wait_for_audio_reader_restart(gid: &str, stop: &AtomicBool, stats: &GuestStats) {
    tracing::info!(guest = %gid, "guest video: no picture; waiting for the guest's audio reader to restart");
    let mut started = stats.audio_spawns.load(Ordering::Relaxed);
    loop {
        std::thread::sleep(RESPAWN_DELAY);
        if stop.load(Ordering::SeqCst) {
            return;
        }
        let now = stats.audio_spawns.load(Ordering::Relaxed);
        if started == 0 {
            started = now;
            continue;
        }
        if now != started {
            tracing::info!(guest = %gid, "guest audio reader restarted; restarting the video reader");
            return;
        }
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

    /// A child that exits a set time after its kill, or never.
    struct LateChild {
        killed: Arc<Mutex<Option<Instant>>>,
        exits_after: Option<Duration>,
        polls: Arc<AtomicU64>,
    }

    impl LateChild {
        fn new(
            exits_after: Option<Duration>,
        ) -> (Self, Arc<Mutex<Option<Instant>>>, Arc<AtomicU64>) {
            let killed = Arc::new(Mutex::new(None));
            let polls = Arc::new(AtomicU64::new(0));
            let c = LateChild {
                killed: killed.clone(),
                exits_after,
                polls: polls.clone(),
            };
            (c, killed, polls)
        }
    }

    impl Reap for LateChild {
        fn kill(&mut self) -> std::io::Result<()> {
            *self.killed.lock().unwrap() = Some(Instant::now());
            Ok(())
        }
        fn exited(&mut self) -> std::io::Result<bool> {
            self.polls.fetch_add(1, Ordering::SeqCst);
            let killed = *self.killed.lock().unwrap();
            Ok(
                matches!((killed, self.exits_after), (Some(at), Some(after)) if at.elapsed() >= after),
            )
        }
    }

    fn wait_for_kill(killed: &Mutex<Option<Instant>>) {
        let t = Instant::now();
        while killed.lock().unwrap().is_none() {
            assert!(
                t.elapsed() < Duration::from_secs(2),
                "the child was never killed"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn poison<C: Send + 'static>(slot: &Arc<Mutex<Option<C>>>) {
        let s = slot.clone();
        let _ = std::thread::spawn(move || {
            let _held = s.lock().unwrap();
            panic!("poisoning the slot on purpose");
        })
        .join();
        assert!(slot.is_poisoned());
    }

    /// Item 4c: a video reader that ended without a picture waits for the
    /// guest's audio reader to restart, not for its first start (the audio
    /// reader may start after the video reader has already ended), and a
    /// stopping session ends the wait.
    #[test]
    fn the_video_wait_ends_on_an_audio_reader_restart_not_its_first_start() {
        let stats = Arc::new(GuestStats::default());
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let (st, sp) = (stats.clone(), stop.clone());
        std::thread::spawn(move || {
            wait_for_audio_reader_restart("g", &sp, &st);
            let _ = tx.send(());
        });
        std::thread::sleep(RESPAWN_DELAY * 2);
        stats.audio_spawns.store(1, Ordering::Relaxed); // its first start
        assert!(
            rx.recv_timeout(RESPAWN_DELAY * 3).is_err(),
            "the audio reader's first start is not a restart"
        );
        stats.audio_spawns.store(2, Ordering::Relaxed); // a restart
        assert!(
            rx.recv_timeout(RESPAWN_DELAY * 3).is_ok(),
            "an audio reader restart ends the wait"
        );

        let (tx, rx) = std::sync::mpsc::channel();
        let (st, sp) = (stats.clone(), stop.clone());
        std::thread::spawn(move || {
            wait_for_audio_reader_restart("g", &sp, &st);
            let _ = tx.send(());
        });
        assert!(rx.recv_timeout(RESPAWN_DELAY * 3).is_err(), "it waits");
        stop.store(true, Ordering::SeqCst);
        assert!(
            rx.recv_timeout(RESPAWN_DELAY * 3).is_ok(),
            "a stopping session ends the wait"
        );
    }

    /// Item 4a (the user's ruling of 2026-11-15 20:55): `reap` and `keep` kill
    /// and wait for a child
    /// with its slot's lock released, so another thread (the session's drop)
    /// can take the lock meanwhile; the wait itself polls, so it can stop.
    /// What this cannot prove: how a real child that a kill cannot end (one
    /// stuck in the kernel) behaves. The stand-in here only says "not yet".
    #[test]
    fn the_slot_lock_is_free_while_a_killed_child_is_waited_for() {
        // reap: a child in the slot, exiting 300 ms after its kill.
        let (child, killed, polls) = LateChild::new(Some(Duration::from_millis(300)));
        let slot = Arc::new(Mutex::new(Some(child)));
        let (tx, rx) = std::sync::mpsc::channel();
        let s = slot.clone();
        std::thread::spawn(move || {
            reap(&s);
            let _ = tx.send(());
        });
        wait_for_kill(&killed);
        std::thread::sleep(Duration::from_millis(50));
        assert!(
            rx.try_recv().is_err(),
            "reap is still waiting for the child"
        );
        assert!(
            slot.try_lock().is_ok(),
            "reap must not hold the slot's lock while it waits for the child"
        );
        assert!(
            rx.recv_timeout(Duration::from_secs(2)).is_ok(),
            "reap returns once the child exits"
        );
        assert!(polls.load(Ordering::SeqCst) > 1, "the wait polls");
        assert!(slot.lock().unwrap().is_none());

        // keep, while the session stops: the child is not stored, and is
        // killed and waited for with the lock released.
        let (child, killed, _) = LateChild::new(Some(Duration::from_millis(300)));
        let slot: Arc<Mutex<Option<LateChild>>> = Arc::default();
        let stop = Arc::new(AtomicBool::new(true));
        let (tx, rx) = std::sync::mpsc::channel();
        let (s, st) = (slot.clone(), stop.clone());
        std::thread::spawn(move || {
            let _ = tx.send(keep(&s, &st, child));
        });
        wait_for_kill(&killed);
        std::thread::sleep(Duration::from_millis(50));
        assert!(
            rx.try_recv().is_err(),
            "keep is still waiting for the child"
        );
        assert!(
            slot.try_lock().is_ok(),
            "keep must not hold the slot's lock while it waits for the child"
        );
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(2)),
            Ok(false),
            "a stopping session keeps nothing"
        );
        assert!(slot.lock().unwrap().is_none());
    }

    /// Item 4a's bound: a child that never exits after its kill is given up
    /// on at the deadline; the wait does not hang the session's teardown.
    #[test]
    fn a_child_that_never_exits_is_given_up_on_at_the_deadline() {
        let (child, _, _) = LateChild::new(None);
        let slot = Arc::new(Mutex::new(Some(child)));
        let (tx, rx) = std::sync::mpsc::channel();
        let s = slot.clone();
        let t = Instant::now();
        std::thread::spawn(move || {
            reap(&s);
            let _ = tx.send(());
        });
        assert!(
            rx.recv_timeout(REAP_DEADLINE + Duration::from_secs(1))
                .is_ok(),
            "reap must give up on a child that never exits"
        );
        let waited = t.elapsed();
        assert!(
            waited >= REAP_DEADLINE,
            "it waited the deadline out first: {waited:?}"
        );
    }

    /// Item 4b (the user's ruling of 2026-11-15 20:55): a poisoned slot lock
    /// is taken anyway, as 7d
    /// does for the live-audio locks: `reap` reaps, `keep` keeps, and a
    /// session with a poisoned slot drops without a panic and still ends its
    /// child.
    #[test]
    fn a_poisoned_slot_is_reaped_and_dropped_not_a_panic() {
        let (child, killed, _) = LateChild::new(Some(Duration::ZERO));
        let slot = Arc::new(Mutex::new(Some(child)));
        poison(&slot);
        reap(&slot);
        assert!(
            killed.lock().unwrap().is_some(),
            "reap killed the child in a poisoned slot"
        );
        assert!(lock(&slot).is_none());
        let (child, _, _) = LateChild::new(Some(Duration::ZERO));
        assert!(
            keep(&slot, &AtomicBool::new(false), child),
            "keep stores into a poisoned slot"
        );

        let real = Command::new("sleep").arg("30").spawn().expect("sleep runs");
        let pid = real.id();
        let slot = Arc::new(Mutex::new(Some(real)));
        poison(&slot);
        let session = GuestSession {
            guest_id: "g-poisoned".into(),
            routes: routes("http://127.0.0.1:8889/g/whip").unwrap(),
            stats: Arc::default(),
            stop: Arc::default(),
            children: vec![slot],
            threads: Vec::new(),
        };
        drop(session);
        let ps = Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "stat="])
            .output()
            .expect("ps runs");
        assert!(
            String::from_utf8_lossy(&ps.stdout).trim().is_empty(),
            "the child of a poisoned slot is killed and reaped by the session's drop (ps: {:?})",
            String::from_utf8_lossy(&ps.stdout)
        );
    }
}
