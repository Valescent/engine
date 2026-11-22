//! Column and section assembly: materials, surface rules, and biome picks.
//!
//! Everything here is a pure move from the original `pipeline` module; the
//! fixed `F1.5` order is unchanged.

use std::time::Instant;

use crate::climate::{Biome, BiomeTable};
use crate::content::ContentPack;
use crate::description::{HYDRO_TILE, MACRO_CELL, SEA, TILE_BLOCKS, WORLD_HEIGHT};
use crate::error::{ErrorKind, Stage, TerrainError};
use crate::features::{self, ApplyContext, ColumnField, Palette};
use crate::section::{BlockId, Section};
use crate::strata::Strata;
use crate::surface::{BiomeRules, ColumnFacts, Layer, Predicate, Rule, SurfaceStack, SurfaceTable};
use crate::volume::ColumnInput;

use super::hydro::{hydro_cell, river_level};
use super::terrain::{COLUMN, LATTICE_SIDE, TILE_CELLS, Terrain, TileData, micros};

/// Air. `core:air` sorts first, so its id is `0` by construction (`§0.1`, `§2.6`).
pub const AIR: BlockId = 0;
/// Water. Kept for the `terrain-gen` caller; pinned to the v1 pack by a test.
///
/// Generation never reads this constant: [`TerrainMaterials::water`] is the
/// authoritative handle.
pub const WATER: BlockId = 18;

/// The runtime block ids the column pipeline emits, resolved once from a
/// [`ContentPack`] by stable name (`§2.6`, `§0.1`).
///
/// Ids are runtime-only and never persisted, so this struct is the single
/// place the generator learns them. A missing record is a typed
/// [`ErrorKind::ManifestIncompatible`], never a default and never a panic.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TerrainMaterials {
    /// `core:air`.
    pub air: BlockId,
    /// `core:stone`; the strata resolver's symbolic fallback.
    pub stone: BlockId,
    /// `core:dirt`.
    pub dirt: BlockId,
    /// `core:grass`.
    pub grass: BlockId,
    /// `core:sand`.
    pub sand: BlockId,
    /// `core:gravel`.
    pub gravel: BlockId,
    /// `core:clay`.
    pub clay: BlockId,
    /// `core:snow`.
    pub snow: BlockId,
    /// `core:ice`.
    pub ice: BlockId,
    /// `core:water`.
    pub water: BlockId,
    /// `core:bedrock`; the `y = 0` floor and surface slot 9.
    pub bedrock: BlockId,
    /// `core:leaves`.
    pub leaves: BlockId,
    /// `core:wood`.
    pub wood: BlockId,
    /// `core:plant`.
    pub plant: BlockId,
    /// `core:lamp`.
    pub lamp: BlockId,
    /// `core:boulder`.
    pub boulder: BlockId,
    /// `core:ore_copper`.
    pub ore_copper: BlockId,
    /// `core:ore_iron`.
    pub ore_iron: BlockId,
}

impl TerrainMaterials {
    /// Resolves every id the pipeline emits from stable names (`§2.6`).
    ///
    /// # Errors
    ///
    /// [`ErrorKind::ManifestIncompatible`] naming the first missing record.
    pub fn from_pack(pack: &ContentPack) -> Result<Self, TerrainError> {
        fn resolve(pack: &ContentPack, name: &'static str) -> Result<BlockId, TerrainError> {
            pack.id(name).ok_or_else(|| {
                TerrainError::new(
                    Stage::Description,
                    ErrorKind::ManifestIncompatible {
                        id: name.to_owned(),
                        reason: "missing block record",
                    },
                )
            })
        }

        Ok(Self {
            air: resolve(pack, "core:air")?,
            stone: resolve(pack, "core:stone")?,
            dirt: resolve(pack, "core:dirt")?,
            grass: resolve(pack, "core:grass")?,
            sand: resolve(pack, "core:sand")?,
            gravel: resolve(pack, "core:gravel")?,
            clay: resolve(pack, "core:clay")?,
            snow: resolve(pack, "core:snow")?,
            ice: resolve(pack, "core:ice")?,
            water: resolve(pack, "core:water")?,
            bedrock: resolve(pack, "core:bedrock")?,
            leaves: resolve(pack, "core:leaves")?,
            wood: resolve(pack, "core:wood")?,
            plant: resolve(pack, "core:plant")?,
            lamp: resolve(pack, "core:lamp")?,
            boulder: resolve(pack, "core:boulder")?,
            ore_copper: resolve(pack, "core:ore_copper")?,
            ore_iron: resolve(pack, "core:ore_iron")?,
        })
    }
}

impl Terrain {
    /// The per-column biome pick at one world column (`E3.3`).
    ///
    /// This is the same value [`Terrain::column`] uses for the surface rules and
    /// `Terrain::footprint` uses for features. It is exposed for the blend
    /// tests and debug dumps; generation itself reads the cached tile directly.
    ///
    /// # Errors
    ///
    /// Propagates tile failures.
    pub fn biome_at(&self, block_x: i32, block_z: i32) -> Result<u8, TerrainError> {
        let tile_x = crate::lattice::floor_div(block_x, TILE_BLOCKS);
        let tile_z = crate::lattice::floor_div(block_z, TILE_BLOCKS);
        let tile = self.tile(tile_x, tile_z)?;
        let local_x = block_x - tile_x * TILE_BLOCKS;
        let local_z = block_z - tile_z * TILE_BLOCKS;
        Ok(self.column_biome(&tile, local_x, local_z, block_x, block_z))
    }
}

impl Terrain {
    /// The per-column biome pick (`E3.3`).
    ///
    /// One cell lookup and one hash per column. The climate tile already owns the
    /// quantised margin, so this does not re-derive a weight; the hash is keyed on
    /// `(x, z)` only, so the surface rules and the feature footprint for a column
    /// resolve the same biome.
    fn column_biome(
        &self,
        tile: &TileData,
        local_x: i32,
        local_z: i32,
        block_x: i32,
        block_z: i32,
    ) -> u8 {
        tile.climate.biome_pick(
            local_x.div_euclid(MACRO_CELL),
            local_z.div_euclid(MACRO_CELL),
            block_x,
            block_z,
            self.seed,
            self.dither_tag,
        )
    }
}

impl Terrain {
    /// Generates one column without consulting the memo.
    pub(super) fn generate_column(
        &self,
        block_x: i32,
        block_z: i32,
    ) -> Result<[BlockId; COLUMN], TerrainError> {
        let tile_x = crate::lattice::floor_div(block_x, TILE_BLOCKS);
        let tile_z = crate::lattice::floor_div(block_z, TILE_BLOCKS);
        let tile = self.tile(tile_x, tile_z)?;
        let local_x = block_x - tile_x * TILE_BLOCKS;
        let local_z = block_z - tile_z * TILE_BLOCKS;
        let top = tile.height.top(local_x, local_z);
        let slope = tile.height.slope_drop(local_x, local_z);
        let macro_h = tile.macro_grid.sample(local_x, local_z);
        let continentalness = continentalness_at(&tile, block_x, block_z, tile_x, tile_z);
        let base = self.base_blocks(block_x as f32, block_z as f32, continentalness);
        let rough = self.roughness.eval_2d(block_x as f32, block_z as f32);
        let cell_x = local_x.div_euclid(MACRO_CELL);
        let cell_z = local_z.div_euclid(MACRO_CELL);
        let hydro = self.hydro_tile(
            crate::lattice::floor_div(block_x, crate::description::HYDRO_TILE),
            crate::lattice::floor_div(block_z, crate::description::HYDRO_TILE),
        )?;
        let (hydro_x, hydro_z) = hydro_cell(block_x, block_z);
        let river_width = hydro.river_width(hydro_x, hydro_z);
        let lake_level = hydro.lake_level(hydro_x, hydro_z);
        let humidity = tile.climate.humidity(cell_x, cell_z);
        let temp = tile.climate.temp(cell_x, cell_z);
        let biome = self.column_biome(&tile, local_x, local_z, block_x, block_z);
        let mut column = [self.materials.air; COLUMN];
        debug_assert!(humidity.is_finite());
        let facts = ColumnFacts {
            x: block_x,
            z: block_z,
            top,
            base,
            temp,
            slope_drop: slope,
            is_river: river_width > 0.5,
            is_lake: lake_level > SEA,
            is_bed: river_width > 0.5 || lake_level > SEA,
            near_coast: base.abs() < 4.0,
            water_adjacent: river_width > 0.0,
        };
        fill_stack(
            &mut column,
            top,
            &facts,
            biome,
            &self.surface,
            &self.strata,
            &self.materials,
        );
        let volume_started = Instant::now();
        self.volume.apply(
            &mut column,
            &ColumnInput {
                x: block_x,
                z: block_z,
                top,
                rough,
                slope,
                is_river: river_width > 0.5,
                is_lake: lake_level > SEA,
                near_bank: river_width > 0.5 && river_width < 2.0,
                macro_height: macro_h,
            },
            self.materials.air,
            self.materials.stone,
        );
        // One sample per column. A single column is usually under a millisecond,
        // so the pinned integer-millisecond unit honestly reports 0 for it; a
        // mountain column that crosses the millisecond does show up.
        self.counters
            .record_latency(Stage::Volume, micros(volume_started));
        let water_level = SEA
            .max(lake_level)
            .max(river_level(&hydro, hydro_x, hydro_z));
        for y in (top + 1).max(1)..=water_level.min(WORLD_HEIGHT as i32 - 1) {
            let index = y as usize;
            if column[index] == self.materials.air {
                column[index] = self.materials.water;
            }
        }
        column[0] = self.materials.bedrock;
        Ok(column)
    }
}

impl Terrain {
    /// A whole 32^3 section, generated column by column.
    ///
    /// `F1.5` runs the feature passes between the column and
    /// [`Section::from_blocks`], so ores, boulders, trees, and plants are already
    /// in the block array the section is built from. The feature passes walk
    /// `features::WRITE_ORDER`; nothing here re-states their order.
    ///
    /// # Errors
    ///
    /// Propagates column failures.
    pub fn section(
        &self,
        section_x: i32,
        section_y: i32,
        section_z: i32,
    ) -> Result<Section, TerrainError> {
        let base_x = section_x * 32;
        let base_y = section_y * 32;
        let base_z = section_z * 32;
        let mut blocks =
            vec![self.materials.air; crate::description::SECTION_VOLUME].into_boxed_slice();
        for z in 0..32 {
            for x in 0..32 {
                let column = self.shared_column(base_x + x, base_z + z)?;
                for y in 0..32 {
                    let world_y = base_y + y;
                    if world_y < 0 || world_y >= WORLD_HEIGHT as i32 {
                        continue;
                    }
                    blocks[crate::section::section_index(
                        u32::try_from(x).unwrap_or(0),
                        u32::try_from(y).unwrap_or(0),
                        u32::try_from(z).unwrap_or(0),
                    )] = column[world_y as usize];
                }
            }
        }
        let mut blocks: Box<[BlockId; crate::description::SECTION_VOLUME]> =
            blocks.try_into().map_err(|_| {
                TerrainError::new(
                    crate::error::Stage::Height,
                    crate::error::ErrorKind::InvariantViolated {
                        check: "section_length",
                        coord: None,
                    },
                )
            })?;

        self.apply_features(
            &mut blocks,
            [section_x, section_y, section_z],
            [base_x, base_y, base_z],
        )?;
        // `lod0_columns` counts distinct LOD0 columns with at least one generated
        // section (`N1.4`). No LOD derive runs in this pipeline, so `lod1_columns`
        // and `lod2_columns` stay 0 until a caller derives a coarser level.
        let lod0 = {
            let mut columns = self.lod0_columns.borrow_mut();
            columns.insert((section_x, section_z));
            u64::try_from(columns.len()).unwrap_or(u64::MAX)
        };
        self.counters.set("lod0_columns", lod0);
        Ok(Section::from_blocks(&blocks))
    }

    /// `F1.5` step 6: apply the feature passes to one finished section (`H2.1`).
    ///
    /// `H2.2`: a uniform section is skipped only when no candidate can write its
    /// id, so a canopy may cross a section top and an ore vein may enter a fully
    /// stone section. Split from [`Terrain::section`] to keep that function
    /// readable; behaviour is identical.
    fn apply_features(
        &self,
        blocks: &mut [BlockId; crate::description::SECTION_VOLUME],
        section: [i32; 3],
        base: [i32; 3],
    ) -> Result<(), TerrainError> {
        let uniform = blocks
            .first()
            .copied()
            .filter(|first| blocks.iter().all(|id| id == first));
        let specs = features::v1_specs();
        let candidates = features::gather(section, self.seed, &specs);
        // `G3.2`: the stone family only. `dirt` is the pipeline's placeholder soil
        // as well as a band material, so excluding it keeps veins out of the
        // surface stack.
        let host_rock = [self.materials.stone];
        let palette = Palette {
            air: self.materials.air,
            water: self.materials.water,
            bedrock: self.materials.bedrock,
            log: self.materials.wood,
            leaf: self.materials.leaves,
            boulder: self.materials.boulder,
        };
        let wanted = uniform.is_none_or(|id| {
            features::uniform_writable(id, base, &candidates, palette, &host_rock, &specs)
        });
        if !wanted {
            return Ok(());
        }
        let features_started = Instant::now();
        let footprint = self.footprint(section[0], section[2])?;
        let ores = [
            features::OreBlock {
                name: "ore_iron",
                id: self.materials.ore_iron,
            },
            features::OreBlock {
                name: "ore_copper",
                id: self.materials.ore_copper,
            },
        ];
        let scatter = [self.materials.plant];
        let ctx = ApplyContext {
            seed: self.seed,
            palette,
            ores: &ores,
            host_rock: &host_rock,
            columns: footprint.field(),
            regions: features::gather_origins(section),
            scatter: &scatter,
        };
        let _ = features::apply(blocks, &candidates, &ctx, &specs);
        if base[1] == 0 {
            // `H2.3`: nothing writes bedrock. Put the `y = 0` floor back after
            // the feature passes.
            for z in 0..32 {
                for x in 0..32 {
                    blocks[crate::section::section_index(
                        u32::try_from(x).unwrap_or(0),
                        0,
                        u32::try_from(z).unwrap_or(0),
                    )] = self.materials.bedrock;
                }
            }
        }
        // Diagnostics only: the feature pass is complete and `blocks` is final.
        // Nothing measured here can reach the section.
        self.counters
            .record_latency(Stage::Features, micros(features_started));
        Ok(())
    }

    /// Column data for the `3 x 3` feature footprint of one section.
    ///
    /// Tops and slopes are the same [`HeightTile`] values the column pipeline
    /// reads (`E4.3`, `E4.6`), so a tree stands on the surface before detail and
    /// caves. Biomes are the same per-column `biome_pick` result the surface
    /// rules use, and flags mirror the river and bank facts of
    /// [`Terrain::column`]. This reads tile caches only; it never generates a
    /// column a second time.
    pub(super) fn footprint(
        &self,
        section_x: i32,
        section_z: i32,
    ) -> Result<Footprint, TerrainError> {
        let origin = [
            section_x * features::REGION_SIZE - features::REGION_SIZE,
            section_z * features::REGION_SIZE - features::REGION_SIZE,
        ];
        let mut footprint = Footprint::new(origin);
        for local_z in 0..features::FOOTPRINT {
            for local_x in 0..features::FOOTPRINT {
                let block_x = origin[0] + local_x;
                let block_z = origin[1] + local_z;
                let tile_x = crate::lattice::floor_div(block_x, TILE_BLOCKS);
                let tile_z = crate::lattice::floor_div(block_z, TILE_BLOCKS);
                let tile = self.tile(tile_x, tile_z)?;
                let tile_local_x = block_x - tile_x * TILE_BLOCKS;
                let tile_local_z = block_z - tile_z * TILE_BLOCKS;
                let slot = usize::try_from(local_x + features::FOOTPRINT * local_z)
                    .expect("a footprint slot fits usize");

                footprint.tops[slot] = tile.height.top(tile_local_x, tile_local_z);
                footprint.slope[slot] = tile.height.slope_drop(tile_local_x, tile_local_z);
                footprint.biome[slot] =
                    self.column_biome(&tile, tile_local_x, tile_local_z, block_x, block_z);

                let hydro = self.hydro_tile(
                    crate::lattice::floor_div(block_x, HYDRO_TILE),
                    crate::lattice::floor_div(block_z, HYDRO_TILE),
                )?;
                let (hydro_x, hydro_z) = hydro_cell(block_x, block_z);
                let width = hydro.river_width(hydro_x, hydro_z);
                let lake = hydro.lake_level(hydro_x, hydro_z);
                let mut flags = 0_u8;
                if width > 0.5 || lake > SEA {
                    flags |= features::COLUMN_RIVER;
                }
                if width > 0.5 && width < 2.0 {
                    flags |= features::COLUMN_BANK;
                }
                footprint.flags[slot] = flags;
            }
        }
        Ok(footprint)
    }
}

/// Owns the column arrays an [`ApplyContext`] borrows for one section.
pub(super) struct Footprint {
    /// World `(x, z)` of the first column.
    pub(super) origin: [i32; 2],
    /// Top solid block per column.
    tops: Vec<i32>,
    /// Largest one-block step to a neighbour.
    slope: Vec<i32>,
    /// Biome id per column.
    pub(super) biome: Vec<u8>,
    /// River and bank bits per column.
    flags: Vec<u8>,
}

impl Footprint {
    fn new(origin: [i32; 2]) -> Self {
        Self {
            origin,
            tops: vec![0; features::COLUMNS],
            slope: vec![0; features::COLUMNS],
            biome: vec![0; features::COLUMNS],
            flags: vec![0; features::COLUMNS],
        }
    }

    fn field(&self) -> ColumnField<'_> {
        ColumnField {
            origin: self.origin,
            tops: &self.tops,
            slope: &self.slope,
            biome: &self.biome,
            flags: &self.flags,
        }
    }
}

fn fill_stack(
    column: &mut [BlockId; COLUMN],
    top: i32,
    facts: &ColumnFacts,
    biome: u8,
    surface: &SurfaceTable,
    strata: &Strata,
    materials: &TerrainMaterials,
) {
    let rules = v1_rules(materials);
    let stack: SurfaceStack =
        surface.resolve_column(facts, &rules[usize::from(biome) % rules.len()], strata);
    for y in 1..=top.min(WORLD_HEIGHT as i32 - 1) {
        let depth = top - y;
        let id = match stack.layer_at(depth) {
            Some(Layer::Block(id)) => id,
            Some(Layer::Stone) | None => strata.stone_at(facts.x, y, facts.z),
        };
        column[y as usize] = id;
    }
}

fn v1_rules(materials: &TerrainMaterials) -> Vec<BiomeRules> {
    let topsoils = [
        materials.grass,
        materials.grass,
        materials.sand,
        materials.grass,
    ];
    topsoils
        .iter()
        .map(|&id| BiomeRules {
            rules: vec![
                Rule {
                    predicate: Predicate::And(Vec::new()),
                    block: id,
                    depth_blocks: 4,
                },
                Rule {
                    predicate: Predicate::And(Vec::new()),
                    block: materials.dirt,
                    depth_blocks: 4,
                },
                Rule {
                    predicate: Predicate::And(Vec::new()),
                    block: materials.stone,
                    depth_blocks: 1,
                },
            ],
        })
        .collect()
}

/// The answered v1 climate biome set (`E3.2`), in stable id order:
/// `0 plains`, `1 forest`, `2 desert`, `3 tundra`.
///
/// Desert carries weight `1.15` so its humidity reach matches the measured dry
/// tail; the other three use weight `1.0`. Every priority is `0`. `ocean` and `beach` are
/// **override surface sets**, not climate biomes, so they are deliberately
/// absent here and never enter climate space (`E3.1`, `E3.2`). "Mountains" is
/// not a biome either: the alpine look comes from snow cap, slope stone,
/// strata, and deep rock.
pub(super) fn v1_biomes() -> BiomeTable {
    BiomeTable::new(vec![
        Biome {
            id: 0,
            temp_c: 0.55,
            humid_c: 0.45,
            weight: 1.0,
            priority: 0,
        },
        Biome {
            id: 1,
            temp_c: 0.50,
            humid_c: 0.75,
            weight: 1.0,
            priority: 0,
        },
        Biome {
            id: 2,
            temp_c: 0.84,
            humid_c: 0.15,
            weight: 1.15,
            priority: 0,
        },
        Biome {
            id: 3,
            temp_c: 0.10,
            humid_c: 0.35,
            weight: 1.0,
            priority: 0,
        },
    ])
    .expect("the answered v1 biome set is valid")
}

fn continentalness_at(
    tile: &TileData,
    block_x: i32,
    block_z: i32,
    tile_x: i32,
    tile_z: i32,
) -> f32 {
    let cell_x = crate::lattice::floor_div(block_x, MACRO_CELL) - tile_x * TILE_CELLS + 1;
    let cell_z = crate::lattice::floor_div(block_z, MACRO_CELL) - tile_z * TILE_CELLS + 1;
    let index = (cell_x.clamp(0, LATTICE_SIDE - 1)
        + LATTICE_SIDE * cell_z.clamp(0, LATTICE_SIDE - 1)) as usize;
    tile.continentalness[index]
}
