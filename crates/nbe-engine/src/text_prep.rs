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

use crate::text::{FontBook, TextRaster, TextRefusal, TextSpec};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex, MutexGuard};
use std::time::Duration;

/// A lock whose holder panicked is still usable: the maps behind these locks
/// hold whole entries, and the worker's panic guard (review4 step 1) must not
/// turn one failed job into a panic on the render thread's next lookup.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// What became of one request to prepare a text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrepOutcome {
    /// The texture exists.
    Ready,
    /// The worker will not make it: the text does not fit a texture, the GPU
    /// refused the texture, or the job panicked. The reason is for the
    /// operator and the log.
    Refused(String),
    /// Not finished within the bound (or the worker is gone).
    TimedOut,
}

/// A texture ready to draw, with what the ticker's window needs to slide it.
#[derive(Debug, Clone)]
pub struct PreparedText {
    pub tex: wgpu::Texture,
    pub period_px: u32,
    pub width: u32,
}

/// The raster spec for a string in its element's box — the one formula,
/// shared by the render loop's cache and this worker.
///
/// The raster is the box's size in View pixels (`rect` is the element's
/// normalized x, y, w, h), so it is drawn 1:1 and never stretched. Until the
/// sprint's rehearsal dry run it was always View-wide and one 6%-of-height
/// line tall, then stretched into the box: a narrow box squeezed its glyphs,
/// and a second line (a name strap's role, a lower third's subhead) had no
/// room and was not drawn. The font size now fits the box's height to the
/// text's line count, a line being 1.25 of the size, filling 90% of the box.
/// A ticker is one line at 72% of its band's height, laid out as wide as the
/// text needs.
/// Where a text is laid out: its element's box (normalized x, y, w, h) and
/// how far inside it the text sits, in View pixels (its backing's padding).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextBox {
    pub rect: [f32; 4],
    pub inset_px: f32,
}

/// A text layer's box inset by `inset_px` View pixels on every side (its
/// backing's padding): where its text is laid out and drawn. An inset of 0
/// returns the box unchanged, so text without padding is drawn exactly as
/// before. The inner box never goes below one pixel.
pub fn inset_rect(rect: [f32; 4], inset_px: f32, view_w: u32, view_h: u32) -> [f32; 4] {
    if inset_px <= 0.0 {
        return rect;
    }
    let (vw, vh) = (view_w.max(1) as f32, view_h.max(1) as f32);
    let (dx, dy) = (inset_px / vw, inset_px / vh);
    let w = (rect[2] - 2.0 * dx).max(1.0 / vw);
    let h = (rect[3] - 2.0 * dy).max(1.0 / vh);
    [
        rect[0] + (rect[2] - w) / 2.0,
        rect[1] + (rect[3] - h) / 2.0,
        w,
        h,
    ]
}

pub fn spec_for(
    content: &str,
    color: [f32; 4],
    scroll: bool,
    view_w: u32,
    view_h: u32,
    rect: [f32; 4],
) -> TextSpec {
    let box_w = (rect[2] * view_w.max(1) as f32).round().clamp(8.0, 8192.0) as u32;
    let box_h = (rect[3] * view_h.max(1) as f32).round().clamp(8.0, 4096.0) as u32;
    // A ticker's raster holds the item twice, separated by a gap, so the
    // scroll can wrap without a seam and without a repeating sampler.
    let laid_out = if scroll {
        crate::text::ticker_layout(content)
    } else {
        content.to_string()
    };
    let size_px = if scroll {
        box_h as f32 * 0.72
    } else {
        let lines = content.lines().count().max(1) as f32;
        (box_h as f32 * 0.9 / (lines * 1.25)).max(6.0)
    };
    TextSpec {
        text: laid_out,
        size_px,
        color,
        width_px: box_w,
        height_px: box_h,
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
        done: Option<mpsc::SyncSender<PrepOutcome>>,
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
    /// Keys the worker refused. A refused key is never queued again, so the
    /// render loop's per-frame `request` for a layer it cannot draw does not
    /// reshape the same text every frame.
    refused: Arc<Mutex<HashSet<String>>>,
    /// Texts refused (too large for a texture, or a GPU validation error).
    pub refused_total: Arc<AtomicU64>,
    /// Jobs that panicked; each one rebuilt the worker's font book and the
    /// worker went on.
    pub restarts_total: Arc<AtomicU64>,
    /// TEST SEAM: the next job panics inside the worker, as a shaping or GPU
    /// fault would. Production never sets it.
    pub injected_panic_next_job: Arc<AtomicBool>,
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
        let refused: Arc<Mutex<HashSet<String>>> = Arc::default();
        let refused_total = Arc::new(AtomicU64::new(0));
        let restarts_total = Arc::new(AtomicU64::new(0));
        let injected_panic_next_job = Arc::new(AtomicBool::new(false));
        {
            let ready = ready.clone();
            let pending = pending.clone();
            let shaped_total = shaped_total.clone();
            let refused = refused.clone();
            let refused_total = refused_total.clone();
            let restarts_total = restarts_total.clone();
            let injected = injected_panic_next_job.clone();
            std::thread::Builder::new()
                .name("nbe-text-prep".into())
                .spawn(move || {
                    let mut faces: Vec<Vec<u8>> = Vec::new();
                    let mut book = FontBook::from_packaged(Vec::new());
                    let mut generation: Option<u64> = None;
                    while let Ok(msg) = rx.recv() {
                        match msg {
                            Msg::Fonts(g, new_faces) => {
                                if generation.is_some_and(|cur| g <= cur) {
                                    continue;
                                }
                                generation = Some(g);
                                faces = new_faces;
                                book = FontBook::from_packaged(faces.clone());
                                lock(&ready).clear();
                                lock(&refused).clear();
                            }
                            Msg::Job {
                                key,
                                element_id,
                                spec,
                                scroll,
                                done,
                            } => {
                                // One job cannot take the worker down: a panic
                                // in shaping or in a GPU call fails that job,
                                // the font book is rebuilt from the package's
                                // faces, and the next job runs (review4 step 1).
                                let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                                    || {
                                        if injected.swap(false, Ordering::SeqCst) {
                                            panic!("injected text-prep panic (test seam)");
                                        }
                                        prepare_one(
                                            &device,
                                            &queue,
                                            &mut book,
                                            &ready,
                                            &shaped_total,
                                            &key,
                                            &element_id,
                                            &spec,
                                            scroll,
                                        )
                                    },
                                ));
                                let outcome = match ran {
                                    Ok(outcome) => outcome,
                                    Err(_) => {
                                        restarts_total.fetch_add(1, Ordering::SeqCst);
                                        book = FontBook::from_packaged(faces.clone());
                                        tracing::error!(
                                            element = %element_id,
                                            "text prep: a job panicked; the worker rebuilt its fonts and goes on"
                                        );
                                        PrepOutcome::Refused(
                                            "the text worker failed on this text and restarted".into(),
                                        )
                                    }
                                };
                                if let PrepOutcome::Refused(why) = &outcome {
                                    refused_total.fetch_add(1, Ordering::SeqCst);
                                    lock(&refused).insert(key.clone());
                                    tracing::warn!(element = %element_id, reason = %why, "text prep: refused");
                                }
                                lock(&pending).remove(&key);
                                if let Some(done) = done {
                                    let _ = done.send(outcome);
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
            refused,
            refused_total,
            restarts_total,
            injected_panic_next_job,
        })
    }

    /// The package's faces, in declaration order, for load generation
    /// `generation`. A newer generation clears every prepared texture (new
    /// faces, stale pixels); a repeat of the current one changes nothing.
    pub fn set_fonts(&self, generation: u64, faces: Vec<Vec<u8>>) {
        let _ = lock(&self.tx).send(Msg::Fonts(generation, faces));
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

    /// The key and spec a string would be prepared under at the current View
    /// size, laid out in its box inset by `inset_px` ([`inset_rect`]), as the
    /// render loop draws it.
    pub fn key_and_spec(
        &self,
        element_id: &str,
        content: &str,
        color: [f32; 4],
        scroll: bool,
        rect: [f32; 4],
        inset_px: f32,
    ) -> (String, TextSpec) {
        let (w, h) = self.view_size();
        let spec = spec_for(
            content,
            color,
            scroll,
            w,
            h,
            inset_rect(rect, inset_px, w, h),
        );
        (key_for(element_id, &spec), spec)
    }

    /// Ask for a texture without waiting. A request already queued is not
    /// queued twice.
    pub fn request(&self, key: String, element_id: &str, spec: TextSpec, scroll: bool) {
        if lock(&self.ready).contains_key(&key) || lock(&self.refused).contains(&key) {
            return;
        }
        if !lock(&self.pending).insert(key.clone()) {
            return;
        }
        let _ = lock(&self.tx).send(Msg::Job {
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
        text_box: TextBox,
        timeout: Duration,
    ) -> bool {
        self.prepare_checked(element_id, content, color, scroll, text_box, timeout)
            == PrepOutcome::Ready
    }

    /// [`prepare_blocking`](Self::prepare_blocking), saying why a text is not
    /// ready: refused (it will never be) or not yet.
    pub fn prepare_checked(
        &self,
        element_id: &str,
        content: &str,
        color: [f32; 4],
        scroll: bool,
        text_box: TextBox,
        timeout: Duration,
    ) -> PrepOutcome {
        let TextBox { rect, inset_px } = text_box;
        let (key, spec) = self.key_and_spec(element_id, content, color, scroll, rect, inset_px);
        if lock(&self.ready).contains_key(&key) {
            return PrepOutcome::Ready;
        }
        let (done_tx, done_rx) = mpsc::sync_channel(1);
        lock(&self.pending).insert(key.clone());
        if lock(&self.tx)
            .send(Msg::Job {
                key,
                element_id: element_id.to_string(),
                spec,
                scroll,
                done: Some(done_tx),
            })
            .is_err()
        {
            return PrepOutcome::TimedOut;
        }
        done_rx
            .recv_timeout(timeout)
            .unwrap_or(PrepOutcome::TimedOut)
    }

    /// The prepared texture for a key, if the worker has made it.
    pub fn get(&self, key: &str) -> Option<PreparedText> {
        lock(&self.ready).get(key).cloned()
    }

    /// Hand a prepared texture over to the render loop's own cache, removing
    /// it here, so a stream of clock faces or ticker overrides does not
    /// accumulate textures in two places.
    pub fn take(&self, key: &str) -> Option<PreparedText> {
        lock(&self.ready).remove(key)
    }
}

/// One job: shape, check, make the texture inside an error scope, upload.
#[allow(clippy::too_many_arguments)]
fn prepare_one(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    book: &mut FontBook,
    ready: &Mutex<HashMap<String, PreparedText>>,
    shaped_total: &AtomicU64,
    key: &str,
    element_id: &str,
    spec: &TextSpec,
    scroll: bool,
) -> PrepOutcome {
    if lock(ready).contains_key(key) {
        return PrepOutcome::Ready;
    }
    let raster = match TextRaster::rasterize_checked(book, spec) {
        Ok(r) => r,
        // Nothing to draw is not a refusal: the caller sees "not ready", as
        // before this check existed.
        Err(TextRefusal::Nothing) => return PrepOutcome::TimedOut,
        Err(too_large) => return PrepOutcome::Refused(too_large.to_string()),
    };
    shaped_total.fetch_add(1, Ordering::SeqCst);
    let tex = match make_texture_checked(
        device,
        queue,
        raster.width,
        raster.height,
        &raster.rgba,
        element_id,
    ) {
        Ok(tex) => tex,
        Err(why) => return PrepOutcome::Refused(why),
    };
    let period_px = if scroll {
        crate::text::ticker_period_px(raster.text_width)
    } else {
        raster.text_width.max(1)
    };
    lock(ready).insert(
        key.to_string(),
        PreparedText {
            tex,
            period_px,
            width: raster.width,
        },
    );
    PrepOutcome::Ready
}

/// Make a text texture and upload its pixels inside wgpu error scopes, so a
/// texture the device cannot make is an `Err` here instead of the device's
/// uncaptured-error handler, which panics the calling thread (review4
/// step 1). The size check in [`TextRaster::rasterize_checked`] refuses the
/// known case first; this catches whatever else the device refuses.
pub fn make_texture_checked(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    width: u32,
    height: u32,
    rgba: &[u8],
    label: &str,
) -> Result<wgpu::Texture, String> {
    let oom = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let tex = make_texture(device, width, height, label);
    upload(queue, &tex, width, height, rgba);
    let invalid = pop_now(validation.pop());
    let out_of_memory = pop_now(oom.pop());
    match invalid.or(out_of_memory) {
        None => Ok(tex),
        Some(e) => Err(format!(
            "the GPU refused a {width}x{height} text texture: {e}"
        )),
    }
}

/// An error scope's result. On native wgpu the pop resolves at once; a scope
/// that has not resolved after a few polls is read as no error.
fn pop_now(fut: impl std::future::Future<Output = Option<wgpu::Error>>) -> Option<wgpu::Error> {
    let mut fut = std::pin::pin!(fut);
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    for _ in 0..8 {
        if let std::task::Poll::Ready(e) = fut.as_mut().poll(&mut cx) {
            return e;
        }
    }
    None
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The #110 follow-up (ruling of 2026-11-16 21:55, item 5): the text's
    /// box inside its bar. No padding returns the box bit for bit, so text
    /// with no padding is laid out and drawn exactly as before; padding moves
    /// every edge in by that many View pixels; the inner box never inverts.
    #[test]
    fn a_text_box_is_inset_by_its_padding_in_view_pixels() {
        let lower_third = [0.05, 0.75, 0.9, 0.12];
        assert_eq!(inset_rect(lower_third, 0.0, 1920, 1080), lower_third);
        let r = inset_rect(lower_third, 24.0, 1920, 1080);
        let near = |v: f32, n: u32, want: f32, what: &str| {
            let got = v * n as f32;
            assert!((got - want).abs() < 0.01, "{what}: {got} px, want {want}");
        };
        near(r[0], 1920, 96.0 + 24.0, "left edge 24 px in");
        near(r[1], 1080, 810.0 + 24.0, "top edge 24 px in");
        near(r[2], 1920, 1728.0 - 48.0, "24 px off each side");
        near(r[3], 1080, 129.6 - 48.0, "24 px off top and bottom");
        let s0 = spec_for("PADDED", [1.0; 4], false, 1920, 1080, lower_third);
        let s24 = spec_for("PADDED", [1.0; 4], false, 1920, 1080, r);
        assert_eq!((s0.width_px, s0.height_px), (1728, 130));
        assert_eq!(
            (s24.width_px, s24.height_px),
            (1680, 82),
            "laid out at the inner box"
        );
        let tiny = inset_rect([0.0, 0.0, 0.01, 0.01], 100.0, 1920, 1080);
        assert!(tiny[2] > 0.0 && tiny[3] > 0.0, "never inverted: {tiny:?}");
    }

    #[test]
    fn a_raster_is_its_box_and_its_font_fits_the_lines() {
        // A lower third 0.9 x 0.12 of 1080p: 1728 x 130 px, two lines.
        let s = spec_for(
            "HEADLINE\nSUBHEAD",
            [1.0; 4],
            false,
            1920,
            1080,
            [0.05, 0.75, 0.9, 0.12],
        );
        assert_eq!((s.width_px, s.height_px), (1728, 130));
        // Two lines at 1.25 x the size fill at most 90% of the box.
        assert!(
            2.0 * s.size_px * 1.25 <= 130.0 * 0.9 + 0.01,
            "size {}",
            s.size_px
        );
        // One line in the same box is twice the size.
        let one = spec_for(
            "HEADLINE",
            [1.0; 4],
            false,
            1920,
            1080,
            [0.05, 0.75, 0.9, 0.12],
        );
        assert!((one.size_px - 2.0 * s.size_px).abs() < 0.01);
    }

    #[test]
    fn a_ticker_is_one_line_at_its_bands_height_and_as_wide_as_its_text() {
        let s = spec_for("A B C", [1.0; 4], true, 1920, 1080, [0.0, 0.9, 1.0, 0.1]);
        assert_eq!(s.height_px, 108);
        assert!((s.size_px - 108.0 * 0.72).abs() < 0.01);
        assert!(s.grow_to_text);
    }
}
