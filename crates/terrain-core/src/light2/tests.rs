//! Column light tests, including the fixed-seed output pin.

use super::bfs::{PADDED_CELLS, Ring};
use super::column::section_cell;
use super::*;
use crate::hash::{domain, hash64};
use crate::section::{BlockId, Section};

/// Builds all 12 sections of a column from a block function.
fn column(f: impl Fn(i32, i32, i32) -> BlockId) -> [Section; COLUMN_SECTIONS] {
    std::array::from_fn(|section| section_from(section, &f))
}

fn section_from(section: usize, f: &impl Fn(i32, i32, i32) -> BlockId) -> Section {
    let mut blocks = vec![0_u16; SECTION_CELLS].into_boxed_slice();
    let base = i32::try_from(section).expect("a section index fits i32") * 32;
    for y in 0..32_i32 {
        for z in 0..32_i32 {
            for x in 0..32_i32 {
                let index = section_cell(
                    usize::try_from(x).expect("an interior x is non-negative"),
                    usize::try_from(y).expect("a local y is non-negative"),
                    usize::try_from(z).expect("an interior z is non-negative"),
                );
                blocks[index] = f(x, base + y, z);
            }
        }
    }
    let array: Box<[BlockId; SECTION_CELLS]> = blocks
        .try_into()
        .expect("the scratch matches the section volume");
    Section::from_blocks(&array)
}

fn column_blocks(f: impl Fn(i32, i32, i32) -> BlockId) -> ColumnBlocks {
    ColumnBlocks::new(column(f), &[None; 8])
}

#[test]
fn open_column_is_full_sky() {
    let blocks = column_blocks(|_, _, _| AIR_ID);
    let light = compute_column(&blocks, &default_table());

    for y in 0..COLUMN_HEIGHT {
        for z in 0..32 {
            for x in 0..32 {
                assert_eq!(light.sky(x, y, z), 15, "sky {x},{y},{z}");
                assert_eq!(light.block(x, y, z), 0, "block {x},{y},{z}");
            }
        }
    }
    for section in 0..COLUMN_SECTIONS {
        assert_eq!(light.sky_slot(section).is_uniform(), Some(15));
        assert_eq!(light.block_slot(section).is_uniform(), Some(0));
        assert!(!light.sky_slot(section).is_packed());
    }
}

#[test]
fn padded_section_reads_vertical_neighbours_and_mirrors_edges() {
    let table = default_table();
    let blocks = column_blocks(|_, y, _| if y <= 100 { 1 } else { AIR_ID });
    let light = compute_column(&blocks, &table);
    let padded = light.padded_section(3);

    assert_eq!(
        padded.get(5, 0, 7),
        (light.sky(5, 96, 7), light.block(5, 96, 7)),
        "the interior is the section itself"
    );
    assert_eq!(
        padded.get(5, 32, 7),
        (light.sky(5, 128, 7), light.block(5, 128, 7)),
        "the top pad reads the section above"
    );
    assert_eq!(
        padded.get(5, -1, 7),
        (light.sky(5, 95, 7), light.block(5, 95, 7)),
        "the bottom pad reads the section below"
    );
    assert_eq!(
        padded.get(-1, 5, 7),
        (light.sky(0, 101, 7), light.block(0, 101, 7)),
        "the horizontal pad mirrors the edge row"
    );
    assert_eq!(
        padded.get(32, 5, 7),
        (light.sky(31, 101, 7), light.block(31, 101, 7))
    );
    assert_eq!(
        padded.get(-1, 32, -1),
        (light.sky(0, 128, 0), light.block(0, 128, 0)),
        "a corner combines horizontal mirroring with the vertical neighbour"
    );

    assert_eq!(light.padded_section(0).get(0, -1, 0), (0, 0), "below world");
    assert_eq!(
        light.padded_section(COLUMN_SECTIONS - 1).get(0, 32, 0),
        (LIGHT_LEVELS, 0),
        "above world"
    );
}

#[test]
fn roof_shadows_the_column() {
    let blocks = column_blocks(|_, y, _| if y == 100 { 1 } else { AIR_ID });
    let light = compute_column(&blocks, &default_table());

    for y in 0..COLUMN_HEIGHT {
        for z in 0..32 {
            for x in 0..32 {
                let expected = if y > 100 { 15 } else { 0 };
                assert_eq!(light.sky(x, y, z), expected, "sky {x},{y},{z}");
                assert_eq!(light.block(x, y, z), 0, "block {x},{y},{z}");
            }
        }
    }
}

#[test]
fn vertical_propagation_has_no_loss() {
    let blocks = column_blocks(|x, y, z| {
        if x == 16 && z == 16 {
            AIR_ID
        } else if y <= 200 {
            1
        } else {
            AIR_ID
        }
    });
    let light = compute_column(&blocks, &default_table());

    for y in 0..=200 {
        assert_eq!(light.sky(16, y, 16), 15, "shaft at {y}");
    }
    assert_eq!(light.sky(15, 100, 16), 0, "solid below the top");
    assert_eq!(light.sky(17, 100, 16), 0);
    assert_eq!(light.sky(16, 201, 16), 15);
}

#[test]
fn lateral_decay_is_one_per_step() {
    let blocks = column_blocks(|x, y, z| {
        if y >= 202 {
            AIR_ID
        } else if y == 201 {
            if x == 0 && z == 16 { AIR_ID } else { 1 }
        } else if y == 200 {
            if z == 16 && x <= 10 { AIR_ID } else { 1 }
        } else {
            1
        }
    });
    let light = compute_column(&blocks, &default_table());

    for x in 0..=10 {
        assert_eq!(
            light.sky(x, 200, 16),
            15 - u8::try_from(x).expect("x fits u8"),
            "tunnel at x={x}"
        );
    }
    for x in 11..32 {
        assert_eq!(light.sky(x, 200, 16), 0, "dead end at x={x}");
    }
}

#[test]
fn water_attenuates_sky_only() {
    let blocks = column_blocks(|x, y, z| {
        if x != 16 || z != 16 {
            return if y <= 111 { 1 } else { AIR_ID };
        }
        match y {
            99 => LAMP_ID,
            100..=110 => WATER_ID,
            _ => AIR_ID,
        }
    });
    let light = compute_column(&blocks, &default_table());

    assert_eq!(light.sky(16, 111, 16), 15, "air above the water");
    for y in 100..=110 {
        let depth = u8::try_from(111 - y).expect("depth fits u8");
        assert_eq!(light.sky(16, y, 16), 15 - depth, "sky at {y}");
    }
    for y in 99..=110 {
        let step = u8::try_from(y - 99).expect("step fits u8");
        assert_eq!(light.block(16, y, 16), 14 - step, "block at {y}");
    }
}

#[test]
fn vertical_sky_loses_water_attenuation_per_block() {
    // Four water blocks on the stone surface, open to the sky and with open
    // air beside them. If a sideways step could enter water, each cell would
    // be refreshed to 14 from its neighbour; `L2.4` says the depth charge
    // must survive.
    let blocks = column_blocks(|x, y, z| {
        if x == 16 && z == 16 && (100..=103).contains(&y) {
            WATER_ID
        } else if y <= 99 {
            1
        } else {
            AIR_ID
        }
    });
    let light = compute_column(&blocks, &default_table());

    assert_eq!(light.sky(16, 104, 16), 15, "air above the column");
    assert_eq!(light.sky(16, 103, 16), 14, "the surface water");
    assert_eq!(light.sky(16, 102, 16), 13);
    assert_eq!(light.sky(16, 101, 16), 12);
    assert_eq!(light.sky(16, 100, 16), 11, "the deepest water");
    assert_eq!(light.sky(16, 99, 16), 0, "the block under the column");
}

#[test]
fn water_column_is_darker_with_depth() {
    let bottom = |depth: i32| {
        let blocks = column_blocks(|x, y, z| {
            if x == 16 && z == 16 && ((104 - depth)..=103).contains(&y) {
                WATER_ID
            } else if y <= 99 {
                1
            } else {
                AIR_ID
            }
        });
        let light = compute_column(&blocks, &default_table());
        light.sky(16, 104 - depth, 16)
    };

    let shallow = bottom(1);
    let deep = bottom(4);
    assert_eq!(shallow, 14, "the bottom of a 1-deep column");
    assert_eq!(deep, 11, "the bottom of a 4-deep column");
    assert_eq!(
        shallow - deep,
        3,
        "three extra water blocks cost three levels"
    );
}

#[test]
fn incremental_equals_full_recompute() {
    const EDITS: u64 = 20_000;
    const BATCH: usize = 500;
    let tag = domain("light2:fuzz");
    let table = default_table();
    let mut blocks = column_blocks(|_, y, _| if y <= 120 { 1 } else { AIR_ID });
    let mut light = compute_column(&blocks, &table);
    let mut edits = Vec::with_capacity(BATCH);
    let mut changed_batches = 0_u32;

    for step in 0..EDITS {
        let hash = hash64(7, tag, &[i64::try_from(step).expect("step fits i64")]);
        let x = i32::try_from(hash % 32).expect("x fits");
        let z = i32::try_from((hash >> 8) % 32).expect("z fits");
        let y = i32::try_from((hash >> 16) % 200).expect("y fits");
        let id = match (hash >> 32) % 7 {
            0 => AIR_ID,
            1 => WATER_ID,
            2 => LEAVES_ID,
            3 => LAMP_ID,
            4 => 1,
            5 => 3,
            _ => 7,
        };
        blocks.set(x, y, z, id);
        edits.push(Edit::new(x, y, z, id));
        let last = step + 1 == EDITS;
        if edits.len() < BATCH && !last {
            continue;
        }

        let before = light.clone();
        let changed = light.relight_column(&blocks, &table, &edits);
        let fresh = compute_column(&blocks, &table);
        assert_eq!(light, fresh, "incremental diverged at edit {step}");
        assert_eq!(
            light.packed_bytes(),
            fresh.packed_bytes(),
            "slot representation diverged at edit {step}"
        );
        assert_eq!(
            changed,
            diff_count(&before, &light),
            "changed count at edit {step}"
        );
        if changed > 0 {
            changed_batches += 1;
        }
        edits.clear();
    }

    assert!(changed_batches > 0, "the fuzz never changed light");
    assert_eq!(
        light.relight_column(&blocks, &table, &[]),
        0,
        "an empty batch is a no-op"
    );
}

fn diff_count(before: &ColumnLight, after: &ColumnLight) -> u32 {
    let mut changed = 0_u32;
    for section in 0..COLUMN_SECTIONS {
        for cell in 0..SECTION_CELLS {
            if before.sky_slot(section).get(cell) != after.sky_slot(section).get(cell)
                || before.block_slot(section).get(cell) != after.block_slot(section).get(cell)
            {
                changed += 1;
            }
        }
    }
    changed
}

#[test]
fn section_differs_names_only_the_changed_section() {
    let table = default_table();
    let before = compute_column(
        &column_blocks(|_, y, _| if y <= 120 { 1 } else { AIR_ID }),
        &table,
    );
    let after = compute_column(
        &column_blocks(|x, y, z| {
            let top = if x == 5 && z == 5 { 121 } else { 120 };
            if y <= top { 1 } else { AIR_ID }
        }),
        &table,
    );

    assert!(
        before.section_differs(&after, 3),
        "section 3 holds the change"
    );
    for section in (0..COLUMN_SECTIONS).filter(|section| *section != 3) {
        assert!(
            !before.section_differs(&after, section),
            "section {section} is unchanged"
        );
    }
    assert!(!before.section_differs(&before.clone(), 3));
}

#[test]
fn lateral_light_falls_straight_down_without_loss() {
    // A roofed vertical shaft is lit only through a side tunnel at y=300.
    // Sky light that arrives sideways must still run down the shaft with no
    // loss, exactly like the pass-1 seed.
    let blocks = column_blocks(|x, y, z| {
        if z != 16 {
            return 1;
        }
        match (x, y) {
            (16, 100..=382) | (17..=20, 300) | (20, 300..=383) => AIR_ID,
            _ => 1,
        }
    });
    let light = compute_column(&blocks, &default_table());

    assert_eq!(light.sky(20, 300, 16), 15, "the open shaft");
    assert_eq!(light.sky(19, 300, 16), 14);
    assert_eq!(light.sky(16, 300, 16), 11);
    for y in 100..=300 {
        assert_eq!(light.sky(16, y, 16), 11, "shaft at {y}");
    }
    assert_eq!(light.sky(16, 99, 16), 0, "the floor");
}

#[test]
fn uniform_promotion_and_demotion() {
    let table = default_table();
    let mut blocks = column_blocks(|_, _, _| AIR_ID);
    let mut light = compute_column(&blocks, &table);
    assert!(light.is_uniform());

    // One dark cell reallocates the touched section. Open air below the
    // block reads 14 from its neighbours, so a whole section below it
    // becomes a uniform 14 slot.
    blocks.set(16, 200, 16, 1);
    let before = light.clone();
    let edits = [Edit::new(16, 200, 16, 1)];
    let changed = light.relight_column(&blocks, &table, &edits);
    assert_eq!(changed, diff_count(&before, &light));
    assert_eq!(light.sky(16, 200, 16), 0);
    assert_eq!(light.sky(16, 199, 16), 14);
    assert!(light.sky_slot(6).is_packed());
    assert_eq!(light.sky_slot(6).is_uniform(), None);
    assert_eq!(light.sky_slot(6).get(section_cell(16, 8, 16)), 0);
    assert_eq!(light.sky(16, 10, 16), 14);
    assert_eq!(light.sky(15, 10, 16), 15);

    blocks.set(16, 200, 16, AIR_ID);
    let edits = [Edit::new(16, 200, 16, AIR_ID)];
    light.relight_column(&blocks, &table, &edits);
    assert!(light.is_uniform(), "the section must collapse again");
    assert_eq!(light.sky_slot(6).is_uniform(), Some(15));
    assert!(!light.sky_slot(6).is_packed());
}

#[test]
fn border_light_crosses_the_seam() {
    let a_sections = column(|x, y, z| {
        let shaft = x == 31 && z == 16;
        if y >= 100 || shaft { AIR_ID } else { 1 }
    });
    let b_sections = column(|x, y, z| {
        let shaft = x == 0 && z == 16;
        if y >= 100 || shaft { AIR_ID } else { 1 }
    });
    let a_copy = a_sections.clone();
    let mut borders_a: [Option<&[Section; COLUMN_SECTIONS]>; 8] = [None; 8];
    borders_a[4] = Some(&b_sections);
    let a = ColumnBlocks::new(a_sections, &borders_a);
    let mut borders_b: [Option<&[Section; COLUMN_SECTIONS]>; 8] = [None; 8];
    borders_b[3] = Some(&a_copy);
    let b = ColumnBlocks::new(b_sections, &borders_b);

    let table = default_table();
    let light_a = compute_column(&a, &table);
    let light_b = compute_column(&b, &table);

    for y in 0..COLUMN_HEIGHT {
        for z in 0..32 {
            assert_eq!(
                light_a.sky(31, y, z),
                light_b.sky(0, y, z),
                "seam disagreement at {y},{z}"
            );
            assert_eq!(light_a.block(31, y, z), light_b.block(0, y, z));
        }
    }
    assert_eq!(light_a.sky(31, 50, 16), 15, "the shared shaft is open");
    assert_eq!(light_b.sky(0, 50, 16), 15);
    assert_eq!(light_a.sky(31, 50, 17), 0, "and the wall is dark");
}

#[test]
fn queue_has_a_fixed_capacity() {
    let mut ring = Ring::new();
    for entry in 0..u32::try_from(QUEUE_CAPACITY).expect("the capacity fits u32") {
        assert!(ring.push(entry), "push {entry}");
    }
    assert!(!ring.push(0), "a full ring refuses the push");
    assert_eq!(ring.pop(), Some(0));
    assert!(ring.push(0), "a pop makes room again");
    assert_eq!(ring.pop(), Some(1));
}

#[test]
fn top_span_skips_open_air() {
    let table = default_table();
    let mut blocks = column_blocks(|_, y, _| if y <= 8 { 1 } else { AIR_ID });

    let (plain, plain_stats) = compute_column_stats(&blocks, &table);
    assert_eq!(plain_stats.vertical_cells, PADDED_CELLS);

    blocks.set_top_span(8);
    let (hinted, hinted_stats) = compute_column_stats(&blocks, &table);
    assert_eq!(hinted, plain, "the hint must not change light");
    assert!(
        hinted_stats.vertical_cells < 100_000,
        "the hint visited {} cells",
        hinted_stats.vertical_cells
    );
    assert!(hinted_stats.vertical_cells > 0);
}

/// Pins one fixed seed's whole-column light.
///
/// The block field comes from [`hash64`] alone, so the pin does not depend
/// on the generator. Sky, block, and the interleaved digest are pinned
/// separately: a change in either channel names itself.
#[test]
fn fixed_seed_light_matches_the_pin() {
    const SEED: u64 = 0x5EED_1127;
    const SKY_PIN: u64 = 0xd10a_0390_d09a_3326;
    const BLOCK_PIN: u64 = 0xc536_0f12_3e1a_9c75;
    const COMBINED_PIN: u64 = 0x84d8_c396_bd39_720c;

    let tag = domain("light2:pin");
    let blocks = column_blocks(|x, y, z| {
        let hash = hash64(SEED, tag, &[i64::from(x), i64::from(y), i64::from(z)]);
        match hash % 13 {
            0 => AIR_ID,
            1 => WATER_ID,
            2 => LEAVES_ID,
            3 => LAMP_ID,
            4 => 1,
            _ => 3,
        }
    });
    let light = compute_column(&blocks, &default_table());

    let cells = COLUMN_SECTIONS * SECTION_CELLS;
    let mut sky = Vec::with_capacity(cells);
    let mut block = Vec::with_capacity(cells);
    let mut combined = Vec::with_capacity(2 * cells);
    for section in 0..COLUMN_SECTIONS {
        for cell in 0..SECTION_CELLS {
            let sky_value = i64::from(light.sky_slot(section).get(cell));
            let block_value = i64::from(light.block_slot(section).get(cell));
            sky.push(sky_value);
            block.push(block_value);
            combined.push(sky_value);
            combined.push(block_value);
        }
    }

    assert_eq!(
        hash64(0, domain("light2:pin:sky"), &sky),
        SKY_PIN,
        "sky channel"
    );
    assert_eq!(
        hash64(0, domain("light2:pin:block"), &block),
        BLOCK_PIN,
        "block channel"
    );
    assert_eq!(
        hash64(0, domain("light2:pin:combined"), &combined),
        COMBINED_PIN,
        "interleaved channels"
    );
}
