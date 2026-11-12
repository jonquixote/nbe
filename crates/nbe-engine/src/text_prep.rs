//! Text shaped and uploaded off the render thread.
//!
//! Shaping a line is milliseconds (cosmic-text with advanced shaping), and
//! uploading its raster is a `write_texture` with its own staging buffer. The
//! render loop's own text path did both inline on the frame that first needed
//! the string — "off the frame path" only in the sense that it happened once
//! per content change. For text a *command* changes at an arbitrary moment
//! (`graphic.show`/`update`, `breaking.show`, `ticker.override`), that moment
//! is a frame on air.
//!
//! [`TextPrep`] owns one worker thread with its own [`FontBook`] and a clone
//! of the render device and queue. A command handler asks it to prepare a
//! string and waits (on the blocking pool, never the render thread) until the
//! texture exists; only then does it publish the graphic. The render loop
//! finds the texture already made and only looks it up. A clock asks for its
//! *next* second's face a second early, so a face change is a lookup too.
//!
//! The key a texture is prepared under is the render loop's own cache key
//! ([`key_for`] over [`spec_for`]), so the two sides cannot disagree about
//! which pixels a string needs.

use crate::text::{FontBook, TextRaster, TextSpec};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

/// A texture ready to draw, with what the ticker's window needs to slide it.
#[derive(Debug, Clone)]
pub struct PreparedText {
    pub tex: wgpu::Texture,
    pub period_px: u32,
    pub width: u32,
}

/// The raster spec for a string at a View size — the one formula, shared by
/// the render loop's cache and this worker.
///
/// The raster is sized against the View, not the element box: a band 10% of a
/// 1080-line frame is 108 px tall, and text sized to the box is text that
/// reads the same at any output resolution.
pub fn spec_for(
    content: &str,
    color: [f32; 4],
    scroll: bool,
    view_w: u32,
    view_h: u32,
) -> TextSpec {
    let height_px = (view_h.max(1) as f32 * 0.06).round().max(8.0) as u32;
    // A ticker's raster holds the item twice, separated by a gap, so the
    // scroll can wrap without a seam and without a repeating sampler.
    let laid_out = if scroll {
        crate::text::ticker_layout(content)
    } else {
        content.to_string()
    };
    TextSpec {
        text: laid_out,
        size_px: height_px as f32 * 0.72,
        color,
        width_px: view_w.max(1),
        height_px,
        grow_to_text: scroll,
    }
}

/// The cache key: element id plus the spec's identity.
pub fn key_for(element_id: &str, spec: &TextSpec) -> String {
    format!("{}|{}", element_id, spec.cache_key())
}

enum Msg {
    /// A package's faces, tagged with its load generation. Both the load and
    /// the render loop's package sync send it; the second for one generation
    /// is a no-op, so a texture prepared between the two is not thrown away.
    Fonts(u64, Vec<Vec<u8>>),
    Job {
        key: String,
        element_id: String,
        spec: TextSpec,
        scroll: bool,
        done: Option<mpsc::SyncSender<bool>>,
    },
}

/// The worker and the textures it has made.
pub struct TextPrep {
    tx: Mutex<mpsc::Sender<Msg>>,
    ready: Arc<Mutex<HashMap<String, PreparedText>>>,
    pending: Arc<Mutex<HashSet<String>>>,
    view_w: AtomicU32,
    view_h: AtomicU32,
    /// Rasterizations the worker has done.
    pub shaped_total: Arc<AtomicU64>,
    /// Frames on which the render loop needed a prepared-only texture that was
    /// not ready, and drew that layer as nothing rather than shape it inline.
    pub misses_total: AtomicU64,
}

impl std::fmt::Debug for TextPrep {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextPrep").finish_non_exhaustive()
    }
}

impl TextPrep {
    /// Start the worker with clones of the render device and queue.
    pub fn spawn(device: wgpu::Device, queue: wgpu::Queue, view_w: u32, view_h: u32) -> Arc<Self> {
        let (tx, rx) = mpsc::channel::<Msg>();
        let ready: Arc<Mutex<HashMap<String, PreparedText>>> = Arc::default();
        let pending: Arc<Mutex<HashSet<String>>> = Arc::default();
        let shaped_total = Arc::new(AtomicU64::new(0));
        {
            let ready = ready.clone();
            let pending = pending.clone();
            let shaped_total = shaped_total.clone();
            std::thread::Builder::new()
                .name("nbe-text-prep".into())
                .spawn(move || {
                    let mut book = FontBook::from_packaged(Vec::new());
                    let mut generation: Option<u64> = None;
                    while let Ok(msg) = rx.recv() {
                        match msg {
                            Msg::Fonts(g, faces) => {
                                if generation.is_some_and(|cur| g <= cur) {
                                    continue;
                                }
                                generation = Some(g);
                                book = FontBook::from_packaged(faces);
                                ready.lock().unwrap().clear();
                            }
                            Msg::Job {
                                key,
                                element_id,
                                spec,
                                scroll,
                                done,
                            } => {
                                let made = if ready.lock().unwrap().contains_key(&key) {
                                    true
                                } else if let Some(raster) = TextRaster::rasterize(&mut book, &spec)
                                {
                                    shaped_total.fetch_add(1, Ordering::SeqCst);
                                    let tex = make_texture(
                                        &device,
                                        raster.width,
                                        raster.height,
                                        &element_id,
                                    );
                                    upload(&queue, &tex, raster.width, raster.height, &raster.rgba);
                                    let period_px = if scroll {
                                        crate::text::ticker_period_px(raster.text_width)
                                    } else {
                                        raster.text_width.max(1)
                                    };
                                    ready.lock().unwrap().insert(
                                        key.clone(),
                                        PreparedText {
                                            tex,
                                            period_px,
                                            width: raster.width,
                                        },
                                    );
                                    true
                                } else {
                                    false
                                };
                                pending.lock().unwrap().remove(&key);
                                if let Some(done) = done {
                                    let _ = done.send(made);
                                }
                            }
                        }
                    }
                })
                .expect("spawn the text-prep thread");
        }
        Arc::new(Self {
            tx: Mutex::new(tx),
            ready,
            pending,
            view_w: AtomicU32::new(view_w),
            view_h: AtomicU32::new(view_h),
            shaped_total,
            misses_total: AtomicU64::new(0),
        })
    }

    /// The package's faces, in declaration order, for load generation
    /// `generation`. A newer generation clears every prepared texture (new
    /// faces, stale pixels); a repeat of the current one changes nothing.
    pub fn set_fonts(&self, generation: u64, faces: Vec<Vec<u8>>) {
        let _ = self.tx.lock().unwrap().send(Msg::Fonts(generation, faces));
    }

    /// The View size specs are computed against.
    pub fn set_view_size(&self, w: u32, h: u32) {
        self.view_w.store(w, Ordering::SeqCst);
        self.view_h.store(h, Ordering::SeqCst);
    }

    pub fn view_size(&self) -> (u32, u32) {
        (
            self.view_w.load(Ordering::SeqCst),
            self.view_h.load(Ordering::SeqCst),
        )
    }

    /// The key and spec a string would be prepared under at the current View size.
    pub fn key_and_spec(
        &self,
        element_id: &str,
        content: &str,
        color: [f32; 4],
        scroll: bool,
    ) -> (String, TextSpec) {
        let (w, h) = self.view_size();
        let spec = spec_for(content, color, scroll, w, h);
        (key_for(element_id, &spec), spec)
    }

    /// Ask for a texture without waiting. A request already queued is not
    /// queued twice.
    pub fn request(&self, key: String, element_id: &str, spec: TextSpec, scroll: bool) {
        if self.ready.lock().unwrap().contains_key(&key) {
            return;
        }
        if !self.pending.lock().unwrap().insert(key.clone()) {
            return;
        }
        let _ = self.tx.lock().unwrap().send(Msg::Job {
            key,
            element_id: element_id.to_string(),
            spec,
            scroll,
            done: None,
        });
    }

    /// Prepare a texture and wait until it exists (or `timeout` passes).
    /// Blocking: call it from a command handler's blocking pool, never from
    /// the render thread. Returns whether the texture is ready.
    pub fn prepare_blocking(
        &self,
        element_id: &str,
        content: &str,
        color: [f32; 4],
        scroll: bool,
        timeout: Duration,
    ) -> bool {
        let (key, spec) = self.key_and_spec(element_id, content, color, scroll);
        if self.ready.lock().unwrap().contains_key(&key) {
            return true;
        }
        let (done_tx, done_rx) = mpsc::sync_channel(1);
        self.pending.lock().unwrap().insert(key.clone());
        if self
            .tx
            .lock()
            .unwrap()
            .send(Msg::Job {
                key,
                element_id: element_id.to_string(),
                spec,
                scroll,
                done: Some(done_tx),
            })
            .is_err()
        {
            return false;
        }
        done_rx.recv_timeout(timeout).unwrap_or(false)
    }

    /// The prepared texture for a key, if the worker has made it.
    pub fn get(&self, key: &str) -> Option<PreparedText> {
        self.ready.lock().unwrap().get(key).cloned()
    }

    /// Hand a prepared texture over to the render loop's own cache, removing
    /// it here, so a stream of clock faces or ticker overrides does not
    /// accumulate textures in two places.
    pub fn take(&self, key: &str) -> Option<PreparedText> {
        self.ready.lock().unwrap().remove(key)
    }
}

fn make_texture(device: &wgpu::Device, width: u32, height: u32, label: &str) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    })
}

fn upload(queue: &wgpu::Queue, tex: &wgpu::Texture, width: u32, height: u32, rgba: &[u8]) {
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * 4),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
}
