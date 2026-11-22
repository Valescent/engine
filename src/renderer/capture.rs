//! Frame capture: the `ENGINE_CAPTURE` readback buffer and BMP writer.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tracing::{info, warn};

/// Default delay before the armed capture is read back.
const DEFAULT_CAPTURE_DELAY_SECS: f64 = 20.0;

/// Largest accepted capture delay, in seconds.
///
/// A day is far beyond any benchmark delay, and the bound keeps
/// [`Duration::from_secs_f64`] well inside its panic-free range.
const MAX_CAPTURE_DELAY_SECS: f64 = 86_400.0;

/// Parses `ENGINE_CAPTURE_DELAY` seconds, falling back to a safe default.
///
/// `Duration::from_secs_f64` panics on non-finite and out-of-range values, so
/// `inf`, `NaN`, and absurdly large inputs fall back to the default delay.
/// Finite negative values still capture immediately, matching the previous
/// `.max(0.0)` clamp.
pub(super) fn capture_delay(value: Option<&str>) -> Duration {
    let delay = value
        .and_then(|value| value.trim().parse::<f64>().ok())
        .filter(|delay| delay.is_finite() && *delay <= MAX_CAPTURE_DELAY_SECS)
        .map_or(DEFAULT_CAPTURE_DELAY_SECS, |delay| delay.max(0.0));
    Duration::from_secs_f64(delay)
}

/// One pending frame readback, armed by `ENGINE_CAPTURE`.
pub(super) struct Capture {
    pub(super) path: PathBuf,
    delay: Duration,
    started: Instant,
    pub(super) buffer: wgpu::Buffer,
    pub(super) bytes_per_row: u32,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) format: wgpu::TextureFormat,
    pub(super) done: bool,
}

impl Capture {
    pub(super) fn new(device: &wgpu::Device, config: &wgpu::SurfaceConfiguration) -> Option<Self> {
        let path = PathBuf::from(std::env::var("ENGINE_CAPTURE").ok()?);
        if !config.usage.contains(wgpu::TextureUsages::COPY_SRC) {
            warn!("surface cannot be copied; frame capture disabled");
            return None;
        }
        if !matches!(
            config.format,
            wgpu::TextureFormat::Bgra8UnormSrgb | wgpu::TextureFormat::Rgba8UnormSrgb
        ) {
            warn!(
                format = ?config.format,
                "unsupported surface format for frame capture; frame capture disabled"
            );
            return None;
        }
        let delay = capture_delay(std::env::var("ENGINE_CAPTURE_DELAY").ok().as_deref());
        let bytes_per_row = capture_bytes_per_row(config.width);
        let buffer = create_capture_buffer(device, bytes_per_row, config.height);
        info!(
            path = %path.display(),
            delay_secs = delay.as_secs_f64(),
            "frame capture armed"
        );
        Some(Self {
            path,
            delay,
            started: Instant::now(),
            buffer,
            bytes_per_row,
            width: config.width,
            height: config.height,
            format: config.format,
            done: false,
        })
    }

    /// Recreates the readback buffer when the surface extent changes.
    ///
    /// The pending deadline is kept, so a resize does not restart the delay and
    /// the next due frame captures at the new size.
    pub(super) fn resize(&mut self, device: &wgpu::Device, config: &wgpu::SurfaceConfiguration) {
        if self.done || (self.width == config.width && self.height == config.height) {
            return;
        }
        self.width = config.width;
        self.height = config.height;
        self.bytes_per_row = capture_bytes_per_row(config.width);
        self.buffer = create_capture_buffer(device, self.bytes_per_row, self.height);
    }

    pub(super) fn due(&self) -> bool {
        !self.done && self.started.elapsed() >= self.delay
    }
}

/// Padded readback row size for a four-byte-per-pixel surface.
pub(super) fn capture_bytes_per_row(width: u32) -> u32 {
    (width * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT
}

fn create_capture_buffer(device: &wgpu::Device, bytes_per_row: u32, height: u32) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("frame capture"),
        size: u64::from(bytes_per_row) * u64::from(height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    })
}

/// Writes one BGRA/RGBA readback as a 24-bit BMP.
#[allow(clippy::cast_possible_truncation)]
pub(super) fn write_bmp(
    path: &Path,
    width: u32,
    height: u32,
    bytes_per_row: u32,
    format: wgpu::TextureFormat,
    data: &[u8],
) -> std::io::Result<()> {
    let rgba = match format {
        wgpu::TextureFormat::Rgba8UnormSrgb => true,
        wgpu::TextureFormat::Bgra8UnormSrgb => false,
        _ => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("unsupported capture format: {format:?}"),
            ));
        }
    };
    let row_bytes = width.saturating_mul(3);
    let padding = (4 - row_bytes % 4) % 4;
    let image_size = (row_bytes + padding).saturating_mul(height);
    let file_size = 54 + image_size;
    let mut out = Vec::with_capacity(usize::try_from(file_size).unwrap_or(0));
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&file_size.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&54_u32.to_le_bytes());
    out.extend_from_slice(&40_u32.to_le_bytes());
    out.extend_from_slice(&i32::try_from(width).unwrap_or(i32::MAX).to_le_bytes());
    out.extend_from_slice(&i32::try_from(height).unwrap_or(i32::MAX).to_le_bytes());
    out.extend_from_slice(&1_u16.to_le_bytes());
    out.extend_from_slice(&24_u16.to_le_bytes());
    out.extend_from_slice(&0_u32.to_le_bytes());
    out.extend_from_slice(&image_size.to_le_bytes());
    out.extend_from_slice(&0_u32.to_le_bytes());
    out.extend_from_slice(&0_u32.to_le_bytes());
    out.extend_from_slice(&0_u32.to_le_bytes());
    out.extend_from_slice(&0_u32.to_le_bytes());
    for y in (0..height).rev() {
        let row = &data[y as usize * bytes_per_row as usize..];
        for x in 0..width {
            let i = x as usize * 4;
            let (blue, green, red) = if rgba {
                (row[i + 2], row[i + 1], row[i])
            } else {
                (row[i], row[i + 1], row[i + 2])
            };
            out.extend_from_slice(&[blue, green, red]);
        }
        out.extend(std::iter::repeat_n(0, padding as usize));
    }
    std::fs::write(path, out)
}
