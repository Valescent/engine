//! Seed-native LOD sections: coordinate math, coarse generation, and meshing.
//!
//! A section always stores `32³` samples at a spacing of `2^level` blocks.
//! Level 0 is the existing chunk; levels 1-4 cover eight times the volume per
//! step. Sections are derived rendering data and are never saved.

#![allow(dead_code)] // The renderer and streaming path consume this slice next.

use std::collections::HashMap;

use glam::IVec3;

use crate::block::Block;
use crate::generation::{Biome, Generator, SEA_LEVEL, WORLD_HEIGHT, surface_block};
use crate::light::LightField;
use crate::mesh::{
    BorderBlocks, ChunkSnapshot, MeshData, build_section_mesh, build_section_water_mesh,
};
use crate::world::{CHUNK_SIZE, Chunk, ChunkCoord};

/// Samples per section axis.
pub(crate) const SECTION_SIZE: i32 = CHUNK_SIZE;

/// Highest hierarchy level in the first slice.
pub(crate) const MAX_LEVEL: u8 = 4;

/// Full L0 chunks whose final blocks override the seed in a coarse rebuild.
pub(crate) type SectionEdits = HashMap<ChunkCoord, Chunk>;

/// Identity of one section: a level and a coordinate in that level's grid.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct SectionKey {
    pub(crate) level: u8,
    pub(crate) coord: IVec3,
}

impl SectionKey {
    pub(crate) const fn new(level: u8, coord: IVec3) -> Self {
        Self { level, coord }
    }

    /// Blocks per sample on each axis.
    pub(crate) const fn scale(self) -> i32 {
        1_i32 << self.level
    }

    /// Blocks per section axis.
    pub(crate) const fn extent(self) -> i32 {
        SECTION_SIZE * self.scale()
    }

    /// Section at `level` that contains `world`.
    pub(crate) fn from_world(level: u8, world: IVec3) -> Self {
        let extent = Self::new(level, IVec3::ZERO).extent();
        Self {
            level,
            coord: world.div_euclid(IVec3::splat(extent)),
        }
    }

    /// Minimum-corner world coordinate.
    pub(crate) fn origin(self) -> IVec3 {
        self.coord * self.extent()
    }

    /// Parent section, one level coarser.
    pub(crate) fn parent(self) -> Option<Self> {
        (self.level < MAX_LEVEL).then(|| Self {
            level: self.level + 1,
            coord: self.coord.div_euclid(IVec3::splat(2)),
        })
    }

    /// One of eight children, one level finer.
    pub(crate) fn child(self, index: usize) -> Option<Self> {
        if self.level == 0 || index >= 8 {
            return None;
        }
        let offset = IVec3::new(
            i32::try_from(index & 1).expect("child index fits"),
            i32::try_from((index >> 1) & 1).expect("child index fits"),
            i32::try_from((index >> 2) & 1).expect("child index fits"),
        );
        Some(Self {
            level: self.level - 1,
            coord: self.coord * 2 + offset,
        })
    }

    /// World coordinate of a sample.
    pub(crate) fn sample_origin(self, local: IVec3) -> IVec3 {
        self.origin() + local * self.scale()
    }
}

/// Levels whose sections can receive an L0 edit.
const EDIT_LEVELS: std::ops::RangeInclusive<u8> = 1..=MAX_LEVEL;

/// Section keys at [`EDIT_LEVELS`] whose volume overlaps the inclusive bounds.
///
/// Every section whose extent intersects the box is returned, so an arbitrarily
/// large box may overlap any number of sections per axis. Only a box no larger
/// than one chunk (see [`overlapping_sections_for_chunk`]) can rely on the
/// tighter bound of at most two sections per axis per level, because section
/// extents are multiples of the chunk size.
pub(crate) fn overlapping_sections(min: IVec3, max: IVec3) -> Vec<SectionKey> {
    let mut keys = Vec::new();
    for level in EDIT_LEVELS {
        let extent = IVec3::splat(SectionKey::new(level, IVec3::ZERO).extent());
        let min_coord = min.div_euclid(extent);
        let max_coord = max.div_euclid(extent);
        for x in min_coord.x..=max_coord.x {
            for y in min_coord.y..=max_coord.y {
                for z in min_coord.z..=max_coord.z {
                    keys.push(SectionKey::new(level, IVec3::new(x, y, z)));
                }
            }
        }
    }
    keys
}

/// Section keys at [`EDIT_LEVELS`] whose volume overlaps one L0 chunk.
pub(crate) fn overlapping_sections_for_chunk(chunk: ChunkCoord) -> Vec<SectionKey> {
    let min = chunk.world_origin();
    let max = min + IVec3::splat(CHUNK_SIZE - 1);
    overlapping_sections(min, max)
}

/// Conservative visible bounds of one section, in world metres.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SectionBounds {
    /// Lowest solid terrain top, used as the horizon occluder height.
    pub(crate) min_top: f32,
    /// Highest visible top, including water, used as the candidate height.
    pub(crate) max_top: f32,
}

/// One generated `32³` sample grid.
#[derive(Debug)]
pub(crate) struct Section {
    key: SectionKey,
    samples: Chunk,
    /// Lowest sample layer that may hold a non-air block; `SECTION_SIZE` until
    /// the first write.
    sample_y_min: i32,
    /// Highest sample layer that may hold a non-air block; `-1` until the
    /// first write.
    sample_y_max: i32,
}

impl Section {
    pub(crate) fn new(key: SectionKey) -> Self {
        Self {
            key,
            samples: Chunk::default(),
            sample_y_min: SECTION_SIZE,
            sample_y_max: -1,
        }
    }

    pub(crate) fn key(&self) -> SectionKey {
        self.key
    }

    pub(crate) fn sample(&self, local: IVec3) -> Block {
        self.samples.block(local)
    }

    pub(crate) fn set_sample(&mut self, local: IVec3, block: Block) {
        self.note_sample_y(local.y, block);
        self.samples.set_block(local, block);
    }

    /// Fills one column's sample layers in bulk.
    ///
    /// The range is clamped to the grid. This is the generation fast path for
    /// columns without an overlapping edit.
    fn fill_sample_column(&mut self, sx: i32, sz: i32, y_start: i32, y_end: i32, block: Block) {
        let start = y_start.max(0);
        let end = y_end.min(SECTION_SIZE - 1);
        if start > end || block == Block::Air {
            return;
        }
        self.samples.fill_column(sx, sz, start, end, block);
        self.sample_y_min = self.sample_y_min.min(start);
        self.sample_y_max = self.sample_y_max.max(end);
    }

    fn note_sample_y(&mut self, y: i32, block: Block) {
        if block != Block::Air {
            self.sample_y_min = self.sample_y_min.min(y);
            self.sample_y_max = self.sample_y_max.max(y);
        }
    }

    /// Conservative sample layers the mesher has to scan.
    ///
    /// One layer above and below the non-air range covers light/AO sampling,
    /// and the result is clamped to the grid.
    fn mesh_y_range(&self) -> std::ops::RangeInclusive<i32> {
        (self.sample_y_min - 1).max(0)..=(self.sample_y_max + 1).min(SECTION_SIZE - 1)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Lowest solid terrain top and highest visible surface in the section, in
    /// world metres.
    ///
    /// Water is visible but transparent, so it raises `max_top` without
    /// lowering the occluder height. All-air columns are ignored.
    #[allow(clippy::cast_precision_loss)]
    pub(crate) fn bounds(&self) -> Option<SectionBounds> {
        let scale = self.key.scale();
        let origin = self.key.origin();
        let mut min_top = f32::INFINITY;
        let mut max_top = f32::NEG_INFINITY;
        for x in 0..SECTION_SIZE {
            for z in 0..SECTION_SIZE {
                let mut visible = f32::NEG_INFINITY;
                let mut terrain = f32::NEG_INFINITY;
                for y in (0..SECTION_SIZE).rev() {
                    let block = self.samples.block(IVec3::new(x, y, z));
                    if block == Block::Air {
                        continue;
                    }
                    let top = (origin.y + (y + 1) * scale) as f32;
                    if visible == f32::NEG_INFINITY {
                        visible = top;
                    }
                    if block.is_solid() {
                        terrain = top;
                    }
                    if visible != f32::NEG_INFINITY && terrain != f32::NEG_INFINITY {
                        break;
                    }
                }
                max_top = max_top.max(visible);
                if terrain != f32::NEG_INFINITY {
                    min_top = min_top.min(terrain);
                }
            }
        }
        max_top
            .is_finite()
            .then_some(SectionBounds { min_top, max_top })
    }

    /// A snapshot of the sample grid for the mesher.
    pub(crate) fn snapshot(&self) -> ChunkSnapshot {
        ChunkSnapshot::from_chunk(&self.samples)
    }

    /// Coarse BFS sky light over the sample grid and its one-cell halo.
    ///
    /// Open-sky samples start at full light and spread into caves and recesses;
    /// water carries light without attenuation. The halo reuses the nearest edge
    /// sample, with open sky above the edge top, because a section has no
    /// neighbouring section data.
    pub(crate) fn light(&self) -> LightField {
        LightField::coarse(&self.samples)
    }

    /// Meshes the sample grid at the section's spacing.
    #[allow(clippy::cast_precision_loss)]
    pub(crate) fn mesh(&self, light: Option<&LightField>) -> MeshData {
        let snapshot = self.snapshot();
        build_section_mesh(
            &snapshot,
            &BorderBlocks::default(),
            light,
            self.key.scale() as f32,
            self.mesh_y_range(),
        )
    }

    /// Meshes the water samples at the section's spacing.
    #[allow(clippy::cast_precision_loss)]
    pub(crate) fn water_mesh(&self, light: Option<&LightField>) -> MeshData {
        let snapshot = self.snapshot();
        build_section_water_mesh(
            &snapshot,
            &BorderBlocks::default(),
            light,
            self.key.scale() as f32,
            self.mesh_y_range(),
        )
    }
}

/// Generates one section from the seed and the modified-chunk overlay.
///
/// Columns with no overlapping edit are filled in bulk: the top four depth
/// samples use [`surface_block`] and the rest of the solid run is stone, which
/// matches the per-sample rule for every biome. A column that may carry an
/// edit keeps the original per-sample path so partially edited columns stay
/// bit-identical.
pub(crate) fn generate_section(
    generator: &Generator,
    key: SectionKey,
    edits: &SectionEdits,
) -> Section {
    let scale = key.scale();
    let origin = key.origin();
    let extent = key.extent();
    let probes = probe_offsets(scale);
    let sea_sample = (SEA_LEVEL - origin.y).div_euclid(scale);
    let edited = edited_columns(edits, origin, scale, extent);
    let mut section = Section::new(key);

    for sx in 0..SECTION_SIZE {
        for sz in 0..SECTION_SIZE {
            let (top_height, rep_x, rep_z) =
                footprint_top(generator, origin, scale, sx, sz, &probes);
            let biome = generator.biome(rep_x, rep_z);
            let top_sample = (top_height - origin.y).div_euclid(scale);
            let column = usize::try_from(sx * SECTION_SIZE + sz).expect("column index fits");

            if edited.as_ref().is_some_and(|mask| mask[column]) {
                for sy in 0..SECTION_SIZE {
                    set_generated_sample(
                        &mut section,
                        generator,
                        edits,
                        origin,
                        scale,
                        IVec3::new(sx, sy, sz),
                        top_sample,
                        sea_sample,
                        biome,
                    );
                }
                continue;
            }

            if top_sample >= 0 {
                // Every block below the top four depth layers is stone in
                // every biome, and `surface_block` returns stone there too.
                let surface_start = (top_sample - (SURFACE_DEPTH - 1)).max(0);
                let solid_end = top_sample.min(SECTION_SIZE - 1);
                for sy in surface_start..=solid_end {
                    section.set_sample(
                        IVec3::new(sx, sy, sz),
                        surface_block(biome, top_sample - sy),
                    );
                }
                if surface_start > 0 {
                    section.fill_sample_column(sx, sz, 0, surface_start - 1, Block::Stone);
                }
            }

            let water_start = (top_sample + 1).max(0);
            let water_end = sea_sample.min(SECTION_SIZE - 1);
            if water_start <= water_end {
                section.fill_sample_column(sx, sz, water_start, water_end, Block::Water);
            }
        }
    }

    section
}

/// Samples above the deepest non-stone surface layer.
///
/// Plains keeps grass and three dirt layers, Hills one dirt layer, and Desert
/// three sand layers; four covers all of them.
const SURFACE_DEPTH: i32 = 4;

/// The original per-sample generation path for a column that may carry an edit.
#[allow(clippy::too_many_arguments)]
fn set_generated_sample(
    section: &mut Section,
    generator: &Generator,
    edits: &SectionEdits,
    origin: IVec3,
    scale: i32,
    local: IVec3,
    top_sample: i32,
    sea_sample: i32,
    biome: Biome,
) {
    let sy = local.y;
    let base = origin + local * scale;
    let block = if sy <= top_sample {
        surface_block(biome, top_sample - sy)
    } else if sy <= sea_sample {
        Block::Water
    } else {
        Block::Air
    };
    let block = if edits.is_empty() || !overlaps_edits(edits, base, scale) {
        block
    } else {
        edited_sample(generator, edits, base, scale)
    };
    if block != Block::Air {
        section.set_sample(local, block);
    }
}

/// Marks sample columns whose XZ footprint touches an edited chunk.
///
/// Returns `None` when there are no edits, so the common case does no lookup
/// work per column. A marked column goes through the per-sample edited path,
/// which re-checks the exact Y overlap.
fn edited_columns(
    edits: &SectionEdits,
    origin: IVec3,
    scale: i32,
    extent: i32,
) -> Option<Vec<bool>> {
    if edits.is_empty() {
        return None;
    }
    let mut mask = vec![false; (SECTION_SIZE * SECTION_SIZE) as usize];
    let section_max = origin + IVec3::splat(extent - 1);
    for chunk in edits.keys() {
        let min = chunk.world_origin();
        let max = min + IVec3::splat(CHUNK_SIZE - 1);
        if !min.cmple(section_max).all() || !origin.cmple(max).all() {
            continue;
        }
        let columns_x = (min.x - origin.x)
            .div_euclid(scale)
            .clamp(0, SECTION_SIZE - 1)
            ..=(max.x - origin.x)
                .div_euclid(scale)
                .clamp(0, SECTION_SIZE - 1);
        let columns_z = (min.z - origin.z)
            .div_euclid(scale)
            .clamp(0, SECTION_SIZE - 1)
            ..=(max.z - origin.z)
                .div_euclid(scale)
                .clamp(0, SECTION_SIZE - 1);
        for sx in columns_x {
            for sz in columns_z.clone() {
                let column = usize::try_from(sx * SECTION_SIZE + sz).expect("column index fits");
                mask[column] = true;
            }
        }
    }
    Some(mask)
}

/// One L0 block from the coarse seed rule: no caves and no trees.
pub(crate) fn seed_block(generator: &Generator, world: IVec3) -> Block {
    if !(0..WORLD_HEIGHT).contains(&world.y) {
        return Block::Air;
    }
    let height = generator.height(world.x, world.z);
    if world.y <= height {
        surface_block(generator.biome(world.x, world.z), height - world.y)
    } else if world.y <= SEA_LEVEL {
        Block::Water
    } else {
        Block::Air
    }
}

fn probe_offsets(scale: i32) -> Vec<i32> {
    if scale <= 4 {
        (0..scale).collect()
    } else {
        vec![0, scale / 2, scale - 1]
    }
}

fn footprint_top(
    generator: &Generator,
    origin: IVec3,
    scale: i32,
    sx: i32,
    sz: i32,
    probes: &[i32],
) -> (i32, i32, i32) {
    let mut top = i32::MIN;
    let mut rep = (origin.x + sx * scale, origin.z + sz * scale);
    for &dx in probes {
        for &dz in probes {
            let x = origin.x + sx * scale + dx;
            let z = origin.z + sz * scale + dz;
            let height = generator.height(x, z);
            if height > top || (height == top && (x, z) < rep) {
                top = height;
                rep = (x, z);
            }
        }
    }
    (top, rep.0, rep.1)
}

fn overlaps_edits(edits: &SectionEdits, base: IVec3, scale: i32) -> bool {
    let max = base + IVec3::splat(scale - 1);
    let min_chunk = base.div_euclid(IVec3::splat(CHUNK_SIZE));
    let max_chunk = max.div_euclid(IVec3::splat(CHUNK_SIZE));
    for x in min_chunk.x..=max_chunk.x {
        for y in min_chunk.y..=max_chunk.y {
            for z in min_chunk.z..=max_chunk.z {
                if edits.contains_key(&ChunkCoord::new(x, y, z)) {
                    return true;
                }
            }
        }
    }
    false
}

fn edited_sample(generator: &Generator, edits: &SectionEdits, base: IVec3, scale: i32) -> Block {
    let mut solid = None;
    let mut water = false;
    for dy in 0..scale {
        for dx in 0..scale {
            for dz in 0..scale {
                let world = base + IVec3::new(dx, dy, dz);
                let (chunk, local) = ChunkCoord::from_world(world);
                let block = edits
                    .get(&chunk)
                    .map_or_else(|| seed_block(generator, world), |data| data.block(local));
                if block.is_solid() {
                    solid = Some(block);
                } else if block == Block::Water {
                    water = true;
                }
            }
        }
    }
    solid.unwrap_or(if water { Block::Water } else { Block::Air })
}

#[cfg(test)]
mod tests;
