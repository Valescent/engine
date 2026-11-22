//! Record layout, packing, and output-order tests.

use super::*;
use crate::description::SECTION_VOLUME;
use crate::padded::PaddedBlocks;
use crate::section::Section;

#[test]
fn record_layout_is_64_bits() {
    let zero = Quad::encode(0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0);
    assert_eq!(zero.bits(), 0);
    assert_eq!(
        (
            zero.x(),
            zero.y(),
            zero.z(),
            zero.w1(),
            zero.h1(),
            zero.normal(),
            zero.ao(),
            zero.sky(),
            zero.blk(),
            zero.tile(),
            zero.tint(),
        ),
        (0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0)
    );

    let max = Quad::encode(31, 31, 31, 31, 31, 7, 255, 15, 15, u16::MAX, 15);
    assert_eq!(
        max.bits(),
        u64::MAX,
        "every bit must belong to a field; the encoder masks nothing away"
    );
    assert_eq!(
        (
            max.x(),
            max.y(),
            max.z(),
            max.w1(),
            max.h1(),
            max.normal(),
            max.ao(),
            max.sky(),
            max.blk(),
            max.tile(),
            max.tint(),
        ),
        (31, 31, 31, 31, 31, 7, 255, 15, 15, u16::MAX, 15)
    );
}

#[test]
fn coordinates_round_trip_over_the_whole_section() {
    for x in 0..32_u8 {
        for y in 0..32_u8 {
            for z in 0..32_u8 {
                let quad = Quad::encode(x, y, z, 0, 0, 0, 0, 0, 0, 0, 0);
                assert_eq!((quad.x(), quad.y(), quad.z()), (x, y, z));
            }
        }
    }
}

#[test]
fn output_order_is_deterministic() {
    let stone = 1;
    let leaves = 5;

    let mut data: Box<[BlockId; SECTION_VOLUME]> = vec![0_u16; SECTION_VOLUME]
        .into_boxed_slice()
        .try_into()
        .expect("the scratch array matches the section volume");
    for y in 0..32_usize {
        for z in 0..32_usize {
            for x in 0..32_usize {
                let block = match (x * 7 + z * 13 + y) % 11 {
                    0 | 1 => 0,
                    2 => leaves,
                    _ => stone,
                };
                data[crate::section::section_index(
                    u32::try_from(x).expect("fits"),
                    u32::try_from(y).expect("fits"),
                    u32::try_from(z).expect("fits"),
                )] = block;
            }
        }
    }

    let direct = Section::from_blocks(&data);
    let mut incremental = Section::uniform(0);
    for (index, block) in data.iter().enumerate().rev() {
        incremental.set_index(index, *block);
    }
    assert_eq!(direct, incremental);

    let direct_pad = padded(&direct);
    let mut incremental_pad = PaddedBlocks::new();
    incremental_pad.fill_from(&incremental);
    let material = props();
    let light = lit();

    let first = mesh(&direct, &direct_pad, &light, &material, MeshLod::Lod0);
    let second = mesh(&direct, &direct_pad, &light, &material, MeshLod::Lod0);
    let third = mesh(
        &incremental,
        &incremental_pad,
        &light,
        &material,
        MeshLod::Lod0,
    );
    assert_eq!(raw(&first), raw(&second));
    assert_eq!(raw(&first), raw(&third));
}

/// Hashes one generated section's three ranges so a refactor cannot change
/// the bytes the mesher emits for a fixed seed and column.
#[test]
fn generated_section_mesh_is_hash_stable() {
    use crate::hash::sha256;
    use crate::pipeline::Terrain;
    use std::fmt::Write as _;

    fn digest(quads: &[Quad]) -> String {
        let mut bytes = Vec::with_capacity(quads.len() * 8);
        for quad in quads {
            bytes.extend_from_slice(&quad.bits().to_le_bytes());
        }
        let mut out = String::new();
        for byte in sha256(&bytes) {
            let _ = write!(out, "{byte:02x}");
        }
        out
    }

    let pack = crate::content::v1_default();
    let mut materials = MaterialProps::new();
    for record in pack.records() {
        let id = pack.id(&record.name).expect("every pack record has an id");
        materials.set(
            id,
            Material {
                solid: record.collision,
                opaque: record.opacity >= crate::content::LIGHT_LEVELS,
                water: record.liquid,
                cross_quad: record.render_shape == crate::content::RenderShape::CrossQuad,
                texture_top: 0,
                texture_side: 0,
                texture_bottom: 0,
                tint: 0,
                emissive: record.emission > 0,
            },
        );
    }

    let terrain = Terrain::new(2).expect("the fixture terrain builds");
    let section = terrain
        .section(7, 4, 1)
        .expect("the fixture section generates");
    let air = Section::uniform(0);
    let owners: [Option<&Section>; 26] = [Some(&air); 26];
    let blocks = PaddedBlocks::from_sections(&section, &owners);
    let result = mesh(&section, &blocks, &lit(), &materials, MeshLod::Lod0);

    assert_eq!(
        digest(&result.opaque),
        "5a2e86e7b1e39241efbdcbee2c2cc0b135109c6a90f5efc1acd4fca4f07357fe"
    );
    assert_eq!(
        digest(&result.cutout),
        "92fc8acf2625cd0a4da055029edf034ac5e7ceeec903a55890f2e791f511a962"
    );
    assert_eq!(
        digest(&result.water),
        "ddf55caf61f2c48bf8fc16807d55617512b94da54f3becc7527ad7eb9e32c21f"
    );
}
