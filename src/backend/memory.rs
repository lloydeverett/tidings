//! The memory Backend: every Area lives in process memory, shared by every clone of the Store.
//!
//! Each Area's Files are behind an `Arc`, and each File behind one too, so a Snapshot is a clone
//! of the Area's `Arc`: taking one copies nothing. A Commit copies the Area's map before changing
//! it only if a Snapshot still shares it (`Arc::make_mut`), and even then copies each Path and a
//! pointer to each File, never a File's contents. Once copied, the map is the Backend's alone
//! again, so later Commits copy nothing until the next Snapshot. A Snapshot never takes the
//! Backend's lock, so holding or reading one never holds up a Commit.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use super::{AreaState, CommitOutcome, CommitRequest, RawChange};
use crate::area::PerArea;
use crate::staging::Action;
use crate::{Area, ChangeKind, File, Path, Prefix, PrefixRevision, Result, Revision, Stat};

#[derive(Debug, Default)]
pub(crate) struct MemoryBackend {
    areas: Mutex<PerArea<Arc<Files>>>,
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
        read(self.areas.lock().unwrap().get(area), path)
    }

    pub(crate) fn stat(&self, area: Area, path: &Path) -> Option<Stat> {
        stat(self.areas.lock().unwrap().get(area), path)
    }

    pub(crate) fn list(&self, area: Area, prefix: &Prefix) -> Vec<Path> {
        list(self.areas.lock().unwrap().get(area), prefix)
    }

    pub(crate) fn stat_prefix(&self, area: Area, prefix: &Prefix) -> Result<PrefixRevision> {
        let areas = self.areas.lock().unwrap();
        let files = areas.get(area).revisions_under(prefix)?;
        Ok(PrefixRevision::of(area, prefix.clone(), files))
    }

    pub(crate) fn snapshot(&self, area: Area) -> MemorySnapshot {
        MemorySnapshot { files: Arc::clone(self.areas.lock().unwrap().get(area)) }
    }

    /// Checks every Precondition, then applies every write and delete at once, under the one
    /// lock.
    pub(crate) fn commit(&self, request: CommitRequest) -> Result<CommitOutcome> {
        let CommitRequest { timestamp, mut staged } = request;
        let mut areas = self.areas.lock().unwrap();
        let files = areas.get_mut(staged.area);
        staged.check_preconditions(&**files)?;
        staged.expand_prefix_deletes(files.keys());
        staged.refuse_clashing_paths(files.keys())?;
        // Copies the map (not the Files) if a Snapshot still shares it.
        let files = Arc::make_mut(files);
        let mut outcome = CommitOutcome::default();
        for (path, action) in staged.actions {
            match action {
                Action::Write(contents) => {
                    let revision = Revision::of(&contents);
                    outcome.revisions.insert(path.clone(), revision);
                    // A write that changes nothing is left out: the File keeps its time.
                    if files.get(&path).is_some_and(|stored| stored.stat.revision() == revision) {
                        continue;
                    }
                    let stat = Stat::new(timestamp, revision);
                    files.insert(path.clone(), Arc::new(Stored { contents, stat }));
                    outcome.changes.push(RawChange { path, kind: ChangeKind::Changed });
                }
                Action::Delete => {
                    if files.remove(&path).is_some() {
                        outcome.changes.push(RawChange { path, kind: ChangeKind::Removed });
                    }
                }
            }
        }
        Ok(outcome)
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

impl AreaState for Files {
    fn revision(&self, path: &Path) -> Result<Option<Revision>> {
        Ok(self.get(path).map(|stored| stored.stat.revision()))
    }

    fn revisions_under(&self, prefix: &Prefix) -> Result<Vec<(Path, Revision)>> {
        let under = self.iter().filter(|(path, _)| prefix.covers(path));
        Ok(under.map(|(path, stored)| (path.clone(), stored.stat.revision())).collect())
    }
}
