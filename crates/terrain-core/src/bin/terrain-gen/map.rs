use terrain_core::description::SEA;
use terrain_core::pipeline::{AIR, Terrain, WATER};

use crate::cli::{Cursor, Options, number, parse_seed};
use crate::output::{CliFailure, EXIT_OK, report};

pub(crate) fn cmd_map(args: &[String], options: &Options) -> u8 {
    match map_outcome(args) {
        Ok(outcome) => {
            if options.json {
                println!(
                    "{{\"ok\":true,\"seed\":{},\"from\":[{},{}],\"size\":{},\"cell\":{},\"rows\":[{}]}}",
                    outcome.seed,
                    outcome.from.0,
                    outcome.from.1,
                    outcome.size,
                    outcome.cell,
                    outcome
                        .rows
                        .iter()
                        .map(|row| format!("\"{row}\""))
                        .collect::<Vec<_>>()
                        .join(",")
                );
            } else {
                println!(
                    "map seed={} from={},{} size={} cell={}",
                    outcome.seed, outcome.from.0, outcome.from.1, outcome.size, outcome.cell
                );
                for row in &outcome.rows {
                    println!("{row}");
                }
            }
            EXIT_OK
        }
        Err(failure) => report(&failure, options.json, "map"),
    }
}

struct MapOutcome {
    seed: u64,
    from: (i32, i32),
    size: usize,
    cell: i32,
    rows: Vec<String>,
}

fn map_outcome(args: &[String]) -> Result<MapOutcome, CliFailure> {
    let mut cursor = Cursor::new(args);
    let mut seed = None;
    let mut from = None;
    let mut size = 32_usize;
    let mut cell = 4_i32;
    while let Some(argument) = cursor.arg() {
        match argument {
            "--seed" => seed = Some(parse_seed(cursor.value("seed")?)?),
            "--from" => {
                let x = number::<i32>(cursor.value("from x")?, "from x")?;
                let z = number::<i32>(cursor.value("from z")?, "from z")?;
                from = Some((x, z));
            }
            "--size" => {
                let value = number::<i32>(cursor.value("size")?, "size")?;
                size = usize::try_from(value)
                    .ok()
                    .filter(|value| *value > 0 && *value <= 256)
                    .ok_or_else(|| CliFailure::Usage("--size must be in 1..=256".to_owned()))?;
            }
            "--cell" => {
                cell = number::<i32>(cursor.value("cell")?, "cell")?;
                if cell <= 0 {
                    return Err(CliFailure::Usage("--cell must be positive".to_owned()));
                }
            }
            other => {
                return Err(CliFailure::Usage(format!(
                    "unexpected argument {other:?} for map"
                )));
            }
        }
    }
    let seed = seed.ok_or_else(|| CliFailure::Usage("map needs --seed <u64>".to_owned()))?;
    let from = from.unwrap_or((0, 0));
    let terrain = Terrain::new(seed)?;
    let mut rows = Vec::with_capacity(size);
    for row in 0..size {
        let mut line = String::with_capacity(size);
        for column in 0..size {
            let x = from.0 + i32::try_from(column).unwrap_or(0) * cell;
            let z = from.1 + i32::try_from(row).unwrap_or(0) * cell;
            let blocks = terrain.column(x, z)?;
            let top = blocks
                .iter()
                .rposition(|id| *id != AIR)
                .map_or(SEA, |index| i32::try_from(index).unwrap_or(SEA));
            let id = blocks
                .get(usize::try_from(top).unwrap_or(0))
                .copied()
                .unwrap_or(AIR);
            let glyph = if id == WATER || top < SEA {
                '~'
            } else {
                let band = (top - SEA) / 8;
                match band {
                    i32::MIN..=0 => '.',
                    1 => ',',
                    2 => '-',
                    3 => '+',
                    4 => '*',
                    5 => '#',
                    6 => '%',
                    _ => '@',
                }
            };
            line.push(glyph);
        }
        rows.push(line);
    }
    Ok(MapOutcome {
        seed,
        from,
        size,
        cell,
        rows,
    })
}
