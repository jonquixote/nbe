use nbe_engine::decode::{probe_asset, DecodeSession};
use std::path::Path;

#[test]
fn probe_fixtures() {
    for name in [
        "cfr_30.mp4",
        "cadence_12.mp4",
        "loop_10.mp4",
        "vfr.mp4",
        "wrong_res.mp4",
    ] {
        let p = format!("../../tests/fixtures/media/{name}");
        match probe_asset(Path::new(&p), 200) {
            Ok(pr) => println!(
                "{name}: {}x{} nominal={} frames={} cfr={} measured={:.3} alpha={}",
                pr.width,
                pr.height,
                pr.nominal_frame_rate,
                pr.frame_count,
                pr.cfr,
                pr.measured_frame_rate,
                pr.has_alpha
            ),
            Err(e) => println!("{name}: ERROR {e}"),
        }
    }
    let mut s = DecodeSession::open(Path::new("../../tests/fixtures/media/cfr_30.mp4")).unwrap();
    let frames = s.decode_all(30).unwrap();
    for i in [0usize, 12, 25] {
        let f = &frames[i];
        let c = ((f.height / 2 * f.width + f.width / 2) * 4) as usize;
        println!("frame {} px = {:?}", f.index, &f.rgba[c..c + 4]);
    }
    match probe_asset(Path::new("../../tests/fixtures/media/corrupt.mp4"), 10) {
        Ok(p) => println!("corrupt: UNEXPECTEDLY OK {p:?}"),
        Err(e) => println!("corrupt: correctly failed: {e}"),
    }
}
