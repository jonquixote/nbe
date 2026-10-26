//! GPU setup and quality profile probe (Step 1).
//!
//! Probe the adapter once, hold the device/queue for the sibling lifetime,
//! and expose the probed quality profile (capped by the manifest's
//! requested profile, per SPEC §10.1.1).

use nbe_protocol::QualityProfile;
use wgpu::{Adapter, Device, Queue};

/// One wgpu context: device + queue + probe result.
pub struct Gpu {
    pub device: Device,
    pub queue: Queue,
    pub quality: QualityProfile,
    pub adapter_name: String,
}

impl Gpu {
    /// Probe the adapter and open a device.
    ///
    /// There is no `requested` parameter. One existed, threaded from
    /// `RenderLoop::new`, and was always `None` in production — `main` never
    /// passed one, and the manifest cap is applied where it belongs, in
    /// `EngineState::publish_quality_profile` (which is tested). Deleting the
    /// cap here left the suite green because the real one is elsewhere; the
    /// parameter was dead weight that made `gpu.quality` read as "probed"
    /// when it holds the probed value.
    pub async fn init() -> anyhow::Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        // HighPerformance, explicitly. The reference target is a dual-GPU
        // Intel MacBook Pro — Intel UHD 630 (1536 MB dynamic) plus a Radeon
        // Pro 555X (4 GB dedicated), per `docs/hardware-baseline.txt`. wgpu's
        // default preference is not `HighPerformance`, so relying on it means
        // relying on a heuristic that varies with wgpu version, macOS version
        // and whether the machine is on battery. Naming the preference makes
        // adapter choice a decision rather than a default.
        //
        // wgpu's own `WGPU_POWER_PREF` (`low`, `high`) overrides it, for
        // measurement only: R31's falsification runs the probe on both GPUs of
        // the reference machine. Unset, as in production, it is
        // `HighPerformance`; the adapter chosen is logged below either way.
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::from_env()
                    .unwrap_or(wgpu::PowerPreference::HighPerformance),
                ..Default::default()
            })
            .await
            .map_err(|e| anyhow::anyhow!("no wgpu adapter available: {e}"))?;
        let info = adapter.get_info();
        let name = info.name.clone();

        let probed = probe_quality(&adapter);

        // Logged because on a dual-GPU machine the adapter actually chosen is
        // the evidence, and the dress-rehearsal report quotes it.
        tracing::info!(
            adapter = %name,
            backend = ?info.backend,
            device_type = ?info.device_type,
            quality = ?probed,
            "gpu adapter selected"
        );

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await?;
        Ok(Self {
            device,
            queue,
            quality: probed,
            adapter_name: name,
        })
    }

    /// A render target / sampled texture in the engine's working format.
    pub fn make_texture(&self, width: u32, height: u32, label: &str) -> wgpu::Texture {
        self.device.create_texture(&wgpu::TextureDescriptor {
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

    /// Upload RGBA8 pixels into a texture. Called at load time, never in the
    /// per-frame path (SPEC §7.13): `write_texture` creates a staging buffer
    /// per call, 10.5 ms for one 1080p frame on the reference machine. The
    /// frame path uploads through an [`UploadPool`].
    pub fn upload_rgba(&self, tex: &wgpu::Texture, width: u32, height: u32, rgba: &[u8]) {
        self.queue.write_texture(
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

    /// Copy a 32-bit RGBA texture back to CPU memory.
    /// Caller is responsible for awaiting completion (poll + map callback).
    pub async fn readback_rgba(&self, tex: &wgpu::Texture) -> Vec<u8> {
        let width = tex.width();
        let height = tex.height();
        let row_bytes = aligned_bytes_per_row(width * 4);
        let size = (row_bytes as u64) * (height as u64);
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row_bytes),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        let idx = self.queue.submit([enc.finish()]);
        let _ = self.device.poll(wgpu::PollType::Wait {
            submission_index: Some(idx),
            timeout: Some(std::time::Duration::from_secs(5)),
        });
        let slice = buf.slice(..);
        slice.map_async(
            wgpu::MapMode::Read,
            |_: Result<(), wgpu::BufferAsyncError>| {},
        );
        let _ = self.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(std::time::Duration::from_secs(5)),
        });
        let data = slice.get_mapped_range().expect("buffer mapped after Wait");
        let row_bytes_out = (width * 4) as usize;
        let mut out = Vec::with_capacity(row_bytes_out * height as usize);
        for row in 0..height as usize {
            let off = row * row_bytes as usize;
            out.extend_from_slice(&data[off..off + row_bytes_out]);
        }
        out
    }
}

/// Staging buffers for uploads in the frame path (SPEC §12.8.1's read-ahead),
/// bounded and reused.
///
/// [`Gpu::upload_rgba`] is `queue.write_texture`, which creates a staging
/// buffer for every call: 10.5 ms for one 1080p RGBA frame on the reference
/// machine, against 0.9 ms to copy the same bytes into a buffer that already
/// exists and record the copy (measured on R20's branch). At the read-ahead's
/// four uploads a frame that was 42 ms against a 33 ms deadline, and every
/// recorded frame was skipped.
///
/// The pool holds at most `cap` buffers, creates them as it first needs them,
/// and takes each back once the GPU has consumed it. It never waits: with
/// every buffer in flight, [`stage`](Self::stage) refuses and the caller
/// uploads that frame on a later tick.
pub struct UploadPool {
    cap: usize,
    /// Buffers created and not lost (a failed re-map loses one).
    live: usize,
    /// Mapped, ready to write.
    free: Vec<wgpu::Buffer>,
    /// Written and unmapped, waiting for their encoder's submission.
    staged: Vec<wgpu::Buffer>,
    back_tx: std::sync::mpsc::Sender<Option<wgpu::Buffer>>,
    back_rx: std::sync::mpsc::Receiver<Option<wgpu::Buffer>>,
}

impl UploadPool {
    pub fn new(cap: usize) -> Self {
        let (back_tx, back_rx) = std::sync::mpsc::channel();
        Self {
            cap,
            live: 0,
            free: Vec::new(),
            staged: Vec::new(),
            back_tx,
            back_rx,
        }
    }

    /// Take back every buffer the GPU has finished with. Never waits, and
    /// does not poll the device with nothing in flight.
    pub fn reclaim(&mut self, gpu: &Gpu) {
        if self.live == self.free.len() + self.staged.len() {
            return;
        }
        let _ = gpu.device.poll(wgpu::PollType::Poll);
        self.drain_returned();
    }

    /// Wait for the GPU, then take back what it finished with. The load
    /// boundary only: the frame path never calls this.
    pub fn reclaim_waiting(&mut self, gpu: &Gpu) {
        let _ = gpu.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(std::time::Duration::from_secs(5)),
        });
        self.drain_returned();
    }

    fn drain_returned(&mut self) {
        while let Ok(back) = self.back_rx.try_recv() {
            match back {
                Some(b) => self.free.push(b),
                None => self.live -= 1,
            }
        }
    }

    /// Record an upload of `rgba` into `tex` on `enc`, through a free buffer.
    /// `false`, with nothing recorded, when every buffer is in flight. The
    /// copy lands when `enc` is submitted; call [`submitted`](Self::submitted)
    /// after that.
    pub fn stage(
        &mut self,
        gpu: &Gpu,
        enc: &mut wgpu::CommandEncoder,
        tex: &wgpu::Texture,
        width: u32,
        height: u32,
        rgba: &[u8],
    ) -> bool {
        let row = (width * 4) as usize;
        let stride = aligned_bytes_per_row(width * 4);
        let size = stride as u64 * height as u64;
        let buffer = match self.free.iter().position(|b| b.size() >= size) {
            Some(i) => self.free.swap_remove(i),
            None => {
                // Every free buffer is too small for this frame: give one up,
                // so a mixed-size package cannot wedge a full pool.
                if self.live >= self.cap && self.free.pop().is_some() {
                    self.live -= 1;
                }
                if self.live >= self.cap {
                    return false;
                }
                self.live += 1;
                gpu.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("upload staging"),
                    size,
                    usage: wgpu::BufferUsages::MAP_WRITE | wgpu::BufferUsages::COPY_SRC,
                    mapped_at_creation: true,
                })
            }
        };
        {
            let mut view = buffer
                .slice(..size)
                .get_mapped_range_mut()
                .expect("a free staging buffer is mapped");
            if stride as usize == row {
                view.copy_from_slice(&rgba[..row * height as usize]);
            } else {
                for (y, src) in rgba.chunks_exact(row).take(height as usize).enumerate() {
                    let at = y * stride as usize;
                    view.slice(at..at + row).copy_from_slice(src);
                }
            }
        }
        buffer.unmap();
        enc.copy_buffer_to_texture(
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(stride),
                    rows_per_image: Some(height),
                },
            },
            wgpu::TexelCopyTextureInfo {
                texture: tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.staged.push(buffer);
        true
    }

    /// Whether anything is staged and waiting for a submission.
    pub fn has_staged(&self) -> bool {
        !self.staged.is_empty()
    }

    /// After the encoder holding the staged copies is submitted: map each
    /// buffer again. It comes back through `reclaim` once the GPU has
    /// consumed it, or is counted lost if the map fails.
    pub fn submitted(&mut self) {
        for buffer in self.staged.drain(..) {
            let back = self.back_tx.clone();
            let returned = buffer.clone();
            buffer.slice(..).map_async(wgpu::MapMode::Write, move |r| {
                let _ = back.send(r.is_ok().then_some(returned));
            });
        }
    }
}

/// A probe heuristic: this engine does not claim to know the exact GPU class
/// — it answers from what the adapter reports.
fn probe_quality(adapter: &Adapter) -> QualityProfile {
    let info = adapter.get_info();
    match info.device_type {
        wgpu::DeviceType::Cpu => QualityProfile::Potato,
        _ => QualityProfile::Consumer,
    }
}

/// Align a row's byte count to wgpu.row-stride requirements.
pub fn aligned_bytes_per_row(bytes: u32) -> u32 {
    bytes.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT
}
