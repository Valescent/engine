//! Asynchronous chunk meshing: jobs, revisions, and a bounded worker pool.

use std::collections::{HashMap, HashSet};

use glam::IVec2;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::generation::{GenJob, GenResult, Generator};
use crate::light::{LightField, LightJob, LightResult, LightSnapshot};
use crate::mesh::{BorderBlocks, ChunkSnapshot, Lod, build_mesh, build_water_mesh, select_lod};
use crate::voxel::Vertex;
use crate::world::{ChunkCoord, DirtyChunks, World};

thread_local! {
    static WORKER_NAMED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Names each pool thread once so Tracy lanes are labelled.
fn name_worker_thread() {
    if !WORKER_NAMED.with(std::cell::Cell::get) {
        profiling::register_thread!("chunk worker");
        WORKER_NAMED.with(|named| named.set(true));
    }
}

/// Whether a job must be started soon or may be dropped under load.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Priority {
    Urgent,
    Background,
}

/// Which mesh of a chunk a job builds.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum MeshKind {
    Opaque,
    Water,
}

/// One immutable meshing request for a worker.
pub(crate) struct MeshJob {
    pub(crate) chunk: ChunkCoord,
    pub(crate) lod: Lod,
    pub(crate) kind: MeshKind,
    pub(crate) revision: u64,
    pub(crate) snapshot: ChunkSnapshot,
    pub(crate) border: BorderBlocks,
    pub(crate) light: Option<Arc<LightField>>,
}

/// One unit of work for the pool.
pub(crate) enum Job {
    Mesh(MeshJob),
    Light(LightJob),
    Generate(GenJob),
}

/// A finished job of either kind.
#[derive(Debug)]
pub(crate) enum JobResult {
    Mesh(MeshResult),
    Light(LightResult),
    Generate(GenResult),
}

/// One finished mesh, tagged so stale results can be discarded.
#[derive(Debug)]
pub(crate) struct MeshResult {
    pub(crate) chunk: ChunkCoord,
    pub(crate) lod: Lod,
    pub(crate) kind: MeshKind,
    pub(crate) revision: u64,
    pub(crate) mesh: Vec<Vertex>,
    pub(crate) elapsed: Duration,
}

/// Non-blocking access to worker threads.
pub(crate) trait MeshPool: Send + Sync {
    /// Submits a job, returning whether it was accepted.
    fn submit(&self, job: Job, priority: Priority) -> bool;
    fn try_recv(&self) -> Option<JobResult>;
}

/// micropool-backed pool, with a bounded backlog that drops background work.
#[derive(Debug)]
pub(crate) struct WorkerPool {
    sender: Sender<JobResult>,
    receiver: Mutex<Receiver<JobResult>>,
    in_flight: Arc<AtomicUsize>,
    capacity: usize,
}

impl WorkerPool {
    /// Creates a pool sized from micropool's default worker count.
    pub(crate) fn new() -> Self {
        let (sender, receiver) = mpsc::channel();
        let workers = micropool::num_threads().max(1);
        Self {
            sender,
            receiver: Mutex::new(receiver),
            in_flight: Arc::new(AtomicUsize::new(0)),
            capacity: workers * 4,
        }
    }
}

impl MeshPool for WorkerPool {
    fn submit(&self, job: Job, priority: Priority) -> bool {
        let in_flight = self.in_flight.load(Ordering::Relaxed);
        if !admit(priority, in_flight, self.capacity) {
            return false;
        }
        self.in_flight.fetch_add(1, Ordering::Relaxed);

        let sender = self.sender.clone();
        let counter = Arc::clone(&self.in_flight);
        drop(micropool::spawn_owned(move || {
            run_job(&job, &sender, &counter);
        }));
        true
    }

    fn try_recv(&self) -> Option<JobResult> {
        receive_result(&self.receiver)
    }
}

/// rayon-backed pool with the same admission rules, for A/B comparisons.
#[cfg(feature = "rayon-pool")]
#[derive(Debug)]
pub(crate) struct RayonPool {
    sender: Sender<JobResult>,
    receiver: Mutex<Receiver<JobResult>>,
    in_flight: Arc<AtomicUsize>,
    capacity: usize,
}

#[cfg(feature = "rayon-pool")]
impl RayonPool {
    /// Creates a pool sized from rayon's default worker count.
    pub(crate) fn new() -> Self {
        let (sender, receiver) = mpsc::channel();
        let workers = rayon::current_num_threads().max(1);
        Self {
            sender,
            receiver: Mutex::new(receiver),
            in_flight: Arc::new(AtomicUsize::new(0)),
            capacity: workers * 4,
        }
    }
}

#[cfg(feature = "rayon-pool")]
impl MeshPool for RayonPool {
    fn submit(&self, job: Job, priority: Priority) -> bool {
        let in_flight = self.in_flight.load(Ordering::Relaxed);
        if !admit(priority, in_flight, self.capacity) {
            return false;
        }
        self.in_flight.fetch_add(1, Ordering::Relaxed);

        let sender = self.sender.clone();
        let counter = Arc::clone(&self.in_flight);
        rayon::spawn(move || {
            run_job(&job, &sender, &counter);
        });
        true
    }

    fn try_recv(&self) -> Option<JobResult> {
        receive_result(&self.receiver)
    }
}

/// The pool selected by the build, defaulting to micropool.
///
/// With the `rayon-pool` feature, `ENGINE_MESH_POOL=rayon` selects the rayon
/// pool at startup for A/B runs.
pub(crate) fn create_pool() -> Arc<dyn MeshPool> {
    #[cfg(feature = "rayon-pool")]
    if std::env::var("ENGINE_MESH_POOL").is_ok_and(|value| value == "rayon") {
        tracing::info!(
            pool = "rayon",
            workers = rayon::current_num_threads(),
            "mesh worker pool ready"
        );
        return Arc::new(RayonPool::new());
    }
    tracing::info!(
        pool = "micropool",
        workers = micropool::num_threads(),
        "mesh worker pool ready"
    );
    Arc::new(WorkerPool::new())
}

/// Runs one job and reports its result, then releases the in-flight slot.
fn run_job(job: &Job, sender: &Sender<JobResult>, counter: &AtomicUsize) {
    name_worker_thread();
    let started = Instant::now();
    let result = match job {
        Job::Mesh(job) => {
            profiling::scope!("chunk mesh");
            let mesh = match job.kind {
                MeshKind::Opaque => {
                    build_mesh(&job.snapshot, &job.border, job.light.as_deref(), job.lod)
                }
                MeshKind::Water => {
                    build_water_mesh(&job.snapshot, &job.border, job.light.as_deref())
                }
            };
            JobResult::Mesh(MeshResult {
                chunk: job.chunk,
                lod: job.lod,
                kind: job.kind,
                revision: job.revision,
                mesh,
                elapsed: started.elapsed(),
            })
        }
        Job::Generate(job) => {
            profiling::scope!("chunk generate");
            let generator = Generator::new(job.seed);
            JobResult::Generate(GenResult {
                column: job.column,
                chunks: generator.generate_column(job.column),
                elapsed: started.elapsed(),
            })
        }
        Job::Light(job) => {
            profiling::scope!("chunk light");
            JobResult::Light(LightResult {
                chunk: job.chunk,
                revision: job.revision,
                light: crate::light::compute(&job.snapshot, job.chunk),
                elapsed: started.elapsed(),
            })
        }
    };
    let _ = sender.send(result);
    counter.fetch_sub(1, Ordering::Relaxed);
}

fn receive_result(receiver: &Mutex<Receiver<JobResult>>) -> Option<JobResult> {
    receiver
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .try_recv()
        .ok()
}

/// Returns whether a job may be submitted to a queue of `capacity`.
///
/// Background work is dropped when the backlog is full; urgent work always
/// proceeds so an edit never waits on the queue.
pub(crate) fn admit(priority: Priority, in_flight: usize, capacity: usize) -> bool {
    priority == Priority::Urgent || in_flight < capacity
}

/// Per-chunk revisions and the jobs currently in flight.
#[derive(Debug, Default)]
pub(crate) struct MeshQueue {
    revisions: HashMap<ChunkCoord, u64>,
    pending: HashMap<(ChunkCoord, Lod, MeshKind), u64>,
    built: HashMap<(ChunkCoord, Lod, MeshKind), u64>,
    light_pending: HashMap<ChunkCoord, u64>,
    light_built: HashMap<ChunkCoord, u64>,
    gen_pending: HashSet<IVec2>,
    desired: HashMap<ChunkCoord, Lod>,
}

impl MeshQueue {
    /// Current revision of a chunk; new chunks start at zero.
    pub(crate) fn revision(&self, chunk: ChunkCoord) -> u64 {
        self.revisions.get(&chunk).copied().unwrap_or(0)
    }

    /// Invalidates every mesh of the edited chunks and their queued jobs.
    ///
    /// GPU buffers stay in place until replacements arrive, so the previous
    /// mesh may remain visible for a few frames.
    pub(crate) fn invalidate(&mut self, dirty: &DirtyChunks) {
        for chunk in dirty {
            *self.revisions.entry(*chunk).or_insert(0) += 1;
            self.pending.retain(|(pending, _, _), _| pending != chunk);
            self.built.retain(|(built, _, _), _| built != chunk);
            self.light_pending.remove(chunk);
            self.light_built.remove(chunk);
        }
    }

    /// Forgets the pending work for a chunk that left the world.
    ///
    /// Revisions are kept so an in-flight result is still rejected.
    pub(crate) fn remove(&mut self, chunk: ChunkCoord) {
        self.desired.remove(&chunk);
        self.pending.retain(|(pending, _, _), _| *pending != chunk);
        self.built.retain(|(built, _, _), _| *built != chunk);
        self.light_pending.remove(&chunk);
        self.light_built.remove(&chunk);
    }

    /// Submits a mesh job unless the same revision is already in flight.
    pub(crate) fn request(
        &mut self,
        pool: &dyn MeshPool,
        world: &World,
        chunk: ChunkCoord,
        lod: Lod,
        kind: MeshKind,
        priority: Priority,
    ) {
        let revision = self.revision(chunk);
        if self.pending.get(&(chunk, lod, kind)) == Some(&revision) {
            return;
        }
        let job = MeshJob {
            chunk,
            lod,
            revision,
            kind,
            snapshot: ChunkSnapshot::new(world, chunk),
            border: BorderBlocks::new(world, chunk),
            light: world.light_field(chunk).cloned(),
        };
        if pool.submit(Job::Mesh(job), priority) {
            self.pending.insert((chunk, lod, kind), revision);
        }
    }

    /// Requests generation for one column unless it is already queued.
    pub(crate) fn request_generation(
        &mut self,
        pool: &dyn MeshPool,
        seed: u64,
        column: IVec2,
        priority: Priority,
    ) {
        if !self.gen_pending.insert(column) {
            return;
        }
        let job = GenJob { column, seed };
        if !pool.submit(Job::Generate(job), priority) {
            self.gen_pending.remove(&column);
        }
    }

    /// Accepts a generated column that is still pending.
    pub(crate) fn accept_generation(&mut self, result: &GenResult) -> bool {
        self.gen_pending.remove(&result.column)
    }

    /// Forgets a column's pending generation; a late result is discarded.
    pub(crate) fn cancel_generation(&mut self, column: IVec2) {
        self.gen_pending.remove(&column);
    }

    /// Submits a light job unless the same revision is already in flight.
    pub(crate) fn request_light(
        &mut self,
        pool: &dyn MeshPool,
        world: &World,
        chunk: ChunkCoord,
        priority: Priority,
    ) {
        let revision = self.revision(chunk);
        if self.light_pending.get(&chunk) == Some(&revision)
            || self.light_built.get(&chunk) == Some(&revision)
        {
            return;
        }
        let job = LightJob {
            chunk,
            revision,
            snapshot: LightSnapshot::new(world, chunk),
        };
        if pool.submit(Job::Light(job), priority) {
            self.light_pending.insert(chunk, revision);
        }
    }

    /// Marks a chunk as carrying current light, used after a synchronous relight.
    pub(crate) fn mark_light_built(&mut self, chunk: ChunkCoord) {
        self.light_pending.remove(&chunk);
        self.light_built.insert(chunk, self.revision(chunk));
    }

    /// Returns whether the chunk has an accepted light field at its revision.
    pub(crate) fn light_is_current(&self, chunk: ChunkCoord) -> bool {
        self.light_built.get(&chunk) == Some(&self.revision(chunk))
    }

    /// Accepts a light result only when its revision and request are current.
    pub(crate) fn accept_light(&mut self, result: &LightResult) -> bool {
        if result.revision != self.revision(result.chunk) {
            return false;
        }
        if self.light_pending.get(&result.chunk) != Some(&result.revision) {
            return false;
        }
        self.light_pending.remove(&result.chunk);
        self.light_built.insert(result.chunk, result.revision);
        true
    }

    /// Accepts a result only when its revision and request are still current.
    pub(crate) fn accept(&mut self, result: &MeshResult) -> bool {
        if result.revision != self.revision(result.chunk) {
            return false;
        }
        let key = (result.chunk, result.lod, result.kind);
        if self.pending.get(&key) != Some(&result.revision) {
            return false;
        }
        self.pending.remove(&key);
        self.built.insert(key, result.revision);
        true
    }

    /// Returns whether a mesh for this chunk, LOD, and kind is current.
    pub(crate) fn is_built(&self, chunk: ChunkCoord, lod: Lod, kind: MeshKind) -> bool {
        self.built.get(&(chunk, lod, kind)) == Some(&self.revision(chunk))
    }

    /// Recalculates the desired LOD from the player's chunk with hysteresis.
    pub(crate) fn desired_lod(&mut self, chunk: ChunkCoord, player_chunk: ChunkCoord) -> Lod {
        let distance = (chunk.coordinates() - player_chunk.coordinates())
            .as_vec3()
            .length();
        let lod = select_lod(self.desired.get(&chunk).copied(), distance);
        self.desired.insert(chunk, lod);
        lod
    }

    /// Desired LOD last selected for a chunk, if any.
    pub(crate) fn desired(&self, chunk: ChunkCoord) -> Option<Lod> {
        self.desired.get(&chunk).copied()
    }

    /// Number of mesh jobs currently in flight.
    pub(crate) fn pending_len(&self) -> usize {
        self.pending.len()
    }
}

/// Accepts up to `budget` current meshes and returns them with the total worker
/// time of every result received, stale ones included.
pub(crate) fn collect_finished(
    queue: &mut MeshQueue,
    pool: &dyn MeshPool,
    budget: usize,
) -> (Vec<MeshResult>, Vec<LightResult>, Vec<GenResult>, Duration) {
    let mut meshes = Vec::new();
    let mut lights = Vec::new();
    let mut generations = Vec::new();
    let mut worker_time = Duration::ZERO;
    while meshes.len() < budget {
        let Some(result) = pool.try_recv() else {
            break;
        };
        match result {
            JobResult::Mesh(result) => {
                worker_time += result.elapsed;
                if queue.accept(&result) {
                    meshes.push(result);
                }
            }
            JobResult::Light(result) => {
                worker_time += result.elapsed;
                if queue.accept_light(&result) {
                    lights.push(result);
                }
            }
            JobResult::Generate(result) => {
                worker_time += result.elapsed;
                if queue.accept_generation(&result) {
                    generations.push(result);
                }
            }
        }
    }
    (meshes, lights, generations, worker_time)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[derive(Debug, Default)]
    struct FakePool {
        results: Mutex<Vec<JobResult>>,
        submissions: AtomicUsize,
    }

    impl FakePool {
        fn with(results: Vec<MeshResult>) -> Self {
            Self {
                results: Mutex::new(results.into_iter().map(JobResult::Mesh).collect()),
                submissions: AtomicUsize::new(0),
            }
        }
    }

    impl MeshPool for FakePool {
        fn submit(&self, _job: Job, _priority: Priority) -> bool {
            self.submissions.fetch_add(1, Ordering::Relaxed);
            true
        }

        fn try_recv(&self) -> Option<JobResult> {
            self.results
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .pop()
        }
    }

    fn chunk(x: i32) -> ChunkCoord {
        ChunkCoord::from_world(glam::IVec3::new(x * 32, 0, 0)).0
    }

    fn result(chunk: ChunkCoord, lod: Lod, revision: u64) -> MeshResult {
        result_of(chunk, lod, MeshKind::Opaque, revision)
    }

    fn water_result(chunk: ChunkCoord, lod: Lod, revision: u64) -> MeshResult {
        result_of(chunk, lod, MeshKind::Water, revision)
    }

    fn result_of(chunk: ChunkCoord, lod: Lod, kind: MeshKind, revision: u64) -> MeshResult {
        MeshResult {
            chunk,
            lod,
            kind,
            revision,
            mesh: Vec::new(),
            elapsed: Duration::from_millis(1),
        }
    }

    fn light_result(chunk: ChunkCoord, revision: u64) -> LightResult {
        LightResult {
            chunk,
            revision,
            light: crate::light::compute(&LightSnapshot::default(), chunk),
            elapsed: Duration::from_millis(1),
        }
    }

    #[test]
    fn light_results_follow_revisions_and_pending_state() {
        let pool = FakePool::default();
        let mut queue = MeshQueue::default();
        let target = chunk(0);

        assert!(!queue.light_is_current(target));
        queue.request_light(&pool, &World::default(), target, Priority::Urgent);
        assert!(!queue.light_is_current(target));

        assert!(queue.accept_light(&light_result(target, 0)));
        assert!(queue.light_is_current(target));

        queue.invalidate(&DirtyChunks::from([target]));
        assert!(!queue.light_is_current(target));
        assert!(!queue.accept_light(&light_result(target, 0)));

        queue.request_light(&pool, &World::default(), target, Priority::Urgent);
        assert!(queue.accept_light(&light_result(target, 1)));
    }

    #[test]
    fn water_meshes_track_their_own_build_state() {
        let pool = FakePool::default();
        let mut queue = MeshQueue::default();
        let target = chunk(0);

        queue.request(
            &pool,
            &World::default(),
            target,
            Lod::Lod0,
            MeshKind::Water,
            Priority::Background,
        );
        assert!(!queue.is_built(target, Lod::Lod0, MeshKind::Water));

        assert!(queue.accept(&water_result(target, Lod::Lod0, 0)));
        assert!(queue.is_built(target, Lod::Lod0, MeshKind::Water));
        assert!(!queue.is_built(target, Lod::Lod0, MeshKind::Opaque));

        queue.invalidate(&DirtyChunks::from([target]));
        assert!(!queue.is_built(target, Lod::Lod0, MeshKind::Water));
    }

    #[test]
    fn background_jobs_drop_when_the_queue_is_full() {
        assert!(admit(Priority::Background, 0, 4));
        assert!(admit(Priority::Background, 3, 4));
        assert!(!admit(Priority::Background, 4, 4));
        assert!(admit(Priority::Urgent, 4, 4));
        assert!(admit(Priority::Urgent, 100, 4));
    }

    #[test]
    fn revisions_start_at_zero_and_increment_on_edits() {
        let mut queue = MeshQueue::default();
        let edited = chunk(0);
        let untouched = chunk(1);

        assert_eq!(queue.revision(edited), 0);
        queue.invalidate(&DirtyChunks::from([edited]));

        assert_eq!(queue.revision(edited), 1);
        assert_eq!(queue.revision(untouched), 0);
    }

    #[test]
    fn stale_results_are_rejected_after_an_edit() {
        let pool = FakePool::default();
        let mut queue = MeshQueue::default();
        let target = chunk(0);
        queue.request(
            &pool,
            &World::default(),
            target,
            Lod::Lod0,
            MeshKind::Opaque,
            Priority::Urgent,
        );

        assert!(queue.accept(&result(target, Lod::Lod0, 0)));

        queue.request(
            &pool,
            &World::default(),
            target,
            Lod::Lod0,
            MeshKind::Opaque,
            Priority::Urgent,
        );
        queue.invalidate(&DirtyChunks::from([target]));

        assert!(!queue.accept(&result(target, Lod::Lod0, 0)));

        queue.request(
            &pool,
            &World::default(),
            target,
            Lod::Lod0,
            MeshKind::Opaque,
            Priority::Urgent,
        );
        assert!(queue.accept(&result(target, Lod::Lod0, 1)));
    }

    #[test]
    fn each_revision_is_requested_once() {
        let pool = FakePool::default();
        let mut queue = MeshQueue::default();
        let target = chunk(0);

        queue.request(
            &pool,
            &World::default(),
            target,
            Lod::Lod0,
            MeshKind::Opaque,
            Priority::Urgent,
        );
        queue.request(
            &pool,
            &World::default(),
            target,
            Lod::Lod0,
            MeshKind::Opaque,
            Priority::Urgent,
        );

        assert_eq!(pool.submissions.load(Ordering::Relaxed), 1);
        assert!(queue.accept(&result(target, Lod::Lod0, 0)));
        assert!(!queue.accept(&result(target, Lod::Lod0, 0)));
    }

    #[test]
    fn lod_can_be_cached_per_chunk() {
        let pool = FakePool::default();
        let mut queue = MeshQueue::default();
        let target = chunk(0);

        queue.request(
            &pool,
            &World::default(),
            target,
            Lod::Lod0,
            MeshKind::Opaque,
            Priority::Urgent,
        );
        queue.request(
            &pool,
            &World::default(),
            target,
            Lod::Lod1,
            MeshKind::Opaque,
            Priority::Background,
        );

        assert!(queue.accept(&result(target, Lod::Lod1, 0)));
        assert!(queue.accept(&result(target, Lod::Lod0, 0)));
    }

    #[test]
    fn at_most_two_meshes_are_collected_per_frame() {
        let pool = FakePool::with((0..4).map(|x| result(chunk(x), Lod::Lod0, 0)).collect());
        let mut queue = MeshQueue::default();
        for x in 0..4 {
            queue.request(
                &pool,
                &World::default(),
                chunk(x),
                Lod::Lod0,
                MeshKind::Opaque,
                Priority::Urgent,
            );
        }

        let (first, lights, generations, worker_time) = collect_finished(&mut queue, &pool, 2);
        assert_eq!(first.len(), 2);
        assert!(lights.is_empty());
        assert!(generations.is_empty());
        assert_eq!(worker_time, Duration::from_millis(2));

        let (second, _, _, _) = collect_finished(&mut queue, &pool, 2);
        assert_eq!(second.len(), 2);
    }

    #[test]
    fn an_empty_pool_uploads_nothing() {
        let pool = FakePool::default();
        let mut queue = MeshQueue::default();

        let (results, lights, generations, worker_time) = collect_finished(&mut queue, &pool, 2);

        assert!(results.is_empty());
        assert!(lights.is_empty());
        assert!(generations.is_empty());
        assert!(generations.is_empty());
        assert!(worker_time.is_zero());
    }

    #[test]
    fn a_refused_job_is_not_recorded_as_pending() {
        #[derive(Debug)]
        struct RefusingPool;

        impl MeshPool for RefusingPool {
            fn submit(&self, _job: Job, _priority: Priority) -> bool {
                false
            }

            fn try_recv(&self) -> Option<JobResult> {
                None
            }
        }

        let mut queue = MeshQueue::default();
        let target = chunk(0);

        queue.request(
            &RefusingPool,
            &World::default(),
            target,
            Lod::Lod1,
            MeshKind::Opaque,
            Priority::Background,
        );

        assert!(!queue.accept(&result(target, Lod::Lod1, 0)));
    }

    #[test]
    fn desired_lod_uses_distance_hysteresis() {
        let mut queue = MeshQueue::default();
        let player = chunk(0);

        assert_eq!(queue.desired_lod(chunk(0), player), Lod::Lod0);
        assert_eq!(queue.desired_lod(chunk(4), player), Lod::Lod0);
        assert_eq!(queue.desired_lod(chunk(8), player), Lod::Lod1);

        let target = chunk(0);
        assert_eq!(queue.desired_lod(target, chunk(10)), Lod::Lod1);
        assert_eq!(queue.desired_lod(target, chunk(5)), Lod::Lod1);
        assert_eq!(queue.desired_lod(target, chunk(3)), Lod::Lod0);
        assert_eq!(queue.desired_lod(target, chunk(6)), Lod::Lod0);
        assert_eq!(queue.desired_lod(target, chunk(12)), Lod::Lod1);
        assert_eq!(queue.desired(target), Some(Lod::Lod1));
    }
}
