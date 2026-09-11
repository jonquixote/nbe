//! Prompt 07b — the graphics layer: text, templates, ticker and clock.
//!
//! The overlay level (07) built the host; this proves what composites onto it.
//! Every render test drives a real `RenderLoop` headless on wgpu and reads the
//! View back, the same pattern `prompt07_overlay.rs` established.
//!
//! Two rules are load-bearing and each has a test that fails without it:
//! shaping happens off the frame path (§6.5, §7.13), and the packaged font is
//! the only font (§0.1 assumption 11).

use nbe_engine::directive::DirectiveHandler;
use nbe_engine::scene::{ClockSpec, LayerSource, PackageIndex, TextContent};
use nbe_engine::state::{EngineState, OutgoingQueue};
use nbe_engine::text::{shape_report, FontBook, TextRaster, TextSpec};
use nbe_protocol::{DirectiveFrame, DirectiveKind, PROTOCOL_VERSION};
use std::path::Path;
use std::sync::Arc;

/// The packaged face every test here uses, read from the fixture that declares
/// it. Nothing loads a system font, and there is no code path that could.
const PACKAGED_FONT: &str = "../../tests/fixtures/overlay_show/media/Amiri-Regular.ttf";

fn font_bytes() -> Vec<u8> {
    std::fs::read(PACKAGED_FONT)
        .or_else(|_| std::fs::read("tests/fixtures/overlay_show/media/Amiri-Regular.ttf"))
        .expect("the fixture's packaged font must be readable")
}

fn spec(text: &str, grow: bool) -> TextSpec {
    TextSpec {
        text: text.into(),
        size_px: 48.0,
        color: [1.0, 1.0, 1.0, 1.0],
        width_px: 1280,
        height_px: 64,
        grow_to_text: grow,
    }
}

// ---------------------------------------------------------------------------
// AC-15 — RTL, Unicode, multilingual. Asserted on what the shaper DID, not on
// whether pixels happened to be lit: a left-to-right run of isolated Arabic
// forms also lights pixels, and it is wrong.
// ---------------------------------------------------------------------------

#[test]
fn ac15_latin_shapes_left_to_right_in_source_order() {
    let mut book = FontBook::from_packaged(vec![font_bytes()]);
    let r = shape_report(&mut book, &spec("BREAKING NEWS", true), true).unwrap();
    assert!(!r.any_rtl, "Latin must not resolve right-to-left");
    assert_eq!(r.glyph_count, 13, "one glyph per character for this string");
    let ascending: Vec<usize> = (0..13).collect();
    assert_eq!(
        r.starts, ascending,
        "LTR text lays out in source order; got {:?}",
        r.starts
    );
}

#[test]
fn ac15_arabic_resolves_rtl_and_uses_joined_forms() {
    let mut book = FontBook::from_packaged(vec![font_bytes()]);
    let advanced = shape_report(&mut book, &spec("عاجل", true), true).unwrap();
    let basic = shape_report(&mut book, &spec("عاجل", true), false).unwrap();

    assert!(advanced.any_rtl, "Arabic must resolve right-to-left");
    // The discriminating half. Advanced shaping selects the *joined* form of
    // each letter; basic shaping selects the isolated one. Same string, same
    // font, different glyph ids — and if shaping were off, these would match.
    assert_ne!(
        advanced.glyph_ids, basic.glyph_ids,
        "advanced shaping must select different glyphs from basic; \
         identical ids mean the shaper ran in isolated-form mode"
    );
}

#[test]
fn ac15_a_mixed_paragraph_reorders_the_rtl_run_inside_it() {
    // The strongest evidence available without eyeballing pixels: in "NEWS عاجل"
    // the Latin run lays out in ascending source order and the Arabic run that
    // follows lays out in DESCENDING source order, because bidi reversed it
    // inside an otherwise left-to-right paragraph.
    let mut book = FontBook::from_packaged(vec![font_bytes()]);
    let r = shape_report(&mut book, &spec("NEWS عاجل", true), true).unwrap();

    let latin: Vec<usize> = r.starts.iter().copied().take(5).collect();
    let arabic: Vec<usize> = r.starts.iter().copied().skip(5).collect();

    assert!(
        latin.windows(2).all(|w| w[0] < w[1]),
        "the Latin run must ascend; got {latin:?}"
    );
    assert!(
        arabic.len() >= 2 && arabic.windows(2).all(|w| w[0] > w[1]),
        "the Arabic run must descend — that descent IS the bidi reordering; got {arabic:?}"
    );
}

#[test]
fn ac15_accented_latin_keeps_its_diacritics() {
    let mut book = FontBook::from_packaged(vec![font_bytes()]);
    let plain = TextRaster::rasterize(&mut book, &spec("Eleccion Inigo", true)).unwrap();
    let accented = TextRaster::rasterize(&mut book, &spec("Elección Íñigo", true)).unwrap();
    let ink = |r: &TextRaster| r.rgba.chunks(4).filter(|p| p[3] > 0).count();
    assert!(
        ink(&accented) > ink(&plain),
        "accents are additional ink: accented {} vs plain {}",
        ink(&accented),
        ink(&plain)
    );
}

// ---------------------------------------------------------------------------
// The packaged-font rule (§0.1 assumption 11, portability.md row 7).
// ---------------------------------------------------------------------------

#[test]
fn without_a_packaged_face_nothing_is_drawn_and_no_host_font_is_found() {
    // This machine has hundreds of system fonts installed. A book built from no
    // packaged bytes must still find none of them.
    let mut empty = FontBook::from_packaged(vec![]);
    assert!(empty.is_empty());
    assert!(
        TextRaster::rasterize(&mut empty, &spec("BREAKING", false)).is_none(),
        "an empty book must draw nothing rather than fall back to a host face"
    );
    let mut packaged = FontBook::from_packaged(vec![font_bytes()]);
    assert_eq!(
        packaged.families(),
        &["Amiri".to_string()],
        "the only family available is the one the package declared"
    );
    assert!(TextRaster::rasterize(&mut packaged, &spec("BREAKING", false)).is_some());
}

// ---------------------------------------------------------------------------
// Texture discipline (§6.5: shape once per content change; §7.13: never on the
// frame path). Asserted on layout CALL COUNTS, not on timings.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_second_of_scrolling_triggers_no_relayout_while_a_content_change_triggers_one() {
    let dir = tempfile::tempdir().unwrap();
    write_graphics_package(dir.path());
    let (state, handler, mut render) = render_engine(dir.path()).await;
    show_overlay(&handler, "ol_ticker").await;
    *state.view_item.lock().unwrap() = Some("A1".into());

    // First frame: the content is new, so exactly one rasterization.
    render.render_frame(1, None);
    let after_first = render
        .text_rasterizations
        .load(std::sync::atomic::Ordering::SeqCst);
    assert_eq!(after_first, 1, "the first frame shapes the ticker once");

    // A second of scrolling at the house rate. The offset moves every frame and
    // the content does not, so this must add nothing.
    for f in 2..=31 {
        render.render_frame(f, None);
    }
    assert_eq!(
        render
            .text_rasterizations
            .load(std::sync::atomic::Ordering::SeqCst),
        after_first,
        "30 frames of scrolling must not re-shape: scroll is a sampling offset, \
         not a relayout"
    );
}

#[tokio::test]
async fn the_clock_reshapes_once_a_second_not_once_a_frame() {
    let dir = tempfile::tempdir().unwrap();
    write_graphics_package(dir.path());
    let (state, handler, mut render) = render_engine(dir.path()).await;
    show_overlay(&handler, "ol_clock").await;
    *state.view_item.lock().unwrap() = Some("A1".into());

    // A clock's content is a function of the master clock, so it is the hardest
    // case for "no per-frame relayout" — and it still must not shape per frame.
    // Two seconds at 30 fps with blinkColon on changes the string twice a
    // second (the colon), so the bound is one shape per half-second, not 60.
    for f in 0..60 {
        render.render_frame(f, None);
    }
    let n = render
        .text_rasterizations
        .load(std::sync::atomic::Ordering::SeqCst);
    assert!(
        n <= 5,
        "two seconds of clock must shape a handful of times, not once a frame; got {n}"
    );
    assert!(n >= 2, "the clock face must actually change; got {n}");
}

// ---------------------------------------------------------------------------
// The clock reads the MASTER clock, never wall time (§16.13, §11).
// ---------------------------------------------------------------------------

#[test]
fn show_elapsed_is_derived_from_the_master_frame() {
    // Rendered through the same helper the engine uses, so this is the real
    // derivation rather than a restatement of it.
    let at = |frame: u64, blink: bool| {
        nbe_engine::render::clock_face_for_test(
            &ClockSpec {
                show_elapsed: true,
                format: "HH:mm:ss".into(),
                blink_colon: blink,
            },
            frame,
            30,
        )
    };
    assert_eq!(at(0, false), "00:00:00");
    assert_eq!(
        at(30, false),
        "00:00:01",
        "one second is one house-rate of frames"
    );
    assert_eq!(at(30 * 59, false), "00:00:59");
    assert_eq!(at(30 * 60, false), "00:01:00");
    assert_eq!(at(30 * 3600, false), "01:00:00");
    // Frame 29 is still second zero: the clock counts completed seconds, so it
    // never shows a second the show has not finished.
    assert_eq!(at(29, false), "00:00:00");
}

#[test]
fn blink_colon_blinks_on_the_beat() {
    let at = |frame: u64| {
        nbe_engine::render::clock_face_for_test(
            &ClockSpec {
                show_elapsed: true,
                format: "HH:mm:ss".into(),
                blink_colon: true,
            },
            frame,
            30,
        )
    };
    // Lit for the first half of each second, dark for the second half — a
    // function of the frame, so it cannot drift.
    assert!(at(0).contains(':'), "lit at the top of the second");
    assert!(at(14).contains(':'), "still lit just before the half");
    assert!(!at(15).contains(':'), "dark at the half");
    assert!(!at(29).contains(':'), "still dark at the end of the second");
    assert!(at(30).contains(':'), "lit again on the next second");
}

// ---------------------------------------------------------------------------
// AC-24's remaining half: a take must not perturb the ticker's scroll.
// ---------------------------------------------------------------------------

#[test]
fn the_scroll_offset_is_unperturbed_by_a_take() {
    // The offset is a pure function of (masterFrame, speed, period). A take
    // changes the scene under the band and touches none of those, so the only
    // way it could perturb the scroll is if the implementation kept an
    // accumulator — which is exactly what this forbids.
    let period = 900u32;
    let speed = 4.0;
    let before: Vec<f32> = (100..110)
        .map(|f| nbe_engine::text::ticker_offset_px(f, speed, period))
        .collect();
    // "Take" here is anything that is not the master frame: re-evaluating in a
    // different order, after other work, with other elements on air.
    let after: Vec<f32> = (100..110)
        .rev()
        .map(|f| nbe_engine::text::ticker_offset_px(f, speed, period))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    assert_eq!(
        before, after,
        "the offset depends on the frame and nothing else"
    );
    assert_eq!(
        before[0], 400.0,
        "frame 100 x 4 px = 400 px into the period"
    );
}

#[tokio::test]
async fn the_ticker_survives_a_cut_and_a_mix_untouched() {
    let dir = tempfile::tempdir().unwrap();
    write_graphics_package(dir.path());
    let (state, handler, mut render) = render_engine(dir.path()).await;
    show_overlay(&handler, "ol_ticker").await;
    *state.view_item.lock().unwrap() = Some("A1".into());

    render.render_frame(10, None);
    let shapes_before = render
        .text_rasterizations
        .load(std::sync::atomic::Ordering::SeqCst);

    // A cut, then a mix, under the band.
    *state.view_item.lock().unwrap() = Some("A2".into());
    render.render_frame(11, None);
    *state.transition.lock().unwrap() = Some(nbe_engine::scene::Transition {
        from_item: Some("A2".into()),
        from_start_frame: 0,
        to_item: "A1".into(),
        kind: nbe_engine::scene::TransitionKind::Mix,
        duration_frames: 15,
        start_frame: 12,
    });
    for f in 12..=27 {
        render.render_frame(f, None);
    }

    assert_eq!(
        render
            .text_rasterizations
            .load(std::sync::atomic::Ordering::SeqCst),
        shapes_before,
        "neither a cut nor a mix may re-shape the ticker — its content did not change"
    );
}

// ---------------------------------------------------------------------------
// The composition seam: text resolves through the same walk as everything else.
// ---------------------------------------------------------------------------

#[test]
fn a_ticker_element_resolves_to_a_text_layer_through_the_scene_walk() {
    let dir = tempfile::tempdir().unwrap();
    write_graphics_package(dir.path());
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join("manifest.json")).unwrap())
            .unwrap();
    let idx = PackageIndex::build(&manifest, dir.path());

    let layers = idx.resolve_overlay("ol_ticker").layers;
    assert_eq!(layers.len(), 1);
    match &layers[0].source {
        LayerSource::Text(t) => {
            assert!(t.scroll, "a ticker scrolls");
            assert_eq!(
                t.font_asset_ids,
                vec!["amiri".to_string()],
                "the font comes from the element's template, not from a default"
            );
            match &t.content {
                TextContent::Static(s) => assert!(s.contains("BREAKING")),
                other => panic!("a ticker carries static text, got {other:?}"),
            }
        }
        other => panic!("a ticker must resolve to a text layer, got {other:?}"),
    }

    // And a clock resolves to clock content, not to a string frozen at index
    // time — the string is the render loop's to derive per frame.
    match &idx.resolve_overlay("ol_clock").layers[0].source {
        LayerSource::Text(t) => match &t.content {
            TextContent::Clock(c) => {
                assert!(c.show_elapsed, "the fixture asks for showElapsed");
                assert!(c.blink_colon);
            }
            other => panic!("a clock carries clock content, got {other:?}"),
        },
        other => panic!("a clock must resolve to a text layer, got {other:?}"),
    }
}

#[test]
fn a_template_naming_no_font_draws_no_text() {
    // The manifest can declare a template with no fontAssetIds. Preflight warns
    // about a font that does not resolve; a template that names none is legal,
    // and the engine's answer is an empty font list — never a host face.
    let dir = tempfile::tempdir().unwrap();
    write_graphics_package(dir.path());
    let raw = std::fs::read_to_string(dir.path().join("manifest.json")).unwrap();
    let mut manifest: serde_json::Value = serde_json::from_str(&raw).unwrap();
    manifest["templates"][0]
        .as_object_mut()
        .unwrap()
        .remove("fontAssetIds");
    let idx = PackageIndex::build(&manifest, dir.path());
    match &idx.resolve_overlay("ol_ticker").layers[0].source {
        LayerSource::Text(t) => assert!(
            t.font_asset_ids.is_empty(),
            "a template naming no face must offer none"
        ),
        other => panic!("expected text, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn directive(
    command: &str,
    sv: u64,
    target: serde_json::Value,
    payload: serde_json::Value,
) -> DirectiveFrame {
    DirectiveFrame {
        v: PROTOCOL_VERSION.into(),
        kind: DirectiveKind::Directive,
        seq: sv,
        state_version: sv,
        command: command.into(),
        target,
        payload,
    }
}

async fn show_overlay(handler: &DirectiveHandler, id: &str) {
    handler
        .apply(&directive(
            "overlay.show",
            2,
            serde_json::json!({ "overlayId": id }),
            serde_json::json!({}),
        ))
        .await
        .unwrap();
}

async fn render_engine(
    dir: &Path,
) -> (
    Arc<EngineState>,
    DirectiveHandler,
    nbe_engine::render::RenderLoop,
) {
    let state = Arc::new(EngineState::new(30));
    let handler = DirectiveHandler::new(state.clone(), Arc::new(OutgoingQueue::default()));
    handler
        .apply(&directive(
            "show.load",
            1,
            serde_json::json!({}),
            serde_json::json!({ "packagePath": dir.to_string_lossy() }),
        ))
        .await
        .unwrap();
    let render = nbe_engine::render::RenderLoop::new(state.clone())
        .await
        .unwrap();
    (state, handler, render)
}

/// A package with a real packaged font, a ticker, a banner and a clock.
fn write_graphics_package(dir: &Path) {
    std::fs::create_dir_all(dir.join("media")).unwrap();
    std::fs::write(dir.join("media/Amiri-Regular.ttf"), font_bytes()).unwrap();
    let mut png = Vec::new();
    image::RgbaImage::from_pixel(4, 4, image::Rgba([10, 10, 10, 255]))
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    std::fs::write(dir.join("media/slate.png"), &png).unwrap();

    let manifest = serde_json::json!({
        "manifestVersion": "0.4",
        "show": { "id": "s", "title": "graphics", "fallbackAssetId": "slate", "video": { "frameRate": 30, "resolution": "1920x1080" } },
        "assets": [
            { "id": "amiri", "kind": "font", "source": "media/Amiri-Regular.ttf" },
            { "id": "slate", "kind": "image", "source": "media/slate.png", "format": "png" }
        ],
        "templates": [
            { "id": "tpl_ticker", "kind": "ticker", "fontAssetIds": ["amiri"],
              "fields": [{ "name": "text" }] },
            { "id": "tpl_banner", "kind": "breakingBanner", "fontAssetIds": ["amiri"],
              "fields": [{ "name": "headline" }] }
        ],
        "scenes": [
            { "id": "SCN_A", "elements": [{ "id": "bg_a", "kind": "graphic", "z": 0,
                "fields": { "color": "#ff0000" }, "transform": { "x": 0.0, "y": 0.0, "w": 1.0, "h": 1.0 } }] },
            { "id": "SCN_B", "elements": [{ "id": "bg_b", "kind": "graphic", "z": 0,
                "fields": { "color": "#0000ff" }, "transform": { "x": 0.0, "y": 0.0, "w": 1.0, "h": 1.0 } }] }
        ],
        "overlays": [
            { "id": "ol_ticker", "elements": [{ "id": "tick", "kind": "ticker", "z": 1,
                "templateId": "tpl_ticker",
                "fields": { "text": "BREAKING — Elección Íñigo — عاجل", "speedPxPerFrame": 4 },
                "transform": { "x": 0.0, "y": 0.9, "w": 1.0, "h": 0.1 } }] },
            { "id": "ol_banner", "elements": [{ "id": "band", "kind": "graphic", "z": 1,
                "templateId": "tpl_banner", "fields": { "headline": "BREAKING NEWS" },
                "transform": { "x": 0.0, "y": 0.0, "w": 1.0, "h": 0.08 } }] },
            { "id": "ol_clock", "elements": [{ "id": "clk", "kind": "clock", "z": 1,
                "templateId": "tpl_banner",
                "clock": { "mode": "showElapsed", "format": "HH:mm:ss", "blinkColon": true },
                "transform": { "x": 0.0, "y": 0.0, "w": 0.2, "h": 0.08 } }] }
        ],
        "rundown": { "items": [
            { "id": "A1", "kind": "sceneRef", "sceneRef": "SCN_A" },
            { "id": "A2", "kind": "sceneRef", "sceneRef": "SCN_B" }
        ] }
    });
    std::fs::write(
        dir.join("manifest.json"),
        serde_json::to_string_pretty(&manifest).unwrap(),
    )
    .unwrap();
}
