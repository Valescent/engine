//! The resident demo patch: column storage, block access, light/mesh
//! installation, and LOD selection.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use glam::{IVec2, IVec3, Vec3};
use tracing::{debug, warn};

use crate::content;
use crate::description;
use crate::light2;
use crate::mesher2;
use crate::padded;
use crate::player;
use crate::section;

#[cfg(test)]
use super::build::{DemoEdit, demo_rebuild};
use super::build::{DemoPatch, DemoRebuild};
use super::content::{demo_materials, solid_table};
use super::lod::{coarse_bands, demo_coarse_regions_masked, demo_lod_sets};
use super::{COLUMN_SECTIONS, TERRAIN_DEMO_FINE_MARGIN, TERRAIN_DEMO_FINE_RADIUS};

/// One meshed section of the demo patch.
#[derive(Clone)]
pub(crate) struct DemoSection {
    /// Section-aligned world block origin.
    pub(crate) origin: [i32; 3],
    /// World blocks per cell (`1` fine, `2` LOD1).
    pub(crate) scale: u32,
    pub(crate) mesh: mesher2::SectionMesh,
}

/// Columns drawn by each clipped LOD1 base.
pub(crate) type PartialMasks = BTreeMap<(i32, i32), BTreeSet<(i32, i32)>>;

/// Clipped LOD1 sections and the masks they were built from.
pub(crate) type PartialBuild = (Vec<DemoSection>, PartialMasks);

/// The static new-pipeline terrain patch, built once before the window opens.
#[allow(clippy::struct_excessive_bools)]
pub(crate) struct TerrainDemo {
    pub(crate) seed: u64,
    /// Column the patch is centred on, `Streaming` column coordinates.
    pub(crate) origin_column: IVec2,
    /// Chebyshev radius of the resident column ring.
    pub(crate) view_radius: i32,
    /// Chebyshev radius meshed at LOD0; beyond it, complete 2x2 groups are LOD1.
    pub(crate) fine_radius: i32,
    /// Every generated column, kept for collision and relighting.
    pub(crate) columns: BTreeMap<(i32, i32), Arc<[section::Section; COLUMN_SECTIONS]>>,
    /// Per-column light, recomputed after an edit (`L1.2`).
    pub(crate) light: BTreeMap<(i32, i32), light2::ColumnLight>,
    /// LOD0 sections, keyed by the column they belong to.
    pub(crate) fine: BTreeMap<(i32, i32), Vec<DemoSection>>,
    /// LOD1 sections for every complete 2x2 region in the middle band.
    pub(crate) coarse: Vec<DemoSection>,
    /// Columns drawn by each clipped LOD1 base in `coarse`.
    pub(crate) partial_columns: BTreeMap<(i32, i32), BTreeSet<(i32, i32)>>,
    /// LOD2 sections for every complete 4x4 region beyond the LOD1 band.
    pub(crate) far: Vec<DemoSection>,
    /// A deferred LOD2 rebuild is due after an edit. LOD2 is expensive, so it
    /// waits for the throttled whole-ring coarse job instead of rebuilding in
    /// the edit path.
    pub(crate) far_dirty: bool,
    /// Clipped LOD1 sections must be remeshed even if their masks are unchanged.
    /// Set by an edit so the next coarse job refreshes the rim geometry.
    pub(crate) partial_dirty: bool,
    /// Mesher and light material tables, resolved once from the pack.
    pub(crate) materials: mesher2::MaterialProps,
    /// The content pack, for resolving stable names on save (`A5.1`).
    pub(crate) pack: content::ContentPack,
    /// Generated baselines for sections that were loaded or edited (`J4.1`).
    pub(crate) baselines: BTreeMap<section::SectionKey, [u8; 32]>,
    /// Columns edited since their last queued save. Never evicted.
    pub(crate) unsaved: BTreeSet<(i32, i32)>,
    /// Next committed edit revision (`J1.2`).
    pub(crate) revision: u64,
    /// A world that loaded an unknown name is read-only (`A16`).
    pub(crate) read_only: bool,
    /// Air id, for edits.
    pub(crate) air: section::BlockId,
    /// `solid[id]` mirrors the content pack's collision flag.
    pub(crate) solid: Vec<bool>,
    /// Feet position on the patch's surface at the centre column. Valid once
    /// the centre column has light (`spawn_ready`).
    pub(crate) spawn: Vec3,
    /// Whether the centre column has been generated, lit, and measured.
    pub(crate) spawn_ready: bool,
    /// Active sections submitted to the renderer (fine and coarse by distance).
    pub(crate) sections: Vec<DemoSection>,
    /// Column the current LOD selection was made for.
    pub(crate) lod_column: (i32, i32),
    pub(crate) quads: usize,
    /// World block range covered by emitting sections, for the startup log.
    pub(crate) min_y: i32,
    pub(crate) max_y: i32,
    /// One solve shared by every worker's `Terrain` (`A7.1`).
    pub(crate) world_hydro: Arc<crate::hydro::WorldHydro>,
}

impl crate::player::BlockSource for TerrainDemo {
    fn solid_blocks_in(&self, min: IVec3, max: IVec3) -> Vec<IVec3> {
        let mut blocks = Vec::new();
        if min.cmpgt(max).any() {
            return blocks;
        }
        for x in min.x..=max.x {
            for y in min.y..=max.y {
                for z in min.z..=max.z {
                    let cell = IVec3::new(x, y, z);
                    if self.is_solid(cell) {
                        blocks.push(cell);
                    }
                }
            }
        }
        blocks
    }
}

impl TerrainDemo {
    /// An empty ring centred on `column`, ready to stream columns into.
    #[allow(clippy::cast_precision_loss)]
    pub(crate) fn empty(
        seed: u64,
        column: IVec2,
        view_radius: i32,
        world_hydro: Arc<crate::hydro::WorldHydro>,
    ) -> Self {
        let pack = content::v1_default();
        let solid = solid_table(&pack);
        let air = pack.id("core:air").unwrap_or(0);
        let section_blocks = i32::try_from(description::SECTION_SIZE).unwrap_or(32);
        Self {
            seed,
            origin_column: column,
            view_radius,
            fine_radius: TERRAIN_DEMO_FINE_RADIUS,
            columns: BTreeMap::new(),
            light: BTreeMap::new(),
            fine: BTreeMap::new(),
            coarse: Vec::new(),
            partial_columns: BTreeMap::new(),
            far: Vec::new(),
            far_dirty: false,
            partial_dirty: true,
            materials: demo_materials(),
            pack,
            baselines: BTreeMap::new(),
            unsaved: BTreeSet::new(),
            revision: 0,
            read_only: false,
            air,
            solid,
            spawn: Vec3::new(
                (column.x * section_blocks) as f32 + 16.5,
                64.0,
                (column.y * section_blocks) as f32 + 16.5,
            ),
            spawn_ready: false,
            sections: Vec::new(),
            lod_column: (column.x, column.y),
            quads: 0,
            min_y: 0,
            max_y: 0,
            world_hydro,
        }
    }

    /// The block at a world cell, or `None` outside the patch.
    pub(crate) fn block_at(&self, cell: IVec3) -> Option<section::BlockId> {
        if cell.y < 0 {
            return None;
        }
        let size = i32::try_from(description::SECTION_SIZE).ok()?;
        let column = (cell.x.div_euclid(size), cell.z.div_euclid(size));
        let stack = self.columns.get(&column)?;
        let section = stack.get(usize::try_from(cell.y.div_euclid(size)).ok()?)?;
        Some(section.get(
            u32::try_from(cell.x.rem_euclid(size)).ok()?,
            u32::try_from(cell.y.rem_euclid(size)).ok()?,
            u32::try_from(cell.z.rem_euclid(size)).ok()?,
        ))
    }

    /// Whether a cell is solid for collision and raycasts (`A5.2`).
    pub(crate) fn is_solid(&self, cell: IVec3) -> bool {
        self.block_at(cell)
            .is_some_and(|id| self.solid.get(usize::from(id)).copied().unwrap_or(false))
    }

    /// Whether a cell is inside the patch and empty.
    pub(crate) fn is_empty(&self, cell: IVec3) -> bool {
        matches!(self.block_at(cell), Some(id) if id == self.air)
    }

    /// Whether a column has light, which is the readiness gate for physics.
    pub(crate) fn column_ready(&self, column: (i32, i32)) -> bool {
        self.light.contains_key(&column)
    }

    /// Whether a player AABB at `position` overlaps only ready columns.
    #[allow(clippy::cast_possible_truncation)]
    pub(crate) fn aabb_ready(&self, position: Vec3) -> bool {
        let half = player::PLAYER_WIDTH / 2.0;
        let min = IVec2::new(
            (position.x - half).floor() as i32,
            (position.z - half).floor() as i32,
        );
        let max = IVec2::new(
            (position.x + half).floor() as i32,
            (position.z + half).floor() as i32,
        );
        let size = i32::try_from(description::SECTION_SIZE).unwrap_or(32);
        for x in min.x.div_euclid(size)..=max.x.div_euclid(size) {
            for z in min.y.div_euclid(size)..=max.y.div_euclid(size) {
                if !self.column_ready((x, z)) {
                    return false;
                }
            }
        }
        true
    }

    /// The generated baseline for a section, computed once (`J4.1`).
    ///
    /// Callers compute this **before** mutating the section, or it would hash
    /// the edited blocks instead of the first generation.
    pub(crate) fn baseline_for(&mut self, key: section::SectionKey) -> [u8; 32] {
        if let Some(hash) = self.baselines.get(&key) {
            return *hash;
        }
        let hash = self
            .columns
            .get(&(key.column[0], key.column[1]))
            .and_then(|stack| stack.get(usize::from(key.section)))
            .map_or([0; 32], section::baseline_hash);
        self.baselines.insert(key, hash);
        hash
    }

    /// Inserts a generated column. Replacing a column also drops its light and
    /// fine mesh, so the next build pass recomputes them.
    pub(crate) fn insert_column(
        &mut self,
        column: (i32, i32),
        stack: [section::Section; COLUMN_SECTIONS],
    ) {
        self.invalidate_partial_near(column);
        self.columns.insert(column, Arc::new(stack));
        self.light.remove(&column);
        self.fine.remove(&column);
    }

    /// Drops a column and everything derived from it.
    pub(crate) fn remove_column(&mut self, column: (i32, i32)) {
        self.invalidate_partial_near(column);
        self.columns.remove(&column);
        self.light.remove(&column);
        self.fine.remove(&column);
    }

    /// A clipped region culls against its neighbours, so a column within one
    /// expanded base (`2` columns) invalidates the cached clipped meshes.
    pub(crate) fn invalidate_partial_near(&mut self, column: (i32, i32)) {
        if self
            .partial_columns
            .keys()
            .any(|&(x, z)| (column.0 - x).abs() <= 2 && (column.1 - z).abs() <= 2)
        {
            self.partial_dirty = true;
        }
    }

    /// Installs computed light for the columns a build job returned.
    pub(crate) fn apply_light(&mut self, light: BTreeMap<(i32, i32), light2::ColumnLight>) {
        for (column, value) in light {
            if self.columns.contains_key(&column) {
                self.light.insert(column, value);
            }
        }
    }

    /// Installs fine-section replacements, keeping each column's list sorted.
    pub(crate) fn apply_patches(&mut self, patches: BTreeMap<(i32, i32), Vec<DemoPatch>>) {
        for (column, patches) in patches {
            if !self.columns.contains_key(&column) {
                continue;
            }
            let sections = self.fine.entry(column).or_default();
            for patch in patches {
                match patch.mesh {
                    Some(mesh) => {
                        let section = DemoSection {
                            origin: patch.origin,
                            scale: 1,
                            mesh,
                        };
                        match sections.binary_search_by_key(&patch.origin, |s| s.origin) {
                            Ok(index) => sections[index] = section,
                            Err(index) => sections.insert(index, section),
                        }
                    }
                    None => {
                        if let Ok(index) =
                            sections.binary_search_by_key(&patch.origin, |s| s.origin)
                        {
                            sections.remove(index);
                        }
                    }
                }
            }
        }
    }

    /// Measures the surface of the centre column and remembers the spawn.
    #[allow(clippy::cast_precision_loss)]
    pub(crate) fn settle_spawn(&mut self) {
        let column = (self.origin_column.x, self.origin_column.y);
        let Some(stack) = self.columns.get(&column).cloned() else {
            return;
        };
        let section_blocks = i32::try_from(description::SECTION_SIZE).unwrap_or(32);
        let bx = self.origin_column.x * section_blocks + section_blocks / 2;
        let bz = self.origin_column.y * section_blocks + section_blocks / 2;
        let height = i32::try_from(description::WORLD_HEIGHT).unwrap_or(384);
        let top = (0..height).rev().find(|&y| {
            let sy = usize::try_from(y.div_euclid(section_blocks)).unwrap_or(0);
            let Some(section) = stack.get(sy) else {
                return false;
            };
            let id = section.get(
                u32::try_from(bx.rem_euclid(section_blocks)).unwrap_or(0),
                u32::try_from(y.rem_euclid(section_blocks)).unwrap_or(0),
                u32::try_from(bz.rem_euclid(section_blocks)).unwrap_or(0),
            );
            self.solid.get(usize::from(id)).copied().unwrap_or(false)
        });
        self.spawn = Vec3::new(
            bx as f32 + 0.5,
            top.map_or(description::SEA, |y| y + 1) as f32,
            bz as f32 + 0.5,
        );
        self.spawn_ready = true;
    }

    /// Layers saved sections over the resident columns and refreshes light,
    /// fine meshes, and coarse regions. Used after `world-repair`.
    pub(crate) fn apply_saved(&mut self, saved: &[section::SavedSection]) -> bool {
        let mut changed = false;
        for record in saved {
            let key = record.key;
            let Some(stack) = self.columns.get_mut(&(key.column[0], key.column[1])) else {
                continue;
            };
            let Some(slot) = Arc::make_mut(stack).get_mut(usize::from(key.section)) else {
                continue;
            };
            let generated = section::baseline_hash(slot);
            if generated != record.baseline_hash {
                warn!(
                    column = ?key.column,
                    section = key.section,
                    "saved terrain section does not match this generator; ignored"
                );
                continue;
            }
            if !record.unknown_names.is_empty() {
                self.read_only = true;
            }
            *slot = record.section.clone();
            self.baselines.insert(key, record.baseline_hash);
            changed = true;
        }
        if changed {
            self.light = demo_column_light(&self.columns);
            self.fine = demo_fine_all(&self.columns, &self.light, &self.materials);
            let sets = demo_lod_sets(
                &self.columns,
                &self.materials,
                &self.pack,
                self.lod_column,
                self.view_radius,
                true,
            );
            self.coarse = sets.coarse;
            self.coarse.extend(sets.partial_sections);
            self.coarse.sort_by_key(|section| section.origin);
            self.partial_columns = sets.partial;
            self.far = sets.far;
            self.far_dirty = false;
            self.partial_dirty = false;
            self.select_lod(self.lod_column);
        }
        changed
    }

    /// Writes one block into the patch. Returns whether anything changed.
    ///
    /// Light and meshes are rebuilt separately: [`demo_rebuild`] does that off the
    /// main thread, so a click never stalls the frame.
    pub(crate) fn set_block(&mut self, cell: IVec3, id: section::BlockId) -> bool {
        if cell.y < 0 {
            return false;
        }
        let size = i32::try_from(description::SECTION_SIZE).unwrap_or(32);
        let key = (cell.x.div_euclid(size), cell.z.div_euclid(size));
        let Some(stack) = self.columns.get_mut(&key) else {
            return false;
        };
        let Ok(sy) = usize::try_from(cell.y.div_euclid(size)) else {
            return false;
        };
        let stack = Arc::make_mut(stack);
        let Some(section) = stack.get_mut(sy) else {
            return false;
        };
        let (x, y, z) = (
            u32::try_from(cell.x.rem_euclid(size)).unwrap_or(0),
            u32::try_from(cell.y.rem_euclid(size)).unwrap_or(0),
            u32::try_from(cell.z.rem_euclid(size)).unwrap_or(0),
        );
        if section.get(x, y, z) == id {
            return false;
        }
        section.set(x, y, z, id);
        true
    }

    /// Installs a finished background rebuild.
    pub(crate) fn apply_rebuild(&mut self, rebuild: DemoRebuild) {
        self.light = rebuild.light;
        self.apply_patches(rebuild.patches);
        // Coarse regions are disposable: refresh the ones an edit can reach.
        // LOD2 reads 4x4 column groups and is too expensive for the edit path,
        // so it is rebuilt by the next throttled whole-ring job.
        self.refresh_coarse_around(&rebuild.columns);
        self.far_dirty = true;
        self.partial_dirty = true;
        self.select_lod(self.lod_column);
    }

    /// Rebuilds every coarse region whose `2x2` column group touches `columns`.
    pub(crate) fn refresh_coarse_around(&mut self, columns: &BTreeSet<(i32, i32)>) {
        let mut regions = BTreeSet::new();
        for &(cx, cz) in columns {
            let bx = cx - cx.rem_euclid(2);
            let bz = cz - cz.rem_euclid(2);
            for dx in -1..=1_i32 {
                for dz in -1..=1_i32 {
                    regions.insert((bx + 2 * dx, bz + 2 * dz));
                }
            }
        }
        if regions.is_empty() {
            return;
        }
        // A rim or LOD0-boundary region is clipped, so use the mask the current
        // band selection assigns to each base rather than requiring all four.
        // Clipped regions are keyed by their anchor column, so a group that
        // touches a changed column but is anchored elsewhere is added too.
        let bands = coarse_bands(&self.columns, self.lod_column, self.view_radius);
        let mut masks: BTreeMap<(i32, i32), BTreeSet<(i32, i32)>> = BTreeMap::new();
        for &region in &regions {
            let mask = bands.partial.get(&region).cloned().unwrap_or_else(|| {
                BTreeSet::from([
                    region,
                    (region.0 + 1, region.1),
                    (region.0, region.1 + 1),
                    (region.0 + 1, region.1 + 1),
                ])
            });
            masks.insert(region, mask);
        }
        let touched = |point: (i32, i32)| {
            columns
                .iter()
                .any(|&(cx, cz)| (point.0 - cx).abs() <= 3 && (point.1 - cz).abs() <= 3)
        };
        for (&anchor, mask) in &bands.partial {
            if touched(anchor) || mask.iter().any(|&column| touched(column)) {
                masks.insert(anchor, mask.clone());
            }
        }
        let rebuilt =
            demo_coarse_regions_masked(&self.columns, &self.materials, &self.pack, &masks);
        self.coarse.retain(|section| {
            !masks.contains_key(&(section.origin[0] / 32, section.origin[2] / 32))
        });
        for (&origin, sections) in &rebuilt {
            if masks.contains_key(&origin) {
                self.coarse.extend(sections.iter().cloned());
            }
        }
        self.coarse.sort_by_key(|section| section.origin);
        self.partial_columns
            .retain(|anchor, _| !masks.contains_key(anchor));
        for (&anchor, mask) in &bands.partial {
            if masks.contains_key(&anchor) {
                self.partial_columns.insert(anchor, mask.clone());
            }
        }
    }

    /// Chooses LOD0 near the player, LOD1 for the middle band, and LOD2 far.
    ///
    /// Fine meshes are kept one ring beyond the fine radius as a transition
    /// buffer. An LOD1 region is active only when all of its drawn columns are
    /// outside that buffer and it is not covered by an active LOD2 region, so
    /// the three bands never overlap. LOD2 regions are complete 4x4 column
    /// squares built entirely beyond [`TERRAIN_DEMO_LOD1_RADIUS`]. The clipped
    /// LOD1 regions in `coarse` cover the positive rim and the LOD0-boundary
    /// leftovers that no complete 2x2 square can own; they follow the same
    /// far/near rules as a complete region, so a clipped region never overlaps
    /// an LOD2 square. A clipped region whose aligned base stays inside the
    /// LOD0 band is anchored at a drawn column instead, so its origin never
    /// collides with a fine section.
    pub(crate) fn select_lod(&mut self, player_column: (i32, i32)) {
        self.lod_column = player_column;
        let cutoff = self.fine_radius + TERRAIN_DEMO_FINE_MARGIN;
        let ring = |column: (i32, i32)| {
            (column.0 - player_column.0)
                .abs()
                .max((column.1 - player_column.1).abs())
        };
        let mut active: Vec<DemoSection> = self
            .fine
            .iter()
            .filter(|&(&column, _)| ring(column) <= cutoff)
            .flat_map(|(_, sections)| sections.iter().cloned())
            .collect();

        // Far regions still draw while the player walks toward them; the next
        // throttled rebuild reclassifies them. All 16 columns must be clear of
        // the fine buffer, which keeps LOD2 off the LOD0 transition.
        let far_regions: BTreeSet<(i32, i32)> = self
            .far
            .iter()
            .map(|section| (section.origin[0] / 32, section.origin[2] / 32))
            .filter(|&(x, z)| (0..4).all(|dx| (0..4).all(|dz| ring((x + dx, z + dz)) > cutoff)))
            .collect();
        for section in &self.far {
            if far_regions.contains(&(section.origin[0] / 32, section.origin[2] / 32)) {
                active.push(section.clone());
            }
        }

        for section in &self.coarse {
            let region = (section.origin[0] / 32, section.origin[2] / 32);
            let near = self.partial_columns.get(&region).map_or_else(
                || {
                    [
                        region,
                        (region.0 + 1, region.1),
                        (region.0, region.1 + 1),
                        (region.0 + 1, region.1 + 1),
                    ]
                    .into_iter()
                    .any(|column| ring(column) <= cutoff)
                },
                |columns| columns.iter().any(|&column| ring(column) <= cutoff),
            );
            let far_base = (region.0.div_euclid(4) * 4, region.1.div_euclid(4) * 4);
            if !near && !far_regions.contains(&far_base) {
                active.push(section.clone());
            }
        }
        // The bands are built to be disjoint, but a throttled coarse set can be
        // one or two ring layers stale. Resolve any residual claim on one
        // origin deterministically: the finer scale wins, then the smaller
        // origin. Fine is 1, LOD1 is 2, and LOD2 is 4, so this keeps the most
        // detailed section and never drops a whole band from the upload.
        active.sort_by_key(|section| (section.origin, section.scale));
        active.dedup_by(|later, kept| {
            if later.origin == kept.origin {
                debug!(
                    origin = ?kept.origin,
                    kept_scale = kept.scale,
                    dropped_scale = later.scale,
                    "two LOD bands claimed one section origin; keeping the finer"
                );
                true
            } else {
                false
            }
        });
        active.sort_by_key(|section| section.origin);
        self.sections = active;
        // Drop fine meshes that fell out of the transition buffer. Added
        // columns are rebuilt when the player returns.
        self.fine.retain(|&column, _| ring(column) <= cutoff);
        self.refresh_stats();
    }

    /// Recomputes the renderer-facing counters from the active section set.
    pub(crate) fn refresh_stats(&mut self) {
        self.quads = self
            .sections
            .iter()
            .map(|section| mesher2::quad_count(&section.mesh))
            .sum();
        let section_blocks = i32::try_from(description::SECTION_SIZE).unwrap_or(32);
        self.min_y = self
            .sections
            .iter()
            .map(|section| section.origin[1])
            .min()
            .unwrap_or(i32::MAX);
        self.max_y = self
            .sections
            .iter()
            .map(|section| {
                section.origin[1] + section_blocks * i32::try_from(section.scale).unwrap_or(1)
            })
            .max()
            .unwrap_or(i32::MIN);
    }

    /// Approximate heap footprint of resident blocks, light, and meshes.
    pub(crate) fn footprint_bytes(&self) -> usize {
        let sections: usize = self
            .columns
            .values()
            .flat_map(|stack| stack.iter())
            .map(section_bytes)
            .sum();
        let light: usize = self
            .light
            .values()
            .map(light2::ColumnLight::packed_bytes)
            .sum();
        let meshes: usize = self
            .fine
            .values()
            .flatten()
            .chain(&self.coarse)
            .chain(&self.far)
            .map(mesh_bytes)
            .sum();
        sections + light + meshes
    }

    /// Synchronous rebuild, used by tests and by the startup path.
    #[cfg(test)]
    pub(crate) fn edit(&mut self, cell: IVec3, id: section::BlockId) -> bool {
        if !self.set_block(cell, id) {
            return false;
        }
        let edit = DemoEdit { cell };
        let rebuild = demo_rebuild(&self.columns, &self.light, &self.materials, &[edit]);
        self.apply_rebuild(rebuild);
        true
    }
}

/// Heap bytes held by one section's palette data.
pub(crate) fn section_bytes(section: &section::Section) -> usize {
    std::mem::size_of_val(section.words()) + section.palette().len() * 2
}

/// Heap bytes held by one mesh's quad lists.
pub(crate) fn mesh_bytes(section: &DemoSection) -> usize {
    mesher2::quad_count(&section.mesh) * std::mem::size_of::<mesher2::Quad>()
}

/// The light scratch for one section, with the *real* neighbour light on the
/// horizontal border.
///
/// [`light2::ColumnLight::padded_section`] mirrors its own edge row, which is the
/// `I2.3` world-edge rule. At an internal section boundary that mirror reads the
/// solid edge cell, so a wall on `x = 0` or `x = 31` renders with sky 0. This
/// overlays the adjacent column's light, which is what the caller is expected to
/// do.
pub(crate) fn demo_section_light(
    lit: &BTreeMap<(i32, i32), light2::ColumnLight>,
    sx: i32,
    sz: i32,
    sy: usize,
) -> padded::PaddedLight {
    let Some(column) = lit.get(&(sx, sz)) else {
        return padded::PaddedLight::default();
    };
    let mut padded = column.padded_section(sy);
    let base_y = i32::try_from(sy).unwrap_or(0) * 32;
    let origin_x = sx * 32;
    let origin_z = sz * 32;
    for y in padded::LOCAL_MIN..=padded::LOCAL_MAX {
        let world_y = base_y + y;
        for z in padded::LOCAL_MIN..=padded::LOCAL_MAX {
            for x in padded::LOCAL_MIN..=padded::LOCAL_MAX {
                if x != padded::LOCAL_MIN
                    && x != padded::LOCAL_MAX
                    && z != padded::LOCAL_MIN
                    && z != padded::LOCAL_MAX
                {
                    continue;
                }
                let world_x = origin_x + x;
                let world_z = origin_z + z;
                let key = (world_x.div_euclid(32), world_z.div_euclid(32));
                if key == (sx, sz) {
                    continue;
                }
                let Some(neighbour) = lit.get(&key) else {
                    continue;
                };
                padded.set(
                    x,
                    y,
                    z,
                    neighbour.sky(world_x.rem_euclid(32), world_y, world_z.rem_euclid(32)),
                    neighbour.block(world_x.rem_euclid(32), world_y, world_z.rem_euclid(32)),
                );
            }
        }
    }
    padded
}

/// One `light2` job per column (`L1.2`), using the 8 horizontal neighbours.
pub(crate) fn demo_column_light(
    columns: &BTreeMap<(i32, i32), Arc<[section::Section; COLUMN_SECTIONS]>>,
) -> BTreeMap<(i32, i32), light2::ColumnLight> {
    let table = demo_light_table();
    // One job per column, fanned out across the pool. Output is keyed, so the
    // order threads finish in cannot change the result (`P1.9`).
    let keys: Vec<(i32, i32)> = columns.keys().copied().collect();
    if keys.is_empty() {
        return BTreeMap::new();
    }
    let workers = worker_count().min(keys.len());
    let chunk = keys.len().div_ceil(workers);
    let mut lit = BTreeMap::new();
    std::thread::scope(|scope| {
        let table = &table;
        let handles: Vec<_> = keys
            .chunks(chunk)
            .map(|chunk| {
                scope.spawn(move || {
                    let mut out = Vec::with_capacity(chunk.len());
                    for &(sx, sz) in chunk {
                        let stack = &columns[&(sx, sz)];
                        let mut borders: [Option<&[section::Section; COLUMN_SECTIONS]>; 8] =
                            [None; 8];
                        let mut slot = 0;
                        for dz in -1..=1_i32 {
                            for dx in -1..=1_i32 {
                                if dx == 0 && dz == 0 {
                                    continue;
                                }
                                borders[slot] =
                                    columns.get(&(sx + dx, sz + dz)).map(|stack| &**stack);
                                slot += 1;
                            }
                        }
                        let blocks = light2::ColumnBlocks::new((**stack).clone(), &borders);
                        out.push(((sx, sz), light2::compute_column(&blocks, table)));
                    }
                    out
                })
            })
            .collect();
        for handle in handles {
            for (key, column) in handle.join().expect("a light job panicked") {
                lit.insert(key, column);
            }
        }
    });
    lit
}

/// Worker count for one fan-out: the available parallelism, at least one.
pub(crate) fn worker_count() -> usize {
    std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get)
}

/// Meshes LOD0 sections for every column (`M3.2`), keyed by column.
///
/// Every column gets an entry, even an empty one, so the streamer can tell a
/// meshed column from a column that has not been visited.
pub(crate) fn demo_fine_all(
    columns: &BTreeMap<(i32, i32), Arc<[section::Section; COLUMN_SECTIONS]>>,
    light: &BTreeMap<(i32, i32), light2::ColumnLight>,
    materials: &mesher2::MaterialProps,
) -> BTreeMap<(i32, i32), Vec<DemoSection>> {
    let tasks: Vec<(i32, i32)> = columns.keys().copied().collect();
    if tasks.is_empty() {
        return BTreeMap::new();
    }
    let workers = worker_count().min(tasks.len());
    let chunk = tasks.len().div_ceil(workers);
    let mut fine = BTreeMap::new();
    std::thread::scope(|scope| {
        let handles: Vec<_> = tasks
            .chunks(chunk)
            .map(|chunk| {
                scope.spawn(move || {
                    let mut out = Vec::with_capacity(chunk.len());
                    for &(sx, sz) in chunk {
                        out.push((sx, sz, demo_fine_column(columns, light, materials, sx, sz)));
                    }
                    out
                })
            })
            .collect();
        for handle in handles {
            for (sx, sz, sections) in handle.join().expect("a mesh job panicked") {
                fine.insert((sx, sz), sections);
            }
        }
    });
    fine
}

/// Meshes every LOD0 section of one column.
pub(crate) fn demo_fine_column(
    columns: &BTreeMap<(i32, i32), Arc<[section::Section; COLUMN_SECTIONS]>>,
    light: &BTreeMap<(i32, i32), light2::ColumnLight>,
    materials: &mesher2::MaterialProps,
    sx: i32,
    sz: i32,
) -> Vec<DemoSection> {
    let mut out = Vec::new();
    for sy in 0..COLUMN_SECTIONS {
        if let Some(section) = demo_section(columns, light, materials, sx, sz, sy) {
            out.push(section);
        }
    }
    out.sort_by_key(|section| section.origin);
    out
}

/// Meshes one section, or `None` when it emits nothing.
///
/// A missing neighbour column contributes `None` to the 26-neighbour array,
/// which keeps `mesher2`'s `I2.3` world-edge mirror rule while a ring fills.
pub(crate) fn demo_section(
    columns: &BTreeMap<(i32, i32), Arc<[section::Section; COLUMN_SECTIONS]>>,
    light: &BTreeMap<(i32, i32), light2::ColumnLight>,
    materials: &mesher2::MaterialProps,
    sx: i32,
    sz: i32,
    sy: usize,
) -> Option<DemoSection> {
    let section = columns.get(&(sx, sz))?.get(sy)?;
    let neighbours: [Option<&section::Section>; 26] =
        padded::NEIGHBOUR_ORDER.map(|(dx, dy, dz)| {
            let ny = i32::try_from(sy).ok()? + dy;
            let neighbour = columns.get(&(sx + dx, sz + dz))?;
            neighbour.get(usize::try_from(ny).ok()?)
        });
    let light = demo_section_light(light, sx, sz, sy);
    let mesh = mesher2::mesh_from_neighbours(
        section,
        &neighbours,
        &light,
        materials,
        mesher2::MeshLod::Lod0,
    );
    if mesher2::quad_count(&mesh) == 0 {
        return None;
    }
    let section_blocks = i32::try_from(description::SECTION_SIZE).unwrap_or(32);
    Some(DemoSection {
        origin: [
            sx * section_blocks,
            i32::try_from(sy).unwrap_or(0) * section_blocks,
            sz * section_blocks,
        ],
        scale: 1,
        mesh,
    })
}

/// The light table for the built-in v1 pack (`L1.1`, `L2.4`, `L2.1`).
///
/// `sky_atten` is the extra skylight loss (water is 1), `opacity` is the block-light
/// loss, and `emission` seeds block light. Ids come from the pack, so a record's
/// light properties are never hardcoded here.
pub(crate) fn demo_light_table() -> light2::LightTable {
    let pack = content::v1_default();
    let mut table = light2::LightTable::new(light2::LightProps::OPAQUE);
    for record in pack.records() {
        let Some(id) = pack.id(&record.name) else {
            continue;
        };
        table.set(
            id,
            light2::LightProps {
                sky_atten: record.sky_atten,
                block_opacity: record.opacity,
            },
        );
        if record.emission > 0 {
            table.set_emission(id, record.emission);
        }
    }
    table
}

#[cfg(test)]
mod tests;
