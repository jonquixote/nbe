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
//! Item 4c of the user's ruling of 2026-11-15 20:55 is here too: an
//! audio-only guest's video reader is not restarted while its audio arrives
//! (a minimal RTSP server that sends audio only).
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

/// A minimal RTSP server (TCP interleaved only) whose stream has audio and no
/// video, or audio plus a declared video track that never carries a packet:
/// the two shapes an audio-only guest's stream can take. Every client that
/// PLAYs gets 48 kHz stereo L16 audio, one 10 ms RTP packet every 10 ms, on
/// the channel it set the audio track up on.
fn audio_only_server(declare_video: bool) -> GuestRoutes {
    use std::io::Write;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for s in listener.incoming().flatten() {
            std::thread::spawn(move || {
                let out = Arc::new(std::sync::Mutex::new(s.try_clone().unwrap()));
                let send = |text: String| {
                    let _ = out.lock().unwrap().write_all(text.as_bytes());
                };
                let mut reader = std::io::BufReader::new(s);
                let mut audio_channel: Option<u8> = None;
                loop {
                    // One request: lines up to a blank line (no bodies).
                    let mut head = Vec::new();
                    loop {
                        let mut line = String::new();
                        match std::io::BufRead::read_line(&mut reader, &mut line) {
                            Ok(0) | Err(_) => return,
                            Ok(_) if line == "\r\n" || line == "\n" => break,
                            Ok(_) => head.push(line.trim_end().to_string()),
                        }
                    }
                    let Some(first) = head.first().cloned() else {
                        continue;
                    };
                    let header = |name: &str| {
                        head.iter().find_map(|l| {
                            let (k, v) = l.split_once(':')?;
                            k.trim()
                                .eq_ignore_ascii_case(name)
                                .then(|| v.trim().to_string())
                        })
                    };
                    let cseq = header("CSeq").unwrap_or_default();
                    let method = first.split_whitespace().next().unwrap_or("").to_string();
                    let url = first.split_whitespace().nth(1).unwrap_or("").to_string();
                    match method.as_str() {
                        "DESCRIBE" => {
                            let mut sdp = String::from(
                                "v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\ns=nbe test\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\n\
                                 m=audio 0 RTP/AVP 97\r\na=rtpmap:97 L16/48000/2\r\na=control:trackID=0\r\n",
                            );
                            if declare_video {
                                sdp.push_str(
                                    "m=video 0 RTP/AVP 96\r\na=rtpmap:96 H264/90000\r\n\
                                     a=fmtp:96 packetization-mode=1\r\na=control:trackID=1\r\n",
                                );
                            }
                            send(format!(
                                "RTSP/1.0 200 OK\r\nCSeq: {cseq}\r\nContent-Base: {url}/\r\nContent-Type: application/sdp\r\nContent-Length: {}\r\n\r\n{sdp}",
                                sdp.len()
                            ));
                        }
                        "SETUP" => {
                            let transport = header("Transport").unwrap_or_default();
                            if url.ends_with("trackID=0") {
                                audio_channel = transport
                                    .split(';')
                                    .find_map(|p| p.strip_prefix("interleaved="))
                                    .and_then(|r| r.split('-').next()?.parse().ok());
                            }
                            send(format!(
                                "RTSP/1.0 200 OK\r\nCSeq: {cseq}\r\nTransport: {transport}\r\nSession: 4c4c4c4c;timeout=60\r\n\r\n"
                            ));
                        }
                        "PLAY" => {
                            send(format!(
                                "RTSP/1.0 200 OK\r\nCSeq: {cseq}\r\nSession: 4c4c4c4c\r\nRange: npt=0.000-\r\n\r\n"
                            ));
                            if let Some(ch) = audio_channel {
                                let out = out.clone();
                                std::thread::spawn(move || {
                                    let started = Instant::now();
                                    for n in 0u32.. {
                                        let mut packet = vec![b'$', ch, 0, 0];
                                        let rtp_len = 12 + 480 * 2 * 2;
                                        packet[2..4].copy_from_slice(&(rtp_len as u16).to_be_bytes());
                                        packet.extend_from_slice(&[0x80, 97]);
                                        packet.extend_from_slice(&(n as u16).to_be_bytes());
                                        packet.extend_from_slice(&(n.wrapping_mul(480)).to_be_bytes());
                                        packet.extend_from_slice(&0x4c4c_4c4cu32.to_be_bytes());
                                        packet.resize(4 + rtp_len, 0);
                                        if out.lock().unwrap().write_all(&packet).is_err() {
                                            return;
                                        }
                                        let next = started + Duration::from_millis(10) * (n + 1);
                                        std::thread::sleep(next.saturating_duration_since(Instant::now()));
                                    }
                                });
                            }
                        }
                        "TEARDOWN" => {
                            send(format!("RTSP/1.0 200 OK\r\nCSeq: {cseq}\r\n\r\n"));
                            return;
                        }
                        _ => send(format!(
                            "RTSP/1.0 200 OK\r\nCSeq: {cseq}\r\nPublic: OPTIONS, DESCRIBE, SETUP, PLAY, TEARDOWN, GET_PARAMETER\r\n\r\n"
                        )),
                    }
                }
            });
        }
    });
    GuestRoutes {
        path: "g1".into(),
        rtsp_in: format!("rtsp://127.0.0.1:{port}/g1"),
        rtsp_return: format!("rtsp://127.0.0.1:{port}/g1-return"),
        whep_return: format!("http://127.0.0.1:{port}/g1-return/whep"),
    }
}

/// Item 4c (the user's ruling of 2026-11-15 20:55): an audio-only guest's
/// video reader is not
/// restarted while the guest's audio arrives. Without the wait it was: a
/// stream with no video track ends the reader at once ("no stream", every
/// ~0.6 s), and a declared video track that carries no packets ends it after
/// a 5 s probe (every ~5.5 s). Both shapes run here at once for 20 s; the
/// audio must have flowed for at least 10 s of them, or the test proves
/// nothing. (With the declared video track, the audio reader's own probe
/// also waits about 5 s for it before the first audio block.)
#[test]
fn an_audio_only_guest_s_video_reader_is_not_restarted_while_its_audio_arrives() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    if !ffmpeg_present() {
        eprintln!("SKIP: no ffmpeg on PATH (or NBE_FFMPEG)");
        return;
    }
    let sessions: Vec<_> = [false, true]
        .into_iter()
        .map(|declare_video| (declare_video, session(&audio_only_server(declare_video))))
        .collect();
    std::thread::sleep(Duration::from_secs(20));
    let ord = std::sync::atomic::Ordering::Relaxed;
    for (declare_video, s) in sessions {
        let (video, audio, blocks) = (
            s.stats.video_spawns.load(ord),
            s.stats.audio_spawns.load(ord),
            s.stats.audio_blocks_in.load(ord),
        );
        drop(s);
        let shape = if declare_video {
            "a declared video track with no packets"
        } else {
            "no video track"
        };
        println!("item 4c: audio only ({shape}), 20 s: video reader started {video} times, audio reader {audio}, {blocks} audio blocks of 10 ms");
        assert!(
            blocks >= 1_000,
            "{shape}: the audio must flow for at least 10 of the 20 s ({blocks} blocks of 10 ms)"
        );
        assert_eq!(audio, 1, "{shape}: the audio reader runs throughout");
        assert_eq!(
            video, 1,
            "{shape}: the video reader starts once and is not restarted while the audio arrives"
        );
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
/// waiting on the input for good. Item 4c keeps it: no audio arrives on a
/// quiet input either, so the video reader still starts again (after 1 s of
/// quiet audio, at about 6 s), within the 7.5 s here.
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
