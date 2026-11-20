//! Level-to-level LOD derivation (`N1.4`, `N3.1`, `N3.2`).
//!
//! A coarse section stores `32^3` cells at two blocks per cell, so it covers
//! `64^3` blocks and is derived from **eight** fine sections, one per octant.
//! Nothing here generates terrain: LOD is derived, disposable, never saved, and
//! never authority (`N1.3`).
//!
//! The rule is most-frequent-solid, not any-solid: any-solid inflates distant
//! terrain and duplicates the water rule badly (`N3.1`).

use crate::description::SECTION_VOLUME;
use crate::error::{ErrorKind, Stage, TerrainError};
use crate::section::{BlockId, Section, section_index};

/// Downward skirt depth in coarse cells along a lower-resolution edge (`N3.2`).
pub const SKIRT_DEPTH: i32 = 2;

/// Air, by registry convention.
const AIR: BlockId = 0;

/// The eight fine sections of one coarse section, in octant order
/// `(dx, dz, dy)` with `dx` fastest.
pub type Octants<'a> = [&'a Section; 8];

/// Derives one coarse section from its eight fine octants.
///
/// Per `2x2x2` group: if any cell is solid, take the most frequent solid id and
/// break ties by the lowest id; otherwise water if any cell is water; otherwise
/// air. Scatter ids are dropped before the vote, so a flower never wins a coarse
/// cell. Logs and leaves are ordinary solids and are kept.
///
/// # Errors
///
/// Returns [`Stage::Height`] with [`ErrorKind::InvariantViolated`] if the block
/// buffer cannot become a fixed array, which cannot happen for a 32^3 section.
pub fn downsample(
    octants: &Octants<'_>,
    water: BlockId,
    scatter: &[BlockId],
) -> Result<Section, TerrainError> {
    let side = 32_i32;
    let mut blocks = vec![AIR; SECTION_VOLUME];
    for cz in 0..side {
        for cx in 0..side {
            for cy in 0..side {
                let mut group = [AIR; 8];
                let octant = octant_of(cx, cy, cz);
                let base_x = (2 * cx) & 31;
                let base_y = (2 * cy) & 31;
                let base_z = (2 * cz) & 31;
                for dy in 0..2 {
                    for dz in 0..2 {
                        for dx in 0..2 {
                            let slot = usize::try_from(dx | (dz << 1) | (dy << 2)).unwrap_or(0);
                            group[slot] = octants[octant].get(
                                (base_x + dx).cast_unsigned(),
                                (base_y + dy).cast_unsigned(),
                                (base_z + dz).cast_unsigned(),
                            );
                        }
                    }
                }
                let cell = choose(&group, water, scatter);
                let index =
                    section_index(cx.cast_unsigned(), cy.cast_unsigned(), cz.cast_unsigned());
                blocks[index] = cell;
            }
        }
    }
    let blocks: Box<[BlockId; SECTION_VOLUME]> = blocks.try_into().map_err(|_| {
        TerrainError::new(
            Stage::Height,
            ErrorKind::InvariantViolated {
                check: "lod_octant",
                coord: None,
            },
        )
    })?;
    Ok(Section::from_blocks(&blocks))
}

/// Which fine octant holds a coarse cell.
///
/// A coarse cell covers fine blocks `2c..2c+2`, which is even and therefore
/// never straddles a 32-block octant boundary. So every `2x2x2` group lives
/// inside exactly one octant, chosen by the coarse cell's half.
fn octant_of(cx: i32, cy: i32, cz: i32) -> usize {
    let ox = usize::try_from(cx / 16).unwrap_or(0);
    let oz = usize::try_from(cz / 16).unwrap_or(0);
    let oy = usize::try_from(cy / 16).unwrap_or(0);
    ox | (oz << 1) | (oy << 2)
}

/// The `N1.4` vote for one `2x2x2` group.
fn choose(group: &[BlockId; 8], water: BlockId, scatter: &[BlockId]) -> BlockId {
    let mut best = AIR;
    let mut best_count = 0_usize;
    for (position, &id) in group.iter().enumerate() {
        if id == AIR || id == water || scatter.contains(&id) {
            continue;
        }
        let count = group
            .iter()
            .skip(position + 1)
            .filter(|&&other| other == id)
            .count()
            .saturating_add(1);
        if count > best_count || (count == best_count && id < best) {
            best = id;
            best_count = count;
        }
    }
    if best_count > 0 {
        return best;
    }
    if group.contains(&water) {
        return water;
    }
    AIR
}

#[cfg(test)]
mod tests {
    use super::*;

    const STONE: BlockId = 1;
    const DIRT: BlockId = 2;
    const WATER: BlockId = 7;
    const GRASS: BlockId = 94;

    fn uniform(id: BlockId) -> Section {
        Section::uniform(id)
    }

    #[test]
    fn uniform_input_stays_uniform() {
        let octants = [
            &uniform(STONE),
            &uniform(STONE),
            &uniform(STONE),
            &uniform(STONE),
            &uniform(STONE),
            &uniform(STONE),
            &uniform(STONE),
            &uniform(STONE),
        ];
        let coarse = downsample(&octants, WATER, &[]).unwrap();
        assert_eq!(coarse.is_uniform(), Some(STONE));
    }

    #[test]
    fn any_solid_beats_water() {
        let water = uniform(WATER);
        let octants = [
            &water, &water, &water, &water, &water, &water, &water, &water,
        ];
        assert_eq!(
            downsample(&octants, WATER, &[]).unwrap().is_uniform(),
            Some(WATER)
        );

        let air = uniform(AIR);
        let stone = uniform(STONE);
        let octants = [
            &water, &stone, &water, &water, &water, &water, &water, &water,
        ];
        assert_eq!(
            downsample(&octants, WATER, &[]).unwrap().is_uniform(),
            None,
            "one solid octant makes the section mixed"
        );
        let octants = [
            &stone, &stone, &stone, &stone, &stone, &stone, &stone, &stone,
        ];
        assert_eq!(
            downsample(&octants, WATER, &[]).unwrap().is_uniform(),
            Some(STONE)
        );
        let octants = [&air, &air, &air, &air, &air, &air, &air, &air];
        assert_eq!(
            downsample(&octants, WATER, &[]).unwrap().is_uniform(),
            Some(AIR)
        );
    }

    #[test]
    fn a_cell_group_votes_by_frequency() {
        let mut a = uniform(AIR);
        a.set(0, 0, 0, STONE);
        a.set(1, 0, 0, DIRT);
        a.set(0, 1, 0, DIRT);
        let octants = [
            &a,
            &uniform(AIR),
            &uniform(AIR),
            &uniform(AIR),
            &uniform(AIR),
            &uniform(AIR),
            &uniform(AIR),
            &uniform(AIR),
        ];
        let coarse = downsample(&octants, WATER, &[]).unwrap();
        assert_eq!(coarse.get(0, 0, 0), DIRT, "two dirts beat one stone");
    }

    #[test]
    fn ties_pick_the_lowest_id() {
        let mut a = uniform(AIR);
        a.set(2, 4, 4, DIRT);
        a.set(3, 4, 4, STONE);
        let octants = [
            &a,
            &uniform(AIR),
            &uniform(AIR),
            &uniform(AIR),
            &uniform(AIR),
            &uniform(AIR),
            &uniform(AIR),
            &uniform(AIR),
        ];
        let coarse = downsample(&octants, WATER, &[]).unwrap();
        assert_eq!(coarse.get(1, 2, 2), STONE, "one stone against one dirt");
    }

    #[test]
    fn scatter_is_dropped_before_the_vote() {
        let mut a = uniform(AIR);
        a.set(0, 0, 0, GRASS);
        a.set(1, 0, 0, GRASS);
        a.set(0, 1, 0, STONE);
        let octants = [
            &a,
            &uniform(AIR),
            &uniform(AIR),
            &uniform(AIR),
            &uniform(AIR),
            &uniform(AIR),
            &uniform(AIR),
            &uniform(AIR),
        ];
        let coarse = downsample(&octants, WATER, &[GRASS]).unwrap();
        assert_eq!(
            coarse.get(0, 0, 0),
            STONE,
            "scatter never wins a coarse cell"
        );
    }

    #[test]
    fn all_air_stays_air() {
        let octants = [
            &uniform(AIR),
            &uniform(AIR),
            &uniform(AIR),
            &uniform(AIR),
            &uniform(AIR),
            &uniform(AIR),
            &uniform(AIR),
            &uniform(AIR),
        ];
        assert_eq!(
            downsample(&octants, WATER, &[]).unwrap().is_uniform(),
            Some(AIR)
        );
    }

    #[test]
    fn octants_land_in_the_right_cell() {
        let empty = uniform(AIR);
        let mut last = uniform(AIR);
        last.set(31, 31, 31, STONE);
        let octants = [
            &empty, &empty, &empty, &empty, &empty, &empty, &empty, &last,
        ];
        let coarse = downsample(&octants, WATER, &[]).unwrap();
        assert_eq!(coarse.get(31, 31, 31), STONE, "octant 7 is the far corner");
        assert_eq!(coarse.get(0, 0, 0), AIR);
    }

    #[test]
    fn derivation_is_pure() {
        let a = uniform(STONE);
        let b = uniform(DIRT);
        let octants = [&a, &b, &a, &b, &a, &b, &a, &b];
        let first = downsample(&octants, WATER, &[]).unwrap();
        let second = downsample(&octants, WATER, &[]).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.content_hash(), second.content_hash());
    }
}
