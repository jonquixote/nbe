//! Scene resolution (Prompt 04 Step 1): show state → pixels.
//!
//! The chain is `view_item` / `preview_item` → rundown Item → Scene → Elements
//! → layers. Everything here is CPU-side and allocation-light; the GPU side
//! (`render.rs`) turns a `ResolvedScene` into draws.
//!
//! Decoding happens here, at `show.load` time, never in the render loop
//! (SPEC §7.13).

use std::collections::HashMap;

/// A decoded, ready-to-upload image.
#[derive(Debug, Clone)]
pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl DecodedImage {
    /// A generated slate: the engine can always show something, even with no
    /// usable asset on disk (SPEC §7.14 — the fallback is never a disk read at
    /// trigger time, and never nothing).
    pub fn generated_slate(width: u32, height: u32, rgba: [u8; 4]) -> Self {
        let mut px = Vec::with_capacity((width * height * 4) as usize);
        for _ in 0..(width * height) {
            px.extend_from_slice(&rgba);
        }
        Self {
            width,
            height,
            rgba: px,
        }
    }

    /// Decode image bytes. Returns None when the bytes are not a usable image
    /// — the caller substitutes a generated slate and logs.
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let img = image::load_from_memory(bytes).ok()?;
        let rgba = img.to_rgba8();
        Some(Self {
            width: rgba.width(),
            height: rgba.height(),
            rgba: rgba.into_raw(),
        })
    }
}

/// What a single element contributes to a frame.
#[derive(Debug, Clone, PartialEq)]
pub enum LayerSource {
    /// A solid fill (`graphic` elements carrying `fields.color`).
    Solid([f32; 4]),
    /// A decoded image, addressed by asset id.
    Image(String),
}

/// One composited element, already ordered and resolved.
#[derive(Debug, Clone, PartialEq)]
pub struct Layer {
    pub source: LayerSource,
    /// Normalized destination rect (x, y, w, h) in 0..1 of the target.
    pub rect: [f32; 4],
    pub opacity: f32,
    pub z: i64,
}

/// The resolved contents of one bus at one master-clock frame.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResolvedScene {
    pub layers: Vec<Layer>,
}

impl ResolvedScene {
    pub fn is_empty(&self) -> bool {
        self.layers.is_empty()
    }
}

/// The slice of the manifest the renderer needs, indexed at `show.load`.
#[derive(Debug, Default)]
pub struct PackageIndex {
    /// item id → scene id (for `sceneRef` items).
    pub item_scene: HashMap<String, String>,
    /// item id → item kind (`slate` renders a generated scene).
    pub item_kind: HashMap<String, String>,
    /// scene id → elements, already sorted low-z to high-z.
    pub scenes: HashMap<String, Vec<ElementSpec>>,
    /// asset id → decoded image, for `image`-kind assets only.
    pub images: HashMap<String, DecodedImage>,
    /// asset id → kind, so video references can be skipped by scope, not error.
    pub asset_kind: HashMap<String, String>,
    /// The manifest's requested quality profile (SPEC §10.1.1).
    pub requested_quality: Option<nbe_protocol::QualityProfile>,
}

/// One element, reduced to what this prompt can draw.
#[derive(Debug, Clone, PartialEq)]
pub struct ElementSpec {
    pub id: String,
    pub kind: String,
    pub z: i64,
    pub visible: bool,
    pub asset_id: Option<String>,
    pub color: Option<[f32; 4]>,
    pub rect: [f32; 4],
    pub opacity: f32,
}

impl PackageIndex {
    /// Build the index from a manifest value plus the package root (for asset
    /// decode). Validation is the control plane's job (SPEC §1.4 of the
    /// Prompt 02a addendum) — this only reads structure.
    pub fn build(manifest: &serde_json::Value, package_root: &std::path::Path) -> Self {
        let mut idx = PackageIndex {
            requested_quality: manifest
                .get("qualityProfile")
                .and_then(|v| v.as_str())
                .and_then(parse_quality),
            ..Default::default()
        };

        for asset in manifest
            .get("assets")
            .and_then(|a| a.as_array())
            .into_iter()
            .flatten()
        {
            let (Some(id), Some(kind), Some(src)) = (
                asset.get("id").and_then(|v| v.as_str()),
                asset.get("kind").and_then(|v| v.as_str()),
                asset.get("source").and_then(|v| v.as_str()),
            ) else {
                continue;
            };
            idx.asset_kind.insert(id.to_string(), kind.to_string());
            if kind != "image" {
                continue; // video decode is Prompt 05; a scope boundary, not a fault
            }
            match std::fs::read(package_root.join(src)).ok().as_deref() {
                Some(bytes) => match DecodedImage::decode(bytes) {
                    Some(img) => {
                        idx.images.insert(id.to_string(), img);
                    }
                    None => tracing::warn!(asset = id, "image asset is not decodable; skipping"),
                },
                None => tracing::warn!(asset = id, "image asset unreadable; skipping"),
            }
        }

        for scene in manifest
            .get("scenes")
            .and_then(|s| s.as_array())
            .into_iter()
            .flatten()
        {
            let Some(id) = scene.get("id").and_then(|v| v.as_str()) else {
                continue;
            };
            let mut elements: Vec<ElementSpec> = scene
                .get("elements")
                .and_then(|e| e.as_array())
                .into_iter()
                .flatten()
                .filter_map(element_spec)
                .collect();
            elements.sort_by_key(|e| e.z);
            idx.scenes.insert(id.to_string(), elements);
        }

        index_sequence(manifest.get("rundown"), &mut idx);
        idx
    }

    /// Resolve one item reference to drawable layers. An item that resolves to
    /// nothing yields an empty scene, which renders as black — a defined
    /// picture, not an error.
    pub fn resolve(&self, item_ref: Option<&str>) -> ResolvedScene {
        let Some(item) = item_ref else {
            return ResolvedScene::default();
        };
        if self.item_kind.get(item).map(String::as_str) == Some("slate") {
            return ResolvedScene {
                layers: vec![Layer {
                    source: LayerSource::Solid(SLATE_COLOR),
                    rect: FULL_RECT,
                    opacity: 1.0,
                    z: 0,
                }],
            };
        }
        let Some(scene_id) = self.item_scene.get(item) else {
            return ResolvedScene::default();
        };
        let Some(elements) = self.scenes.get(scene_id) else {
            return ResolvedScene::default();
        };
        let layers = elements
            .iter()
            .filter(|e| e.visible)
            .filter_map(|e| self.layer_for(e))
            .collect();
        ResolvedScene { layers }
    }

    fn layer_for(&self, e: &ElementSpec) -> Option<Layer> {
        let source = match e.kind.as_str() {
            "graphic" => LayerSource::Solid(e.color.unwrap_or(PLACEHOLDER_GRAPHIC)),
            "clip" | "videoLoop" => {
                let asset = e.asset_id.as_deref()?;
                match self.asset_kind.get(asset).map(String::as_str) {
                    Some("image") if self.images.contains_key(asset) => {
                        LayerSource::Image(asset.to_string())
                    }
                    Some("video") | Some("alphaVideo") => {
                        // Prompt 05 owns video. A scope boundary, not a decode
                        // fault: reporting it as one would drive the item to
                        // ERROR in the control plane's §17.3 machine.
                        tracing::info!(element = %e.id, asset, "video element unsupported until Prompt 05");
                        return None;
                    }
                    _ => return None,
                }
            }
            _ => return None,
        };
        Some(Layer {
            source,
            rect: e.rect,
            opacity: e.opacity,
            z: e.z,
        })
    }
}

pub const FULL_RECT: [f32; 4] = [0.0, 0.0, 1.0, 1.0];
/// The generated slate colour, used for `slate` items and for a fallback whose
/// asset could not be decoded.
pub const SLATE_COLOR: [f32; 4] = [0.75, 0.35, 0.05, 1.0];
const PLACEHOLDER_GRAPHIC: [f32; 4] = [0.5, 0.5, 0.5, 1.0];

fn index_sequence(sequence: Option<&serde_json::Value>, idx: &mut PackageIndex) {
    let Some(items) = sequence
        .and_then(|s| s.get("items"))
        .and_then(|i| i.as_array())
    else {
        return;
    };
    for item in items {
        let Some(id) = item.get("id").and_then(|v| v.as_str()) else {
            continue;
        };
        if let Some(kind) = item.get("kind").and_then(|v| v.as_str()) {
            idx.item_kind.insert(id.to_string(), kind.to_string());
        }
        if let Some(scene) = item.get("sceneRef").and_then(|v| v.as_str()) {
            idx.item_scene.insert(id.to_string(), scene.to_string());
        }
    }
}

fn element_spec(e: &serde_json::Value) -> Option<ElementSpec> {
    let id = e.get("id").and_then(|v| v.as_str())?.to_string();
    let kind = e.get("kind").and_then(|v| v.as_str())?.to_string();
    let z = e.get("z").and_then(|v| v.as_i64()).unwrap_or(0);
    let visible = e.get("visible").and_then(|v| v.as_bool()).unwrap_or(true);
    let opacity = e.get("opacity").and_then(|v| v.as_f64()).unwrap_or(1.0) as f32;
    let color = e
        .get("fields")
        .and_then(|f| f.get("color"))
        .and_then(|c| c.as_str())
        .and_then(parse_hex_rgb);
    Some(ElementSpec {
        id,
        kind,
        z,
        visible,
        asset_id: e
            .get("assetId")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        color,
        rect: rect_of(e.get("transform")),
        opacity,
    })
}

/// Transform → normalized destination rect. Absent fields mean full frame.
fn rect_of(transform: Option<&serde_json::Value>) -> [f32; 4] {
    let Some(t) = transform else { return FULL_RECT };
    let get = |k: &str, d: f64| t.get(k).and_then(|v| v.as_f64()).unwrap_or(d) as f32;
    [get("x", 0.0), get("y", 0.0), get("w", 1.0), get("h", 1.0)]
}

fn parse_hex_rgb(s: &str) -> Option<[f32; 4]> {
    let h = s.strip_prefix('#')?;
    if h.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(h, 16).ok()?;
    Some([
        ((v >> 16) & 0xff) as f32 / 255.0,
        ((v >> 8) & 0xff) as f32 / 255.0,
        (v & 0xff) as f32 / 255.0,
        1.0,
    ])
}

fn parse_quality(s: &str) -> Option<nbe_protocol::QualityProfile> {
    use nbe_protocol::QualityProfile as Q;
    match s {
        "potato" => Some(Q::Potato),
        "consumer" => Some(Q::Consumer),
        "pro" => Some(Q::Pro),
        "reference" => Some(Q::Reference),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Transitions (Step 2): quantized to master-clock frames.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitionKind {
    Cut,
    Mix,
}

/// An in-flight transition between two items on the View bus.
#[derive(Debug, Clone)]
pub struct Transition {
    pub from_item: Option<String>,
    pub to_item: String,
    pub kind: TransitionKind,
    pub duration_frames: u64,
    /// The first frame on which the transition is visible. A transition never
    /// begins mid-frame: the control plane's directive lands, and the next
    /// boundary is where it takes effect.
    pub start_frame: u64,
}

impl Transition {
    /// Progress at `frame`: 0.0 before the start, 1.0 once complete. A cut is
    /// a mix of duration zero.
    pub fn progress(&self, frame: u64) -> f32 {
        if frame < self.start_frame {
            return 0.0;
        }
        if self.kind == TransitionKind::Cut || self.duration_frames == 0 {
            return 1.0;
        }
        let elapsed = frame - self.start_frame;
        if elapsed >= self.duration_frames {
            1.0
        } else {
            elapsed as f32 / self.duration_frames as f32
        }
    }

    pub fn is_complete(&self, frame: u64) -> bool {
        self.progress(frame) >= 1.0
    }
}
