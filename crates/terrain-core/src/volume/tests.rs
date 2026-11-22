//! Tests for the volume pass.

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]

use super::caves::{
    CHEESE_DEPTH_END, CHEESE_DEPTH_FLAT_END, CHEESE_DEPTH_FULL, CHEESE_FLOOR_FULL,
    CHEESE_FLOOR_START, CHEESE_SEAL_BLOCKS, ENTRANCE_RAMP, LOWLAND_CEILING_DEPTH,
    SPAGHETTI_DEPTH_FULL, SPAGHETTI_END_Y, SPAGHETTI_FLAT_END_Y, SPAGHETTI_FULL_Y,
    SPAGHETTI_START_Y, cheese_depth_weight, cheese_floor_weight, spaghetti_depth_weight,
    spaghetti_y_weight,
};
use super::detail::DETAIL_SLOPE_SCALE;
use super::*;
use crate::hash::{hash01, hash3};
use crate::lattice::macro_cell;

/// Air, as the caller supplies it.
const AIR: BlockId = 0;
/// Stone, as the caller supplies it.
const STONE: BlockId = 7;

fn volume(seed: u64) -> Volume {
    Volume::new(VolumeParams::v1_default(seed)).expect("the v1 defaults are valid")
}

fn land(x: i32, z: i32, top: i32) -> ColumnInput {
    ColumnInput {
        x,
        z,
        top,
        rough: 1.0,
        slope: 8,
        is_river: false,
        is_lake: false,
        near_bank: false,
        #[allow(clippy::cast_precision_loss)]
        macro_height: top as f32 + 0.4,
    }
}

fn solid_column(top: i32) -> [BlockId; COLUMN_HEIGHT] {
    let mut column = [AIR; COLUMN_HEIGHT];
    for y in 0..=top {
        column[usize::try_from(y).unwrap()] = STONE;
    }
    column
}

/// One deterministic coordinate triple per index.
fn sample_coords(seed: u64, index: i32) -> (f32, f32, f32) {
    let tag = domain("volume-test-samples");
    let x = hash01(hash3(seed, tag, index, 0, 1)) * 4096.0 - 2048.0;
    let z = hash01(hash3(seed, tag, index, 2, 3)) * 4096.0 - 2048.0;
    let y = 1.0 + hash01(hash3(seed, tag, index, 4, 5)) * 350.0;
    (x, y, z)
}

#[test]
fn density_stays_in_range() {
    let volume = volume(0x5EED);
    let mut clamps = 0_u32;
    let mut carved = 0_u32;
    for index in 0..100_000 {
        let (x, y, z) = sample_coords(7, index);
        let sample = volume.cave_density(x, y, z, crate::description::MAX_TERRAIN_Y);
        assert!(
            (0.0..=1.0).contains(&sample.density),
            "{} at {x} {y} {z}",
            sample.density
        );
        clamps += u32::from(sample.clamped);
        carved += u32::from(sample.density > volume.params().cave_t);
        assert_eq!(
            sample.taps, CAVE_TAP_BUDGET,
            "a 3D sample must spend exactly the budget"
        );
    }
    assert_eq!(clamps, 0, "unit()'s clamp is a backstop and must not fire");
    assert!(carved > 0, "the fixture must carve somewhere");
    assert!(
        carved < 100_000 / 2,
        "cheese must not fill half the underground: {carved}"
    );
}

#[test]
#[allow(clippy::float_cmp)]
fn cave_window_curves_are_monotone() {
    // Cheese depth: rises to 40, flat to 140, falls to zero by 200.
    let mut previous = cheese_depth_weight(0.0);
    assert_eq!(previous, 0.0, "no cheese above d = 12");
    for depth in 1..=CHEESE_DEPTH_FULL {
        let weight = cheese_depth_weight(depth as f32);
        assert!(weight >= previous, "cheese depth fell at {depth}");
        previous = weight;
    }
    assert_eq!(cheese_depth_weight(CHEESE_DEPTH_FULL as f32), 1.0);
    for depth in CHEESE_DEPTH_FULL..=CHEESE_DEPTH_FLAT_END {
        assert_eq!(
            cheese_depth_weight(depth as f32),
            1.0,
            "cheese depth is not flat at {depth}"
        );
    }
    previous = 1.0;
    for depth in CHEESE_DEPTH_FLAT_END..=CHEESE_DEPTH_END {
        let weight = cheese_depth_weight(depth as f32);
        assert!(weight <= previous, "cheese tail rose at {depth}");
        previous = weight;
    }
    assert_eq!(cheese_depth_weight(CHEESE_DEPTH_END as f32), 0.0);

    // Cheese floor: zero below 8, full by 24.
    let mut previous = cheese_floor_weight(0.0);
    assert_eq!(previous, 0.0);
    for y in 1..=CHEESE_FLOOR_FULL {
        let weight = cheese_floor_weight(y as f32);
        assert!(weight >= previous, "cheese floor fell at {y}");
        previous = weight;
    }
    assert_eq!(cheese_floor_weight(CHEESE_FLOOR_FULL as f32), 1.0);

    // Spaghetti `y`: rises to 24, flat to 296, falls to zero by 308.
    let mut previous = spaghetti_y_weight(0.0);
    assert_eq!(previous, 0.0);
    for y in 1..=SPAGHETTI_FULL_Y {
        let weight = spaghetti_y_weight(y as f32);
        assert!(weight >= previous, "spaghetti y fell at {y}");
        previous = weight;
    }
    assert_eq!(spaghetti_y_weight(SPAGHETTI_FULL_Y as f32), 1.0);
    for y in SPAGHETTI_FULL_Y..=SPAGHETTI_FLAT_END_Y {
        assert_eq!(spaghetti_y_weight(y as f32), 1.0, "not flat at {y}");
    }
    previous = 1.0;
    for y in SPAGHETTI_FLAT_END_Y..=SPAGHETTI_END_Y {
        let weight = spaghetti_y_weight(y as f32);
        assert!(weight <= previous, "spaghetti tail rose at {y}");
        previous = weight;
    }
    assert_eq!(spaghetti_y_weight(SPAGHETTI_END_Y as f32), 0.0);

    // Spaghetti depth: zero at d < 4, full by 16.
    let mut previous = spaghetti_depth_weight(0.0);
    assert_eq!(previous, 0.0);
    for depth in 1..=SPAGHETTI_DEPTH_FULL {
        let weight = spaghetti_depth_weight(depth as f32);
        assert!(weight >= previous, "spaghetti depth fell at {depth}");
        previous = weight;
    }
    assert_eq!(spaghetti_depth_weight(SPAGHETTI_DEPTH_FULL as f32), 1.0);
    assert_eq!(spaghetti_depth_weight(40.0), 1.0);
}

#[test]
#[allow(clippy::float_cmp)]
fn no_cheese_below_the_floor_or_near_the_top() {
    let volume = volume(0x5EED);
    let top = 60;
    for y in 0..CHEESE_FLOOR_START {
        assert_eq!(
            volume.cheese_weight(top, y as f32),
            0.0,
            "cheese at y = {y}"
        );
    }
    assert!(volume.cheese_weight(top, CHEESE_FLOOR_FULL as f32) > 0.0);
}

#[test]
#[allow(clippy::float_cmp)]
fn a_near_sea_column_stays_sealed_under_its_surface() {
    let volume = volume(0x5EED);
    let top = SEA + 1;

    // Cheese is zero for the whole 24 blocks under a beach or sea floor
    // (the cell exactly 24 blocks down is at the boundary, not inside it).
    for y in (top - CHEESE_SEAL_BLOCKS + 1)..=top {
        assert_eq!(
            volume.cheese_weight(top, y as f32),
            0.0,
            "cheese breached the seal at y = {y}"
        );
    }
    assert!(
        volume.cheese_weight(top, (top - CHEESE_SEAL_BLOCKS - 1) as f32) > 0.0
            || top - CHEESE_SEAL_BLOCKS - 1 < CHEESE_FLOOR_START,
        "the seal must be a local rule, not a global zero"
    );

    // End to end with an aggressive threshold: no carve above the lowland
    // spaghetti ceiling, so the sea floor stays sealed.
    let aggressive = Volume::new(VolumeParams {
        cave_t: 0.0,
        detail_amp: 0.0,
        ..VolumeParams::v1_default(0x5EED)
    })
    .expect("valid");
    let input = ColumnInput {
        slope: 0,
        rough: 0.0,
        ..land(3, 4, top)
    };
    let mut column = solid_column(top);
    aggressive.apply(&mut column, &input, AIR, STONE);
    // The combined density is what carves, and spaghetti is still allowed
    // below the lowland ceiling; above it the sea floor is sealed.
    for y in (SEA - LOWLAND_CEILING_DEPTH + 1)..=top {
        assert_ne!(
            column[usize::try_from(y).unwrap()],
            AIR,
            "lowland ceiling allowed a tunnel at y = {y}"
        );
    }
}

#[test]
fn nothing_carves_within_four_blocks_of_the_surface_except_mouths() {
    let volume = Volume::new(VolumeParams {
        cave_t: 0.0,
        detail_amp: 0.0,
        ..VolumeParams::v1_default(0x5EED)
    })
    .expect("valid");

    let mut quiet = None;
    'scan: for z in 0..96 {
        for x in 0..96 {
            // `macro_height` sits just above the integer top so the detail
            // pass, which is disabled by amplitude but still evaluates its
            // base term, keeps the top block.
            let input = ColumnInput {
                slope: 0,
                rough: 0.0,
                macro_height: 200.6,
                ..land(x, z, 200)
            };
            if !volume.mouth_open(&input) {
                quiet = Some((x, z, input));
                break 'scan;
            }
        }
    }
    let (x, z, input) = quiet.expect("the fixture finds a non-mouth column");

    let mut column = solid_column(input.top);
    let stats = volume.apply(&mut column, &input, AIR, STONE);
    assert_eq!(stats.mouths, 0, "a flat column is not a mouth");
    assert!(stats.cave_cells > 0, "the carve must reach somewhere");
    for offset in 0..CAVE_MIN_ROOF {
        let y = input.top - offset;
        assert_ne!(
            column[usize::try_from(y).unwrap()],
            AIR,
            "cave carved {offset} blocks under the surface at {x},{z}"
        );
    }
}

#[test]
fn tap_budget_is_enforced() {
    let volume = volume(11);
    let input = land(3, 4, 180);
    let mut cave_max = 0_u8;
    let mut detail_max = 0_u8;
    for index in 0..10_000 {
        let (x, y, z) = sample_coords(13, index);
        let cave = volume.cave_density(x, y, z, input.top);
        cave_max = cave_max.max(cave.taps);
        let block_x = (index % 64) - 32;
        let block_z = (index / 64) - 32;
        let detail = volume.detail_sample(block_x, 180, block_z, &input);
        detail_max = detail_max.max(detail.taps);
    }
    assert_eq!(cave_max, CAVE_TAP_BUDGET);
    assert!(cave_max <= CAVE_TAP_BUDGET);
    assert_eq!(detail_max, 1);
    assert!(detail_max <= DETAIL_TAP_BUDGET);
    assert!(detail_max > 0);
}

#[test]
fn lattice_skips_above_the_roof() {
    let params = VolumeParams {
        detail_amp: 0.0,
        ..VolumeParams::v1_default(5)
    };
    let volume = Volume::new(params).expect("valid");
    let input = ColumnInput {
        slope: 0,
        rough: 0.0,
        ..land(0, 0, 100)
    };
    let mut column = solid_column(100);
    let stats = volume.apply(&mut column, &input, AIR, STONE);

    assert_eq!(stats.mouths, 0, "a flat column cannot open a mouth");
    assert_eq!(stats.detail_cells, 0, "detail is off in this fixture");
    // Cells 2..=24 are the only ones the floor and roof allow: 23 cells,
    // four taps each. Cells 0 and 1 are below the `F2.3` floor.
    assert_eq!(stats.taps, 23 * u32::from(CAVE_TAP_BUDGET));

    let band = skippable_band(100);
    assert_eq!(band.y_min, 100 - CAVE_MIN_ROOF + 1);
    assert_eq!(band.y_max, WORLD_LAST_Y);
    assert!(cell_skipped(25, 100), "cell 25 starts above the roof");
    assert!(
        !cell_skipped(24, 100),
        "cell 24 still touches the roof band"
    );
    assert!(!cell_skipped(2, 100), "the y = 8 cell is sampled");
    assert!(
        cell_skipped(1, 100),
        "nothing below the F2.3 floor is sampled"
    );
    assert!(cell_skipped(0, 100), "the y = 0 cell is below the floor");
    assert!(cell_skipped(-1, 100), "nothing below y = 1 is sampled");
    for cell in 25..LATTICE_CELLS as i32 {
        assert!(cell_skipped(cell, 100), "cell {cell} is above the roof");
    }
    for cell in -8..0 {
        assert!(cell_skipped(cell, 100), "cell {cell} is below the world");
    }
}

#[test]
#[allow(clippy::float_cmp)]
fn band_fade_is_zero_at_both_edges() {
    let volume = volume(0);
    let input = land(17, 23, 200);
    let low = 200 - DETAIL_DOWN;
    let high = 200 + DETAIL_UP;

    let below = volume.detail_sample(input.x, low, input.z, &input);
    let above = volume.detail_sample(input.x, high, input.z, &input);
    assert_eq!(below.fade, 0.0);
    assert_eq!(above.fade, 0.0);
    assert_eq!(below.delta, 0.0, "the edge contributes exactly nothing");
    assert_eq!(above.delta, 0.0, "the edge contributes exactly nothing");
    assert_eq!(
        volume
            .detail_sample(input.x, low - 1, input.z, &input)
            .delta,
        0.0
    );
    assert_eq!(
        volume
            .detail_sample(input.x, high + 1, input.z, &input)
            .delta,
        0.0
    );

    // Somewhere on the band the noise term is positive; the centre is the
    // natural place, and the fixture picks a column where n3 is positive.
    let mut found = false;
    'scan: for z in -16..16 {
        for x in -16..16 {
            let centre = volume.detail_sample(x, 200, z, &input);
            if centre.fade == 1.0 && centre.delta > 0.0 {
                found = true;
                break 'scan;
            }
        }
    }
    assert!(found, "the detail centre must be capable of raising rock");
}

#[test]
fn detail_cannot_make_a_floater_smaller_than_float_min() {
    let params = VolumeParams {
        cave_t: 1.0,
        detail_amp: 1.0,
        ..VolumeParams::v1_default(21)
    };
    let volume = Volume::new(params).expect("valid");
    let input = land(0, 0, 200);
    let target = 201;

    let mut small = None;
    let mut large = None;
    'scan: for z in 0..64 {
        for x in 0..64 {
            let sample = volume.detail_sample(x, target, z, &input);
            if small.is_none() && sample.delta > 0.02 && sample.delta < volume.params().float_min {
                small = Some((x, z));
            }
            if large.is_none() && sample.delta > 0.4 {
                large = Some((x, z));
            }
            if small.is_some() && large.is_some() {
                break 'scan;
            }
        }
    }
    let small = small.expect("the fixture finds a sub-threshold bump");
    let large = large.expect("the fixture finds a super-threshold bump");

    let mut quiet = solid_column(200);
    let quiet_input = ColumnInput {
        x: small.0,
        z: small.1,
        ..input
    };
    volume.apply(&mut quiet, &quiet_input, AIR, STONE);
    assert_eq!(
        quiet[usize::try_from(target).unwrap()],
        AIR,
        "a bump below FLOAT_MIN must not spawn a block"
    );

    let mut loud = solid_column(200);
    let loud_input = ColumnInput {
        x: large.0,
        z: large.1,
        ..input
    };
    volume.apply(&mut loud, &loud_input, AIR, STONE);
    assert_eq!(
        loud[usize::try_from(target).unwrap()],
        STONE,
        "a bump above FLOAT_MIN spawns a floater"
    );

    // And the invariant behind the two cases, end to end: every block the detail
    // pass solidified above the surface cleared the float gate.
    let mut checked = 0;
    for index in 0..256 {
        let (x, _, z) = sample_coords(23, index);
        #[allow(clippy::cast_possible_truncation)]
        let (block_x, block_z) = (x as i32, z as i32);
        let column_input = ColumnInput {
            x: block_x,
            z: block_z,
            ..input
        };
        let mut filled = solid_column(200);
        volume.apply(&mut filled, &column_input, AIR, STONE);
        for y in target..=200 + DETAIL_UP {
            let index = usize::try_from(y).unwrap();
            if filled[index] == AIR {
                continue;
            }
            let sample = volume.detail_sample(block_x, y, block_z, &column_input);
            #[allow(clippy::cast_precision_loss)]
            let base = ((column_input.macro_height - (y as f32 + 0.5)) / DETAIL_SLOPE_SCALE)
                .clamp(-1.0, 1.0);
            assert!(
                base + sample.delta >= volume.params().float_min,
                "a floater at {block_x} {y} {block_z} is below FLOAT_MIN"
            );
            checked += 1;
        }
    }
    assert!(checked > 0, "the sweep must see at least one floater");
}

#[test]
#[allow(clippy::float_cmp)]
fn entrance_lowers_the_roof_but_not_the_exclusions() {
    let volume = volume(0xABCD);
    let deep_enough = volume.params().entrance_t + ENTRANCE_RAMP;

    let mut mouth = None;
    'scan: for z in 0..96 {
        for x in 0..96 {
            if volume.entrance(x, z) >= deep_enough {
                mouth = Some((x, z));
                break 'scan;
            }
        }
    }
    let (x, z) = mouth.expect("the v1 entrance field has mouths in a 96-block window");

    let open = land(x, z, 200);
    assert!(volume.is_mouth(x, z), "the field must flag this column");
    assert!(volume.mouth_open(&open));
    assert!(
        volume.roof(&open) < volume.params().cave_min_roof as f32,
        "the entrance must lower the roof"
    );
    assert!(volume.roof(&open) < 0.05, "a full mouth opens to the sky");

    // Hard exclusions: the mask never overrides any of them.
    let excluded = [
        ColumnInput {
            top: volume.params().sea + 1,
            ..open
        },
        ColumnInput {
            is_river: true,
            ..open
        },
        ColumnInput {
            is_lake: true,
            ..open
        },
        ColumnInput {
            near_bank: true,
            ..open
        },
        ColumnInput {
            slope: MOUTH_SLOPE_MIN - 1,
            ..open
        },
    ];
    for column in excluded {
        assert!(!volume.mouth_open(&column), "{column:?}");
        assert_eq!(volume.roof(&column), volume.params().cave_min_roof as f32);
    }

    // A mouth column counts once, and an excluded one never counts.
    let mut opened = solid_column(200);
    let stats = volume.apply(&mut opened, &open, AIR, STONE);
    assert_eq!(stats.mouths, 1);
    assert!(stats.cave_cells > 0, "an open mouth must carve something");

    let river = ColumnInput {
        is_river: true,
        ..open
    };
    let mut river_column = solid_column(200);
    let river_stats = volume.apply(&mut river_column, &river, AIR, STONE);
    assert_eq!(river_stats.mouths, 0);

    // The `F2.3` floor keeps everything below `y = 8` solid even at the
    // most aggressive threshold; the carve starts above it.
    let aggressive = Volume::new(VolumeParams {
        cave_t: 0.0,
        ..VolumeParams::v1_default(0xABCD)
    })
    .expect("valid");
    let mut floor_column = solid_column(200);
    aggressive.apply(&mut floor_column, &open, AIR, STONE);
    assert_ne!(floor_column[0], AIR, "bedrock is never carved");
    for y in 1..CHEESE_FLOOR_START {
        assert_ne!(
            floor_column[usize::try_from(y).unwrap()],
            AIR,
            "the cheese floor was breached at y = {y}"
        );
    }
    assert!(
        floor_column
            .iter()
            .skip(usize::try_from(CHEESE_FLOOR_START).unwrap())
            .any(|id| *id == AIR),
        "the carve must start above the cheese floor"
    );
}

#[test]
#[allow(clippy::float_cmp)]
fn mouth_requires_slope() {
    let volume = volume(0xABCD);
    let deep_enough = volume.params().entrance_t + ENTRANCE_RAMP;
    let mut mouth = None;
    'scan: for z in 0..96 {
        for x in 0..96 {
            if volume.entrance(x, z) >= deep_enough {
                mouth = Some((x, z));
                break 'scan;
            }
        }
    }
    let (x, z) = mouth.expect("the fixture finds a mouth candidate");

    for slope in 0..MOUTH_SLOPE_MIN {
        let flat = ColumnInput {
            slope,
            ..land(x, z, 200)
        };
        assert!(volume.is_mouth(x, z), "the field is still high");
        assert!(!volume.mouth_open(&flat), "slope {slope} is not enough");
        assert_eq!(volume.roof(&flat), volume.params().cave_min_roof as f32);
        let mut column = solid_column(200);
        assert_eq!(volume.apply(&mut column, &flat, AIR, STONE).mouths, 0);
    }

    let steep = ColumnInput {
        slope: MOUTH_SLOPE_MIN,
        ..land(x, z, 200)
    };
    assert!(volume.mouth_open(&steep));
    assert!(volume.roof(&steep) < volume.params().cave_min_roof as f32);
}

#[test]
fn one_span_matches_the_surface_stack() {
    let column = solid_column(100);
    let spans = resolve_spans(&column, AIR);
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].y0, 0);
    assert_eq!(spans[0].y1, 100);
    assert_eq!(spans[0].kind, SpanKind::Surface);
    assert_eq!(span_rule(spans[0].kind, true, true), SpanRule::Stack);
}

#[test]
fn spans_split_at_air() {
    let mut column = solid_column(100);
    for y in 60..=69 {
        column[usize::try_from(y).unwrap()] = AIR;
    }
    let spans = resolve_spans(&column, AIR);
    assert_eq!(spans.len(), 2);
    assert_eq!(
        spans[0],
        Span {
            y0: 70,
            y1: 100,
            kind: SpanKind::Ceiling
        }
    );
    assert_eq!(
        spans[1],
        Span {
            y0: 0,
            y1: 59,
            kind: SpanKind::CaveFloor
        }
    );
    // The ceiling keeps the full stack because it is the highest span; the cave
    // floor below it does not.
    assert_eq!(span_rule(spans[0].kind, true, false), SpanRule::Stack);
    assert_eq!(span_rule(spans[1].kind, false, false), SpanRule::CaveFloor);
}

#[test]
fn cave_floor_never_gets_snow_or_beach() {
    let mut column = solid_column(100);
    for y in 80..=89 {
        column[usize::try_from(y).unwrap()] = AIR;
    }
    for y in 40..=49 {
        column[usize::try_from(y).unwrap()] = AIR;
    }
    let spans = resolve_spans(&column, AIR);
    assert_eq!(spans.len(), 3);
    assert_eq!(spans[0].kind, SpanKind::Ceiling);
    assert_eq!(spans[1].kind, SpanKind::CaveFloor);
    assert_eq!(spans[2].kind, SpanKind::CaveFloor);

    for span in spans.iter().skip(1) {
        let rule = span_rule(span.kind, false, false);
        assert_ne!(rule, SpanRule::Stack, "a lower top never gets the stack");
        assert!(matches!(rule, SpanRule::CaveFloor | SpanRule::Stone));
        assert!(
            span.y1 < spans[0].y1,
            "a lower span must sit below the top span"
        );
    }
    // Only the full stack can produce snow (slot 4) or beach (slot 3).
    assert_eq!(span_rule(SpanKind::Surface, true, false), SpanRule::Stack);
    // An open lower top may take topsoil, but still never snow or beach.
    assert_eq!(
        span_rule(SpanKind::CaveFloor, false, true),
        SpanRule::OpenFloor
    );
    // A lower ceiling is always stone.
    assert_eq!(span_rule(SpanKind::Ceiling, false, true), SpanRule::Stone);
}

#[test]
fn open_to_sky_tracks_the_visible_column() {
    let open = solid_column(100);
    assert!(open_to_sky(&open, AIR, 100));

    // A roofed pocket: the solid above the void blocks the sky.
    let mut pocket = solid_column(100);
    for y in 60..=69 {
        pocket[usize::try_from(y).unwrap()] = AIR;
    }
    assert!(!open_to_sky(&pocket, AIR, 59));
    assert!(open_to_sky(&pocket, AIR, 100));

    // A shaft through the surface: the cell under it is open even though the
    // column above it is mostly solid.
    let mut shaft = solid_column(100);
    for y in 51..=100 {
        shaft[usize::try_from(y).unwrap()] = AIR;
    }
    assert!(open_to_sky(&shaft, AIR, 50));
    assert!(!open_to_sky(&shaft, AIR, 0));

    // A pocket under a one-block roof is not open.
    let mut lid = solid_column(100);
    for y in 61..=99 {
        lid[usize::try_from(y).unwrap()] = AIR;
    }
    assert!(!open_to_sky(&lid, AIR, 60));
}

#[test]
fn caves_never_breach_bedrock() {
    let aggressive = Volume::new(VolumeParams {
        cave_t: 0.2,
        ..VolumeParams::v1_default(101)
    })
    .expect("valid");
    let tag = domain("volume-bedrock-sweep");
    for index in 0..1_000_i32 {
        let x = hash01(hash3(3, tag, index, 0, 9)) * 8192.0 - 4096.0;
        let z = hash01(hash3(3, tag, index, 1, 8)) * 8192.0 - 4096.0;
        #[allow(clippy::cast_possible_truncation)]
        let (block_x, block_z) = (x as i32, z as i32);
        #[allow(clippy::cast_precision_loss)]
        let top = 60 + (hash01(hash3(3, tag, index, 2, 7)) * 260.0) as i32;
        let input = ColumnInput {
            slope: 4,
            rough: 0.8,
            ..land(block_x, block_z, top)
        };
        let mut column = solid_column(top);
        aggressive.apply(&mut column, &input, AIR, STONE);
        assert_ne!(column[0], AIR, "column {index} breached bedrock");
        assert_eq!(column[0], STONE);
    }
}

#[test]
fn lattice_cell_agrees_with_macro_cell() {
    for y in [
        i32::MIN + 1,
        -1025,
        -5,
        -4,
        -1,
        0,
        1,
        3,
        4,
        383,
        4096,
        i32::MAX - 1,
    ] {
        assert_eq!(lattice_cell(y), macro_cell(y), "y = {y}");
    }
    assert_eq!(lattice_cell(-1), (-1, 0.75));
    assert_eq!(lattice_cell(0), (0, 0.0));
    assert_eq!(lattice_cell(3), (0, 0.75));
    assert_eq!(lattice_cell(4), (1, 0.0));
}

#[test]
fn params_validation_rejects_bad_values() {
    assert!(VolumeParams::v1_default(1).validate().is_ok());

    let cases = [
        VolumeParams {
            cave_t: 1.5,
            ..VolumeParams::v1_default(1)
        },
        VolumeParams {
            entrance_t: f32::NAN,
            ..VolumeParams::v1_default(1)
        },
        VolumeParams {
            cave_min_roof: -1,
            ..VolumeParams::v1_default(1)
        },
        VolumeParams {
            detail_down: -1,
            ..VolumeParams::v1_default(1)
        },
        VolumeParams {
            detail_up: -1,
            ..VolumeParams::v1_default(1)
        },
        VolumeParams {
            detail_amp: -0.5,
            ..VolumeParams::v1_default(1)
        },
        VolumeParams {
            float_min: 1.5,
            ..VolumeParams::v1_default(1)
        },
        VolumeParams {
            sea: 0,
            ..VolumeParams::v1_default(1)
        },
    ];
    for params in cases {
        let error = params.validate().expect_err("invalid params are rejected");
        assert_eq!(error.stage, Stage::Volume);
        assert!(matches!(error.kind, ErrorKind::DescriptionInvalid { .. }));
        assert!(Volume::new(params).is_err());
    }
}

#[test]
fn tunnel_radius_at_least_three_blocks() {
    // A tunnel axis is a cell where both spaghetti fields are near neutral, so
    // `tunnel` is a local maximum in all six directions. The six chord half-widths
    // average out the chord geometry of an oblique tunnel; the median over sampled
    // axes is the design radius. A level set can pinch, so the median, not the
    // worst single cell, is what `F2.1` promises.
    let volume = volume(0x5EED);
    let threshold = volume.params().cave_t;
    let top = crate::description::MAX_TERRAIN_Y;
    let mut means = Vec::new();

    for z in (0..192).step_by(2) {
        for x in (0..192).step_by(2) {
            let mut y = (SPAGHETTI_START_Y + 32) as f32;
            while y < (SPAGHETTI_FLAT_END_Y - 32) as f32 {
                #[allow(clippy::cast_precision_loss)]
                let (fx, fz) = (x as f32, z as f32);
                if is_tunnel_axis(&volume, fx, y, fz, top) {
                    let mean = tunnel_radius(&volume, fx, y, fz, top, threshold);
                    means.push(mean);

                    if means.len() == 32 {
                        break;
                    }
                }
                y += 2.0;
            }
            if means.len() == 32 {
                break;
            }
        }
        if means.len() == 32 {
            break;
        }
    }
    assert!(means.len() >= 8, "found only {} tunnel axes", means.len());
    means.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let lower_quartile = means[means.len() / 4];
    let median = means[means.len() / 2];
    assert!(
        median >= 3.0,
        "median tunnel radius is {median} blocks: {means:?}"
    );
    assert!(
        lower_quartile >= 3.0,
        "a quarter of tunnels are under three blocks: {means:?}"
    );
    assert!(
        *means.last().expect("non-empty") >= 5.0,
        "no wide tunnel in the sample: {means:?}"
    );
}

/// Whether `(x, y, z)` is a spaghetti tunnel axis.
fn is_tunnel_axis(volume: &Volume, x: f32, y: f32, z: f32, top: i32) -> bool {
    let here = volume.spaghetti(x, y, z, top);
    here > 0.9
        && here > volume.spaghetti(x + 0.5, y, z, top)
        && here > volume.spaghetti(x - 0.5, y, z, top)
        && here > volume.spaghetti(x, y + 0.5, z, top)
        && here > volume.spaghetti(x, y - 0.5, z, top)
        && here > volume.spaghetti(x, y, z + 0.5, top)
        && here > volume.spaghetti(x, y, z - 0.5, top)
}

/// Mean half-width of the tunnel through `(x, y, z)` along the six axes.
fn tunnel_radius(volume: &Volume, x: f32, y: f32, z: f32, top: i32, threshold: f32) -> f32 {
    let mut sum = 0.0;
    for (dx, dy, dz) in [
        (1.0, 0.0, 0.0),
        (-1.0, 0.0, 0.0),
        (0.0, 1.0, 0.0),
        (0.0, -1.0, 0.0),
        (0.0, 0.0, 1.0),
        (0.0, 0.0, -1.0),
    ] {
        sum += walk_to_edge(volume, x, y, z, dx, dy, dz, top, threshold);
    }
    sum / 6.0
}

/// Distance from `(x, y, z)` to the first cell at or below `threshold`.
#[allow(clippy::too_many_arguments)]
fn walk_to_edge(
    volume: &Volume,
    x: f32,
    y: f32,
    z: f32,
    dx: f32,
    dy: f32,
    dz: f32,
    top: i32,
    threshold: f32,
) -> f32 {
    let mut step = 1.0_f32;
    while step <= 64.0 {
        if volume.spaghetti(x + dx * step, y + dy * step, z + dz * step, top) <= threshold {
            return step - 1.0;
        }
        step += 1.0;
    }
    64.0
}
