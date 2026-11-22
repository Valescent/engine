//! Climate tests, grouped by the submodule they exercise.
//!
//! The fixtures below are shared: `params_tests` covers the latitude and altitude
//! terms, `table_tests` covers validation and the raw nearest-two rule, and
//! `field_tests` covers the solved tile, the rain shadow, the dither, and the pinned
//! absolute output.

use crate::curve::Curve;
use crate::description::{MACRO_CELL, SEA};
use crate::error::{ErrorKind, TerrainError};
use crate::hash::domain;
use crate::noise::{fbm_2d, unit};

use super::field::{
    HUMIDITY_NOISE_FREQ, HUMIDITY_NOISE_OCTAVES, MACRO_CELL_F, NOISE_GAIN, NOISE_LACUNARITY,
    SEA_HEIGHT, clamp_unit, sample_position, stream_seed,
};
use super::*;

const TILE_BLOCKS: i32 = 128;

/// A fixture with a short half span, so one tile covers most of the latitude
/// range, and straight curves, so closed forms are exact.
fn test_params() -> ClimateParams {
    ClimateParams {
        equator: 0,
        half_span: 64,
        lapse: 1.0 / 320.0,
        blend_width: blend_ramp_blocks(24),
        latitude: Curve::linear(&[(0.0, 0.92), (1.0, 0.3)]).unwrap(),
        shadow: Curve::linear(&[(0.0, 0.0), (1.0, 1.0)]).unwrap(),
    }
}

/// Shape-only parameters with a world-sized half span. Every cell of one tile stays
/// in the warm half, so a fixture with land above sea has clamp headroom.
fn wide_params() -> ClimateParams {
    ClimateParams {
        half_span: 1_024,
        ..test_params()
    }
}

fn table_2x2() -> BiomeTable {
    BiomeTable::new(vec![
        Biome {
            id: 1,
            temp_c: 0.2,
            humid_c: 0.2,
            weight: 1.0,
            priority: 0,
        },
        Biome {
            id: 2,
            temp_c: 0.2,
            humid_c: 0.8,
            weight: 1.0,
            priority: 0,
        },
        Biome {
            id: 3,
            temp_c: 0.8,
            humid_c: 0.2,
            weight: 1.0,
            priority: 0,
        },
        Biome {
            id: 4,
            temp_c: 0.8,
            humid_c: 0.8,
            weight: 1.0,
            priority: 0,
        },
    ])
    .unwrap()
}

/// Builds a padded grid from a global height function, mapping tile-local cells to
/// global cells through the origin.
fn cells(origin_x: i32, origin_z: i32, height: impl Fn(i32, i32) -> f32) -> MacroCells {
    let mut values = vec![0.0_f32; PADDED_CELLS];
    for cell_z in -1..=(SIDE - 2) {
        for cell_x in -(SHADOW_RANGE_CELLS + 1)..=(SIDE - 2) {
            values[padded_index(cell_x, cell_z)] = height(origin_x + cell_x, origin_z + cell_z);
        }
    }
    MacroCells::new(SIDE + SHADOW_RANGE_CELLS, SIDE, values.into_boxed_slice()).unwrap()
}

/// The wide latitude span of [`wide_params`] with the non-saturating v1
/// shadow curve, so the order test exercises a partial shadow.
fn own_shadow_params() -> ClimateParams {
    ClimateParams {
        shadow: Curve::new(&[(0.0, 0.0), (0.35, 0.18), (0.75, 0.60), (1.0, 1.0)])
            .expect("the v1 shadow knots are valid"),
        ..wide_params()
    }
}

mod params_tests {
    use super::*;

    #[test]
    fn temperature_falls_with_latitude() {
        let params = test_params();

        // The latitude term is exact: it falls from the equator to the half span and
        // is symmetric about the equator.
        let mut previous = f32::INFINITY;
        for block_z in 0..=params.half_span {
            let value = latitude(&params, block_z);
            assert!(value <= previous, "not monotone at {block_z}");
            assert_eq!(
                value.to_bits(),
                latitude(&params, -block_z).to_bits(),
                "not symmetric at {block_z}"
            );
            previous = value;
        }
        assert!(latitude(&params, 0) > latitude(&params, params.half_span));

        // The whole field follows it across one tile. The noise is bounded and its
        // slope is small next to the latitude slope at this span.
        let climate = Climate::new();
        let tile = climate.solve(
            &cells(0, 0, |_, _| SEA_HEIGHT + 2.0),
            &table_2x2(),
            &params,
            7,
        );
        let mut previous = f32::INFINITY;
        for cell_z in 0..=(params.half_span / MACRO_CELL) {
            let value = tile.temp(0, cell_z);
            assert!(value <= previous, "temperature rose at cell {cell_z}");
            previous = value;
        }
        assert!(tile.temp(0, 0) > tile.temp(0, params.half_span / MACRO_CELL));
        assert_eq!(tile.clamp_fires(), 0);
    }

    #[test]
    fn lapse_only_above_sea() {
        let params = wide_params();
        let climate = Climate::new();
        let table = table_2x2();
        let (cell_x, cell_z) = (4, 5);

        assert_eq!(altitude(&params, SEA_HEIGHT + 8.0), 8.0 * params.lapse);
        assert_eq!(altitude(&params, SEA_HEIGHT), 0.0);
        assert_eq!(altitude(&params, SEA_HEIGHT - 64.0), 0.0);

        // Land fixtures: no ocean upwind, so no coast term and no saturation.
        let seven_up = climate.solve(&cells(0, 0, |_, _| SEA_HEIGHT + 7.0), &table, &params, 3);
        let eight_up = climate.solve(&cells(0, 0, |_, _| SEA_HEIGHT + 8.0), &table, &params, 3);
        let seventeen_up =
            climate.solve(&cells(0, 0, |_, _| SEA_HEIGHT + 17.0), &table, &params, 3);
        let drop_one = seven_up.temp(cell_x, cell_z) - eight_up.temp(cell_x, cell_z);
        assert!((drop_one - params.lapse).abs() < 1e-6);
        let drop_ten = seven_up.temp(cell_x, cell_z) - seventeen_up.temp(cell_x, cell_z);
        assert!((drop_ten - 10.0 * params.lapse).abs() < 1e-6);
        assert_eq!(seven_up.clamp_fires(), 0);
        assert_eq!(seventeen_up.clamp_fires(), 0);

        // At and below sea the altitude term is zero, so the temperature is identical.
        // Both fixtures are ocean by definition, which is why their clamp counters are
        // not asserted here.
        let at_sea = climate.solve(&cells(0, 0, |_, _| SEA_HEIGHT), &table, &params, 3);
        let below = climate.solve(&cells(0, 0, |_, _| SEA_HEIGHT - 40.0), &table, &params, 3);
        assert_eq!(
            below.temp(cell_x, cell_z).to_bits(),
            at_sea.temp(cell_x, cell_z).to_bits(),
            "no lapse below sea"
        );
    }
}

mod table_tests {
    use super::*;

    #[test]
    fn biome_pick_is_nearest_in_climate_space() {
        let table = table_2x2();
        for (temperature, humidity, expected) in [
            (0.2_f32, 0.2_f32, 1_u8),
            (0.2, 0.8, 2),
            (0.8, 0.2, 3),
            (0.8, 0.8, 4),
        ] {
            let (best, second, _) = table.nearest_two(temperature, humidity, 0.05);
            assert_eq!(best, expected, "at ({temperature}, {humidity})");
            assert_ne!(second, best);
        }

        // Exactly equidistant centres tie, and the lower id wins.
        let (best, second, _) = table.nearest_two(0.5, 0.2, 0.05);
        assert_eq!(best, 1);
        assert_eq!(second, 3);
    }

    #[test]
    fn bad_tables_are_rejected() {
        let good = Biome {
            id: 1,
            temp_c: 0.5,
            humid_c: 0.5,
            weight: 1.0,
            priority: 0,
        };
        let rejected = |biomes: Vec<Biome>| {
            matches!(
                BiomeTable::new(biomes),
                Err(TerrainError {
                    kind: ErrorKind::DescriptionInvalid { key: "BIOMES", .. },
                    ..
                })
            )
        };

        assert!(rejected(vec![good, good]), "duplicate id and centre");
        assert!(
            rejected(vec![good, Biome { id: 2, ..good },]),
            "duplicate centre"
        );
        assert!(
            rejected(vec![Biome {
                temp_c: 1.5,
                ..good
            }]),
            "temperature centre out of range"
        );
        assert!(
            rejected(vec![Biome {
                humid_c: -0.1,
                ..good
            }]),
            "humidity centre out of range"
        );
        assert!(
            rejected(vec![Biome {
                weight: 0.0,
                ..good
            }]),
            "zero weight"
        );

        // 256 entries: a 16x16 grid of distinct centres.
        let many: Vec<Biome> = (0..16)
            .flat_map(|temp_index| {
                (0..16).map(move |humid_index| Biome {
                    id: (temp_index * 16 + humid_index) as u8,
                    temp_c: temp_index as f32 / 15.0,
                    humid_c: humid_index as f32 / 15.0,
                    weight: 1.0,
                    priority: 0,
                })
            })
            .collect();
        assert_eq!(many.len(), 256);
        assert!(rejected(many), "256 biomes");

        assert!(rejected(Vec::new()), "empty table");
        assert!(BiomeTable::new(vec![good]).is_ok());
    }
}

mod field_tests {
    use super::*;

    #[test]
    fn climate_ignores_per_block_height() {
        // A per-block field, with a river carved into it and a one-block bump, is built
        // here and never consulted. `MacroCells` is the only height channel and it is
        // cell-resolution, so neither can reach the field.
        let smooth = vec![SEA; (TILE_BLOCKS * TILE_BLOCKS) as usize];
        let mut detailed = smooth.clone();
        for block_z in 0..TILE_BLOCKS {
            detailed[(block_z * TILE_BLOCKS + 60) as usize] -= 6;
        }
        detailed[(64 * TILE_BLOCKS + 64) as usize] += 1;
        assert_ne!(smooth, detailed);

        let params = wide_params();
        let climate = Climate::new();
        let table = table_2x2();
        let tile = climate.solve(&cells(0, 0, |_, _| SEA_HEIGHT + 10.0), &table, &params, 11);
        let repeat = climate.solve(&cells(0, 0, |_, _| SEA_HEIGHT + 10.0), &table, &params, 11);
        for cell_z in MIN_CELL..=MAX_CELL {
            for cell_x in MIN_CELL..=MAX_CELL {
                assert_eq!(
                    tile.temp(cell_x, cell_z).to_bits(),
                    repeat.temp(cell_x, cell_z).to_bits()
                );
                assert_eq!(
                    tile.humidity(cell_x, cell_z).to_bits(),
                    repeat.humidity(cell_x, cell_z).to_bits()
                );
                assert_eq!(tile.margin(cell_x, cell_z), repeat.margin(cell_x, cell_z));
            }
        }

        // A cell-resolution bump does move the field, so the comparison above is not
        // vacuous: the macro grid really is read.
        let bumped = cells(0, 0, |cell_x, cell_z| {
            if cell_x == 0 && cell_z == 0 {
                SEA_HEIGHT + 14.0
            } else {
                SEA_HEIGHT + 10.0
            }
        });
        let bumped_tile = climate.solve(&bumped, &table, &params, 11);
        assert_ne!(tile.temp(0, 0).to_bits(), bumped_tile.temp(0, 0).to_bits());
    }

    #[test]
    fn rain_shadow_is_lee_side_only() {
        // A single ridge at cell x = 24, land everywhere else. Wind blows toward +x, so
        // cells above the ridge are the lee and cells below it are windward.
        let params = wide_params();
        let climate = Climate::new();
        let table = table_2x2();
        let grid = cells(0, 0, |cell_x, _| {
            if cell_x == 24 {
                SEA_HEIGHT + 100.0
            } else {
                SEA_HEIGHT + 2.0
            }
        });
        let tile = climate.solve(&grid, &table, &params, 13);

        for cell_z in MIN_CELL..=MAX_CELL {
            for cell_x in 25..=32 {
                assert_eq!(
                    tile.humidity(cell_x, cell_z),
                    0.0,
                    "lee cell {cell_x} was not dried"
                );
            }
            for cell_x in 0..=23 {
                assert!(
                    tile.humidity(cell_x, cell_z) > 0.0,
                    "windward cell {cell_x} was dried"
                );
            }
        }
    }

    #[test]
    fn shadow_march_is_fixed_and_bounded() {
        // h(x) = SEA - r*x: every upwind cell rises by r per cell, so the closed form is
        // max_k (r - SLOPE_K*MACRO_CELL) * k, which is largest at k = 32 for r = 3.
        let rise = 3.0_f32;
        let rising = cells(0, 0, |cell_x, _| SEA_HEIGHT - rise * cell_x as f32);
        let step = SLOPE_K * MACRO_CELL_F;
        let expected = (rise - step) * SHADOW_RANGE_CELLS as f32;
        for cell_x in MIN_CELL..=MAX_CELL {
            assert_eq!(shadow(&rising, cell_x, 0).depth, expected, "at {cell_x}");
        }

        // A ramp exactly as steep as the slope term casts nothing; half again as steep
        // casts exactly half the range.
        let slack = cells(0, 0, |cell_x, _| SEA_HEIGHT - step * cell_x as f32);
        assert_eq!(shadow(&slack, 0, 0).depth, 0.0);
        let steeper = cells(0, 0, |cell_x, _| SEA_HEIGHT - (step + 0.5) * cell_x as f32);
        assert_eq!(
            shadow(&steeper, 0, 0).depth,
            0.5 * SHADOW_RANGE_CELLS as f32
        );

        // Ocean upwind is reported, land upwind is not.
        let coast = cells(0, 0, |cell_x, _| {
            if cell_x <= -8 {
                SEA_HEIGHT - 4.0
            } else {
                SEA_HEIGHT + 2.0
            }
        });
        assert!(shadow(&coast, 0, 0).ocean_upwind);
        assert!(!shadow(&rising, 0, 0).ocean_upwind);
    }

    #[test]
    fn maritime_coast_adds_humidity() {
        // Two grids differ only in an ocean band upwind of cell x = 0. The shadow is
        // zero in both, so the only difference is the coast term.
        let params = wide_params();
        let climate = Climate::new();
        let table = table_2x2();
        let wet = climate.solve(
            &cells(0, 0, |cell_x, _| {
                if (-8..=-4).contains(&cell_x) {
                    SEA_HEIGHT - 4.0
                } else {
                    SEA_HEIGHT + 2.0
                }
            }),
            &table,
            &params,
            17,
        );
        let dry = climate.solve(&cells(0, 0, |_, _| SEA_HEIGHT + 2.0), &table, &params, 17);

        let mut compared = 0;
        for cell_z in MIN_CELL..=MAX_CELL {
            let coastal = wet.humidity(0, cell_z);
            let inland = dry.humidity(0, cell_z);
            assert!(coastal >= inland, "coast was drier at cell_z {cell_z}");
            if inland < 1.0 - MARITIME {
                compared += 1;
                assert!(
                    (coastal - inland - MARITIME).abs() < 1e-5,
                    "coast added {} at cell_z {cell_z}",
                    coastal - inland
                );
            }
        }
        assert!(
            compared >= 8,
            "only {compared} unsaturated columns compared"
        );
    }

    /// `E2.4`: the humidity order is pinned. With no shadow and no coast the
    /// stored value must be exactly `unit(noise)`, bit for bit.
    #[test]
    fn humidity_is_unit_noise_when_shadow_and_coast_are_zero() {
        let params = wide_params();
        let climate = Climate::new();
        let table = table_2x2();
        let seed = 0x5EED;
        let grid = cells(0, 0, |_, _| SEA_HEIGHT + 2.0);
        let tile = climate.solve(&grid, &table, &params, seed);

        let humid_seed = stream_seed(seed, domain(HUMIDITY_DOMAIN));
        for cell_z in MIN_CELL..=MAX_CELL {
            for cell_x in MIN_CELL..=MAX_CELL {
                let march = shadow(&grid, cell_x, cell_z);
                assert_eq!(march.depth, 0.0, "the fixture must have no shadow");
                assert!(!march.ocean_upwind, "the fixture must have no coast");

                let noise = unit(fbm_2d(
                    humid_seed,
                    sample_position(cell_x),
                    sample_position(cell_z),
                    HUMIDITY_NOISE_FREQ,
                    HUMIDITY_NOISE_OCTAVES,
                    NOISE_LACUNARITY,
                    NOISE_GAIN,
                ));
                assert_eq!(
                    tile.humidity(cell_x, cell_z).to_bits(),
                    noise.to_bits(),
                    "humidity at {cell_x},{cell_z} is not the raw unit noise"
                );
            }
        }
        assert_eq!(tile.clamp_fires(), 0);
    }

    /// `E2.4`: multiplying by the shadow before adding maritime is observable.
    /// The swapped expression must differ where both terms are live, and the
    /// stored value must be the pinned one.
    #[test]
    fn humidity_order_cannot_be_swapped() {
        let params = own_shadow_params();
        let climate = Climate::new();
        let table = table_2x2();
        let seed = 0x0DDE;

        // A ridge 12 cells upwind gives a partial shadow; an ocean cell further
        // upwind supplies the coast term, so both are live at cell (0, 0).
        let grid = cells(0, 0, |cell_x, _| {
            if cell_x == -30 {
                SEA_HEIGHT - 20.0
            } else if cell_x == -12 {
                SEA_HEIGHT + 40.0
            } else {
                SEA_HEIGHT + 2.0
            }
        });
        let march = shadow(&grid, 0, 0);
        assert!(march.depth > 0.0 && march.depth < SHADOW_NORM, "{march:?}");
        assert!(
            march.ocean_upwind,
            "the fixture must have a live coast term"
        );

        let tile = climate.solve(&grid, &table, &params, seed);
        let humid_seed = stream_seed(seed, domain(HUMIDITY_DOMAIN));
        let noise = unit(fbm_2d(
            humid_seed,
            sample_position(0),
            sample_position(0),
            HUMIDITY_NOISE_FREQ,
            HUMIDITY_NOISE_OCTAVES,
            NOISE_LACUNARITY,
            NOISE_GAIN,
        ));
        let drying = params.shadow.sample(march.depth / SHADOW_NORM);
        let mut fires = 0_u64;
        let pinned = clamp_unit(
            noise * (1.0 - SHADOW_STRENGTH * drying) + MARITIME,
            &mut fires,
        );
        let mut swapped_fires = 0_u64;
        let swapped = clamp_unit(
            (noise + MARITIME) * (1.0 - SHADOW_STRENGTH * drying),
            &mut swapped_fires,
        );

        assert_ne!(
            pinned.to_bits(),
            swapped.to_bits(),
            "the two orders must be distinguishable here"
        );
        assert_eq!(
            tile.humidity(0, 0).to_bits(),
            pinned.to_bits(),
            "the implementation did not use multiply-then-add"
        );
        assert_ne!(
            tile.humidity(0, 0).to_bits(),
            swapped.to_bits(),
            "the implementation used the swapped order"
        );
    }

    #[test]
    fn margin_quantises_to_255_deep_inside() {
        // One biome: `E3.5` pins `second = best` and `q = 255` regardless of climate.
        let single = BiomeTable::new(vec![Biome {
            id: 7,
            temp_c: 0.5,
            humid_c: 0.5,
            weight: 1.0,
            priority: 0,
        }])
        .unwrap();
        let params = wide_params();
        let climate = Climate::new();
        let tile = climate.solve(&cells(0, 0, |_, _| SEA_HEIGHT + 2.0), &single, &params, 5);
        let (cell_x, cell_z) = (3, -1);
        assert_eq!(tile.margin(cell_x, cell_z), 255);
        assert_eq!(tile.best(cell_x, cell_z), 7);
        assert_eq!(tile.second(cell_x, cell_z), 7);

        // `w = 1` deep inside, so every coordinate picks `best`. There is no `y`
        // argument: a column has one pick by construction.
        let tag = domain(DITHER_DOMAIN);
        for block_z in -8..=8 {
            for block_x in -8..=8 {
                let first = tile.biome_pick(cell_x, cell_z, block_x * 64, block_z * 64, 5, tag);
                let again = tile.biome_pick(cell_x, cell_z, block_x * 64, block_z * 64, 5, tag);
                assert_eq!(first, 7);
                assert_eq!(again, first);
            }
        }
        assert_eq!(tile.clamp_fires(), 0);
    }

    #[test]
    fn dither_share_matches_the_margin() {
        let params = wide_params();
        let climate = Climate::new();
        let tile = climate.solve(
            &cells(0, 0, |_, _| SEA_HEIGHT + 2.0),
            &table_2x2(),
            &params,
            9,
        );

        // Find a cell that is neither on a seam nor deep inside, so both outcomes are
        // reachable.
        let mut target = None;
        for cell_z in MIN_CELL..=MAX_CELL {
            for cell_x in MIN_CELL..=MAX_CELL {
                let q = tile.margin(cell_x, cell_z);
                if (40..=200).contains(&q) {
                    target = Some((cell_x, cell_z, q));
                    break;
                }
            }
            if target.is_some() {
                break;
            }
        }
        let (cell_x, cell_z, q) = target.expect("a mid-margin cell exists in this fixture");
        let weight = 0.5 + 0.5 * smoothstep01(f32::from(q) / 255.0);
        assert!(weight > 0.5 && weight < 1.0);

        let tag = domain(DITHER_DOMAIN);
        let mut near = 0_u32;
        let mut total = 0_u32;
        for block_z in -32..32 {
            for block_x in -32..32 {
                let pick = tile.biome_pick(cell_x, cell_z, block_x, block_z, 17, tag);
                assert!(pick == tile.best(cell_x, cell_z) || pick == tile.second(cell_x, cell_z));
                if pick == tile.best(cell_x, cell_z) {
                    near += 1;
                }
                total += 1;
            }
        }
        let share = f64::from(near) / f64::from(total);
        assert!(
            (share - f64::from(weight)).abs() <= 0.02,
            "share {share} vs w {weight} at q {q}"
        );
    }

    /// Absolute pin: the fixture's low border cell has one concrete `q`, and the keyed
    /// dither falls on both sides of the pair at fixed block coordinates. A change to a
    /// centre, weight, blend ramp, hash key, or `w` formula moves one of these numbers.
    #[test]
    fn biome_pick_and_margin_are_pinned() {
        let params = wide_params();
        let climate = Climate::new();
        let tile = climate.solve(
            &cells(0, 0, |_, _| SEA_HEIGHT + 2.0),
            &table_2x2(),
            &params,
            0x5EED,
        );
        let (cell_x, cell_z) = (-1, -1);
        assert_eq!(tile.margin(cell_x, cell_z), 71);
        assert_eq!(tile.best(cell_x, cell_z), 4);
        assert_eq!(tile.second(cell_x, cell_z), 3);

        let tag = domain(DITHER_DOMAIN);
        assert_eq!(tile.biome_pick(cell_x, cell_z, 0, 0, 0x5EED, tag), 4);
        assert_eq!(tile.biome_pick(cell_x, cell_z, 6, 0, 0x5EED, tag), 3);
    }

    #[test]
    fn border_cells_match_the_neighbour() {
        let params = wide_params();
        let climate = Climate::new();
        let table = table_2x2();
        let height = |cell_x: i32, cell_z: i32| {
            SEA_HEIGHT + 40.0 + cell_x as f32 * 0.25 - cell_z as f32 * 0.5
        };

        let here = climate.solve_at(0, 0, &cells(0, 0, height), &table, &params, 21);
        let neighbour = climate.solve_at(32, 0, &cells(32, 0, height), &table, &params, 21);

        for cell_z in MIN_CELL..=MAX_CELL {
            for local in [31, 32] {
                let away = local - 32;
                assert!((MIN_CELL..=MAX_CELL).contains(&away));
                assert_eq!(
                    here.temp(local, cell_z).to_bits(),
                    neighbour.temp(away, cell_z).to_bits()
                );
                assert_eq!(
                    here.humidity(local, cell_z).to_bits(),
                    neighbour.humidity(away, cell_z).to_bits()
                );
                assert_eq!(here.best(local, cell_z), neighbour.best(away, cell_z));
                assert_eq!(here.second(local, cell_z), neighbour.second(away, cell_z));
                assert_eq!(here.margin(local, cell_z), neighbour.margin(away, cell_z));
            }
        }
        assert_eq!(here.clamp_fires(), 0);
        assert_eq!(neighbour.clamp_fires(), 0);
    }
}
