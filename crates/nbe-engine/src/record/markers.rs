//! Recording markers → chapters + always-sidecar (Prompt 09 WU5 `[RI-5]`,
//! SPEC §16.11).
//!
//! `marker.add { name, timecode? }` requires a running show; the engine
//! records the marker only while [`RecordState::Recording`][crate::state::RecordState].
//! While Idle the directive is refused with `E_FORBIDDEN_STATE` (see
//! `DirectiveHandler::on_marker_add`) — accept-but-ignore would be silent
//! loss, so it is an error instead.
//!
//! ## Honest container story
//!
//! The writer only supports fragmented MP4 today, and fMP4 carries chapters
//! poorly — so chapters ride an **always-sidecar** JSON file beside the
//! recording, for every container, and the Matroska in-container chapter
//! muxing is a later concern. [`chapters`] is the seam that muxer will read:
//! the frame-ordered chapter list. Nothing here claims an MP4 carries chapters.
//!
//! ## Explicit deferral: per-container select + Matroska chapters
//!
//! SPEC §9.3 allows Matroska as an alternative container and §9.3 SHOULD wants
//! markers as chapters where the container supports them. Both are DEFERRED:
//!
//! * Per-container select: `record.start` carries no container field (SPEC
//!   §16.14 payload is `{ outputId? }`), and no manifest field selects one
//!   either — every take is fragmented MP4. An `outputId → container` mapping
//!   would be invented schema, and schema is out of scope for this unit.
//! * Matroska chapters: no EBML mux exists beside the hand-rolled fMP4 boxes,
//!   and bolting one on would double the mux surface (seek-heads, tracks,
//!   cues, chapter atoms) without a second conformance oracle in the suite.
//!
//! What changes the answer (trigger): a manifest-declared container select
//! (control-plane schema + engine plumbing for it) or a take that must carry
//! chapters in-container for a downstream that will not read the sidecar.
//! Then: implement the EBML mux beside `writer.rs` (same fragment discipline:
//! header upfront, per-second clusters flushed immediately, no finalization
//! required), read [`chapters`] at cluster boundaries, and keep the
//! always-sidecar anyway (it is the container-independent chapter record the
//! suite asserts).

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use super::RecordError;

/// One recording marker: the operator's name, the master frame it landed on,
/// and the verbatim `timecode` string when the directive carried one.
///
/// Frame rule: `frame` is `master_frame() + 1` — the marker takes effect on
/// the NEXT frame boundary, the same discipline as a take and an overlay,
/// never mid-frame. Timecode rule: `timecode` is kept verbatim as given;
/// `frame` is the authority for ordering/chapters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Marker {
    /// Operator-supplied chapter name (`marker.add` payload `name`).
    pub name: String,
    /// `master_frame() + 1`: the next-frame boundary the directive landed on.
    pub frame: u64,
    /// Verbatim `timecode` string from the payload, when present. Kept as
    /// given; `frame` (not this) orders chapters.
    pub timecode: Option<String>,
}

/// Markers recorded during the current take. Process-wide because
/// `EngineState` (untouchable in this work unit) has no marker field to hang
/// them on; the directive path pushes here while Recording and the finish
/// path snapshots via [`list`] into [`write_sidecar`]. Tests reset with
/// [`clear`].
static STORE: Mutex<Vec<Marker>> = Mutex::new(Vec::new());

/// Record one marker in the process-wide store.
pub fn add(marker: Marker) {
    STORE.lock().unwrap().push(marker);
}

/// Snapshot the stored markers, in arrival order.
pub fn list() -> Vec<Marker> {
    STORE.lock().unwrap().clone()
}

/// Drop all stored markers (test seam + take boundary).
pub fn clear() {
    STORE.lock().unwrap().clear();
}

/// The chapter list for the container muxer: markers ordered by frame.
/// The writer only supports fMP4 today (chapters unsupported there), so these
/// currently ride the sidecar; the Matroska muxer will read this same order.
pub fn chapters(markers: &[Marker]) -> Vec<Marker> {
    let mut out = markers.to_vec();
    out.sort_by_key(|m| m.frame);
    out
}

/// Sidecar path beside the recording file: `<stem>.markers.json`.
/// `show_ep_20260914T120000Z.mp4` → `show_ep_20260914T120000Z.markers.json`.
pub fn sidecar_path(video_path: &Path) -> PathBuf {
    video_path.with_extension("markers.json")
}

/// Write the always-sidecar JSON beside the recording file, carrying the full
/// marker list with frames + timecodes. Returns the sidecar path. A write
/// failure is `E_DISK` (SPEC §16.11's failure mode for `marker.add`).
pub fn write_sidecar(video_path: &Path, markers: &[Marker]) -> Result<PathBuf, RecordError> {
    let path = sidecar_path(video_path);
    let body = serde_json::to_string_pretty(&serde_json::json!({ "markers": chapters(markers) }))
        .map_err(|e| RecordError::Disk(format!("marker sidecar serialize: {e}")))?;
    std::fs::write(&path, body).map_err(|e| RecordError::Disk(format!("marker sidecar: {e}")))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidecar_path_sits_beside_the_file() {
        let video = Path::new("/tmp/show_ep_20260914T120000Z.mp4");
        assert_eq!(
            sidecar_path(video),
            PathBuf::from("/tmp/show_ep_20260914T120000Z.markers.json")
        );
    }

    #[test]
    fn store_round_trips_in_arrival_order() {
        clear();
        add(Marker {
            name: "a".into(),
            frame: 3,
            timecode: None,
        });
        add(Marker {
            name: "b".into(),
            frame: 1,
            timecode: Some("00:00:00:01".into()),
        });
        let got = list();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].name, "a");
        clear();
        assert!(list().is_empty());
    }
}
