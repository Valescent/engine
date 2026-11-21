//! The journal IO thread (`J3.1`).

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::thread::JoinHandle;

use crate::editlog::EditBatch;
use crate::error::{ErrorKind, Stage, TerrainError};
use crate::section::{Section, SectionKey};

use super::io_error;
use super::world::WorldSave;

/// One queued save action.
enum SaveCommand {
    /// Buffer one committed edit batch.
    Batch(EditBatch),
    /// Write one section after the batches that produced it.
    Section {
        key: SectionKey,
        baseline_hash: [u8; 32],
        edited: bool,
        section: Section,
        /// Greatest revision the section includes.
        through: u64,
    },
    /// Make the journal durable; leaves the header revision alone.
    Flush,
    /// Flush and stop.
    Shutdown,
}

/// The journal IO thread (`J3.1`).
///
/// The worker owns [`WorldSave`], so section writes and journal flushes happen
/// off the frame loop and in submission order. The channel is bounded: a slow
/// disk applies backpressure instead of growing memory without limit.
#[derive(Debug)]
pub struct SaveWorker {
    sender: Option<SyncSender<SaveCommand>>,
    handle: Option<JoinHandle<Result<(), TerrainError>>>,
}

impl SaveWorker {
    /// Moves `world` to a new IO thread.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::IoError`] when the OS refuses to spawn the thread.
    pub fn start(world: WorldSave, capacity: usize) -> Result<Self, TerrainError> {
        let (sender, receiver) = sync_channel(capacity.max(1));
        let handle = std::thread::Builder::new()
            .name("terrain-save".to_owned())
            .spawn(move || run(world, &receiver))
            .map_err(|error| io_error("spawn", Path::new("<save worker>"), &error))?;
        Ok(Self {
            sender: Some(sender),
            handle: Some(handle),
        })
    }

    /// Queues one committed batch.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::IoError`] when the worker has stopped.
    pub fn record_batch(&self, batch: EditBatch) -> Result<(), TerrainError> {
        self.send(SaveCommand::Batch(batch))
    }

    /// Queues one section write. It follows every batch already queued.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::IoError`] when the worker has stopped.
    pub fn save_section(
        &self,
        key: SectionKey,
        baseline_hash: [u8; 32],
        edited: bool,
        section: Section,
        through: u64,
    ) -> Result<(), TerrainError> {
        self.send(SaveCommand::Section {
            key,
            baseline_hash,
            edited,
            section,
            through,
        })
    }

    /// Queues a journal flush.
    ///
    /// This does not advance the header revision: a frame is covered only by a
    /// section save.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::IoError`] when the worker has stopped.
    pub fn flush(&self) -> Result<(), TerrainError> {
        self.send(SaveCommand::Flush)
    }

    /// Flushes and joins the worker.
    ///
    /// # Errors
    ///
    /// The first save error the worker hit, or `JobPanicked` if the thread
    /// panicked.
    pub fn shutdown(mut self) -> Result<(), TerrainError> {
        if let Some(sender) = self.sender.take() {
            let _ = sender.send(SaveCommand::Shutdown);
        }
        self.join()
    }

    fn send(&self, command: SaveCommand) -> Result<(), TerrainError> {
        let Some(sender) = &self.sender else {
            return Err(worker_error());
        };
        sender.send(command).map_err(|_| worker_error())
    }

    fn join(&mut self) -> Result<(), TerrainError> {
        let Some(handle) = self.handle.take() else {
            return Ok(());
        };
        handle.join().unwrap_or_else(|_| {
            Err(TerrainError::new(
                Stage::Persistence,
                ErrorKind::JobPanicked {
                    stage: Stage::Persistence,
                },
            ))
        })
    }
}

impl Drop for SaveWorker {
    fn drop(&mut self) {
        if let Some(sender) = self.sender.take() {
            let _ = sender.send(SaveCommand::Shutdown);
        }
        let _ = self.join();
    }
}

/// Drains commands until shutdown; a failed write stops the worker.
fn run(mut world: WorldSave, receiver: &Receiver<SaveCommand>) -> Result<(), TerrainError> {
    while let Ok(command) = receiver.recv() {
        match command {
            SaveCommand::Batch(batch) => world.record_batch(&batch)?,
            SaveCommand::Section {
                key,
                baseline_hash,
                edited,
                section,
                through,
            } => world.save_section(key, baseline_hash, edited, &section, through)?,
            SaveCommand::Flush => world.flush()?,
            SaveCommand::Shutdown => return world.flush(),
        }
    }
    world.flush()
}

/// The error for a worker that has already stopped.
fn worker_error() -> TerrainError {
    TerrainError::new(
        Stage::Persistence,
        ErrorKind::IoError {
            op: "send",
            path: PathBuf::from("<save worker>"),
            os_error: 0,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content;
    use crate::world_save::fixtures::{TempWorld, batch, key, uniform};

    #[test]
    fn the_save_worker_persists_batches_and_sections_in_order() {
        let world = TempWorld::new("worker");
        let opened = WorldSave::create(world.path(), 7, "Test world").expect("the world creates");
        let worker = SaveWorker::start(opened, 4).expect("the worker starts");
        let stone = content::v1_default()
            .id("core:stone")
            .expect("the v1 pack has stone");
        let section = uniform(stone);
        worker.record_batch(batch(3)).expect("the batch queues");
        worker
            .save_section(key(1, 2, 3), [9; 32], true, section.clone(), 3)
            .expect("the section queues");
        worker.shutdown().expect("the worker stops");

        let reopened = WorldSave::open(world.path(), 7).expect("the world reopens");
        assert_eq!(reopened.header.revision, 3);
        assert!(
            reopened.recovered().is_empty(),
            "the saved section covers the frame and trims it"
        );
        let sections = reopened.read_sections().expect("the sections read");
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].section, section);
        assert_eq!(sections[0].baseline_hash, [9; 32]);
    }
}
