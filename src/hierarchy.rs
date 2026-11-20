//! Hierarchy target selection for the first slice.
//!
//! L0 covers a detail radius in chunks. Level-1 sections (64 m) fill the ring
//! out to 768 m, and level-2 sections (128 m) fill out to 2,048 m. Selection is
//! distance-banded for now; projected-size refinement replaces it later.

#![allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]

use glam::{IVec2, IVec3};

use crate::section::{SECTION_SIZE, SectionKey};

/// Default L0 detail radius in chunks.
pub(crate) const DEFAULT_DETAIL_CHUNKS: i32 = 16;

/// Level-1 outer radius in metres.
const LEVEL1_OUTER_METERS: i32 = 768;
/// Level-2 outer radius in metres.
const LEVEL2_OUTER_METERS: i32 = 2_048;

const LEVEL1_EXTENT: i32 = SECTION_SIZE * 2;
const LEVEL2_EXTENT: i32 = SECTION_SIZE * 4;

/// Selects level-1 and level-2 sections around the player, nearest first.
pub(crate) fn select_sections(player: glam::Vec3, detail_chunks: i32) -> Vec<SectionKey> {
    let mut keys = Vec::new();
    let detail_meters = detail_chunks.max(1) * 32;
    let player_xz = IVec2::new(player.x.floor() as i32, player.z.floor() as i32);

    let level1_inner = ceil_div(detail_meters, LEVEL1_EXTENT);
    let level1_outer = ceil_div(LEVEL1_OUTER_METERS, LEVEL1_EXTENT);
    let level2_inner = ceil_div(LEVEL1_OUTER_METERS, LEVEL2_EXTENT);
    let level2_outer = ceil_div(LEVEL2_OUTER_METERS, LEVEL2_EXTENT);

    ring(
        &mut keys,
        player_xz,
        1,
        LEVEL1_EXTENT,
        level1_inner,
        level1_outer,
        2,
    );
    ring(
        &mut keys,
        player_xz,
        2,
        LEVEL2_EXTENT,
        level2_inner,
        level2_outer,
        1,
    );

    keys.sort_unstable_by_key(|key| {
        let extent = key.extent();
        let center = key.coord * extent + IVec3::splat(extent / 2);
        let dx = i64::from(center.x - player_xz.x);
        let dz = i64::from(center.z - player_xz.y);
        (dx * dx + dz * dz, u8::MAX - key.level)
    });
    keys
}

fn ceil_div(value: i32, divisor: i32) -> i32 {
    (value + divisor - 1) / divisor
}

#[allow(clippy::too_many_arguments)]
fn ring(
    keys: &mut Vec<SectionKey>,
    player_xz: IVec2,
    level: u8,
    extent: i32,
    inner: i32,
    outer: i32,
    vertical: i32,
) {
    let center = IVec2::new(
        player_xz.x.div_euclid(extent),
        player_xz.y.div_euclid(extent),
    );
    for sx in -outer..=outer {
        for sz in -outer..=outer {
            if sx.abs().max(sz.abs()) < inner {
                continue;
            }
            let coord = center + IVec2::new(sx, sz);
            for y in 0..vertical {
                keys.push(SectionKey::new(level, IVec3::new(coord.x, y, coord.y)));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_bands_do_not_overlap() {
        let keys = select_sections(glam::Vec3::new(16.0, 40.0, 16.0), 16);
        assert_ne!(keys.len(), 0);
        for key in &keys {
            match key.level {
                1 => assert!(key.coord.y == 0 || key.coord.y == 1),
                2 => assert_eq!(key.coord.y, 0),
                _ => panic!("unexpected level {key:?}"),
            }
            let offset = IVec2::new(key.coord.x, key.coord.z);
            let ring = offset.x.abs().max(offset.y.abs());
            match key.level {
                1 => assert!((8..=12).contains(&ring), "level 1 ring {ring}: {key:?}"),
                2 => assert!((6..=16).contains(&ring), "level 2 ring {ring}: {key:?}"),
                _ => unreachable!(),
            }
        }
        assert!(keys.iter().any(|key| key.level == 1 && key.coord.y == 1));
        assert!(keys.iter().any(|key| key.level == 2));
    }

    #[test]
    fn nearest_sections_come_first() {
        let keys = select_sections(glam::Vec3::ZERO, 16);
        let distance = |key: &SectionKey| {
            let extent = key.extent();
            let center = key.coord * extent + IVec3::splat(extent / 2);
            i64::from(center.x) * i64::from(center.x) + i64::from(center.z) * i64::from(center.z)
        };
        assert!(
            keys.windows(2)
                .all(|pair| distance(&pair[0]) <= distance(&pair[1]))
        );
    }

    #[test]
    fn negative_player_coordinates_select_both_rings() {
        let keys = select_sections(glam::Vec3::new(-1_200.0, 40.0, -1_200.0), 16);
        assert!(keys.iter().any(|key| key.level == 1));
        assert!(keys.iter().any(|key| key.level == 2));
    }

    #[test]
    fn a_smaller_detail_radius_adds_near_level1_sections() {
        let near = select_sections(glam::Vec3::ZERO, 4);
        let far = select_sections(glam::Vec3::ZERO, 16);
        assert!(near.len() > far.len());
    }
}
