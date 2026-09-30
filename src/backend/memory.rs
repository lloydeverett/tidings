//! The memory Backend: a Store's Files live in process memory, shared by every clone of the Store.
//!
//! The Files are behind an `Arc`, and each File behind one too, so a Snapshot is a clone of that
//! `Arc`: taking one copies nothing. A Commit that changes the Files copies the map
//! first only if a Snapshot still shares it (`Arc::make_mut`), and even then copies each Path and
//! a pointer to each File, never a File's contents. A Commit that changes nothing copies nothing.
//! Once copied, the map is the Backend's alone again, so later Commits copy nothing until the next
//! Snapshot. A Snapshot never takes the Backend's lock, so holding or reading one never holds up a
//! Commit.
//!
//! Beside the Files, the Backend keeps the letter-case fold of each Path, for the check
//! every Commit makes. Snapshots don't need it, so it isn't shared with them and is never copied.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use super::{CommitOutcome, CommitRequest, Planned, StoreState};
use crate::path::{letter_case_fold, range_under};
use crate::staging::has_name;
use crate::{File, Path, Prefix, Result, Revision, Stat};

#[derive(Debug, Default)]
pub(crate) struct MemoryBackend {
    held: Mutex<Held>,
}

/// The Files, as the Backend holds them.
#[derive(Debug, Default)]
struct Held {
    files: Arc<Files>,
    /// Each Path, by its [`letter_case_fold`]. No two Paths fold the same, since the check every
    /// Commit makes refuses that.
    folds: BTreeMap<String, Path>,
}

/// The Files, by Path.
type Files = BTreeMap<Path, Arc<Stored>>;

/// A File as the memory Backend keeps it.
#[derive(Debug)]
struct Stored {
    contents: String,
    stat: Stat,
}

/// The Files as they were when the Snapshot was taken. Nothing can change them.
#[derive(Debug)]
pub(crate) struct MemorySnapshot {
    files: Arc<Files>,
}

impl MemoryBackend {
    pub(crate) fn read(&self, path: &Path) -> Option<File> {
        read(&self.held.lock().unwrap().files, path)
    }

    pub(crate) fn stat(&self, path: &Path) -> Option<Stat> {
        stat(&self.held.lock().unwrap().files, path)
    }

    pub(crate) fn list(&self, prefix: &Prefix) -> Vec<Path> {
        list(&self.held.lock().unwrap().files, prefix)
    }

    /// The Path and Revision of every File under `prefix`, in order of Path.
    pub(crate) fn revisions_under(&self, prefix: &Prefix) -> Result<Vec<(Path, Revision)>> {
        self.held.lock().unwrap().revisions_under(prefix)
    }

    pub(crate) fn snapshot(&self) -> MemorySnapshot {
        MemorySnapshot { files: Arc::clone(&self.held.lock().unwrap().files) }
    }

    /// Works out what the Commit changes, then changes it, all under the one lock.
    pub(crate) fn commit(&self, request: CommitRequest) -> Result<CommitOutcome> {
        let mut held = self.held.lock().unwrap();
        let plan = request.plan(&*held)?;
        let Held { files, folds } = &mut *held;
        plan.apply(|path, planned| {
            match planned {
                Planned::Write { contents, stat } => {
                    let stored = Arc::new(Stored { contents, stat });
                    if Arc::make_mut(files).insert(path.clone(), stored).is_none() {
                        folds.insert(letter_case_fold(path.as_str()), path.clone());
                    }
                }
                Planned::Remove { .. } => {
                    Arc::make_mut(files).remove(path);
                    folds.remove(&letter_case_fold(path.as_str()));
                }
            }
            Ok(())
        })
    }
}

impl MemorySnapshot {
    pub(crate) fn read(&self, path: &Path) -> Option<File> {
        read(&self.files, path)
    }

    pub(crate) fn stat(&self, path: &Path) -> Option<Stat> {
        stat(&self.files, path)
    }

    pub(crate) fn list(&self, prefix: &Prefix) -> Vec<Path> {
        list(&self.files, prefix)
    }
}

fn read(files: &Files, path: &Path) -> Option<File> {
    let stored = files.get(path)?;
    Some(File::new(path.clone(), stored.contents.clone(), stored.stat))
}

fn stat(files: &Files, path: &Path) -> Option<Stat> {
    Some(files.get(path)?.stat)
}

fn list(files: &Files, prefix: &Prefix) -> Vec<Path> {
    files.keys().filter(|path| prefix.covers(path)).cloned().collect()
}

impl StoreState for Held {
    fn revision(&self, path: &Path) -> Result<Option<Revision>> {
        Ok(self.files.get(path).map(|stored| stored.stat.revision()))
    }

    fn revisions_under(&self, prefix: &Prefix) -> Result<Vec<(Path, Revision)>> {
        let under = self.files.iter().filter(|(path, _)| prefix.covers(path));
        Ok(under.map(|(path, stored)| (path.clone(), stored.stat.revision())).collect())
    }

    fn paths_under(&self, prefix: &Prefix) -> Result<Vec<Path>> {
        Ok(list(&self.files, prefix))
    }

    fn paths_named_like(&self, name: &str, fold: &str) -> Result<Vec<Path>> {
        let under = self.folds.range(range_under(fold)).map(|(_, path)| path);
        let folding = self.folds.get(fold).into_iter().chain(under);
        Ok(folding.filter(|path| !has_name(path, name)).cloned().collect())
    }
}
