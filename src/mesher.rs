//! Asynchronous chunk meshing: jobs, revisions, and a bounded worker pool.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::light::LightField;
use crate::mesh::{BorderBlocks, ChunkSnapshot, Lod, build_mesh, select_lod};
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

/// One immutable meshing request for a worker.
pub(crate) struct MeshJob {
    pub(crate) chunk: ChunkCoord,
    pub(crate) lod: Lod,
    pub(crate) revision: u64,
    pub(crate) snapshot: ChunkSnapshot,
    pub(crate) border: BorderBlocks,
    pub(crate) light: Option<Arc<LightField>>,
}

/// One finished mesh, tagged so stale results can be discarded.
#[derive(Debug)]
pub(crate) struct MeshResult {
    pub(crate) chunk: ChunkCoord,
    pub(crate) lod: Lod,
    pub(crate) revision: u64,
    pub(crate) mesh: Vec<Vertex>,
    pub(crate) elapsed: Duration,
}

/// Non-blocking access to worker threads.
pub(crate) trait MeshPool: Send + Sync {
    /// Submits a job, returning whether it was accepted.
    fn submit(&self, job: MeshJob, priority: Priority) -> bool;
    fn try_recv(&self) -> Option<MeshResult>;
}

/// micropool-backed pool, with a bounded backlog that drops background work.
#[derive(Debug)]
pub(crate) struct WorkerPool {
    sender: Sender<MeshResult>,
    receiver: Mutex<Receiver<MeshResult>>,
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
    fn submit(&self, job: MeshJob, priority: Priority) -> bool {
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

    fn try_recv(&self) -> Option<MeshResult> {
        receive_result(&self.receiver)
    }
}

/// rayon-backed pool with the same admission rules, for A/B comparisons.
#[cfg(feature = "rayon-pool")]
#[derive(Debug)]
pub(crate) struct RayonPool {
    sender: Sender<MeshResult>,
    receiver: Mutex<Receiver<MeshResult>>,
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
    fn submit(&self, job: MeshJob, priority: Priority) -> bool {
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

    fn try_recv(&self) -> Option<MeshResult> {
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
fn run_job(job: &MeshJob, sender: &Sender<MeshResult>, counter: &AtomicUsize) {
    name_worker_thread();
    profiling::scope!("chunk mesh");
    let started = Instant::now();
    let mesh = build_mesh(&job.snapshot, &job.border, job.light.as_deref(), job.lod);
    let result = MeshResult {
        chunk: job.chunk,
        lod: job.lod,
        revision: job.revision,
        mesh,
        elapsed: started.elapsed(),
    };
    let _ = sender.send(result);
    counter.fetch_sub(1, Ordering::Relaxed);
}

fn receive_result(receiver: &Mutex<Receiver<MeshResult>>) -> Option<MeshResult> {
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
    pending: HashMap<(ChunkCoord, Lod), u64>,
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
            self.pending.retain(|(pending, _), _| pending != chunk);
        }
    }

    /// Forgets the pending work for a chunk that left the world.
    ///
    /// Revisions are kept so an in-flight result is still rejected.
    pub(crate) fn remove(&mut self, chunk: ChunkCoord) {
        self.desired.remove(&chunk);
        self.pending.retain(|(pending, _), _| *pending != chunk);
    }

    /// Submits a mesh job unless the same revision is already in flight.
    pub(crate) fn request(
        &mut self,
        pool: &dyn MeshPool,
        world: &World,
        chunk: ChunkCoord,
        lod: Lod,
        priority: Priority,
    ) {
        let revision = self.revision(chunk);
        if self.pending.get(&(chunk, lod)) == Some(&revision) {
            return;
        }
        let job = MeshJob {
            chunk,
            lod,
            revision,
            snapshot: ChunkSnapshot::new(world, chunk),
            border: BorderBlocks::new(world, chunk),
            light: world.light_field(chunk).cloned(),
        };
        if pool.submit(job, priority) {
            self.pending.insert((chunk, lod), revision);
        }
    }

    /// Accepts a result only when its revision and request are still current.
    pub(crate) fn accept(&mut self, result: &MeshResult) -> bool {
        if result.revision != self.revision(result.chunk) {
            return false;
        }
        let key = (result.chunk, result.lod);
        if self.pending.get(&key) != Some(&result.revision) {
            return false;
        }
        self.pending.remove(&key);
        true
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
) -> (Vec<MeshResult>, Duration) {
    let mut accepted = Vec::new();
    let mut worker_time = Duration::ZERO;
    while accepted.len() < budget {
        let Some(result) = pool.try_recv() else {
            break;
        };
        worker_time += result.elapsed;
        if queue.accept(&result) {
            accepted.push(result);
        }
    }
    (accepted, worker_time)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[derive(Debug, Default)]
    struct FakePool {
        results: Mutex<Vec<MeshResult>>,
        submissions: AtomicUsize,
    }

    impl FakePool {
        fn with(results: Vec<MeshResult>) -> Self {
            Self {
                results: Mutex::new(results),
                submissions: AtomicUsize::new(0),
            }
        }
    }

    impl MeshPool for FakePool {
        fn submit(&self, _job: MeshJob, _priority: Priority) -> bool {
            self.submissions.fetch_add(1, Ordering::Relaxed);
            true
        }

        fn try_recv(&self) -> Option<MeshResult> {
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
        MeshResult {
            chunk,
            lod,
            revision,
            mesh: Vec::new(),
            elapsed: Duration::from_millis(1),
        }
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
            Priority::Urgent,
        );

        assert!(queue.accept(&result(target, Lod::Lod0, 0)));

        queue.request(
            &pool,
            &World::default(),
            target,
            Lod::Lod0,
            Priority::Urgent,
        );
        queue.invalidate(&DirtyChunks::from([target]));

        assert!(!queue.accept(&result(target, Lod::Lod0, 0)));

        queue.request(
            &pool,
            &World::default(),
            target,
            Lod::Lod0,
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
            Priority::Urgent,
        );
        queue.request(
            &pool,
            &World::default(),
            target,
            Lod::Lod0,
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
            Priority::Urgent,
        );
        queue.request(
            &pool,
            &World::default(),
            target,
            Lod::Lod1,
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
                Priority::Urgent,
            );
        }

        let (first, worker_time) = collect_finished(&mut queue, &pool, 2);
        assert_eq!(first.len(), 2);
        assert_eq!(worker_time, Duration::from_millis(2));

        let (second, _) = collect_finished(&mut queue, &pool, 2);
        assert_eq!(second.len(), 2);
    }

    #[test]
    fn an_empty_pool_uploads_nothing() {
        let pool = FakePool::default();
        let mut queue = MeshQueue::default();

        let (results, worker_time) = collect_finished(&mut queue, &pool, 2);

        assert!(results.is_empty());
        assert!(worker_time.is_zero());
    }

    #[test]
    fn a_refused_job_is_not_recorded_as_pending() {
        #[derive(Debug)]
        struct RefusingPool;

        impl MeshPool for RefusingPool {
            fn submit(&self, _job: MeshJob, _priority: Priority) -> bool {
                false
            }

            fn try_recv(&self) -> Option<MeshResult> {
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
