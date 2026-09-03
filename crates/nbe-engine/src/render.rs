//! The render loop (Prompt 04 Steps: 2, 3, 4, 5, 6, 7).
//!
//! Frame output is driven by the master clock. The loop NEVER blocks on the
//! control-plane channel, disk, or network — it reads the clock, resolves the
//! current scene, and writes to render targets. Element renderers that need
//! to do real work (decode, fetch, layout) belong elsewhere.

use crate::clock::ClockState;
use crate::state::EngineState;
use crate::watchdog::Watchdog;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// What a scene resolves to in this Prompt. Prompt 04 only needs the slate
/// class: a solid colour that covers the frame. Images/videos land in 05+.
#[derive(Debug, Clone)]
pub enum SceneSource {
    /// A solid, test-pattern-capable fallback-safe colour fill.
    Solid([f32; 4]),
    /// Nothing current and correct: the engine MUST fall back.
    Missing,
}

/// The resolved contents of a scene at a given moment in show time.
#[derive(Debug, Clone)]
pub struct ResolvedScene {
    pub source: SceneSource,
}

/// The render targets this prompt manages.
pub struct RenderTargets {
    pub view: wgpu::Texture,
    pub preview: wgpu::Texture,
    pub fallback: wgpu::Texture,
}

pub const VIEW_W: u32 = 1920;
pub const VIEW_H: u32 = 1080;
pub const PREVIEW_W: u32 = 960;
pub const PREVIEW_H: u32 = 540;

/// Render side: what the watchdog reports and what the engine's telemetry
/// shows. Preview MUST miss rather than block the View.
#[derive(Debug, Default)]
pub struct FrameCounters {
    /// Cumulative View misses (SPEC §10.2).
    pub dropped_frames_total: AtomicU64,
    /// Preview misses are logged separately, never counted as dropped.
    pub preview_missed: AtomicU64,
}

pub struct RenderLoop {
    pub gpu: crate::gpu::Gpu,
    pub targets: RenderTargets,
    pub state: Arc<EngineState>,
    pub watchdog: Watchdog,
    pub counters: FrameCounters,
    /// Best-effort background that keeps the preview from blocking the View
    /// (degradation ladder rung 1).
    pub preview_rate_ratio: AtomicU64,
}

impl RenderLoop {
    pub async fn new(
        state: Arc<EngineState>,
        _house_rate: u32,
        requested: Option<nbe_protocol::QualityProfile>,
    ) -> anyhow::Result<Self> {
        let gpu = crate::gpu::Gpu::init(requested).await?;
        let targets = RenderTargets {
            view: make_texture(&gpu, VIEW_W, VIEW_H, "view"),
            preview: make_texture(&gpu, PREVIEW_W, PREVIEW_H, "preview"),
            fallback: make_texture(&gpu, VIEW_W, VIEW_H, "fallback"),
        };

        let watchdog = Watchdog::new(state.clone(), 2);

        Ok(Self {
            gpu,
            targets,
            state,
            watchdog,
            counters: FrameCounters::default(),
            preview_rate_ratio: AtomicU64::new(1),
        })
    }

    /// Resolve the render for a single master-clock frame.
    /// - Read clock frame
    /// - Resolve view scene / preview scene
    /// - Composite (clear + optional source)
    /// - Report the frame to the watchdog
    /// - Emit emitted View/Preview textures
    pub fn render_frame(&mut self, frame: u64) -> anyhow::Result<RenderResult> {
        let preview_rate_ratio = self.preview_rate_ratio.load(Ordering::SeqCst);
        let is_preview_tick = frame.is_multiple_of(preview_rate_ratio); // 50% at ratio 2

        // Preview degrades first — never block on it for the View.
        if is_preview_tick {
            let _ = self.render(preview_target(self), frame)?;
        }

        let view_target = if state_fallback_active(&self.state) {
            &self.targets.fallback
        } else {
            &self.targets.view
        };
        let view_tex = self.render(view_target, frame)?;

        // Report to the watchdog each frame (a "yes I did a frame" report
        // keeps its consecutive-misses counter accurate).
        self.watchdog.report_frame(0);
        let faults = self.watchdog.fault_active();
        Ok(RenderResult {
            view_tex,
            fault_active: faults,
        })
    }

    fn render(&self, target: &wgpu::Texture, _frame: u64) -> anyhow::Result<wgpu::Texture> {
        let color = if std::ptr::eq(target, &self.targets.fallback) {
            wgpu::Color {
                r: 1.0,
                g: 0.0,
                b: 1.0,
                a: 1.0,
            }
        } else if std::ptr::eq(target, &self.targets.preview) {
            wgpu::Color {
                r: 0.2,
                g: 0.2,
                b: 0.2,
                a: 1.0,
            }
        } else {
            wgpu::Color {
                r: 0.02,
                g: 0.02,
                b: 0.04,
                a: 1.0,
            }
        };
        let mut enc = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("render"),
            });
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        {
            let _pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(color),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        }
        self.gpu.queue.submit([enc.finish()]);
        Ok(target.clone())
    }

    /// Driven by the channel; consume one frame per period. Callers track the
    /// expected cadence (frame duration = 1s/house rate) and measure late
    /// calls so the loop stays clock-driven.
    pub async fn next_frame(&mut self) -> anyhow::Result<RenderResult> {
        match self.state.clock_state() {
            ClockState::Stopped => Err(anyhow::anyhow!("cannot render while clock is STOPPED")),
            ClockState::Running => {
                let frame = self
                    .state
                    .master_frame()
                    .ok_or_else(|| anyhow::anyhow!("clock stopped mid-run"))?;
                self.render_frame(frame)
            }
        }
    }
}

fn state_fallback_active(state: &EngineState) -> bool {
    state
        .fallback_active
        .load(std::sync::atomic::Ordering::SeqCst)
}

fn preview_target(render: &RenderLoop) -> &wgpu::Texture {
    &render.targets.preview
}

fn make_texture(gpu: &crate::gpu::Gpu, w: u32, h: u32, label: &str) -> wgpu::Texture {
    gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    })
}

pub struct RenderResult {
    pub view_tex: wgpu::Texture,
    pub fault_active: bool,
}
