//! Parent-retained hierarchy selection for the first LOD slice.
//!
//! L0 covers [`DEFAULT_DETAIL_CHUNKS`] chunks around the player. Level-1
//! sections (64 m) fill the ring from the detail radius out to 768 m, and
//! level-2 sections (128 m) fill the far ring from 768 m to 2,048 m.
//!
//! Level-2 parents are selected from the far ring and stay drawable as soon as
//! they are requested. A parent whose projected size exceeds [`REFINE_PIXELS`]
//! is refined into its eight level-1 children, but the swap only happens when
//! all eight children are ready (uploaded or empty). Until then the parent is
//! drawn and the children build underneath it. Once a parent is refined, it
//! stays refined until its projected size drops below [`COLLAPSE_PIXELS`]; the
//! band between the two thresholds is the hysteresis that stops a boundary from
//! thrashing. A selection never contains a parent and its children together.
//!
//! The projection uses `extent / distance * pixels_per_radian`, where distance
//! is measured from the camera to the nearest point of the section footprint.
//! The viewport is not threaded through this module yet, so the caller passes
//! [`DEFAULT_PIXELS_PER_RADIAN`] (`900 px/rad`, a 720 px viewport with a 60°
//! vertical field of view).

#![allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]

use std::collections::{HashMap, HashSet};

use glam::{IVec2, IVec3, Vec2, Vec3};

use crate::section::{SECTION_SIZE, SectionKey};

/// Default L0 detail radius in chunks.
pub(crate) const DEFAULT_DETAIL_CHUNKS: i32 = 16;

/// Level-1 outer radius in metres.
const LEVEL1_OUTER_METERS: i32 = 768;
/// Level-2 outer radius in metres.
const LEVEL2_OUTER_METERS: i32 = 2_048;

const LEVEL1_EXTENT: i32 = SECTION_SIZE * 2;
const LEVEL2_EXTENT: i32 = SECTION_SIZE * 4;

/// Projected child size, in pixels, above which a level-2 parent refines.
///
/// With the default projection this refines parents whose nearest point is
/// closer than about 800 m, so the innermost far ring gains detail.
pub(crate) const REFINE_PIXELS: f32 = 72.0;

/// Projected child size, in pixels, below which refined children collapse back
/// into their parent. The band between this and [`REFINE_PIXELS`] is the
/// hysteresis that keeps a section on one side of the boundary from thrashing.
pub(crate) const COLLAPSE_PIXELS: f32 = 62.0;

/// Projection scale used when the viewport height is not known: roughly a
/// 720 px viewport with a 60° vertical field of view (`720 / (pi / 3)`).
pub(crate) const DEFAULT_PIXELS_PER_RADIAN: f32 = 900.0;

/// How far outside the target an uploaded section stays resident before it is
/// evicted. The margin hides section-sized steps as the player crosses a cell.
const EVICTION_MARGIN_METERS: i32 = 128;

/// Per-key build readiness.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum Readiness {
    /// Not selected, or not requested yet.
    #[default]
    Missing,
    /// Queued for a build.
    Requested,
    /// Being generated, lit, meshed, or uploaded.
    Building,
    /// Uploaded, or generated empty and therefore drawable as nothing.
    Ready,
}

/// Projected size of a section face in pixels.
pub(crate) fn projected_pixels(extent: i32, distance: f32, pixels_per_radian: f32) -> f32 {
    extent as f32 / distance.max(1.0) * pixels_per_radian
}

/// Horizontal distance from the camera to the nearest point of a footprint.
fn nearest_xz_distance(player: Vec3, key: SectionKey) -> f32 {
    let origin = key.origin();
    let min = Vec2::new(origin.x as f32, origin.z as f32);
    let max = min + Vec2::splat(key.extent() as f32);
    let player = Vec2::new(player.x, player.z);
    (player - player.clamp(min, max)).length()
}

/// Horizontal distance from the camera to the farthest point of a footprint.
#[cfg(test)]
fn farthest_xz_distance(player: Vec3, key: SectionKey) -> f32 {
    let origin = key.origin();
    let min = Vec2::new(origin.x as f32, origin.z as f32);
    let max = min + Vec2::splat(key.extent() as f32);
    let offset = Vec2::new(
        (player.x - min.x).abs().max((player.x - max.x).abs()),
        (player.z - min.y).abs().max((player.z - max.y).abs()),
    );
    offset.length()
}

/// Projected size of one level-1 child at a parent's distance.
fn parent_pixels(player: Vec3, parent: SectionKey, pixels_per_radian: f32) -> f32 {
    projected_pixels(
        LEVEL1_EXTENT,
        nearest_xz_distance(player, parent),
        pixels_per_radian,
    )
}

/// Refinement memory for one projected size.
fn refined_for_pixels(refined: bool, pixels: f32) -> bool {
    if refined {
        pixels > COLLAPSE_PIXELS
    } else {
        pixels > REFINE_PIXELS
    }
}

/// Whether one coarse parent should draw its children at this projected size.
fn refine_state(refined: bool, parent: SectionKey, player: Vec3, pixels_per_radian: f32) -> bool {
    refined_for_pixels(refined, parent_pixels(player, parent, pixels_per_radian))
}

/// The eight level-1 children of a level-2 parent.
fn children_of(parent: SectionKey) -> [SectionKey; 8] {
    std::array::from_fn(|index| {
        parent
            .child(index)
            .expect("only level-2 parents are refined")
    })
}

/// World metres covered by the L0 detail radius.
fn detail_meters(detail_chunks: i32) -> i32 {
    detail_chunks.max(1) * SECTION_SIZE
}

/// Section cell containing a world position at `extent` metres per cell.
fn cell_at(position: Vec3, extent: i32) -> IVec2 {
    IVec2::new(
        (position.x.floor() as i32).div_euclid(extent),
        (position.z.floor() as i32).div_euclid(extent),
    )
}

fn ceil_div(value: i32, divisor: i32) -> i32 {
    (value + divisor - 1) / divisor
}

/// Appends the square ring of sections between `inner` and `outer` cells.
#[allow(clippy::too_many_arguments)]
fn ring(
    keys: &mut Vec<SectionKey>,
    player: Vec3,
    level: u8,
    extent: i32,
    inner: i32,
    outer: i32,
    layers: i32,
) {
    if inner > outer {
        return;
    }
    let center = cell_at(player, extent);
    for sx in -outer..=outer {
        for sz in -outer..=outer {
            if sx.abs().max(sz.abs()) < inner {
                continue;
            }
            let coord = center + IVec2::new(sx, sz);
            for y in 0..layers {
                keys.push(SectionKey::new(level, IVec3::new(coord.x, y, coord.y)));
            }
        }
    }
}

/// Ancestors of a section, nearest first.
fn ancestors(key: SectionKey) -> impl Iterator<Item = SectionKey> {
    std::iter::successors(key.parent(), |parent| parent.parent())
}

/// Deterministic coordinate key for stable ordering.
fn coord_order(key: SectionKey) -> (i32, i32, i32) {
    (key.coord.x, key.coord.y, key.coord.z)
}

/// Whether a parent is fully covered by eight drawn children.
fn covered_by_children(key: SectionKey, target: &HashSet<SectionKey>) -> bool {
    !target.contains(&key)
        && (0..8).all(|index| {
            key.child(index)
                .is_some_and(|child| target.contains(&child))
        })
}

/// Whether an upload is outside the target region plus the outer margin.
///
/// The outer margin is deliberately one-sided: a section that has fallen into
/// the L0 hole is evicted immediately, because keeping it would draw coarse
/// geometry over the exact chunks.
fn outside_target_region(key: SectionKey, player: Vec3, detail_meters: i32) -> bool {
    let nearest = nearest_xz_distance(player, key);
    nearest < detail_meters as f32
        || nearest > (LEVEL2_OUTER_METERS + EVICTION_MARGIN_METERS) as f32
}

/// Sections that stay resident for a draw list and its fallback parents.
///
/// This is the target, every ancestor of a target, and every parent whose
/// children are still pending. The margin and the double-draw rules are applied
/// by [`evictions`].
fn retained_keys(
    target: &[SectionKey],
    fallbacks: &HashMap<SectionKey, Vec<SectionKey>>,
) -> HashSet<SectionKey> {
    let mut keep: HashSet<SectionKey> = target.iter().copied().collect();
    for key in target {
        keep.extend(ancestors(*key));
    }
    for (parent, children) in fallbacks {
        keep.insert(*parent);
        keep.extend(children.iter().copied());
    }
    keep
}

/// Uploaded sections that must leave the renderer.
///
/// The target and its fallback parents are always kept. A one-ring margin
/// beyond the outer target keeps just-evicted far parents resident so crossing
/// a section cell does not rebuild the same ring. Three rules override the
/// margin so the renderer, which draws every upload, never draws a parent and
/// its children: a parent covered by eight drawn children is evicted, a child
/// of a drawn parent is evicted, and an upload that has fallen into the L0 hole
/// is evicted.
pub(crate) fn evictions(
    uploaded: &HashSet<SectionKey>,
    target: &[SectionKey],
    fallbacks: &HashMap<SectionKey, Vec<SectionKey>>,
    player: Vec3,
    detail_meters: i32,
) -> Vec<SectionKey> {
    let target_set: HashSet<SectionKey> = target.iter().copied().collect();
    let keep = retained_keys(target, fallbacks);

    let mut stale: HashSet<SectionKey> = HashSet::new();
    for key in uploaded {
        let covered = covered_by_children(*key, &target_set);
        let under_drawn_parent = key
            .parent()
            .is_some_and(|parent| target_set.contains(&parent) && !fallbacks.contains_key(&parent));
        let outside = !keep.contains(key) && outside_target_region(*key, player, detail_meters);
        if covered || under_drawn_parent || outside {
            stale.insert(*key);
        }
    }

    let mut stale: Vec<SectionKey> = stale.into_iter().collect();
    stale.sort_unstable_by_key(|key| (key.level, key.coord.x, key.coord.y, key.coord.z));
    stale
}

/// Draw list, build queue, and readiness for hierarchy sections.
#[derive(Debug, Default)]
pub(crate) struct Hierarchy {
    states: HashMap<SectionKey, Readiness>,
    /// Parents whose children are currently preferred.
    refined: HashSet<SectionKey>,
    /// Parent to children that are still waiting, in either direction:
    /// children building under a drawn parent, or a parent rebuilding under
    /// drawn children about to be replaced.
    fallbacks: HashMap<SectionKey, Vec<SectionKey>>,
    /// Exclusive draw list: never a parent and one of its children.
    target: Vec<SectionKey>,
    /// Desired sections that are not ready, parents before their children.
    queue: Vec<SectionKey>,
    /// Sections invalidated by an L0 edit; rebuilt before the plain queue.
    dirty_sections: Vec<SectionKey>,
    /// Section cell of the last rebuild.
    center: Option<IVec2>,
    /// A readiness or eviction change needs a new target.
    dirty: bool,
}

impl Hierarchy {
    /// Rebuilds the draw list when the player crosses a 64 m cell or readiness
    /// changed. Returns whether a rebuild happened.
    pub(crate) fn update(
        &mut self,
        player: Vec3,
        detail_chunks: i32,
        pixels_per_radian: f32,
    ) -> bool {
        let center = cell_at(player, LEVEL1_EXTENT);
        if self.center == Some(center) && !self.dirty {
            return false;
        }
        self.center = Some(center);
        self.dirty = false;
        self.rebuild(player, detail_chunks, pixels_per_radian);
        true
    }

    /// Sections to draw this frame, nearest first.
    pub(crate) fn target(&self) -> &[SectionKey] {
        &self.target
    }

    /// Sections that still need a build, parents before their children.
    pub(crate) fn queue(&self) -> &[SectionKey] {
        &self.queue
    }

    pub(crate) fn state(&self, key: SectionKey) -> Readiness {
        self.states.get(&key).copied().unwrap_or_default()
    }

    pub(crate) fn is_ready(&self, key: SectionKey) -> bool {
        self.state(key) == Readiness::Ready
    }

    pub(crate) fn mark_requested(&mut self, key: SectionKey) {
        self.states.entry(key).or_insert(Readiness::Requested);
    }

    pub(crate) fn mark_building(&mut self, key: SectionKey) {
        if !self.is_ready(key) {
            self.states.insert(key, Readiness::Building);
        }
    }

    /// Records a generated/uploaded section. Readiness gates the parent swap,
    /// so a transition always schedules a new target.
    pub(crate) fn mark_ready(&mut self, key: SectionKey) {
        if self.states.insert(key, Readiness::Ready) != Some(Readiness::Ready) {
            self.dirty = true;
        }
    }

    /// Drops a section that is no longer resident.
    pub(crate) fn forget(&mut self, key: SectionKey) {
        self.states.remove(&key);
        self.dirty = true;
    }

    /// Marks sections whose coarse geometry an L0 edit invalidated.
    ///
    /// Readiness returns to [`Readiness::Requested`] so [`Self::next_build`]
    /// rebuilds the section before the normal queue, whether or not it was
    /// uploaded. A key that is outside the current target is retained until
    /// [`Self::next_build`] discards it; marking far sections is harmless and
    /// they are never uploaded.
    pub(crate) fn mark_dirty(&mut self, keys: impl IntoIterator<Item = SectionKey>) {
        for key in keys {
            self.states.insert(key, Readiness::Requested);
            if !self.dirty_sections.contains(&key) {
                self.dirty_sections.push(key);
            }
        }
    }

    /// Next unready key: dirty sections first, then the normal queue.
    ///
    /// Dirty sections do not consume the queue cursor, so a dirty rebuild never
    /// skips a new target section.
    pub(crate) fn next_build(&mut self, cursor: &mut usize) -> Option<SectionKey> {
        while !self.dirty_sections.is_empty() {
            let key = self.dirty_sections.remove(0);
            if self.target.contains(&key) {
                return Some(key);
            }
        }
        while let Some(key) = self.queue.get(*cursor).copied() {
            *cursor += 1;
            if !self.is_ready(key) {
                return Some(key);
            }
        }
        None
    }

    /// Uploaded sections that should leave the renderer for this target.
    pub(crate) fn evictions(
        &self,
        uploaded: &HashSet<SectionKey>,
        player: Vec3,
        detail_chunks: i32,
    ) -> Vec<SectionKey> {
        evictions(
            uploaded,
            &self.target,
            &self.fallbacks,
            player,
            detail_meters(detail_chunks),
        )
    }

    /// Selects parents and direct level-1 sections, resolves refinement, and
    /// orders the build queue. The target is exclusive: a parent that is
    /// refined and ready contributes only its children.
    fn rebuild(&mut self, player: Vec3, detail_chunks: i32, pixels_per_radian: f32) {
        let detail = detail_meters(detail_chunks);
        let l2_inner = ceil_div(LEVEL1_OUTER_METERS.max(detail), LEVEL2_EXTENT);
        let l2_outer = ceil_div(LEVEL2_OUTER_METERS, LEVEL2_EXTENT);
        let l1_inner = ceil_div(detail, LEVEL1_EXTENT);
        let l1_outer = ceil_div(LEVEL1_OUTER_METERS, LEVEL1_EXTENT);

        let mut parents = Vec::new();
        ring(
            &mut parents,
            player,
            2,
            LEVEL2_EXTENT,
            l2_inner,
            l2_outer,
            1,
        );
        let parent_set: HashSet<SectionKey> = parents.iter().copied().collect();
        self.refined.retain(|key| parent_set.contains(key));

        let mut target = HashSet::new();
        let mut fallbacks = HashMap::new();
        let mut units = Vec::new();
        self.resolve_parents(
            &parents,
            player,
            pixels_per_radian,
            &mut target,
            &mut fallbacks,
            &mut units,
        );

        let mut direct = Vec::new();
        ring(&mut direct, player, 1, LEVEL1_EXTENT, l1_inner, l1_outer, 2);
        add_direct_sections(&direct, &mut target);

        let queue = self.finish_queue(&target, &mut units, player);
        let target = sorted_target(target, player);
        self.prune_states(&target, &fallbacks);
        for key in &queue {
            self.mark_requested(*key);
        }

        self.target = target;
        self.queue = queue;
        self.fallbacks = fallbacks;
    }

    /// Resolves every far parent: refined parents hand over to eight ready
    /// children, otherwise the parent stays selected as the fallback.
    fn resolve_parents(
        &mut self,
        parents: &[SectionKey],
        player: Vec3,
        pixels_per_radian: f32,
        target: &mut HashSet<SectionKey>,
        fallbacks: &mut HashMap<SectionKey, Vec<SectionKey>>,
        units: &mut Vec<(SectionKey, Vec<SectionKey>)>,
    ) {
        for &parent in parents {
            let refined = refine_state(
                self.refined.contains(&parent),
                parent,
                player,
                pixels_per_radian,
            );
            if refined {
                self.refined.insert(parent);
            } else {
                self.refined.remove(&parent);
            }
            let children = children_of(parent);
            if refined && children.iter().all(|child| self.is_ready(*child)) {
                target.extend(children);
                continue;
            }

            if !refined
                && !self.is_ready(parent)
                && children.iter().all(|child| self.is_ready(*child))
            {
                // Collapsing back to a parent that was released while its
                // children were drawn. Keep the children until the parent is
                // uploaded, then swap atomically instead of showing a hole.
                target.extend(children);
                fallbacks.insert(parent, children.to_vec());
                units.push((parent, vec![parent]));
                continue;
            }

            // Until all children are ready the parent is the fallback. It is
            // also the only section selected while it builds.
            target.insert(parent);
            if refined {
                let open: Vec<SectionKey> = children
                    .iter()
                    .copied()
                    .filter(|child| !self.is_ready(*child))
                    .collect();
                let mut keys = Vec::new();
                if !self.is_ready(parent) {
                    keys.push(parent);
                }
                keys.extend(open.iter().copied());
                units.push((parent, keys));
                fallbacks.insert(parent, open);
            } else if !self.is_ready(parent) {
                units.push((parent, vec![parent]));
            }
        }
    }

    /// Adds the unqueued, unready target keys as single-section units, then
    /// sorts units nearest first. A unit keeps its parent ahead of its
    /// children.
    fn finish_queue(
        &self,
        target: &HashSet<SectionKey>,
        units: &mut Vec<(SectionKey, Vec<SectionKey>)>,
        player: Vec3,
    ) -> Vec<SectionKey> {
        let mut queued: HashSet<SectionKey> = HashSet::new();
        for (_, keys) in units.iter() {
            queued.extend(keys.iter().copied());
        }
        for key in target {
            if !self.is_ready(*key) && !queued.contains(key) {
                units.push((*key, vec![*key]));
            }
        }
        units.sort_unstable_by(|(left, _), (right, _)| {
            nearest_xz_distance(player, *left)
                .total_cmp(&nearest_xz_distance(player, *right))
                .then_with(|| coord_order(*left).cmp(&coord_order(*right)))
        });
        units
            .iter()
            .flat_map(|(_, keys)| keys.iter().copied())
            .collect()
    }

    /// Drops request/build state for sections that left the selection. Ready
    /// state survives until eviction so a margin upload is not rebuilt.
    fn prune_states(
        &mut self,
        target: &[SectionKey],
        fallbacks: &HashMap<SectionKey, Vec<SectionKey>>,
    ) {
        let desired: HashSet<SectionKey> = target
            .iter()
            .copied()
            .chain(fallbacks.keys().copied())
            .chain(fallbacks.values().flatten().copied())
            .collect();
        self.states
            .retain(|key, state| *state == Readiness::Ready || desired.contains(key));
    }
}

/// Adds the near level-1 ring, skipping a section whose parent is drawn.
fn add_direct_sections(direct: &[SectionKey], target: &mut HashSet<SectionKey>) {
    for key in direct {
        if key.parent().is_some_and(|parent| target.contains(&parent)) {
            continue;
        }
        target.insert(*key);
    }
}

/// Collects a target into a nearest-first draw list.
fn sorted_target(target: HashSet<SectionKey>, player: Vec3) -> Vec<SectionKey> {
    let mut target: Vec<SectionKey> = target.into_iter().collect();
    target.sort_unstable_by(|left, right| {
        nearest_xz_distance(player, *left)
            .total_cmp(&nearest_xz_distance(player, *right))
            .then_with(|| left.level.cmp(&right.level))
            .then_with(|| coord_order(*left).cmp(&coord_order(*right)))
    });
    target
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAYER: Vec3 = Vec3::new(16.0, 40.0, 16.0);
    const PP_RAD: f32 = DEFAULT_PIXELS_PER_RADIAN;

    fn selected(player: Vec3) -> Hierarchy {
        let mut hierarchy = Hierarchy::default();
        assert!(hierarchy.update(player, DEFAULT_DETAIL_CHUNKS, PP_RAD));
        hierarchy
    }

    /// Level-2 key at a cell offset from the player's `(0, 0)` far cell.
    fn parent_at(offset: IVec2) -> SectionKey {
        SectionKey::new(2, IVec3::new(offset.x, 0, offset.y))
    }

    fn assert_no_double_draw(hierarchy: &Hierarchy) {
        let target: HashSet<SectionKey> = hierarchy.target().iter().copied().collect();
        for key in hierarchy.target() {
            if let Some(parent) = key.parent() {
                assert!(
                    !target.contains(&parent),
                    "{parent:?} and its child {key:?} are both selected"
                );
            }
        }
    }

    #[test]
    fn section_extents_match_the_level_math() {
        assert_eq!(SectionKey::new(1, IVec3::ZERO).extent(), 64);
        assert_eq!(SectionKey::new(2, IVec3::ZERO).extent(), 128);
        assert_eq!(LEVEL1_EXTENT, 64);
        assert_eq!(LEVEL2_EXTENT, 128);
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
                let pixels = parent_pixels(PLAYER, *key, PP_RAD);
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
        for child in children {
            hierarchy.mark_ready(child);
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
    fn negative_coordinates_select_both_levels() {
        let path_player = Vec3::new(-1_200.0, 40.0, -1_200.0);
        let hierarchy = selected(path_player);

        assert!(hierarchy.target().iter().any(|key| key.level == 1));
        assert!(hierarchy.target().iter().any(|key| key.level == 2));
        assert_no_double_draw(&hierarchy);
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
        let children = children_of(parent).to_vec();
        let mut fallbacks = HashMap::new();
        fallbacks.insert(parent, children.clone());
        let keep = retained_keys(&target, &fallbacks);
        assert!(keep.contains(&parent));
        assert!(children.iter().all(|child| keep.contains(child)));
    }

    #[test]
    fn eviction_keeps_the_margin_outside_the_target() {
        let detail = detail_meters(DEFAULT_DETAIL_CHUNKS);
        let target = vec![SectionKey::new(1, IVec3::new(0, 0, 0))];
        let margin = SectionKey::new(2, IVec3::new(17, 0, 0));
        let beyond = SectionKey::new(2, IVec3::new(18, 0, 0));
        let inside_l0 = SectionKey::new(1, IVec3::new(0, 0, 1));
        // A section that merely touches the L0 boundary must not stay drawn
        // over the exact chunks.
        let overlapping_l0 = SectionKey::new(1, IVec3::new(0, 0, 7));
        let uploaded: HashSet<SectionKey> = [margin, beyond, inside_l0, overlapping_l0]
            .into_iter()
            .collect();

        let stale = evictions(&uploaded, &target, &HashMap::new(), Vec3::ZERO, detail);

        assert!(stale.contains(&inside_l0));
        assert!(stale.contains(&overlapping_l0));
        assert!(!stale.contains(&margin), "the margin keeps one ring alive");
        assert!(stale.contains(&beyond));
    }

    #[test]
    fn eviction_drops_a_parent_covered_by_its_children() {
        let detail = detail_meters(DEFAULT_DETAIL_CHUNKS);
        let parent = parent_at(IVec2::new(6, 0));
        let target: Vec<SectionKey> = children_of(parent).to_vec();
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
                        x >= origin.x
                            && x < origin.x + extent
                            && z >= origin.z
                            && z < origin.z + extent
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
}
