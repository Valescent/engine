//! Parent-retained hierarchy selection for the LOD slices.
//!
//! L0 covers [`DEFAULT_DETAIL_CHUNKS`] chunks around the player. Level-1
//! sections (64 m) fill the ring from the detail radius out to 768 m, level-2
//! sections (128 m) cover 768-2,048 m, level-3 sections (256 m) cover
//! 2,048-4,096 m, and level-4 sections (512 m) cover 4,096-8,192 m.
//!
//! Selection is recursive. Level-2 parents, level-3 parents (the 2-4 km band),
//! and level-4 parents (the 4-8 km band) are resolved by the same rule: a node
//! whose projected size exceeds [`REFINE_PIXELS`] is replaced by its in-world
//! children as soon as every one of those children is ready. Until then the
//! parent stays drawn and the children build underneath it. Once a node is
//! refined it stays refined until its projected size drops below
//! [`COLLAPSE_PIXELS`]; the band between the two thresholds is the hysteresis
//! that stops a boundary from thrashing. A selection never contains a parent
//! and any of its descendants together.
//!
//! Level 2 keeps the committed near-field rule and projects its 64 m children.
//! The far roots project their own 256 m or 512 m footprint, so the far leaves
//! are screen-error driven and, at the shipped thresholds, each band gains one
//! level of detail in its inner half. Children whose origin is at or above
//! [`WORLD_HEIGHT`] are empty: they are never requested or built and count as
//! ready for the parent swap. A collapse keeps the drawn descendants until the
//! parent is uploaded, then swaps atomically.
//!
//! Ring boundaries are aligned on the grid of the coarser neighbouring level,
//! so adjacent levels meet exactly instead of overlapping or leaving a seam as
//! the player crosses a cell.
//!
//! The projection uses `extent / distance * pixels_per_radian`, where distance
//! is measured from the camera to the nearest point of the section footprint.
//! The viewport is not threaded through this module yet, so the caller passes
//! [`DEFAULT_PIXELS_PER_RADIAN`] (`900 px/rad`, a 720 px viewport with a 60°
//! vertical field of view).

#![allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]

use std::collections::{HashMap, HashSet};

use glam::{IVec2, IVec3, Vec2, Vec3};

use crate::generation::WORLD_HEIGHT;
use crate::section::{MAX_LEVEL, SECTION_SIZE, SectionKey};

/// Default L0 detail radius in chunks.
pub(crate) const DEFAULT_DETAIL_CHUNKS: i32 = 16;

/// Level-1 outer radius in metres.
const LEVEL1_OUTER_METERS: i32 = 768;
/// Level-2 outer radius in metres.
const LEVEL2_OUTER_METERS: i32 = 2_048;
/// Level-3 outer radius in metres.
const LEVEL3_OUTER_METERS: i32 = 4_096;
/// Level-4 outer radius in metres, the hierarchy's 8 km limit.
const LEVEL4_OUTER_METERS: i32 = 8_192;

const LEVEL1_EXTENT: i32 = SECTION_SIZE * 2;
const LEVEL2_EXTENT: i32 = SECTION_SIZE * 4;
const LEVEL3_EXTENT: i32 = SECTION_SIZE * 8;
const LEVEL4_EXTENT: i32 = SECTION_SIZE * 16;

/// Projected refinement size, in pixels, above which a node refines.
///
/// Level-2 parents project their 64 m children, so the default projection
/// refines parents whose nearest point is closer than about 800 m. Far roots
/// project their own footprint, so the 2 km and 4 km bands each gain a level of
/// detail in their inner half.
pub(crate) const REFINE_PIXELS: f32 = 72.0;

/// Projected refinement size, in pixels, below which refined children collapse
/// back into their parent. The band between this and [`REFINE_PIXELS`] is the
/// hysteresis that keeps a section on one side of the boundary from thrashing.
pub(crate) const COLLAPSE_PIXELS: f32 = 62.0;

/// Projection scale used when the viewport height is not known: roughly a
/// 720 px viewport with a 60° vertical field of view (`720 / (pi / 3)`).
pub(crate) const DEFAULT_PIXELS_PER_RADIAN: f32 = 900.0;

/// How far outside the target an uploaded section stays resident before it is
/// evicted. One level-4 section hides the largest section-sized step as the
/// player crosses a cell.
const EVICTION_MARGIN_METERS: i32 = LEVEL4_EXTENT;

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

/// Extent whose projection decides whether `node` refines.
///
/// Level-2 parents keep the slice-1 rule and project their 64 m children. The
/// far roots project their own footprint: their children belong to the next
/// finer level, so this keeps each 2 km band to one refinement step at the
/// shipped thresholds instead of recursively refining down to level 1.
fn refine_extent(node: SectionKey) -> i32 {
    if node.level >= 3 {
        node.extent()
    } else {
        node.extent() / 2
    }
}

/// Projected refinement size of one node at the camera's nearest distance.
fn refine_pixels(player: Vec3, node: SectionKey, pixels_per_radian: f32) -> f32 {
    projected_pixels(
        refine_extent(node),
        nearest_xz_distance(player, node),
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

/// Whether one coarse node should draw its children at this projected size.
fn refine_state(refined: bool, node: SectionKey, player: Vec3, pixels_per_radian: f32) -> bool {
    refined_for_pixels(refined, refine_pixels(player, node, pixels_per_radian))
}

/// The children of `node` that overlap the world's vertical extent.
///
/// A section whose origin is at or above [`WORLD_HEIGHT`] is entirely empty. It
/// is never selected or built and counts as ready for its parent's swap.
fn in_world_children(node: SectionKey) -> Vec<SectionKey> {
    debug_assert!(node.level >= 2, "only section parents refine");
    (0..8)
        .filter_map(|index| node.child(index))
        .filter(|child| child.origin().y < WORLD_HEIGHT)
        .collect()
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
    (value + divisor - 1).div_euclid(divisor)
}

/// Cell index of the last boundary at or below `value`.
fn floor_cell(value: i32, extent: i32) -> i32 {
    value.div_euclid(extent)
}

/// Cell index of the first boundary at or above `value`.
fn ceil_cell(value: i32, extent: i32) -> i32 {
    ceil_div(value, extent)
}

/// Cell range of one axis for a ring between two world radii.
///
/// The inner hole is covered by the finer level and the outer band by the
/// coarser one. Both edges are snapped to the grid of the level they border so
/// adjacent rings share an edge as the player moves.
#[derive(Clone, Copy, Debug)]
struct AxisRing {
    inner_min: i32,
    inner_max: i32,
    outer_min: i32,
    outer_max: i32,
}

fn axis_ring(
    position: i32,
    extent: i32,
    inner_radius: i32,
    outer_radius: i32,
    outermost: bool,
) -> AxisRing {
    let inner_min = ceil_cell(position - inner_radius, extent);
    let inner_max = floor_cell(position + inner_radius, extent) - 1;

    // Interior boundaries use the coarser neighbour's extent, so the finer and
    // coarser rings meet on a shared cell edge. The outermost level has no
    // neighbour beyond it, so it rounds outward and covers the full radius.
    let (outer_min, outer_max) = if outermost {
        (
            floor_cell(position - outer_radius, extent),
            ceil_cell(position + outer_radius, extent) - 1,
        )
    } else {
        let coarser = extent * 2;
        let cells_per_coarse = coarser / extent;
        (
            ceil_cell(position - outer_radius, coarser) * cells_per_coarse,
            floor_cell(position + outer_radius, coarser) * cells_per_coarse - 1,
        )
    };

    AxisRing {
        inner_min,
        inner_max,
        outer_min,
        outer_max,
    }
}

/// Appends the square ring of sections between `inner_radius` and
/// `outer_radius` world metres around `player`.
#[allow(clippy::too_many_arguments)]
fn ring(
    keys: &mut Vec<SectionKey>,
    player: Vec3,
    level: u8,
    extent: i32,
    inner_radius: i32,
    outer_radius: i32,
    layers: i32,
) {
    let outermost = level == MAX_LEVEL;
    let x = axis_ring(
        player.x.floor() as i32,
        extent,
        inner_radius,
        outer_radius,
        outermost,
    );
    let z = axis_ring(
        player.z.floor() as i32,
        extent,
        inner_radius,
        outer_radius,
        outermost,
    );
    for sx in x.outer_min..=x.outer_max {
        for sz in z.outer_min..=z.outer_max {
            if (x.inner_min..=x.inner_max).contains(&sx)
                && (z.inner_min..=z.inner_max).contains(&sz)
            {
                continue;
            }
            for y in 0..layers {
                keys.push(SectionKey::new(level, IVec3::new(sx, y, sz)));
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

/// Whether a parent is fully covered by drawn descendants.
///
/// A child counts when it is drawn itself or when its own in-world children
/// cover it. This releases a far root after a multi-level refinement even
/// though its leaves are grandchildren.
fn covered_by_children(key: SectionKey, target: &HashSet<SectionKey>) -> bool {
    !target.contains(&key)
        && key.level >= 2
        && in_world_children(key)
            .iter()
            .all(|child| target.contains(child) || covered_by_children(*child, target))
}

/// Whether an upload is outside the target region plus the outer margin.
///
/// The outer margin is deliberately one-sided: a section that has fallen into
/// the L0 hole is evicted immediately, because keeping it would draw coarse
/// geometry over the exact chunks.
fn outside_target_region(key: SectionKey, player: Vec3, detail_meters: i32) -> bool {
    let nearest = nearest_xz_distance(player, key);
    nearest < detail_meters as f32
        || nearest > (LEVEL4_OUTER_METERS + EVICTION_MARGIN_METERS) as f32
}

/// Sections that stay resident for a draw list and its fallback parents.
///
/// This is the target, every ancestor of a target, and every parent whose
/// in-world children are still pending. The margin and the double-draw rules
/// are applied by [`evictions`].
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
/// margin so the renderer never holds a stale parent over its replacement
/// leaves: a parent covered by drawn descendants is evicted, a child of a drawn
/// parent is evicted, and an upload that has fallen into the L0 hole is evicted.
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

/// One rebuild's exclusive draw set, retained fallbacks, and queued units.
#[derive(Debug, Default)]
struct Selection {
    target: HashSet<SectionKey>,
    fallbacks: HashMap<SectionKey, Vec<SectionKey>>,
    units: Vec<(SectionKey, Vec<SectionKey>)>,
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
    /// Exclusive draw list: never a parent and one of its descendants.
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
    ///
    /// Ready sections are retained only while desired. Use
    /// [`Self::update_with_resident`] when the renderer still holds uploaded
    /// sections, so a resident section keeps its readiness across a rebuild.
    #[cfg(test)]
    pub(crate) fn update(
        &mut self,
        player: Vec3,
        detail_chunks: i32,
        pixels_per_radian: f32,
    ) -> bool {
        self.update_with_resident(player, detail_chunks, pixels_per_radian, &HashSet::new())
    }

    /// Rebuilds the draw list, keeping readiness for desired or resident keys.
    pub(crate) fn update_with_resident(
        &mut self,
        player: Vec3,
        detail_chunks: i32,
        pixels_per_radian: f32,
        resident: &HashSet<SectionKey>,
    ) -> bool {
        let center = cell_at(player, LEVEL1_EXTENT);
        if self.center == Some(center) && !self.dirty {
            return false;
        }
        self.center = Some(center);
        self.dirty = false;
        self.rebuild(player, detail_chunks, pixels_per_radian, resident);
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

    /// Selects the level-2/3/4 roots, resolves each recursively, adds the
    /// direct level-1 ring, and orders the build queue. The target is
    /// exclusive: a parent that is refined and ready contributes only its
    /// descendants.
    fn rebuild(
        &mut self,
        player: Vec3,
        detail_chunks: i32,
        pixels_per_radian: f32,
        resident: &HashSet<SectionKey>,
    ) {
        let detail = detail_meters(detail_chunks);
        let l2_inner = LEVEL1_OUTER_METERS.max(detail);

        let mut roots = Vec::new();
        ring(
            &mut roots,
            player,
            2,
            LEVEL2_EXTENT,
            l2_inner,
            LEVEL2_OUTER_METERS,
            1,
        );
        ring(
            &mut roots,
            player,
            3,
            LEVEL3_EXTENT,
            LEVEL2_OUTER_METERS,
            LEVEL3_OUTER_METERS,
            1,
        );
        ring(
            &mut roots,
            player,
            4,
            LEVEL4_EXTENT,
            LEVEL3_OUTER_METERS,
            LEVEL4_OUTER_METERS,
            1,
        );

        let mut selection = Selection::default();
        for root in &roots {
            self.resolve_node(*root, player, pixels_per_radian, &mut selection);
        }

        let mut direct = Vec::new();
        ring(
            &mut direct,
            player,
            1,
            LEVEL1_EXTENT,
            detail,
            LEVEL1_OUTER_METERS,
            2,
        );
        add_direct_sections(&direct, &mut selection.target);

        let queue = self.finish_queue(&selection.target, &mut selection.units, player);
        let target = sorted_target(selection.target, player);
        let fallbacks = selection.fallbacks;
        self.prune_states(&target, &fallbacks, resident);
        self.retain_refined(&target, &fallbacks);
        for key in &queue {
            self.mark_requested(*key);
        }

        self.target = target;
        self.queue = queue;
        self.fallbacks = fallbacks;
    }

    /// Resolves one node and, when it refines, its in-world children.
    ///
    /// A ready refined node contributes only its descendants. While any child
    /// is still missing the node stays drawn and the children build underneath
    /// it. A collapsed node likewise waits for its own upload before replacing
    /// the drawn descendants, so a frame never holds both sides of the swap.
    fn resolve_node(
        &mut self,
        node: SectionKey,
        player: Vec3,
        pixels_per_radian: f32,
        selection: &mut Selection,
    ) {
        if node.level <= 1 {
            selection.target.insert(node);
            return;
        }

        let refined = refine_state(
            self.refined.contains(&node),
            node,
            player,
            pixels_per_radian,
        );
        if refined {
            self.refined.insert(node);
        } else {
            self.refined.remove(&node);
        }
        let children = in_world_children(node);

        if refined {
            if children.iter().all(|child| self.is_ready(*child)) {
                for child in children {
                    self.resolve_node(child, player, pixels_per_radian, selection);
                }
                return;
            }

            // Until all in-world children are ready the parent is the only
            // drawn section. Ready children stay uploaded and hidden.
            selection.target.insert(node);
            let open: Vec<SectionKey> = children
                .iter()
                .copied()
                .filter(|child| !self.is_ready(*child))
                .collect();
            let mut keys = Vec::with_capacity(1 + open.len());
            if !self.is_ready(node) {
                keys.push(node);
            }
            keys.extend(open);
            if !keys.is_empty() {
                selection.units.push((node, keys));
            }
            selection.fallbacks.insert(node, children);
            return;
        }

        if !self.is_ready(node) && children.iter().all(|child| self.is_ready(*child)) {
            // Collapsing back to a parent that was released while its
            // descendants were drawn. Keep covering with them until the parent
            // is uploaded, then swap atomically instead of showing a hole.
            for child in &children {
                self.resolve_node(*child, player, pixels_per_radian, selection);
            }
            selection.fallbacks.insert(node, children);
            selection.units.push((node, vec![node]));
            return;
        }

        // The node is the leaf: it is ready, or nothing below it fully covers
        // the footprint while it builds.
        selection.target.insert(node);
        if !self.is_ready(node) {
            selection.units.push((node, vec![node]));
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

    /// Drops request/build state for sections that left the selection.
    ///
    /// Readiness survives only for desired sections and for uploaded sections
    /// the renderer still holds. An empty section never enters the uploaded
    /// set, so an off-target `Ready` entry here means the section was empty and
    /// can be forgotten without a rebuild.
    fn prune_states(
        &mut self,
        target: &[SectionKey],
        fallbacks: &HashMap<SectionKey, Vec<SectionKey>>,
        resident: &HashSet<SectionKey>,
    ) {
        let desired = desired_keys(target, fallbacks);
        self.states.retain(|key, state| {
            desired.contains(key) || (*state == Readiness::Ready && resident.contains(key))
        });
    }

    /// Drops hysteresis memory for nodes that are no longer selected.
    ///
    /// Refined nodes whose children are drawn are ancestors of target leaves,
    /// so [`retained_keys`] keeps their memory across the swap. A pending
    /// fallback retains its own subtree in the same set.
    fn retain_refined(
        &mut self,
        target: &[SectionKey],
        fallbacks: &HashMap<SectionKey, Vec<SectionKey>>,
    ) {
        let desired = retained_keys(target, fallbacks);
        self.refined.retain(|key| desired.contains(key));
    }
}

/// Keys the current selection draws or keeps resident for a pending swap.
fn desired_keys(
    target: &[SectionKey],
    fallbacks: &HashMap<SectionKey, Vec<SectionKey>>,
) -> HashSet<SectionKey> {
    target
        .iter()
        .copied()
        .chain(fallbacks.keys().copied())
        .chain(fallbacks.values().flatten().copied())
        .collect()
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
mod tests;
