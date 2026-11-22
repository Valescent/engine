//! The per-frame work: tick order, frame delta and timing accumulation,
//! overlay text, resize handling, the mesh/streaming phases, and the demo upload
//! adapter.

use std::time::{Duration, Instant};

use glam::{IVec2, Vec2, Vec3};
use tracing::{debug, error, info, warn};
use winit::dpi::PhysicalSize;
use winit::keyboard::KeyCode;

use crate::demo::DemoRuntime;
use crate::frustum::Frustum;
use crate::mesh::Lod;
use crate::mesher::{MeshKind, MeshPool, MeshQueue, Priority, collect_finished};
use crate::metrics;
use crate::player::{Player, PlayerInput};
use crate::renderer::{FrameStats, Renderer, RendererError, TerrainSection};
use crate::streaming::Streaming;
use crate::voxel::build_highlight_mesh;
use crate::world::{CHUNK_COLUMN, ChunkCoord};
use crate::world_io::{insert_generated_chunk, release_column_tracked, save_directory};

use super::App;
use super::input::axis;

const MAX_FRAME_DELTA: Duration = Duration::from_millis(100);
const FRAME_LOG_INTERVAL: Duration = Duration::from_secs(1);
const MAX_COLUMNS_PER_FRAME: usize = 2;
const MAX_UPLOADS_PER_FRAME: usize = 2;
const LEVEL_WORK_BUDGET: Duration = Duration::from_millis(2);
const UPLOAD_RETRY_WARNING_COUNT: u8 = 8;

/// Column that owns `chunk`.
fn chunk_column(chunk: ChunkCoord) -> IVec2 {
    let coordinates = chunk.coordinates();
    IVec2::new(coordinates.x, coordinates.z)
}

/// LODs other than `active`.
fn inactive_lods(active: Option<Lod>) -> Vec<Lod> {
    [Lod::Lod0, Lod::Lod1, Lod::Lod2]
        .into_iter()
        .filter(|lod| Some(*lod) != active)
        .collect()
}

/// Drops built meshes for LODs the renderer no longer draws.
fn forget_inactive_lods(queue: &mut MeshQueue, renderer: &Renderer, chunk: ChunkCoord) {
    let Some(active) = renderer.active_lod(chunk) else {
        return;
    };
    for lod in inactive_lods(Some(active)) {
        if !renderer.has_lod(chunk, lod) {
            queue.forget_built(chunk, lod, MeshKind::Opaque);
        }
    }
}

/// Uploads the demo's active section set through the renderer's packed-quad path.
pub(super) fn upload_terrain_demo(
    renderer: &mut Renderer,
    demo: &DemoRuntime,
) -> Result<(), RendererError> {
    let sections: Vec<TerrainSection<'_>> = demo
        .sections()
        .iter()
        .map(|section| TerrainSection {
            origin: section.origin,
            scale: section.scale,
            opaque: &section.mesh.opaque,
            cutout: &section.mesh.cutout,
            water: &section.mesh.water,
        })
        .collect();
    renderer.submit_terrain(&sections)
}

/// Per-second accumulation of the v0.0.5 metrics.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct FrameAccumulator {
    pub(crate) totals: FrameStats,
    pub(crate) rebuild_ms: f64,
    pub(crate) upload_ms: f64,
    pub(crate) last_rebuild_ms: f64,
    pub(crate) last_upload_ms: f64,
    pub(crate) pending_jobs: usize,
    pub(crate) generated_columns: u32,
    pub(crate) generation_ms: f64,
}

impl FrameAccumulator {
    fn reset(&mut self) {
        *self = Self::default();
    }
}

impl App {
    /// Recalculates every chunk's LOD and requests any missing mesh.
    ///
    /// Only chunks with outstanding level work are visited. Generation commits,
    /// edits, and a player chunk change feed the set; a chunk stays in it until
    /// its light, opaque mesh, and water mesh are built.
    pub(crate) fn request_levels(&mut self, pool: &dyn MeshPool) {
        profiling::scope!("mesh request");
        let feet = self.player.position.floor().as_ivec3();
        let player_chunk = ChunkCoord::from_world(feet).0;
        if player_chunk != self.level_player_chunk {
            self.level_player_chunk = player_chunk;
            self.level_work
                .extend(self.world.iter_chunks().map(|(chunk, _)| chunk));
            self.admit_deferred_light();
        }

        let started = Instant::now();
        while let Some(chunk) = self.level_work.pop_first() {
            let chunk_started = Instant::now();
            if let Some(data) = self.world.chunk(chunk) {
                let has_water = data.has_water();
                let mut retry = false;
                if self.queue.light_is_current(chunk) {
                    let lod = self.queue.desired_lod(chunk, player_chunk);
                    if !self.queue.is_built(chunk, lod, MeshKind::Opaque)
                        && !self.queue.is_pending(chunk, lod, MeshKind::Opaque)
                    {
                        self.queue.request(
                            pool,
                            &self.world,
                            chunk,
                            lod,
                            MeshKind::Opaque,
                            Priority::Background,
                        );
                        retry = !self.queue.is_pending(chunk, lod, MeshKind::Opaque);
                    }

                    if has_water
                        && !self.queue.is_built(chunk, Lod::Lod0, MeshKind::Water)
                        && !self.queue.is_pending(chunk, Lod::Lod0, MeshKind::Water)
                    {
                        self.queue.request(
                            pool,
                            &self.world,
                            chunk,
                            Lod::Lod0,
                            MeshKind::Water,
                            Priority::Background,
                        );
                        retry = retry || !self.queue.is_pending(chunk, Lod::Lod0, MeshKind::Water);
                    }
                } else if !self.queue.light_pending(chunk) {
                    if self.light_neighbourhood_stable(chunk_column(chunk)) {
                        self.queue
                            .request_light(pool, &self.world, chunk, Priority::Background);
                        retry = !self.queue.light_pending(chunk);
                    } else {
                        self.deferred_light.insert(chunk);
                    }
                }
                if retry {
                    self.level_work.insert(chunk);
                }
            }
            let took = chunk_started.elapsed();
            if took.as_millis() >= 2 {
                debug!(
                    chunk = ?chunk,
                    ms = took.as_secs_f64() * 1000.0,
                    "slow level request"
                );
            }
            if started.elapsed() >= LEVEL_WORK_BUDGET {
                break;
            }
        }
        self.stats.pending_jobs = self.queue.pending_len();
    }

    /// Whether every adjacent column is loaded or will never load.
    fn light_neighbourhood_stable(&self, column: IVec2) -> bool {
        let Some(streaming) = &self.streaming else {
            return true;
        };
        for dx in -1..=1 {
            for dz in -1..=1 {
                if dx == 0 && dz == 0 {
                    continue;
                }
                let neighbour = column + IVec2::new(dx, dz);
                if !streaming.is_loaded(neighbour) && self.column_is_wanted(neighbour) {
                    return false;
                }
            }
        }
        true
    }

    /// Moves deferred light work back into the level pass once it can be final.
    fn admit_deferred_light(&mut self) {
        if self.deferred_light.is_empty() {
            return;
        }
        let ready: Vec<ChunkCoord> = self
            .deferred_light
            .iter()
            .copied()
            .filter(|chunk| self.light_neighbourhood_stable(chunk_column(*chunk)))
            .collect();
        for chunk in &ready {
            self.deferred_light.remove(chunk);
        }
        self.level_work.extend(ready);
    }

    /// Whether the streamer may still generate this column for the player.
    fn column_is_wanted(&self, column: IVec2) -> bool {
        let Some(streaming) = &self.streaming else {
            return false;
        };
        let player_column = Streaming::column_at(self.player.position);
        let delta = (column - player_column).abs();
        delta.x.max(delta.y) <= streaming.render_distance()
    }

    pub(crate) fn upload_finished_meshes(&mut self, pool: &dyn MeshPool) {
        profiling::scope!("mesh upload");
        let finished = collect_finished(&mut self.queue, pool, MAX_UPLOADS_PER_FRAME);
        let rebuild_ms = finished.worker_time.as_secs_f64() * 1000.0;
        self.stats.rebuild_ms += rebuild_ms;
        self.stats.last_rebuild_ms = rebuild_ms;

        for light in finished.lights {
            self.level_work.insert(light.chunk);
            self.world.set_light_field(light.chunk, light.light);
        }

        let chunk_directory = save_directory(self.stress);
        for generation in finished.generations {
            self.stats.generated_columns += 1;
            self.stats.generation_ms += generation.elapsed.as_secs_f64() * 1000.0;
            let mut loaded = true;
            for (chunk, blocks) in generation.chunks {
                let saved = if let Some(directory) = chunk_directory {
                    insert_generated_chunk(&mut self.world, directory, chunk, blocks)
                } else {
                    self.world.insert_chunk(chunk, blocks);
                    Ok(())
                };
                if let Err(error) = saved {
                    error!(%error, chunk = ?chunk, "failed to load a saved chunk");
                    loaded = false;
                }
                self.level_work.insert(chunk);
            }
            if loaded && let Some(streaming) = &mut self.streaming {
                streaming.mark_loaded(generation.column);
            }
        }

        let Some(renderer) = &mut self.renderer else {
            return;
        };
        let mut upload_ms = 0.0;
        let mut outcomes = Vec::with_capacity(finished.meshes.len());
        for result in &finished.meshes {
            let started = Instant::now();
            let uploaded = match result.kind {
                MeshKind::Opaque => {
                    let uploaded = renderer.upload_chunk(
                        result.chunk,
                        result.lod,
                        result.revision,
                        &result.mesh,
                    );
                    if uploaded {
                        let desired = self.queue.desired(result.chunk);
                        let target = desired
                            .filter(|&lod| renderer.has_lod(result.chunk, lod))
                            .unwrap_or(result.lod);
                        renderer.select_active(result.chunk, target);
                        forget_inactive_lods(&mut self.queue, renderer, result.chunk);
                    }
                    uploaded
                }
                MeshKind::Water => {
                    renderer.upload_water_chunk(result.chunk, result.revision, &result.mesh)
                }
            };
            outcomes.push(uploaded);
            upload_ms += started.elapsed().as_secs_f64() * 1000.0;
        }
        self.stats.upload_ms += upload_ms;
        self.stats.last_upload_ms = upload_ms;
        for (result, uploaded) in finished.meshes.iter().zip(outcomes) {
            self.record_mesh_upload(result, uploaded);
        }
        for result in &finished.sections {
            self.upload_section_result(result);
        }
    }

    /// Updates the per-chunk retry state for one mesh upload attempt.
    fn record_mesh_upload(&mut self, result: &crate::mesher::MeshResult, uploaded: bool) -> bool {
        if uploaded {
            self.upload_failures.remove(&result.chunk);
            self.level_work.insert(result.chunk);
            return true;
        }
        self.queue
            .forget_built(result.chunk, result.lod, result.kind);
        let count = self.upload_failures.entry(result.chunk).or_insert(0);
        *count = count.saturating_add(1);
        let count = *count;
        if count == UPLOAD_RETRY_WARNING_COUNT {
            warn!(
                chunk = ?result.chunk,
                count,
                "mesh upload keeps failing; retrying when pool or slot space frees"
            );
        }
        self.level_work.insert(result.chunk);
        false
    }

    fn select_active_levels(&mut self) {
        let Some(renderer) = &mut self.renderer else {
            return;
        };
        for (chunk, _) in self.world.iter_chunks() {
            if let Some(lod) = self.queue.desired(chunk) {
                renderer.select_active(chunk, lod);
            }
        }
    }

    /// Advances the demo one frame and applies its player and renderer results.
    ///
    /// The runtime owns the ring, the rebuild, and the throttles; the app places
    /// the spawned player and uploads the section set it reports.
    fn tick_demo(&mut self, player_column: (i32, i32)) {
        let focused = self.focused;
        let frame = self
            .demo
            .as_mut()
            .map(|demo| demo.tick(Instant::now(), player_column, focused))
            .unwrap_or_default();
        if let Some(spawn) = frame.spawn {
            self.set_spawn(spawn);
            info!(?spawn, "standing on the centre terrain column");
        }
        let upload_started = Instant::now();
        if frame.upload {
            self.upload_demo();
        }
        let upload_ms = upload_started.elapsed().as_secs_f64() * 1000.0;
        if let Some(rebuild) = frame.rebuild {
            debug!(
                light_ms = rebuild.light_ms,
                mesh_ms = rebuild.mesh_ms,
                upload_ms,
                patches = rebuild.patches,
                "terrain rebuild applied"
            );
        }
    }

    /// Rebuilds the renderer's demo buffers.
    pub(super) fn upload_demo(&mut self) {
        let Some(demo) = self.demo.as_ref() else {
            return;
        };
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        if let Err(error) = upload_terrain_demo(renderer, demo) {
            error!(%error, "terrain upload failed");
        }
    }
    fn update_highlight(&mut self) {
        if self.renderer.is_none() {
            return;
        }
        profiling::scope!("highlight");
        let hit = self.target_hit().map(|hit| hit.block);
        if self.highlighted_block == hit {
            return;
        }
        self.highlighted_block = hit;
        let mesh = hit.map_or_else(Vec::new, build_highlight_mesh);
        if let Some(renderer) = &mut self.renderer {
            renderer.set_highlight(&mesh);
        }
    }

    pub(super) fn resize(&mut self, size: PhysicalSize<u32>) {
        self.camera.set_aspect(size.width, size.height);
        if let Some(renderer) = &mut self.renderer {
            renderer.resize(size, &self.camera);
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    #[allow(clippy::cast_precision_loss)]
    /// Builds this frame's movement input and advances the player against
    /// whichever block source is live: the demo patch, or the legacy world.
    fn step_player(&mut self, delta_seconds: f32) {
        let forward_axis = axis(
            self.pressed.contains(&KeyCode::KeyW),
            self.pressed.contains(&KeyCode::KeyS),
        );
        let right_axis = axis(
            self.pressed.contains(&KeyCode::KeyD),
            self.pressed.contains(&KeyCode::KeyA),
        );
        let jump_pressed = std::mem::take(&mut self.jump_requested);
        let input = PlayerInput {
            forward: forward_axis,
            right: right_axis,
            jump: if self.player.flying {
                self.pressed.contains(&KeyCode::Space)
            } else {
                jump_pressed
            },
            descend: self.player.flying && self.modifiers.control_key(),
            yaw: self.camera.yaw(),
        };
        profiling::scope!("player update");
        let streaming = self.streaming.as_ref();
        let demo = self.demo.as_ref();
        let ready = |position: Vec3| match demo {
            Some(demo) => demo.is_ready(position),
            None => streaming.is_none_or(|streaming| streaming.aabb_ready(position)),
        };
        let source: &dyn crate::player::BlockSource = match &self.demo {
            Some(demo) => demo,
            None => &self.world,
        };
        self.player
            .update_guarded(source, input, delta_seconds, &ready);
    }

    #[allow(clippy::cast_precision_loss, clippy::too_many_lines)]
    pub(super) fn redraw(&mut self) {
        if self.occluded {
            return;
        }

        profiling::scope!("frame");

        let now = Instant::now();
        let elapsed = now.saturating_duration_since(self.last_frame);
        self.last_frame = now;

        if self.focused {
            self.record_frame(elapsed);
        }
        let player = Streaming::column_at(self.player.position);
        let player_column = (player.x, player.y);
        self.tick_demo(player_column);
        if self.focused {
            let delta_seconds = elapsed.min(MAX_FRAME_DELTA).as_secs_f32();
            self.step_player(delta_seconds);
            self.sky.advance(delta_seconds);
            self.camera.set_position(self.player.eye_position());
            self.update_highlight();
        }

        let slow0 = Instant::now();
        if let Some(pool) = self.mesher.clone() {
            self.stream_columns(&*pool);
            let slow1 = Instant::now();
            self.request_levels(&*pool);
            let slow2 = Instant::now();
            self.upload_finished_meshes(&*pool);
            let slow3 = Instant::now();
            profiling::scope!("select active");
            self.select_active_levels();
            let slow4 = Instant::now();
            if slow4.saturating_duration_since(slow0).as_millis() > 5 {
                let ms =
                    |a: Instant, b: Instant| b.saturating_duration_since(a).as_secs_f64() * 1000.0;
                debug!(
                    stream = ms(slow0, slow1),
                    levels = ms(slow1, slow2),
                    upload = ms(slow2, slow3),
                    select = ms(slow3, slow4),
                    "slow phases"
                );
            }
        }

        if self.hierarchy {
            let pool = self.mesher.clone();
            self.ensure_hierarchy(pool.as_deref());
        }

        let player_column = Streaming::column_at(self.player.position);
        let player_column = (player_column.x, player_column.y);
        if self
            .demo
            .as_mut()
            .is_some_and(|demo| demo.advance_lod(player_column))
        {
            self.upload_demo();
        }
        let overlay = if let Some(key) = self.demo.as_ref().and_then(DemoRuntime::repair_key) {
            Some(format!(
                "REPAIR c.{}.{}.{}: R RESTORE  Q QUARANTINE  S SALVAGE  (OR CLICK LEFT/MID/RIGHT)",
                key.column[0], key.column[1], key.section
            ))
        } else {
            self.show_overlay.then(|| {
                format!(
                    "FPS {:.0}  {:.2} MS",
                    self.overlay_fps, self.overlay_frame_ms
                )
            })
        };
        // One render origin for the frame: the same value builds the frustum
        // and rebases every submitted section/chunk origin.
        let view = self.camera.render_view();
        let frustum = {
            profiling::scope!("frustum");
            Frustum::from_view_projection(view.view_projection)
        };
        if self.horizon_cull && self.horizon_at.elapsed() >= Duration::from_millis(250) {
            self.rebuild_horizon();
        }
        let slow_render = Instant::now();
        if let Some(renderer) = &mut self.renderer {
            renderer.set_camera(&view);
            renderer.set_sky(&self.sky.sample(), self.unlit);
            renderer.set_overlay(overlay.as_deref());
            self.stats.totals = renderer.render(&frustum, &self.horizon);
        }
        if slow_render.elapsed().as_millis() > 5 {
            debug!(
                render_ms = slow_render.elapsed().as_secs_f64() * 1000.0,
                "slow render"
            );
        }

        metrics::frame_ms(elapsed.as_secs_f64() * 1000.0);
        metrics::visible_chunks(f64::from(self.stats.totals.visible_chunks));
        metrics::draw_calls(f64::from(self.stats.totals.draw_calls));
        metrics::visible_vertices(self.stats.totals.visible_vertices as f64);
        metrics::geometry_bytes(self.stats.totals.geometry_bytes as f64);
        metrics::visible_geometry_bytes(self.stats.totals.visible_bytes as f64);
        metrics::rebuild_ms(self.stats.last_rebuild_ms);
        metrics::upload_ms(self.stats.last_upload_ms);
        metrics::mesh_jobs(self.stats.pending_jobs as f64);
        metrics::generated_columns(f64::from(self.stats.generated_columns));
        metrics::generation_ms(self.stats.generation_ms);
        metrics::terrain_sections_uploaded(f64::from(self.stats.totals.terrain_sections_uploaded));
        profiling::finish_frame!();

        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn record_frame(&mut self, delta: Duration) {
        self.frame_accum += delta;
        self.frame_count += 1;
        if self.frame_accum >= FRAME_LOG_INTERVAL {
            let seconds = self.frame_accum.as_secs_f64();
            let frames = f64::from(self.frame_count);
            debug!(
                fps = frames / seconds,
                frame_ms = seconds * 1000.0 / frames,
                visible_chunks = self.stats.totals.visible_chunks,
                total_chunks = self.stats.totals.total_chunks,
                visible_vertices = self.stats.totals.visible_vertices,
                draw_calls = self.stats.totals.draw_calls,
                rebuild_ms = self.stats.rebuild_ms,
                upload_ms = self.stats.upload_ms,
                generated_columns = self.stats.generated_columns,
                generation_ms = self.stats.generation_ms,
                gpu_ms = self.stats.totals.gpu_ms,
                player = ?self.player.position,
                on_ground = self.player.on_ground,
                "frame timing"
            );
            if let Some(demo) = &mut self.demo {
                let stats = demo.stats();
                info!(
                    wanted = stats.wanted,
                    resident = stats.resident,
                    ready = stats.ready,
                    fine = stats.fine,
                    coarse = stats.coarse,
                    lod2 = stats.lod2,
                    quads = stats.quads,
                    in_flight = stats.in_flight,
                    bytes = stats.bytes,
                    "terrain ring"
                );
                if stats.complete {
                    info!(
                        wanted = stats.wanted,
                        bytes = stats.bytes,
                        "terrain ring complete"
                    );
                }
            }
            self.overlay_fps = frames / seconds;
            self.overlay_frame_ms = seconds * 1000.0 / frames;
            self.frame_accum = Duration::ZERO;
            self.frame_count = 0;
            self.stats.reset();
        }
    }

    #[allow(clippy::cast_precision_loss)]
    pub(crate) fn apply_render_distance(&mut self) {
        let distance = self.config.render_distance_chunks as f32 * 32.0;
        if self.hierarchy {
            self.camera.set_far_plane(9_216.0);
            self.camera.set_fog_end(9_200.0);
        } else {
            self.camera.set_far_plane(distance + 64.0);
            self.camera.set_fog_end(distance);
        }
    }

    /// Whether all four chunks of a column are lit and meshed.
    ///
    /// A chunk with no stored blocks is empty air and needs no light or mesh.
    fn column_ready(&self, column: IVec2) -> bool {
        (0..CHUNK_COLUMN).all(|y| {
            let chunk = ChunkCoord::new(column.x, y, column.y);
            self.world.chunk(chunk).is_none()
                || (self.queue.light_is_current(chunk)
                    && (self.queue.is_built(chunk, Lod::Lod0, MeshKind::Opaque)
                        || self.queue.is_built(chunk, Lod::Lod1, MeshKind::Opaque)
                        || self.queue.is_built(chunk, Lod::Lod2, MeshKind::Opaque)))
        })
    }

    /// Marks ready columns, unloads distant ones, and requests the nearest missing.
    ///
    /// Both the wanted and unloadable lists are rebuilt only when the player
    /// crosses a column, so a settled frame does not rescan the world.
    pub(crate) fn stream_columns(&mut self, pool: &dyn MeshPool) {
        profiling::scope!("stream columns");
        let Some(seed) = self.seed else {
            return;
        };
        let player_column = Streaming::column_at(self.player.position);

        if self.wanted_player != Some(player_column) {
            let forward = self.camera.forward();
            let forward = Vec2::new(forward.x, forward.z).normalize_or_zero();
            let (wanted, unload) = self
                .streaming
                .as_ref()
                .map(|streaming| {
                    (
                        streaming.wanted(self.player.position, forward),
                        streaming.unloadable(player_column),
                    )
                })
                .unwrap_or_default();
            self.wanted_columns = wanted;
            self.unload_columns = unload;
            self.wanted_cursor = 0;
            self.wanted_player = Some(player_column);
        }

        let loaded: Vec<IVec2> = self
            .streaming
            .as_ref()
            .map(|streaming| streaming.loaded().collect())
            .unwrap_or_default();
        for column in loaded {
            if self.column_ready(column)
                && let Some(streaming) = &mut self.streaming
            {
                streaming.mark_ready(column);
            }
        }

        let mut failed_unload = false;
        for column in std::mem::take(&mut self.unload_columns) {
            if !self.unload_column(column) {
                self.unload_columns.push(column);
                failed_unload = true;
            }
        }
        if failed_unload {
            // Rebuild both lists next frame and retry the save.
            self.wanted_player = None;
        }

        let mut requested = 0;
        while requested < MAX_COLUMNS_PER_FRAME && self.wanted_cursor < self.wanted_columns.len() {
            let column = self.wanted_columns[self.wanted_cursor];
            self.wanted_cursor += 1;
            let loaded = self
                .streaming
                .as_ref()
                .is_some_and(|streaming| streaming.is_loaded(column));
            if loaded || self.queue.generation_pending(column) {
                continue;
            }
            self.queue
                .request_generation(pool, seed, column, Priority::Background);
            if self.queue.generation_pending(column) {
                requested += 1;
            } else {
                // The pool is behind; keep this candidate for the next frame.
                self.wanted_cursor -= 1;
                break;
            }
        }
    }

    /// Saves and drops one column that left the unload margin.
    ///
    /// Returns whether the column was released.
    fn unload_column(&mut self, column: IVec2) -> bool {
        let Some(directory) = self.save_directory.clone() else {
            return false;
        };
        self.invalidate_neighbour_columns(column);
        if let Err(error) = release_column_tracked(
            &mut self.world,
            &mut self.queue,
            &directory,
            &mut self.saved_chunks,
            column,
        ) {
            error!(%error, ?column, "failed to save an unloaded chunk");
            return false;
        }
        if let Some(renderer) = &mut self.renderer {
            for y in 0..CHUNK_COLUMN {
                renderer.remove_chunk(ChunkCoord::new(column.x, y, column.y));
            }
        }
        for y in 0..CHUNK_COLUMN {
            let chunk = ChunkCoord::new(column.x, y, column.y);
            self.level_work.remove(&chunk);
            self.deferred_light.remove(&chunk);
            self.upload_failures.remove(&chunk);
        }
        if let Some(streaming) = &mut self.streaming {
            streaming.mark_unloaded(column);
        }
        debug!(?column, "unloaded column");
        true
    }

    /// Invalidates the meshes of stored columns adjacent to an unloaded one.
    fn invalidate_neighbour_columns(&mut self, column: IVec2) {
        let neighbours = self.world.neighbour_column_chunks(column);
        self.queue.invalidate(&neighbours);
        self.level_work.extend(neighbours.iter().copied());
    }

    pub(crate) fn set_spawn(&mut self, position: Vec3) {
        self.player = Player::new(position);
        self.camera.set_position(self.player.eye_position());
    }

    pub(super) fn toggle_overlay(&mut self) {
        self.show_overlay = !self.show_overlay;
        debug!(show_overlay = self.show_overlay, "debug overlay toggled");
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::Duration;

    use glam::IVec3;

    use crate::block::Block;
    use crate::generation::GenResult;
    use crate::mesh;
    use crate::mesher::{MeshQueue, MeshResult, SectionResult};
    use crate::residency;
    use crate::section::SectionKey;
    use crate::world::World;

    use super::super::tests::{InlinePool, demo_world, temp_directory};
    use super::*;

    #[test]
    fn overlay_values_update_once_per_second() {
        let mut app = App::new(demo_world());
        assert_eq!(app.overlay_fps, 0.0);

        app.record_frame(Duration::from_millis(400));
        assert_eq!(app.overlay_fps, 0.0);

        app.record_frame(Duration::from_millis(600));
        assert!((app.overlay_fps - 2.0).abs() < 1e-5);
        assert!((app.overlay_frame_ms - 500.0).abs() < 1e-5);
    }

    #[test]
    fn a_late_generation_result_is_discarded_after_unload() {
        let pool = InlinePool::default();
        let column = IVec2::new(2, 0);
        let result = GenResult {
            column,
            chunks: Vec::new(),
            elapsed: Duration::ZERO,
        };
        let mut queue = MeshQueue::default();

        queue.request_generation(&pool, 7, column, Priority::Background);
        assert!(queue.accept_generation(&result));

        queue.request_generation(&pool, 7, column, Priority::Background);
        queue.cancel_generation(column);
        assert!(!queue.accept_generation(&result));
    }

    #[test]
    fn the_render_distance_sets_the_far_plane_and_fog_range() {
        let mut app = App::new(demo_world());
        app.config.render_distance_chunks = 12;

        app.apply_render_distance();

        assert_eq!(app.camera.far_plane(), 448.0);
        assert_eq!(app.camera.fog_end(), 384.0);
    }

    #[test]
    #[allow(clippy::cast_precision_loss)]
    fn the_ring_radius_never_exceeds_the_camera_visible_radius() {
        let mut app = App::new(demo_world());
        for render_distance in [8, 12, 16, 24, 32] {
            app.config.render_distance_chunks = render_distance;
            app.apply_render_distance();

            let radius = residency::demo_view_radius(render_distance);
            let ring_extent = radius as f32 * 32.0;
            assert!(
                ring_extent <= app.camera.fog_end(),
                "radius {radius} ends at {ring_extent} blocks, past fog end {}",
                app.camera.fog_end()
            );
            assert!(
                ring_extent <= app.camera.far_plane(),
                "radius {radius} ends at {ring_extent} blocks, past far plane {}",
                app.camera.far_plane()
            );
        }

        // The default selects the 625-column ring; the spec distance selects
        // the 1,089-column cap.
        assert_eq!(residency::demo_view_radius(12), 12);
        assert_eq!(residency::demo_view_radius(16), residency::DEMO_VIEW_RADIUS);
    }

    #[test]
    fn a_column_built_at_lod2_is_ready() {
        let mut app = App::with_world(World::default(), false);
        let chunk = ChunkCoord::new(0, 0, 0);
        app.world.set_block(IVec3::new(0, 0, 0), Block::Stone);
        app.queue.mark_light_built(chunk);
        let pool = InlinePool::default();
        app.queue.request(
            &pool,
            &app.world,
            chunk,
            Lod::Lod2,
            MeshKind::Opaque,
            Priority::Background,
        );
        let revision = app.queue.revision(chunk);

        assert!(app.queue.accept(&MeshResult {
            chunk,
            lod: Lod::Lod2,
            kind: MeshKind::Opaque,
            revision,
            mesh: mesh::MeshData::default(),
            elapsed: Duration::ZERO,
        }));
        assert!(app.column_ready(IVec2::ZERO));
    }

    #[test]
    fn an_all_air_column_is_ready() {
        let app = App::with_world(World::default(), false);

        assert!(app.column_ready(IVec2::new(3, -2)));
    }

    #[test]
    fn absent_chunks_are_ignored_but_stored_chunks_must_be_lit() {
        let mut app = App::with_world(World::default(), false);
        app.world.set_block(IVec3::new(0, 0, 0), Block::Stone);

        assert!(!app.column_ready(IVec2::ZERO));

        app.queue.mark_light_built(ChunkCoord::new(0, 0, 0));

        assert!(!app.column_ready(IVec2::ZERO));
    }

    #[test]
    fn streaming_unloads_a_distant_column_after_saving_it() {
        let directory = temp_directory("unload-stream");
        let chunk = ChunkCoord::new(0, 0, 0);
        let coordinate = IVec3::new(5, 0, 5);
        let mut app = App::with_world(World::default(), false);
        app.save_directory = Some(directory.clone());
        app.seed = Some(42);
        app.world.set_block(coordinate, Block::Stone);
        let mut streaming = Streaming::new(8);
        streaming.mark_loaded(IVec2::ZERO);
        streaming.mark_ready(IVec2::ZERO);
        app.streaming = Some(streaming);
        app.player.position = Vec3::new(60.0 * 32.0, 40.0, 0.5);

        app.stream_columns(&InlinePool::default());

        assert!(app.world.chunk(chunk).is_none());
        let saved = World::load_saved_chunk(&directory, chunk).unwrap().unwrap();
        assert_eq!(saved.block(coordinate), Block::Stone);
        assert!(!app.streaming.as_ref().unwrap().is_ready(IVec2::ZERO));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_stale_section_result_is_dropped() {
        let mut app = App::with_world(World::default(), false);
        let key = SectionKey::new(1, IVec3::ZERO);
        app.section_jobs.insert(key, 7);
        app.sections_in_flight.insert(key);
        app.sections.mark_building(key);

        let stale = SectionResult {
            key,
            job: 6,
            opaque: mesh::MeshData::default(),
            water: mesh::MeshData::default(),
            bounds: None,
            elapsed: Duration::ZERO,
        };
        assert!(!app.upload_section_result(&stale));
        assert_ne!(app.sections.state(key), crate::hierarchy::Readiness::Ready);
        assert!(app.sections_in_flight.contains(&key));

        let fresh = SectionResult { job: 7, ..stale };
        assert!(app.upload_section_result(&fresh));
        assert_eq!(app.sections.state(key), crate::hierarchy::Readiness::Ready);
        assert!(!app.sections_in_flight.contains(&key));
    }

    #[test]
    fn inactive_lods_exclude_only_the_active_level() {
        assert_eq!(inactive_lods(Some(Lod::Lod0)), vec![Lod::Lod1, Lod::Lod2]);
        assert_eq!(inactive_lods(Some(Lod::Lod1)), vec![Lod::Lod0, Lod::Lod2]);
        assert_eq!(inactive_lods(Some(Lod::Lod2)), vec![Lod::Lod0, Lod::Lod1]);
        assert_eq!(inactive_lods(None), vec![Lod::Lod0, Lod::Lod1, Lod::Lod2]);
    }
}
