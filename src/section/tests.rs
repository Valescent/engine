use std::collections::HashSet;

use super::*;

fn chunk_at(world: IVec3) -> ChunkCoord {
    ChunkCoord::from_world(world).0
}

#[test]
fn from_world_uses_euclidean_division_for_negative_coordinates() {
    let cases = [
        (1, IVec3::new(-1, 0, -65), IVec3::new(-1, 0, -2)),
        (1, IVec3::new(63, 63, 63), IVec3::ZERO),
        (1, IVec3::new(64, 64, 64), IVec3::splat(1)),
        (2, IVec3::new(-129, 127, -128), IVec3::new(-2, 0, -1)),
        (2, IVec3::new(-1, -1, -1), IVec3::splat(-1)),
    ];

    for (level, world, coord) in cases {
        let key = SectionKey::from_world(level, world);
        assert_eq!(key.level, level);
        assert_eq!(key.coord, coord, "{world}");
        let origin = key.origin();
        assert!(origin.cmple(world).all(), "{world} before {origin}");
        assert!(
            world.cmplt(origin + IVec3::splat(key.extent())).all(),
            "{world} after the section at {origin}"
        );
    }
}

#[test]
fn a_chunk_on_a_section_corner_maps_to_one_section_per_level() {
    // Chunk (2, 0, 2) starts at world (64, 0, 64), the minimum corner of
    // level-1 section (1, 0, 1) and level-2 section (0, 0, 0).
    let keys = overlapping_sections_for_chunk(ChunkCoord::new(2, 0, 2));

    assert_eq!(
        keys,
        vec![
            SectionKey::new(1, IVec3::new(1, 0, 1)),
            SectionKey::new(2, IVec3::new(0, 0, 0)),
            SectionKey::new(3, IVec3::new(0, 0, 0)),
            SectionKey::new(4, IVec3::new(0, 0, 0)),
        ]
    );
}

#[test]
fn a_box_crossing_a_boundary_overlaps_two_sections_per_axis() {
    let keys = overlapping_sections(IVec3::new(63, 0, 63), IVec3::new(64, 0, 64));
    let level1: Vec<SectionKey> = keys.iter().copied().filter(|key| key.level == 1).collect();

    assert_eq!(
        level1,
        vec![
            SectionKey::new(1, IVec3::new(0, 0, 0)),
            SectionKey::new(1, IVec3::new(0, 0, 1)),
            SectionKey::new(1, IVec3::new(1, 0, 0)),
            SectionKey::new(1, IVec3::new(1, 0, 1)),
        ]
    );
    for level in 2..=MAX_LEVEL {
        assert_eq!(
            keys.iter().filter(|key| key.level == level).count(),
            1,
            "level {level}"
        );
    }
}

#[test]
fn an_edit_invalidates_all_four_section_levels() {
    let chunk = ChunkCoord::new(40, 0, -60);
    let chunk_min = chunk.world_origin();
    let chunk_max = chunk_min + IVec3::splat(CHUNK_SIZE - 1);
    let keys = overlapping_sections_for_chunk(chunk);
    let levels: HashSet<u8> = keys.iter().map(|key| key.level).collect();

    assert_eq!(levels, HashSet::from([1, 2, 3, 4]));
    for key in keys {
        assert!(key.origin().cmple(chunk_max).all(), "{key:?}");
        assert!(
            (key.origin() + IVec3::splat(key.extent()))
                .cmpgt(chunk_min)
                .all(),
            "{key:?}"
        );
    }
}

#[test]
fn level_math_round_trips_parent_and_children() {
    let root = SectionKey::new(2, IVec3::new(-3, 1, 5));
    assert_eq!(root.scale(), 4);
    assert_eq!(root.extent(), 128);
    assert_eq!(root.origin(), IVec3::new(-384, 128, 640));
    assert_eq!(
        root.parent(),
        Some(SectionKey::new(3, IVec3::new(-2, 0, 2)))
    );
    assert_eq!(SectionKey::new(MAX_LEVEL, IVec3::ZERO).parent(), None);

    for index in 0..8 {
        let child = root.child(index).expect("child");
        assert_eq!(child.level, 1);
        assert_eq!(child.parent(), Some(root), "child {index}");
    }
    assert_eq!(root.child(8), None);
    assert_eq!(SectionKey::new(0, IVec3::ZERO).child(0), None);
}

#[test]
fn level1_section_matches_the_exhaustive_seed_rule() {
    let generator = Generator::new(42);
    let key = SectionKey::new(1, IVec3::new(0, 1, 0));
    let section = generate_section(&generator, key, &SectionEdits::new());
    let scale = key.scale();

    for sx in (0..SECTION_SIZE).step_by(8) {
        for sz in (0..SECTION_SIZE).step_by(8) {
            for sy in 0..SECTION_SIZE {
                let base = key.origin() + IVec3::new(sx, sy, sz) * scale;
                let expected = exhaustive_seed_sample(&generator, base, scale);
                assert_eq!(
                    section.sample(IVec3::new(sx, sy, sz)),
                    expected,
                    "sample {sx},{sy},{sz}"
                );
            }
        }
    }
}

fn exhaustive_seed_sample(generator: &Generator, base: IVec3, scale: i32) -> Block {
    let mut solid = None;
    let mut water = false;
    for dy in 0..scale {
        for dx in 0..scale {
            for dz in 0..scale {
                let block = seed_block(generator, base + IVec3::new(dx, dy, dz));
                if block.is_solid() {
                    solid = Some(block);
                } else if block == Block::Water {
                    water = true;
                }
            }
        }
    }
    solid.unwrap_or(if water { Block::Water } else { Block::Air })
}

#[test]
fn water_fills_samples_between_the_solid_top_and_sea_level() {
    let generator = Generator::new(7);
    let sea_sample = SEA_LEVEL.div_euclid(2);
    let mut checked = false;

    'sections: for cx in -16..=16 {
        for cz in -16..=16 {
            let key = SectionKey::new(1, IVec3::new(cx, 0, cz));
            let section = generate_section(&generator, key, &SectionEdits::new());
            for sx in 0..SECTION_SIZE {
                for sz in 0..SECTION_SIZE {
                    let top = (0..SECTION_SIZE)
                        .rev()
                        .find(|&sy| section.sample(IVec3::new(sx, sy, sz)).is_solid());
                    if let Some(top) = top
                        && top < sea_sample
                        && section.sample(IVec3::new(sx, sea_sample, sz)) == Block::Water
                    {
                        assert_eq!(
                            section.sample(IVec3::new(sx, sea_sample + 1, sz)),
                            Block::Air
                        );
                        checked = true;
                        break 'sections;
                    }
                }
            }
        }
    }
    assert!(checked, "a below-sea column");
}

#[test]
fn edits_raise_and_clear_a_sample() {
    let generator = Generator::new(42);
    let key = SectionKey::new(1, IVec3::new(0, 1, 0));
    let world = IVec3::new(1, 70, 1);
    let chunk = chunk_at(world);
    let (_, local) = ChunkCoord::from_world(world);
    let sample = IVec3::new(
        (world.x - key.origin().x).div_euclid(2),
        (world.y - key.origin().y).div_euclid(2),
        (world.z - key.origin().z).div_euclid(2),
    );

    let mut edited = Chunk::default();
    edited.set_block(local, Block::Stone);
    let mut edits = SectionEdits::new();
    edits.insert(chunk, edited);
    let section = generate_section(&generator, key, &edits);
    assert_eq!(section.sample(sample), Block::Stone);

    edits.insert(chunk, Chunk::default());
    let cleared = generate_section(&generator, key, &edits);
    assert_eq!(cleared.sample(sample), Block::Air);
}

#[test]
fn coarse_light_is_open_above_the_top_and_dark_below() {
    let generator = Generator::new(42);
    let key = SectionKey::new(1, IVec3::new(0, 0, 0));
    let section = generate_section(&generator, key, &SectionEdits::new());
    let light = section.light();

    let mut checked = false;
    for x in 0..SECTION_SIZE {
        for z in 0..SECTION_SIZE {
            let top = (0..SECTION_SIZE)
                .rev()
                .find(|&y| section.sample(IVec3::new(x, y, z)).is_solid());
            if let Some(top) = top
                && top + 1 < SECTION_SIZE
            {
                assert_eq!(light.sky(IVec3::new(x, top, z)), 0);
                assert_eq!(light.sky(IVec3::new(x, top + 1, z)), 15);
                checked = true;
            }
        }
    }
    assert!(checked, "a column with a top");
}

#[test]
fn coarse_water_cells_are_open_sky() {
    let mut section = Section::new(SectionKey::new(1, IVec3::ZERO));
    section.set_sample(IVec3::new(1, 5, 1), Block::Water);
    let light = section.light();
    assert_eq!(light.sky(IVec3::new(1, 5, 1)), 15);

    section.set_sample(IVec3::new(1, 4, 1), Block::Stone);
    let light = section.light();
    assert_eq!(light.sky(IVec3::new(1, 5, 1)), 15, "water stays open");
    assert_eq!(light.sky(IVec3::new(1, 4, 1)), 0, "the seabed is solid");
}

#[test]
fn sky_light_spreads_into_a_cliff_recess_and_falls_off() {
    let mut section = Section::new(SectionKey::new(1, IVec3::ZERO));
    for x in 0..SECTION_SIZE {
        for y in 0..SECTION_SIZE {
            for z in 0..SECTION_SIZE {
                if x < 16 || y == 0 {
                    section.set_sample(IVec3::new(x, y, z), Block::Stone);
                }
            }
        }
    }
    // A one-sample-high recess in the wall, opened by the sky-lit column
    // at x = 16.
    for x in 0..16 {
        section.set_sample(IVec3::new(x, 8, 16), Block::Air);
    }

    let light = section.light();

    assert_eq!(light.sky(IVec3::new(16, 8, 16)), 15);
    assert_eq!(light.sky(IVec3::new(15, 8, 16)), 14);
    assert_eq!(light.sky(IVec3::new(14, 8, 16)), 13);
    assert_eq!(light.sky(IVec3::new(13, 8, 16)), 12);
    assert!(
        light.sky(IVec3::new(0, 8, 16)) < light.sky(IVec3::new(15, 8, 16)),
        "light falls off away from the opening"
    );
}

#[test]
fn an_enclosed_cave_stays_dark() {
    let mut section = Section::new(SectionKey::new(1, IVec3::ZERO));
    for x in 0..SECTION_SIZE {
        for y in 0..SECTION_SIZE {
            for z in 0..SECTION_SIZE {
                section.set_sample(IVec3::new(x, y, z), Block::Stone);
            }
        }
    }
    for x in 10..=14 {
        for y in 10..=14 {
            for z in 10..=14 {
                section.set_sample(IVec3::new(x, y, z), Block::Air);
            }
        }
    }

    let light = section.light();

    for x in 10..=14 {
        for y in 10..=14 {
            for z in 10..=14 {
                assert_eq!(light.sky(IVec3::new(x, y, z)), 0, "{x},{y},{z}");
            }
        }
    }
    assert_eq!(light.block(IVec3::new(12, 12, 12)), 0);
}

#[test]
fn water_carries_sky_light_to_the_seabed() {
    let mut section = Section::new(SectionKey::new(1, IVec3::ZERO));
    for x in 0..SECTION_SIZE {
        for z in 0..SECTION_SIZE {
            section.set_sample(IVec3::new(x, 0, z), Block::Stone);
            for y in 1..=5 {
                section.set_sample(IVec3::new(x, y, z), Block::Water);
            }
        }
    }

    let light = section.light();
    for y in 1..=5 {
        assert_eq!(light.sky(IVec3::new(8, y, 8)), 15, "water y {y}");
    }
    assert_eq!(light.sky(IVec3::new(8, 0, 8)), 0, "the seabed is solid");
    assert_eq!(light.sky(IVec3::new(8, 6, 8)), 15, "air above the surface");
}

#[test]
fn water_under_a_roof_keeps_full_light_from_the_opening() {
    let mut section = Section::new(SectionKey::new(1, IVec3::ZERO));
    for x in 0..SECTION_SIZE {
        for y in 0..SECTION_SIZE {
            for z in 0..SECTION_SIZE {
                if x < 16 || y == 0 {
                    section.set_sample(IVec3::new(x, y, z), Block::Stone);
                }
            }
        }
    }
    // A covered water channel fed by the open column at x = 16.
    for x in 0..16 {
        section.set_sample(IVec3::new(x, 8, 16), Block::Water);
    }

    let light = section.light();
    for x in 0..16 {
        assert_eq!(light.sky(IVec3::new(x, 8, 16)), 15, "water x {x}");
    }
}

#[test]
fn open_flat_terrain_is_lit_above_the_solid_top() {
    let mut section = Section::new(SectionKey::new(1, IVec3::ZERO));
    for x in 0..SECTION_SIZE {
        for z in 0..SECTION_SIZE {
            section.set_sample(IVec3::new(x, 0, z), Block::Stone);
        }
    }

    let light = section.light();
    for x in [0, 16, SECTION_SIZE - 1] {
        for z in [0, 16, SECTION_SIZE - 1] {
            assert_eq!(light.sky(IVec3::new(x, 0, z)), 0, "{x},{z}");
            assert_eq!(light.sky(IVec3::new(x, 1, z)), 15, "{x},{z}");
            assert_eq!(light.sky(IVec3::new(x, SECTION_SIZE - 1, z)), 15);
        }
    }
}

#[test]
fn coarse_light_stays_in_range_and_fills_the_halo() {
    let mut section = Section::new(SectionKey::new(1, IVec3::ZERO));
    for x in 0..SECTION_SIZE {
        for z in 0..SECTION_SIZE {
            section.set_sample(IVec3::new(x, 0, z), Block::Stone);
        }
    }

    let light = section.light();
    for x in -1..=SECTION_SIZE {
        for y in -1..=SECTION_SIZE {
            for z in -1..=SECTION_SIZE {
                assert!(light.sky(IVec3::new(x, y, z)) <= 15, "{x},{y},{z}");
            }
        }
    }
    // The halo reuses the nearest edge sample, and the sky above the edge
    // top stays open.
    assert_eq!(light.sky(IVec3::new(-1, 1, -1)), 15);
    assert_eq!(light.sky(IVec3::new(SECTION_SIZE, 1, SECTION_SIZE)), 15);
    assert_eq!(light.sky(IVec3::new(-1, SECTION_SIZE, 0)), 15);
    assert_eq!(light.sky(IVec3::new(SECTION_SIZE, 0, 0)), 0);
    assert_eq!(light.block(IVec3::new(-1, 1, -1)), 0);

    let cells = usize::try_from(34 * 34 * 34).expect("halo field fits");
    assert_eq!(light.resident_bytes(), cells.div_ceil(2));
}

#[test]
fn coarse_light_does_not_change_section_geometry() {
    let generator = Generator::new(42);
    let key = SectionKey::new(1, IVec3::ZERO);
    let section = generate_section(&generator, key, &SectionEdits::new());
    let light = section.light();
    let mesh = section.mesh(Some(&light));

    // Baseline from the per-column light path at 86f99d9. Light changes
    // vertex values, not which faces exist or how they greedily merge.
    assert_eq!(mesh.quads.len(), 2234);
}

#[test]
fn bounds_use_the_lowest_and_highest_visible_columns() {
    let mut section = Section::new(SectionKey::new(1, IVec3::ZERO));
    assert!(section.bounds().is_none());
    section.set_sample(IVec3::new(0, 0, 0), Block::Stone);
    section.set_sample(IVec3::new(1, 3, 1), Block::Water);
    let bounds = section.bounds().expect("visible columns");
    assert_eq!(bounds.min_top, 2.0);
    assert_eq!(bounds.max_top, 8.0);
}

#[test]
fn a_generated_section_meshes_at_its_spacing() {
    let generator = Generator::new(42);
    let key = SectionKey::new(1, IVec3::new(0, 0, 0));
    let section = generate_section(&generator, key, &SectionEdits::new());
    let light = section.light();
    let mesh = section.mesh(Some(&light));

    assert_ne!(mesh.quads.len(), 0);
    assert_eq!(mesh.quads.len() * 4, mesh.vertices.len());
    assert_eq!(mesh.quads.len() * 6, mesh.indices.len());
}

/// A surface-level section at `level` covers its whole footprint. The quad
/// records stay in sample cells and the renderer applies the section's
/// `2^level` scale, so the reconstructed corners must span the world extent.
fn assert_mesh_uses_section_scale(key: SectionKey, mesh: &MeshData) {
    let scale = key.scale();
    let extent = key.extent();
    let size = u32::try_from(SECTION_SIZE).expect("section size fits");
    assert!(!mesh.quads.is_empty(), "{key:?} produced no quads");
    assert_eq!(mesh.quads.len() * 4, mesh.vertices.len());
    assert_eq!(mesh.quads.len() * 6, mesh.indices.len());

    let mut max_x = 0;
    let mut max_z = 0;
    for quad in &mesh.quads {
        assert!((1..=size).contains(&quad.width()), "{key:?} width");
        assert!((1..=size).contains(&quad.height()), "{key:?} height");
        for corner in quad.corners() {
            for value in [corner.x, corner.y, corner.z] {
                assert!(
                    (0..=SECTION_SIZE).contains(&value),
                    "{key:?} corner {value} is outside the sample grid"
                );
            }
            max_x = max_x.max(corner.x);
            max_z = max_z.max(corner.z);
        }
    }
    assert_eq!(max_x * scale, extent, "{key:?} X corners stop early");
    assert_eq!(max_z * scale, extent, "{key:?} Z corners stop early");
}

#[test]
fn level3_and_level4_surface_sections_generate_and_mesh() {
    let generator = Generator::new(42);
    for level in [3, 4] {
        let key = SectionKey::new(level, IVec3::new(0, 0, 0));
        let section = generate_section(&generator, key, &SectionEdits::new());
        assert!(!section.is_empty(), "{key:?} generated empty");
        let light = section.light();
        let mesh = section.mesh(Some(&light));
        assert_mesh_uses_section_scale(key, &mesh);
    }
}

/// The per-sample generator from before the bulk fill, kept as a reference.
fn reference_generate_section(
    generator: &Generator,
    key: SectionKey,
    edits: &SectionEdits,
) -> Section {
    let scale = key.scale();
    let origin = key.origin();
    let probes = probe_offsets(scale);
    let sea_sample = (SEA_LEVEL - origin.y).div_euclid(scale);
    let mut section = Section::new(key);
    for sx in 0..SECTION_SIZE {
        for sz in 0..SECTION_SIZE {
            let (top_height, rep_x, rep_z) =
                footprint_top(generator, origin, scale, sx, sz, &probes);
            let biome = generator.biome(rep_x, rep_z);
            let top_sample = (top_height - origin.y).div_euclid(scale);
            for sy in 0..SECTION_SIZE {
                let local = IVec3::new(sx, sy, sz);
                let base = origin + local * scale;
                let block = if sy <= top_sample {
                    surface_block(biome, top_sample - sy)
                } else if sy <= sea_sample {
                    Block::Water
                } else {
                    Block::Air
                };
                let block = if edits.is_empty() || !overlaps_edits(edits, base, scale) {
                    block
                } else {
                    edited_sample(generator, edits, base, scale)
                };
                if block != Block::Air {
                    section.set_sample(local, block);
                }
            }
        }
    }
    section
}

fn assert_same_samples(expected: &Section, actual: &Section) {
    for sy in 0..SECTION_SIZE {
        for sx in 0..SECTION_SIZE {
            for sz in 0..SECTION_SIZE {
                let local = IVec3::new(sx, sy, sz);
                assert_eq!(
                    actual.sample(local),
                    expected.sample(local),
                    "key {:?} sample {local}",
                    actual.key()
                );
            }
        }
    }
}

#[test]
fn bulk_generation_matches_the_per_sample_path() {
    let generator = Generator::new(42);
    let keys = [
        SectionKey::new(1, IVec3::new(0, 0, 0)),
        SectionKey::new(1, IVec3::new(0, 1, 0)),
        SectionKey::new(2, IVec3::new(0, 0, 0)),
        SectionKey::new(3, IVec3::new(0, 0, 0)),
    ];

    for key in keys {
        let expected = reference_generate_section(&generator, key, &SectionEdits::new());
        let actual = generate_section(&generator, key, &SectionEdits::new());
        assert_same_samples(&expected, &actual);
    }
}

#[test]
fn bulk_generation_matches_the_per_sample_path_on_edited_columns() {
    let generator = Generator::new(42);
    let keys = [
        SectionKey::new(1, IVec3::new(0, 0, 0)),
        SectionKey::new(1, IVec3::new(0, 1, 0)),
        SectionKey::new(2, IVec3::new(0, 0, 0)),
        SectionKey::new(3, IVec3::new(0, 0, 0)),
    ];

    for key in keys {
        let world = key.origin() + IVec3::splat(12);
        let (chunk, _) = ChunkCoord::from_world(world);
        let mut edited = Chunk::default();
        edited.set_block(IVec3::new(3, 4, 3), Block::Stone);
        edited.set_block(IVec3::new(4, 5, 4), Block::Water);
        // A second edited chunk further up exercises a column whose edit
        // only covers part of the section's Y range.
        let upper = world + IVec3::new(0, CHUNK_SIZE, 0);
        let (upper_chunk, _) = ChunkCoord::from_world(upper);
        let mut upper_edit = Chunk::default();
        upper_edit.set_block(IVec3::new(8, 9, 8), Block::Dirt);
        let mut edits = SectionEdits::new();
        edits.insert(chunk, edited);
        edits.insert(upper_chunk, upper_edit);

        let expected = reference_generate_section(&generator, key, &edits);
        let actual = generate_section(&generator, key, &edits);
        assert_same_samples(&expected, &actual);
    }
}

#[allow(clippy::cast_precision_loss)]
fn unbounded_mesh(section: &Section, light: Option<&LightField>, water: bool) -> MeshData {
    let snapshot = section.snapshot();
    let scale = section.key().scale() as f32;
    let range = 0..=SECTION_SIZE - 1;
    if water {
        build_section_water_mesh(
            &snapshot,
            &BorderBlocks::default(),
            light,
            scale,
            range.clone(),
        )
    } else {
        build_section_mesh(&snapshot, &BorderBlocks::default(), light, scale, range)
    }
}

fn assert_bounded_matches_unbounded(section: &Section, light: Option<&LightField>) {
    assert_eq!(
        section.mesh(light),
        unbounded_mesh(section, light, false),
        "opaque {:?}",
        section.key()
    );
    assert_eq!(
        section.water_mesh(light),
        unbounded_mesh(section, light, true),
        "water {:?}",
        section.key()
    );
}

#[test]
fn bounds_limited_meshing_matches_the_unbounded_mesh_on_generated_sections() {
    let generator = Generator::new(42);
    let keys = [
        SectionKey::new(1, IVec3::new(0, 0, 0)),
        SectionKey::new(1, IVec3::new(1, 0, 1)),
        SectionKey::new(2, IVec3::new(0, 0, 0)),
        SectionKey::new(3, IVec3::new(0, 0, 0)),
    ];
    for key in keys {
        let section = generate_section(&generator, key, &SectionEdits::new());
        let light = section.light();
        assert_bounded_matches_unbounded(&section, Some(&light));
    }
}

fn slab_section(key: SectionKey, top: bool) -> Section {
    let mut section = Section::new(key);
    for x in 0..SECTION_SIZE {
        for z in 0..SECTION_SIZE {
            if top {
                for y in (SECTION_SIZE - 6)..SECTION_SIZE {
                    section.set_sample(IVec3::new(x, y, z), Block::Stone);
                }
                for y in (SECTION_SIZE - 3)..SECTION_SIZE {
                    section.set_sample(IVec3::new(x, y, z), Block::Water);
                }
            } else {
                for y in 0..=2 {
                    section.set_sample(IVec3::new(x, y, z), Block::Stone);
                }
                for y in 3..=5 {
                    section.set_sample(IVec3::new(x, y, z), Block::Water);
                }
            }
            section.set_sample(IVec3::new(x, 12, z), Block::Sand);
        }
    }
    section
}

#[test]
fn bounds_limited_meshing_matches_the_unbounded_mesh_on_top_and_bottom_slabs() {
    for key in [
        SectionKey::new(1, IVec3::ZERO),
        SectionKey::new(2, IVec3::new(-1, 0, 2)),
    ] {
        for top in [false, true] {
            let section = slab_section(key, top);
            let light = section.light();
            assert_bounded_matches_unbounded(&section, Some(&light));
        }
    }
}

#[test]
fn overlapping_edits_are_detected_without_touching_clean_sections() {
    let generator = Generator::new(42);
    let key = SectionKey::new(1, IVec3::new(0, 0, 0));
    let mut edits = SectionEdits::new();
    edits.insert(ChunkCoord::new(0, 0, 0), Chunk::default());
    let section = generate_section(&generator, key, &edits);

    let mut clean = SectionEdits::new();
    clean.insert(ChunkCoord::new(50, 50, 50), Chunk::default());
    let other = generate_section(&generator, key, &clean);

    assert_eq!(section.sample(IVec3::new(0, 0, 0)), Block::Air);
    assert_eq!(
        section.sample(IVec3::new(16, 16, 16)),
        other.sample(IVec3::new(16, 16, 16))
    );
}
