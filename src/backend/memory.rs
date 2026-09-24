//! The memory Backend: every Area lives in process memory, shared by every clone of the Store.
//!
//! Each Area's Files are behind an `Arc`, and each File behind one too, so a Snapshot is a clone
//! of the Area's `Arc`: taking one copies nothing. A Commit that changes the Area copies its map
//! first only if a Snapshot still shares it (`Arc::make_mut`), and even then copies each Path and
//! a pointer to each File, never a File's contents. A Commit that changes nothing copies nothing.
//! Once copied, the map is the Backend's alone again, so later Commits copy nothing until the next
//! Snapshot. A Snapshot never takes the Backend's lock, so holding or reading one never holds up a
//! Commit.
//!
//! Beside each Area's Files, the Backend keeps the letter-case fold of each Path, for the check
//! every Commit makes. Snapshots don't need it, so it isn't shared with them and is never copied.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use super::{AreaState, CommitOutcome, CommitRequest, Planned};
use crate::area::PerArea;
use crate::path::{letter_case_fold, range_under};
use crate::staging::has_name;
use crate::{Area, File, Path, Prefix, PrefixRevision, Result, Revision, Stat};

#[derive(Debug, Default)]
pub(crate) struct MemoryBackend {
    areas: Mutex<PerArea<AreaInMemory>>,
}

/// One Area as the Backend holds it.
#[derive(Debug, Default)]
struct AreaInMemory {
    files: Arc<Files>,
    /// Each Path, by its [`letter_case_fold`]. No two Paths fold the same, since the check every
    /// Commit makes refuses that.
    folds: BTreeMap<String, Path>,
}

/// One Area's Files.
type Files = BTreeMap<Path, Arc<Stored>>;

/// A File as the memory Backend keeps it.
#[derive(Debug)]
struct Stored {
    contents: String,
    stat: Stat,
}

/// One Area's Files as they were when the Snapshot was taken. Nothing can change them.
#[derive(Debug)]
pub(crate) struct MemorySnapshot {
    files: Arc<Files>,
}

impl MemoryBackend {
    pub(crate) fn read(&self, area: Area, path: &Path) -> Option<File> {
        read(&self.areas.lock().unwrap().get(area).files, path)
    }

    pub(crate) fn stat(&self, area: Area, path: &Path) -> Option<Stat> {
        stat(&self.areas.lock().unwrap().get(area).files, path)
    }

    pub(crate) fn list(&self, area: Area, prefix: &Prefix) -> Vec<Path> {
        list(&self.areas.lock().unwrap().get(area).files, prefix)
    }

    pub(crate) fn stat_prefix(&self, area: Area, prefix: &Prefix) -> Result<PrefixRevision> {
        let areas = self.areas.lock().unwrap();
        let files = areas.get(area).revisions_under(prefix)?;
        Ok(PrefixRevision::of(area, prefix.clone(), files))
    }

    pub(crate) fn snapshot(&self, area: Area) -> MemorySnapshot {
        MemorySnapshot { files: Arc::clone(&self.areas.lock().unwrap().get(area).files) }
    }

    /// Works out what the Commit changes, then changes it, all under the one lock.
    pub(crate) fn commit(&self, request: CommitRequest) -> Result<CommitOutcome> {
        let mut areas = self.areas.lock().unwrap();
        let area = areas.get_mut(request.staged.area);
        let plan = request.plan(&*area)?;
        let AreaInMemory { files, folds } = area;
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

impl AreaState for AreaInMemory {
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
