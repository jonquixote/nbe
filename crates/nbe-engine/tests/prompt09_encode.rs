//! Prompt 09 WU2 (SPEC §9.2): hardware-only H.264 encode.
//!
//! TDD: this test was written BEFORE the implementation (RED first).
//! The session itself lives in `nbe-decode` (the crate allowed `unsafe`);
//! `nbe_engine::encode` re-exports it, mirroring the existing
//! `pub use nbe_decode as decode` pattern.

use nbe_engine::encode::{is_available, EncodeSession};

fn synthetic_rgba(width: u32, height: u32, frame: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            out.push(((x + frame * 7) % 256) as u8);
            out.push(((y + frame * 13) % 256) as u8);
            out.push(((x + y + frame * 3) % 256) as u8);
            out.push(255);
        }
    }
    out
}

#[test]
fn hardware_encode_roundtrip_emits_keyframed_access_units() {
    // Capability gate (not a skip of convenience): headless CI runners have
    // no GPU, so VideoToolbox exposes no hardware encoder there and this
    // test cannot run. The refusal path itself is pinned by
    // forced_software_seam_refuses_with_no_hardware_encoder, which runs
    // everywhere; this test pins the happy path where hardware exists.
    if !is_available() {
        eprintln!("SKIP hardware roundtrip: no hardware H.264 encoder on this machine");
        return;
    }
    let mut session = EncodeSession::open(640, 360, 30, 1_000_000)
        .expect("EncodeSession::open must succeed where hardware exists");

    // One second of synthetic frames. 640 wide: the hardware encoder
    // refuses narrower widths (see `is_available`), so the roundtrip runs
    // at the smallest size the hardware accepts.
    for frame in 0..30 {
        let rgba = synthetic_rgba(640, 360, frame);
        session
            .encode_rgba(&rgba)
            .expect("feeding a well-formed RGBA frame must succeed");
    }
    let units = session.finish().expect("finish must complete the stream");

    assert!(!units.is_empty(), "30 frames in must yield access units");
    assert!(
        units.iter().all(|u| !u.data.is_empty()),
        "every access unit must carry payload"
    );
    let mut prev = f64::NEG_INFINITY;
    for u in &units {
        assert!(
            u.pts_seconds > prev,
            "PTS must increase monotonically, got {} after {prev}",
            u.pts_seconds
        );
        prev = u.pts_seconds;
    }
    assert!(
        units.iter().any(|u| u.is_keyframe),
        "the first second must contain at least one keyframe"
    );
}

#[test]
fn forced_software_seam_refuses_with_no_hardware_encoder() {
    // The refusal path must be testable even where hardware IS present:
    // forcing the software leg refuses exactly as a machine with no
    // hardware encoder would — E_NO_HARDWARE_ENCODER, never CPU fallback.
    // 640x360 deliberately: hardware accepts this size, so only the
    // force_software seam itself can produce the refusal below. (At 320x240
    // the test passed for the wrong reason — the HW size floor refuses first
    // and masks a deleted seam.)
    let err = EncodeSession::open_with_options(640, 360, 30, 1_000_000, true)
        .expect_err("forced software must refuse: hardware encoders only");
    assert!(
        err.to_string().contains("E_NO_HARDWARE_ENCODER"),
        "expected E_NO_HARDWARE_ENCODER, got: {err}"
    );
}
