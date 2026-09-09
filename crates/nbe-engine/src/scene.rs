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
    /// A decoded video asset, addressed by asset id. Which frame is shown is
    /// a function of the master clock, resolved by the render loop
    /// (SPEC §12.1) — the layer only names the source.
    Video {
        asset_id: String,
        /// Whether the element loops (`videoLoop`) or plays once (`clip`).
        looping: bool,
    },
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
    /// item id → `audioPolicy` (SPEC §17: `clip` | `bed` | `mute`, default
    /// `clip`). The manifest states what an item's audio should do; the
    /// resolution below obeys it rather than inferring it.
    pub item_audio_policy: HashMap<String, String>,
    /// scene id → elements, already sorted low-z to high-z.
    pub scenes: HashMap<String, Vec<ElementSpec>>,
    /// overlay id → elements, already sorted low-z to high-z (§7.10). Same
    /// `ElementSpec` shape as scenes: the overlay level composites the same
    /// elements the transition composites, only per its own on-air set.
    pub overlays: HashMap<String, Vec<ElementSpec>>,
    /// overlay id → enter-animation duration in frames, read off the elements'
    /// declared `enterAnimation` at index time. The bound is the maximum across
    /// the overlay's elements; 1 when nothing is declared (a show lands at the
    /// next frame boundary and is complete on it).
    pub overlay_enter_frames: HashMap<String, u64>,
    /// overlay id → exit-animation duration in frames, same derivation.
    pub overlay_exit_frames: HashMap<String, u64>,
    /// asset id → decoded image, for `image`-kind assets only.
    pub images: HashMap<String, DecodedImage>,
    /// asset id → kind, so video references can be skipped by scope, not error.
    pub asset_kind: HashMap<String, String>,
    /// The manifest's requested quality profile (SPEC §10.1.1).
    pub requested_quality: Option<nbe_protocol::QualityProfile>,
    /// asset id → source path, for assets decoded after indexing (video).
    pub asset_source: HashMap<String, String>,
    /// asset id → declared `loop.periodFrames` (SPEC §12.9: the manifest's
    /// declaration takes precedence over the decoded frame count).
    pub declared_loop_period: HashMap<String, u32>,
    /// asset id → declared `loop.vramBudgetMib` (SPEC §12.4/§12.5).
    ///
    /// The engine ignored this entirely and planned every loop against a
    /// hardcoded 1024 MiB, so a package asking for a small cache got a large
    /// one and preflight's residency answer did not match the engine's.
    pub declared_vram_budget_mib: HashMap<String, u32>,
    /// The house rate the manifest declares (`show.video.frameRate`).
    ///
    /// Carried so the engine can check it against the rate it is actually
    /// running. Nothing reconciled the two before: the engine takes its rate
    /// from `NBE_HOUSE_RATE` (default 30) and never looked at the manifest, so
    /// a 25 fps package loaded without that variable mapped every
    /// non-house-rate asset against 30 and no path detected it.
    pub declared_house_rate: Option<u32>,
}

impl PackageIndex {
    /// The §12.4 budget one loop asset is planned against.
    ///
    /// Named once, here, because the engine and preflight must plan the same
    /// loop against the same ceiling. The engine used to build a
    /// `CacheBudget { per_loop_mib: 1024, total_mib: 4096 }` inline at the load
    /// site — four and eight times §12.4's table, a Prompt 05 placeholder that
    /// outlived the spec section that would have corrected it — and read the
    /// manifest's `vramBudgetMib` not at all. Preflight meanwhile held §12.4's
    /// real numbers, so the two sides of one shared planner disagreed about
    /// which loops are resident: a 100-frame 1080p RGBA8 loop was streamed and
    /// charged nothing by preflight while the engine made it resident at
    /// 791 MiB, and the package was air-ready.
    pub fn loop_budget(&self, asset_id: &str) -> nbe_core::loop_cache::CacheBudget {
        nbe_core::loop_cache::CacheBudget::from_manifest(
            self.declared_vram_budget_mib.get(asset_id).copied(),
        )
    }
}

/// One element, reduced to what this prompt can draw.
#[derive(Debug, Clone, PartialEq)]
pub struct ElementSpec {
    pub id: String,
    pub kind: String,
    pub z: i64,
    pub visible: bool,
    pub asset_id: Option<String>,
    /// `element.audio.bus` (SPEC §7.4 `LayerAudio`), defaulted to `clip`.
    /// The manifest says which bus an element's audio belongs on; the audio
    /// resolution reads it instead of guessing from z-order.
    pub audio_bus: String,
    /// `element.audio.muted` (SPEC §7.4). Also in the schema, also previously
    /// unread — a muted element went to air.
    pub audio_muted: bool,
    pub color: Option<[f32; 4]>,
    pub rect: [f32; 4],
    pub opacity: f32,
    /// `enterAnimation.durationFrames`, if the element declares one. The
    /// overlay level reads these to bound a show/hide animation (§7.10); the
    /// scene path does not animate.
    pub enter_frames: Option<u64>,
    /// `exitAnimation.durationFrames`, same derivation.
    pub exit_frames: Option<u64>,
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
            declared_house_rate: manifest
                .get("show")
                .and_then(|sh| sh.get("video"))
                .and_then(|v| v.get("frameRate"))
                .and_then(|v| v.as_u64())
                .map(|r| r as u32),
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
            idx.asset_source.insert(id.to_string(), src.to_string());
            if let Some(period) = asset
                .get("loop")
                .and_then(|l| l.get("periodFrames"))
                .and_then(|v| v.as_u64())
            {
                idx.declared_loop_period
                    .insert(id.to_string(), period as u32);
            }
            if let Some(mib) = asset
                .get("loop")
                .and_then(|l| l.get("vramBudgetMib"))
                .and_then(|v| v.as_u64())
            {
                idx.declared_vram_budget_mib
                    .insert(id.to_string(), mib.min(u32::MAX as u64) as u32);
            }
            if kind != "image" {
                continue; // video is decoded by `video.rs`, not here
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

        for overlay in manifest
            .get("overlays")
            .and_then(|s| s.as_array())
            .into_iter()
            .flatten()
        {
            let Some(id) = overlay.get("id").and_then(|v| v.as_str()) else {
                continue;
            };
            let mut elements: Vec<ElementSpec> = overlay
                .get("elements")
                .and_then(|e| e.as_array())
                .into_iter()
                .flatten()
                .filter_map(element_spec)
                .collect();
            elements.sort_by_key(|e| e.z);
            idx.overlay_enter_frames.insert(
                id.to_string(),
                elements.iter().filter_map(|e| e.enter_frames).max().unwrap_or(1),
            );
            idx.overlay_exit_frames.insert(
                id.to_string(),
                elements.iter().filter_map(|e| e.exit_frames).max().unwrap_or(1),
            );
            idx.overlays.insert(id.to_string(), elements);
        }

        index_sequence(manifest.get("rundown"), &mut idx);
        idx
    }

    /// The audio-bearing asset an item shows: item → scene → elements → asset.
    ///
    /// The forward direction of `items_using_asset`, and the fix for the P0 the
    /// midpoint review found. Audio is made resident at `show.load` keyed by
    /// **asset id**; a take arrives carrying an **item ref**. Every package
    /// used in testing happened to name them the same string, so the lookup
    /// appeared to work; on the dress show — item `A1`, asset `A1_clip` — it
    /// missed and the take installed no source. Every bus sat at −120 dBFS
    /// with the graph, the driver and the decode path all working.
    ///
    /// Resolved at load and arm time, never on the take path: §7.13 forbids
    /// this kind of walk on the frame path, and a take must not depend on a
    /// map lookup that can fail silently.
    ///
    /// The asset whose audio a take installs on the CLIP bus for this item.
    ///
    /// Four rules have been wrong here and five production bugs came out of
    /// them, every one the same mistake: inferring from an element's POSITION
    /// what the manifest STATES. So this version states its premises and reads
    /// every field the schema provides.
    ///
    /// The candidate must be **drawn** — it comes from `drawn_elements`, the
    /// same walk `resolve` uses, so it cannot be an element the renderer never
    /// shows. Then, all of:
    ///
    /// - the item is not `audioPolicy: "mute"` (SPEC §17)
    /// - the element is a `clip` — programme picture. A `videoLoop` is a loop
    ///   or an overlay bug; its audio is not the take's programme audio
    /// - the element does not declare `audio.muted` (SPEC §7.4)
    /// - the element's `audio.bus` is `clip`. **No positional fallback.** An
    ///   element declaring `guest`, `music` or `sfx` is declaring that its
    ///   audio is not programme audio, and the previous rule overrode that
    ///   declaration with "…but it was first". Routing those to their declared
    ///   buses needs more than one source, which is §8.7.5 per-source envelope
    ///   work deferred to Prompt 07
    /// - its asset is `video` or `alphaVideo` — picture that can carry a track
    ///
    /// Ties break by z (lowest first), then manifest order, both from the
    /// stable sort in `build`. Documented because a picture-in-picture has two
    /// legitimate clip elements.
    ///
    /// `audioPolicy: "bed"` resolves as `clip` and is NOT implemented: a bed
    /// playing alongside the clip is two sources on two buses. Deferred to
    /// Prompt 07 with the per-source envelope work, and logged so an author
    /// who writes `bed` is not silently given `clip`.
    pub fn item_audio_asset(&self, item_ref: &str) -> Option<String> {
        match self.item_audio_policy.get(item_ref).map(String::as_str) {
            Some("mute") => return None,
            Some("bed") => tracing::warn!(
                item = %item_ref,
                "audioPolicy \"bed\" is not implemented; treating as \"clip\" \
                 (SPEC §8.7.5, deferred to Prompt 07)"
            ),
            _ => {}
        }
        self.drawn_elements(item_ref)
            .into_iter()
            .filter(|e| e.kind == "clip")
            .filter(|e| !e.audio_muted)
            .filter(|e| e.audio_bus == "clip")
            .find_map(|e| {
                let asset = e.asset_id.as_deref()?;
                self.asset_is_picture_with_audio(asset)
                    .then(|| asset.to_string())
            })
    }

    /// Whether an asset is picture that can also carry an audio track.
    ///
    /// `audio`-kind assets are deliberately excluded: a soundboard stab or a
    /// music bed is not a take's programme audio, and treating it as one is
    /// how a bed authored below the clip silenced the clip.
    fn asset_is_picture_with_audio(&self, asset_id: &str) -> bool {
        matches!(
            self.asset_kind.get(asset_id).map(String::as_str),
            Some("video") | Some("alphaVideo")
        )
    }

    /// item ref → the asset whose audio that item plays, for every item in the
    /// package. Built once at load.
    pub fn item_audio_map(&self) -> std::collections::BTreeMap<String, String> {
        self.item_scene
            .keys()
            .filter_map(|item| {
                self.item_audio_asset(item)
                    .map(|asset| (item.clone(), asset))
            })
            .collect()
    }

    /// Which rundown items would SHOW this asset.
    ///
    /// A decode failure has to be attributed to something the control plane's
    /// §17.3 machine tracks. That machine tracks Items; an asset id is
    /// unresolvable to it, so a fault reported against an asset cannot drive
    /// anything to `ERROR` (SPEC §5.9.3).
    ///
    /// Derived from the LAYERS an item draws, and that is the whole point.
    ///
    /// That is not cosmetic. `server.ts` turns an `itemEvent` into
    /// `state.markError(itemRef)`; §17.3 makes `ERROR` recoverable only via
    /// `item.reset`, and if the item is on air the control plane raises
    /// `fallbackActive`. So a broken asset the renderer never touched takes a
    /// healthy item off the air.
    ///
    /// This has been wrong twice, each time because the question was asked of
    /// the wrong thing:
    ///
    /// 1. It walked `scenes` → `item_scene` directly, with none of
    ///    `drawn_elements`' filters, so it blamed items for assets behind
    ///    `visible: false`, at `opacity: 0`, on an element kind the renderer
    ///    does not draw, or in the scene of a slated item.
    /// 2. Fixing that unified the ELEMENT predicate — `drawn_elements` plus
    ///    `layer_for(e).is_some()` — but left the ASSET predicate independent:
    ///    `e.asset_id == Some(asset_id)`. Those are two different questions,
    ///    and `layer_for`'s `graphic` arm returns `Some(Solid)` **without ever
    ///    reading `asset_id`**. The schema permits `assetId` on every element
    ///    kind, so a `graphic` card carrying one is drawn as a solid, shows
    ///    nothing of the asset, and was still blamed for it.
    ///
    /// So the predicate now asks the LAYER what it shows, rather than asking
    /// the element what it mentions. An element can only be blamed for an
    /// asset its own layer names. That closes the class rather than the
    /// instance: a future `layer_for` arm that ignores `asset_id` is correct
    /// here by construction instead of needing another filter.
    pub fn items_using_asset(&self, asset_id: &str) -> Vec<String> {
        let mut items: Vec<String> = self
            .item_scene
            .keys()
            .filter(|item| {
                self.drawn_elements(item)
                    .iter()
                    .filter_map(|e| self.layer_for(e))
                    .any(|layer| match &layer.source {
                        LayerSource::Image(a) => a == asset_id,
                        LayerSource::Video { asset_id: a, .. } => a == asset_id,
                        // A solid shows no asset, whatever the element names.
                        LayerSource::Solid(_) => false,
                    })
            })
            .cloned()
            .collect();
        items.sort();
        items
    }

    /// Resolve one overlay to drawable layers (§7.10). An overlay composites above
    /// the transition; its elements resolve through the same `layer_for` walk as
    /// scene elements, so the two can never disagree about what a given element
    /// draws.
    pub fn resolve_overlay(&self, overlay_id: &str) -> ResolvedScene {
        let layers = self
            .overlays
            .get(overlay_id)
            .map(|els| els.iter().filter_map(|e| self.layer_for(e)).collect())
            .unwrap_or_default();
        ResolvedScene { layers }
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
        let layers = self
            .drawn_elements(item)
            .into_iter()
            .filter_map(|e| self.layer_for(e))
            .collect();
        ResolvedScene { layers }
    }

    /// The elements this item actually puts on screen, in draw order.
    ///
    /// The single source of truth for "what is this item showing". `resolve`
    /// turns these into layers, and `item_audio_asset` picks the take's audio
    /// from the same list.
    ///
    /// Deriving both from one function is the point. Five production bugs have
    /// been found in the audio rule across four rewrites, and the sharpest
    /// class — audio resolving to an asset the renderer never draws — kept
    /// surviving because the two paths walked the scene SEPARATELY and could
    /// disagree. A `slate` item was the last instance: `resolve` short-circuits
    /// to a generated solid and never reads the scene, while the audio walk
    /// read the scene anyway, so slating an item left its clip audio on air.
    ///
    /// One walk cannot disagree with itself. That is structural, in the same
    /// sense as §8.6's mix-minus: not "tested against", but unrepresentable.
    fn drawn_elements(&self, item_ref: &str) -> Vec<&ElementSpec> {
        // A slate draws a generated solid and nothing from the scene, so it
        // shows no elements — and therefore carries no item audio.
        if self.item_kind.get(item_ref).map(String::as_str) == Some("slate") {
            return Vec::new();
        }
        let Some(scene_id) = self.item_scene.get(item_ref) else {
            return Vec::new();
        };
        let Some(elements) = self.scenes.get(scene_id) else {
            return Vec::new();
        };
        elements
            .iter()
            .filter(|e| e.visible)
            // Fully transparent is not on screen. `visible: false` was
            // filtered and `opacity: 0` was not, so a transparent element was
            // not the picture but was the audio.
            .filter(|e| e.opacity > 0.0)
            .collect()
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
                    // Prompt 05: video is drawn. The scope boundary Prompt 04
                    // drew is gone; a genuine decode failure is now a fault the
                    // caller reports as `itemEvent: decodeError`.
                    Some("video") | Some("alphaVideo") => LayerSource::Video {
                        asset_id: asset.to_string(),
                        looping: e.kind == "videoLoop",
                    },
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
        idx.item_audio_policy.insert(
            id.to_string(),
            item.get("audioPolicy")
                .and_then(|v| v.as_str())
                .unwrap_or("clip")
                .to_string(),
        );
        if let Some(scene) = item.get("sceneRef").and_then(|v| v.as_str()) {
            idx.item_scene.insert(id.to_string(), scene.to_string());
        }
    }
}

fn element_spec(e: &serde_json::Value) -> Option<ElementSpec> {
    // Lenient on air, but never silent: an element dropped here is a piece of
    // the picture that will not be there, and someone will be looking at a
    // black frame wondering why.
    let Some(id) = e.get("id").and_then(|v| v.as_str()).map(str::to_string) else {
        tracing::warn!(element = %e, "scene element has no id; skipping");
        return None;
    };
    let Some(kind) = e.get("kind").and_then(|v| v.as_str()).map(str::to_string) else {
        tracing::warn!(element = %id, "scene element has no kind; skipping");
        return None;
    };
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
        audio_bus: e
            .get("audio")
            .and_then(|a| a.get("bus"))
            .and_then(|v| v.as_str())
            .unwrap_or("clip")
            .to_string(),
        audio_muted: e
            .get("audio")
            .and_then(|a| a.get("muted"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        color,
        rect: rect_of(e.get("transform")),
        opacity,
        enter_frames: e
            .get("enterAnimation")
            .and_then(|a| a.get("durationFrames"))
            .and_then(|v| v.as_u64()),
        exit_frames: e
            .get("exitAnimation")
            .and_then(|a| a.get("durationFrames"))
            .and_then(|v| v.as_u64()),
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
    /// The master frame the OUTGOING item went on air. A mix renders both
    /// scenes, and each must be read at its own point in its own timeline.
    pub from_start_frame: u64,
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

#[cfg(test)]
mod budget_tests {
    use super::*;
    use nbe_core::loop_cache::{self, CachePolicy, LoopSpec};

    fn index_with_loop(loop_json: &str) -> PackageIndex {
        let manifest: serde_json::Value = serde_json::from_str(&format!(
            r#"{{
              "manifestVersion": "0.4",
              "show": {{ "video": {{ "width": 1920, "height": 1080, "frameRate": 30 }} }},
              "assets": [
                {{ "id": "L", "kind": "video", "source": "media/l.mp4", "loop": {loop_json} }}
              ],
              "scenes": [], "rundown": {{ "id": "R", "items": [] }}
            }}"#
        ))
        .expect("fixture parses");
        PackageIndex::build(&manifest, std::path::Path::new("/nonexistent"))
    }

    /// A 100-frame 1080p RGBA8 loop, planned the way the engine plans it.
    fn plan_100_rgba8(budget: loop_cache::CacheBudget) -> loop_cache::CachePlan {
        loop_cache::plan(
            LoopSpec {
                width: 1920,
                height: 1080,
                period_frames: 100,
                has_alpha: false,
                // The engine's decode path is RGBA8 today (`video.rs`).
                yuv_sampling: false,
                gop_frames: 0,
                declared_format: None,
            },
            budget,
        )
    }

    #[test]
    fn an_undeclared_loop_is_planned_against_the_spec_table() {
        // SPEC §12.4: 256 MiB per loop, 512 MiB total. The engine planned every
        // loop against a hardcoded 1024/4096 instead — four and eight times the
        // table — so it held loops preflight had reported as streamed and
        // charged nothing for. Nothing tested it, which is why a placeholder
        // from Prompt 05 survived the section that superseded it.
        let index = index_with_loop(r#"{ "periodFrames": 100 }"#);
        let budget = index.loop_budget("L");
        assert_eq!(budget.per_loop_mib(), loop_cache::DEFAULT_PER_LOOP_MIB);
        assert_eq!(budget.total_mib(), loop_cache::DEFAULT_TOTAL_LOOP_MIB);
        assert_eq!(budget.effective_mib(), 256);

        // And the consequence that made the divergence visible: at 256 MiB a
        // 100-frame RGBA8 loop does NOT fit (32 frames do), so the engine
        // streams it — the same answer preflight reports.
        let plan = plan_100_rgba8(budget);
        assert_eq!(plan.max_frames_by_budget, 32);
        assert_eq!(plan.cache_policy_selected, CachePolicy::Streaming);
    }

    #[test]
    fn a_declared_vram_budget_is_honoured() {
        // The engine read `vramBudgetMib` nowhere, so a package asking for a
        // small cache got a large one.
        let small = index_with_loop(r#"{ "periodFrames": 100, "vramBudgetMib": 128 }"#);
        assert_eq!(small.loop_budget("L").per_loop_mib(), 128);
        assert_eq!(small.loop_budget("L").effective_mib(), 128);

        // A declaration ABOVE §12.4's total is still bounded by it: the total
        // short-loop budget is shared, and one loop may not claim past it.
        let large = index_with_loop(r#"{ "periodFrames": 100, "vramBudgetMib": 1024 }"#);
        assert_eq!(large.loop_budget("L").per_loop_mib(), 1024);
        assert_eq!(
            large.loop_budget("L").effective_mib(),
            loop_cache::DEFAULT_TOTAL_LOOP_MIB,
            "§12.4's total is a ceiling, not a suggestion"
        );

        // An asset with no loop at all falls back to the table.
        assert_eq!(
            small.loop_budget("nosuch").per_loop_mib(),
            loop_cache::DEFAULT_PER_LOOP_MIB
        );
    }
}
