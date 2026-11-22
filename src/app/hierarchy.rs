//! The seed-native section hierarchy: horizon rebuilds, section uploads,
//! and the build queue.

use std::time::{Duration, Instant};

use glam::IVec2;
use tracing::debug;

use crate::mesher::{Job, MeshPool, Priority, SectionJob, SectionResult, build_section};
use crate::section::SectionKey;

use super::App;

const MAX_SECTION_BUILDS_PER_FRAME: usize = 1;
const MAX_SECTION_JOBS: usize = 11;

impl App {
    /// Rebuilds the horizon from the near chunks and the uploaded sections.
    pub(super) fn rebuild_horizon(&mut self) {
        let camera = self.player.eye_position();
        let built = if self.hierarchy {
            crate::horizon::Horizon::build_with_sections(
                camera,
                &mut self.world,
                &self.section_bounds,
            )
        } else {
            crate::horizon::Horizon::build(camera, &mut self.world)
        };
        self.horizon = built;
        self.horizon_at = Instant::now();
    }

    /// Uploads one finished section and advances its hierarchy state.
    pub(super) fn upload_section_result(&mut self, result: &SectionResult) -> bool {
        let key = result.key;
        if self.section_jobs.get(&key) != Some(&result.job) {
            return false;
        }
        self.section_jobs.remove(&key);
        self.sections_in_flight.remove(&key);
        if let Some(bounds) = result.bounds {
            self.section_bounds.insert(key, bounds);
        }
        let mut newly_uploaded = false;
        let ready = if result.opaque.quads.is_empty() && result.water.quads.is_empty() {
            true
        } else {
            let uploaded = self.renderer.as_mut().map(|renderer| {
                let mut uploaded = true;
                if !result.opaque.quads.is_empty() {
                    uploaded = renderer.upload_section(key, &result.opaque);
                }
                if uploaded && !result.water.quads.is_empty() {
                    uploaded = renderer.upload_section_water(key, &result.water);
                }
                uploaded
            });
            let Some(uploaded) = uploaded else {
                self.reset_failed_section(key);
                return false;
            };
            if uploaded {
                newly_uploaded = self.sections_uploaded.insert(key);
            }
            uploaded
        };
        if ready {
            self.sections.mark_ready(key);
            if newly_uploaded && self.sections_uploaded.len().is_multiple_of(100) {
                debug!(
                    sections = self.sections_uploaded.len(),
                    "hierarchy progress"
                );
            }
        } else {
            debug!(
                ?key,
                uploaded = self.sections_uploaded.len(),
                "section upload deferred: pool or slot limit"
            );
            self.reset_failed_section(key);
        }
        ready
    }

    /// Drops any partial section upload and re-queues the section.
    fn reset_failed_section(&mut self, key: SectionKey) {
        if let Some(renderer) = &mut self.renderer {
            renderer.remove_section(key);
        }
        self.section_bounds.remove(&key);
        self.sections_uploaded.remove(&key);
        self.sections.mark_dirty([key]);
    }

    /// Selects, generates, and uploads hierarchy sections.
    #[allow(clippy::cast_possible_truncation)]
    pub(super) fn ensure_hierarchy(&mut self, pool: Option<&dyn MeshPool>) {
        profiling::scope!("hierarchy");
        if !self.hierarchy {
            return;
        }
        let Some(seed) = self.seed else {
            return;
        };

        let cell = IVec2::new(
            (self.player.position.x.floor() as i32).div_euclid(64),
            (self.player.position.z.floor() as i32).div_euclid(64),
        );
        let due = self.hierarchy_cell != Some(cell)
            || self.hierarchy_at.elapsed() >= Duration::from_millis(250);
        if due {
            self.hierarchy_cell = Some(cell);
            self.hierarchy_at = Instant::now();
            if self.sections.update_with_resident(
                self.player.position,
                self.detail_chunks,
                crate::hierarchy::DEFAULT_PIXELS_PER_RADIAN,
                &self.sections_uploaded,
            ) {
                self.section_cursor = 0;
                self.evict_sections();
                if let Some(renderer) = &mut self.renderer {
                    renderer.set_section_visibility(self.sections.target());
                }
                debug!(
                    target = self.sections.target().len(),
                    queue = self.sections.queue().len(),
                    in_flight = self.sections_in_flight.len(),
                    detail = self.detail_chunks,
                    "hierarchy target"
                );
            }
        }

        if self.renderer.is_none() {
            return;
        }
        if let Some(pool) = pool {
            self.submit_section_jobs(pool, seed);
            return;
        }

        let mut built = 0;
        while built < MAX_SECTION_BUILDS_PER_FRAME {
            let Some(key) = self.sections.next_build(&mut self.section_cursor) else {
                break;
            };
            self.sections.mark_building(key);
            let edits =
                self.world
                    .section_edits(key, self.save_directory.as_deref(), &self.saved_chunks);
            let job = self.next_section_job;
            self.next_section_job = self.next_section_job.wrapping_add(1);
            let result = build_section(&SectionJob {
                key,
                job,
                seed,
                edits,
            });
            self.section_jobs.insert(key, job);
            self.sections_in_flight.insert(key);
            if self.upload_section_result(&result) {
                built += 1;
            } else {
                break;
            }
        }
    }

    /// Fills the worker pool with coarse section jobs.
    fn submit_section_jobs(&mut self, pool: &dyn MeshPool, seed: u64) {
        while self.sections_in_flight.len() < MAX_SECTION_JOBS
            && pool.can_submit(Priority::Background)
        {
            let Some(key) = self.sections.next_build(&mut self.section_cursor) else {
                break;
            };
            if self.sections_in_flight.contains(&key) {
                continue;
            }
            let edits =
                self.world
                    .section_edits(key, self.save_directory.as_deref(), &self.saved_chunks);
            let job = self.next_section_job;
            self.next_section_job = self.next_section_job.wrapping_add(1);
            let job = SectionJob {
                key,
                job,
                seed,
                edits,
            };
            let job_id = job.job;
            if pool.submit(Job::Section(job), Priority::Background) {
                self.sections.mark_building(key);
                self.sections_in_flight.insert(key);
                self.section_jobs.insert(key, job_id);
            } else {
                self.sections.mark_dirty([key]);
                break;
            }
        }
    }

    /// Removes uploaded sections that the current target no longer needs.
    fn evict_sections(&mut self) {
        let stale = self.sections.evictions(
            &self.sections_uploaded,
            self.player.position,
            self.detail_chunks,
        );
        if stale.is_empty() {
            return;
        }
        if let Some(renderer) = &mut self.renderer {
            for key in &stale {
                renderer.remove_section(*key);
            }
        }
        for key in &stale {
            self.sections_uploaded.remove(key);
            self.section_bounds.remove(key);
            self.section_jobs.remove(key);
            self.sections_in_flight.remove(key);
            self.sections.forget(*key);
        }
    }
}
