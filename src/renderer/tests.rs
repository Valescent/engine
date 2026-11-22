
use std::time::Duration;

use super::capture::{capture_bytes_per_row, capture_delay, write_bmp};
use super::chunk::{SHARED_QUADS, shared_index_data};
use super::*;

#[test]
fn pool_support_requires_indirect_vertex_storage_and_limits() {
    let supported = PoolCapabilities {
        legacy_forced: false,
        indirect_first_instance: true,
        indirect_execution: true,
        vertex_storage: true,
        storage_buffers_per_stage: 2,
        storage_buffer_binding_size: 1024,
        max_buffer_size: 1024,
        pool_bytes: 1024,
    };
    assert!(pool_supported(supported));
    assert!(!pool_supported(PoolCapabilities {
        legacy_forced: true,
        ..supported
    }));
    assert!(!pool_supported(PoolCapabilities {
        indirect_first_instance: false,
        ..supported
    }));
    assert!(!pool_supported(PoolCapabilities {
        indirect_execution: false,
        ..supported
    }));
    assert!(!pool_supported(PoolCapabilities {
        vertex_storage: false,
        ..supported
    }));
    assert!(!pool_supported(PoolCapabilities {
        storage_buffers_per_stage: 1,
        ..supported
    }));
    assert!(!pool_supported(PoolCapabilities {
        storage_buffer_binding_size: 512,
        ..supported
    }));
    assert!(!pool_supported(PoolCapabilities {
        max_buffer_size: 512,
        ..supported
    }));
}

#[test]
fn capture_rows_are_aligned_for_four_byte_pixels() {
    assert_eq!(capture_bytes_per_row(1), 256);
    assert_eq!(capture_bytes_per_row(64), 256);
    assert_eq!(capture_bytes_per_row(65), 512);
}

#[test]
fn bmp_decodes_rgba_surfaces() {
    let directory = std::env::temp_dir().join(format!("engine-bmp-rgba-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("capture.bmp");
    let data = [10, 20, 30, 255, 40, 50, 60, 255];
    write_bmp(&path, 2, 1, 8, wgpu::TextureFormat::Rgba8UnormSrgb, &data).unwrap();

    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(&bytes[54..60], &[30, 20, 10, 60, 50, 40]);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn bmp_rejects_formats_it_cannot_decode() {
    let path = std::env::temp_dir().join(format!("engine-bmp-bad-{}", std::process::id()));
    let error = write_bmp(&path, 1, 1, 4, wgpu::TextureFormat::Rgba16Float, &[0; 8]).unwrap_err();

    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    assert!(!path.exists());
}

#[test]
#[allow(clippy::cast_precision_loss)]
fn fallback_render_path_honours_the_horizon() {
    use crate::block::Block;
    use crate::world::World;
    use glam::IVec3;

    let mut world = World::default();
    for x in 64..96 {
        for z in 0..32 {
            world.set_block(IVec3::new(x, 10, z), Block::Stone);
        }
    }
    world.set_block(IVec3::new(320, 2, 0), Block::Stone);

    let mut camera = Camera::new(1.0);
    camera.set_position(Vec3::new(0.0, 10.0, 0.0));
    camera.set_yaw(std::f32::consts::FRAC_PI_2);
    camera.set_far_plane(1024.0);
    let frustum = Frustum::from_view_projection(camera.view_projection());
    let horizon = Horizon::build(camera.position(), &mut world);

    let chunk = ChunkCoord::new(10, 0, 0);
    let min = chunk.origin();
    assert!(frustum.intersects_aabb(min, min + Vec3::splat(CHUNK_SIZE as f32)));
    assert!(
        horizon.occludes(chunk),
        "the fallback path must skip a chunk its frustum keeps but the horizon hides"
    );
}

#[test]
fn capture_delay_rejects_non_finite_and_oversized_values() {
    assert_eq!(capture_delay(None), Duration::from_secs_f64(20.0));
    assert_eq!(capture_delay(Some("0.5")), Duration::from_secs_f64(0.5));
    assert_eq!(capture_delay(Some(" 3 ")), Duration::from_secs(3));
    assert_eq!(capture_delay(Some("-4")), Duration::ZERO);
    assert_eq!(capture_delay(Some("nonsense")), Duration::from_secs(20));
    assert_eq!(capture_delay(Some("inf")), Duration::from_secs(20));
    assert_eq!(capture_delay(Some("-inf")), Duration::from_secs(20));
    assert_eq!(capture_delay(Some("NaN")), Duration::from_secs(20));
    assert_eq!(capture_delay(Some("1e300")), Duration::from_secs(20));
}

#[test]
fn bmp_rows_are_bottom_up_bgr_with_padding() {
    let directory = std::env::temp_dir().join(format!("engine-bmp-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("capture.bmp");
    let data = [10, 20, 30, 255, 40, 50, 60, 255];
    write_bmp(&path, 2, 1, 8, wgpu::TextureFormat::Bgra8UnormSrgb, &data).unwrap();

    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(&bytes[..2], b"BM");
    assert_eq!(bytes.len(), 54 + 6 + 2);
    assert_eq!(&bytes[18..22], &2_i32.to_le_bytes());
    assert_eq!(&bytes[22..26], &1_i32.to_le_bytes());
    assert_eq!(&bytes[54..60], &[10, 20, 30, 40, 50, 60]);
    assert_eq!(&bytes[60..62], &[0, 0]);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn shared_index_pattern_matches_the_mesher() {
    let data = shared_index_data(3);
    assert_eq!(data.len(), 18);
    assert_eq!(&data[..6], &[0, 1, 2, 0, 2, 3]);
    assert_eq!(&data[6..12], &[4, 5, 6, 4, 6, 7]);
    assert_eq!(data.iter().max(), Some(&11));
    assert_eq!(
        shared_index_data(SHARED_QUADS).len(),
        SHARED_QUADS as usize * 6
    );
}

#[test]
fn vsync_prefers_fifo_and_tearing_falls_back() {
    assert_eq!(
        preferred_present_mode(true, &[wgpu::PresentMode::Fifo]),
        wgpu::PresentMode::Fifo
    );
    assert_eq!(
        preferred_present_mode(
            false,
            &[wgpu::PresentMode::Fifo, wgpu::PresentMode::Immediate]
        ),
        wgpu::PresentMode::Immediate
    );
    assert_eq!(
        preferred_present_mode(
            false,
            &[wgpu::PresentMode::Fifo, wgpu::PresentMode::Mailbox]
        ),
        wgpu::PresentMode::Mailbox
    );
    assert_eq!(
        preferred_present_mode(false, &[wgpu::PresentMode::Fifo]),
        wgpu::PresentMode::Fifo
    );
}
