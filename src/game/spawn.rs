//! Safe respawn search over the current world.

use glam::{IVec3, Vec3};

use super::environment::EnvironmentProbe;

/// Lowest feet height searched.
const MIN_Y: i32 = 1;
/// Highest feet height searched.
const MAX_Y: i32 = 255;
/// Horizontal ring radius searched before falling back.
const SEARCH_RADIUS: i32 = 4;

/// Finds the nearest safe respawn above a solid, dry floor.
///
/// A safe cell has two non-solid, non-water cells above a solid base. Columns
/// are searched from the origin outward; within a column the nearest safe
/// height to `origin.y` wins. If no cell qualifies, the original point is
/// returned unchanged.
#[allow(clippy::cast_precision_loss)]
pub(crate) fn find_safe_spawn(probe: &dyn EnvironmentProbe, origin: Vec3) -> Vec3 {
    let base = origin.floor().as_ivec3();

    for radius in 0..=SEARCH_RADIUS {
        for dx in -radius..=radius {
            for dz in -radius..=radius {
                if radius > 0 && dx.abs() != radius && dz.abs() != radius {
                    continue;
                }
                let x = base.x + dx;
                let z = base.z + dz;
                let mut best: Option<(i32, i32)> = None;
                for y in MIN_Y..=MAX_Y {
                    let below = IVec3::new(x, y - 1, z);
                    let feet = IVec3::new(x, y, z);
                    let head = IVec3::new(x, y + 1, z);
                    if !probe.is_solid(below)
                        || probe.is_solid(feet)
                        || probe.is_solid(head)
                        || probe.is_water(feet)
                        || probe.is_water(head)
                        || probe.is_lava(feet)
                        || probe.is_lava(head)
                    {
                        continue;
                    }
                    let distance = (y - base.y).abs();
                    if best.is_none_or(|(best_distance, _)| distance < best_distance) {
                        best = Some((distance, y));
                    }
                }
                if let Some((_, y)) = best {
                    return Vec3::new(x as f32 + 0.5, y as f32, z as f32 + 0.5);
                }
            }
        }
    }

    origin
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[derive(Default)]
    struct Probe {
        solid: HashSet<IVec3>,
        water: HashSet<IVec3>,
    }

    impl Probe {
        fn floor(&mut self, y: i32) {
            for x in -2..=2 {
                for z in -2..=2 {
                    self.solid.insert(IVec3::new(x, y, z));
                }
            }
        }
    }

    impl EnvironmentProbe for Probe {
        fn is_solid(&self, cell: IVec3) -> bool {
            self.solid.contains(&cell)
        }

        fn is_water(&self, cell: IVec3) -> bool {
            self.water.contains(&cell)
        }
    }

    #[test]
    fn finds_the_nearest_safe_height_in_the_column() {
        let mut probe = Probe::default();
        probe.floor(0);
        probe.floor(8);

        let spawn = find_safe_spawn(&probe, Vec3::new(0.5, 9.5, 0.5));

        assert_eq!(spawn, Vec3::new(0.5, 9.0, 0.5));
    }

    #[test]
    fn avoids_water_and_flooded_columns() {
        let mut probe = Probe::default();
        probe.floor(0);
        probe.water.insert(IVec3::new(0, 1, 0));
        probe.water.insert(IVec3::new(0, 2, 0));

        let spawn = find_safe_spawn(&probe, Vec3::new(0.5, 3.0, 0.5));

        assert_eq!(spawn.y, 1.0);
        assert_ne!(spawn.x, 0.5, "the flooded origin column must be skipped");
        assert!(!probe.is_water(spawn.floor().as_ivec3()));
    }

    #[test]
    fn falls_back_to_the_origin_without_a_floor() {
        let probe = Probe::default();
        let origin = Vec3::new(0.5, 4.0, 0.5);

        assert_eq!(find_safe_spawn(&probe, origin), origin);
    }
}
