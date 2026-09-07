//! Preflight report model, matching SPEC Section 19.2 exactly.
//! Preflight produces `preflight_report.json`; CI blocks on exit code.

use serde::{Deserialize, Serialize};

/// Top-level preflight report (SPEC 19.2).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreflightReport {
    pub manifest_valid: bool,
    pub air_ready: bool,
    #[serde(default)]
    pub errors: Vec<String>,
    #[serde(default)]
    pub warnings: Vec<String>,
    #[serde(default)]
    pub assets: Vec<AssetReport>,
    #[serde(default)]
    pub loops: Vec<LoopReport>,
    #[serde(default)]
    pub scenes: Vec<SceneReport>,
    #[serde(default)]
    pub plugins: Vec<PluginReport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact_sheet: Option<String>,
    /// SPEC §19.2.1 (v0.4). Always populated — a number an operator can read
    /// is the deliverable, not only a threshold that trips.
    #[serde(default)]
    pub resources: ResourceReport,
}

/// SPEC §12.11: what a package will demand if it goes to air.
///
/// Reported unconditionally; the refusal lives where both facts are known
/// (§7.15, §12.11.3). Preflight validates a package in isolation and cannot
/// know the target machine, so it reports and warns.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceReport {
    /// Worst-case resident texture demand (§12.11.1).
    pub vram_demand_mib: u64,
    /// RAM-resident audio demand (§8.4 residency, §12.11.1).
    pub audio_demand_mib: u64,
    /// The package's declared house rate, so a caller that knows the engine's
    /// rate can compare it (§7.15).
    ///
    /// Serialized even when `None`. §19.2.1 calls this block REQUIRED and
    /// always-populated, and an absent field is a different failure from a
    /// null one: a consumer can tell "no rate declared" from "field missing"
    /// only if the field is always there.
    #[serde(default)]
    pub declared_house_rate: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetReport {
    pub id: String,
    #[serde(default)]
    pub kind: Option<String>,
    pub exists: bool,
    #[serde(default)]
    pub sha256_ok: Option<bool>,
    #[serde(default)]
    pub decode_first_frame_ok: Option<bool>,
    #[serde(default)]
    pub decode_last_frame_ok: Option<bool>,
    #[serde(default)]
    pub cfr: Option<bool>,
    #[serde(default)]
    pub frame_rate: Option<f64>,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
    #[serde(default)]
    pub duration_frames: Option<u32>,
    #[serde(default)]
    pub cadence_ok: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loudness: Option<LoudnessSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoudnessSummary {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub integrated_lufs: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub true_peak_dbtp: Option<f64>,
}

/// Loop report (extended shape per SPEC 12.10 / 19.2).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoopReport {
    pub asset_id: String,
    #[serde(default)]
    pub period_frames: Option<u32>,
    #[serde(default)]
    pub seamless: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_texture_format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame_cost_mib: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_budget_mib: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_frames_by_budget: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_policy_selected: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vram_resident: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SceneReport {
    pub scene_id: String,
    #[serde(default)]
    pub references_ok: bool,
    #[serde(default)]
    pub dag_ok: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginReport {
    pub plugin_id: String,
    #[serde(default)]
    pub sandbox_ok: bool,
}

impl PreflightReport {
    /// Recompute `air_ready` from accumulated errors/warnings.
    /// `manifest_valid` tracks schema validity only and is set by the caller.
    pub fn finalize(&mut self) {
        self.air_ready = self.errors.is_empty() && self.warnings.is_empty();
    }

    pub fn push_error(&mut self, msg: impl Into<String>) {
        self.errors.push(msg.into());
    }

    pub fn push_warning(&mut self, msg: impl Into<String>) {
        self.warnings.push(msg.into());
    }
}
