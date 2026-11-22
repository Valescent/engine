//! Dropped item entities: physics, pickup, merging, and despawn.
//!
//! The store is deliberately small (ADR-0020): one item kind, stable ids, a
//! spatial hash for pickup, and explicit save records.

use std::collections::{HashMap, HashSet};

use glam::{IVec3, Vec3};

use super::inventory::Inventory;
use super::item::{ItemRegistry, ItemStack};
use crate::player::{BlockSource, GRAVITY};

/// How long a dropped item survives before it is removed.
pub(crate) const DESPAWN_SECONDS: f32 = 300.0;
/// Half the item cube's edge, used by the landing probe.
const ITEM_HALF: f32 = 0.125;

/// A stable entity identifier.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct EntityId(u64);

impl EntityId {
    pub(crate) const fn get(self) -> u64 {
        self.0
    }
}

/// One dropped item.
#[derive(Clone, Debug)]
pub(crate) struct ItemEntity {
    pub(crate) id: EntityId,
    pub(crate) stack: ItemStack,
    pub(crate) position: Vec3,
    pub(crate) velocity: Vec3,
    pub(crate) age: f32,
}

/// A serialisable entity record. Names are stable; ids are not reused.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct EntityRecord {
    pub(crate) id: u64,
    pub(crate) item: String,
    pub(crate) count: u16,
    pub(crate) durability: u16,
    pub(crate) position: [f32; 3],
    pub(crate) velocity: [f32; 3],
    pub(crate) age: f32,
}

/// The world's dropped items.
#[derive(Clone, Debug, Default)]
pub(crate) struct EntityStore {
    next_id: u64,
    items: Vec<ItemEntity>,
    index: HashMap<EntityId, usize>,
    spatial: HashMap<IVec3, Vec<EntityId>>,
}

impl EntityStore {
    /// Spawns one item at `position` (its centre) with `velocity`.
    pub(crate) fn spawn(&mut self, stack: ItemStack, position: Vec3, velocity: Vec3) -> EntityId {
        let id = EntityId(self.next_id);
        self.next_id += 1;
        let index = self.items.len();
        self.items.push(ItemEntity {
            id,
            stack,
            position,
            velocity,
            age: 0.0,
        });
        self.index.insert(id, index);
        self.insert_spatial(id, position);
        id
    }

    /// Advances physics by one fixed step and removes expired items.
    pub(crate) fn tick(&mut self, world: &dyn BlockSource, delta: f32) {
        if delta <= 0.0 {
            return;
        }
        for entity in &mut self.items {
            entity.age += delta;
            entity.velocity.y -= GRAVITY * delta;
            let motion = entity.velocity * delta;
            move_axis(world, entity, 1, motion.y);
            move_axis(world, entity, 0, motion.x);
            move_axis(world, entity, 2, motion.z);
            entity.velocity.x *= 0.90;
            entity.velocity.z *= 0.90;
        }

        let before = self.items.len();
        self.items.retain(|entity| entity.age < DESPAWN_SECONDS);
        if self.items.len() != before {
            self.reindex();
        }
        self.rebuild_spatial();
    }

    /// Moves every item within `radius` of `position` into the inventory.
    ///
    /// Returns the number of items picked up. A full inventory leaves the
    /// remainder on the ground.
    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    pub(crate) fn pickup(
        &mut self,
        position: Vec3,
        radius: f32,
        inventory: &mut Inventory,
        registry: &ItemRegistry,
    ) -> u16 {
        if radius <= 0.0 || self.items.is_empty() {
            return 0;
        }
        let reach = radius.ceil() as i32;
        let centre = cell_of(position);
        let mut candidates = HashSet::new();
        for dx in -reach..=reach {
            for dy in -reach..=reach {
                for dz in -reach..=reach {
                    let key = centre + IVec3::new(dx, dy, dz);
                    if let Some(ids) = self.spatial.get(&key) {
                        candidates.extend(ids.iter().copied());
                    }
                }
            }
        }

        let radius_squared = radius * radius;
        let mut picked = 0_u32;
        for entity in &mut self.items {
            if !candidates.contains(&entity.id)
                || entity.position.distance_squared(position) > radius_squared
            {
                continue;
            }
            let leftover = inventory.add(entity.stack, registry);
            let remaining = leftover.map_or(0, |stack| stack.count);
            picked += u32::from(entity.stack.count - remaining);
            entity.stack.count = remaining;
        }
        self.items.retain(|entity| entity.stack.count > 0);
        self.reindex();
        self.rebuild_spatial();
        u16::try_from(picked).unwrap_or(u16::MAX)
    }

    /// The number of live entities.
    #[allow(dead_code, reason = "used by tests and the 0.0.10.2 HUD")]
    pub(crate) fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether the store is empty.
    #[allow(dead_code, reason = "used by tests and the 0.0.10.2 HUD")]
    pub(crate) fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Iterates live entities in insertion order.
    pub(crate) fn iter(&self) -> impl Iterator<Item = &ItemEntity> + '_ {
        self.items.iter()
    }

    /// The serialisable records for the player save.
    pub(crate) fn records(&self, registry: &ItemRegistry) -> Vec<EntityRecord> {
        self.items
            .iter()
            .map(|entity| EntityRecord {
                id: entity.id.get(),
                item: registry.name(entity.stack.item).to_owned(),
                count: entity.stack.count,
                durability: entity.stack.durability,
                position: entity.position.to_array(),
                velocity: entity.velocity.to_array(),
                age: entity.age,
            })
            .collect()
    }

    /// Rebuilds the store from save records, skipping unknown items and
    /// duplicate ids.
    pub(crate) fn from_records(records: Vec<EntityRecord>, registry: &ItemRegistry) -> Self {
        let mut store = Self::default();
        let mut highest = 0_u64;
        for record in records {
            let Some(mut stack) = registry.stack(&record.item, record.count) else {
                continue;
            };
            stack.durability = record.durability;
            let id = EntityId(record.id);
            if store.index.contains_key(&id) {
                continue;
            }
            highest = highest.max(record.id);
            let index = store.items.len();
            store.items.push(ItemEntity {
                id,
                stack,
                position: Vec3::from_array(record.position),
                velocity: Vec3::from_array(record.velocity),
                age: record.age,
            });
            store.index.insert(id, index);
        }
        store.next_id = highest.saturating_add(1);
        for (index, entity) in store.items.iter().enumerate() {
            store.index.insert(entity.id, index);
        }
        store.rebuild_spatial();
        store
    }

    fn reindex(&mut self) {
        self.index.clear();
        for (index, entity) in self.items.iter().enumerate() {
            self.index.insert(entity.id, index);
        }
    }

    fn rebuild_spatial(&mut self) {
        self.spatial.clear();
        let ids: Vec<(EntityId, Vec3)> = self
            .items
            .iter()
            .map(|entity| (entity.id, entity.position))
            .collect();
        for (id, position) in ids {
            self.insert_spatial(id, position);
        }
    }

    fn insert_spatial(&mut self, id: EntityId, position: Vec3) {
        self.spatial.entry(cell_of(position)).or_default().push(id);
    }
}

fn cell_of(position: Vec3) -> IVec3 {
    position.floor().as_ivec3()
}

fn solid_at(world: &dyn BlockSource, cell: IVec3) -> bool {
    !world.solid_blocks_in(cell, cell).is_empty()
}

#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn move_axis(world: &dyn BlockSource, entity: &mut ItemEntity, axis: usize, distance: f32) {
    if distance == 0.0 {
        return;
    }
    let mut candidate = entity.position;
    candidate[axis] += distance;
    let probe = if distance > 0.0 {
        candidate + Vec3::splat(ITEM_HALF)
    } else {
        candidate - Vec3::splat(ITEM_HALF)
    };
    let candidate_cell = cell_of(candidate);
    let probe_cell = cell_of(probe);
    let candidate_solid = solid_at(world, candidate_cell);
    let blocked = candidate_solid || solid_at(world, probe_cell);
    if blocked {
        entity.velocity[axis] = 0.0;
        if axis == 1 {
            let contact = if candidate_solid {
                candidate_cell
            } else {
                probe_cell
            };
            entity.position.y = if distance < 0.0 {
                (contact.y + 1) as f32 + ITEM_HALF
            } else {
                contact.y as f32 - ITEM_HALF
            };
        }
    } else {
        entity.position = candidate;
    }
}

#[cfg(test)]
mod tests {
    use crate::block::Block;
    use crate::world::World;

    use super::super::inventory::Inventory;
    use super::super::item::ItemRegistry;
    use super::*;

    fn registry() -> ItemRegistry {
        ItemRegistry::builtin(&crate::content::v1_default())
    }

    fn floor() -> World {
        let mut world = World::default();
        for x in -2..=2 {
            for z in -2..=2 {
                world.set_block(IVec3::new(x, 0, z), Block::Stone);
            }
        }
        world
    }

    #[test]
    fn items_fall_and_rest_on_the_floor() {
        let registry = registry();
        let mut store = EntityStore::default();
        store.spawn(
            registry.stack("core:stone", 1).unwrap(),
            Vec3::new(0.5, 4.0, 0.5),
            Vec3::ZERO,
        );
        let world = floor();

        for _ in 0..180 {
            store.tick(&world, 1.0 / 60.0);
        }

        let entity = store.iter().next().unwrap();
        assert!(
            (entity.position.y - 1.125).abs() < 1e-3,
            "{:?}",
            entity.position
        );
        assert_eq!(entity.velocity.y, 0.0);
    }

    #[test]
    fn pickup_moves_items_into_the_inventory() {
        let registry = registry();
        let mut store = EntityStore::default();
        let item = registry.stack("core:stone", 5).unwrap();
        store.spawn(item, Vec3::new(0.5, 1.0, 0.5), Vec3::ZERO);
        let mut inventory = Inventory::new();

        let picked = store.pickup(Vec3::new(0.5, 1.0, 0.5), 1.5, &mut inventory, &registry);

        assert_eq!(picked, 5);
        assert!(store.is_empty());
        assert_eq!(inventory.count(item.item), 5);
    }

    #[test]
    fn a_full_inventory_leaves_the_remainder_on_the_ground() {
        let registry = registry();
        let item = registry.stack("core:stone", 5).unwrap();
        let mut store = EntityStore::default();
        store.spawn(item, Vec3::new(0.5, 1.0, 0.5), Vec3::ZERO);
        let mut inventory = Inventory::new();
        for index in 0..super::super::inventory::TOTAL_SLOTS {
            inventory.set_slot(index, registry.stack("core:dirt", 64));
        }

        let picked = store.pickup(Vec3::new(0.5, 1.0, 0.5), 1.5, &mut inventory, &registry);

        assert_eq!(picked, 0);
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn old_items_despawn() {
        let registry = registry();
        let mut store = EntityStore::default();
        store.spawn(
            registry.stack("core:stone", 1).unwrap(),
            Vec3::new(0.5, 2.0, 0.5),
            Vec3::ZERO,
        );
        store.items[0].age = DESPAWN_SECONDS;

        store.tick(&World::default(), 1.0 / 60.0);

        assert!(store.is_empty());
    }

    #[test]
    fn records_round_trip_ids_and_stacks() {
        let registry = registry();
        let mut store = EntityStore::default();
        let id = store.spawn(
            registry.stack("core:stone", 3).unwrap(),
            Vec3::new(1.0, 2.0, 3.0),
            Vec3::new(0.5, 0.0, -0.5),
        );
        let records = store.records(&registry);
        let restored = EntityStore::from_records(records, &registry);

        assert_eq!(restored.len(), 1);
        assert_eq!(restored.iter().next().unwrap().id, id);
        assert_eq!(restored.iter().next().unwrap().stack.count, 3);
    }
}
