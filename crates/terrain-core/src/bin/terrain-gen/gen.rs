use crate::cli::{Cursor, Options, number, parse_seed};
use crate::output::{CliFailure, EXIT_OK, hex32, json_str, report};
use crate::{Counts, digest_columns, generate_columns};

/// Default `gen` square side.
const DEFAULT_GEN_SIZE: i32 = 16;

// ---------------------------------------------------------------------------
// gen
// ---------------------------------------------------------------------------

struct GenOutcome {
    seed: u64,
    from: (i32, i32),
    size: i32,
    columns: usize,
    hash: [u8; 32],
    counts: Counts,
}

pub(crate) fn cmd_gen(args: &[String], options: &Options) -> u8 {
    match gen_outcome(args, options) {
        Ok(outcome) => {
            print_gen(&outcome, options.json);
            EXIT_OK
        }
        Err(failure) => report(&failure, options.json, "gen"),
    }
}

fn gen_outcome(args: &[String], options: &Options) -> Result<GenOutcome, CliFailure> {
    let mut cursor = Cursor::new(args);
    let mut seed = None;
    let mut from = None;
    let mut size = None;
    while let Some(argument) = cursor.arg() {
        match argument {
            "--seed" => {
                seed = Some(parse_seed(cursor.value("seed")?)?);
            }
            "--from" => {
                let x = number::<i32>(cursor.value("from x")?, "from x")?;
                let z = number::<i32>(cursor.value("from z")?, "from z")?;
                from = Some((x, z));
            }
            "--size" => {
                size = Some(number::<i32>(cursor.value("size")?, "size")?);
            }
            other => {
                return Err(CliFailure::Usage(format!(
                    "unexpected argument {other:?} for gen"
                )));
            }
        }
    }
    let seed = seed.ok_or_else(|| CliFailure::Usage("gen needs --seed <u64>".to_owned()))?;
    let from = from.ok_or_else(|| CliFailure::Usage("gen needs --from <x> <z>".to_owned()))?;
    let size = size.unwrap_or(DEFAULT_GEN_SIZE);
    if size <= 0 {
        return Err(CliFailure::Usage(format!(
            "--size must be at least 1, found {size}"
        )));
    }
    let extent = usize::try_from(size)
        .map_err(|_| CliFailure::Usage(format!("--size {size} is too large")))?;
    let count = extent
        .checked_mul(extent)
        .ok_or_else(|| CliFailure::Usage(format!("--size {size} is too large")))?;

    let mut coords = Vec::with_capacity(count);
    for row in 0..size {
        for column in 0..size {
            coords.push((from.0 + column, from.1 + row));
        }
    }
    let columns = generate_columns(seed, &coords, options.threads, options.order)?;
    let (hash, counts) = digest_columns(&columns);
    Ok(GenOutcome {
        seed,
        from,
        size,
        columns: columns.len(),
        hash,
        counts,
    })
}

fn print_gen(outcome: &GenOutcome, json: bool) {
    let GenOutcome {
        seed,
        from,
        size,
        columns,
        hash,
        counts,
    } = outcome;
    if json {
        println!(
            "{{\"subcommand\":\"gen\",\"seed\":{seed},\"from\":[{},{}],\"size\":{size},\"columns\":{columns},\"hash\":{},\"solid\":{},\"water\":{},\"air\":{}}}",
            from.0,
            from.1,
            json_str(&hex32(hash)),
            counts.solid,
            counts.water,
            counts.air
        );
    } else {
        println!("seed {seed}");
        println!("from {} {}", from.0, from.1);
        println!("size {size}");
        println!("columns {columns}");
        println!("hash {}", hex32(hash));
        println!("solid {}", counts.solid);
        println!("water {}", counts.water);
        println!("air {}", counts.air);
    }
}
