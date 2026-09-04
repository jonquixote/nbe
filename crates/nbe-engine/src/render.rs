//! The render loop (Prompt 04, Steps 2–4, 6–8).
//!
//! Pixels are a function of show state: the loop reads the clock, resolves the
//! View and Preview scenes from the package index, and composites them low-z
//! to high-z through one textured-quad pipeline. Transitions are quantized to
//! master-clock frames. Nothing here touches disk, network, or the control
//! plane channel (SPEC §7.13) — decode and upload happen at `show.load`.

use crate::gpu::Gpu;
use crate::scene::{Layer, LayerSource, ResolvedScene, TransitionKind};
use crate::state::EngineState;
use crate::watchdog::Watchdog;
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const VIEW_W: u32 = 1920;
pub const VIEW_H: u32 = 1080;
pub const PREVIEW_W: u32 = 960;
pub const PREVIEW_H: u32 = 540;

/// Uniform block per drawn layer: colour multiplier and destination rect.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct LayerUniform {
    color: [f32; 4],
    rect: [f32; 4],
}

/// Uniform stride, aligned for per-draw offsets.
const UNIFORM_STRIDE: u64 = 256;
/// Maximum layers composited in one frame.
const MAX_LAYERS: u64 = 64;

pub struct RenderTargets {
    pub view: wgpu::Texture,
    pub preview: wgpu::Texture,
}

/// Which bus a render call is for. Preview may be skipped or fail without
/// ever affecting the View (SPEC §10.2, §10.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bus {
    View,
    Preview,
}

/// Degradation ladder rungs implemented in this prompt (SPEC §10.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rung {
    /// Healthy: preview renders every frame.
    Nominal = 0,
    /// Under View deadline pressure: preview renders every other frame. The
    /// View is never degraded.
    PreviewHalfRate = 1,
}

/// What one frame did, for the caller's accounting and for tests.
#[derive(Debug, Clone)]
pub struct FrameReport {
    pub frame: u64,
    pub rendered_preview: bool,
    pub view_late_by: Option<Duration>,
    pub preview_failed: bool,
}

pub struct RenderLoop {
    pub gpu: Gpu,
    pub targets: RenderTargets,
    pub state: Arc<EngineState>,
    pub watchdog: Watchdog,
    /// Uploaded image textures by asset id; rebuilt when the package
    /// generation changes, never per frame.
    textures: HashMap<String, wgpu::Texture>,
    /// The uploaded fallback slate.
    fallback_tex: wgpu::Texture,
    /// 1x1 white texture; solid layers are this modulated by a colour.
    white: wgpu::Texture,
    loaded_generation: u64,
    pipeline: wgpu::RenderPipeline,
    bind_layout: wgpu::BindGroupLayout,
    uniforms: wgpu::Buffer,
    sampler: wgpu::Sampler,
    /// Test seam: extra work per View frame, to drive the deadline path.
    pub injected_view_delay: Option<Duration>,
    /// Test seam: make the preview render fail.
    pub fail_preview: bool,
    consecutive_late: u32,
    consecutive_on_time: u32,
}

impl RenderLoop {
    pub async fn new(
        state: Arc<EngineState>,
        requested: Option<nbe_protocol::QualityProfile>,
    ) -> anyhow::Result<Self> {
        let gpu = Gpu::init(requested).await?;
        // Step 5: the probe result is written into shared state HERE, in
        // production, so telemetry reports it because the engine put it there.
        *state.quality_profile.lock().unwrap() = Some(gpu.quality);

        let targets = RenderTargets {
            view: gpu.make_texture(VIEW_W, VIEW_H, "view"),
            preview: gpu.make_texture(PREVIEW_W, PREVIEW_H, "preview"),
        };
        let white = gpu.make_texture(1, 1, "white");
        gpu.upload_rgba(&white, 1, 1, &[255, 255, 255, 255]);
        let fallback_tex = gpu.make_texture(VIEW_W, VIEW_H, "fallback");

        let (pipeline, bind_layout) = build_pipeline(&gpu);
        let uniforms = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("layer-uniforms"),
            size: UNIFORM_STRIDE * MAX_LAYERS,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sampler = gpu.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("layer-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let watchdog = Watchdog::new(state.clone(), 2);

        let mut me = Self {
            gpu,
            targets,
            state,
            watchdog,
            textures: HashMap::new(),
            fallback_tex,
            white,
            loaded_generation: u64::MAX,
            pipeline,
            bind_layout,
            uniforms,
            sampler,
            injected_view_delay: None,
            fail_preview: false,
            consecutive_late: 0,
            consecutive_on_time: 0,
        };
        me.sync_package();
        Ok(me)
    }

    /// Upload decoded assets when the package generation changes. Runs at load
    /// boundaries, not per frame (SPEC §7.13).
    pub fn sync_package(&mut self) {
        let generation = self.state.package_generation.load(Ordering::SeqCst);
        if generation == self.loaded_generation {
            return;
        }
        self.loaded_generation = generation;
        self.textures.clear();

        {
            let index = self.state.package.lock().unwrap();
            if let Some(index) = index.as_ref() {
                for (asset_id, img) in &index.images {
                    let tex = self.gpu.make_texture(img.width, img.height, asset_id);
                    self.gpu.upload_rgba(&tex, img.width, img.height, &img.rgba);
                    self.textures.insert(asset_id.clone(), tex);
                }
            }
        }

        // Step 4: the fallback slate is uploaded at load time. Its bytes come
        // from Prompt 03's residency load; an asset that will not decode
        // becomes a generated slate rather than nothing on air.
        let slate = self.state.fallback_image();
        self.fallback_tex = self
            .gpu
            .make_texture(slate.width, slate.height, "fallback-slate");
        self.gpu
            .upload_rgba(&self.fallback_tex, slate.width, slate.height, &slate.rgba);
    }

    /// Render one master-clock frame on both buses.
    ///
    /// The View renders unconditionally — including while the clock is
    /// STOPPED, where the engine still owes the operator a picture.
    pub fn render_frame(&mut self, frame: u64, deadline: Option<Duration>) -> FrameReport {
        self.sync_package();

        let preview_due = match self.state.rung() {
            Rung::Nominal => true,
            Rung::PreviewHalfRate => frame.is_multiple_of(2),
        };

        // Preview first, and never allowed to affect the View: its failure is
        // counted and swallowed here (SPEC §10.2 — preview misses are logged,
        // never counted as dropped frames).
        let mut preview_failed = false;
        if preview_due {
            if let Err(e) = self.render_bus(Bus::Preview, frame) {
                preview_failed = true;
                self.state.preview_missed.fetch_add(1, Ordering::SeqCst);
                tracing::warn!(err = %e, frame, "preview render missed (View unaffected)");
            }
        }

        let started = Instant::now();
        if let Some(d) = self.injected_view_delay {
            std::thread::sleep(d);
        }
        if let Err(e) = self.render_bus(Bus::View, frame) {
            tracing::error!(err = %e, frame, "view render failed; engaging fallback");
            self.state.fallback_active.store(true, Ordering::SeqCst);
        }
        let elapsed = started.elapsed();

        // Step 3: a View frame not submitted by its deadline is a dropped
        // frame, and the watchdog hears the measured lateness — never a
        // constant.
        let view_late_by = match deadline {
            Some(budget) if elapsed > budget => {
                self.state
                    .dropped_frames_total
                    .fetch_add(1, Ordering::SeqCst);
                let late = elapsed - budget;
                let frames_missed = (late.as_secs_f64() / budget.as_secs_f64()).ceil() as u64;
                self.watchdog.report_frame(frames_missed.max(1));
                Some(late)
            }
            _ => {
                self.watchdog.report_frame(0);
                None
            }
        };
        self.update_rung(view_late_by.is_some());

        FrameReport {
            frame,
            rendered_preview: preview_due && !preview_failed,
            view_late_by,
            preview_failed,
        }
    }

    /// Degradation ladder: preview yields first, the View never does.
    fn update_rung(&mut self, late: bool) {
        if late {
            self.consecutive_late += 1;
            self.consecutive_on_time = 0;
            if self.consecutive_late >= 2 {
                self.state.set_rung(Rung::PreviewHalfRate);
            }
        } else {
            self.consecutive_on_time += 1;
            self.consecutive_late = 0;
            if self.consecutive_on_time >= 30 {
                self.state.set_rung(Rung::Nominal);
            }
        }
    }

    /// Resolve what a bus shows this frame, as (scene, alpha) pairs drawn in
    /// order. A running mix contributes two entries.
    pub fn scene_for(&self, bus: Bus, frame: u64) -> Vec<(ResolvedScene, f32)> {
        let index = self.state.package.lock().unwrap();
        let Some(index) = index.as_ref() else {
            return Vec::new();
        };
        if bus == Bus::Preview {
            let item = self.state.preview_item.lock().unwrap().clone();
            return vec![(index.resolve(item.as_deref()), 1.0)];
        }
        let transition = self.state.transition.lock().unwrap().clone();
        match transition {
            // Before the boundary the transition was issued for, the outgoing
            // item is still what is on air: no transition begins mid-frame.
            Some(t) if frame < t.start_frame => {
                vec![(index.resolve(t.from_item.as_deref()), 1.0)]
            }
            Some(t) if t.kind == TransitionKind::Mix && !t.is_complete(frame) => {
                let alpha = t.progress(frame);
                let mut out = Vec::new();
                if let Some(from) = t.from_item.as_deref() {
                    out.push((index.resolve(Some(from)), 1.0));
                }
                out.push((index.resolve(Some(&t.to_item)), alpha));
                out
            }
            _ => {
                let item = self.state.view_item.lock().unwrap().clone();
                vec![(index.resolve(item.as_deref()), 1.0)]
            }
        }
    }

    fn render_bus(&self, bus: Bus, frame: u64) -> anyhow::Result<()> {
        if bus == Bus::Preview && self.fail_preview {
            anyhow::bail!("injected preview failure");
        }
        let target = match bus {
            Bus::View => &self.targets.view,
            Bus::Preview => &self.targets.preview,
        };
        let show_fallback = bus == Bus::View && self.state.fallback_active.load(Ordering::SeqCst);

        let mut draws: Vec<(wgpu::Texture, LayerUniform)> = Vec::new();
        if show_fallback {
            draws.push((
                self.fallback_tex.clone(),
                LayerUniform {
                    color: [1.0, 1.0, 1.0, 1.0],
                    rect: crate::scene::FULL_RECT,
                },
            ));
        } else {
            for (scene, alpha) in self.scene_for(bus, frame) {
                for layer in scene.layers {
                    if let Some(d) = self.draw_for(&layer, alpha) {
                        draws.push(d);
                    }
                }
            }
        }

        let mut enc = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("composite"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        // An empty scene is black — a defined picture.
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            for (i, (tex, uniform)) in draws.iter().take(MAX_LAYERS as usize).enumerate() {
                let offset = UNIFORM_STRIDE * i as u64;
                self.gpu
                    .queue
                    .write_buffer(&self.uniforms, offset, bytemuck::bytes_of(uniform));
                let tex_view = tex.create_view(&wgpu::TextureViewDescriptor::default());
                let bind = self
                    .gpu
                    .device
                    .create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("layer"),
                        layout: &self.bind_layout,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                                    buffer: &self.uniforms,
                                    offset,
                                    size: std::num::NonZeroU64::new(
                                        std::mem::size_of::<LayerUniform>() as u64,
                                    ),
                                }),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: wgpu::BindingResource::TextureView(&tex_view),
                            },
                            wgpu::BindGroupEntry {
                                binding: 2,
                                resource: wgpu::BindingResource::Sampler(&self.sampler),
                            },
                        ],
                    });
                pass.set_bind_group(0, &bind, &[]);
                pass.draw(0..6, 0..1);
            }
        }
        self.gpu.queue.submit([enc.finish()]);
        Ok(())
    }

    fn draw_for(&self, layer: &Layer, alpha: f32) -> Option<(wgpu::Texture, LayerUniform)> {
        let (tex, mut color) = match &layer.source {
            LayerSource::Solid(c) => (self.white.clone(), *c),
            LayerSource::Image(asset) => (self.textures.get(asset)?.clone(), [1.0, 1.0, 1.0, 1.0]),
        };
        color[3] *= layer.opacity * alpha;
        Some((
            tex,
            LayerUniform {
                color,
                rect: layer.rect,
            },
        ))
    }

    /// Read the View target back as RGBA8. Inspection and test path only.
    pub async fn readback_view(&self) -> Vec<u8> {
        self.gpu.readback_rgba(&self.targets.view).await
    }

    /// Read the Preview target back as RGBA8.
    pub async fn readback_preview(&self) -> Vec<u8> {
        self.gpu.readback_rgba(&self.targets.preview).await
    }
}

fn build_pipeline(gpu: &Gpu) -> (wgpu::RenderPipeline, wgpu::BindGroupLayout) {
    let shader = gpu
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("composite"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
    let bind_layout = gpu
        .device
        .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("layer"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
    let layout = gpu
        .device
        .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("composite"),
            bind_group_layouts: &[Some(&bind_layout)],
            immediate_size: 0,
        });
    let pipeline = gpu
        .device
        .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("composite"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
    (pipeline, bind_layout)
}

const SHADER: &str = r#"
struct Params {
  color: vec4<f32>,
  rect: vec4<f32>,
};
@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var tex: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;

struct VsOut {
  @builtin(position) pos: vec4<f32>,
  @location(0) uv: vec2<f32>,
};

@vertex
fn vs(@builtin(vertex_index) i: u32) -> VsOut {
  var corners = array<vec2<f32>, 6>(
    vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
    vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0)
  );
  let c = corners[i];
  let x = p.rect.x + c.x * p.rect.z;
  let y = p.rect.y + c.y * p.rect.w;
  var out: VsOut;
  out.pos = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
  out.uv = c;
  return out;
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
  return textureSample(tex, samp, in.uv) * p.color;
}
"#;
