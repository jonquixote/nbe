//! Runtime graphics: what `graphic.*`, `breaking.*`, `ticker.*` and
//! `clock.configure` put on air (SPEC §16.5, §16.7, §16.13).
//!
//! Until this module the engine logged each of these as "directive ignored
//! (no engine effect)": the control plane accepted them, bumped its state and
//! forwarded them, and the audience saw nothing change.
//!
//! **What is drawn and where.** A §6.5 template declares a class, faces and
//! field names, but no geometry. A runtime graphic is therefore drawn in the
//! box of a *declared* element:
//!
//! 1. the element the command names (`elementId`), if the package declares
//!    one by that id;
//! 2. otherwise the first declared element that uses the template (overlays
//!    first, then scenes, in id order);
//! 3. otherwise a default band for the template's class ([`default_rect`]).
//!
//! The graphic's text is the template's fields (`headline` + `subhead`,
//! `name` + `role`, `text`), merged across `graphic.update`s; its colour is
//! `fields.color`, else the host element's, else white.
//!
//! **Where it composites.** At the overlay level (§7.10), after the declared
//! overlays, low z to high z, on the View only. A fallback slate covers it as
//! it covers every overlay.
//!
//! **Shaping.** Every string here is shaped and uploaded off the render thread
//! before the command is acknowledged ([`crate::text_prep`]); the layers this
//! module hands the render loop are `prepared_only`.

use crate::scene::{ClockSpec, ElementSpec, Layer, LayerSource, PackageIndex, TextContent};
use std::collections::BTreeMap;

/// The id the breaking banner is kept under. `breaking.show` carries no
/// element id (§16.5), and there is one banner.
pub const BREAKING_ID: &str = "breaking";

/// Between ticker override items: wide enough to read as a break at 120 px/s.
pub const TICKER_ITEM_SEPARATOR: &str = "   \u{2022}   ";

/// One graphic a command put on air.
#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeGraphic {
    pub element_id: String,
    pub template_id: String,
    /// The fields as last shown or updated, merged.
    pub fields: serde_json::Map<String, serde_json::Value>,
    pub z: i64,
    pub rect: [f32; 4],
    pub color: [f32; 4],
    pub font_asset_ids: Vec<String>,
}

impl RuntimeGraphic {
    /// The text this graphic draws, from the template fields (`scene::text_of`'s
    /// rule, so a declared element and a runtime one read fields the same way).
    pub fn text(&self) -> Option<String> {
        crate::scene::text_of(Some(&serde_json::Value::Object(self.fields.clone())))
    }

    /// The colour: `fields.color` when the command set one, else what the
    /// graphic was created with.
    fn effective_color(&self) -> [f32; 4] {
        self.fields
            .get("color")
            .and_then(|c| c.as_str())
            .and_then(crate::scene::parse_hex_rgb)
            .unwrap_or(self.color)
    }

    /// The layer this graphic draws: text when it has any, the solid fill a
    /// colour-only graphic always was otherwise.
    pub fn layer(&self) -> Option<Layer> {
        let source = match self.text() {
            Some(text) => LayerSource::Text(crate::scene::TextLayer {
                element_id: self.element_id.clone(),
                content: TextContent::Static(text),
                color: self.effective_color(),
                font_asset_ids: self.font_asset_ids.clone(),
                scroll: false,
                speed_px_per_frame: 0.0,
                prepared_only: true,
            }),
            None if self.fields.contains_key("color") => LayerSource::Solid(self.effective_color()),
            None => return None,
        };
        Some(Layer {
            source,
            rect: self.rect,
            opacity: 1.0,
            z: self.z,
        })
    }
}

/// The engine's runtime graphics state. One mutex in [`crate::state::EngineState`];
/// the render loop reads it once per frame, a command writes it once.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GraphicsRuntime {
    /// element id → on-air graphic, the breaking banner under [`BREAKING_ID`].
    pub graphics: BTreeMap<String, RuntimeGraphic>,
    /// `ticker.override`'s items, already ordered by the control plane
    /// (priority, then insertion: §16.7), as their texts.
    pub ticker_override: Vec<String>,
    /// `ticker.setSource`: `manual` (the default), `rss` or `mixed`.
    pub ticker_source: String,
    /// element id → the clock configuration `clock.configure` set, merged over
    /// the declared one.
    pub clock_overrides: BTreeMap<String, ClockSpec>,
}

impl GraphicsRuntime {
    pub fn new() -> Self {
        Self {
            ticker_source: "manual".into(),
            ..Default::default()
        }
    }

    /// What a declared ticker shows, given its own declared text.
    ///
    /// - `manual`: the override items when there are any, else the declared text;
    /// - `mixed`: the override items, then the declared text;
    /// - `rss`: the declared text. RSS fetching is not built (the control
    ///   plane's `ticker.refreshRss` fetches nothing), so there is no feed text
    ///   to show and the override is set aside, as the source says.
    ///
    /// Returns the text and whether it came from a command (and so was shaped
    /// off the render thread).
    pub fn ticker_text(&self, declared: &str) -> (String, bool) {
        let items = self.ticker_override.join(TICKER_ITEM_SEPARATOR);
        match self.ticker_source.as_str() {
            "rss" => (declared.to_string(), false),
            "mixed" if !items.is_empty() => {
                (format!("{items}{TICKER_ITEM_SEPARATOR}{declared}"), true)
            }
            _ if !items.is_empty() => (items, true),
            _ => (declared.to_string(), false),
        }
    }

    /// The runtime graphics' layers, low z to high z (ties by element id).
    pub fn layers(&self) -> Vec<Layer> {
        let mut out: Vec<Layer> = self
            .graphics
            .values()
            .filter_map(RuntimeGraphic::layer)
            .collect();
        out.sort_by_key(|l| l.z);
        out
    }

    /// Element ids on air, for the telemetry line.
    pub fn on_air_ids(&self) -> Vec<String> {
        self.graphics
            .keys()
            .filter(|k| k.as_str() != BREAKING_ID)
            .cloned()
            .collect()
    }
}

/// The default band for a template class with no declared host element.
pub fn default_rect(template_kind: &str) -> [f32; 4] {
    match template_kind {
        "breakingBanner" => [0.0, 0.0, 1.0, 0.08],
        "ticker" => [0.0, 0.9, 1.0, 0.1],
        "lowerThirdName" => [0.05, 0.72, 0.5, 0.1],
        // lowerThirdHeadline and generic
        _ => [0.05, 0.75, 0.9, 0.12],
    }
}

/// Build the graphic a `graphic.show` (or `breaking.show`) puts on air.
///
/// `element_id` is the command's, already defaulted to `<templateId>-graphic`
/// as the control plane defaults it (`commands/element.ts`), so the two sides
/// key the same graphic by the same id.
pub fn build_graphic(
    index: &PackageIndex,
    template_id: &str,
    element_id: &str,
    fields: serde_json::Map<String, serde_json::Value>,
    z: Option<i64>,
) -> RuntimeGraphic {
    let host: Option<&ElementSpec> = index
        .declared_element(element_id)
        .or_else(|| index.template_host(template_id));
    let kind = index
        .template_kind
        .get(template_id)
        .map(String::as_str)
        .unwrap_or("generic");
    let font_asset_ids = index
        .template_fonts
        .get(template_id)
        .cloned()
        .unwrap_or_default();
    RuntimeGraphic {
        element_id: element_id.to_string(),
        template_id: template_id.to_string(),
        fields,
        // Above the declared overlay elements unless the command says.
        z: z.or(host.map(|h| h.z)).unwrap_or(100),
        rect: host.map(|h| h.rect).unwrap_or_else(|| default_rect(kind)),
        color: host.and_then(|h| h.color).unwrap_or([1.0, 1.0, 1.0, 1.0]),
        font_asset_ids,
    }
}

/// Merge a `clock.configure` payload over a clock's current configuration.
/// `timezone` and `locale` are accepted and not applied: the clock face
/// renders host-local wall time (or the master clock for `showElapsed`), as it
/// did before this command reached the engine.
pub fn merge_clock(base: &ClockSpec, patch: &serde_json::Value) -> ClockSpec {
    let mut out = base.clone();
    if let Some(mode) = patch.get("mode").and_then(|v| v.as_str()) {
        out.show_elapsed = mode == "showElapsed";
    }
    if let Some(format) = patch.get("format").and_then(|v| v.as_str()) {
        out.format = format.to_string();
    }
    if let Some(b) = patch.get("blinkColon").and_then(|v| v.as_bool()) {
        out.blink_colon = b;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_override_replaces_the_declared_text_and_clears_back() {
        let mut g = GraphicsRuntime::new();
        assert_eq!(g.ticker_text("DECLARED"), ("DECLARED".into(), false));
        g.ticker_override = vec!["ONE".into(), "TWO".into()];
        assert_eq!(
            g.ticker_text("DECLARED"),
            (format!("ONE{TICKER_ITEM_SEPARATOR}TWO"), true)
        );
        g.ticker_source = "mixed".into();
        assert_eq!(
            g.ticker_text("DECLARED").0,
            format!("ONE{TICKER_ITEM_SEPARATOR}TWO{TICKER_ITEM_SEPARATOR}DECLARED")
        );
        g.ticker_source = "rss".into();
        assert_eq!(g.ticker_text("DECLARED"), ("DECLARED".into(), false));
    }

    #[test]
    fn clock_patch_merges_over_the_declared_spec() {
        let base = ClockSpec {
            show_elapsed: true,
            format: "HH:mm:ss".into(),
            blink_colon: true,
        };
        let out = merge_clock(&base, &serde_json::json!({ "format": "HH:mm" }));
        assert_eq!(out.format, "HH:mm");
        assert!(out.show_elapsed && out.blink_colon);
    }
}
