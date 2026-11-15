//! The second key's item 7a (the user's ruling of 2026-11-14 22:20): a guest
//! session's ffmpeg children are reaped. Join and leave a guest again and
//! again; after each leave none of its children is left running or defunct,
//! and the leave itself returns promptly.
//!
//! The media server here is a TCP listener that accepts and then says
//! nothing, so an ffmpeg reader blocks in its RTSP handshake instead of
//! failing fast and exiting by itself. A child spawned while a session is
//! stopping would then block for good, and the session's drop would wait
//! on the thread reading it: the race this guards.
//!
//! Needs ffmpeg on PATH (or `NBE_FFMPEG`); without it the test says so and
//! returns.

use std::io::Read;
use std::net::TcpListener;
use std::sync::Arc;
use std::time::{Duration, Instant};

use nbe_engine::audio_ring::ring;
use nbe_engine::guest::{GuestRoutes, GuestSession};
use nbe_engine::live::{FrameMailbox, LiveAudio};

/// The two tests here count this process's children, so they run one at a
/// time: one test's readers are not the other's left-behind children.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// This process's direct children: (pid, state, command name).
fn children() -> Vec<(u32, String, String)> {
    let me = std::process::id();
    let out = std::process::Command::new("ps")
        .args(["-A", "-o", "pid=,ppid=,stat=,comm="])
        .output()
        .expect("ps runs");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            let pid: u32 = f.next()?.parse().ok()?;
            let ppid: u32 = f.next()?.parse().ok()?;
            let stat = f.next()?.to_string();
            let comm = f.collect::<Vec<_>>().join(" ");
            (ppid == me).then_some((pid, stat, comm))
        })
        .collect()
}

fn ffmpeg_present() -> bool {
    let bin = std::env::var("NBE_FFMPEG").unwrap_or_else(|_| "ffmpeg".into());
    std::process::Command::new(bin)
        .arg("-version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// A media server that accepts and then never answers; the routes to it.
fn silent_server() -> GuestRoutes {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let mut held = Vec::new();
        for s in listener.incoming().flatten() {
            let mut s2 = s.try_clone().unwrap();
            std::thread::spawn(move || {
                let mut buf = [0u8; 1024];
                while matches!(s2.read(&mut buf), Ok(n) if n > 0) {}
            });
            held.push(s);
        }
    });
    GuestRoutes {
        path: "g1".into(),
        rtsp_in: format!("rtsp://127.0.0.1:{port}/g1"),
        rtsp_return: format!("rtsp://127.0.0.1:{port}/g1-return"),
        whep_return: format!("http://127.0.0.1:{port}/g1-return/whep"),
    }
}

fn session(routes: &GuestRoutes) -> GuestSession {
    let (audio, producer) = LiveAudio::new();
    let (_return_in, return_out) = ring(9_600);
    GuestSession::start(
        "g1",
        routes.clone(),
        Arc::new(FrameMailbox::default()),
        (audio, producer),
        return_out,
    )
}

/// Item 7c's other half: a reader whose input goes quiet ends by itself
/// (ffmpeg's 5 s socket timeout) and is restarted. Data flowing, a reader
/// left behind by a killed engine ends on its failed write (the guest e2e's
/// kill -9 test); with nothing flowing, it ends on this timeout instead of
/// waiting on the input for good.
#[test]
fn a_reader_on_a_quiet_input_ends_by_itself_and_is_restarted() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    if !ffmpeg_present() {
        eprintln!("SKIP: no ffmpeg on PATH (or NBE_FFMPEG)");
        return;
    }
    let routes = silent_server();
    let s = session(&routes);
    std::thread::sleep(Duration::from_millis(7_500));
    let (video, audio) = (
        s.stats
            .video_spawns
            .load(std::sync::atomic::Ordering::Relaxed),
        s.stats
            .audio_spawns
            .load(std::sync::atomic::Ordering::Relaxed),
    );
    drop(s);
    println!("item 7c: on a silent input, 7.5 s: video reader started {video} times, audio reader {audio}");
    assert!(
        video >= 2 && audio >= 2,
        "a reader on a silent input ends after its 5 s timeout and is restarted (video {video}, audio {audio} starts)"
    );
}

#[test]
fn a_guest_that_joins_and_leaves_twenty_times_leaves_no_ffmpeg_running_or_defunct() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    if !ffmpeg_present() {
        eprintln!("SKIP: no ffmpeg on PATH (or NBE_FFMPEG)");
        return;
    }
    let routes = silent_server();

    let mut slowest = Duration::ZERO;
    for i in 0..20u64 {
        let session = session(&routes);
        // Leave at once, or after the children are up: 0 to 38 ms.
        std::thread::sleep(Duration::from_millis(i * 2));
        let left = Instant::now();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            drop(session);
            let _ = tx.send(());
        });
        assert!(
            rx.recv_timeout(Duration::from_secs(3)).is_ok(),
            "leave {i}: the session's drop did not return within 3 s (a thread is reading a child no one killed)"
        );
        slowest = slowest.max(left.elapsed());
        let left_behind: Vec<_> = children()
            .into_iter()
            .filter(|(_, stat, comm)| stat.contains('Z') || comm.contains("ffmpeg"))
            .collect();
        assert!(
            left_behind.is_empty(),
            "leave {i}: children left running or defunct: {left_behind:?}"
        );
    }
    println!("item 7a: twenty joins and leaves, no ffmpeg left running or defunct; slowest leave {slowest:?}");
}
