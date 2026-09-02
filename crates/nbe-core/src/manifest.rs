//! Manifest model mirroring `schemas/manifest.v0.3.json`.
//! The schema file is normative; these types are the typed view of it.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Manifest {
    pub manifest_version: String,
    pub network: Network,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<Channel>,
    pub show: Show,
    pub assets: Vec<Asset>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub templates: Vec<GraphicTemplate>,
    pub rundown: Sequence,
    pub control: Control,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub features: Option<Features>,
    pub scenes: Vec<Scene>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overlays: Vec<Overlay>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transitions: Vec<TransitionPreset>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub automation: Vec<AutomationRule>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plugins: Vec<Plugin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality_profile: Option<QualityProfile>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum QualityProfile {
    Potato,
    Consumer,
    Pro,
    Reference,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Network {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logo_asset_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_audio_asset_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Channel {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub future_scheduler: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Show {
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episode_code: Option<String>,
    pub video: VideoSpec,
    pub audio: AudioSpec,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transitions: Option<TransitionDefaults>,
    pub fallback_asset_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outputs: Option<OutputDefaults>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VideoSpec {
    pub width: u32,
    pub height: u32,
    pub frame_rate: FrameRate,
    pub color_space: ColorSpace,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aspect_ratio: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "u32", into = "u32")]
pub enum FrameRate {
    Fps30,
    Fps60,
}

impl From<u32> for FrameRate {
    fn from(v: u32) -> Self {
        match v {
            60 => Self::Fps60,
            _ => Self::Fps30,
        }
    }
}

impl From<FrameRate> for u32 {
    fn from(f: FrameRate) -> u32 {
        match f {
            FrameRate::Fps30 => 30,
            FrameRate::Fps60 => 60,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ColorSpace {
    #[serde(rename = "rec709")]
    Rec709,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AudioSpec {
    pub sample_rate: u32,
    #[serde(default = "default_loudness")]
    pub loudness_target_lufs: f64,
    #[serde(default = "default_true_peak")]
    pub true_peak_dbtp: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_language: Option<String>,
}

fn default_loudness() -> f64 {
    -16.0
}

fn default_true_peak() -> f64 {
    -1.5
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum DefaultTake {
    #[default]
    Cut,
    Mix,
}

fn default_mix_duration() -> u32 {
    15
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TransitionDefaults {
    #[serde(default)]
    pub default_take: DefaultTake,
    #[serde(default = "default_mix_duration")]
    pub mix_duration_frames: u32,
}

impl Default for TransitionDefaults {
    fn default() -> Self {
        Self {
            default_take: DefaultTake::default(),
            mix_duration_frames: default_mix_duration(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutputDefaults {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record: Option<RecordOutput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream: Option<StreamOutput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<PreviewOutput>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecordOutput {
    #[serde(default)]
    pub container: RecordContainer,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directory: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isolation: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum RecordContainer {
    #[default]
    FragmentedMp4,
    Matroska,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StreamOutput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<StreamProtocol>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video_bitrate_kbps: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_bitrate_kbps: Option<u32>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum StreamProtocol {
    Rtmp,
    Srt,
    Whip,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreviewOutput {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub protocol: PreviewProtocol,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum PreviewProtocol {
    #[default]
    Whep,
    Mjpeg,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Asset {
    pub id: String,
    pub kind: AssetKind,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<AssetFormat>,
    #[serde(default)]
    pub cadence: Cadence,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_frame_rate: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_duration_frames: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pulldown: Option<Pulldown>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loop_: Option<LoopMetadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loudness: Option<LoudnessReport>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AssetKind {
    Video,
    AlphaVideo,
    Audio,
    Image,
    Font,
    Rss,
    Wasm,
    Wgsl,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AssetFormat {
    H264,
    Prores4444,
    HapAlpha,
    PngSequence,
    Aac,
    Pcm,
    Wav,
    Png,
    Svg,
    Ttf,
    Otf,
    Rss,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum Cadence {
    #[default]
    Preserve,
    Interpolate,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "mode", deny_unknown_fields)]
pub enum Pulldown {
    #[serde(rename = "pattern")]
    Pattern { pattern: Vec<u32> },
    #[serde(rename = "repeatNthSourceFrame")]
    RepeatNthSourceFrame { n: u32 },
    #[serde(rename = "repeatOnePerNSourceFrames")]
    RepeatOnePerNSourceFrames { n: u32 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LoopMetadata {
    pub period_frames: u32,
    #[serde(default)]
    pub t0_frames: u32,
    #[serde(default = "default_true")]
    pub seamless: bool,
    #[serde(default)]
    pub cache_policy: CachePolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vram_budget_mib: Option<u32>,
    #[serde(default)]
    pub texture_format: TextureFormat,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum CachePolicy {
    #[default]
    Auto,
    Vram,
    Stream,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum TextureFormat {
    #[default]
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "nv12")]
    Nv12,
    #[serde(rename = "rgba8")]
    Rgba8,
    #[serde(rename = "nv12Alpha")]
    Nv12Alpha,
    #[serde(rename = "bc7")]
    Bc7,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LoudnessReport {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub integrated_lufs: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub true_peak_dbtp: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loudness_range: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub measured_by: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GraphicTemplate {
    pub id: String,
    pub kind: TemplateKind,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub font_asset_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<TemplateField>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TemplateKind {
    LowerThirdHeadline,
    LowerThirdName,
    BreakingBanner,
    Ticker,
    Generic,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TemplateField {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default)]
    pub multiline: bool,
    #[serde(default)]
    pub direction: TextDirection,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_length: Option<u32>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum TextDirection {
    #[default]
    Auto,
    Ltr,
    Rtl,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Transform {
    #[serde(default)]
    pub x: f64,
    #[serde(default)]
    pub y: f64,
    #[serde(default = "default_one")]
    pub w: f64,
    #[serde(default = "default_one")]
    pub h: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crop: Option<Crop>,
}

fn default_one() -> f64 {
    1.0
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Crop {
    #[serde(default)]
    pub u: f64,
    #[serde(default)]
    pub v: f64,
    #[serde(default)]
    pub w: f64,
    #[serde(default)]
    pub h: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChromaKey {
    pub enabled: bool,
    #[serde(default)]
    pub color: ChromaColor,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_color_hex: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tolerance: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub softness: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spill_suppression: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edge_feather: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub garbage_matte: Option<Transform>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ChromaColor {
    #[default]
    Green,
    Blue,
    Custom,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LayerAudio {
    #[serde(default)]
    pub bus: AudioBus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gain_db: Option<f64>,
    #[serde(default)]
    pub muted: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum AudioBus {
    #[default]
    Clip,
    Guest,
    Sfx,
    Music,
    Mic,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClockConfig {
    #[serde(default)]
    pub mode: ClockMode,
    #[serde(default = "default_timezone")]
    pub timezone: String,
    #[serde(default)]
    pub format: ClockFormat,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locale: Option<String>,
    #[serde(default)]
    pub blink_colon: bool,
}

fn default_timezone() -> String {
    "local".to_string()
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum ClockMode {
    #[default]
    Wall,
    ShowElapsed,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum ClockFormat {
    #[serde(rename = "HH:mm")]
    HhMm,
    #[default]
    #[serde(rename = "HH:mm:ss")]
    HhMmSs,
    #[serde(rename = "hh:mm A")]
    HhMmA,
    #[serde(rename = "locale")]
    Locale,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Control {
    pub bindings: Vec<ControlBinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub companion: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ControlBinding {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<BindingTrigger>,
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BindingTrigger {
    pub kind: TriggerKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bank: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TriggerKind {
    CompanionKey,
    Hotkey,
    Midi,
    WebButton,
    Osc,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Features {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ndi: Option<NdiFeature>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NdiFeature {
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Element {
    pub id: String,
    pub kind: ElementKind,
    pub z: i32,
    #[serde(default = "default_true")]
    pub visible: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feed_asset_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guest_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields: Option<HashMap<String, serde_json::Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loop_: Option<LoopMetadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transform: Option<Transform>,
    #[serde(default = "default_one")]
    pub opacity: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chroma_key: Option<ChromaKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<LayerAudio>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clock: Option<ClockConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scene_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enter_animation: Option<Animation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_animation: Option<Animation>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ElementKind {
    VideoLoop,
    Clip,
    Camera,
    Guest,
    Graphic,
    Ticker,
    Clock,
    SceneRef,
    Group,
    Plugin,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Animation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_frames: Option<u32>,
    #[serde(default)]
    pub delay_frames: u32,
    #[serde(default)]
    pub easing: Easing,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bezier: Option<[f64; 4]>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum Easing {
    Linear,
    EaseIn,
    EaseOut,
    #[default]
    EaseInOut,
    CubicBezier,
    Spring,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Scene {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    #[serde(default)]
    pub merge_mode: MergeMode,
    #[serde(default)]
    pub elements: Vec<Element>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<HashMap<String, serde_json::Value>>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum MergeMode {
    #[default]
    Inherit,
    Replace,
    Merge,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Overlay {
    pub id: String,
    #[serde(default)]
    pub elements: Vec<Element>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TransitionPreset {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<TransitionKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_frames: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub easing: Option<Easing>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub element_overrides: Option<HashMap<String, serde_json::Value>>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TransitionKind {
    Cut,
    Mix,
    Wipe,
    Sting,
    Move,
    Dve,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationRule {
    pub id: String,
    pub trigger: AutomationTrigger,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<HashMap<String, serde_json::Value>>,
    pub action: AutomationAction,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationTrigger {
    pub kind: AutomationTriggerKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<HashMap<String, serde_json::Value>>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AutomationTriggerKind {
    MediaEnd,
    MediaStart,
    Timer,
    TimeOfDay,
    AudioLevel,
    Hotkey,
    RssKeyword,
    StreamHealth,
    StateChange,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationAction {
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<HashMap<String, serde_json::Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Sequence {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Item {
    pub id: String,
    pub kind: ItemKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scene_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequence_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_frames: Option<u32>,
    #[serde(default)]
    pub auto_follow: bool,
    #[serde(default)]
    pub audio_policy: AudioPolicy,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ItemKind {
    SceneRef,
    SequenceRef,
    ClipRef,
    LiveRef,
    Slate,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum AudioPolicy {
    #[default]
    Clip,
    Bed,
    Mute,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Plugin {
    pub id: String,
    pub kind: PluginKind,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default = "default_max_memory")]
    pub max_memory_mib: u32,
    #[serde(default)]
    pub permissions: Vec<PluginPermission>,
}

fn default_max_memory() -> u32 {
    64
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PluginKind {
    Effect,
    Element,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PluginPermission {
    Network,
    Disk,
    Camera,
    Microphone,
}
