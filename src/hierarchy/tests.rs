use super::*;

const PLAYER: Vec3 = Vec3::new(16.0, 40.0, 16.0);
const PP_RAD: f32 = DEFAULT_PIXELS_PER_RADIAN;

fn selected(player: Vec3) -> Hierarchy {
    selected_at(player, PP_RAD)
}

fn selected_at(player: Vec3, pixels_per_radian: f32) -> Hierarchy {
    let mut hierarchy = Hierarchy::default();
    assert!(hierarchy.update(player, DEFAULT_DETAIL_CHUNKS, pixels_per_radian));
    hierarchy
}

/// Level-2 key at a cell offset from the player's `(0, 0)` far cell.
fn parent_at(offset: IVec2) -> SectionKey {
    SectionKey::new(2, IVec3::new(offset.x, 0, offset.y))
}

fn children_of(parent: SectionKey) -> Vec<SectionKey> {
    in_world_children(parent)
}

fn ground_keys(hierarchy: &Hierarchy, level: u8) -> Vec<SectionKey> {
    hierarchy
        .target()
        .iter()
        .copied()
        .filter(|key| key.level == level && key.coord.y == 0)
        .collect()
}

fn ring_keys(
    player: Vec3,
    level: u8,
    extent: i32,
    inner_radius: i32,
    outer_radius: i32,
    layers: i32,
) -> Vec<SectionKey> {
    let mut keys = Vec::new();
    ring(
        &mut keys,
        player,
        level,
        extent,
        inner_radius,
        outer_radius,
        layers,
    );
    keys
}

fn footprints_overlap(left: SectionKey, right: SectionKey) -> bool {
    let left_min = left.origin();
    let left_max = left_min + IVec3::splat(left.extent());
    let right_min = right.origin();
    let right_max = right_min + IVec3::splat(right.extent());
    left_min.cmplt(right_max).all() && right_min.cmplt(left_max).all()
}

fn assert_disjoint(left: &[SectionKey], right: &[SectionKey]) {
    for left_key in left {
        for right_key in right {
            assert!(
                !footprints_overlap(*left_key, *right_key),
                "{left_key:?} overlaps {right_key:?}"
            );
        }
    }
}

/// Samples circles at the far radii and requires exactly one selected
/// ground footprint covering each sample.
fn assert_unique_ground_coverage(hierarchy: &Hierarchy, player: Vec3, radii: &[f32]) {
    let ground: Vec<SectionKey> = hierarchy
        .target()
        .iter()
        .copied()
        .filter(|key| key.coord.y == 0)
        .collect();
    for &radius in radii {
        for step in 0..24_u8 {
            let angle = f32::from(step) * std::f32::consts::TAU / 24.0;
            let x = player.x + radius * angle.cos();
            let z = player.z + radius * angle.sin();
            let hits = ground
                .iter()
                .filter(|key| {
                    let origin = key.origin();
                    let max = origin + IVec3::splat(key.extent());
                    x >= origin.x as f32
                        && x < max.x as f32
                        && z >= origin.z as f32
                        && z < max.z as f32
                })
                .count();
            assert_eq!(
                hits, 1,
                "radius {radius} angle {step}: expected one footprint at ({x}, {z})"
            );
        }
    }
}

fn assert_no_double_draw(hierarchy: &Hierarchy) {
    let target: HashSet<SectionKey> = hierarchy.target().iter().copied().collect();
    for key in hierarchy.target() {
        for ancestor in ancestors(*key) {
            assert!(
                !target.contains(&ancestor),
                "{ancestor:?} and its descendant {key:?} are both selected"
            );
        }
    }
}

#[test]
fn section_extents_match_the_level_math() {
    assert_eq!(SectionKey::new(1, IVec3::ZERO).extent(), 64);
    assert_eq!(SectionKey::new(2, IVec3::ZERO).extent(), 128);
    assert_eq!(SectionKey::new(3, IVec3::ZERO).extent(), 256);
    assert_eq!(SectionKey::new(4, IVec3::ZERO).extent(), 512);
    assert_eq!(LEVEL1_EXTENT, 64);
    assert_eq!(LEVEL2_EXTENT, 128);
    assert_eq!(LEVEL3_EXTENT, 256);
    assert_eq!(LEVEL4_EXTENT, 512);
}

#[test]
fn a_parent_owns_eight_distinct_children() {
    let parent = SectionKey::new(2, IVec3::new(-1, 0, -3));
    let children = children_of(parent);
    let unique: HashSet<SectionKey> = children.iter().copied().collect();

    assert_eq!(unique.len(), 8);
    for child in children {
        assert_eq!(child.level, 1);
        assert_eq!(child.parent(), Some(parent));
        assert!(child.coord.x < 0 && child.coord.z < 0, "{child:?}");
    }
}

#[test]
fn projected_size_falls_with_distance() {
    let near = projected_pixels(LEVEL1_EXTENT, 400.0, PP_RAD);
    let far = projected_pixels(LEVEL1_EXTENT, 800.0, PP_RAD);

    assert!(near > far);
    assert!((near - 144.0).abs() < 1e-4);
    assert!((far - 72.0).abs() < 1e-4);
    assert_eq!(projected_pixels(LEVEL1_EXTENT, 0.0, PP_RAD), 64.0 * PP_RAD);
}

#[test]
fn hysteresis_holds_inside_the_band_and_flips_outside_it() {
    assert!(!refined_for_pixels(false, REFINE_PIXELS));
    assert!(refined_for_pixels(false, REFINE_PIXELS + 0.1));
    assert!(refined_for_pixels(true, COLLAPSE_PIXELS + 0.1));
    assert!(!refined_for_pixels(true, COLLAPSE_PIXELS));
    const { assert!(COLLAPSE_PIXELS < REFINE_PIXELS) };

    let band = (-8..=8)
        .flat_map(|x| (-8..=8).map(move |z| parent_at(IVec2::new(x, z))))
        .find(|key| {
            let pixels = refine_pixels(PLAYER, *key, PP_RAD);
            pixels > COLLAPSE_PIXELS && pixels < REFINE_PIXELS
        })
        .expect("a parent inside the hysteresis band");

    assert!(!refine_state(false, band, PLAYER, PP_RAD));
    assert!(refine_state(true, band, PLAYER, PP_RAD));
}

#[test]
fn refinement_refines_near_and_collapses_far() {
    let near = parent_at(IVec2::new(6, 0));
    let far = parent_at(IVec2::new(12, 0));

    assert!(refine_state(false, near, PLAYER, PP_RAD));
    assert!(refine_state(true, near, PLAYER, PP_RAD));
    assert!(!refine_state(true, far, PLAYER, PP_RAD));
    assert!(!refine_state(false, far, PLAYER, PP_RAD));
}

#[test]
fn a_parent_stays_selected_until_all_children_are_ready() {
    let mut hierarchy = selected(PLAYER);
    let parent = parent_at(IVec2::new(6, 0));
    let children = children_of(parent);

    assert!(hierarchy.refined.contains(&parent));
    assert!(hierarchy.target().contains(&parent));

    for child in &children[..7] {
        hierarchy.mark_ready(*child);
    }
    assert!(hierarchy.update(PLAYER, DEFAULT_DETAIL_CHUNKS, PP_RAD));
    assert!(
        hierarchy.target().contains(&parent),
        "parent must stay while a child is missing"
    );
    assert!(
        children
            .iter()
            .all(|child| !hierarchy.target().contains(child)),
        "a partial child set must not draw"
    );

    hierarchy.mark_ready(children[7]);
    assert!(hierarchy.update(PLAYER, DEFAULT_DETAIL_CHUNKS, PP_RAD));
    assert!(!hierarchy.target().contains(&parent));
    assert!(
        children
            .iter()
            .all(|child| hierarchy.target().contains(child))
    );
    assert_no_double_draw(&hierarchy);
}

#[test]
fn collapse_keeps_the_children_until_the_parent_is_ready() {
    let mut hierarchy = selected(PLAYER);
    let parent = parent_at(IVec2::new(6, 0));
    let children = children_of(parent);
    for child in &children {
        hierarchy.mark_ready(*child);
    }
    assert!(hierarchy.update(PLAYER, DEFAULT_DETAIL_CHUNKS, PP_RAD));
    assert!(!hierarchy.target().contains(&parent));

    // Walk away until the projection drops below the collapse threshold.
    let far = Vec3::new(-240.0, 40.0, 16.0);
    assert!(hierarchy.update(far, DEFAULT_DETAIL_CHUNKS, PP_RAD));
    assert!(!hierarchy.refined.contains(&parent));
    assert!(
        children
            .iter()
            .all(|child| hierarchy.target().contains(child)),
        "children keep covering the parent until it is ready"
    );
    assert!(!hierarchy.target().contains(&parent));
    assert!(hierarchy.queue().contains(&parent));

    hierarchy.mark_ready(parent);
    assert!(hierarchy.update(far, DEFAULT_DETAIL_CHUNKS, PP_RAD));
    assert!(hierarchy.target().contains(&parent));
    assert!(
        children
            .iter()
            .all(|child| !hierarchy.target().contains(child))
    );
    assert_no_double_draw(&hierarchy);
}

#[test]
fn the_selection_never_holds_a_parent_and_child_together() {
    let hierarchy = selected(PLAYER);
    assert_no_double_draw(&hierarchy);

    let mut hierarchy = hierarchy;
    let parent = *hierarchy
        .fallbacks
        .keys()
        .next()
        .expect("a refined parent waiting on children");
    for child in children_of(parent) {
        hierarchy.mark_ready(child);
    }
    assert!(hierarchy.update(PLAYER, DEFAULT_DETAIL_CHUNKS, PP_RAD));
    assert_no_double_draw(&hierarchy);
}

#[test]
fn readiness_moves_from_requested_to_building_to_ready() {
    let mut hierarchy = Hierarchy::default();
    let key = SectionKey::new(1, IVec3::ZERO);

    assert_eq!(hierarchy.state(key), Readiness::Missing);
    hierarchy.mark_requested(key);
    assert_eq!(hierarchy.state(key), Readiness::Requested);
    assert!(!hierarchy.is_ready(key));
    hierarchy.mark_building(key);
    assert_eq!(hierarchy.state(key), Readiness::Building);
    hierarchy.mark_ready(key);
    assert!(hierarchy.is_ready(key));
}

#[test]
fn off_target_ready_sections_are_forgotten_unless_resident() {
    let mut hierarchy = selected(PLAYER);
    let desired = hierarchy.target()[0];
    let off_target = SectionKey::new(2, IVec3::new(100, 0, 100));
    let resident_key = SectionKey::new(2, IVec3::new(200, 0, 200));
    for key in [desired, off_target, resident_key] {
        hierarchy.mark_ready(key);
    }
    let resident: HashSet<SectionKey> = HashSet::from([resident_key]);

    assert!(hierarchy.update_with_resident(PLAYER, DEFAULT_DETAIL_CHUNKS, PP_RAD, &resident));

    assert_eq!(
        hierarchy.state(desired),
        Readiness::Ready,
        "a desired empty section keeps its readiness"
    );
    assert_eq!(
        hierarchy.state(off_target),
        Readiness::Missing,
        "an off-target empty section must not accumulate state"
    );
    assert_eq!(
        hierarchy.state(resident_key),
        Readiness::Ready,
        "a resident non-empty section keeps its readiness"
    );
}

#[test]
fn dirty_sections_rebuild_before_new_target_sections() {
    let mut hierarchy = Hierarchy::default();
    let dirty = SectionKey::new(1, IVec3::new(5, 0, 5));
    let target = SectionKey::new(1, IVec3::ZERO);
    hierarchy.target = vec![dirty, target];
    hierarchy.queue = vec![target];
    hierarchy.states.insert(dirty, Readiness::Ready);
    hierarchy.states.insert(target, Readiness::Requested);

    hierarchy.mark_dirty([dirty]);

    assert_eq!(hierarchy.state(dirty), Readiness::Requested);
    let mut cursor = 0;
    assert_eq!(hierarchy.next_build(&mut cursor), Some(dirty));
    assert_eq!(cursor, 0, "a dirty rebuild must not consume the queue");
    assert_eq!(hierarchy.next_build(&mut cursor), Some(target));
    assert_eq!(cursor, 1);
    assert_eq!(hierarchy.next_build(&mut cursor), None);
}

#[test]
fn an_edited_selected_section_rebuilds_before_the_queue() {
    let mut hierarchy = selected(PLAYER);
    let key = hierarchy.target()[0];
    hierarchy.mark_ready(key);
    assert!(hierarchy.update(PLAYER, DEFAULT_DETAIL_CHUNKS, PP_RAD));
    assert!(hierarchy.target().contains(&key));

    hierarchy.mark_dirty([key]);

    assert_eq!(hierarchy.state(key), Readiness::Requested);
    let mut cursor = 0;
    assert_eq!(hierarchy.next_build(&mut cursor), Some(key));
}

#[test]
fn a_dirty_section_outside_the_target_is_not_built() {
    let mut hierarchy = Hierarchy::default();
    let outside = SectionKey::new(2, IVec3::new(40, 0, 40));

    hierarchy.mark_dirty([outside]);

    assert_eq!(hierarchy.state(outside), Readiness::Requested);
    let mut cursor = 0;
    assert_eq!(hierarchy.next_build(&mut cursor), None);
}

#[test]
fn a_dirty_far_section_rebuilds_before_the_queue() {
    let mut hierarchy = selected(PLAYER);
    let far = *hierarchy
        .target()
        .iter()
        .rev()
        .find(|key| key.level == 4)
        .expect("a level-4 target");
    hierarchy.mark_ready(far);
    assert!(hierarchy.update(PLAYER, DEFAULT_DETAIL_CHUNKS, PP_RAD));
    assert!(hierarchy.target().contains(&far));

    hierarchy.mark_dirty([far]);

    assert_eq!(hierarchy.state(far), Readiness::Requested);
    let mut cursor = 0;
    assert_eq!(hierarchy.next_build(&mut cursor), Some(far));
    assert_eq!(cursor, 0, "a dirty rebuild must not consume the queue");
}

#[test]
fn a_dirty_far_section_outside_the_target_is_discarded() {
    for level in [3, 4] {
        let mut hierarchy = selected(PLAYER);
        hierarchy.queue.clear();
        let outside = SectionKey::new(level, IVec3::new(100, 0, 100));

        hierarchy.mark_dirty([outside]);

        assert_eq!(hierarchy.state(outside), Readiness::Requested);
        let mut cursor = 0;
        assert_eq!(hierarchy.next_build(&mut cursor), None, "{outside:?}");
    }
}

#[test]
fn the_queue_builds_a_parent_before_its_children() {
    let hierarchy = selected(PLAYER);
    let parent = *hierarchy
        .fallbacks
        .keys()
        .next()
        .expect("a refined parent waiting on children");
    let children = hierarchy.fallbacks[&parent].clone();
    let position = |key| {
        hierarchy
            .queue()
            .iter()
            .position(|queued| *queued == key)
            .expect("key is queued")
    };

    let parent_position = position(parent);
    for child in children {
        assert!(
            position(child) > parent_position,
            "child {child:?} is built before its parent {parent:?}"
        );
    }
}

#[test]
fn the_target_is_ordered_nearest_first() {
    let hierarchy = selected(PLAYER);
    let distances: Vec<f32> = hierarchy
        .target()
        .iter()
        .map(|key| nearest_xz_distance(PLAYER, *key))
        .collect();

    assert!(distances.windows(2).all(|pair| pair[0] <= pair[1]));
}

#[test]
fn the_detail_radius_is_excluded() {
    let hierarchy = selected(PLAYER);
    let detail = detail_meters(DEFAULT_DETAIL_CHUNKS) as f32;

    assert_ne!(hierarchy.target().len(), 0);
    for key in hierarchy.target() {
        assert!(
            farthest_xz_distance(PLAYER, *key) >= detail,
            "{key:?} lies inside the L0 detail radius"
        );
    }
}

#[test]
fn negative_coordinates_select_all_four_levels() {
    let path_player = Vec3::new(-1_200.0, 40.0, -1_200.0);
    let hierarchy = selected(path_player);

    for level in 1..=4 {
        assert!(
            hierarchy.target().iter().any(|key| key.level == level),
            "level {level} missing"
        );
    }
    assert_no_double_draw(&hierarchy);
}

#[test]
fn far_levels_cover_the_annuli_with_recursive_refinement() {
    let hierarchy = selected(PLAYER);
    let l1 = ground_keys(&hierarchy, 1);
    let l2 = ground_keys(&hierarchy, 2);
    let l3 = ground_keys(&hierarchy, 3);
    let l4 = ground_keys(&hierarchy, 4);

    assert_ne!(l1.len(), 0);
    assert_ne!(l2.len(), 0);
    assert_ne!(l3.len(), 0);
    assert_ne!(l4.len(), 0);
    assert!(
        hierarchy.refined.iter().any(|key| key.level == 3),
        "no level-3 root refined"
    );
    assert!(
        hierarchy.refined.iter().any(|key| key.level == 4),
        "no level-4 root refined"
    );
    assert!(
        hierarchy
            .target()
            .iter()
            .all(|key| key.origin().y < WORLD_HEIGHT),
        "an above-world child was selected"
    );

    assert_disjoint(&l1, &l2);
    assert_disjoint(&l1, &l3);
    assert_disjoint(&l1, &l4);
    assert_disjoint(&l2, &l3);
    assert_disjoint(&l2, &l4);
    assert_disjoint(&l3, &l4);
    assert_unique_ground_coverage(
        &hierarchy,
        PLAYER,
        &[
            1_793.0, 2_049.0, 3_000.0, 3_584.0, 4_000.0, 4_097.0, 5_000.0, 6_390.0, 6_400.0,
            7_000.0, 8_000.0, 8_191.0,
        ],
    );
    assert_no_double_draw(&hierarchy);
}

#[test]
fn a_boosted_projection_refines_far_levels_recursively() {
    // A 2,000 px/rad viewport refines the 4 km roots into level-3 children
    // and those children into level-2 leaves.
    const BOOSTED_PP_RAD: f32 = 2_000.0;
    let hierarchy = selected_at(PLAYER, BOOSTED_PP_RAD);
    let nested_level2 = hierarchy.target().iter().any(|key| {
        key.level == 2 && nearest_xz_distance(PLAYER, *key) > LEVEL2_OUTER_METERS as f32
    });

    assert!(nested_level2, "the level-4 band did not recurse to level 2");
    assert!(
        hierarchy
            .target()
            .iter()
            .all(|key| key.origin().y < WORLD_HEIGHT),
        "a far target key was above the world"
    );
    assert_unique_ground_coverage(
        &hierarchy,
        PLAYER,
        &[
            1_793.0, 2_049.0, 3_000.0, 3_584.0, 4_097.0, 5_000.0, 6_400.0, 7_000.0, 8_191.0,
        ],
    );
    assert_no_double_draw(&hierarchy);
}

#[test]
fn far_levels_follow_negative_player_coordinates() {
    let player = Vec3::new(-5_700.0, 40.0, -3_300.0);
    let hierarchy = selected(player);
    let l2 = ground_keys(&hierarchy, 2);
    let l3 = ground_keys(&hierarchy, 3);
    let l4 = ground_keys(&hierarchy, 4);

    assert_ne!(l2.len(), 0);
    assert_ne!(l3.len(), 0);
    assert_ne!(l4.len(), 0);
    assert_disjoint(&l2, &l3);
    assert_disjoint(&l2, &l4);
    assert_disjoint(&l3, &l4);
    assert_unique_ground_coverage(
        &hierarchy,
        player,
        &[
            1_793.0, 2_049.0, 3_000.0, 3_584.0, 4_097.0, 5_000.0, 6_390.0, 8_000.0, 8_191.0,
        ],
    );
    assert!(
        l4.iter().all(|key| key.coord.y == 0),
        "level-4 sections are selected only at y = 0"
    );
    assert_no_double_draw(&hierarchy);
}

/// The refined parent at `level` in a fresh selection.
fn refined_parent(hierarchy: &Hierarchy, level: u8) -> SectionKey {
    *hierarchy
        .refined
        .iter()
        .find(|key| key.level == level)
        .unwrap_or_else(|| panic!("no refined level-{level} parent"))
}

#[test]
fn every_far_level_refines_only_into_in_world_children() {
    for level in 2..=4 {
        let hierarchy = selected(PLAYER);
        let parent = refined_parent(&hierarchy, level);
        let children = in_world_children(parent);
        let empty: Vec<SectionKey> = (0..8)
            .filter_map(|index| parent.child(index))
            .filter(|child| child.origin().y >= WORLD_HEIGHT)
            .collect();

        assert_eq!(children.len(), if level == 2 { 8 } else { 4 });
        assert_eq!(children.len() + empty.len(), 8);
        assert!(
            children.iter().all(|child| child.origin().y < WORLD_HEIGHT),
            "{parent:?}"
        );
        assert!(
            empty.iter().all(|child| child.origin().y >= WORLD_HEIGHT),
            "{parent:?}"
        );
        assert!(
            hierarchy.fallbacks[&parent] == children,
            "the fallback list must contain exactly the in-world children"
        );
        for child in &empty {
            assert_eq!(hierarchy.state(*child), Readiness::Missing);
            assert!(!hierarchy.target().contains(child));
            assert!(!hierarchy.queue().contains(child));
        }
    }
}

#[test]
fn far_parent_waits_for_every_in_world_child_before_the_swap() {
    for level in 2..=4 {
        let mut hierarchy = selected(PLAYER);
        let parent = refined_parent(&hierarchy, level);
        let children = in_world_children(parent);

        assert!(hierarchy.target().contains(&parent));
        for child in &children[..children.len() - 1] {
            hierarchy.mark_ready(*child);
        }
        assert!(hierarchy.update(PLAYER, DEFAULT_DETAIL_CHUNKS, PP_RAD));
        assert!(
            hierarchy.target().contains(&parent),
            "level-{level} parent must stay while a child is missing"
        );
        assert!(
            children
                .iter()
                .all(|child| !hierarchy.target().contains(child)),
            "a partial level-{level} child set must not draw"
        );

        hierarchy.mark_ready(children[children.len() - 1]);
        assert!(hierarchy.update(PLAYER, DEFAULT_DETAIL_CHUNKS, PP_RAD));
        assert!(!hierarchy.target().contains(&parent));
        assert!(
            children
                .iter()
                .all(|child| hierarchy.target().contains(child)),
            "the level-{level} in-world children must swap in together"
        );
        assert!(
            hierarchy.refined.contains(&parent),
            "the level-{level} hysteresis memory must survive the swap"
        );
        assert_no_double_draw(&hierarchy);
    }
}

#[test]
fn far_collapse_keeps_the_children_until_the_parent_is_ready() {
    // Low projection collapses every far root without moving the rings.
    const COLLAPSING_PP_RAD: f32 = 400.0;

    for level in [3, 4] {
        let mut hierarchy = selected(PLAYER);
        let parent = refined_parent(&hierarchy, level);
        let children = in_world_children(parent);
        for child in &children {
            hierarchy.mark_ready(*child);
        }
        assert!(hierarchy.update(PLAYER, DEFAULT_DETAIL_CHUNKS, PP_RAD));
        assert!(!hierarchy.target().contains(&parent));

        // The projection is fixed at the shipped value in production;
        // force the rebuild here because only the threshold input changed.
        hierarchy.dirty = true;
        assert!(hierarchy.update(PLAYER, DEFAULT_DETAIL_CHUNKS, COLLAPSING_PP_RAD));
        assert!(
            !hierarchy.refined.contains(&parent),
            "level-{level} parent stayed refined below the collapse threshold"
        );
        assert!(
            children
                .iter()
                .all(|child| hierarchy.target().contains(child)),
            "level-{level} children keep covering the parent until it is ready"
        );
        assert!(!hierarchy.target().contains(&parent));
        assert!(hierarchy.queue().contains(&parent));
        assert_no_double_draw(&hierarchy);

        hierarchy.mark_ready(parent);
        assert!(hierarchy.update(PLAYER, DEFAULT_DETAIL_CHUNKS, COLLAPSING_PP_RAD));
        assert!(hierarchy.target().contains(&parent));
        assert!(
            children
                .iter()
                .all(|child| !hierarchy.target().contains(child)),
            "level-{level} children must leave when the parent is drawn"
        );
        assert_no_double_draw(&hierarchy);
    }
}

#[test]
fn far_nodes_honor_hysteresis_at_both_thresholds() {
    for level in [3, 4] {
        let (extent, inner, outer) = match level {
            3 => (LEVEL3_EXTENT, LEVEL2_OUTER_METERS, LEVEL3_OUTER_METERS),
            _ => (LEVEL4_EXTENT, LEVEL3_OUTER_METERS, LEVEL4_OUTER_METERS),
        };
        let candidates = ring_keys(PLAYER, level, extent, inner, outer, 1);
        let pixels = |key| refine_pixels(PLAYER, key, PP_RAD);

        let band = candidates
            .iter()
            .copied()
            .find(|key| {
                let value = pixels(*key);
                value > COLLAPSE_PIXELS && value < REFINE_PIXELS
            })
            .unwrap_or_else(|| panic!("no level-{level} node in the hysteresis band"));
        let near = candidates
            .iter()
            .copied()
            .find(|key| pixels(*key) > REFINE_PIXELS)
            .unwrap_or_else(|| panic!("no level-{level} node above the refine threshold"));
        let far = candidates
            .iter()
            .copied()
            .find(|key| pixels(*key) < COLLAPSE_PIXELS)
            .unwrap_or_else(|| panic!("no level-{level} node below the collapse threshold"));

        assert!(!refine_state(false, band, PLAYER, PP_RAD));
        assert!(refine_state(true, band, PLAYER, PP_RAD));
        assert!(refine_state(false, near, PLAYER, PP_RAD));
        assert!(!refine_state(true, far, PLAYER, PP_RAD));
    }
}

#[test]
fn eviction_keeps_ancestors_and_pending_parents() {
    let target = vec![SectionKey::new(1, IVec3::new(10, 0, 10))];
    let ancestor = target[0].parent().expect("parent");
    let grandparent = ancestor.parent().expect("grandparent");

    let keep = retained_keys(&target, &HashMap::new());
    assert!(keep.contains(&target[0]));
    assert!(keep.contains(&ancestor));
    assert!(keep.contains(&grandparent));

    let parent = parent_at(IVec2::new(6, 0));
    let children = children_of(parent);
    let mut fallbacks = HashMap::new();
    fallbacks.insert(parent, children.clone());
    let keep = retained_keys(&target, &fallbacks);
    assert!(keep.contains(&parent));
    assert!(children.iter().all(|child| keep.contains(child)));
}

#[test]
fn eviction_keeps_the_eight_kilometre_margin_outside_the_target() {
    let detail = detail_meters(DEFAULT_DETAIL_CHUNKS);
    let target = vec![SectionKey::new(4, IVec3::new(15, 0, 0))];
    // 8,192-8,704 m and 8,704-9,216 m sit inside the level-4 margin.
    let margin = SectionKey::new(4, IVec3::new(16, 0, 0));
    let edge = SectionKey::new(4, IVec3::new(17, 0, 0));
    // 9,216 m is beyond the 8,192 m outer radius plus the 512 m margin.
    let beyond = SectionKey::new(4, IVec3::new(18, 0, 0));
    let inside_l0 = SectionKey::new(1, IVec3::new(0, 0, 1));
    // A section that merely touches the L0 boundary must not stay drawn
    // over the exact chunks.
    let overlapping_l0 = SectionKey::new(1, IVec3::new(0, 0, 7));
    let uploaded: HashSet<SectionKey> = [margin, edge, beyond, inside_l0, overlapping_l0]
        .into_iter()
        .collect();

    let stale = evictions(&uploaded, &target, &HashMap::new(), Vec3::ZERO, detail);

    assert!(stale.contains(&inside_l0));
    assert!(stale.contains(&overlapping_l0));
    assert!(
        !stale.contains(&margin),
        "the 8 km margin keeps a level-4 ring alive"
    );
    assert!(!stale.contains(&edge), "the edge of the margin stays");
    assert!(stale.contains(&beyond));
}

#[test]
fn eviction_drops_a_parent_covered_by_its_children() {
    let detail = detail_meters(DEFAULT_DETAIL_CHUNKS);
    let parent = parent_at(IVec2::new(6, 0));
    let target: Vec<SectionKey> = children_of(parent);
    let uploaded: HashSet<SectionKey> = [parent].into_iter().collect();

    let stale = evictions(&uploaded, &target, &HashMap::new(), PLAYER, detail);

    assert_eq!(stale, vec![parent]);
}

#[test]
fn eviction_drops_a_child_under_a_drawn_parent() {
    let detail = detail_meters(DEFAULT_DETAIL_CHUNKS);
    let parent = parent_at(IVec2::new(6, 0));
    let child = parent.child(0).expect("child");
    let target = vec![parent];
    let uploaded: HashSet<SectionKey> = [child].into_iter().collect();

    let stale = evictions(&uploaded, &target, &HashMap::new(), PLAYER, detail);

    assert!(stale.contains(&child));
}

#[test]
fn eviction_drops_a_far_parent_covered_by_in_world_children() {
    let detail = detail_meters(DEFAULT_DETAIL_CHUNKS);
    let parent = SectionKey::new(4, IVec3::new(-8, 0, -8));
    let target = in_world_children(parent);
    let uploaded: HashSet<SectionKey> = [parent].into_iter().collect();

    let stale = evictions(&uploaded, &target, &HashMap::new(), PLAYER, detail);

    assert_eq!(stale, vec![parent]);
}

#[test]
fn eviction_drops_a_far_parent_covered_by_grandchildren() {
    let detail = detail_meters(DEFAULT_DETAIL_CHUNKS);
    let parent = SectionKey::new(4, IVec3::new(-8, 0, -8));
    let mut target = Vec::new();
    for child in in_world_children(parent) {
        target.extend(in_world_children(child));
    }
    let uploaded: HashSet<SectionKey> = [parent].into_iter().collect();

    let stale = evictions(&uploaded, &target, &HashMap::new(), PLAYER, detail);

    assert_eq!(
        stale,
        vec![parent],
        "a far root is released once its descendants cover it"
    );
}

#[test]
fn eviction_keeps_a_fallback_parent_and_its_ready_children() {
    let detail = detail_meters(DEFAULT_DETAIL_CHUNKS);
    let parent = SectionKey::new(4, IVec3::new(-8, 0, -8));
    let children = in_world_children(parent);
    let mut fallbacks = HashMap::new();
    fallbacks.insert(parent, children.clone());
    let target = vec![parent];
    let uploaded: HashSet<SectionKey> = children
        .iter()
        .copied()
        .chain(std::iter::once(parent))
        .collect();

    let stale = evictions(&uploaded, &target, &fallbacks, PLAYER, detail);

    assert!(stale.is_empty(), "the pending swap must stay resident");
}

#[test]
fn the_far_annulus_is_covered_exactly_once() {
    let mut hierarchy = selected(PLAYER);
    assert_annulus_covered(&hierarchy);

    // Swap every refined parent for its children and check again.
    let parents: Vec<SectionKey> = hierarchy.fallbacks.keys().copied().collect();
    for parent in parents {
        for child in children_of(parent) {
            hierarchy.mark_ready(child);
        }
    }
    assert!(hierarchy.update(PLAYER, DEFAULT_DETAIL_CHUNKS, PP_RAD));
    assert_annulus_covered(&hierarchy);
    assert_no_double_draw(&hierarchy);
}

/// Samples ground positions around the player and requires exactly one
/// selected footprint from the detail radius out to the far edge.
fn assert_annulus_covered(hierarchy: &Hierarchy) {
    let ground: Vec<&SectionKey> = hierarchy
        .target()
        .iter()
        .filter(|key| key.coord.y == 0)
        .collect();

    let mut uncovered = Vec::new();
    let mut doubled = Vec::new();
    for z in (-2_080..=2_080).step_by(32) {
        for x in (-2_080..=2_080).step_by(32) {
            let chebyshev = (IVec2::new(x, z) - IVec2::new(16, 16)).abs().max_element();
            if !(560..=2_000).contains(&chebyshev) {
                continue;
            }
            let hits = ground
                .iter()
                .filter(|key| {
                    let origin = key.origin();
                    let extent = key.extent();
                    x >= origin.x && x < origin.x + extent && z >= origin.z && z < origin.z + extent
                })
                .count();
            if hits == 0 {
                uncovered.push((x, z));
            } else if hits > 1 {
                doubled.push((x, z));
            }
        }
    }
    assert!(
        uncovered.is_empty(),
        "holes at {:?}",
        &uncovered[..1.min(uncovered.len())]
    );
    assert!(
        doubled.is_empty(),
        "overlaps at {:?}",
        &doubled[..1.min(doubled.len())]
    );
}

#[test]
fn every_refined_parent_has_a_fallback_entry_while_children_build() {
    let hierarchy = selected(PLAYER);

    assert!(!hierarchy.refined.is_empty());
    for parent in &hierarchy.refined {
        if children_of(*parent)
            .iter()
            .all(|child| hierarchy.is_ready(*child))
        {
            continue;
        }
        assert!(hierarchy.fallbacks.contains_key(parent));
        assert!(hierarchy.target().contains(parent));
    }
}
