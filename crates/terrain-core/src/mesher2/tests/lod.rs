//! Coarse-LOD meshing tests.

use super::*;
use crate::padded::{LOCAL_SIZE, PaddedBlocks};

#[test]
fn coarse_skips_ao_and_block_light() {
    let stone = 1;
    let mut floor = PaddedBlocks::new();
    for x in 0..LOCAL_SIZE {
        for z in 0..LOCAL_SIZE {
            floor.set(x, 0, z, stone);
        }
    }
    let mut roofed = floor.clone();
    for x in 0..LOCAL_SIZE {
        for z in 0..LOCAL_SIZE {
            roofed.set(x, 10, z, stone);
        }
    }

    let open = mesh_coarse(&floor, &props(), MeshLod::Lod1, 1);
    let open_tops: Vec<&Quad> = open
        .opaque
        .iter()
        .filter(|quad| quad.normal() == NORMAL_POS_Y && quad.y() == 0)
        .collect();
    assert_eq!(open_tops.len(), 1);
    assert_eq!(open_tops[0].sky(), 15, "open ceiling is full sky");

    let shaded = mesh_coarse(&roofed, &props(), MeshLod::Lod1, 1);
    let shaded_tops: Vec<&Quad> = shaded
        .opaque
        .iter()
        .filter(|quad| quad.normal() == NORMAL_POS_Y && quad.y() == 0)
        .collect();
    assert_eq!(shaded_tops.len(), 1, "the uniform sky still merges");
    assert_eq!(shaded_tops[0].sky(), 6, "15 - (10 - 1)");
    assert!(shaded.opaque.iter().all(|quad| quad.ao() == 0));
    assert!(shaded.opaque.iter().all(|quad| quad.blk() == 0));
}
