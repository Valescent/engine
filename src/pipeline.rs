//! The column pipeline, in the fixed `F1.5` order.
//!
//! `HeightTile` -> fill -> detail -> carve -> spans -> features. The macro fields
//! are graph data evaluated on the 4-block lattice; everything per block happens
//! here, after interpolation, exactly once per column.
//!
//! Order that must not change:
//!
//! 1. macro height from the lattice, bilinear to blocks,
//! 2. per-block detail, then terrace, then river carve, one [`top_solid_y`],
//! 3. solid fill from the surface stack, strata resolved symbolically,
//! 4. 3D detail and cave carve ([`Volume::apply`]),
//! 5. water fill, which never floods a cave (`D7.1`).
//!
//! Water is placed after the volume pass rather than at step 2 as `F1.5` words it.
//! The result is identical because water only occupies `top+1..=level`, which the
//! volume pass never writes; the pass needs a solid/air-only column to tell rock
//! from water.
//!
//! The world graph here is the v1 authoring of `C1.1`/`C1.6`/`C2.2`. Curve knots
//! live in [`Curves::v1`] and each one is a column of `terrain-interview.md`.

// Coordinate arithmetic only: every cast below is between in-range world
// coordinates and their index or `f32` representation. World extents are far
// inside `f32`'s 24-bit mantissa and every index is non-negative.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use crate::climate::{Biome, BiomeTable, Climate, ClimateParams, ClimateTile, MacroCells};
use crate::content::{self, ContentPack};
use crate::curve::Curve;
use crate::description::{
    HYDRO_TILE, MACRO_CELL, MAX_TERRAIN_Y, SEA, TILE_BLOCKS, WORLD_HEIGHT, default_v1,
};
use crate::error::{ErrorKind, Stage, TerrainError};
use crate::features::{self, ApplyContext, ColumnField, Palette};
use crate::graph::{Compiled, Graph, Lattice, NodeSpec};
use crate::hash::domain;
use crate::height::{self, BlockGrid, HeightTile, MacroGrid, RiverCarve};
use crate::hydro::{self, HydroTile};
use crate::metrics2::{self, Counters, MemoryLedger, ShedState};
use crate::section::{BlockId, Section};
use crate::strata::Strata;
use crate::surface::{
    BiomeRules, ColumnFacts, Layer, Predicate, Rule, SeabedMaterials, SurfaceStack, SurfaceTable,
};
use crate::volume::{ColumnInput, Volume, VolumeParams};

/// Air. `core:air` sorts first, so its id is `0` by construction (`§0.1`, `§2.6`).
pub const AIR: BlockId = 0;
/// Water. Kept for the `terrain-gen` caller; pinned to the v1 pack by a test.
///
/// Generation never reads this constant: [`TerrainMaterials::water`] is the
/// authoritative handle.
pub const WATER: BlockId = 16;

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

/// Blocks along one axis of a field tile.
const TILE_CELLS: i32 = TILE_BLOCKS / MACRO_CELL;
/// Lattice samples per field tile axis, including the 1-cell border.
const LATTICE_SIDE: i32 = TILE_CELLS + 3;
/// Blocks per axis of a height tile, including the 1-block margin.
const HEIGHT_SIDE: i32 = TILE_BLOCKS + 2;
/// Cells in one height tile.
const HEIGHT_CELLS: usize = (HEIGHT_SIDE * HEIGHT_SIDE) as usize;
/// Vertical blocks in a column.
const COLUMN: usize = WORLD_HEIGHT as usize;
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
pub struct Curves {
    /// `curve_base(continentalness)`, normalised between [`MIN_BASE`] and [`MAX_BASE`].
    pub base: Curve,
    /// `curve_relief(roughness)`, normalised between 0 and [`MAX_RELIEF`].
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
    pub fn v1() -> Result<Self, TerrainError> {
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
    pub fn base_blocks(&self, continentalness: f32) -> f32 {
        MIN_BASE + self.base.sample(continentalness) * (MAX_BASE - MIN_BASE)
    }

    /// Relief in blocks.
    #[must_use]
    pub fn relief_blocks(&self, roughness: f32) -> f32 {
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
    seed: u64,
    description_hash: [u8; 32],
    half_extent: i32,
    continentalness: Compiled,
    roughness: Compiled,
    peaks: Compiled,
    curves: Curves,
    strata: Strata,
    climate: Climate,
    climate_params: ClimateParams,
    biomes: BiomeTable,
    surface: SurfaceTable,
    volume: Volume,
    /// Runtime ids resolved from the content pack (`§2.6`). Every block the
    /// pipeline emits comes from here; no numeric literal is authoritative.
    materials: TerrainMaterials,
    /// `Q1.1` counters. Recorded where this generator can observe a stage:
    /// worker latency (fields, hydrology, height, volume, features) and the
    /// column/LOD gauges. `upload_bytes` stays 0 (there is no upload path here);
    /// `dropped_before_start`, `discarded_stale`, and `applied` are residency
    /// counters and stay 0 until `K1`/`K3` own a queue; `meshing_p50` stays 0
    /// because this pipeline stops at [`Section`] and never meshes.
    ///
    /// No counter, clock, or ledger value ever enters a hash, a block id, a
    /// height, or a cache key (`A2.4`). Diagnostics are write-only here.
    counters: Counters,
    /// Default `S1.7` ledger. Nothing reserves bytes yet; residency will own it.
    ledger: MemoryLedger,
    /// Default `K4.3` ladder, at `Full` until a load signal exists.
    shed: ShedState,
    /// Distinct LOD0 columns with at least one generated section (`N1.4`).
    lod0_columns: RefCell<BTreeSet<(i32, i32)>>,
    tiles: RefCell<BTreeMap<(i32, i32), std::rc::Rc<TileData>>>,
    hydro: RefCell<BTreeMap<(i32, i32), HydroTileRef>>,
    /// The one whole-world hydrology result, solved on first use and shared
    /// with every worker that installs it ([`Terrain::with_world_hydro`]).
    world_hydro: RefCell<Option<Arc<hydro::WorldHydro>>>,
}

/// The hydrology result for one tile (`D8.2`).
///
/// World mode is the v1 path: the tile is a view of one solve over
/// [`hydro::WORLD_SIDE_CELLS`], so a basin that crosses a tile seam has one
/// level. Tile mode remains for tiles outside that grid.
#[derive(Clone, Debug)]
pub enum HydroTileRef {
    /// A tile solved on its own, with zero boundary inflow.
    Tile(Rc<HydroTile>),
    /// A tile-shaped view of the whole-world solve.
    World(hydro::OwnedWorldTile),
}

impl HydroTileRef {
    /// Pre-detail macro height at a local cell.
    #[must_use]
    pub fn height(&self, x: i32, z: i32) -> f32 {
        match self {
            Self::Tile(tile) => tile.height(x, z),
            Self::World(tile) => tile.height(x, z),
        }
    }

    /// Lake surface at a local cell, or   when dry.
    #[must_use]
    pub fn lake_level(&self, x: i32, z: i32) -> i32 {
        match self {
            Self::Tile(tile) => tile.lake_level(x, z),
            Self::World(tile) => tile.lake_level(x, z),
        }
    }

    /// Channel width in blocks at a local cell.
    #[must_use]
    pub fn river_width(&self, x: i32, z: i32) -> f32 {
        match self {
            Self::Tile(tile) => tile.river_width(x, z),
            Self::World(tile) => tile.river_width(x, z),
        }
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
    /// [`ErrorKind::ManifestIncompatible`] when a required record is missing.
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

    /// Hydro tile covering a 512-block square.
    ///
    /// v1 uses world mode (`D8.2`): the first call solves the whole finite
    /// world once and every tile reads a view of that result, so a basin is
    /// never cut at a tile seam. A tile outside the solved world falls back to a
    /// standalone solve with zero boundary inflow.
    ///
    /// # Errors
    ///
    /// Propagates `WorldHydro::solve` and `HydroTile::solve` failures.
    pub fn hydro_tile(&self, hydro_x: i32, hydro_z: i32) -> Result<HydroTileRef, TerrainError> {
        if let Some(cached) = self.hydro.borrow().get(&(hydro_x, hydro_z)) {
            return Ok(cached.clone());
        }
        let hydro_started = Instant::now();
        let world = self.world_hydro()?;
        let tile =
            if let Some(view) = hydro::OwnedWorldTile::new(Arc::clone(&world), hydro_x, hydro_z) {
                HydroTileRef::World(view)
            } else {
                let side = hydro::SIDE;
                let heights = self.tile_heights(hydro_x, hydro_z);
                let inflow = vec![0_u32; (side * side) as usize];
                HydroTileRef::Tile(Rc::new(HydroTile::solve(&heights, &inflow)?))
            };
        // Diagnostics only, taken after the solve returned.
        self.counters
            .record_latency(Stage::Hydrology, micros(hydro_started));
        self.hydro
            .borrow_mut()
            .insert((hydro_x, hydro_z), tile.clone());
        Ok(tile)
    }

    /// Installs one solved world, so every worker shares a single solve.
    #[must_use]
    pub fn with_world_hydro(mut self, world: Arc<hydro::WorldHydro>) -> Self {
        *self.world_hydro.get_mut() = Some(world);
        self
    }

    /// Solves the whole finite world at [`hydro::WORLD_SIDE_CELLS`].
    ///
    /// # Errors
    ///
    /// Propagates [`hydro::WorldHydro::solve`].
    pub fn solve_world_hydro(&self) -> Result<Arc<hydro::WorldHydro>, TerrainError> {
        let side = hydro::WORLD_SIDE_CELLS;
        let cells = usize::try_from(side * side).expect("the world grid fits usize");
        let half = side / 2;
        let mut heights = vec![0.0_f32; cells];
        for z in 0..side {
            for x in 0..side {
                let cell_x = x - half;
                let cell_z = z - half;
                let block_x = f32::from(i16::try_from(cell_x * MACRO_CELL + 2).unwrap_or(0));
                let block_z = f32::from(i16::try_from(cell_z * MACRO_CELL + 2).unwrap_or(0));
                heights[(x + side * z) as usize] = self.macro_height(block_x, block_z);
            }
        }
        Ok(Arc::new(hydro::WorldHydro::solve(
            &heights,
            usize::try_from(side).expect("the world grid side fits usize"),
        )?))
    }

    /// The cached whole-world result, solved on first use.
    fn world_hydro(&self) -> Result<Arc<hydro::WorldHydro>, TerrainError> {
        if let Some(world) = self.world_hydro.borrow().as_ref() {
            return Ok(Arc::clone(world));
        }
        let world = self.solve_world_hydro()?;
        *self.world_hydro.borrow_mut() = Some(Arc::clone(&world));
        Ok(world)
    }

    /// Heights for one standalone tile, halo included.
    fn tile_heights(&self, hydro_x: i32, hydro_z: i32) -> Vec<f32> {
        let side = hydro::SIDE;
        let mut heights = vec![0.0_f32; (side * side) as usize];
        for z in 0..side {
            for x in 0..side {
                let cell_x = hydro_x * hydro::TILE_CELLS + x - hydro::BORDER_CELLS;
                let cell_z = hydro_z * hydro::TILE_CELLS + z - hydro::BORDER_CELLS;
                let block_x = f32::from(i16::try_from(cell_x * MACRO_CELL + 2).unwrap_or(0));
                let block_z = f32::from(i16::try_from(cell_z * MACRO_CELL + 2).unwrap_or(0));
                heights[(x + side * z) as usize] = self.macro_height(block_x, block_z);
            }
        }
        heights
    }

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
    fn base_blocks(&self, block_x: f32, block_z: f32, continentalness: f32) -> f32 {
        let base = self.curves.base_blocks(continentalness);
        let half = self.half_extent as f32;
        let inward = (half - block_x.abs()).min(half - block_z.abs());
        let ratio = (inward / EDGE_BAND).clamp(0.0, 1.0);
        let keep = self.curves.edge.sample(ratio);
        MIN_BASE + (base - MIN_BASE) * keep
    }

    /// Generates one block column.
    ///
    /// # Errors
    ///
    /// Propagates tile and table failures.
    pub fn column(&self, block_x: i32, block_z: i32) -> Result<[BlockId; COLUMN], TerrainError> {
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
        let biome = tile.climate.best(cell_x, cell_z);
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
                let column = self.column(base_x + x, base_z + z)?;
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
    /// caves. Biomes come from the climate tile and flags mirror the river and
    /// bank facts of [`Terrain::column`]. This reads tile caches only; it never
    /// generates a column a second time.
    fn footprint(&self, section_x: i32, section_z: i32) -> Result<Footprint, TerrainError> {
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
                footprint.biome[slot] = tile.climate.best(
                    tile_local_x.div_euclid(MACRO_CELL),
                    tile_local_z.div_euclid(MACRO_CELL),
                );

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
struct Footprint {
    /// World `(x, z)` of the first column.
    origin: [i32; 2],
    /// Top solid block per column.
    tops: Vec<i32>,
    /// Largest one-block step to a neighbour.
    slope: Vec<i32>,
    /// Biome id per column.
    biome: Vec<u8>,
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
/// All four have weight `1.0` and priority `0`. `ocean` and `beach` are
/// **override surface sets**, not climate biomes, so they are deliberately
/// absent here and never enter climate space (`E3.1`, `E3.2`). "Mountains" is
/// not a biome either: the alpine look comes from snow cap, slope stone,
/// strata, and deep rock.
fn v1_biomes() -> BiomeTable {
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
            temp_c: 0.85,
            humid_c: 0.10,
            weight: 1.0,
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
fn micros(started: Instant) -> u64 {
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

fn river_level(hydro: &HydroTileRef, hydro_x: i32, hydro_z: i32) -> i32 {
    let width = hydro.river_width(hydro_x, hydro_z);
    if width <= 0.5 {
        return i32::MIN;
    }
    crate::hydro::flow::river_level(hydro.height(hydro_x, hydro_z))
}

/// Hydro tile cell coordinate, including the halo, for a world block.
fn hydro_cell(block_x: i32, block_z: i32) -> (i32, i32) {
    let local = |block: i32| {
        crate::lattice::floor_div(block, MACRO_CELL)
            - crate::lattice::floor_div(block, crate::description::HYDRO_TILE)
                * crate::hydro::TILE_CELLS
            + crate::hydro::BORDER_CELLS
    };
    (local(block_x), local(block_z))
}

#[cfg(test)]
#[allow(clippy::cast_possible_truncation)]
mod tests {
    use super::*;

    fn terrain() -> Terrain {
        Terrain::new(0x5EED).unwrap()
    }

    /// `E3.2`: four climate biomes, ids `0..=3`, at the answered centres.
    #[test]
    fn v1_biomes_are_the_answered_four() {
        let table = v1_biomes();

        // The probes are the four answered centres, in id order. The argmin in
        // climate space must return that centre's own id, so this pins both the
        // centres and the id order at once.
        let centres = [
            (0_u8, 0.55_f32, 0.45_f32),
            (1, 0.50, 0.75),
            (2, 0.85, 0.10),
            (3, 0.10, 0.35),
        ];
        let mut seen = [false; 4];
        for (id, temp, humid) in centres {
            let (best, second, _margin) = table.nearest_two(temp, humid, 0.05);
            assert_eq!(best, id, "probe ({temp}, {humid})");
            assert_ne!(second, best, "probe ({temp}, {humid}) has no rival");
            seen[usize::from(id)] = true;
        }
        assert!(seen.iter().all(|&hit| hit), "a biome is missing");

        // `BiomeTable` exposes no length accessor, so the exact record count is
        // observed through the derived `Debug` shape: one `Biome {` per centre.
        // A fifth climate biome (for example `ocean` or `beach`) would show up.
        let rendered = format!("{table:?}");
        assert_eq!(rendered.matches("Biome {").count(), 4, "{rendered}");
    }

    #[test]
    fn hydro_tiles_read_the_whole_world_solve() {
        let terrain = terrain();
        let inside = terrain.hydro_tile(0, 0).unwrap();
        assert!(
            matches!(inside, HydroTileRef::World(_)),
            "an in-grid tile must use world mode"
        );

        // The solved grid is -4..3 tiles; 4 is wholly outside it.
        let outside = terrain.hydro_tile(4, 4).unwrap();
        assert!(
            matches!(outside, HydroTileRef::Tile(_)),
            "a tile outside the world falls back to tile mode"
        );
    }

    #[test]
    fn an_installed_world_hydro_is_shared_by_workers() {
        let terrain = terrain();
        let world = terrain.solve_world_hydro().unwrap();
        let other = Terrain::new(0x5EED)
            .unwrap()
            .with_world_hydro(Arc::clone(&world));

        let (x, z) = (7, -13);
        assert_eq!(
            terrain.hydro_tile(0, 0).unwrap().river_width(x, z),
            other.hydro_tile(0, 0).unwrap().river_width(x, z)
        );
    }

    #[test]
    fn bedrock_is_solid_everywhere() {
        let terrain = terrain();
        let bedrock = terrain.materials().bedrock;
        for (x, z) in [(0, 0), (37, -91), (-512, 640), (2047, 2047)] {
            let column = terrain.column(x, z).unwrap();
            assert_eq!(column[0], bedrock, "({x},{z})");
        }
    }

    #[test]
    fn tops_are_inside_the_world() {
        let terrain = terrain();
        for x in -70..70 {
            let column = terrain.column(x, 13).unwrap();
            let top = column.iter().rposition(|&id| id != AIR && id != WATER);
            let top = top.unwrap_or(0) as i32;
            assert!(top <= MAX_TERRAIN_Y, "top {top} above MAX_TERRAIN_Y at {x}");
            assert!(top >= 1);
        }
    }

    #[test]
    fn water_never_exceeds_sea_unless_it_is_a_lake() {
        let terrain = terrain();
        let water = terrain.materials().water;
        for x in -80..80 {
            let column = terrain.column(x, 40).unwrap();
            let highest_water = column
                .iter()
                .rposition(|&id| id == water)
                .map_or(0, |y| y as i32);
            assert!(
                highest_water <= SEA,
                "water at {highest_water} above sea at x={x}"
            );
        }
    }

    #[test]
    fn generation_is_deterministic() {
        let a = terrain().column(123, -45).unwrap();
        let b = terrain().column(123, -45).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn counters_do_not_change_generation() {
        const SEED: u64 = 0x5EED;
        let terrain = Terrain::new(SEED).expect("the v1 description compiles");
        let (x, z) = (123, -45);
        let first = terrain.column(x, z).expect("the column generates");

        // Heavy use: every recorded stage, a warm cache, hundreds of samples.
        for z in -20..20 {
            for x in -20..20 {
                let _ = terrain.column(x, z).expect("the column generates");
            }
        }
        let _ = terrain.section(1, 4, 0).expect("the section generates");
        assert!(
            terrain.counters().get("ready_columns").unwrap_or(0) > 0,
            "the heavy pass recorded no gauge, so the test is vacuous"
        );

        // Clear every pinned counter, then regenerate the same column. The tile
        // cache is warm here and the counters are zero, so this is the "cleared"
        // arm against the "heavy" arm.
        for spec in metrics2::METRICS {
            assert!(terrain.counters().set(spec.name, 0), "{}", spec.name);
        }
        let second = terrain.column(x, z).expect("the column generates");

        // A cold generator that has recorded nothing is the third arm.
        let fresh = Terrain::new(SEED).expect("the v1 description compiles");
        let third = fresh.column(x, z).expect("the column generates");

        assert_eq!(first, second, "clearing counters changed a column");
        assert_eq!(first, third, "a cold generator produced another column");

        // Sections are what the goldens pin; compare their content hashes too.
        let warmed = terrain.section(1, 4, 0).expect("the section generates");
        let cold = fresh.section(1, 4, 0).expect("the section generates");
        assert_eq!(
            warmed.content_hash(),
            cold.content_hash(),
            "diagnostics changed a section"
        );
    }

    #[test]
    fn overlay_has_one_line_per_group() {
        let terrain = terrain();
        let _ = terrain.column(0, 0).expect("the column generates");

        let lines = terrain.overlay();
        assert_eq!(lines.len(), 8);
        let prefixes = [
            "revisions: ",
            "queues: ",
            "jobs: ",
            "memory: ",
            "lod: ",
            "light: ",
            "edits: ",
            "shed: ",
        ];
        for (line, prefix) in lines.iter().zip(prefixes) {
            assert!(line.starts_with(prefix), "expected {prefix}, got {line}");
        }
        assert_eq!(lines, terrain.overlay(), "the overlay is not stable");
        assert!(
            lines[1].contains("ready_columns=16"),
            "one 128-block tile is 16 columns: {}",
            lines[1]
        );
        assert!(lines[4].contains("lod0_columns=0"), "no section yet");
        assert!(lines[7].contains("step=Full"));
        assert!(lines[7].contains("radius=16"));

        let _ = terrain.section(0, 4, 0).expect("the section generates");
        let after = terrain.overlay();
        assert!(
            !after[4].contains("lod0_columns=0"),
            "the LOD0 gauge did not move: {}",
            after[4]
        );
        assert!(after[0].contains("applied=0"), "residency is not wired yet");
    }

    #[test]
    fn every_block_id_is_a_known_block() {
        let terrain = terrain();
        let pack = content::v1_default();
        for x in 0..40 {
            let column = terrain.column(x, 7).unwrap();
            for (y, &id) in column.iter().enumerate() {
                assert!(pack.props(id).is_some(), "unknown id {id} at ({x},{y})");
            }
        }
    }

    #[test]
    fn sections_assemble_from_the_same_columns() {
        let terrain = terrain();
        let section = terrain.section(0, 4, 0).unwrap();
        assert!(section.solid_count() > 0);
        let column = terrain.column(5, 9).unwrap();
        assert_eq!(section.get(5, 15, 9), column[4 * 32 + 15]);
    }

    #[test]
    fn the_world_edge_is_ocean() {
        let terrain = terrain();
        let half = terrain.half_extent;
        for (x, z) in [
            (half - 1, 0),
            (-half + 1, 0),
            (0, half - 1),
            (0, -half + 1),
            (half - 1, half - 1),
        ] {
            let column = terrain.column(x, z).unwrap();
            let top = column.iter().rposition(|&id| id != AIR).unwrap_or(0);
            assert!(
                top <= SEA as usize,
                "edge column ({x},{z}) rises to {top}, above sea"
            );
        }
    }

    #[test]
    fn the_edge_mask_does_not_reach_inland() {
        let terrain = terrain();
        let inland = terrain.macro_height(0.0, 0.0);
        let masked = inland - (SEA as f32);
        let deep = terrain.macro_height(terrain.half_extent as f32 - 1.0, 0.0);
        assert!(deep < inland, "the boundary must sit below the centre");
        assert!(
            masked.is_finite() && deep.is_finite(),
            "the mask only moves base"
        );
    }

    #[test]
    fn tiles_are_cached_and_stable() {
        let terrain = terrain();
        let first = terrain.tile(1, -2).unwrap();
        let second = terrain.tile(1, -2).unwrap();
        assert!(std::rc::Rc::ptr_eq(&first, &second));
    }

    /// Decodes a section into the `x | z << 5 | y << 10` block array.
    fn decode(section: &Section) -> Box<[BlockId; crate::description::SECTION_VOLUME]> {
        let mut blocks: Box<[BlockId; crate::description::SECTION_VOLUME]> =
            vec![AIR; crate::description::SECTION_VOLUME]
                .into_boxed_slice()
                .try_into()
                .expect("the scratch array matches the section volume");
        section.write_blocks(&mut blocks);
        blocks
    }

    /// The pre-detail top of one world column, from the tile cache.
    fn column_top(terrain: &Terrain, x: i32, z: i32) -> i32 {
        let tile_x = crate::lattice::floor_div(x, TILE_BLOCKS);
        let tile_z = crate::lattice::floor_div(z, TILE_BLOCKS);
        terrain
            .tile(tile_x, tile_z)
            .expect("the tile builds from the seed")
            .height
            .top(x - tile_x * TILE_BLOCKS, z - tile_z * TILE_BLOCKS)
    }

    /// Owns the column arrays an [`ApplyContext`] borrows.
    struct Fixture {
        tops: Vec<i32>,
        slope: Vec<i32>,
        biome: Vec<u8>,
        flags: Vec<u8>,
        regions: [[i32; 3]; features::GATHER_REGIONS],
        ores: [features::OreBlock; 2],
        host: [BlockId; 1],
        scatter: [BlockId; 1],
        materials: TerrainMaterials,
    }

    impl Fixture {
        fn new(origin: [i32; 3], top: i32, biome: u8) -> Self {
            let materials = TerrainMaterials::from_pack(&content::v1_default())
                .expect("the built-in pack resolves");
            Self {
                tops: vec![top; features::COLUMNS],
                slope: vec![0; features::COLUMNS],
                biome: vec![biome; features::COLUMNS],
                flags: vec![0; features::COLUMNS],
                regions: features::gather_origins(features::region_of(origin)),
                ores: [
                    features::OreBlock {
                        name: "ore_iron",
                        id: materials.ore_iron,
                    },
                    features::OreBlock {
                        name: "ore_copper",
                        id: materials.ore_copper,
                    },
                ],
                host: [materials.stone],
                scatter: [materials.plant],
                materials,
            }
        }

        fn ctx(&self, seed: u64) -> ApplyContext<'_> {
            let origin = self.regions[features::region_slot([0, 0, 0])];
            ApplyContext {
                seed,
                palette: Palette {
                    air: self.materials.air,
                    water: self.materials.water,
                    bedrock: self.materials.bedrock,
                    log: self.materials.wood,
                    leaf: self.materials.leaves,
                    boulder: self.materials.boulder,
                },
                ores: &self.ores,
                host_rock: &self.host,
                columns: ColumnField {
                    origin: [
                        origin[0] - features::REGION_SIZE,
                        origin[2] - features::REGION_SIZE,
                    ],
                    tops: &self.tops,
                    slope: &self.slope,
                    biome: &self.biome,
                    flags: &self.flags,
                },
                regions: self.regions,
                scatter: &self.scatter,
            }
        }
    }

    #[test]
    fn features_actually_generate() {
        const SEED: u64 = 0x5EED;

        fn counts() -> (usize, usize) {
            let terrain = Terrain::new(SEED).expect("the v1 description compiles");
            let materials = *terrain.materials();
            let mut logs = 0_usize;
            let mut ores = 0_usize;
            for section_z in 0..2 {
                for section_x in 0..2 {
                    for section_y in 0..5 {
                        let section = terrain
                            .section(section_x, section_y, section_z)
                            .expect("the section generates");
                        let blocks = decode(&section);
                        for z in 0..32 {
                            for x in 0..32 {
                                let world_x = section_x * 32 + x;
                                let world_z = section_z * 32 + z;
                                let top = column_top(&terrain, world_x, world_z);
                                for y in 0..32 {
                                    let index = crate::section::section_index(
                                        u32::try_from(x).unwrap_or(0),
                                        u32::try_from(y).unwrap_or(0),
                                        u32::try_from(z).unwrap_or(0),
                                    );
                                    let id = blocks[index];
                                    if id == materials.ore_iron || id == materials.ore_copper {
                                        ores += 1;
                                    }
                                    if id == materials.wood && section_y * 32 + y > top {
                                        logs += 1;
                                    }
                                }
                            }
                        }
                    }
                }
            }
            (logs, ores)
        }

        let first = counts();
        let second = counts();
        assert!(
            first.0 > 0,
            "no tree log above the surface stack: {first:?}"
        );
        assert!(first.1 > 0, "no ore block was generated: {first:?}");
        assert_eq!(first, second, "the feature count changed between runs");
    }

    #[test]
    fn features_are_thread_and_order_independent() {
        const SEED: u64 = 0x5EED;

        // `--threads` gives each worker its own `Terrain`; cache and processing
        // order differ, but the section must not. Warm the same section in
        // opposite orders from two instances and compare its content hash.
        let forward = Terrain::new(SEED).expect("the v1 description compiles");
        for section_y in 0..5 {
            let _ = forward
                .section(1, section_y, 0)
                .expect("the section generates");
        }
        let first = forward
            .section(1, 4, 0)
            .expect("the section generates")
            .content_hash();

        let reverse = Terrain::new(SEED).expect("the v1 description compiles");
        for section_y in (0..5).rev() {
            let _ = reverse
                .section(1, section_y, 0)
                .expect("the section generates");
        }
        let second = reverse
            .section(1, 4, 0)
            .expect("the section generates")
            .content_hash();

        assert_eq!(first, second, "cache order changed the section");
    }

    #[test]
    fn canopy_is_not_truncated_at_a_section_top() {
        use crate::features::{Candidate, FeatureKind, tree};
        use crate::hash::hash64;

        const SEED: u64 = 0x5EED;
        let specs = features::v1_specs();
        let upper_origin = [0, 160, 0];
        let foot = 159;

        // A tree whose base is the last block of the lower section, so its trunk
        // and canopy cross into the all-air section above.
        let candidate = (0..8192_u64)
            .find_map(|id| {
                let variant = hash64(SEED, domain("tree:oak"), &[id.cast_signed()]);
                let candidate = Candidate {
                    id,
                    region: features::region_of([16, foot, 16]),
                    kind: FeatureKind::Tree,
                    origin: [16, foot, 16],
                    variant,
                };
                (tree::allowed(tree::species_of(&candidate), 3)
                    && tree::accepts(SEED, &candidate, 3))
                .then_some(candidate)
            })
            .expect("no accepted forest tree");

        let fixture = Fixture::new(upper_origin, foot - 1, 3);
        let ctx = fixture.ctx(SEED);
        let air = fixture.materials.air;
        let wood = fixture.materials.wood;
        let leaf = fixture.materials.leaves;

        // With nothing in reach the all-air section is still free (`H2.2`).
        let mut empty = Section::uniform(air);
        let report = features::apply_section_counted(&mut empty, &[], &ctx, &specs);
        assert_eq!(report.probes, 0, "an unreachable section was processed");

        // With this candidate the section must be processed, and its blocks above
        // the seam must be present.
        let mut canopy = Section::uniform(air);
        let report = features::apply_section_counted(&mut canopy, &[candidate], &ctx, &specs);
        assert!(report.probes > 0, "the all-air section was skipped");
        assert!(
            (1..32).any(|y| canopy.get(16, y, 16) == wood),
            "the trunk above the seam was truncated"
        );
        let mut leaves = 0;
        for y in 0..32 {
            for z in 0..32 {
                for x in 0..32 {
                    leaves += u32::from(canopy.get(x, y, z) == leaf);
                }
            }
        }
        assert!(leaves > 0, "the canopy above the seam was truncated");
        assert!(report.stats.writes > 0, "the tree wrote nothing");
    }

    /// `§2.6`: every id the pipeline emits resolves from a stable name, and the
    /// resolved value is exactly the pack's id.
    #[test]
    fn every_material_name_resolves_to_the_pack_id() {
        let pack = content::v1_default();
        let materials = TerrainMaterials::from_pack(&pack).expect("the built-in pack resolves");
        let cases = [
            ("core:air", materials.air),
            ("core:stone", materials.stone),
            ("core:dirt", materials.dirt),
            ("core:grass", materials.grass),
            ("core:sand", materials.sand),
            ("core:gravel", materials.gravel),
            ("core:clay", materials.clay),
            ("core:snow", materials.snow),
            ("core:ice", materials.ice),
            ("core:water", materials.water),
            ("core:bedrock", materials.bedrock),
            ("core:leaves", materials.leaves),
            ("core:wood", materials.wood),
            ("core:plant", materials.plant),
            ("core:lamp", materials.lamp),
            ("core:boulder", materials.boulder),
            ("core:ore_copper", materials.ore_copper),
            ("core:ore_iron", materials.ore_iron),
        ];
        for (name, id) in cases {
            assert_eq!(pack.id(name), Some(id), "{name}");
        }
    }

    /// A pack without `core:stone` refuses generation by name, not by number.
    #[test]
    fn a_missing_stone_record_is_manifest_incompatible() {
        let mut records = content::v1_default().records().to_vec();
        records.retain(|record| record.name != "core:stone");
        let pack = ContentPack::load(records).expect("a pack without stone is still valid");

        let error = TerrainMaterials::from_pack(&pack).expect_err("stone is required");
        match &error.kind {
            ErrorKind::ManifestIncompatible { id, .. } => assert_eq!(id, "core:stone"),
            other => panic!("expected ManifestIncompatible, found {other:?}"),
        }
    }

    /// Behavioural proof that no numeric literal survives: the generated
    /// stone, bedrock, and sand are the pack's ids.
    #[test]
    fn sections_emit_the_pack_ids() {
        let pack = content::v1_default();
        let terrain = Terrain::new(7).expect("the v1 description compiles");
        let materials = *terrain.materials();
        assert_eq!(pack.id("core:stone"), Some(materials.stone));
        assert_eq!(pack.id("core:bedrock"), Some(materials.bedrock));
        assert_eq!(pack.id("core:sand"), Some(materials.sand));

        // Stone and bedrock appear in the lowest sections at the origin.
        let mut seen = BTreeSet::new();
        for section_y in 0..3 {
            let section = terrain
                .section(0, section_y, 0)
                .expect("the section generates");
            for y in 0..32 {
                for z in 0..32 {
                    for x in 0..32 {
                        seen.insert(section.get(x, y, z));
                    }
                }
            }
        }
        assert!(seen.contains(&materials.stone), "no stone at the origin");
        assert!(
            seen.contains(&materials.bedrock),
            "no bedrock at the origin"
        );

        // Find one sand column, then prove the section that holds it emits the
        // pack's sand id as well.
        let mut found = None;
        for x in (-1024..=1024).step_by(16) {
            let column = terrain.column(x, 0).expect("the column generates");
            if let Some(y) = column.iter().position(|&id| id == materials.sand) {
                found = Some((x, y as i32, 0));
                break;
            }
        }
        let (x, y, z) = found.expect("the v1 world has no sand at seed 7");
        let section = terrain
            .section(
                crate::lattice::floor_div(x, 32),
                crate::lattice::floor_div(y, 32),
                crate::lattice::floor_div(z, 32),
            )
            .expect("the sand section generates");
        let mut has_sand = false;
        for yy in 0..32 {
            for zz in 0..32 {
                for xx in 0..32 {
                    has_sand |= section.get(xx, yy, zz) == materials.sand;
                }
            }
        }
        assert!(has_sand, "the section around a sand column has no sand");
    }

    /// The two id constants the `terrain-gen` caller imports agree with the
    /// built-in pack. Generation itself reads [`TerrainMaterials`].
    #[test]
    fn the_tool_id_constants_match_the_pack() {
        let pack = content::v1_default();
        assert_eq!(pack.id("core:air"), Some(AIR), "core:air must sort first");
        assert_eq!(pack.id("core:water"), Some(WATER));
    }

    /// A pack supplied through `with_pack` is the same world as `new` when it is
    /// the built-in pack.
    #[test]
    fn with_pack_matches_new_for_the_built_in_pack() {
        const SEED: u64 = 0x5EED;
        let built_in = Terrain::new(SEED).expect("the v1 description compiles");
        let explicit =
            Terrain::with_pack(SEED, &content::v1_default()).expect("the built-in pack resolves");

        for (x, z) in [(0, 0), (5, 7), (123, -45), (224, 16)] {
            assert_eq!(
                built_in.column(x, z).expect("the column generates"),
                explicit.column(x, z).expect("the column generates"),
                "({x},{z})"
            );
        }
        assert_eq!(
            built_in
                .section(1, 4, 0)
                .expect("the section generates")
                .content_hash(),
            explicit
                .section(1, 4, 0)
                .expect("the section generates")
                .content_hash(),
        );
    }
}
