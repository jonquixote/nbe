//! Prompt 09 WU7 (SPEC §10.1): `recordSpaceMib` reports free space on the
//! real target volume; `E_DISK` fires on an unwritable target.
//!
//! The helpers under test are `nbe_engine::record::available_space_mib`,
//! `nbe_engine::telemetry::record_space_mib_for`, and
//! `nbe_engine::telemetry::build_tick_for_dir`; the pump wiring under test
//! is `show.load` carrying `show.outputs.record.directory` into engine state
//! and `nbe_engine::channel::pump_tick` measuring against it.

use std::path::Path;

use nbe_engine::state::EngineState;
use nbe_protocol::EngineFrame;

/// Independent filesystem query: `df -k <path>`, 4th column of the last line
/// is Available in 1K blocks. Deliberately does NOT call the implementation.
fn df_avail_mib(path: &Path) -> f64 {
    let out = std::process::Command::new("df")
        .arg("-k")
        .arg(path)
        .output()
        .expect("df must spawn");
    assert!(out.status.success(), "df must succeed");
    let text = String::from_utf8_lossy(&out.stdout);
    let last = text.lines().last().expect("df must emit a data line");
    let cols: Vec<&str> = last.split_whitespace().collect();
    assert!(cols.len() >= 4, "df line must have ≥4 columns: {last}");
    let avail_kib: f64 = cols[3].parse().expect("Available column must parse");
    avail_kib / 1024.0
}

#[test]
fn record_space_mib_measured_against_real_target_volume() {
    let dir = tempfile::tempdir().expect("tempdir must succeed");

    let got = nbe_engine::record::available_space_mib(dir.path())
        .expect("writable tmpdir must report space");
    assert!(got > 0.0, "free space on a real dir must be > 0, got {got}");

    let expected = df_avail_mib(dir.path());
    let tolerance = (expected * 0.05).max(64.0);
    assert!(
        (got - expected).abs() <= tolerance,
        "reported {got:.1} MiB must match df {expected:.1} MiB within {tolerance:.1} MiB"
    );

    // Telemetry view of the same volume agrees.
    let via_telemetry = nbe_engine::telemetry::record_space_mib_for(Some(dir.path()));
    assert!(
        (via_telemetry - expected).abs() <= tolerance,
        "telemetry {via_telemetry:.1} MiB must match df {expected:.1} MiB"
    );
}

#[test]
fn unwritable_target_reports_e_disk_without_panic() {
    // Case 1: a regular file where the record directory should be.
    let dir = tempfile::tempdir().expect("tempdir must succeed");
    let blocker = dir.path().join("not-a-dir");
    std::fs::write(&blocker, b"x").unwrap();

    let err = nbe_engine::record::available_space_mib(&blocker)
        .expect_err("file-as-dir must refuse with E_DISK");
    assert!(
        matches!(err, nbe_engine::record::RecordError::Disk(_)),
        "variant must be RecordError::Disk, got: {err:?}"
    );
    assert!(
        err.to_string().contains("E_DISK"),
        "error must carry E_DISK token, got: {err}"
    );
    assert_eq!(err.kind_token(), "E_DISK");

    // Case 2: a read-only directory.
    let ro = tempfile::tempdir().expect("tempdir must succeed");
    let mut perms = std::fs::metadata(ro.path()).unwrap().permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        perms.set_mode(0o555);
    }
    #[cfg(not(unix))]
    {
        perms.set_readonly(true);
    }
    std::fs::set_permissions(ro.path(), perms).unwrap();

    // Self-calibrating privilege guard: a privileged runner (root/CI-as-root)
    // writes through read-only bits, so the refusal below cannot fire there.
    // Detect it directly instead of sniffing uids: if a probe write succeeds,
    // this case is vacuous on this machine — skip it loudly, keep case 1.
    if std::fs::write(ro.path().join(".privilege-probe"), b"x").is_ok() {
        std::fs::remove_file(ro.path().join(".privilege-probe")).ok();
        eprintln!("SKIP read-only-dir case: runner writes through permission bits (privileged)");
    } else {
        let err = nbe_engine::record::available_space_mib(ro.path())
            .expect_err("read-only dir must refuse with E_DISK");
        assert!(
            matches!(err, nbe_engine::record::RecordError::Disk(_)),
            "variant must be RecordError::Disk, got: {err:?}"
        );
        assert!(err.to_string().contains("E_DISK"));
    } // end privileged-runner guard: case 2 above runs only where bits bind

    // Telemetry degrades to 0.0 on an unwritable target — never panics.
    let v = nbe_engine::telemetry::record_space_mib_for(Some(&blocker));
    assert_eq!(v, 0.0, "unwritable target must degrade to 0.0, got {v}");
}

#[test]
fn telemetry_tick_carries_record_space_without_touching_frame_path() {
    let state = EngineState::new(30);
    let dir = tempfile::tempdir().expect("tempdir must succeed");

    let frame = nbe_engine::telemetry::build_tick_for_dir(&state, Some(dir.path()));
    let space = match frame {
        EngineFrame::EngineTelemetry { fields, .. } => fields.record_space_mib,
        other => panic!("expected engineTelemetry, got {other:?}"),
    };
    assert!(space > 0.0, "tick must carry measured space, got {space}");

    // No target configured: field present, zero — shape stays complete.
    let frame = nbe_engine::telemetry::build_tick_for_dir(&state, None);
    let space = match frame {
        EngineFrame::EngineTelemetry { fields, .. } => fields.record_space_mib,
        other => panic!("expected engineTelemetry, got {other:?}"),
    };
    assert_eq!(space, 0.0);

    // Legacy entry point keeps its signature and behavior (render loop path
    // untouched — the channel pump wires state through `pump_tick` instead).
    let frame = nbe_engine::telemetry::build_tick(&state);
    let space = match frame {
        EngineFrame::EngineTelemetry { fields, .. } => fields.record_space_mib,
        other => panic!("expected engineTelemetry, got {other:?}"),
    };
    assert_eq!(space, 0.0);
}

#[tokio::test]
async fn show_load_wires_record_dir_into_the_pump_tick() {
    use nbe_engine::directive::DirectiveHandler;
    use nbe_protocol::{DirectiveFrame, DirectiveKind, PROTOCOL_VERSION};
    use std::sync::Arc;

    let pkg = tempfile::tempdir().expect("package tempdir must succeed");
    let rec = tempfile::tempdir().expect("record tempdir must succeed");
    std::fs::create_dir_all(pkg.path().join("media")).unwrap();
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        8,
        8,
        image::Rgba([9, 9, 9, 255]),
    ))
    .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
    .unwrap();
    std::fs::write(pkg.path().join("media/slate.png"), &png).unwrap();
    std::fs::write(
        pkg.path().join("manifest.json"),
        serde_json::json!({
            "manifestVersion": "0.3",
            "network": { "id": "nbe", "name": "T" },
            "show": {
                "id": "s", "title": "T",
                "video": { "width": 640, "height": 360, "frameRate": 30, "colorSpace": "rec709" },
                "audio": { "sampleRate": 48000, "loudnessTargetLufs": -16.0, "truePeakDbtp": -1.5 },
                "fallbackAssetId": "slate",
                "outputs": { "record": { "directory": rec.path().to_string_lossy() } }
            },
            "assets": [
                { "id": "slate", "kind": "image", "source": "media/slate.png", "format": "png" }
            ],
            "scenes": [],
            "rundown": { "id": "R", "items": [] },
            "control": { "bindings": [] }
        })
        .to_string(),
    )
    .unwrap();

    let state = Arc::new(EngineState::new(30));
    let outgoing = Arc::new(nbe_engine::state::OutgoingQueue::default());
    let handler = DirectiveHandler::new(state.clone(), outgoing);
    handler
        .apply(&DirectiveFrame {
            v: PROTOCOL_VERSION.into(),
            kind: DirectiveKind::Directive,
            seq: 1,
            state_version: 1,
            command: "show.load".into(),
            target: serde_json::json!({}),
            payload: serde_json::json!({ "packagePath": pkg.path().to_string_lossy() }),
        })
        .await
        .unwrap();

    // The pump reads this exact state: wired means Some, and the tick
    // measured against it is non-zero on a real volume.
    assert_eq!(
        *state.record_dir.lock().unwrap(),
        Some(rec.path().to_path_buf())
    );
    let space = match nbe_engine::channel::pump_tick(&state) {
        EngineFrame::EngineTelemetry { fields, .. } => fields.record_space_mib,
        other => panic!("expected engineTelemetry, got {other:?}"),
    };
    assert!(
        space > 0.0,
        "wired pump tick must report measured space, got {space}"
    );
}
