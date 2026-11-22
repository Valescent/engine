use std::path::PathBuf;

use terrain_core::content;
use terrain_core::description::SECTIONS_PER_COLUMN;
use terrain_core::save::{RepairAction, RepairOutcome, SectionStore};
use terrain_core::section::SectionKey;

use crate::cli::{Cursor, Options};
use crate::output::{CliFailure, EXIT_OK, json_str, report};

// ---------------------------------------------------------------------------
// repair
// ---------------------------------------------------------------------------

pub(crate) fn cmd_repair(args: &[String], options: &Options) -> u8 {
    match repair_outcome(args) {
        Ok((key, outcome)) => {
            let name = format!("c.{}.{}.{}", key.column[0], key.column[1], key.section);
            if options.json {
                println!(
                    "{{\"ok\":true,\"section\":{},\"outcome\":{}}}",
                    json_str(&name),
                    json_str(repair_name(&outcome))
                );
            } else {
                println!("{name}: {}", repair_name(&outcome));
            }
            EXIT_OK
        }
        Err(failure) => report(&failure, options.json, "repair"),
    }
}

fn repair_name(outcome: &RepairOutcome) -> &'static str {
    match outcome {
        RepairOutcome::Restored(_) => "restored",
        RepairOutcome::Quarantined => "quarantined",
        RepairOutcome::Salvaged(_) => "salvaged (disk unchanged)",
    }
}

fn repair_outcome(args: &[String]) -> Result<(SectionKey, RepairOutcome), CliFailure> {
    let mut cursor = Cursor::new(args);
    let mut world = None;
    let mut key = None;
    let mut action = None;
    while let Some(argument) = cursor.arg() {
        match argument {
            "--world" => {
                if world.is_some() {
                    return Err(CliFailure::Usage("duplicate --world".to_owned()));
                }
                world = Some(PathBuf::from(cursor.value("world")?));
            }
            "--section" => {
                if key.is_some() {
                    return Err(CliFailure::Usage("duplicate --section".to_owned()));
                }
                key = Some(parse_repair_key(cursor.value("section")?)?);
            }
            "--action" => {
                if action.is_some() {
                    return Err(CliFailure::Usage("duplicate --action".to_owned()));
                }
                action = Some(match cursor.value("action")? {
                    "restore" => RepairAction::RestoreBackup,
                    "quarantine" => RepairAction::Quarantine,
                    "salvage" => RepairAction::Salvage,
                    other => {
                        return Err(CliFailure::Usage(format!(
                            "invalid --action {other:?}; expected restore, quarantine, or salvage"
                        )));
                    }
                });
            }
            other => {
                return Err(CliFailure::Usage(format!(
                    "unexpected argument {other:?} for repair"
                )));
            }
        }
    }
    let world = world.ok_or_else(|| CliFailure::Usage("repair needs --world <dir>".to_owned()))?;
    let key =
        key.ok_or_else(|| CliFailure::Usage("repair needs --section <cx,cz,sy>".to_owned()))?;
    let action =
        action.ok_or_else(|| CliFailure::Usage("repair needs --action <action>".to_owned()))?;
    let store = SectionStore::new(world, content::v1_default());
    let outcome = store.repair(key, action)?;
    Ok((key, outcome))
}

fn parse_repair_key(value: &str) -> Result<SectionKey, String> {
    let [cx, cz, sy]: [&str; 3] = value
        .split(',')
        .collect::<Vec<_>>()
        .try_into()
        .map_err(|_| format!("invalid --section {value:?}; expected cx,cz,sy"))?;
    let column = [
        cx.parse::<i32>()
            .map_err(|_| format!("invalid section column x {cx:?}"))?,
        cz.parse::<i32>()
            .map_err(|_| format!("invalid section column z {cz:?}"))?,
    ];
    let section = sy
        .parse::<u8>()
        .map_err(|_| format!("invalid section slot {sy:?}"))?;
    if u32::from(section) >= SECTIONS_PER_COLUMN {
        return Err(format!(
            "section slot {section} is outside 0..{SECTIONS_PER_COLUMN}"
        ));
    }
    Ok(SectionKey { column, section })
}
