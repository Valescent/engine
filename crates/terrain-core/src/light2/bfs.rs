//! The propagation scratch: material bits, two light planes, the fixed
//! ring buffer, and the bounded BFS.

use crate::padded::PADDED_WIDTH;
use crate::section::BlockId;

use super::column::{
    COLUMN_HEIGHT, COLUMN_SECTIONS, ColumnLight, LightStats, SECTION_CELLS, WORLD_TOP, nibble,
    section_cell, splat,
};
use super::repair::ColumnView;
use super::table::{LIGHT_LEVELS, LightTable, emission, packed_props, sky_atten, step_cost};

/// Capacity of the fixed BFS ring buffer.
pub(crate) const QUEUE_CAPACITY: usize = 16 * 32_768;

/// [`PADDED_WIDTH`] as the coordinate type.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    reason = "PADDED_WIDTH is 34, far inside i32"
)]
const PADDED_AXIS_I: i32 = PADDED_WIDTH as i32;

/// Cells in the padded column: `34 × 34 × 384`.
pub(super) const PADDED_CELLS: usize = PADDED_WIDTH * PADDED_WIDTH * 384;

/// Nibble words in the padded column.
const PADDED_WORDS: usize = PADDED_CELLS / 16;

/// A section is 32 cells wide on every axis.
pub(super) const LOCAL_MASK: usize = 31;

const _: () = assert!(PADDED_CELLS.is_multiple_of(16));
const _: () = assert!(QUEUE_CAPACITY.is_power_of_two());

/// Fixed neighbour order: `-X, +X, -Z, +Z, -Y, +Y` (`L1.1`).
const NEIGHBOURS: [(i32, i32, i32); 6] = [
    (-1, 0, 0),
    (1, 0, 0),
    (0, 0, -1),
    (0, 0, 1),
    (0, -1, 0),
    (0, 1, 0),
];

/// A channel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Channel {
    Sky,
    Block,
}

/// Fixed-capacity ring of packed BFS entries.
///
/// An entry packs `px | pz << 6 | y << 12 | level << 21`, so a pop needs no
/// division and the whole queue is 2 MiB of `u32`.
pub(super) struct Ring {
    data: Box<[u32]>,
    head: usize,
    len: usize,
}

impl Ring {
    pub(super) fn new() -> Self {
        Self {
            data: vec![0_u32; QUEUE_CAPACITY].into_boxed_slice(),
            head: 0,
            len: 0,
        }
    }

    pub(super) fn push(&mut self, entry: u32) -> bool {
        if self.len == self.data.len() {
            return false;
        }
        let slot = (self.head + self.len) & (QUEUE_CAPACITY - 1);
        self.data[slot] = entry;
        self.len += 1;
        true
    }

    pub(super) fn pop(&mut self) -> Option<u32> {
        if self.len == 0 {
            return None;
        }
        let entry = self.data[self.head];
        self.head = (self.head + 1) & (QUEUE_CAPACITY - 1);
        self.len -= 1;
        Some(entry)
    }
}

/// Per-job scratch: material bits, two light planes, and the queue.
pub(super) struct Scratch {
    cells: Box<[u16]>,
    sky: Box<[u64]>,
    block: Box<[u64]>,
    queue: Ring,
    overflow: bool,
    pub(super) y_lo: i32,
    pub(super) y_hi: i32,
    pub(super) full: bool,
    pub(super) stats: LightStats,
}

impl Scratch {
    pub(super) fn new() -> Self {
        Self {
            cells: vec![0_u16; PADDED_CELLS].into_boxed_slice(),
            sky: vec![0_u64; PADDED_WORDS].into_boxed_slice(),
            block: vec![0_u64; PADDED_WORDS].into_boxed_slice(),
            queue: Ring::new(),
            overflow: false,
            y_lo: 0,
            y_hi: WORLD_TOP,
            full: true,
            stats: LightStats::default(),
        }
    }

    pub(super) fn set_region(&mut self, y_lo: i32, y_hi: i32) {
        self.y_lo = y_lo.max(0);
        self.y_hi = y_hi.min(WORLD_TOP);
        self.full = self.y_lo == 0 && self.y_hi == WORLD_TOP;
    }

    pub(super) fn light(&self, channel: Channel, index: usize) -> u8 {
        let data = match channel {
            Channel::Sky => &self.sky,
            Channel::Block => &self.block,
        };
        nibble(data[index >> 4] >> ((index & 15) << 2))
    }

    pub(super) fn write(&mut self, channel: Channel, index: usize, value: u8) {
        let value = u64::from(value & 0x0F);
        let data = match channel {
            Channel::Sky => &mut self.sky,
            Channel::Block => &mut self.block,
        };
        let shift = (index & 15) << 2;
        let word = &mut data[index >> 4];
        *word = (*word & !(0x0F_u64 << shift)) | (value << shift);
    }

    fn in_w(&self, px: usize, pz: usize, y: i32) -> bool {
        is_ring(px, pz) || (y >= self.y_lo && y <= self.y_hi)
    }

    /// Fills the writable region with one value.
    pub(super) fn prefill(&mut self, channel: Channel, value: u8) {
        if self.full {
            let data = match channel {
                Channel::Sky => &mut self.sky,
                Channel::Block => &mut self.block,
            };
            data.fill(splat(value & 0x0F));
            return;
        }
        for y in self.y_lo..=self.y_hi {
            let py = padded(y);
            for pz in 0..=LOCAL_MASK {
                for px in 0..=LOCAL_MASK {
                    self.write(channel, padded_cell(px + 1, pz + 1, py), value);
                }
            }
        }
        for y in 0..COLUMN_HEIGHT {
            let py = padded(y);
            for px in 0..PADDED_WIDTH {
                self.write(channel, padded_cell(px, 0, py), value);
                self.write(channel, padded_cell(px, PADDED_WIDTH - 1, py), value);
            }
            for pz in 1..PADDED_WIDTH - 1 {
                self.write(channel, padded_cell(0, pz, py), value);
                self.write(channel, padded_cell(PADDED_WIDTH - 1, pz, py), value);
            }
        }
    }

    /// Decodes the column and the 8 border columns into material bits.
    pub(super) fn build(&mut self, view: &ColumnView<'_>, props: &LightTable) {
        let mut decoded: Box<[BlockId; SECTION_CELLS]> = vec![0; SECTION_CELLS]
            .into_boxed_slice()
            .try_into()
            .expect("the scratch matches the section volume");
        for section in 0..COLUMN_SECTIONS {
            let base = section * 32;
            let start = usize::try_from(self.y_lo.max(0)).unwrap_or(0).max(base);
            let end = usize::try_from(self.y_hi.max(0))
                .unwrap_or(0)
                .min(base + LOCAL_MASK);
            if end < start {
                continue;
            }
            view.section(section).write_blocks(&mut decoded);
            for y in start..=end {
                let local = y - base;
                for z in 0..=LOCAL_MASK {
                    for x in 0..=LOCAL_MASK {
                        let id = decoded[section_cell(x, local, z)];
                        self.cells[padded_cell(x + 1, z + 1, y)] = packed_props(props, id);
                    }
                }
            }
        }
        for py in 0..384_usize {
            let y = i32::try_from(py).expect("a padded y fits i32");
            for pz in 0..PADDED_WIDTH {
                for px in 0..PADDED_WIDTH {
                    if !is_ring(px, pz) {
                        continue;
                    }
                    let id = view.get(coord(px), y, coord(pz));
                    self.cells[padded_cell(px, pz, py)] = packed_props(props, id);
                }
            }
        }
    }

    /// Pass 1: sky 15 down from the ceiling, losing only the entering material's
    /// attenuation. Above a cached span the prefill already holds 15.
    pub(super) fn vertical_sky(&mut self, top_span: Option<i32>) {
        for pz in 0..PADDED_WIDTH {
            for px in 0..PADDED_WIDTH {
                let ring = is_ring(px, pz);
                let (bottom, top) = if ring {
                    (0, WORLD_TOP)
                } else {
                    let top = top_span.map_or(self.y_hi, |span| span.clamp(self.y_lo, self.y_hi));
                    (self.y_lo, top)
                };
                let mut level = if top == WORLD_TOP {
                    LIGHT_LEVELS
                } else {
                    let above = usize::try_from(top + 1).expect("a padded y is non-negative");
                    self.light(Channel::Sky, padded_cell(px, pz, above))
                };
                for y in (bottom..=top).rev() {
                    let py = usize::try_from(y).expect("a padded y is non-negative");
                    let index = padded_cell(px, pz, py);
                    level = level.saturating_sub(sky_atten(self.cells[index]));
                    self.write(Channel::Sky, index, level);
                    self.stats.vertical_cells += 1;
                }
            }
        }
    }

    /// Seeds the sky BFS at cells that can improve a writable neighbour.
    pub(super) fn seed_sky(&mut self) {
        for y in self.y_lo..=self.y_hi {
            for pz in 0..=LOCAL_MASK {
                for px in 0..=LOCAL_MASK {
                    self.seed_sky_cell(px + 1, pz + 1, y);
                }
            }
        }
        for y in 0..COLUMN_HEIGHT {
            for px in 0..PADDED_WIDTH {
                self.seed_sky_cell(px, 0, y);
                self.seed_sky_cell(px, PADDED_WIDTH - 1, y);
            }
            for pz in 1..PADDED_WIDTH - 1 {
                self.seed_sky_cell(0, pz, y);
                self.seed_sky_cell(PADDED_WIDTH - 1, pz, y);
            }
        }
        self.seed_outer(Channel::Sky);
    }

    fn seed_sky_cell(&mut self, px: usize, pz: usize, y: i32) {
        let index = padded_cell(px, pz, padded(y));
        let level = self.light(Channel::Sky, index);
        if level > 1 && self.improves(Channel::Sky, px, pz, y, level) {
            self.push(px, pz, y, level);
            self.stats.sky_seeds += 1;
        }
    }

    /// Seeds block light from emitters, then from the unchanged cells around the
    /// writable region.
    pub(super) fn seed_block(&mut self) {
        for y in self.y_lo..=self.y_hi {
            for pz in 0..=LOCAL_MASK {
                for px in 0..=LOCAL_MASK {
                    self.seed_block_cell(px + 1, pz + 1, y);
                }
            }
        }
        for y in 0..COLUMN_HEIGHT {
            for px in 0..PADDED_WIDTH {
                self.seed_block_cell(px, 0, y);
                self.seed_block_cell(px, PADDED_WIDTH - 1, y);
            }
            for pz in 1..PADDED_WIDTH - 1 {
                self.seed_block_cell(0, pz, y);
                self.seed_block_cell(PADDED_WIDTH - 1, pz, y);
            }
        }
        self.seed_outer(Channel::Block);
    }

    fn seed_block_cell(&mut self, px: usize, pz: usize, y: i32) {
        let index = padded_cell(px, pz, padded(y));
        let emission = emission(self.cells[index]);
        if emission > 0 {
            self.write(Channel::Block, index, emission);
            self.push(px, pz, y, emission);
            self.stats.block_seeds += 1;
        }
    }

    /// Seeds from cells outside the writable region that can light into it.
    fn seed_outer(&mut self, channel: Channel) {
        if self.full {
            return;
        }
        let bands = [self.y_lo - 1, self.y_hi + 1];
        for y in bands {
            let Ok(py) = usize::try_from(y) else {
                continue;
            };
            if py >= 384 {
                continue;
            }
            for pz in 0..=LOCAL_MASK {
                for px in 0..=LOCAL_MASK {
                    self.seed_cell(channel, px + 1, pz + 1, py);
                }
            }
        }
        // The edge lines of the interior are adjacent to the border ring for
        // every y, including y outside the halo.
        for py in 0..384_usize {
            let y = i32::try_from(py).expect("a padded y fits i32");
            if y >= self.y_lo && y <= self.y_hi {
                continue;
            }
            for step in 0..=LOCAL_MASK {
                for (px, pz) in [(1, step + 1), (32, step + 1), (step + 1, 1), (step + 1, 32)] {
                    self.seed_cell(channel, px, pz, py);
                }
            }
        }
    }

    fn seed_cell(&mut self, channel: Channel, px: usize, pz: usize, py: usize) {
        let y = i32::try_from(py).expect("a padded y fits i32");
        if self.in_w(px, pz, y) {
            return;
        }
        let level = self.light(channel, padded_cell(px, pz, py));
        if level > 1 && self.improves(channel, px, pz, y, level) {
            self.push(px, pz, y, level);
        }
    }

    /// Whether `level` at this cell can raise a writable neighbour.
    fn improves(&self, channel: Channel, px: usize, pz: usize, y: i32, level: u8) -> bool {
        for (dx, dz, dy) in NEIGHBOURS {
            let Some((nx, nz, ny)) = offset(px, pz, y, dx, dz, dy) else {
                continue;
            };
            if !self.in_w(nx, nz, ny) {
                continue;
            }
            let index = padded_cell(nx, nz, padded(ny));
            let cost = step_cost(channel, self.cells[index], dy);
            if level.saturating_sub(cost) > self.light(channel, index) {
                return true;
            }
        }
        false
    }

    fn push(&mut self, px: usize, pz: usize, y: i32, level: u8) {
        let entry = pack(px, pz, padded(y), level);
        if !self.queue.push(entry) {
            self.overflow = true;
        }
    }

    /// The lateral BFS. Writes only the writable region.
    pub(super) fn flood(&mut self, channel: Channel) {
        while let Some(entry) = self.queue.pop() {
            let level = unpack_level(entry);
            if level <= 1 {
                continue;
            }
            let (px, pz, py) = unpack_cell(entry);
            match channel {
                Channel::Sky => self.stats.sky_steps += 1,
                Channel::Block => self.stats.block_steps += 1,
            }
            let y = i32::try_from(py).expect("a padded y fits i32");
            for (dx, dz, dy) in NEIGHBOURS {
                let Some((nx, nz, ny)) = offset(px, pz, y, dx, dz, dy) else {
                    continue;
                };
                if !self.in_w(nx, nz, ny) {
                    continue;
                }
                let index = padded_cell(nx, nz, padded(ny));
                let next = level.saturating_sub(step_cost(channel, self.cells[index], dy));
                if next > self.light(channel, index) {
                    self.write(channel, index, next);
                    self.push(nx, nz, ny, next);
                }
            }
        }
        if self.overflow {
            self.stats.queue_overflows += 1;
            self.overflow = false;
            self.sweep(channel);
        }
    }

    /// Bounded relaxation to the same fixed point, used after a queue overflow.
    fn sweep(&mut self, channel: Channel) {
        for _ in 0..=LIGHT_LEVELS {
            let mut changed = false;
            for py in 0..384_usize {
                let y = i32::try_from(py).expect("a padded y fits i32");
                for pz in 0..PADDED_WIDTH {
                    for px in 0..PADDED_WIDTH {
                        if !self.in_w(px, pz, y) {
                            continue;
                        }
                        let index = padded_cell(px, pz, py);
                        let current = self.light(channel, index);
                        let mut best = current;
                        for (dx, dz, dy) in NEIGHBOURS {
                            let Some((nx, nz, ny)) = offset(px, pz, y, dx, dz, dy) else {
                                continue;
                            };
                            let source = self.light(channel, padded_cell(nx, nz, padded(ny)));
                            // The light travels from the neighbour to this cell,
                            // so the step direction is the negated offset.
                            let cost = step_cost(channel, self.cells[index], -dy);
                            best = best.max(source.saturating_sub(cost));
                        }
                        if best > current {
                            self.write(channel, index, best);
                            changed = true;
                        }
                    }
                }
            }
            if !changed {
                break;
            }
        }
    }

    /// Reads the interior back into fresh slots.
    pub(super) fn extract(&self) -> ColumnLight {
        let mut light = ColumnLight::new();
        for section in 0..COLUMN_SECTIONS {
            let base = section * 32;
            let sky = &mut light.sky[section];
            let block = &mut light.block[section];
            for y in 0..=LOCAL_MASK {
                for z in 0..=LOCAL_MASK {
                    for x in 0..=LOCAL_MASK {
                        let cell = section_cell(x, y, z);
                        let index = padded_cell(x + 1, z + 1, base + y);
                        sky.set(cell, self.light(Channel::Sky, index));
                        block.set(cell, self.light(Channel::Block, index));
                    }
                }
            }
        }
        light
    }
}

/// Index of one padded cell. `px`, `pz ∈ 0..34` and `py ∈ 0..384`.
pub(super) const fn padded_cell(px: usize, pz: usize, py: usize) -> usize {
    px + PADDED_WIDTH * (pz + PADDED_WIDTH * py)
}

/// Padded `x`/`z` from a world coordinate in `-1..=32`.
pub(super) fn coord(padded_value: usize) -> i32 {
    i32::try_from(padded_value).expect("a padded coordinate fits i32") - 1
}

/// Padded index from a world `y` in `0..384`.
pub(super) fn padded(y: i32) -> usize {
    usize::try_from(y).expect("a padded y is non-negative")
}

/// Whether a padded `x`/`z` is on the border ring.
const fn is_ring(px: usize, pz: usize) -> bool {
    px == 0 || px == PADDED_WIDTH - 1 || pz == 0 || pz == PADDED_WIDTH - 1
}

/// A neighbour in padded space, or `None` outside the padded column.
fn offset(px: usize, pz: usize, y: i32, dx: i32, dz: i32, dy: i32) -> Option<(usize, usize, i32)> {
    let nx = i32::try_from(px).ok()? + dx;
    let nz = i32::try_from(pz).ok()? + dz;
    let ny = y + dy;
    if !(0..PADDED_AXIS_I).contains(&nx)
        || !(0..PADDED_AXIS_I).contains(&nz)
        || !(0..COLUMN_HEIGHT).contains(&ny)
    {
        return None;
    }
    Some((usize::try_from(nx).ok()?, usize::try_from(nz).ok()?, ny))
}

/// Packs a queue entry: `px | pz << 6 | y << 12 | level << 21`.
fn pack(px: usize, pz: usize, py: usize, level: u8) -> u32 {
    let px = u32::try_from(px).expect("a padded x fits u32");
    let pz = u32::try_from(pz).expect("a padded z fits u32");
    let py = u32::try_from(py).expect("a padded y fits u32");
    px | (pz << 6) | (py << 12) | (u32::from(level & 0x0F) << 21)
}

fn unpack_level(entry: u32) -> u8 {
    u8::try_from((entry >> 21) & 0x0F).expect("a level is four bits")
}

fn unpack_cell(entry: u32) -> (usize, usize, usize) {
    let px = usize::try_from(entry & 0x3F).expect("a padded x is six bits");
    let pz = usize::try_from((entry >> 6) & 0x3F).expect("a padded z is six bits");
    let py = usize::try_from((entry >> 12) & 0x1FF).expect("a padded y is nine bits");
    (px, pz, py)
}
