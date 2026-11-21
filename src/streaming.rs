//! Player-centred chunk streaming: load rings, ordering, and readiness.

use std::collections::HashSet;

use glam::{IVec2, Vec2, Vec3};

use crate::player::PLAYER_WIDTH;
use crate::world::CHUNK_SIZE;

/// Columns kept loaded beyond the render distance before unloading.
pub(crate) const UNLOAD_MARGIN: i32 = 2;

/// Streaming state for one generated world.
#[derive(Debug, Default)]
pub(crate) struct Streaming {
    render_distance: i32,
    loaded: HashSet<IVec2>,
    ready: HashSet<IVec2>,
}

impl Streaming {
    pub(crate) fn new(render_distance: i32) -> Self {
        Self {
            render_distance,
            ..Self::default()
        }
    }

    /// Chunk column containing a world position.
    #[allow(clippy::cast_possible_truncation)]
    pub(crate) fn column_at(position: Vec3) -> IVec2 {
        let x = (position.x.floor() as i32).div_euclid(CHUNK_SIZE);
        let z = (position.z.floor() as i32).div_euclid(CHUNK_SIZE);
        IVec2::new(x, z)
    }

    pub(crate) fn mark_loaded(&mut self, column: IVec2) {
        self.loaded.insert(column);
    }

    pub(crate) fn mark_ready(&mut self, column: IVec2) {
        self.ready.insert(column);
    }

    pub(crate) fn is_ready(&self, column: IVec2) -> bool {
        self.ready.contains(&column)
    }

    pub(crate) fn is_loaded(&self, column: IVec2) -> bool {
        self.loaded.contains(&column)
    }

    pub(crate) fn render_distance(&self) -> i32 {
        self.render_distance
    }

    pub(crate) fn loaded_count(&self) -> usize {
        self.loaded.len()
    }

    pub(crate) fn ready_count(&self) -> usize {
        self.ready.len()
    }

    /// Whether a player AABB at `position` overlaps only ready columns.
    #[allow(clippy::cast_possible_truncation)]
    pub(crate) fn aabb_ready(&self, position: Vec3) -> bool {
        let half = PLAYER_WIDTH / 2.0;
        let min = IVec2::new(
            (position.x - half).floor() as i32,
            (position.z - half).floor() as i32,
        );
        let max = IVec2::new(
            (position.x + half).floor() as i32,
            (position.z + half).floor() as i32,
        );
        for x in min.x.div_euclid(CHUNK_SIZE)..=max.x.div_euclid(CHUNK_SIZE) {
            for z in min.y.div_euclid(CHUNK_SIZE)..=max.y.div_euclid(CHUNK_SIZE) {
                if !self.is_ready(IVec2::new(x, z)) {
                    return false;
                }
            }
        }
        true
    }

    /// Columns inside the render distance that are not loaded, ordered for loading.
    ///
    /// Forward columns come first, then nearest, then `(x, z)` ascending.
    #[allow(clippy::cast_precision_loss)]
    pub(crate) fn wanted(&self, player: Vec3, forward: Vec2) -> Vec<IVec2> {
        let player_column = Self::column_at(player);
        let distance = self.render_distance;
        let mut candidates = Vec::new();

        for dx in -distance..=distance {
            for dz in -distance..=distance {
                let column = player_column + IVec2::new(dx, dz);
                if self.loaded.contains(&column) {
                    continue;
                }
                let center = Vec2::new(
                    column.x as f32 * CHUNK_SIZE as f32 + CHUNK_SIZE as f32 * 0.5,
                    column.y as f32 * CHUNK_SIZE as f32 + CHUNK_SIZE as f32 * 0.5,
                );
                let offset = (center - player.truncate()).normalize_or_zero();
                let behind = i32::from(offset.dot(forward) < 0.0);
                candidates.push((behind, dx * dx + dz * dz, column.x, column.y, column));
            }
        }

        candidates.sort_unstable_by_key(|(behind, distance, x, z, _)| (*behind, *distance, *x, *z));
        candidates
            .into_iter()
            .map(|(_, _, _, _, column)| column)
            .collect()
    }

    /// Loaded columns beyond the render distance plus the unload margin.
    pub(crate) fn unloadable(&self, player: IVec2) -> Vec<IVec2> {
        let limit = self.render_distance + UNLOAD_MARGIN;
        let mut columns: Vec<IVec2> = self
            .loaded
            .iter()
            .copied()
            .filter(|column| {
                let delta = (*column - player).abs();
                delta.x.max(delta.y) > limit
            })
            .collect();
        columns.sort_unstable_by_key(|column| (column.x, column.y));
        columns
    }

    pub(crate) fn mark_unloaded(&mut self, column: IVec2) {
        self.loaded.remove(&column);
        self.ready.remove(&column);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FORWARD_X: Vec2 = Vec2::new(1.0, 0.0);

    fn player_at(x: f32, z: f32) -> Vec3 {
        Vec3::new(x, 40.0, z)
    }

    #[test]
    fn column_at_maps_negative_positions() {
        assert_eq!(Streaming::column_at(player_at(0.0, 0.0)), IVec2::ZERO);
        assert_eq!(
            Streaming::column_at(player_at(-0.1, 31.9)),
            IVec2::new(-1, 0)
        );
        assert_eq!(
            Streaming::column_at(player_at(32.0, -32.0)),
            IVec2::new(1, -1)
        );
    }

    #[test]
    fn wanted_covers_the_chebyshev_square() {
        let streaming = Streaming::new(3);
        let columns = streaming.wanted(player_at(16.0, 16.0), FORWARD_X);

        assert_eq!(columns.len(), 7 * 7);
        assert_eq!(columns[0], IVec2::ZERO);
        assert!(columns.contains(&IVec2::new(-3, -3)));
        assert!(columns.contains(&IVec2::new(3, 3)));
    }

    #[test]
    fn loaded_columns_are_not_requested() {
        let mut streaming = Streaming::new(2);
        streaming.mark_loaded(IVec2::ZERO);

        let columns = streaming.wanted(player_at(16.0, 16.0), FORWARD_X);

        assert_eq!(columns.len(), 24);
        assert!(!columns.contains(&IVec2::ZERO));
    }

    #[test]
    fn forward_columns_come_first_and_near_before_far() {
        let streaming = Streaming::new(2);
        let columns = streaming.wanted(player_at(16.0, 16.0), FORWARD_X);

        let first_behind = columns
            .iter()
            .position(|column| column.x < 0)
            .expect("columns behind");
        assert!(columns[..first_behind].iter().all(|column| column.x >= 0));
        assert!(columns[first_behind..].iter().all(|column| column.x < 0));

        let distances: Vec<i32> = columns[..first_behind]
            .iter()
            .map(|column| column.x * column.x + column.y * column.y)
            .collect();
        assert!(distances.windows(2).all(|pair| pair[0] <= pair[1]));
    }

    #[test]
    fn unload_keeps_the_margin() {
        let mut streaming = Streaming::new(8);
        streaming.mark_loaded(IVec2::ZERO);
        streaming.mark_loaded(IVec2::new(10, 0));
        streaming.mark_loaded(IVec2::new(11, 0));

        let unloadable = streaming.unloadable(IVec2::ZERO);

        assert_eq!(unloadable, vec![IVec2::new(11, 0)]);
    }

    #[test]
    fn readiness_tracks_the_player_box_columns() {
        let mut streaming = Streaming::new(4);
        assert!(!streaming.aabb_ready(player_at(16.0, 16.0)));

        streaming.mark_ready(IVec2::ZERO);
        assert!(streaming.aabb_ready(player_at(16.0, 16.0)));

        assert!(!streaming.aabb_ready(player_at(31.9, 16.0)));
        streaming.mark_ready(IVec2::new(1, 0));
        assert!(streaming.aabb_ready(player_at(31.9, 16.0)));
    }

    #[test]
    fn columns_only_unload_after_the_margin() {
        let mut streaming = Streaming::new(8);
        streaming.mark_loaded(IVec2::new(10, 0));
        streaming.mark_ready(IVec2::new(10, 0));

        assert_eq!(streaming.unloadable(IVec2::ZERO), Vec::new());

        streaming.mark_unloaded(IVec2::new(10, 0));
        assert!(!streaming.is_ready(IVec2::new(10, 0)));
        assert_eq!(streaming.loaded_count(), 0);
    }
}
