//! GPU setup and quality profile probe (Step 1).
//!
//! Probe the adapter once, hold the device/queue for the sibling lifetime,
//! and expose the effective quality profile (capped by the manifest's
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
    pub async fn init(requested: Option<QualityProfile>) -> anyhow::Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions::default())
            .await
            .map_err(|e| anyhow::anyhow!("no wgpu adapter available: {e}"))?;
        let info = adapter.get_info();
        let name = info.name.clone();

        let probed = probe_quality(&adapter);
        let effective = requested.map(|req| probed.capped_by(req)).unwrap_or(probed);

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await?;
        Ok(Self {
            device,
            queue,
            quality: effective,
            adapter_name: name,
        })
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
