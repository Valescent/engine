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

/// The 64 fine sections of one LOD2 section, in `4x4x4` order.
///
/// Slot `dx | (dz << 2) | (dy << 4)`: `dx` fastest, then `dz`, then `dy`.
/// A LOD2 section covers `128^3` blocks at four blocks per cell, exactly the
/// volume of four fine sections on each axis.
pub type Fine64<'a> = [&'a Section; 64];

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

/// Derives one LOD2 section from its 64 fine octants.
///
/// Each output cell covers a `4x4x4` group of fine blocks. The vote is the
/// same as [`downsample`]'s: if any cell is solid, take the most frequent solid
/// id and break ties by the lowest id; otherwise water if any cell is water;
/// otherwise air. Scatter ids are dropped before the vote.
///
/// # Errors
///
/// Returns [`Stage::Height`] with [`ErrorKind::InvariantViolated`] if the block
/// buffer cannot become a fixed array, which cannot happen for a 32^3 section.
pub fn downsample4(
    sections: &Fine64<'_>,
    water: BlockId,
    scatter: &[BlockId],
) -> Result<Section, TerrainError> {
    let side = 32_i32;
    let mut blocks = vec![AIR; SECTION_VOLUME];
    for cz in 0..side {
        for cx in 0..side {
            for cy in 0..side {
                let mut group = [AIR; 64];
                let section = section_of4(cx, cy, cz);
                let base_x = (4 * cx) & 31;
                let base_y = (4 * cy) & 31;
                let base_z = (4 * cz) & 31;
                for dy in 0..4 {
                    for dz in 0..4 {
                        for dx in 0..4 {
                            let slot = usize::try_from(dx | (dz << 2) | (dy << 4)).unwrap_or(0);
                            group[slot] = sections[section].get(
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
                check: "lod_group4",
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

/// Which fine section of a `4x4x4` group holds an LOD2 cell.
///
/// A LOD2 cell covers fine blocks `4c..4c+4`, which is aligned and therefore
/// never straddles a 32-block section boundary. Every `4x4x4` group lives
/// inside exactly one fine section, chosen by the cell's quarter.
fn section_of4(cx: i32, cy: i32, cz: i32) -> usize {
    let ox = usize::try_from(cx / 8).unwrap_or(0);
    let oz = usize::try_from(cz / 8).unwrap_or(0);
    let oy = usize::try_from(cy / 8).unwrap_or(0);
    ox | (oz << 2) | (oy << 4)
}

/// The `N1.4` vote for one group.
///
/// The group is normally 8 (`2x2x2`, LOD1) or 64 (`4x4x4`, LOD2) cells. A
/// caller at a clipped edge may pass fewer cells; absent cells simply do not
/// vote, and the conservative rule is otherwise unchanged. At most 64 cells
/// are counted.
fn choose(group: &[BlockId], water: BlockId, scatter: &[BlockId]) -> BlockId {
    // Group counts, in first-seen order. Distinct ids are few in practice.
    let mut counts = [(AIR, 0_u16); 64];
    let mut distinct = 0_usize;
    for &id in group {
        if id == AIR || id == water || scatter.contains(&id) {
            continue;
        }
        if let Some(entry) = counts[..distinct].iter_mut().find(|entry| entry.0 == id) {
            entry.1 = entry.1.saturating_add(1);
        } else if distinct < counts.len() {
            counts[distinct] = (id, 1);
            distinct += 1;
        }
    }
    let mut best = AIR;
    let mut best_count = 0_u16;
    for &(id, count) in &counts[..distinct] {
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

    fn fine64(pool: &[Section; 64]) -> Fine64<'_> {
        std::array::from_fn(|slot| &pool[slot])
    }

    #[test]
    fn lod2_visits_every_full_group() {
        let mut pool: [Section; 64] = std::array::from_fn(|_| uniform(AIR));
        for cz in 0..32_i32 {
            for cx in 0..32_i32 {
                for cy in 0..32_i32 {
                    // One solid cell in every 4x4x4 group makes every output
                    // cell solid, so no group can be skipped.
                    let slot = section_of4(cx, cy, cz);
                    let x = (4 * cx) & 31;
                    let y = (4 * cy) & 31;
                    let z = (4 * cz) & 31;
                    pool[slot].set(
                        x.cast_unsigned(),
                        y.cast_unsigned(),
                        z.cast_unsigned(),
                        STONE,
                    );
                }
            }
        }
        let coarse = downsample4(&fine64(&pool), WATER, &[]).unwrap();
        assert_eq!(coarse.is_uniform(), Some(STONE));
    }

    #[test]
    fn lod2_edge_groups_cover_the_section_corners() {
        let mut pool: [Section; 64] = std::array::from_fn(|_| uniform(AIR));
        // The first and last output cells use different fine sections.
        pool[0].set(0, 0, 0, STONE);
        pool[63].set(31, 31, 31, DIRT);
        let coarse = downsample4(&fine64(&pool), WATER, &[]).unwrap();
        assert_eq!(coarse.get(0, 0, 0), STONE);
        assert_eq!(coarse.get(31, 31, 31), DIRT);
        assert_eq!(coarse.get(31, 0, 0), AIR);
        assert_eq!(coarse.get(0, 31, 31), AIR);
    }

    #[test]
    fn lod2_fine_sections_land_in_their_quarter() {
        let mut pool: [Section; 64] = std::array::from_fn(|_| uniform(AIR));
        // Every fine section's local origin is one output cell, eight cells
        // apart. Slot 3 + 4*2 + 16*1 = 27 is fine (x, z, y) = (3, 2, 1).
        for section in &mut pool {
            section.set(0, 0, 0, STONE);
        }
        let coarse = downsample4(&fine64(&pool), WATER, &[]).unwrap();
        for slot in 0..64_i32 {
            let x = (slot & 3) * 8;
            let z = ((slot >> 2) & 3) * 8;
            let y = ((slot >> 4) & 3) * 8;
            assert_eq!(
                coarse.get(x.cast_unsigned(), y.cast_unsigned(), z.cast_unsigned()),
                STONE,
                "slot {slot} lands at output cell ({x}, {y}, {z})"
            );
        }
    }

    #[test]
    fn partial_group_at_an_edge_uses_the_same_vote() {
        // Fewer than 64 cells are legal for a group clipped by the edge.
        assert_eq!(choose(&[DIRT, DIRT, STONE], WATER, &[]), DIRT);
        assert_eq!(choose(&[STONE], WATER, &[]), STONE);
        assert_eq!(choose(&[WATER, AIR], WATER, &[]), WATER);
        assert_eq!(choose(&[], WATER, &[]), AIR);
        assert_eq!(choose(&[GRASS, GRASS, STONE], WATER, &[GRASS]), STONE);
    }

    #[test]
    fn lod2_derivation_is_pure() {
        let mut pool: [Section; 64] = std::array::from_fn(|slot| {
            let mut section = uniform(AIR);
            let id = BlockId::try_from(slot % 5).unwrap_or(0) + STONE;
            section.set(0, 0, 0, id);
            section
        });
        for section in &mut pool {
            section.set(7, 11, 13, DIRT);
        }
        let sections = fine64(&pool);
        let first = downsample4(&sections, WATER, &[]).unwrap();
        let second = downsample4(&sections, WATER, &[]).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.content_hash(), second.content_hash());
    }
}
