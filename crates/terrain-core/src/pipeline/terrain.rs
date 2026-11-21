//! The terrain generator, its tile caches, and the column memo.
//!
//! A [`Terrain`] owns every compiled field, table, and cache for one seed.
//! Column and section assembly lives in `super::stages`; the world hydrology
//! handle lives in `super::hydro`. The split is organisational only and the
//! generation order is unchanged.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use crate::climate::{BiomeTable, Climate, ClimateParams, ClimateTile, MacroCells};
use crate::content::{self, ContentPack};
use crate::curve::Curve;
use crate::description::{MACRO_CELL, MAX_TERRAIN_Y, SEA, TILE_BLOCKS, WORLD_HEIGHT, default_v1};
use crate::error::{Stage, TerrainError};
use crate::graph::{Compiled, Graph, Lattice, NodeSpec};
use crate::hash::domain;
use crate::height::{self, BlockGrid, HeightTile, MacroGrid, RiverCarve};
use crate::hydro;
use crate::metrics2::{self, Counters, MemoryLedger, ShedState};
use crate::section::BlockId;
use crate::strata::Strata;
use crate::surface::{Layer, SeabedMaterials, SurfaceTable};
use crate::volume::{Volume, VolumeParams};

use super::hydro::{HydroTileRef, hydro_cell};
use super::stages::{TerrainMaterials, v1_biomes};

/// Blocks along one axis of a field tile.
pub(super) const TILE_CELLS: i32 = TILE_BLOCKS / MACRO_CELL;
/// Lattice samples per field tile axis, including the 1-cell border.
pub(super) const LATTICE_SIDE: i32 = TILE_CELLS + 3;
/// Blocks per axis of a height tile, including the 1-block margin.
const HEIGHT_SIDE: i32 = TILE_BLOCKS + 2;
/// Cells in one height tile.
const HEIGHT_CELLS: usize = (HEIGHT_SIDE * HEIGHT_SIDE) as usize;
/// Vertical blocks in a column.
pub(super) const COLUMN: usize = WORLD_HEIGHT as usize;
/// Finished columns kept per [`Terrain`], one full section footprint.
///
/// A section walks `32 x 32` columns and the ring asks for every section of a
/// column before moving on, so this bound lets one section's column work feed
/// the other eleven. It is a hard per-instance bound: about 0.8 MiB of block
/// ids at `BlockId = u16`, plus the key map.
const COLUMN_CACHE_COLUMNS: usize = 32 * 32;
/// Highest relief above sea, in blocks (`C1.5`).
const MAX_RELIEF: f32 = (MAX_TERRAIN_Y - SEA) as f32;
/// Lowest base below sea, in blocks (`C1.6`).
const MIN_BASE: f32 = -32.0;
/// Highest base above sea, in blocks. `C1.6` leaves the inland plateau value open.
const MAX_BASE: f32 = 40.0;
/// Terrace spacing in blocks (`C2.4`).
const TERRACE_STEP: f32 = 8.0;
/// Amplitude of the per-block height detail, in blocks.
const DETAIL_BLOCKS: f32 = 1.5;
/// Edge-ocean band width in blocks (`K1.3`).
const EDGE_BAND: f32 = 256.0;

/// Chunk columns along one field tile axis: a 128-block tile is 4x4 columns
/// (`B4.2`, `SECTION_SIZE = 32`).
const COLUMNS_PER_TILE: u64 = (TILE_BLOCKS / 32) as u64 * (TILE_BLOCKS / 32) as u64;

/// Default `S1.7` memory cap: the pinned ceiling `min(60% RAM, 4 GiB)`.
///
/// Reading host RAM would make diagnostics platform-dependent, and `A1.3` keeps
/// memory caps out of the description hash, so the default ledger uses the
/// ceiling and a runtime owner may replace it.
const DEFAULT_MEM_CAP_BYTES: u64 = 4 * 1024 * 1024 * 1024;

/// Default view radius, a runtime setting (`A1.3`); `N2.1` is measured at 16.
const DEFAULT_VIEW_RADIUS: i32 = 16;

/// The macro curves of `C1.1` and `C1.6`, normalised to `[0, 1]`.
#[derive(Clone, Debug)]
pub(crate) struct Curves {
    /// `curve_base(continentalness)`, normalised between `MIN_BASE` and `MAX_BASE`.
    pub base: Curve,
    /// `curve_relief(roughness)`, normalised between 0 and `MAX_RELIEF`.
    pub relief: Curve,
    /// `curve_shape(peaks)`.
    pub shape: Curve,
    /// Coastal mask: 0 at and below the waterline, 1 inland.
    pub coast: Curve,
    /// Terrace strength by roughness.
    pub terrace: Curve,
    /// Edge-ocean mask: 0 at the world boundary, 1 inland (`K1.3`).
    pub edge: Curve,
}

impl Curves {
    /// The v1 knots.
    ///
    /// # Errors
    ///
    /// Only if a knot table is invalid, which is a code defect rather than data.
    pub(crate) fn v1() -> Result<Self, TerrainError> {
        Ok(Self {
            base: Curve::new(&[
                (0.0, 0.0),
                (0.25, 0.027_778),
                (0.38, 0.277_778),
                (0.45, 0.416_667),
                (0.48, 0.444_444),
                (0.52, 0.486_111),
                (0.60, 0.583_333),
                (1.0, 1.0),
            ])?,
            relief: Curve::new(&[(0.0, 0.0), (0.5, 0.10), (0.8, 0.45), (1.0, 1.0)])?,
            shape: Curve::new(&[(0.0, 0.0), (0.4, 0.15), (0.7, 0.50), (1.0, 1.0)])?,
            coast: Curve::new(&[
                (0.0, 0.0),
                (0.44, 0.0),
                (0.52, 0.30),
                (0.60, 1.0),
                (1.0, 1.0),
            ])?,
            terrace: Curve::new(&[(0.0, 0.0), (0.5, 0.25), (0.8, 0.75), (1.0, 1.0)])?,
            edge: Curve::new(&[(0.0, 0.0), (0.25, 0.15), (0.6, 0.70), (1.0, 1.0)])?,
        })
    }

    /// Base elevation in blocks relative to sea.
    #[must_use]
    pub(crate) fn base_blocks(&self, continentalness: f32) -> f32 {
        MIN_BASE + self.base.sample(continentalness) * (MAX_BASE - MIN_BASE)
    }

    /// Relief in blocks.
    #[must_use]
    pub(crate) fn relief_blocks(&self, roughness: f32) -> f32 {
        self.relief.sample(roughness) * MAX_RELIEF
    }
}

/// Everything one 128-block field tile needs, built once.
#[derive(Debug)]
pub struct TileData {
    /// The 130x130 derived tops (`E4.6`).
    pub height: HeightTile,
    /// Lattice heights, bilinear source.
    pub macro_grid: MacroGrid,
    /// Continentalness per lattice cell, for the beach band.
    pub continentalness: Vec<f32>,
    /// Roughness per lattice cell, for the detail mask.
    pub roughness: Vec<f32>,
    /// Climate and biome selection (`E1`-`E3`).
    pub climate: ClimateTile,
}

/// The terrain generator for one world description.
///
/// Not `Sync`: the tile caches use `RefCell`. Give every worker its own.
pub struct Terrain {
    pub(super) seed: u64,
    description_hash: [u8; 32],
    pub(super) half_extent: i32,
    continentalness: Compiled,
    pub(super) roughness: Compiled,
    peaks: Compiled,
    curves: Curves,
    pub(super) strata: Strata,
    climate: Climate,
    climate_params: ClimateParams,
    biomes: BiomeTable,
    /// Domain tag of the per-column biome dither, hashed once (`E3.3`).
    ///
    /// The tag is a pure function of its name, so caching it changes no output;
    /// it keeps the column path to one hash and no string hashing.
    pub(super) dither_tag: u64,
    pub(super) surface: SurfaceTable,
    pub(super) volume: Volume,
    /// Runtime ids resolved from the content pack (`§2.6`). Every block the
    /// pipeline emits comes from here; no numeric literal is authoritative.
    pub(super) materials: TerrainMaterials,
    /// `Q1.1` counters. Recorded where this generator can observe a stage:
    /// worker latency (fields, hydrology, height, volume, features) and the
    /// column/LOD gauges. `upload_bytes` stays 0 (there is no upload path here);
    /// `dropped_before_start`, `discarded_stale`, and `applied` are residency
    /// counters and stay 0 until `K1`/`K3` own a queue; `meshing_p50` stays 0
    /// because this pipeline stops at [`crate::section::Section`] and never meshes.
    ///
    /// No counter, clock, or ledger value ever enters a hash, a block id, a
    /// height, or a cache key (`A2.4`). Diagnostics are write-only here.
    pub(super) counters: Counters,
    /// Default `S1.7` ledger. Nothing reserves bytes yet; residency will own it.
    ledger: MemoryLedger,
    /// Default `K4.3` ladder, at `Full` until a load signal exists.
    shed: ShedState,
    /// Distinct LOD0 columns with at least one generated section (`N1.4`).
    pub(super) lod0_columns: RefCell<BTreeSet<(i32, i32)>>,
    tiles: RefCell<BTreeMap<(i32, i32), std::rc::Rc<TileData>>>,
    pub(super) hydro: RefCell<BTreeMap<(i32, i32), HydroTileRef>>,
    /// The one whole-world hydrology result, solved on first use and shared
    /// with every worker that installs it ([`Terrain::with_world_hydro`]).
    pub(super) world_hydro: RefCell<Option<Arc<hydro::WorldHydro>>>,
    /// Bounded per-instance memo of finished columns.
    ///
    /// A column's blocks are pure generator output, so reusing one changes no
    /// byte. The cache is keyed by exact world `(x, z)`, is never shared
    /// between `Terrain` instances, and is cleared when the hydrology result
    /// changes. See [`COLUMN_CACHE_COLUMNS`] for the bound.
    pub(super) column_cache: RefCell<ColumnCache>,
}

/// Finished columns kept for reuse across the sections of one column.
#[derive(Default)]
pub(super) struct ColumnCache {
    pub(super) entries: BTreeMap<(i32, i32), Rc<[BlockId; COLUMN]>>,
    /// Insertion order, so eviction is a hard bound with no clock.
    pub(super) order: VecDeque<(i32, i32)>,
}

impl ColumnCache {
    fn get(&self, x: i32, z: i32) -> Option<Rc<[BlockId; COLUMN]>> {
        self.entries.get(&(x, z)).map(Rc::clone)
    }

    fn insert(&mut self, x: i32, z: i32, column: Rc<[BlockId; COLUMN]>) {
        if self.entries.len() >= COLUMN_CACHE_COLUMNS
            && let Some(oldest) = self.order.pop_front()
        {
            self.entries.remove(&oldest);
        }
        self.order.push_back((x, z));
        self.entries.insert((x, z), column);
    }
}

impl std::fmt::Debug for Terrain {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Terrain")
            .field("seed", &self.seed)
            .field("description_hash", &self.description_hash)
            .finish_non_exhaustive()
    }
}

impl Terrain {
    /// Builds the v1 generator for a seed with the built-in content pack.
    ///
    /// # Errors
    ///
    /// Returns a typed error when a curve, graph, table, or content record
    /// fails validation.
    pub fn new(seed: u64) -> Result<Self, TerrainError> {
        Self::with_pack(seed, &content::v1_default())
    }

    /// Builds the v1 generator for a seed against a caller-supplied pack.
    ///
    /// Ids are resolved from stable names once, here (`§2.6`), so generation
    /// never reads a numeric literal.
    ///
    /// # Errors
    ///
    /// Returns a typed error when a curve, graph, or table fails validation, or
    /// [`crate::error::ErrorKind::ManifestIncompatible`] when a required record is missing.
    pub fn with_pack(seed: u64, pack: &ContentPack) -> Result<Self, TerrainError> {
        let description = default_v1();
        let materials = TerrainMaterials::from_pack(pack)?;
        let continentalness = field_graph(NodeSpec::fbm(
            Lattice::Macro,
            stream_seed(seed, "continentalness"),
            1.0 / 2048.0,
            4,
            2.0,
            0.5,
        ))?;
        let roughness = field_graph(NodeSpec::fbm(
            Lattice::Macro,
            stream_seed(seed, "roughness"),
            1.0 / 512.0,
            4,
            2.0,
            0.5,
        ))?;
        let peaks = field_graph(NodeSpec::ridged(
            Lattice::Macro,
            stream_seed(seed, "peaks"),
            1.0 / 256.0,
            4,
            2.0,
            0.5,
        ))?;

        let strata = Strata::new(
            seed,
            24,
            8.0,
            256,
            materials.stone,
            vec![
                materials.stone,
                materials.dirt,
                materials.stone,
                materials.stone,
                materials.dirt,
                materials.stone,
            ],
        )?;
        let surface = SurfaceTable::with_seabed(
            seed,
            // OPEN(cold-beach): the cold-beach material is not pinned; keep sand.
            Layer::Block(materials.sand),
            Layer::Block(materials.sand),
            SeabedMaterials {
                shore_sand: Layer::Block(materials.sand),
                shore_gravel: Layer::Block(materials.gravel),
                shelf_sand: Layer::Block(materials.sand),
                shelf_gravel: Layer::Block(materials.gravel),
                shelf_clay: Layer::Block(materials.clay),
                slope_gravel: Layer::Block(materials.gravel),
                abyss_clay: Layer::Block(materials.clay),
            },
            // OPEN(snow-layer): `§10.5` specifies a snow layer; v1 emits a full cube.
            Layer::Block(materials.snow),
            Layer::Stone,
            Layer::Block(materials.sand),
            Layer::Block(materials.gravel),
            Layer::Block(materials.bedrock),
        );
        let volume = Volume::new(VolumeParams::v1_default(seed))?;
        Ok(Self {
            seed,
            description_hash: description.description_hash()?,
            half_extent: description.world_half_extent,
            continentalness,
            roughness,
            peaks,
            curves: Curves::v1()?,
            strata,
            climate: Climate::default(),
            climate_params: ClimateParams::v1_default(),
            biomes: v1_biomes(),
            dither_tag: domain(crate::climate::DITHER_DOMAIN),
            surface,
            volume,
            materials,
            counters: Counters::new(),
            ledger: MemoryLedger::new(DEFAULT_MEM_CAP_BYTES),
            shed: ShedState::new(DEFAULT_VIEW_RADIUS),
            lod0_columns: RefCell::new(BTreeSet::new()),
            tiles: RefCell::new(BTreeMap::new()),
            hydro: RefCell::new(BTreeMap::new()),
            world_hydro: RefCell::new(None),
            column_cache: RefCell::new(ColumnCache::default()),
        })
    }

    /// The ids this generator resolved from its content pack (`§2.6`).
    #[must_use]
    pub fn materials(&self) -> &TerrainMaterials {
        &self.materials
    }

    /// The pinned `Q1.1` counters this generator has recorded.
    ///
    /// Reads are shared: the slots are cells, so generation records through
    /// `&self` and the HUD reads the same block. The values are diagnostics
    /// only and never feed generated output.
    #[must_use]
    pub fn counters(&self) -> &Counters {
        &self.counters
    }

    /// The `Q1.5` overlay read model: one line per pinned group, 8 lines.
    ///
    /// The ledger is the default `S1.7` budget and the `K4.3` ladder is at
    /// `Full`, because neither has an owner yet: no bytes are reserved and
    /// nothing is shed. `Q1.7` exports these same counters to the HUD.
    #[must_use]
    pub fn overlay(&self) -> Vec<String> {
        metrics2::overlay_lines(&self.counters, &self.ledger, &self.shed)
    }

    /// World identity.
    #[must_use]
    pub fn description_hash(&self) -> [u8; 32] {
        self.description_hash
    }

    /// The world seed.
    #[must_use]
    pub fn seed(&self) -> u64 {
        self.seed
    }
}

impl Terrain {
    /// The whole tile cache entry, built on first use.
    ///
    /// # Errors
    ///
    /// Propagates curve and table failures from the stages.
    pub fn tile(&self, tile_x: i32, tile_z: i32) -> Result<std::rc::Rc<TileData>, TerrainError> {
        if let Some(cached) = self.tiles.borrow().get(&(tile_x, tile_z)) {
            return Ok(std::rc::Rc::clone(cached));
        }
        let tile = std::rc::Rc::new(self.build_tile(tile_x, tile_z)?);
        self.tiles
            .borrow_mut()
            .insert((tile_x, tile_z), std::rc::Rc::clone(&tile));
        self.publish_column_gauges();
        Ok(tile)
    }

    /// Republishes the residency gauges from the tile cache.
    ///
    /// A field tile covers [`COLUMNS_PER_TILE`] chunk columns. This pipeline has
    /// no admission queue and no eviction, so a cached tile is resident and
    /// ready at the same time and no column is ever waiting: `queued_columns` is
    /// structurally zero until `K1` owns residency and can report a real queue.
    fn publish_column_gauges(&self) {
        let columns = u64::try_from(self.tiles.borrow().len())
            .unwrap_or(u64::MAX)
            .saturating_mul(COLUMNS_PER_TILE);
        self.counters.set_gauges(columns, columns, 0);
    }

    fn build_tile(&self, tile_x: i32, tile_z: i32) -> Result<TileData, TerrainError> {
        let fields_started = Instant::now();
        let mut macro_samples = [0.0_f32; height::MACRO_CELLS];
        let mut continental = vec![0.0; (LATTICE_SIDE * LATTICE_SIDE) as usize];
        let mut roughness = vec![0.0; (LATTICE_SIDE * LATTICE_SIDE) as usize];
        for cell_z in 0..LATTICE_SIDE {
            for cell_x in 0..LATTICE_SIDE {
                let world_cell_x = tile_x * TILE_CELLS + cell_x - 1;
                let world_cell_z = tile_z * TILE_CELLS + cell_z - 1;
                let block_x = f32::from(i16::try_from(world_cell_x * MACRO_CELL + 2).unwrap_or(0));
                let block_z = f32::from(i16::try_from(world_cell_z * MACRO_CELL + 2).unwrap_or(0));
                let height = self.macro_height(block_x, block_z);
                let index = (cell_x + LATTICE_SIDE * cell_z) as usize;
                macro_samples[index] = height;
                continental[index] = self.continentalness.eval_2d(block_x, block_z);
                roughness[index] = self.roughness.eval_2d(block_x, block_z);
            }
        }
        // Read after the lattice work, recorded as a diagnostic, never stored.
        self.counters
            .record_latency(Stage::Fields, micros(fields_started));
        let macro_grid = MacroGrid::from_samples(&macro_samples);
        let height = self.build_height_tile(tile_x, tile_z, &macro_grid)?;
        let climate = self.build_climate(tile_x, tile_z)?;
        Ok(TileData {
            height,
            macro_grid,
            continentalness: continental,
            roughness,
            climate,
        })
    }

    fn build_height_tile(
        &self,
        tile_x: i32,
        tile_z: i32,
        macro_grid: &MacroGrid,
    ) -> Result<HeightTile, TerrainError> {
        let height_started = Instant::now();
        let side = HEIGHT_SIDE;
        let mut detail = vec![0.0_f32; HEIGHT_CELLS];
        let mut terrace = vec![0.0_f32; HEIGHT_CELLS];
        let mut width = vec![0.0_f32; HEIGHT_CELLS];
        let mut distance = vec![f32::MAX; HEIGHT_CELLS];
        let detail_seed = stream_seed(self.seed, "detail");
        let hydro = self.hydro_tile(tile_x.div_euclid(4), tile_z.div_euclid(4))?;
        for local_z in 0..side {
            for local_x in 0..side {
                let block_x = tile_x * TILE_BLOCKS + local_x - 1;
                let block_z = tile_z * TILE_BLOCKS + local_z - 1;
                let index = (local_x + side * local_z) as usize;
                let macro_h = macro_grid.sample(local_x - 1, local_z - 1);
                let rough = self.roughness.eval_2d(block_x as f32, block_z as f32);
                let mask = self.curves.terrace.sample(rough);
                detail[index] = crate::noise::fbm_2d(
                    detail_seed,
                    block_x as f32,
                    block_z as f32,
                    1.0 / 48.0,
                    3,
                    2.0,
                    0.5,
                ) * DETAIL_BLOCKS
                    * mask;
                let h = macro_h + detail[index];
                let plateau = quantise(h, TERRACE_STEP);
                terrace[index] = (plateau - h) * mask;
                let cell = hydro_cell(block_x, block_z);
                width[index] = hydro.river_width(cell.0, cell.1);
                distance[index] = hydro.height(cell.0, cell.1);
            }
        }
        let built = height::build(
            macro_grid,
            &BlockGrid::new(detail.into_boxed_slice())?,
            &BlockGrid::new(terrace.into_boxed_slice())?,
            &RiverCarve::new(
                BlockGrid::new(width.into_boxed_slice())?,
                BlockGrid::new(distance.into_boxed_slice())?,
            ),
        );
        // Diagnostics only; the `HeightTile` above is already final.
        self.counters
            .record_latency(Stage::Height, micros(height_started));
        Ok(built)
    }

    fn build_climate(&self, tile_x: i32, tile_z: i32) -> Result<ClimateTile, TerrainError> {
        let width = crate::climate::SIDE + crate::climate::SHADOW_RANGE_CELLS;
        let mut values = vec![0.0_f32; (width * crate::climate::SIDE) as usize];
        for cell_z in 0..crate::climate::SIDE {
            for cell_x in 0..width {
                let world_cell_x = tile_x * TILE_CELLS + cell_x - 1;
                let world_cell_z = tile_z * TILE_CELLS + cell_z - 1;
                let block_x = f32::from(i16::try_from(world_cell_x * MACRO_CELL + 2).unwrap_or(0));
                let block_z = f32::from(i16::try_from(world_cell_z * MACRO_CELL + 2).unwrap_or(0));
                values[(cell_x + width * cell_z) as usize] = self.macro_height(block_x, block_z);
            }
        }
        let cells = MacroCells::new(width, crate::climate::SIDE, values.into_boxed_slice())?;
        Ok(self.climate.solve_at(
            tile_x * TILE_CELLS,
            tile_z * TILE_CELLS,
            &cells,
            &self.biomes,
            &self.climate_params,
            self.seed,
        ))
    }
}

impl Terrain {
    /// Macro height at a block position, `f32` throughout (`C3.5`).
    #[must_use]
    pub fn macro_height(&self, block_x: f32, block_z: f32) -> f32 {
        let continentalness = self.continentalness.eval_2d(block_x, block_z);
        let roughness = self.roughness.eval_2d(block_x, block_z);
        let peaks = self.peaks.eval_2d(block_x, block_z);
        let base = self.base_blocks(block_x, block_z, continentalness);
        let relief = self.curves.relief_blocks(roughness);
        let shape = self.curves.shape.sample(peaks);
        let mask = self.curves.coast.sample(continentalness);
        let h = SEA as f32 + base + relief * shape * mask;
        h.clamp(1.0, MAX_TERRAIN_Y as f32)
    }

    /// Base elevation in blocks, after the edge-ocean mask (`K1.3`).
    ///
    /// The mask pushes `base` toward the ocean floor within `EDGE_BAND` of the
    /// finite world's boundary, before any hydrology reads the height. It is the
    /// same `base` the beach band sees, so a coast can never sit on the rim.
    #[must_use]
    pub(super) fn base_blocks(&self, block_x: f32, block_z: f32, continentalness: f32) -> f32 {
        let base = self.curves.base_blocks(continentalness);
        let half = self.half_extent as f32;
        let inward = (half - block_x.abs()).min(half - block_z.abs());
        let ratio = (inward / EDGE_BAND).clamp(0.0, 1.0);
        let keep = self.curves.edge.sample(ratio);
        MIN_BASE + (base - MIN_BASE) * keep
    }
}

impl Terrain {
    /// Generates one block column.
    ///
    /// The result is served from the per-instance column memo when this column
    /// was generated already, so the sections of one column share its work.
    ///
    /// # Errors
    ///
    /// Propagates tile and table failures.
    pub fn column(&self, block_x: i32, block_z: i32) -> Result<[BlockId; COLUMN], TerrainError> {
        Ok(*self.shared_column(block_x, block_z)?)
    }

    /// One finished column, shared by every section that covers it.
    ///
    /// The memo is bounded by [`COLUMN_CACHE_COLUMNS`] and keyed by the exact
    /// world `(x, z)`. A column is a pure function of the generator state, so
    /// a hit is the same array a fresh [`Terrain::generate_column`] returns.
    pub(super) fn shared_column(
        &self,
        block_x: i32,
        block_z: i32,
    ) -> Result<Rc<[BlockId; COLUMN]>, TerrainError> {
        if let Some(column) = self.column_cache.borrow().get(block_x, block_z) {
            return Ok(column);
        }
        let column = Rc::new(self.generate_column(block_x, block_z)?);
        self.column_cache
            .borrow_mut()
            .insert(block_x, block_z, Rc::clone(&column));
        Ok(column)
    }
}

fn field_graph(node: NodeSpec) -> Result<Compiled, TerrainError> {
    let mut graph = Graph::new();
    let id = graph.push(node);
    graph.set_output(id);
    graph.compile()
}

/// Microseconds elapsed since `started`.
///
/// The clock is read only after a stage returns (`A2.4`), and the value goes
/// nowhere except [`Counters`]: never a hash, block id, height, or cache key.
pub(super) fn micros(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX)
}

fn stream_seed(seed: u64, name: &str) -> u32 {
    let tag = domain(name);
    let hashed = crate::hash::hash64(seed, tag, &[]);
    u32::try_from(hashed ^ (hashed >> 32)).unwrap_or(0)
}

fn quantise(height: f32, step: f32) -> f32 {
    let relative = (height - SEA as f32) / step;
    let k = (relative + 0.5).floor();
    SEA as f32 + k * step
}
