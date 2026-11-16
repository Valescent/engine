//! Chunked voxel storage and dirty-chunk tracking.
//!
//! Chunks are `32 × 32 × 32` cubes. A world coordinate is split into a chunk
//! coordinate and a local coordinate with Euclidean division, so negative
//! worlds map to negative chunks without truncation.

use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap, HashSet};

use glam::{IVec3, Vec3};

/// Edge length of a chunk on every axis.
pub(crate) const CHUNK_SIZE: i32 = 32;

/// Chunks touched by an edit, in deterministic order.
pub(crate) type DirtyChunks = BTreeSet<ChunkCoord>;

/// Coordinate of one chunk in chunk space.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct ChunkCoord(IVec3);

impl ChunkCoord {
    /// Splits a world coordinate into its chunk and local coordinate.
    pub(crate) fn from_world(world: IVec3) -> (Self, IVec3) {
        let size = IVec3::splat(CHUNK_SIZE);
        (Self(world.div_euclid(size)), world.rem_euclid(size))
    }

    /// Minimum-corner world coordinate of the chunk.
    pub(crate) fn world_origin(self) -> IVec3 {
        self.0 * CHUNK_SIZE
    }

    /// Minimum-corner world position in metres.
    pub(crate) fn origin(self) -> Vec3 {
        self.world_origin().as_vec3()
    }

    fn offset(self, by: IVec3) -> Self {
        Self(self.0 + by)
    }
}

impl Ord for ChunkCoord {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.0.x, self.0.y, self.0.z).cmp(&(other.0.x, other.0.y, other.0.z))
    }
}

impl PartialOrd for ChunkCoord {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Voxels owned by one chunk, stored in local coordinates.
#[derive(Debug, Default)]
pub(crate) struct Chunk {
    pub(crate) voxels: HashSet<IVec3>,
}

/// Chunked voxel world.
///
/// Missing chunks are air, and every stored chunk is non-empty.
#[derive(Debug, Default)]
pub(crate) struct World {
    chunks: HashMap<ChunkCoord, Chunk>,
}

impl World {
    /// Marks a world coordinate solid and returns the chunks whose meshes changed.
    pub(crate) fn insert(&mut self, world: IVec3) -> DirtyChunks {
        let (chunk, local) = ChunkCoord::from_world(world);
        if !self.chunks.entry(chunk).or_default().voxels.insert(local) {
            return DirtyChunks::new();
        }
        touched_chunks(chunk, local)
    }

    /// Clears a world coordinate and returns the chunks whose meshes changed.
    pub(crate) fn remove(&mut self, world: IVec3) -> DirtyChunks {
        let (chunk, local) = ChunkCoord::from_world(world);
        let Some(chunk_data) = self.chunks.get_mut(&chunk) else {
            return DirtyChunks::new();
        };
        if !chunk_data.voxels.remove(&local) {
            return DirtyChunks::new();
        }

        if chunk_data.voxels.is_empty() {
            self.chunks.remove(&chunk);
        }
        touched_chunks(chunk, local)
    }

    /// Toggles a world coordinate and returns the chunks whose meshes changed.
    pub(crate) fn toggle(&mut self, world: IVec3) -> DirtyChunks {
        if self.is_solid(world) {
            self.remove(world)
        } else {
            self.insert(world)
        }
    }

    /// Returns whether a world coordinate holds a voxel.
    pub(crate) fn is_solid(&self, world: IVec3) -> bool {
        let (chunk, local) = ChunkCoord::from_world(world);
        self.chunks
            .get(&chunk)
            .is_some_and(|chunk_data| chunk_data.voxels.contains(&local))
    }

    /// Returns a stored chunk, or `None` when the chunk is empty.
    pub(crate) fn chunk(&self, chunk: ChunkCoord) -> Option<&Chunk> {
        self.chunks.get(&chunk)
    }

    /// Iterates over every non-empty chunk in unspecified order.
    pub(crate) fn iter_chunks(&self) -> impl Iterator<Item = (ChunkCoord, &Chunk)> {
        self.chunks
            .iter()
            .map(|(coordinate, chunk)| (*coordinate, chunk))
    }
}

fn touched_chunks(chunk: ChunkCoord, local: IVec3) -> DirtyChunks {
    let mut dirty = DirtyChunks::from([chunk]);
    for axis in [IVec3::X, IVec3::Y, IVec3::Z] {
        match local.dot(axis) {
            0 => {
                dirty.insert(chunk.offset(-axis));
            }
            value if value == CHUNK_SIZE - 1 => {
                dirty.insert(chunk.offset(axis));
            }
            _ => {}
        }
    }
    dirty
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk_at(world: IVec3) -> ChunkCoord {
        ChunkCoord::from_world(world).0
    }

    fn dirty(coordinates: &[IVec3]) -> DirtyChunks {
        coordinates.iter().copied().map(chunk_at).collect()
    }

    #[test]
    fn world_to_chunk_and_local_matches_the_table() {
        let cases = [
            (
                IVec3::new(-33, 0, 0),
                IVec3::new(-2, 0, 0),
                IVec3::new(31, 0, 0),
            ),
            (
                IVec3::new(-32, 0, 0),
                IVec3::new(-1, 0, 0),
                IVec3::new(0, 0, 0),
            ),
            (
                IVec3::new(-1, 0, 0),
                IVec3::new(-1, 0, 0),
                IVec3::new(31, 0, 0),
            ),
            (IVec3::ZERO, IVec3::ZERO, IVec3::ZERO),
            (IVec3::new(31, 0, 0), IVec3::ZERO, IVec3::new(31, 0, 0)),
            (IVec3::new(32, 0, 0), IVec3::new(1, 0, 0), IVec3::ZERO),
        ];

        for (world, chunk, local) in cases {
            let (actual_chunk, actual_local) = ChunkCoord::from_world(world);
            assert_eq!(actual_chunk.0, chunk);
            assert_eq!(actual_local, local);
        }
    }

    #[test]
    fn every_world_coordinate_round_trips() {
        for world in [
            IVec3::MIN,
            IVec3::MAX,
            IVec3::new(i32::MIN, 0, i32::MAX),
            IVec3::new(-31, -1, 31),
            IVec3::new(65_536, -65_536, 0),
        ] {
            let (chunk, local) = ChunkCoord::from_world(world);
            assert_eq!(chunk.world_origin() + local, world);
            assert!(local.cmpge(IVec3::ZERO).all() && local.cmplt(IVec3::splat(CHUNK_SIZE)).all());
        }
    }

    #[test]
    fn chunks_order_lexicographically() {
        let mut chunks = [
            chunk_at(IVec3::new(0, 0, 1)),
            chunk_at(IVec3::new(1, 0, 0)),
            chunk_at(IVec3::new(0, 1, 0)),
            chunk_at(IVec3::new(0, 0, 0)),
        ];
        chunks.sort();

        let expected = [
            chunk_at(IVec3::new(0, 0, 0)),
            chunk_at(IVec3::new(0, 0, 1)),
            chunk_at(IVec3::new(0, 1, 0)),
            chunk_at(IVec3::new(1, 0, 0)),
        ];
        assert_eq!(chunks, expected);
    }

    #[test]
    fn origin_is_the_chunk_minimum_corner() {
        let chunk = chunk_at(IVec3::new(-32, 64, 0));

        assert_eq!(chunk.world_origin(), IVec3::new(-32, 64, 0));
        assert_eq!(chunk.origin(), Vec3::new(-32.0, 64.0, 0.0));
    }

    #[test]
    fn missing_chunks_are_air() {
        let world = World::default();

        assert!(!world.is_solid(IVec3::ZERO));
        assert!(!world.is_solid(IVec3::new(-1, -1, -1)));
    }

    #[test]
    fn insert_is_idempotent() {
        let mut world = World::default();
        let interior = IVec3::new(5, 5, 5);

        assert_eq!(world.insert(interior), dirty(&[interior]));
        assert!(world.insert(interior).is_empty());
        assert!(world.is_solid(interior));
    }

    #[test]
    fn remove_is_idempotent() {
        let mut world = World::default();
        let interior = IVec3::new(5, 5, 5);
        world.insert(interior);

        assert_eq!(world.remove(interior), dirty(&[interior]));
        assert!(world.remove(interior).is_empty());
        assert!(!world.is_solid(interior));
        assert!(world.chunk(chunk_at(interior)).is_none());
    }

    #[test]
    fn interior_edits_dirty_only_the_owner() {
        let mut world = World::default();
        let interior = IVec3::new(5, 5, 5);

        assert_eq!(world.insert(interior), dirty(&[interior]));
        assert_eq!(world.remove(interior), dirty(&[interior]));
    }

    #[test]
    fn border_edits_dirty_touched_neighbours() {
        let mut world = World::default();
        let border = IVec3::new(31, 5, 5);

        assert_eq!(world.insert(border), dirty(&[border, IVec3::new(32, 5, 5)]));
    }

    #[test]
    fn corner_edits_dirty_three_neighbours() {
        let mut world = World::default();
        let corner = IVec3::new(0, 31, 0);

        assert_eq!(
            world.remove(corner),
            DirtyChunks::new(),
            "removing air is not an edit"
        );

        let expected = dirty(&[
            corner,
            IVec3::new(-1, 31, 0),
            IVec3::new(0, 32, 0),
            IVec3::new(0, 31, -1),
        ]);
        assert_eq!(world.insert(corner), expected);
    }

    #[test]
    fn removing_the_last_voxel_deletes_the_chunk_and_dirties_neighbours() {
        let mut world = World::default();
        let border = IVec3::new(32, 0, 0);
        world.insert(border);

        let removed = world.remove(border);

        assert!(world.chunk(chunk_at(border)).is_none());
        assert_eq!(
            removed,
            dirty(&[
                border,
                IVec3::new(31, 0, 0),
                IVec3::new(32, 0, -1),
                IVec3::new(32, -1, 0)
            ])
        );
    }

    #[test]
    fn toggle_switches_between_insert_and_remove() {
        let mut world = World::default();

        let inserted = world.toggle(IVec3::new(31, 1, 31));
        assert!(world.is_solid(IVec3::new(31, 1, 31)));
        assert_eq!(inserted.len(), 3, "owner plus the +X and +Z neighbours");

        let removed = world.toggle(IVec3::new(31, 1, 31));
        assert!(!world.is_solid(IVec3::new(31, 1, 31)));
        assert_eq!(removed, inserted);
    }

    #[test]
    fn stored_chunks_never_go_empty() {
        let mut world = World::default();
        let a = IVec3::new(0, 0, 0);
        let b = IVec3::new(1, 0, 0);
        world.insert(a);
        world.insert(b);
        assert_eq!(world.iter_chunks().count(), 1);

        world.remove(a);
        assert_eq!(world.iter_chunks().count(), 1);
        world.remove(b);
        assert_eq!(world.iter_chunks().count(), 0);
    }

    #[test]
    fn chunk_lookup_matches_storage() {
        let mut world = World::default();
        world.insert(IVec3::new(32, 0, 0));

        let stored = world.chunk(chunk_at(IVec3::new(32, 0, 0))).unwrap();
        assert!(stored.voxels.contains(&IVec3::ZERO));
        assert!(world.chunk(chunk_at(IVec3::ZERO)).is_none());
    }
}
