//! Incremental repair: the borrowed view, the edit span, and the
//! clear/re-light over the writable region.

use crate::description::{SECTION_SIZE, WORLD_HEIGHT};
use crate::section::{BlockId, Section};

use super::bfs::{Channel, LOCAL_MASK, Scratch, coord, padded, padded_cell};
use super::column::{
    COLUMN_HEIGHT, COLUMN_SECTIONS, ColumnBlocks, ColumnLight, WORLD_TOP, border_slot, edge,
    mirror, neighbour_local, section_cell,
};
use super::table::{AIR_ID, BEDROCK_ID, LIGHT_LEVELS, LightTable};

/// Halo, in blocks, that an edit can influence (`L3.1`).
pub const RELIGHT_RADIUS: i32 = 15;

/// One changed cell. The blocks are already updated when a repair runs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Edit {
    /// Local `x` in `0..32`.
    pub x: i32,
    /// World `y` in `0..384`.
    pub y: i32,
    /// Local `z` in `0..32`.
    pub z: i32,
    /// The block now in the cell.
    pub block: BlockId,
}

impl Edit {
    /// Builds an edit record.
    #[must_use]
    pub(crate) const fn new(x: i32, y: i32, z: i32, block: BlockId) -> Self {
        Self { x, y, z, block }
    }
}

/// A borrowed column and its eight border columns.
///
/// This is the read-only view the repair path uses to avoid copying a whole
/// column (and every border) before relighting.
#[derive(Clone, Copy, Debug)]
pub struct ColumnView<'a> {
    pub(super) sections: &'a [Section; COLUMN_SECTIONS],
    pub(super) borders: [Option<&'a [Section; COLUMN_SECTIONS]>; 8],
    pub(super) top_span: Option<i32>,
}

impl<'a> ColumnView<'a> {
    /// Borrows a column and its border order `(-1,-1) ..= (1,1)`.
    #[must_use]
    pub fn new(
        sections: &'a [Section; COLUMN_SECTIONS],
        borders: [Option<&'a [Section; COLUMN_SECTIONS]>; 8],
    ) -> Self {
        Self {
            sections,
            borders,
            top_span: None,
        }
    }

    /// Caches the highest `y` whose block is not sky-clear (`L1.1`).
    #[must_use]
    pub fn with_top_span(mut self, top: i32) -> Self {
        self.top_span = Some(top);
        self
    }

    /// The cached span, if any.
    #[must_use]
    pub fn top_span(&self) -> Option<i32> {
        self.top_span
    }

    /// One section of the column.
    #[must_use]
    pub fn section(&self, index: usize) -> &Section {
        &self.sections[index % COLUMN_SECTIONS]
    }

    /// Reads a block in `x, z ∈ -1..=32` and `y ∈ 0..384` (`I2.3`).
    #[must_use]
    pub fn get(&self, x: i32, y: i32, z: i32) -> BlockId {
        let Some(y) = u32::try_from(y).ok().filter(|value| *value < WORLD_HEIGHT) else {
            return if y < 0 { BEDROCK_ID } else { AIR_ID };
        };
        let section = usize::try_from(y / SECTION_SIZE).expect("a section index fits usize");
        let local_y = y % SECTION_SIZE;
        let dx = edge(x);
        let dz = edge(z);
        if dx == 0 && dz == 0 {
            let local_x = u32::try_from(x).expect("an interior x is non-negative");
            let local_z = u32::try_from(z).expect("an interior z is non-negative");
            return self.sections[section].get(local_x, local_y, local_z);
        }
        let local_x = neighbour_local(x, dx);
        let local_z = neighbour_local(z, dz);
        if let Some(column) = self.borders[border_slot(dx, dz)] {
            return column[section].get(local_x, local_y, local_z);
        }
        self.sections[section].get(
            u32::try_from(mirror(x, dx)).expect("a mirrored x is non-negative"),
            local_y,
            u32::try_from(mirror(z, dz)).expect("a mirrored z is non-negative"),
        )
    }
}

/// The writable `y` range for a batch of edits (`L3.1`).
///
/// Lateral light reaches 15 blocks, so cells more than 15 above an edit cannot
/// change. The vertical sky seed is lossless downward, however: placing a block
/// changes the seed of every cell below it in that column, all the way to the
/// floor. A correct repair therefore always starts at `y = 0` and stops 15 above
/// the highest touched section.
pub(super) fn relight_bounds(edits: &[Edit]) -> Option<(i32, i32)> {
    let mut high = i32::MIN;
    for edit in edits {
        let section_top = edit.y - edit.y.rem_euclid(32);
        high = high.max(section_top + 31 + RELIGHT_RADIUS);
    }
    if high == i32::MIN {
        return None;
    }
    Some((0, high.min(WORLD_TOP)))
}

impl ColumnLight {
    /// Repairs light after a batch of edits, returning how many cells changed.
    ///
    /// The caller has already written every edit into `blocks`; `edits` says
    /// which cells changed so the repair can bound its region (`L3.1`). `self`
    /// must already hold the light for `blocks` before the batch, which is what
    /// [`crate::light2::compute_column`] produces. The two-phase clear/re-light
    /// below produces the same fixed point as [`crate::light2::compute_column`],
    /// which is the `L3.3` gate.
    ///
    /// `props` is the material table: a single [`crate::light2::LightProps`]
    /// record cannot describe a whole column, so the parameter is the table
    /// type.
    pub(crate) fn relight_column(
        &mut self,
        blocks: &ColumnBlocks,
        props: &LightTable,
        edits: &[Edit],
    ) -> u32 {
        let Some((y_lo, y_hi)) = relight_bounds(edits) else {
            return 0;
        };
        self.relight_span(blocks, props, y_lo, y_hi)
    }

    /// Repairs light for an explicit vertical band.
    ///
    /// The caller guarantees no cell outside `y_lo..=y_hi` changed. The band is
    /// widened by one cell so the flood has unchanged values to stop against.
    /// `Self::relight_column` derives the band from edits; a column whose own
    /// sky seed did not move can use a narrow band around the edit instead of
    /// repairing from `y = 0`.
    pub fn relight_span(
        &mut self,
        blocks: &ColumnBlocks,
        props: &LightTable,
        y_lo: i32,
        y_hi: i32,
    ) -> u32 {
        self.relight_span_view(&blocks.view(), props, y_lo, y_hi)
    }

    /// [`Self::relight_span`] over a borrowed column, with no copy.
    pub fn relight_span_view(
        &mut self,
        view: &ColumnView<'_>,
        props: &LightTable,
        y_lo: i32,
        y_hi: i32,
    ) -> u32 {
        let lo = (y_lo - 1).max(0);
        let hi = (y_hi + 1).min(WORLD_TOP);
        if hi < lo {
            return 0;
        }
        let mut scratch = Scratch::new();
        scratch.set_region(lo, hi);
        scratch.build(view, props);
        scratch.copy_interior_from(self);
        scratch.prefill(Channel::Sky, LIGHT_LEVELS);
        scratch.prefill(Channel::Block, 0);
        scratch.vertical_sky(view.top_span());
        scratch.seed_sky();
        scratch.flood(Channel::Sky);
        scratch.seed_block();
        scratch.flood(Channel::Block);
        scratch.store(self)
    }
}

impl Scratch {
    /// Copies the unchanged light the repair can read into the planes.
    ///
    /// Only two kinds of cell matter: the rows directly above and below the
    /// writable band, and the interior edge lines outside it, which seed the
    /// flood along section walls. The prefill overwrites the band itself, and
    /// the flood only visits writable cells, so nothing else is read.
    fn copy_interior_from(&mut self, light: &ColumnLight) {
        if self.full {
            return;
        }
        for y in [self.y_lo - 1, self.y_hi + 1] {
            if (0..COLUMN_HEIGHT).contains(&y) {
                self.copy_light_row(light, y);
            }
        }
        for y in 0..COLUMN_HEIGHT {
            if (y >= self.y_lo && y <= self.y_hi) || y == self.y_lo - 1 || y == self.y_hi + 1 {
                continue;
            }
            self.copy_light_edges(light, y);
        }
    }

    /// Copies one whole interior row from the existing light.
    fn copy_light_row(&mut self, light: &ColumnLight, y: i32) {
        let py = padded(y);
        for pz in 0..=LOCAL_MASK {
            for px in 0..=LOCAL_MASK {
                let index = padded_cell(px + 1, pz + 1, py);
                let (x, z) = (coord(px), coord(pz));
                self.write(Channel::Sky, index, light.sky(x, y, z));
                self.write(Channel::Block, index, light.block(x, y, z));
            }
        }
    }

    /// Copies the four interior edge lines of one row.
    fn copy_light_edges(&mut self, light: &ColumnLight, y: i32) {
        let py = padded(y);
        for step in 0..=LOCAL_MASK {
            for (px, pz) in [(0, step), (LOCAL_MASK, step), (step, 0), (step, LOCAL_MASK)] {
                let index = padded_cell(px + 1, pz + 1, py);
                let (x, z) = (coord(px), coord(pz));
                self.write(Channel::Sky, index, light.sky(x, y, z));
                self.write(Channel::Block, index, light.block(x, y, z));
            }
        }
    }

    /// Writes the interior back, counting cells whose value changed.
    fn store(&self, light: &mut ColumnLight) -> u32 {
        let mut changed = 0_u32;
        let lo = usize::try_from(self.y_lo.max(0)).unwrap_or(0);
        let hi = usize::try_from(self.y_hi.max(0)).unwrap_or(0);
        for section in 0..COLUMN_SECTIONS {
            let base = section * 32;
            let start = lo.max(base);
            let end = hi.min(base + LOCAL_MASK);
            if end < start {
                continue;
            }
            for y in start..=end {
                let local = y - base;
                for z in 0..=LOCAL_MASK {
                    for x in 0..=LOCAL_MASK {
                        let cell = section_cell(x, local, z);
                        let index = padded_cell(x + 1, z + 1, y);
                        let sky = self.light(Channel::Sky, index);
                        let block = self.light(Channel::Block, index);
                        let mut differs = light.sky[section].get(cell) != sky;
                        if differs {
                            light.sky[section].set(cell, sky);
                        }
                        if light.block[section].get(cell) != block {
                            light.block[section].set(cell, block);
                            differs = true;
                        }
                        changed += u32::from(differs);
                    }
                }
            }
        }
        changed
    }
}
