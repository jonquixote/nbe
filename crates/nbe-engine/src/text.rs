//! Text shaping and rasterization for the graphics layer (Prompt 07b).
//!
//! **Everything here runs off the frame path.** SPEC §6.5 forbids per-frame
//! relayout and §7.13 gives the frame budget no room for one: shaping a line of
//! Arabic is milliseconds, and the compositor's whole budget is 33 ms. So a
//! [`TextRaster`] is produced when *content* changes and held as pixels until it
//! changes again. The ticker scrolls by moving where the compositor samples that
//! texture, never by shaping it again.
//!
//! **Fonts come from the show package and nowhere else.** §0.1 assumption 11
//! forbids a host-system fallback, and `docs/portability.md` row 7 records why
//! that is an asset rather than a limitation: a package that carries its faces
//! renders identically on a machine that has never seen it. [`FontBook`] is
//! therefore built by handing it font *bytes*; it has no path to a system
//! directory, and `cosmic-text` is compiled without `fontconfig` so the
//! machinery to find one is not linked in.

use cosmic_text::{Attrs, Buffer, Family, FontSystem, Metrics, Shaping, SwashCache};

/// The faces a show package declared, and nothing else.
///
/// Built from bytes: there is deliberately no constructor that takes a
/// directory. A caller that wants a face must have read it out of the package,
/// which is the only place a face may come from.
pub struct FontBook {
    system: FontSystem,
    cache: SwashCache,
    /// Family names registered, in declaration order. The first is the default
    /// for text that names no family.
    families: Vec<String>,
}

impl std::fmt::Debug for FontBook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FontBook")
            .field("families", &self.families)
            .finish_non_exhaustive()
    }
}

impl FontBook {
    /// Build from packaged font bytes, in the order the manifest declared them.
    ///
    /// An unreadable face is skipped rather than fatal: preflight is what
    /// refuses a package whose `fontAssetIds` do not resolve (§19), and the
    /// engine's job at this point is to draw what it was given. A book with no
    /// usable face draws nothing, which [`TextRaster::rasterize`] reports as
    /// `None` rather than as a panic on air.
    pub fn from_packaged(faces: Vec<Vec<u8>>) -> Self {
        // An empty locale list and no system source: the database starts empty
        // and only gains what we put in it.
        let mut system = FontSystem::new_with_locale_and_db(
            "en-US".to_string(),
            cosmic_text::fontdb::Database::new(),
        );
        let mut families = Vec::new();
        for bytes in faces {
            let db = system.db_mut();
            let before: Vec<_> = db.faces().map(|f| f.id).collect();
            db.load_font_data(bytes);
            // Name the family the face actually declares, rather than assuming.
            for face in db.faces() {
                if !before.contains(&face.id) {
                    if let Some((name, _)) = face.families.first() {
                        if !families.contains(name) {
                            families.push(name.clone());
                        }
                    }
                }
            }
        }
        Self {
            system,
            cache: SwashCache::new(),
            families,
        }
    }

    /// The families this book holds, in declaration order.
    pub fn families(&self) -> &[String] {
        &self.families
    }

    /// True when no packaged face loaded. Rasterizing against an empty book
    /// yields `None`; it never falls back to a system face.
    pub fn is_empty(&self) -> bool {
        self.families.is_empty()
    }
}

/// What to draw, resolved. Everything here is content — change any field and the
/// raster must be rebuilt; change nothing and the existing pixels stand.
#[derive(Debug, Clone, PartialEq)]
pub struct TextSpec {
    pub text: String,
    /// Pixel height of one line.
    pub size_px: f32,
    /// RGBA, straight (not premultiplied), 0..1.
    pub color: [f32; 4],
    /// Target box in pixels. Text is laid out to this width; the raster is this
    /// size unless `grow_to_text` widens it.
    pub width_px: u32,
    pub height_px: u32,
    /// The ticker's case: lay out on one unwrapped line and make the raster as
    /// wide as the text needs, so scrolling is a sampling offset over a texture
    /// that already contains the whole item.
    pub grow_to_text: bool,
}

impl TextSpec {
    /// The identity a cache keys on. Two specs with the same key produce the
    /// same pixels, which is what makes "shape once per content change" a rule
    /// a cache can enforce rather than a promise a caller has to keep.
    pub fn cache_key(&self) -> String {
        format!(
            "{}|{:.2}|{:?}|{}x{}|{}",
            self.text, self.size_px, self.color, self.width_px, self.height_px, self.grow_to_text
        )
    }
}

/// Rasterized text: straight RGBA8, tightly packed, ready to upload.
#[derive(Debug, Clone, PartialEq)]
pub struct TextRaster {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// How wide the shaped text actually ran, in pixels. The ticker needs this:
    /// its scroll period is the text's own width, not the band's.
    pub text_width: u32,
}

impl TextRaster {
    /// Shape and rasterize. `None` when the book has no face, or the spec asks
    /// for a zero-area raster.
    ///
    /// Called on content change only. Never call this from the frame path.
    pub fn rasterize(book: &mut FontBook, spec: &TextSpec) -> Option<Self> {
        if book.is_empty() || spec.width_px == 0 || spec.height_px == 0 {
            return None;
        }

        let family = book.families.first()?.clone();
        let attrs = Attrs::new().family(Family::Name(&family));
        let metrics = Metrics::new(spec.size_px, spec.size_px * 1.25);
        let mut buffer = Buffer::new(&mut book.system, metrics);

        // A ticker item is one unwrapped line; everything else wraps to its box.
        //
        // `None` is the API's own way to say "do not wrap". Passing `f32::MAX`
        // instead looks equivalent and is not: it reaches glyph placement as a
        // real coordinate, and cosmic-text's sub-pixel bucketing overflows on
        // it — `attempt to add with overflow` in `glyph_cache.rs`, reproduced
        // here on the first Arabic string that shaped into more than one run.
        let layout_width = if spec.grow_to_text {
            None
        } else {
            Some(spec.width_px as f32)
        };
        buffer.set_size(layout_width, Some(spec.height_px as f32));
        // `Shaping::Advanced` is what makes RTL and joined scripts correct
        // rather than a left-to-right run of isolated forms (AC-15). It is the
        // whole reason a shaping engine is here instead of a glyph table.
        buffer.set_text(&spec.text, &attrs, Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut book.system, false);

        // How wide the text ran, measured from the shaped runs.
        let mut text_width_f: f32 = 0.0;
        for run in buffer.layout_runs() {
            text_width_f = text_width_f.max(run.line_w);
        }
        let text_width = text_width_f.ceil().max(1.0) as u32;

        let width = if spec.grow_to_text {
            text_width.max(1)
        } else {
            spec.width_px
        };
        let height = spec.height_px;
        let mut rgba = vec![0u8; (width as usize) * (height as usize) * 4];

        let [r, g, b, a] = spec.color;
        let (fr, fg, fb) = ((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8);
        let fa = a.clamp(0.0, 1.0);

        // swash gives coverage per pixel; the colour is ours. Coverage becomes
        // alpha, so the compositor's existing blend does the rest and text needs
        // no special blending path.
        let FontBook {
            system,
            cache,
            families: _,
        } = book;
        buffer.draw(
            system,
            cache,
            cosmic_text::Color::rgba(fr, fg, fb, 255),
            |x, y, w, h, colour| {
                let cov = colour.a() as f32 / 255.0;
                if cov <= 0.0 {
                    return;
                }
                for dy in 0..h {
                    for dx in 0..w {
                        let px = x + dx as i32;
                        let py = y + dy as i32;
                        if px < 0 || py < 0 || px >= width as i32 || py >= height as i32 {
                            continue;
                        }
                        let i = ((py as usize) * (width as usize) + px as usize) * 4;
                        // Source-over against what is already there, so
                        // overlapping glyph boxes do not punch each other out.
                        let sa = cov * fa;
                        let da = rgba[i + 3] as f32 / 255.0;
                        let out_a = sa + da * (1.0 - sa);
                        if out_a <= 0.0 {
                            continue;
                        }
                        for (c, src) in [fr, fg, fb].into_iter().enumerate() {
                            let dc = rgba[i + c] as f32 / 255.0;
                            let oc = (src as f32 / 255.0 * sa + dc * da * (1.0 - sa)) / out_a;
                            rgba[i + c] = (oc * 255.0).round().clamp(0.0, 255.0) as u8;
                        }
                        rgba[i + 3] = (out_a * 255.0).round().clamp(0.0, 255.0) as u8;
                    }
                }
            },
        );

        Some(Self {
            width,
            height,
            rgba,
            text_width,
        })
    }
}

/// What the shaper did, for tests that must assert on shaping rather than on ink.
///
/// AC-15 asks for RTL and multilingual text to be *correct*, and "some pixels
/// were lit" cannot tell correct from a left-to-right run of isolated forms.
/// These are the properties that can: whether bidi resolved the run
/// right-to-left, and which glyph ids the shaper actually selected — joined
/// Arabic forms have different ids from isolated ones, so comparing advanced
/// shaping against basic is a check that fails when shaping is off.
#[derive(Debug, Clone, PartialEq)]
pub struct ShapeReport {
    pub glyph_count: usize,
    /// True when any laid-out run resolved right-to-left.
    pub any_rtl: bool,
    /// Glyph ids in layout order.
    pub glyph_ids: Vec<u16>,
    /// Source byte offsets in layout order.
    pub starts: Vec<usize>,
    pub text_width: u32,
}

/// Shape without rasterizing, and report what the shaper decided.
///
/// `advanced` selects [`Shaping::Advanced`] (the production path) or
/// [`Shaping::Basic`]; the difference between the two reports is the evidence
/// that shaping is doing something.
pub fn shape_report(book: &mut FontBook, spec: &TextSpec, advanced: bool) -> Option<ShapeReport> {
    if book.is_empty() {
        return None;
    }
    let family = book.families.first()?.clone();
    let attrs = Attrs::new().family(Family::Name(&family));
    let metrics = Metrics::new(spec.size_px, spec.size_px * 1.25);
    let mut buffer = Buffer::new(&mut book.system, metrics);
    let layout_width = if spec.grow_to_text {
        None
    } else {
        Some(spec.width_px as f32)
    };
    buffer.set_size(layout_width, Some(spec.height_px as f32));
    buffer.set_text(
        &spec.text,
        &attrs,
        if advanced {
            Shaping::Advanced
        } else {
            Shaping::Basic
        },
        None,
    );
    buffer.shape_until_scroll(&mut book.system, false);

    let mut r = ShapeReport {
        glyph_count: 0,
        any_rtl: false,
        glyph_ids: Vec::new(),
        starts: Vec::new(),
        text_width: 0,
    };
    let mut w: f32 = 0.0;
    for run in buffer.layout_runs() {
        r.any_rtl |= run.rtl;
        w = w.max(run.line_w);
        for g in run.glyphs {
            r.glyph_count += 1;
            r.glyph_ids.push(g.glyph_id);
            r.starts.push(g.start);
        }
    }
    r.text_width = w.ceil().max(0.0) as u32;
    Some(r)
}

/// The ticker's scroll offset, in pixels, as a pure function of the master clock.
///
/// SPEC §6.5: "scrolls by texture offset, driven by the master clock: scroll
/// position is a pure function of `(masterFrame, speedPxPerFrame)`.
/// Deterministic, drift-free, and free at frame time." This is that function,
/// and it is the same discipline `overlay_alpha` follows for alpha — no
/// accumulator, no last-frame state, so a resync or a take cannot perturb it and
/// two engines given the same frame agree.
///
/// The period is the text's own width: past it the offset wraps, so an item
/// scrolls forever without the raster growing.
pub fn ticker_offset_px(master_frame: u64, speed_px_per_frame: f32, text_width: u32) -> f32 {
    if text_width == 0 || speed_px_per_frame == 0.0 {
        return 0.0;
    }
    let period = text_width as f32;
    let raw = master_frame as f32 * speed_px_per_frame;
    raw.rem_euclid(period)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_book_rasterizes_nothing_rather_than_finding_a_system_face() {
        let mut book = FontBook::from_packaged(vec![]);
        assert!(book.is_empty());
        let spec = TextSpec {
            text: "BREAKING".into(),
            size_px: 32.0,
            color: [1.0, 1.0, 1.0, 1.0],
            width_px: 400,
            height_px: 48,
            grow_to_text: false,
        };
        assert!(
            TextRaster::rasterize(&mut book, &spec).is_none(),
            "no packaged face must mean no text, never a host fallback"
        );
    }

    #[test]
    fn the_scroll_offset_is_a_pure_function_of_the_master_frame() {
        // Same frame, same offset, no matter the call order or history.
        assert_eq!(ticker_offset_px(0, 4.0, 100), 0.0);
        assert_eq!(ticker_offset_px(10, 4.0, 100), 40.0);
        assert_eq!(ticker_offset_px(25, 4.0, 100), 0.0, "wraps at the period");
        assert_eq!(ticker_offset_px(26, 4.0, 100), 4.0);
        // Evaluated out of order, the answers do not change.
        assert_eq!(ticker_offset_px(10, 4.0, 100), 40.0);
        // Degenerate inputs are still answers, not panics.
        assert_eq!(ticker_offset_px(99, 0.0, 100), 0.0);
        assert_eq!(ticker_offset_px(99, 4.0, 0), 0.0);
    }
}
