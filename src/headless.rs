//! Windowless run of the normal generation, light, and mesh paths.
//!
//! A headless run builds a world exactly like `run`, drives the streaming and
//! level system at a fixed step without a window or GPU, and prints one
//! machine-readable summary line. It is the CI performance gate and a quick
//! way to test load behaviour.

use std::time::{Duration, Instant};

use glam::Vec2;
use tracing::info;

use crate::App;
use crate::mesher::{MeshPool, create_pool};
use crate::streaming::Streaming;

/// Simulated frame step in seconds.
pub(crate) const FIXED_STEP: f32 = 1.0 / 60.0;

/// Default measured frames when `ENGINE_HEADLESS_FRAMES` is unset.
const DEFAULT_FRAMES: u32 = 600;

/// Wall-clock budget for the settle phase.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(45);

/// Frames and camera movement for one headless run.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct HeadlessOptions {
    pub(crate) frames: u32,
    pub(crate) speed: f32,
}

/// Reads the headless environment, or `None` for a normal windowed run.
pub(crate) fn options() -> Option<HeadlessOptions> {
    if !std::env::var("ENGINE_HEADLESS").is_ok_and(|value| value == "1") {
        return None;
    }
    let frames = std::env::var("ENGINE_HEADLESS_FRAMES")
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .filter(|frames| *frames > 0)
        .unwrap_or(DEFAULT_FRAMES);
    let speed = std::env::var("ENGINE_HEADLESS_SPEED")
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(0.0);
    Some(HeadlessOptions { frames, speed })
}

/// Runs `frames` fixed steps, settles outstanding work, and prints the summary.
pub(crate) fn run(mut app: App, options: HeadlessOptions) {
    let pool = create_pool();
    app.mesher = Some(pool.clone());
    app.request_initial_meshes();

    let mut samples = Vec::with_capacity(usize::try_from(options.frames).unwrap_or(0));
    let step = Duration::from_secs_f32(FIXED_STEP);
    for _ in 0..options.frames {
        let started = Instant::now();
        advance(&mut app, &*pool, options.speed);
        samples.push(started.elapsed());
        if let Some(remaining) = (started + step).checked_duration_since(Instant::now()) {
            std::thread::sleep(remaining);
        }
    }
    let settle_frames = settle(&mut app, &*pool);

    info!(
        frames = options.frames,
        speed = options.speed,
        "headless run complete"
    );
    println!("{}", summary(&app, &samples, options, settle_frames));
}

/// One fixed step of streaming and level work.
fn advance(app: &mut App, pool: &dyn MeshPool, speed: f32) {
    if speed != 0.0 {
        app.player.position.x += speed * FIXED_STEP;
        app.camera.set_position(app.player.eye_position());
    }
    app.stream_columns(pool);
    app.request_levels(pool);
    app.upload_finished_meshes(pool);
}

/// Runs extra steps until no work is pending and the loaded set stops changing.
///
/// The player stays put while settling. Settling is capped by wall time, not frame
/// count: each step is microseconds of work, and the pool needs real time to finish
/// the queued columns.
fn settle(app: &mut App, pool: &dyn MeshPool) -> u32 {
    let deadline = Instant::now() + SETTLE_TIMEOUT;
    let mut last_loaded = usize::MAX;
    let mut frame = 0;
    while Instant::now() < deadline {
        advance(app, pool, 0.0);
        let loaded = app.streaming.as_ref().map_or(0, Streaming::loaded_count);
        let ready = app.streaming.as_ref().map_or(0, Streaming::ready_count);
        if app.queue.pending_len() == 0
            && app.queue.generation_pending_len() == 0
            && app.level_work.is_empty()
            && ready == loaded
            && loaded == last_loaded
        {
            return frame + 1;
        }
        last_loaded = loaded;
        frame += 1;
        std::thread::sleep(Duration::from_millis(1));
    }
    frame
}

/// Format the machine-readable result line.
#[allow(clippy::cast_precision_loss)]
fn summary(
    app: &App,
    samples: &[Duration],
    options: HeadlessOptions,
    settle_frames: u32,
) -> String {
    let mut times: Vec<f64> = samples
        .iter()
        .map(|sample| sample.as_secs_f64() * 1000.0)
        .collect();
    times.sort_by(f64::total_cmp);
    let mean = if times.is_empty() {
        0.0
    } else {
        times.iter().sum::<f64>() / times.len() as f64
    };
    let chunks = app.world.iter_chunks().count();
    let forward = app.camera.forward();
    let forward = Vec2::new(forward.x, forward.z).normalize_or_zero();
    let wanted = app.streaming.as_ref().map_or(0, |streaming| {
        streaming.wanted(app.player.position, forward).len()
    });
    let loaded = app.streaming.as_ref().map_or(0, Streaming::loaded_count);
    let ready = app.streaming.as_ref().map_or(0, Streaming::ready_count);
    let resident = app.world.resident_bytes();
    format!(
        "HEADLESS frames={} settle_frames={settle_frames} mean_ms={mean:.3} p50_ms={:.3} p95_ms={:.3} max_ms={:.3} generated_columns={} chunks={chunks} loaded_columns={loaded} wanted_columns={wanted} ready_columns={ready} pending_jobs={} gen_pending={} resident_bytes={resident}",
        options.frames,
        percentile(&times, 0.50),
        percentile(&times, 0.95),
        times.last().copied().unwrap_or(0.0),
        app.stats.generated_columns,
        app.queue.pending_len(),
        app.queue.generation_pending_len(),
    )
}

/// Nearest-rank percentile over sorted samples.
fn percentile(sorted: &[f64], fraction: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        clippy::cast_sign_loss
    )]
    let index = ((sorted.len() - 1) as f64 * fraction).round() as usize;
    sorted[index.min(sorted.len() - 1)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles_use_nearest_rank() {
        let sorted = [1.0, 2.0, 3.0, 4.0, 5.0];

        assert_eq!(percentile(&sorted, 0.0), 1.0);
        assert_eq!(percentile(&sorted, 0.5), 3.0);
        assert_eq!(percentile(&sorted, 0.95), 5.0);
        assert_eq!(percentile(&[], 0.5), 0.0);
    }
}
