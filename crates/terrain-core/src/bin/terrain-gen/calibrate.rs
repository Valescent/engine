use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::thread;

use terrain_core::climate::smoothstep01;
use terrain_core::description::{self, MACRO_CELL, TILE_BLOCKS};
use terrain_core::error::TerrainError;
use terrain_core::lattice::floor_div;
use terrain_core::pipeline::Terrain;

use crate::cli::{Cursor, Options, Order, number, permutation};
use crate::internal;
use crate::output::{CliFailure, EXIT_INVARIANT, EXIT_OK, report};

// ---------------------------------------------------------------------------
// calibrate-blend
// ---------------------------------------------------------------------------

/// Default anchor stride, in blocks. Must be a multiple of [`MACRO_CELL`].
const DEFAULT_BLEND_STEP: i32 = 16;
/// How far a transect marches from its anchor in either direction, in blocks.
const BLEND_MARCH_BLOCKS: i32 = 256;
/// The deep end of a measured transect: `w = 0.95` (`E3.6`).
const BLEND_END_WEIGHT: f32 = 0.95;
/// The largest sampled `w` still treated as the `w = 0.5` border.
///
/// `q` is quantised, so a bilinear sample only reaches exactly `0.5` where all
/// four surrounding cells have `q = 0`. A local minimum at `w <= 0.51` (`q` up
/// to about 17) cannot be more than one block from the true border.
const BLEND_BORDER_WEIGHT: f32 = 0.51;
/// Lower bound of the accepted per-pair median, in blocks.
const BLEND_MEDIAN_MIN: f64 = 20.0;
/// Upper bound of the accepted per-pair median, in blocks.
const BLEND_MEDIAN_MAX: f64 = 28.0;
/// Lower bound of the accepted per-pair p10, in blocks.
const BLEND_P10_MIN: f64 = 8.0;
/// Upper bound of the accepted per-pair p90, in blocks.
const BLEND_P90_MAX: f64 = 64.0;
/// Transects a pair needs before its acceptance is judged; fewer are reported
/// only (`E3.6`).
const BLEND_MIN_SAMPLES: usize = 20;
/// Cells per field tile, `TILE_BLOCKS / MACRO_CELL` (`B4.2`).
const CLIMATE_CELLS_PER_TILE: i32 = TILE_BLOCKS / MACRO_CELL;

/// `w = 0.5 + 0.5 * smoothstep01(q / 255)`, the exact expression
/// `ClimateTile::biome_pick` evaluates (`E3.3`, `E3.5`).
fn blend_weight(q: f32) -> f32 {
    0.5 + 0.5 * smoothstep01(q / 255.0)
}

#[derive(Clone, Debug)]
struct PairSummary {
    low: u8,
    high: u8,
    samples: usize,
    median: f64,
    p10: f64,
    p90: f64,
}

impl PairSummary {
    fn assessed(&self) -> bool {
        self.samples >= BLEND_MIN_SAMPLES
    }

    fn accepted(&self) -> bool {
        self.assessed()
            && (BLEND_MEDIAN_MIN..=BLEND_MEDIAN_MAX).contains(&self.median)
            && self.p10 >= BLEND_P10_MIN
            && self.p90 <= BLEND_P90_MAX
    }
}

struct BlendOutcome {
    seeds: usize,
    from: (i32, i32),
    size: i32,
    step: i32,
    pairs: Vec<PairSummary>,
}

impl BlendOutcome {
    fn accepted(&self) -> bool {
        let assessed: Vec<&PairSummary> =
            self.pairs.iter().filter(|pair| pair.assessed()).collect();
        !assessed.is_empty() && assessed.iter().all(|pair| pair.accepted())
    }
}

pub(crate) fn cmd_calibrate_blend(args: &[String], options: &Options) -> u8 {
    match calibrate_outcome(args, options) {
        Ok(outcome) => {
            print_calibrate(&outcome, options.json);
            if outcome.accepted() {
                EXIT_OK
            } else {
                EXIT_INVARIANT
            }
        }
        Err(failure) => report(&failure, options.json, "calibrate-blend"),
    }
}

fn calibrate_outcome(args: &[String], options: &Options) -> Result<BlendOutcome, CliFailure> {
    let mut cursor = Cursor::new(args);
    let mut seeds = None;
    let mut from = None;
    let mut size = None;
    let mut step = None;
    while let Some(argument) = cursor.arg() {
        match argument {
            "--seeds" => seeds = Some(number::<usize>(cursor.value("seeds")?, "seeds")?),
            "--from" => {
                let x = number::<i32>(cursor.value("from x")?, "from x")?;
                let z = number::<i32>(cursor.value("from z")?, "from z")?;
                from = Some((x, z));
            }
            "--size" => size = Some(number::<i32>(cursor.value("size")?, "size")?),
            "--step" => step = Some(number::<i32>(cursor.value("step")?, "step")?),
            other => {
                return Err(CliFailure::Usage(format!(
                    "unexpected argument {other:?} for calibrate-blend"
                )));
            }
        }
    }
    let seeds =
        seeds.ok_or_else(|| CliFailure::Usage("calibrate-blend needs --seeds <n>".to_owned()))?;
    if seeds == 0 {
        return Err(CliFailure::Usage("--seeds must be at least 1".to_owned()));
    }
    let from = from.unwrap_or((
        -description::default_v1().world_half_extent,
        -description::default_v1().world_half_extent,
    ));
    let size = size.unwrap_or(description::default_v1().world_half_extent * 2);
    if size <= 0 {
        return Err(CliFailure::Usage(format!(
            "--size must be at least 1, found {size}"
        )));
    }
    let step = step.unwrap_or(DEFAULT_BLEND_STEP);
    if step <= 0 || step % MACRO_CELL != 0 {
        return Err(CliFailure::Usage(format!(
            "--step must be a positive multiple of {MACRO_CELL}, found {step}"
        )));
    }
    if step > size {
        return Err(CliFailure::Usage(format!(
            "--step {step} is larger than --size {size}"
        )));
    }
    let half = i64::from(description::default_v1().world_half_extent);
    let end_x = i64::from(from.0) + i64::from(size);
    let end_z = i64::from(from.1) + i64::from(size);
    if i64::from(from.0) < -half || i64::from(from.1) < -half || end_x > half || end_z > half {
        return Err(CliFailure::Usage(format!(
            "window {}..{} x {}..{} is outside the {}-block half extent",
            from.0, end_x, from.1, end_z, half
        )));
    }

    let seed_values: Vec<u64> = (1..=seeds)
        .map(|seed| u64::try_from(seed).unwrap_or(u64::MAX))
        .collect();
    let widths = blend_seeds(
        &seed_values,
        from,
        size,
        step,
        options.threads,
        options.order,
    )?;
    Ok(BlendOutcome {
        seeds,
        from,
        size,
        step,
        pairs: summarise_pairs(&widths),
    })
}

/// Measures every seed, then merges the per-pair widths in canonical seed order.
fn blend_seeds(
    seeds: &[u64],
    from: (i32, i32),
    size: i32,
    step: i32,
    threads: usize,
    order: Order,
) -> Result<PairWidths, TerrainError> {
    if seeds.is_empty() {
        return Ok(BTreeMap::new());
    }
    let shuffled_key = seeds.first().copied().unwrap_or(0);
    let sequence = permutation(seeds.len(), order, shuffled_key);
    let workers = threads.clamp(1, sequence.len());
    let chunk = sequence.len().div_ceil(workers);
    let mut slots: Vec<Option<PairWidths>> = (0..seeds.len()).map(|_| None).collect();

    thread::scope(|scope| {
        let mut handles = Vec::with_capacity(workers);
        for indices in sequence.chunks(chunk) {
            handles.push(
                scope.spawn(move || -> Result<Vec<PairWidths>, TerrainError> {
                    let mut produced = Vec::with_capacity(indices.len());
                    for &index in indices {
                        produced.push(blend_seed(seeds[index], from, size, step)?);
                    }
                    Ok(produced)
                }),
            );
        }
        for (indices, handle) in sequence.chunks(chunk).zip(handles) {
            let produced = match handle.join() {
                Ok(result) => result?,
                Err(_) => return Err(internal("worker_panic")),
            };
            for (index, result) in indices.iter().zip(produced) {
                slots[*index] = Some(result);
            }
        }
        Ok(())
    })?;

    let mut merged: PairWidths = BTreeMap::new();
    for slot in slots.into_iter().flatten() {
        for (pair, mut widths) in slot {
            merged.entry(pair).or_default().append(&mut widths);
        }
    }
    Ok(merged)
}

/// The anchor grid, in cells: every `step / MACRO_CELL` cells whose centre
/// falls inside `[from, from + size)`.
fn anchor_cells(from: (i32, i32), size: i32, step: i32) -> Vec<(i32, i32)> {
    let stride = step / MACRO_CELL;
    let min_x = ceil_div(from.0 - 2, MACRO_CELL);
    let max_x = floor_div(from.0 + size - 3, MACRO_CELL);
    let min_z = ceil_div(from.1 - 2, MACRO_CELL);
    let max_z = floor_div(from.1 + size - 3, MACRO_CELL);
    if min_x > max_x || min_z > max_z {
        return Vec::new();
    }
    let first_x = min_x.div_euclid(stride) * stride;
    let first_z = min_z.div_euclid(stride) * stride;
    let mut cells = Vec::new();
    let mut cell_z = first_z;
    while cell_z <= max_z {
        let mut cell_x = first_x;
        while cell_x <= max_x {
            cells.push((cell_x, cell_z));
            cell_x += stride;
        }
        cell_z += stride;
    }
    cells
}

fn ceil_div(value: i32, divisor: i32) -> i32 {
    -floor_div(-value, divisor)
}

fn blend_seed(
    seed: u64,
    from: (i32, i32),
    size: i32,
    step: i32,
) -> Result<PairWidths, TerrainError> {
    let terrain = Terrain::new(seed)?;
    let half = description::default_v1().world_half_extent;
    let bounds = world_cell_bounds();
    let mut pairs: PairWidths = BTreeMap::new();
    for (cell_x, cell_z) in anchor_cells(from, size, step) {
        let anchor_x = cell_x * MACRO_CELL + 2;
        let anchor_z = cell_z * MACRO_CELL + 2;
        // Keep every sample and the 5x5 gradient stencil inside the finite
        // world. A transect that cannot reach 128 blocks from a border is not
        // measured.
        let reach = (half - 1 - anchor_x.abs()).min(half - 1 - anchor_z.abs());
        let march = BLEND_MARCH_BLOCKS.min(reach);
        if march < 128 {
            continue;
        }
        let Some(sample) = anchor_sample(&terrain, cell_x, cell_z)? else {
            continue;
        };
        let profile =
            transect_profile(cell_x, cell_z, sample.direction, march, bounds, &|x, z| {
                Ok(f32::from(cell_margin(&terrain, x, z)?))
            })?;
        if let Some(width) = transect_width(&profile) {
            pairs
                .entry((sample.low, sample.high))
                .or_default()
                .push(width);
        }
    }
    Ok(pairs)
}

struct AnchorSample {
    low: u8,
    high: u8,
    /// Unit direction of increasing margin, in block space.
    direction: (f64, f64),
}

/// One cell's contribution to a blend measurement: its two nearest biomes and
/// the quantised margin between them.
#[derive(Clone, Copy, Debug)]
struct CellPicks {
    best: u8,
    second: u8,
    margin: u8,
}

/// Reads one anchor from the terrain's climate tiles.
fn anchor_sample(
    terrain: &Terrain,
    cell_x: i32,
    cell_z: i32,
) -> Result<Option<AnchorSample>, TerrainError> {
    anchor_from_cells(cell_x, cell_z, &|x, z| cell_picks(terrain, x, z))
}

/// Selects one transect anchor from any margin field.
///
/// The anchor must be inside the blend band and have two distinct biomes. The
/// direction comes from [`margin_direction`].
fn anchor_from_cells<F>(
    cell_x: i32,
    cell_z: i32,
    picks: &F,
) -> Result<Option<AnchorSample>, TerrainError>
where
    F: Fn(i32, i32) -> Result<CellPicks, TerrainError>,
{
    let here = picks(cell_x, cell_z)?;
    if here.best == here.second {
        return Ok(None);
    }
    if blend_weight(f32::from(here.margin)) >= BLEND_END_WEIGHT {
        return Ok(None);
    }
    let Some(direction) = margin_direction(cell_x, cell_z, here.best, here.second, picks)? else {
        return Ok(None);
    };
    Ok(Some(AnchorSample {
        low: here.best.min(here.second),
        high: here.best.max(here.second),
        direction,
    }))
}

fn cell_picks(terrain: &Terrain, cell_x: i32, cell_z: i32) -> Result<CellPicks, TerrainError> {
    let tile_x = floor_div(cell_x, CLIMATE_CELLS_PER_TILE);
    let tile_z = floor_div(cell_z, CLIMATE_CELLS_PER_TILE);
    let local_x = cell_x - tile_x * CLIMATE_CELLS_PER_TILE;
    let local_z = cell_z - tile_z * CLIMATE_CELLS_PER_TILE;
    let tile = terrain.tile(tile_x, tile_z)?;
    Ok(CellPicks {
        best: tile.climate.best(local_x, local_z),
        second: tile.climate.second(local_x, local_z),
        margin: tile.climate.margin(local_x, local_z),
    })
}

/// Direction of increasing margin from a least-squares plane through same-side cells.
///
/// The quantised margin is a V across a border: it falls to `0` at the border and
/// rises on both sides, so a plane fitted through both sides has a crease in it and a
/// direction that can be nearly tangential. Only cells with the anchor's ordered
/// `(best, second)` pair are used, which keeps one side of the V, and saturated cells
/// (`q = 255`) are dropped because the distance no longer grows there. The plane has
/// an intercept, so an asymmetric sample does not bias the gradient. The window starts
/// at `5 x 5` and widens to `7 x 7`, then `9 x 9`, until four cells qualify.
#[allow(clippy::similar_names)]
fn margin_direction<F>(
    cell_x: i32,
    cell_z: i32,
    best: u8,
    second: u8,
    picks: &F,
) -> Result<Option<(f64, f64)>, TerrainError>
where
    F: Fn(i32, i32) -> Result<CellPicks, TerrainError>,
{
    for radius in [2_i32, 3, 4] {
        let mut count = 0.0_f32;
        let mut sum_x = 0.0_f32;
        let mut sum_z = 0.0_f32;
        let mut sum_q = 0.0_f32;
        let mut square_x = 0.0_f32;
        let mut square_z = 0.0_f32;
        let mut cross_xz = 0.0_f32;
        let mut cross_xq = 0.0_f32;
        let mut cross_zq = 0.0_f32;
        for delta_z in -radius..=radius {
            for delta_x in -radius..=radius {
                let pick = picks(cell_x + delta_x, cell_z + delta_z)?;
                if pick.best != best || pick.second != second || pick.margin == u8::MAX {
                    continue;
                }
                let x = f32::from(i8::try_from(delta_x).unwrap_or(0));
                let z = f32::from(i8::try_from(delta_z).unwrap_or(0));
                let q = f32::from(pick.margin);
                count += 1.0;
                sum_x += x;
                sum_z += z;
                sum_q += q;
                square_x += x * x;
                square_z += z * z;
                cross_xz += x * z;
                cross_xq += x * q;
                cross_zq += z * q;
            }
        }
        if count < 4.0 {
            continue;
        }
        let mean_x = sum_x / count;
        let mean_z = sum_z / count;
        let mean_q = sum_q / count;
        let var_x = square_x - count * mean_x * mean_x;
        let var_z = square_z - count * mean_z * mean_z;
        let cov_xz = cross_xz - count * mean_x * mean_z;
        let cov_xq = cross_xq - count * mean_x * mean_q;
        let cov_zq = cross_zq - count * mean_z * mean_q;
        let determinant = var_x * var_z - cov_xz * cov_xz;
        if determinant <= f32::EPSILON * var_x * var_z {
            continue;
        }
        let gradient_x = (cov_xq * var_z - cov_zq * cov_xz) / determinant;
        let gradient_z = (cov_zq * var_x - cov_xq * cov_xz) / determinant;
        let magnitude = (gradient_x * gradient_x + gradient_z * gradient_z).sqrt();
        if magnitude == 0.0 || !magnitude.is_finite() {
            continue;
        }
        return Ok(Some((
            f64::from(gradient_x / magnitude),
            f64::from(gradient_z / magnitude),
        )));
    }
    Ok(None)
}

/// Cell bounds that keep every bilinear read inside the finite world.
///
/// The cell centre for cell `c` is block `4c + 2`, so the last centre is block
/// `half - 2`.
fn world_cell_bounds() -> (f64, f64) {
    let half = description::default_v1().world_half_extent;
    (
        f64::from(ceil_div(-half + 1 - 2, MACRO_CELL)),
        f64::from(floor_div(half - 1 - 2, MACRO_CELL)),
    )
}

/// One transect as `(block offset, w)` pairs, sampled one block apart.
///
/// `direction` is a unit vector in block space; one block is a quarter of a
/// cell, so a block offset moves the cell coordinate by `offset / 4`.
fn transect_profile<F>(
    cell_x: i32,
    cell_z: i32,
    direction: (f64, f64),
    march: i32,
    bounds: (f64, f64),
    margin_at: &F,
) -> Result<Vec<(f64, f32)>, TerrainError>
where
    F: Fn(i32, i32) -> Result<f32, TerrainError>,
{
    let mut profile = Vec::with_capacity(usize::try_from(march * 2 + 1).unwrap_or(0));
    for offset in -march..=march {
        let cell_sample_x = f64::from(cell_x) + direction.0 * f64::from(offset) / 4.0;
        let cell_sample_z = f64::from(cell_z) + direction.1 * f64::from(offset) / 4.0;
        let q = bilinear_over(cell_sample_x, cell_sample_z, bounds, margin_at)?;
        profile.push((f64::from(offset), blend_weight(q)));
    }
    Ok(profile)
}

/// Bilinear interpolation over the cell lattice, clamped to `bounds`.
#[allow(clippy::cast_possible_truncation)]
fn bilinear_over<F>(
    cell_x: f64,
    cell_z: f64,
    bounds: (f64, f64),
    margin_at: &F,
) -> Result<f32, TerrainError>
where
    F: Fn(i32, i32) -> Result<f32, TerrainError>,
{
    let (min_cell, max_cell) = bounds;
    let cell_x = cell_x.clamp(min_cell, max_cell);
    let cell_z = cell_z.clamp(min_cell, max_cell);
    let x0 = cell_x.floor() as i32;
    let z0 = cell_z.floor() as i32;
    let x1 = (x0 + 1).min(max_cell as i32);
    let z1 = (z0 + 1).min(max_cell as i32);
    let fx = (cell_x - f64::from(x0)) as f32;
    let fz = (cell_z - f64::from(z0)) as f32;
    let q00 = margin_at(x0, z0)?;
    let q10 = margin_at(x1, z0)?;
    let q01 = margin_at(x0, z1)?;
    let q11 = margin_at(x1, z1)?;
    let top = q00 + (q10 - q00) * fx;
    let bottom = q01 + (q11 - q01) * fx;
    Ok(top + (bottom - top) * fz)
}

fn cell_margin(terrain: &Terrain, cell_x: i32, cell_z: i32) -> Result<u8, TerrainError> {
    let tile_x = floor_div(cell_x, CLIMATE_CELLS_PER_TILE);
    let tile_z = floor_div(cell_z, CLIMATE_CELLS_PER_TILE);
    let tile = terrain.tile(tile_x, tile_z)?;
    Ok(tile.climate.margin(
        cell_x - tile_x * CLIMATE_CELLS_PER_TILE,
        cell_z - tile_z * CLIMATE_CELLS_PER_TILE,
    ))
}

/// The distance between the `w = 0.5` and `w = 0.95` crossings, doubled.
///
/// `profile` is `(block offset, w)`, sorted by offset, with offset `0` at the
/// anchor. The anchor is inside the blend band (`w >= 0.5`, `w < 0.95`), so the
/// rising crossing lies at a non-negative offset and the border crossing at a
/// non-positive one. Returns `None` when either crossing is outside the
/// sampled profile.
///
/// `q` is quantised, so `w = 0.5` is only reached exactly at `q = 0`; between
/// cell centres the bilinear minimum can sit a fraction of a block above it.
/// The border is therefore the first local minimum behind the anchor, accepted
/// when it is within [`BLEND_BORDER_WEIGHT`], which bounds the error below one
/// block.
fn transect_width(profile: &[(f64, f32)]) -> Option<f64> {
    let center = profile.iter().position(|(offset, _)| *offset == 0.0)?;

    let mut previous: Option<(f64, f32)> = None;
    let mut rising = None;
    for &(offset, weight) in &profile[center..] {
        if let Some((low_offset, low_weight)) = previous
            && low_weight < BLEND_END_WEIGHT
            && weight >= BLEND_END_WEIGHT
        {
            rising = Some(crossing(
                (low_offset, low_weight),
                (offset, weight),
                BLEND_END_WEIGHT,
            ));
        }
        if weight >= BLEND_END_WEIGHT {
            break;
        }
        previous = Some((offset, weight));
    }
    let rising = rising?;

    let (_, anchor_weight) = profile[center];
    let mut border = (0.0_f64, anchor_weight);
    for &(offset, weight) in profile[..center].iter().rev() {
        if weight < border.1 {
            border = (offset, weight);
        } else if weight > border.1 {
            // The first rise behind the anchor closes the first local minimum.
            // Scanning past it would pick an unrelated border further back.
            break;
        }
    }
    if border.1 > BLEND_BORDER_WEIGHT {
        return None;
    }

    let width = (rising - border.0) * 2.0;
    (width > 0.0 && width.is_finite()).then_some(width)
}

fn crossing(a: (f64, f32), b: (f64, f32), target: f32) -> f64 {
    let span = b.1 - a.1;
    let fraction = if span == 0.0 {
        0.0
    } else {
        f64::from((target - a.1) / span).clamp(0.0, 1.0)
    };
    a.0 + (b.0 - a.0) * fraction
}

fn summarise_pairs(widths: &PairWidths) -> Vec<PairSummary> {
    widths
        .iter()
        .map(|(&(first, second), values)| {
            let mut sorted = values.clone();
            sorted.sort_by(f64::total_cmp);
            PairSummary {
                low: first.min(second),
                high: first.max(second),
                samples: sorted.len(),
                median: percentile(&sorted, 0.5),
                p10: percentile(&sorted, 0.1),
                p90: percentile(&sorted, 0.9),
            }
        })
        .collect()
}

/// Linear interpolation between order statistics: `index = p * (n - 1)`.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn percentile(sorted: &[f64], fraction: f64) -> f64 {
    let Some(last) = sorted.len().checked_sub(1) else {
        return 0.0;
    };
    if last == 0 {
        return sorted[0];
    }
    let position = fraction.clamp(0.0, 1.0) * last as f64;
    let low = position.floor() as usize;
    let high = position.ceil() as usize;
    if low == high {
        return sorted[low];
    }
    let blend = position - low as f64;
    sorted[low] + (sorted[high] - sorted[low]) * blend
}

fn fixed2(value: f64) -> String {
    let value = if value == 0.0 { 0.0 } else { value };
    format!("{value:.2}")
}

fn calibrate_json(outcome: &BlendOutcome) -> String {
    let BlendOutcome {
        seeds,
        from,
        size,
        step,
        pairs,
    } = outcome;
    let mut out = format!(
        "{{\"subcommand\":\"calibrate-blend\",\"seeds\":{seeds},\"from\":[{},{}],\"size\":{size},\"step\":{step},\"pairs\":[",
        from.0, from.1
    );
    for (index, pair) in pairs.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        let _ = write!(
            out,
            "{{\"pair\":[{},{}],\"samples\":{},\"median_blocks\":{},\"p10_blocks\":{},\"p90_blocks\":{},\"assessed\":{},\"ok\":{}}}",
            pair.low,
            pair.high,
            pair.samples,
            fixed2(pair.median),
            fixed2(pair.p10),
            fixed2(pair.p90),
            pair.assessed(),
            pair.accepted()
        );
    }
    let _ = write!(out, "],\"ok\":{}}}", outcome.accepted());
    out
}

fn print_calibrate(outcome: &BlendOutcome, json: bool) {
    let BlendOutcome {
        seeds,
        from,
        size,
        step,
        pairs,
    } = outcome;
    let accepted = outcome.accepted();
    if json {
        println!("{}", calibrate_json(outcome));
    } else {
        println!(
            "calibrate-blend seeds {seeds} from {},{} size {size} step {step}",
            from.0, from.1
        );
        println!("pair samples median p10 p90 ok");
        for pair in pairs {
            println!(
                "{}-{} {} {} {} {} {}",
                pair.low,
                pair.high,
                pair.samples,
                fixed2(pair.median),
                fixed2(pair.p10),
                fixed2(pair.p90),
                if !pair.assessed() {
                    "n/a"
                } else if pair.accepted() {
                    "yes"
                } else {
                    "no"
                }
            );
        }
        if accepted {
            println!("ok");
        } else {
            if pairs.is_empty() {
                eprintln!("calibrate-blend measured no biome-pair borders in this window");
            }
            for pair in pairs
                .iter()
                .filter(|pair| pair.assessed() && !pair.accepted())
            {
                eprintln!(
                    "blend pair {}-{} out of acceptance: median {} p10 {} p90 {} (need median {}..={}, p10 >= {}, p90 <= {})",
                    pair.low,
                    pair.high,
                    fixed2(pair.median),
                    fixed2(pair.p10),
                    fixed2(pair.p90),
                    fixed2(BLEND_MEDIAN_MIN),
                    fixed2(BLEND_MEDIAN_MAX),
                    fixed2(BLEND_P10_MIN),
                    fixed2(BLEND_P90_MAX)
                );
            }
        }
    }
}

/// Measured transect widths, bucketed by unordered biome pair.
type PairWidths = BTreeMap<(u8, u8), Vec<f64>>;

#[cfg(test)]
mod tests {
    use super::*;
    use terrain_core::climate::{Biome, BiomeTable};

    /// A synthetic transect: `w = 0.5` at and behind the border and a linear
    /// rise to `w = 0.95` at 24 blocks in front of it.
    #[allow(clippy::cast_possible_truncation)]
    fn synthetic_profile(half_width: f64) -> Vec<(f64, f32)> {
        (-64..=64)
            .map(|offset| {
                let distance = f64::from(offset);
                let weight = if distance <= 0.0 {
                    0.5
                } else if distance >= half_width {
                    0.95
                } else {
                    (0.5 + 0.45 * distance / half_width) as f32
                };
                (distance, weight)
            })
            .collect()
    }

    #[test]
    fn transect_width_doubles_the_half_width() {
        let width = transect_width(&synthetic_profile(24.0)).expect("a complete transect measures");
        assert!((width - 48.0).abs() < 1e-9, "measured {width}");

        let width = transect_width(&synthetic_profile(12.0)).expect("a complete transect measures");
        assert!((width - 24.0).abs() < 1e-9, "measured {width}");
    }

    #[test]
    fn transect_width_accepts_a_quantised_border() {
        // The bilinear minimum behind the anchor can sit just above 0.5. The
        // border tolerance bounds that error below one block.
        let mut profile = synthetic_profile(20.0);
        for sample in &mut profile {
            if sample.0 < 0.0 {
                sample.1 = 0.5004;
            }
        }
        let width = transect_width(&profile).expect("a near-border minimum measures");
        assert!((width - 40.0).abs() < 1.0, "measured {width}");
    }

    #[test]
    fn transect_width_rejects_a_saturated_profile() {
        let flat = vec![(-8.0, 0.8_f32), (0.0, 0.85), (8.0, 0.9)];
        assert_eq!(transect_width(&flat), None);
    }

    #[test]
    fn transect_width_stops_at_the_first_minimum_behind_the_anchor() {
        // The anchor is the nearest minimum (a tangential transect). A deeper,
        // unrelated minimum 40 blocks behind must not become the border.
        let profile: Vec<(f64, f32)> = (-64..=64)
            .map(|offset| {
                let distance = f32::from(i16::try_from(offset).unwrap_or(0));
                let weight = if offset == 0 {
                    0.505
                } else if offset >= 12 {
                    0.95
                } else if offset > 0 {
                    0.5 + 0.45 * distance / 12.0
                } else if offset == -40 {
                    0.50
                } else {
                    0.7
                };
                (f64::from(distance), weight)
            })
            .collect();
        // The old search skipped the immediate rise and measured to the far dip.
        let width = transect_width(&profile).expect("the anchor minimum measures");
        assert!((width - 24.0).abs() < 1e-6, "measured {width}");
    }

    /// A synthetic two-centre climate for auditing the measurement itself.
    ///
    /// Temperature rises linearly with block `x` through the border at block 2
    /// (the centre of climate cell 0) and humidity is constant, so the `d2`
    /// margin is `m(x) = 2 * 0.6 * gradient * |x - 2|` before it saturates.
    fn analytic_picks(
        gradient: f32,
        blend_width: f32,
    ) -> impl Fn(i32, i32) -> Result<CellPicks, TerrainError> {
        let table = BiomeTable::new(vec![
            Biome {
                id: 0,
                temp_c: 0.2,
                humid_c: 0.5,
                weight: 1.0,
                priority: 0,
            },
            Biome {
                id: 1,
                temp_c: 0.8,
                humid_c: 0.5,
                weight: 1.0,
                priority: 0,
            },
        ])
        .expect("the audit table is valid");
        move |cell_x, _cell_z| {
            let block_x = cell_x * MACRO_CELL + 2;
            let temperature = 0.5 + gradient * f32::from(i16::try_from(block_x - 2).unwrap_or(0));
            let (best, second, margin) = table.nearest_two(temperature, 0.5, blend_width);
            Ok(CellPicks {
                best,
                second,
                margin,
            })
        }
    }

    /// The tool must measure the closed-form width of a straight, analytic
    /// border. `w` reaches 0.95 at `q = 206`; that margin, divided by the known
    /// `d2` gradient, is the distance to the border, and the tool doubles it.
    #[test]
    fn transect_measures_the_analytic_blend_width() {
        let gradient = 0.0028_f32;
        let blend_width = 0.05_f32;
        let picks = analytic_picks(gradient, blend_width);
        let (cell_x, cell_z) = (2, 0);
        let anchor = anchor_from_cells(cell_x, cell_z, &picks)
            .expect("the analytic field is readable")
            .expect("cell 2 is inside the blend band");
        assert!(
            anchor.direction.0 > 0.999 && anchor.direction.1.abs() < 1e-3,
            "direction {:?}",
            anchor.direction
        );

        let profile = transect_profile(
            cell_x,
            cell_z,
            anchor.direction,
            64,
            world_cell_bounds(),
            &|x, z| Ok(f32::from(picks(x, z)?.margin)),
        )
        .expect("the analytic transect is readable");
        let width = transect_width(&profile).expect("the analytic transect is complete");

        let reached = f64::from(blend_width) * (206.0 - 0.5) / 255.0;
        let expected = 2.0 * reached / (2.0 * 0.6 * f64::from(gradient));
        assert!(
            (width - expected).abs() < 0.75,
            "measured {width}, closed form {expected}"
        );
    }

    #[test]
    fn percentiles_interpolate_between_order_statistics() {
        let values: Vec<f64> = (0..=10).map(f64::from).collect();
        assert!((percentile(&values, 0.5) - 5.0).abs() < 1e-12);
        assert!((percentile(&values, 0.1) - 1.0).abs() < 1e-12);
        assert!((percentile(&values, 0.9) - 9.0).abs() < 1e-12);
        assert_eq!(percentile(&[], 0.5), 0.0);
        assert_eq!(percentile(&[7.0], 0.9), 7.0);
    }

    #[test]
    fn pair_summaries_keep_the_unordered_pair_and_its_acceptance() {
        let mut widths = PairWidths::new();
        widths.insert((3, 1), vec![24.0; 32]);
        widths.insert((0, 2), vec![60.0; 8]);
        let summaries = summarise_pairs(&widths);
        assert_eq!(summaries.len(), 2, "one summary per unordered pair");
        assert_eq!((summaries[0].low, summaries[0].high), (0, 2));
        assert_eq!(summaries[0].samples, 8);
        assert!(
            !summaries[0].assessed(),
            "eight transects are reported only"
        );
        assert_eq!((summaries[1].low, summaries[1].high), (1, 3));
        assert!(summaries[1].accepted(), "a 24-block median is accepted");
    }

    #[test]
    fn calibrate_json_is_sorted_and_fixed_precision() {
        let outcome = BlendOutcome {
            seeds: 3,
            from: (0, 0),
            size: 512,
            step: 16,
            pairs: vec![
                PairSummary {
                    low: 0,
                    high: 1,
                    samples: 25,
                    median: 24.0,
                    p10: 10.5,
                    p90: 40.0,
                },
                PairSummary {
                    low: 2,
                    high: 3,
                    samples: 3,
                    median: 48.333,
                    p10: 20.0,
                    p90: 70.0,
                },
            ],
        };
        let json = calibrate_json(&outcome);
        assert_eq!(
            json,
            "{\"subcommand\":\"calibrate-blend\",\"seeds\":3,\"from\":[0,0],\"size\":512,\"step\":16,\"pairs\":[\
             {\"pair\":[0,1],\"samples\":25,\"median_blocks\":24.00,\"p10_blocks\":10.50,\"p90_blocks\":40.00,\"assessed\":true,\"ok\":true},\
             {\"pair\":[2,3],\"samples\":3,\"median_blocks\":48.33,\"p10_blocks\":20.00,\"p90_blocks\":70.00,\"assessed\":false,\"ok\":false}],\"ok\":true}"
        );
    }

    #[test]
    fn anchor_cells_follow_the_global_stride() {
        let cells = anchor_cells((0, 0), 64, 16);
        assert_eq!(cells.len(), 16, "four anchors per axis");
        assert!(cells.iter().all(|(x, z)| x % 4 == 0 && z % 4 == 0));
        assert_eq!(anchor_cells((0, 0), 2, 16).len(), 0);
    }

    #[test]
    fn blend_weight_matches_the_generation_formula() {
        assert_eq!(blend_weight(0.0), 0.5);
        assert_eq!(blend_weight(255.0), 1.0);
        let mut previous = blend_weight(0.0);
        for q in 1..=255_u8 {
            let weight = blend_weight(f32::from(q));
            assert!(weight >= previous, "w fell at q {q}");
            previous = weight;
        }
    }
}
