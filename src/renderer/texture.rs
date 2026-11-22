//! The procedural block texture array: one `TEXTURE_SIZE` RGBA tile per block
//! layer, generated on the CPU at startup with a full mip chain.

use crate::block::{TEXTURE_LAYERS, TEXTURE_SIZE};

/// Mip levels for a `TEXTURE_SIZE`-square tile: 16, 8, 4, 2, and 1 texels.
const MIP_LEVEL_COUNT: u32 = TEXTURE_SIZE.ilog2() + 1;

/// One level of the block array, stored layer-major as RGBA8 bytes.
struct MipLevel {
    width: u32,
    height: u32,
    data: Vec<u8>,
}

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
        mip_level_count: MIP_LEVEL_COUNT,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (level, mip) in block_texture_mips().iter().enumerate() {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: u32::try_from(level).expect("mip level fits in u32"),
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &mip.data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(mip.width * 4),
                rows_per_image: Some(mip.height),
            },
            wgpu::Extent3d {
                width: mip.width,
                height: mip.height,
                depth_or_array_layers: TEXTURE_LAYERS,
            },
        );
    }
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
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Linear,
        anisotropy_clamp: 1,
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

/// Builds every mip level for the procedural tiles, level 0 first.
fn block_texture_mips() -> Vec<MipLevel> {
    mip_chain(block_texture_data())
}

/// Builds the chain from a layer-major level-0 image. Each level halves the
/// previous one by averaging each 2x2 group of texels; alpha stays opaque.
fn mip_chain(base: Vec<u8>) -> Vec<MipLevel> {
    debug_assert!(
        TEXTURE_SIZE.is_power_of_two(),
        "mip chain requires a power-of-two tile"
    );
    debug_assert_eq!(
        base.len(),
        usize::try_from(TEXTURE_SIZE * TEXTURE_SIZE * 4 * TEXTURE_LAYERS)
            .expect("base image size fits")
    );
    let mut levels = Vec::with_capacity(usize::try_from(MIP_LEVEL_COUNT).expect("mip count fits"));
    levels.push(MipLevel {
        width: TEXTURE_SIZE,
        height: TEXTURE_SIZE,
        data: base,
    });
    for _ in 1..MIP_LEVEL_COUNT {
        let next = downsample_level(levels.last().expect("level 0 exists before downsampling"));
        levels.push(next);
    }
    levels
}

/// Averages each 2x2 group of texels in `previous`, in stored RGBA8 space.
fn downsample_level(previous: &MipLevel) -> MipLevel {
    let width = previous.width / 2;
    let height = previous.height / 2;
    let source_stride = usize::try_from(previous.width * 4).expect("source stride fits");
    let source_layer =
        usize::try_from(previous.height).expect("source height fits") * source_stride;
    let target_stride = usize::try_from(width * 4).expect("target stride fits");
    let target_layer = usize::try_from(height).expect("target height fits") * target_stride;
    let mut data =
        vec![0; target_layer * usize::try_from(TEXTURE_LAYERS).expect("layer count fits")];
    for layer in 0..TEXTURE_LAYERS {
        let source_base = usize::try_from(layer).expect("layer index fits") * source_layer;
        let target_base = usize::try_from(layer).expect("layer index fits") * target_layer;
        for y in 0..height {
            for x in 0..width {
                let mut texel = [0_u8; 4];
                for (channel, channel_out) in texel.iter_mut().take(3).enumerate() {
                    let mut sum = 0_u32;
                    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                        let offset = source_base
                            + usize::try_from((y * 2 + dy) * previous.width + (x * 2 + dx))
                                .expect("source texel fits")
                                * 4
                            + channel;
                        sum += u32::from(previous.data[offset]);
                    }
                    *channel_out = u8::try_from((sum + 2) / 4).expect("average fits in u8");
                }
                texel[3] = u8::MAX;
                let offset =
                    target_base + usize::try_from(y * width + x).expect("target texel fits") * 4;
                data[offset..offset + 4].copy_from_slice(&texel);
            }
        }
    }
    MipLevel {
        width,
        height,
        data,
    }
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

    /// FNV-1a digest of the level-0 bytes, pinned so a generator change is
    /// visible as a test failure even without a golden image.
    const LEVEL_ZERO_DIGEST: u64 = 0xE8BC_9AC7_86E0_FECB;

    fn tile_offset(layer: u32, x: u32, y: u32, width: u32, height: u32) -> usize {
        let layer_stride = usize::try_from(width * height * 4).unwrap();
        usize::try_from(layer).unwrap() * layer_stride + usize::try_from(y * width + x).unwrap() * 4
    }

    fn digest(bytes: &[u8]) -> u64 {
        bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
        })
    }

    #[test]
    fn mip_chain_has_the_expected_levels_and_sizes() {
        let levels = block_texture_mips();
        assert_eq!(
            levels.len(),
            usize::try_from(MIP_LEVEL_COUNT).unwrap(),
            "16x16 has five levels"
        );

        let expected = [(16, 16), (8, 8), (4, 4), (2, 2), (1, 1)];
        for (level, (width, height)) in levels.iter().zip(expected) {
            assert_eq!((level.width, level.height), (width, height));
            assert_eq!(
                level.data.len(),
                usize::try_from(width * height * 4 * TEXTURE_LAYERS).unwrap()
            );
        }
    }

    #[test]
    fn mip_chain_costs_3740_more_bytes_than_level_zero() {
        let levels = block_texture_mips();
        let total: usize = levels.iter().map(|level| level.data.len()).sum();

        assert_eq!(levels[0].data.len(), 11_264);
        assert_eq!(total, 15_004);
        assert_eq!(total - levels[0].data.len(), 3_740);
    }

    #[test]
    fn level_zero_matches_the_tile_generator_byte_for_byte() {
        let levels = block_texture_mips();
        let base = block_texture_data();

        assert_eq!(levels[0].data, base);
        assert_eq!(digest(&levels[0].data), LEVEL_ZERO_DIGEST);
    }

    #[test]
    fn level_one_is_the_exact_box_average_of_level_zero() {
        let levels = block_texture_mips();
        let (base, first) = (&levels[0], &levels[1]);

        for layer in [0_u32, 3, 8] {
            for y in 0..first.height {
                for x in 0..first.width {
                    let mut expected = [0_u8; 4];
                    for (channel, channel_out) in expected.iter_mut().take(3).enumerate() {
                        let mut sum = 0_u32;
                        for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                            let offset =
                                tile_offset(layer, x * 2 + dx, y * 2 + dy, base.width, base.height);
                            sum += u32::from(base.data[offset + channel]);
                        }
                        *channel_out = u8::try_from((sum + 2) / 4).unwrap();
                    }
                    expected[3] = 255;

                    let offset = tile_offset(layer, x, y, first.width, first.height);
                    assert_eq!(
                        &first.data[offset..offset + 4],
                        expected,
                        "layer {layer} texel ({x}, {y})"
                    );
                }
            }
        }
    }

    #[test]
    fn mip_generation_is_deterministic() {
        let first = block_texture_mips();
        let second = block_texture_mips();

        assert_eq!(first.len(), second.len());
        for (a, b) in first.iter().zip(second.iter()) {
            assert_eq!((a.width, a.height), (b.width, b.height));
            assert_eq!(a.data, b.data);
        }
    }

    #[test]
    fn layers_never_bleed_into_each_other() {
        // One solid colour per layer: any cross-layer averaging would turn
        // these into mixed colours by level 1 and beyond.
        let colours: Vec<[u8; 4]> = (0..TEXTURE_LAYERS)
            .map(|layer| {
                let value = u8::try_from(layer * 20).unwrap();
                [value, 255 - value, value / 2 + 7, 255]
            })
            .collect();
        let mut base = Vec::with_capacity(colours.len() * 16 * 16 * 4);
        for colour in &colours {
            for _ in 0..16 * 16 {
                base.extend_from_slice(colour);
            }
        }

        let levels = mip_chain(base);
        for (level, mip) in levels.iter().enumerate() {
            for (layer, colour) in colours.iter().enumerate() {
                let layer = u32::try_from(layer).unwrap();
                for y in 0..mip.height {
                    for x in 0..mip.width {
                        let offset = tile_offset(layer, x, y, mip.width, mip.height);
                        assert_eq!(
                            &mip.data[offset..offset + 4],
                            colour,
                            "level {level} layer {layer} texel ({x}, {y})"
                        );
                    }
                }
            }
        }
    }
}
