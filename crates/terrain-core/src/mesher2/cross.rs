//! Scatter (cross) quad collection.

use crate::padded::{LOCAL_SIZE, PaddedBlocks};

use super::greedy::{LightSource, face_light, signed};
use super::material::MaterialProps;
use super::record::{NORMAL_CROSS_A, NORMAL_CROSS_B, Quad};

/// Collects the two crossed quads of every scatter cell (`M2.4`).
pub(super) fn collect_cross(
    blocks: &PaddedBlocks,
    source: LightSource<'_>,
    props: &MaterialProps,
    out: &mut Vec<Quad>,
) {
    for y in 0..LOCAL_SIZE {
        for z in 0..LOCAL_SIZE {
            for x in 0..LOCAL_SIZE {
                let id = blocks.get(x, y, z);
                if id == 0 {
                    continue;
                }
                let material = props.material(id);
                if !material.cross_quad {
                    continue;
                }
                let (sky, blk) = face_light(source, blocks, props, [x, y, z]);
                for normal in [NORMAL_CROSS_A, NORMAL_CROSS_B] {
                    out.push(Quad::encode(
                        signed(x),
                        signed(y),
                        signed(z),
                        0,
                        0,
                        normal,
                        0,
                        sky,
                        blk,
                        material.texture_side,
                        material.tint,
                    ));
                }
            }
        }
    }
}
