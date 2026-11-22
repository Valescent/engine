//! Tests for the surface rule table, the evaluator, and the sea-floor bands.

use super::rules::{SNOW_LAYER_DEPTH, holds, locked_predicate};
use super::seafloor::SEABED_LAYER_DEPTH;
use super::*;
use crate::description::{SEA, default_v1};
use crate::error::{ErrorKind, Stage};
use crate::section::BlockId;
use crate::strata::Strata;

/// The biome topsoil block of the fixture tables.
const TOP: BlockId = 41;
/// The first filler block of the fixture tables.
const FILL: BlockId = 42;
/// The catch-all filler block of the fixture tables.
const CATCH: BlockId = 43;
/// Cold beach sand.
const COLD: BlockId = 51;
/// Warm beach sand.
const WARM: BlockId = 52;
/// Shallow sea-floor material.
const SHALLOW: BlockId = 53;
/// Deep sea-floor material.
const DEEP: BlockId = 54;
/// Snow cap material.
const SNOW: BlockId = 55;
/// Slope stone veneer.
const SLOPE: BlockId = 56;
/// River bed sand.
const SAND: BlockId = 57;
/// River bed gravel.
const GRAVEL: BlockId = 58;
/// Cold shore gravel (`E4.7`).
const SHORE_GRAVEL: BlockId = 59;
/// Warm shelf sand (`E4.7`).
const SHELF_SAND: BlockId = 60;
/// Cold shelf gravel (`E4.7`).
const SHELF_GRAVEL: BlockId = 61;
/// Hashed shelf clay (`E4.7`).
const SHELF_CLAY: BlockId = 62;
/// Slope gravel (`E4.7`).
const SLOPE_GRAVEL: BlockId = 63;
/// Abyss clay (`E4.7`).
const ABYSS_CLAY: BlockId = 64;
/// Bedrock, supplied by the caller like every other id.
const BEDROCK: BlockId = 1;

fn table(seed: u64) -> SurfaceTable {
    SurfaceTable::new(
        seed,
        Layer::Block(COLD),
        Layer::Block(WARM),
        Layer::Block(SHALLOW),
        Layer::Block(DEEP),
        Layer::Block(SNOW),
        Layer::Block(SLOPE),
        Layer::Block(SAND),
        Layer::Block(GRAVEL),
        Layer::Block(BEDROCK),
    )
}

/// A table with a distinct id per `E4.7` band, so a band mix-up shows.
fn banded_table(seed: u64) -> SurfaceTable {
    SurfaceTable::with_seabed(
        seed,
        Layer::Block(COLD),
        Layer::Block(WARM),
        SeabedMaterials {
            shore_sand: Layer::Block(SHALLOW),
            shore_gravel: Layer::Block(SHORE_GRAVEL),
            shelf_sand: Layer::Block(SHELF_SAND),
            shelf_gravel: Layer::Block(SHELF_GRAVEL),
            shelf_clay: Layer::Block(SHELF_CLAY),
            slope_gravel: Layer::Block(SLOPE_GRAVEL),
            abyss_clay: Layer::Block(ABYSS_CLAY),
        },
        Layer::Block(SNOW),
        Layer::Block(SLOPE),
        Layer::Block(SAND),
        Layer::Block(GRAVEL),
        Layer::Block(BEDROCK),
    )
}

/// Topsoil, one filler, then the catch-all.
fn rules() -> BiomeRules {
    BiomeRules {
        rules: vec![
            Rule {
                predicate: Predicate::DepthBelowTop(0),
                block: TOP,
                depth_blocks: 3,
            },
            Rule {
                predicate: Predicate::DepthBelowTop(3),
                block: FILL,
                depth_blocks: 4,
            },
            Rule {
                predicate: Predicate::And(Vec::new()),
                block: CATCH,
                depth_blocks: 8,
            },
        ],
    }
}

/// A table whose topsoil rule cannot fire, so the first match is a filler.
fn filler_first() -> BiomeRules {
    BiomeRules {
        rules: vec![
            Rule {
                predicate: Predicate::DepthBelowTop(1),
                block: TOP,
                depth_blocks: 3,
            },
            Rule {
                predicate: Predicate::And(Vec::new()),
                block: CATCH,
                depth_blocks: 20,
            },
        ],
    }
}

fn facts(top: i32) -> ColumnFacts {
    ColumnFacts {
        x: 40,
        z: 90,
        top,
        base: 12.0,
        temp: 0.6,
        slope_drop: 0,
        is_river: false,
        is_lake: false,
        is_bed: false,
        near_coast: false,
        water_adjacent: false,
    }
}

fn strata() -> Strata {
    Strata::new(0x5EED, 24, 8.0, 256, 90, vec![80, 81, 82]).expect("the table is valid")
}

#[test]
#[allow(clippy::too_many_lines)]
fn precedence_is_the_answered_order() {
    let table = table(7);
    let strata = strata();
    let rules = rules();
    let mut slots = [0_u8; SLOTS as usize];

    // Slot 1: a river bed, even on a high cold column.
    let bed = ColumnFacts {
        top: 300,
        temp: 0.1,
        base: 40.0,
        is_river: true,
        is_bed: true,
        ..facts(300)
    };
    // Slot 2: a submerged column, shallow enough for the shallow band.
    let seabed = facts(124);
    // Slot 3: the beach band, warm.
    let beach = ColumnFacts {
        top: 140,
        base: 1.5,
        ..facts(140)
    };
    // Slot 4: cold, high, dry land.
    let snow = ColumnFacts {
        top: 300,
        base: 40.0,
        temp: 0.1,
        ..facts(300)
    };
    // Slot 5: at the stone threshold.
    let slope = ColumnFacts {
        top: 200,
        slope_drop: STONE_DROP,
        ..facts(200)
    };
    // Slot 6 and 7: the two biome cases.
    let topsoil = facts(140);
    let filler_column = facts(140);
    // Slot 8: no rule matched.
    let empty = BiomeRules::default();
    let empty_column = facts(140);
    // Slot 9: nothing above bedrock.
    let floor = facts(0);

    let cases: [(u8, &ColumnFacts, &BiomeRules, Layer); 9] = [
        (1, &bed, &rules, Layer::Block(SAND)),
        (2, &seabed, &rules, Layer::Block(SHALLOW)),
        (3, &beach, &rules, Layer::Block(WARM)),
        (4, &snow, &rules, Layer::Block(SNOW)),
        (5, &slope, &rules, Layer::Block(SLOPE)),
        (6, &topsoil, &rules, Layer::Block(TOP)),
        (7, &filler_column, &filler_first(), Layer::Block(CATCH)),
        (8, &empty_column, &empty, Layer::Stone),
        (9, &floor, &rules, Layer::Block(BEDROCK)),
    ];

    for (slot, column, biome, first) in cases {
        assert_eq!(table.fired_slot(column, biome), slot, "slot {slot}");
        let stack = table.resolve_column(column, biome, &strata);
        assert_eq!(stack.slot, slot);
        assert_eq!(stack.layers[0].1, first, "slot {slot} material");
        assert_eq!(
            stack.depth_blocks(),
            column.top.max(0) + 1,
            "slot {slot} must cover the whole column"
        );
        assert_eq!(stack.layers.last().unwrap().1, Layer::Block(BEDROCK));
        slots[usize::from(slot) - 1] = slot;
    }
    assert_eq!(slots, [1, 2, 3, 4, 5, 6, 7, 8, 9]);

    // Deep sea floor takes the other band.
    let deep = table.resolve_column(&facts(100), &rules, &strata);
    assert_eq!(deep.layer_at(0), Some(Layer::Block(DEEP)));
    // A cold beach stays a beach: slot 3 wins over slot 4.
    let cold_beach = ColumnFacts {
        top: 140,
        base: 2.0,
        temp: 0.0,
        ..facts(140)
    };
    assert_eq!(table.fired_slot(&cold_beach, &rules), 3);
    assert_eq!(
        table
            .resolve_column(&cold_beach, &rules, &strata)
            .layer_at(0),
        Some(Layer::Block(COLD))
    );
    // A steep river bed takes gravel.
    let steep_bed = ColumnFacts {
        slope_drop: 4,
        is_river: true,
        is_bed: true,
        ..facts(140)
    };
    assert_eq!(
        table
            .resolve_column(&steep_bed, &rules, &strata)
            .layer_at(0),
        Some(Layer::Block(GRAVEL))
    );
}

#[test]
fn no_topsoil_under_water() {
    let table = table(7);
    let strata = strata();
    let rules = rules();

    let columns = [
        ColumnFacts {
            is_river: true,
            is_bed: true,
            ..facts(140)
        },
        facts(120),
        ColumnFacts {
            base: 1.0,
            ..facts(140)
        },
        ColumnFacts {
            temp: 0.1,
            base: 40.0,
            ..facts(300)
        },
        ColumnFacts {
            slope_drop: 3,
            ..facts(200)
        },
    ];

    for column in columns {
        let slot = table.fired_slot(&column, &rules);
        assert!((1..=5).contains(&slot), "fixture must fire a locked slot");
        assert!(
            table
                .resolve(&column, &rules)
                .iter()
                .all(|(_, block)| *block != TOP),
            "slot {slot} emitted a biome topsoil rule"
        );
        let stack = table.resolve_column(&column, &rules, &strata);
        assert!(
            !stack
                .layers
                .iter()
                .any(|(_, layer)| *layer == Layer::Block(TOP)),
            "slot {slot} emitted a biome topsoil layer"
        );
        // The biome filler is still available below the surface material,
        // except on a cliff (stone all the way) and a sea floor (three
        // band blocks then stone, `E4.7`).
        if slot != 5 && slot != 2 {
            assert!(
                stack
                    .layers
                    .iter()
                    .any(|(_, layer)| *layer == Layer::Block(FILL)
                        || *layer == Layer::Block(CATCH)),
                "slot {slot} dropped the biome filler as well"
            );
        }
    }
}

/// A column whose sea-floor dither and shelf clay hashes are both quiet.
fn quiet_seabed_column(table: &SurfaceTable) -> (i32, i32) {
    for z in 0..64 {
        for x in 0..64 {
            let column = ColumnFacts {
                x,
                z,
                top: SEA - 10,
                ..facts(SEA - 10)
            };
            if !table.seabed_dithers(&column) && !table.shelf_clay_patch(&column) {
                return (x, z);
            }
        }
    }
    panic!("the fixture found no quiet sea-floor column");
}

#[test]
fn depth_below_sea_in_is_an_inclusive_offset_band() {
    let table = banded_table(7);
    let on_sea = facts(SEA);
    let below = |depth: i32| ColumnFacts {
        top: SEA - depth,
        ..facts(SEA - depth)
    };

    assert!(!holds(&Predicate::DepthBelowSeaIn(1, 4), &on_sea, 0));
    assert!(holds(&Predicate::DepthBelowSeaIn(1, 4), &below(1), 0));
    assert!(holds(&Predicate::DepthBelowSeaIn(1, 4), &below(4), 0));
    assert!(!holds(&Predicate::DepthBelowSeaIn(1, 4), &below(5), 0));
    assert!(holds(&Predicate::DepthBelowSeaIn(5, 14), &below(5), 0));
    assert!(holds(&Predicate::DepthBelowSeaIn(5, 14), &below(14), 0));
    assert!(!holds(&Predicate::DepthBelowSeaIn(5, 14), &below(15), 0));

    // It owns the locked sea-floor slot, so a biome rule cannot use it.
    assert!(locked_predicate(&Predicate::DepthBelowSeaIn(1, 4)));
    let early = BiomeRules {
        rules: vec![
            Rule {
                predicate: Predicate::DepthBelowSeaIn(1, 4),
                block: TOP,
                depth_blocks: 3,
            },
            Rule {
                predicate: Predicate::And(Vec::new()),
                block: CATCH,
                depth_blocks: 4,
            },
        ],
    };
    assert!(table.validate_table(&early).is_err());
}

#[test]
fn each_seafloor_band_has_its_known_depth_and_material() {
    let table = banded_table(7);
    let strata = strata();
    let rules = rules();
    let (x, z) = quiet_seabed_column(&table);
    let column = |depth: i32| ColumnFacts {
        x,
        z,
        top: SEA - depth,
        temp: 0.6,
        ..facts(SEA - depth)
    };

    let cases = [
        (1, SHALLOW),
        (2, SHALLOW),
        (4, SHALLOW),
        (5, SHELF_SAND),
        (9, SHELF_SAND),
        (14, SHELF_SAND),
        (15, SLOPE_GRAVEL),
        (24, SLOPE_GRAVEL),
        (25, ABYSS_CLAY),
        (40, ABYSS_CLAY),
    ];

    for (depth, expected) in cases {
        let facts = column(depth);
        assert_eq!(table.fired_slot(&facts, &rules), 2, "depth {depth}");
        let stack = table.resolve_column(&facts, &rules, &strata);
        assert_eq!(
            stack.layers[0],
            (SEABED_LAYER_DEPTH, Layer::Block(expected)),
            "sea-floor depth {depth}"
        );
        assert_eq!(
            stack.layers[1].1,
            Layer::Stone,
            "band material is exactly three blocks, then symbolic stone"
        );
        assert_eq!(
            stack.depth_blocks(),
            facts.top + 1,
            "the sea-floor stack still covers the whole column"
        );
        assert_eq!(stack.layers.last().unwrap().1, Layer::Block(BEDROCK));
        assert!(
            !stack
                .layers
                .iter()
                .any(|(_, layer)| *layer == Layer::Block(TOP)),
            "biome topsoil appeared under water at depth {depth}"
        );
    }
}

#[test]
fn cold_seafloor_variants_pick_gravel() {
    let table = banded_table(7);
    let strata = strata();
    let rules = rules();
    let (x, z) = quiet_seabed_column(&table);
    let layer = |depth: i32, temp: f32| {
        let facts = ColumnFacts {
            x,
            z,
            top: SEA - depth,
            temp,
            ..facts(SEA - depth)
        };
        table.resolve_column(&facts, &rules, &strata).layers[0].1
    };

    // Warm is sand; cold shore and cold shelf are gravel.
    assert_eq!(layer(2, 0.6), Layer::Block(SHALLOW));
    assert_eq!(layer(2, 0.0), Layer::Block(SHORE_GRAVEL));
    assert_eq!(layer(10, 0.6), Layer::Block(SHELF_SAND));
    assert_eq!(layer(10, 0.0), Layer::Block(SHELF_GRAVEL));

    // The thresholds are the named constants, not magic values.
    assert_eq!(layer(2, T_COLD_SHORE - 0.01), Layer::Block(SHORE_GRAVEL));
    assert_eq!(layer(2, T_COLD_SHORE), Layer::Block(SHALLOW));
    assert_eq!(layer(10, T_COLD_SHELF - 0.01), Layer::Block(SHELF_GRAVEL));
    assert_eq!(layer(10, T_COLD_SHELF), Layer::Block(SHELF_SAND));

    // A warm shelf column with a hashed patch takes clay instead of sand.
    let mut patched = None;
    'scan: for scan_z in 0..128 {
        for scan_x in 0..128 {
            let facts = ColumnFacts {
                x: scan_x,
                z: scan_z,
                top: SEA - 10,
                temp: 0.6,
                ..facts(SEA - 10)
            };
            if table.shelf_clay_patch(&facts) {
                patched = Some(table.resolve_column(&facts, &rules, &strata).layers[0].1);
                break 'scan;
            }
        }
    }
    assert_eq!(patched, Some(Layer::Block(SHELF_CLAY)));
}

#[test]
fn seafloor_dither_only_fires_at_band_edges() {
    let table = banded_table(7);
    let strata = strata();
    let rules = rules();

    let normal = |facts: &ColumnFacts, depth: i32| -> BlockId {
        if depth <= SEABED_SHORE_LAST_DEPTH {
            if facts.temp < T_COLD_SHORE {
                SHORE_GRAVEL
            } else {
                SHALLOW
            }
        } else if depth <= SEABED_SHELF_LAST_DEPTH {
            if facts.temp < T_COLD_SHELF {
                SHELF_GRAVEL
            } else if table.shelf_clay_patch(facts) {
                SHELF_CLAY
            } else {
                SHELF_SAND
            }
        } else if depth <= SEABED_SLOPE_LAST_DEPTH {
            SLOPE_GRAVEL
        } else {
            ABYSS_CLAY
        }
    };
    let deeper = |depth: i32| -> BlockId {
        if depth <= SEABED_SHELF_LAST_DEPTH {
            SLOPE_GRAVEL
        } else {
            ABYSS_CLAY
        }
    };
    let material = |facts: &ColumnFacts| table.resolve_column(facts, &rules, &strata).layers[0].1;

    // Interior depths ignore the hash entirely.
    for depth in [1, 2, 3, 5, 9, 13, 15, 20, 23, 25, 31] {
        for z in 0..32 {
            for x in 0..32 {
                let facts = ColumnFacts {
                    x,
                    z,
                    top: SEA - depth,
                    temp: 0.6,
                    ..facts(SEA - depth)
                };
                assert_eq!(
                    material(&facts),
                    Layer::Block(normal(&facts, depth)),
                    "interior depth {depth} at {x},{z}"
                );
            }
        }
    }

    // At the last depth of a band only, both materials appear.
    for depth in [
        SEABED_SHORE_LAST_DEPTH,
        SEABED_SHELF_LAST_DEPTH,
        SEABED_SLOPE_LAST_DEPTH,
    ] {
        let mut base = 0_u32;
        let mut next = 0_u32;
        for z in 0..64 {
            for x in 0..64 {
                let facts = ColumnFacts {
                    x,
                    z,
                    top: SEA - depth,
                    temp: 0.6,
                    ..facts(SEA - depth)
                };
                let got = material(&facts);
                let expected = if table.seabed_dithers(&facts) {
                    Layer::Block(deeper(depth))
                } else {
                    Layer::Block(normal(&facts, depth))
                };
                assert_eq!(got, expected, "edge depth {depth} at {x},{z}");
                if got == Layer::Block(normal(&facts, depth)) {
                    base += 1;
                } else {
                    next += 1;
                }
            }
        }
        assert!(base > 0 && next > 0, "edge depth {depth} was not ragged");
    }

    // The dither is keyed on `(x, z)` only: the same column with a
    // different top still takes the same decision.
    let edge = ColumnFacts {
        x: 11,
        z: 23,
        top: SEA - SEABED_SHORE_LAST_DEPTH,
        ..facts(SEA - SEABED_SHORE_LAST_DEPTH)
    };
    let shifted = ColumnFacts {
        top: SEA - SEABED_SHORE_LAST_DEPTH - 1,
        ..edge
    };
    assert_eq!(table.seabed_dithers(&edge), table.seabed_dithers(&shifted));
}

#[test]
fn bed_beats_everything() {
    let table = table(7);
    let column = ColumnFacts {
        top: 300,
        base: 1.0,
        temp: 0.0,
        slope_drop: 9,
        is_river: true,
        is_bed: true,
        near_coast: true,
        water_adjacent: true,
        ..facts(300)
    };
    assert_eq!(table.fired_slot(&column, &rules()), 1);
    let stack = table.resolve_column(&column, &rules(), &strata());
    assert_eq!(stack.slot, 1);
    assert_eq!(stack.layer_at(0), Some(Layer::Block(GRAVEL)));
}

#[test]
fn snow_needs_cold_and_dry_land() {
    let table = table(7);
    let rules = rules();

    assert_eq!(T_SNOW.to_bits(), default_v1().t_snow.to_bits());

    let land = ColumnFacts {
        top: 300,
        base: 40.0,
        temp: 0.1,
        ..facts(300)
    };
    assert_eq!(table.fired_slot(&land, &rules), 4);

    let sea_floor = facts(110);
    assert_eq!(table.fired_slot(&sea_floor, &rules), 2);

    let beach = ColumnFacts {
        top: 140,
        base: 1.0,
        temp: 0.0,
        ..facts(140)
    };
    assert_eq!(table.fired_slot(&beach, &rules), 3);

    // The jitter is deterministic, in range, and actually varies.
    let mut seen = [false; 5];
    for x in -8..=8 {
        for z in -8..=8 {
            let jitter = snow_jitter(7, x, z);
            assert!((-2..=2).contains(&jitter), "{x},{z} gave {jitter}");
            assert_eq!(jitter, snow_jitter(7, x, z));
            seen[usize::try_from(jitter + 2).unwrap()] = true;
        }
    }
    assert_eq!(seen.iter().filter(|hit| **hit).count(), 5);

    // The cap's thickness uses the same jitter, clamped to at least a block.
    for x in 0..32 {
        for z in 0..32 {
            let column = ColumnFacts { x, z, ..land };
            let stack = table.resolve_column(&column, &rules, &strata());
            let depth = stack.layers[0].0;
            assert!((1..=SNOW_LAYER_DEPTH + 2).contains(&depth), "{depth}");
        }
    }
}

#[test]
#[allow(clippy::cast_precision_loss)]
fn slope_stone_and_its_dither_band() {
    let table = table(7);
    let rules = rules();

    let stone_at_two = ColumnFacts {
        top: 200,
        slope_drop: 2,
        ..facts(200)
    };
    assert_eq!(table.fired_slot(&stone_at_two, &rules), 5);

    let never = ColumnFacts {
        top: 200,
        slope_drop: 0,
        ..facts(200)
    };
    for x in 0..64 {
        for z in 0..64 {
            let column = ColumnFacts { x, z, ..never };
            assert_ne!(table.fired_slot(&column, &rules), 5, "{x},{z}");
        }
    }

    let mut hits = 0_i32;
    let mut total = 0_i32;
    for x in 0..64 {
        for z in 0..64 {
            let column = ColumnFacts {
                x,
                z,
                slope_drop: 1,
                ..facts(200)
            };
            total += 1;
            if table.fired_slot(&column, &rules) == 5 {
                hits += 1;
            }
        }
    }
    assert_eq!(total, 64 * 64);
    let share = hits as f32 / total as f32;
    assert!(
        (0.30..=0.40).contains(&share),
        "the dither band covered {share} of {total} columns"
    );
}

#[test]
fn table_validation_rejects_bad_rules() {
    let table = table(7);
    table
        .validate_table(&rules())
        .expect("the fixture is valid");

    // A biome rule that claims a locked slot.
    let early = BiomeRules {
        rules: vec![
            Rule {
                predicate: Predicate::SlopeAtLeast(3),
                block: TOP,
                depth_blocks: 3,
            },
            Rule {
                predicate: Predicate::And(Vec::new()),
                block: FILL,
                depth_blocks: 4,
            },
        ],
    };
    let error = table.validate_table(&early).expect_err("slot 5 is locked");
    assert_eq!(error.stage, Stage::Surface);
    assert!(
        matches!(
            error.kind,
            ErrorKind::DescriptionInvalid {
                key: "SURFACE_RULES",
                ..
            }
        ),
        "{error:?}"
    );

    // No catch-all.
    let open = BiomeRules {
        rules: vec![Rule {
            predicate: Predicate::DepthBelowTop(2),
            block: TOP,
            depth_blocks: 3,
        }],
    };
    assert!(table.validate_table(&open).is_err());

    // An empty list has no catch-all either.
    assert!(table.validate_table(&BiomeRules::default()).is_err());

    // More entries than a content pack may hold.
    let mut many = Vec::with_capacity(MAX_BIOMES + 1);
    for _ in 0..MAX_BIOMES {
        many.push(Rule {
            predicate: Predicate::DepthBelowTop(1),
            block: FILL,
            depth_blocks: 1,
        });
    }
    many.push(Rule {
        predicate: Predicate::And(Vec::new()),
        block: CATCH,
        depth_blocks: 1,
    });
    let crowded = BiomeRules { rules: many };
    assert!(table.validate_table(&crowded).is_err());

    // A zero-thickness rule would shift every later rule up a slot.
    let flat = BiomeRules {
        rules: vec![
            Rule {
                predicate: Predicate::And(Vec::new()),
                block: TOP,
                depth_blocks: 0,
            },
            Rule {
                predicate: Predicate::And(Vec::new()),
                block: CATCH,
                depth_blocks: 4,
            },
        ],
    };
    assert!(table.validate_table(&flat).is_err());
}

#[test]
fn stone_layer_stays_symbolic() {
    let table = table(7);
    let strata = strata();
    let column = facts(140);

    // No biome rules means no biome contribution: the surface is stone.
    let stack = table.resolve_column(&column, &BiomeRules::default(), &strata);
    assert_eq!(stack.slot, 8);
    assert_eq!(stack.layers[0], (140, Layer::Stone));
    assert!(
        stack.layers[1..]
            .iter()
            .all(|(_, layer)| *layer == Layer::Block(BEDROCK)),
        "only bedrock follows the stone body"
    );

    // The symbolic marker resolves through the strata resolver.
    let resolved = stack
        .block_at(0, &strata, column.x, column.top, column.z)
        .expect("depth 0 is inside the column");
    assert_eq!(resolved, strata.stone_at(column.x, column.top, column.z));
    assert_ne!(resolved, BEDROCK);

    // The cache the stack carries agrees with a fresh column evaluation.
    assert_eq!(
        strata.stone_in(stack.column, column.x, column.top, column.z),
        strata.stone_at(column.x, column.top, column.z)
    );
}
