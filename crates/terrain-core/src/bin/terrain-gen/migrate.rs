use std::path::PathBuf;

use terrain_core::header::{self, Header};
use terrain_core::world_save::{MigrationReport, WorldSave, write_migration_report};

use crate::cli::{Cursor, Options};
use crate::output::{CliFailure, EXIT_OK, report};

pub(crate) fn cmd_migrate(args: &[String], options: &Options) -> u8 {
    match migrate_outcome(args) {
        Ok((from, to, migrated)) => {
            if options.json {
                println!("{{\"ok\":true,\"from\":{from},\"to\":{to},\"sections\":{migrated}}}");
            } else {
                println!("migrated {migrated} sections from format {from} to {to}");
            }
            EXIT_OK
        }
        Err(failure) => report(&failure, options.json, "migrate"),
    }
}

fn migrate_outcome(args: &[String]) -> Result<(u16, u16, usize), CliFailure> {
    let mut cursor = Cursor::new(args);
    let mut world = None;
    let mut from = 1_u16;
    let mut to = header::HEADER_FORMAT_VERSION;
    while let Some(argument) = cursor.arg() {
        match argument {
            "--world" => {
                if world.is_some() {
                    return Err(CliFailure::Usage("duplicate --world".to_owned()));
                }
                world = Some(PathBuf::from(cursor.value("world")?));
            }
            "--from" => {
                from = cursor
                    .value("from")?
                    .parse()
                    .map_err(|_| CliFailure::Usage("--from needs a u16".to_owned()))?;
            }
            "--to" => {
                to = cursor
                    .value("to")?
                    .parse()
                    .map_err(|_| CliFailure::Usage("--to needs a u16".to_owned()))?;
            }
            other => {
                return Err(CliFailure::Usage(format!(
                    "unexpected argument {other:?} for migrate"
                )));
            }
        }
    }
    let world = world.ok_or_else(|| CliFailure::Usage("migrate needs --world <dir>".to_owned()))?;
    let header = Header::read(&world.join(header::HEADER_FILE))?;
    let mut save = WorldSave::open(world, header.seed)?;
    let started = header::now_unix_seconds();
    match save.migrate(from, to) {
        Ok(sections) => {
            let report = MigrationReport {
                from_format: from,
                to_format: to,
                started,
                finished: header::now_unix_seconds(),
                sections,
                ok: true,
                error: None,
            };
            write_migration_report(save.root(), &report)?;
            Ok((from, to, sections))
        }
        Err(error) => {
            let report = MigrationReport {
                from_format: from,
                to_format: to,
                started,
                finished: header::now_unix_seconds(),
                sections: 0,
                ok: false,
                error: Some(error.to_string()),
            };
            let _ = write_migration_report(save.root(), &report);
            Err(error.into())
        }
    }
}
