//! The procedural block texture array: one `TEXTURE_SIZE` RGBA tile per block
//! layer, generated on the CPU at startup.

use crate::block::{TEXTURE_LAYERS, TEXTURE_SIZE};

pub(super) fn create_block_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layout: &wgpu::BindGroupLayout,
) -> (wgpu::Texture, wgpu::BindGroup) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("block texture array"),
        size: wgpu::Extent3d {
            width: TEXTURE_SIZE,
            height: TEXTURE_SIZE,
            depth_or_array_layers: TEXTURE_LAYERS,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let data = block_texture_data();
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(TEXTURE_SIZE * 4),
            rows_per_image: Some(TEXTURE_SIZE),
        },
        wgpu::Extent3d {
            width: TEXTURE_SIZE,
            height: TEXTURE_SIZE,
            depth_or_array_layers: TEXTURE_LAYERS,
        },
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    });
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("block texture sampler"),
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Nearest,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        ..Default::default()
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("block texture bind group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
        ],
    });
    (texture, bind_group)
}

fn block_texture_data() -> Vec<u8> {
    let mut data = Vec::with_capacity(
        usize::try_from(TEXTURE_SIZE * TEXTURE_SIZE * 4 * TEXTURE_LAYERS)
            .expect("texture data size fits in usize"),
    );
    for layer in 0..TEXTURE_LAYERS {
        for y in 0..TEXTURE_SIZE {
            for x in 0..TEXTURE_SIZE {
                data.extend_from_slice(&tile_pixel(layer, x, y));
            }
        }
    }
    data
}

fn tile_pixel(layer: u32, x: u32, y: u32) -> [u8; 4] {
    let noise = tile_noise(layer, x, y);
    let variation = i32::try_from(noise % 25).expect("variation fits in i32") - 12;
    let colour = match layer {
        0 => shade([122, 122, 128], variation),
        2 => shade([82, 142, 66], variation),
        3 if y < 4 => shade([82, 142, 66], variation),
        1 | 3 | 4 => shade([116, 82, 58], variation),
        5 => {
            let dx = i32::try_from(x).expect("tile x fits in i32") - 8;
            let dy = i32::try_from(y).expect("tile y fits in i32") - 8;
            let ring = (dx * dx + dy * dy) / 8 % 2;
            if ring == 0 {
                shade([150, 112, 70], variation)
            } else {
                shade([118, 86, 53], variation)
            }
        }
        6 if x.is_multiple_of(5) => shade([108, 78, 48], variation),
        6 => shade([150, 112, 70], variation),
        8 if x.is_multiple_of(4) || y.is_multiple_of(4) => shade([122, 96, 48], variation),
        8 => shade([238, 198, 108], variation),
        9 if noise.is_multiple_of(11) => shade([64, 132, 196], variation),
        9 => shade([46, 104, 172], variation),
        10 => shade([214, 196, 138], variation),
        _ => {
            if noise.is_multiple_of(7) {
                shade([42, 88, 42], variation)
            } else {
                shade([62, 118, 54], variation)
            }
        }
    };
    [colour[0], colour[1], colour[2], 255]
}

fn shade(base: [i32; 3], variation: i32) -> [u8; 3] {
    base.map(|channel| {
        u8::try_from((channel + variation).clamp(0, 255)).expect("colour channel in range")
    })
}

fn tile_noise(layer: u32, x: u32, y: u32) -> u32 {
    let mut value = layer
        .wrapping_mul(0x9E37_79B9)
        .wrapping_add(x.wrapping_mul(0x85EB_CA6B))
        .wrapping_add(y.wrapping_mul(0xC2B2_AE35));
    value ^= value >> 16;
    value = value.wrapping_mul(0x7FEB_352D);
    value ^= value >> 15;
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texture_data_has_one_rgba_tile_per_layer() {
        let data = block_texture_data();
        let tile_bytes = usize::try_from(TEXTURE_SIZE * TEXTURE_SIZE * 4).unwrap();
        assert_eq!(
            data.len(),
            tile_bytes * usize::try_from(TEXTURE_LAYERS).unwrap()
        );
    }

    #[test]
    fn texture_data_is_deterministic_and_layers_differ() {
        let first = block_texture_data();
        let second = block_texture_data();
        let tile_bytes = usize::try_from(TEXTURE_SIZE * TEXTURE_SIZE * 4).unwrap();

        assert_eq!(first, second);
        assert_ne!(first[..tile_bytes], second[tile_bytes..tile_bytes * 2]);
    }
}
