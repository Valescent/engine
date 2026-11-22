use super::*;
use crate::config::MAX_RENDER_DISTANCE;
use crate::streaming::UNLOAD_MARGIN;
use crate::world::CHUNK_COLUMN;

#[test]
fn section_nodes_use_their_own_origin_and_extent() {
    let key = NodeKey::Section(SectionKey::new(1, glam::IVec3::new(1, 0, -1)));
    assert_eq!(key.extent(), 64.0);
    assert_eq!(key.origin(), Vec3::new(64.0, 0.0, -64.0));
}

#[test]
fn index_pattern_replays_the_quad_corner_order() {
    let corners = [0_u16, 1, 2, 0, 2, 3];
    for quad in 0..MAX_INDEXED_QUADS {
        let base = u16::try_from(quad * 4).unwrap();
        let pattern = index_pattern(base);
        assert_eq!(pattern, corners.map(|corner| base + corner));
        for (vertex, corner) in pattern.into_iter().zip(corners) {
            assert_eq!(u32::from(vertex) / 4, quad);
            assert_eq!(u32::from(vertex) % 4, u32::from(corner));
        }
    }
}

#[test]
fn indirect_args_cover_four_batches() {
    let batch = u64::from(SLOT_COUNT) * size_of::<DrawIndexedArgs>() as u64;
    assert_eq!(ARGS_BYTES, 4 * batch);
    assert!(size_of::<DrawIndexedArgs>() >= size_of::<DrawArgs>());
}

#[test]
fn slot_table_covers_the_largest_render_distance() {
    let radius = MAX_RENDER_DISTANCE + UNLOAD_MARGIN;
    let columns = (2 * radius + 1).pow(2);
    let chunks = u32::try_from(columns * CHUNK_COLUMN).unwrap();
    assert!(
        chunks <= SLOT_COUNT,
        "{chunks} chunks exceed {SLOT_COUNT} slots"
    );
}

#[test]
fn slots_count_only_resident_geometry() {
    let empty = Slot {
        index: 0,
        key: NodeKey::Chunk(ChunkCoord::new(0, 0, 0)),
        visible: true,
        info: ChunkInfo::default(),
        opaque: [None; 3],
        opaque_indexed: [false; 3],
        active: None,
        water: None,
        water_indexed: false,
        active_position: None,
    };
    assert!(!empty.has_geometry());
    assert_eq!(empty.drawn_quads(), 0);

    let range = QuadRange {
        first_quad: 0,
        quads: 3,
        pages: 1,
    };
    let opaque = Slot {
        opaque: [None, Some(range), None],
        active: Some(Lod::Lod1),
        ..empty
    };
    assert!(opaque.has_geometry());
    assert_eq!(opaque.drawn_quads(), 3);

    let inactive = Slot {
        active: None,
        ..opaque
    };
    assert!(inactive.has_geometry());
    assert_eq!(inactive.drawn_quads(), 0);
}

fn empty_slot(index: u32, chunk: ChunkCoord) -> Slot {
    Slot {
        index,
        key: NodeKey::Chunk(chunk),
        visible: true,
        info: ChunkInfo::default(),
        opaque: [None; 3],
        opaque_indexed: [false; 3],
        active: None,
        water: None,
        water_indexed: false,
        active_position: None,
    }
}

#[test]
fn selecting_an_empty_lod_clears_the_active_range() {
    let range = QuadRange {
        first_quad: 64,
        quads: 3,
        pages: 1,
    };
    let mut slot = empty_slot(0, ChunkCoord::new(0, 0, 0));
    slot.opaque[0] = Some(range);
    slot.active = Some(Lod::Lod0);
    slot.info.origin[3] = Lod::Lod0.scale();
    slot.info.opaque_first = range.first_quad;
    slot.info.opaque_quads = range.quads;
    let mut pool = QuadPool::new(64);

    let (_, info) = select_active_slot(&mut slot, Lod::Lod1, &mut pool, 1);

    assert_eq!(slot.active, None);
    assert_eq!(info.opaque_first, 0);
    assert_eq!(info.opaque_quads, 0);
    assert_eq!(slot.drawn_quads(), 0);
    assert_eq!(
        slot.opaque[0],
        Some(range),
        "the LOD0 range stays resident for a later switch"
    );
}

#[test]
fn active_slots_track_geometry_through_upload_and_removal() {
    let range = QuadRange {
        first_quad: 0,
        quads: 3,
        pages: 1,
    };
    let mut slots = vec![
        empty_slot(0, ChunkCoord::new(0, 0, 0)),
        empty_slot(1, ChunkCoord::new(1, 0, 0)),
    ];
    let mut active_slots = Vec::new();

    refresh_active(&mut slots, &mut active_slots, 0);
    assert_eq!(active_slots, Vec::<usize>::new());

    // An upload adds one entry, and refreshing again does not duplicate it.
    slots[0].opaque[0] = Some(range);
    slots[0].active = Some(Lod::Lod0);
    refresh_active(&mut slots, &mut active_slots, 0);
    assert_eq!(active_slots, vec![0]);
    refresh_active(&mut slots, &mut active_slots, 0);
    assert_eq!(active_slots, vec![0]);

    slots[1].water = Some(range);
    refresh_active(&mut slots, &mut active_slots, 1);
    assert_eq!(active_slots, vec![0, 1]);

    // The active list totals the same drawn quads as a full slot scan.
    let all = slots.iter().map(Slot::drawn_quads).sum::<u64>();
    let listed = active_slots
        .iter()
        .map(|&position| slots[position].drawn_quads())
        .sum::<u64>();
    assert_eq!(all, listed);

    // Removal drops the entry and keeps the swapped-in slot's link correct.
    slots[0].opaque[0] = None;
    slots[0].active = None;
    remove_active(&mut slots, &mut active_slots, 0);
    assert_eq!(active_slots, vec![1]);
    assert_eq!(slots[1].active_position, Some(0));
}
