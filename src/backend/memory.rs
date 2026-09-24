//! The memory Backend: every Area lives in process memory, shared by every clone of the Store.

use std::collections::BTreeMap;
use std::sync::Mutex;

use super::{CommitOutcome, CommitRequest, RawChange};
use crate::staging::Action;
use crate::{Area, ChangeKind, File, Path, Prefix, Revision, Stat};

#[derive(Debug, Default)]
pub(crate) struct MemoryBackend {
    areas: Mutex<Areas>,
}

/// Each Area's Files, indexed by `Area as usize`.
type Areas = [BTreeMap<Path, Stored>; 3];

/// A File as the memory Backend keeps it.
#[derive(Debug)]
struct Stored {
    contents: String,
    stat: Stat,
}

impl MemoryBackend {
    pub(crate) fn read(&self, area: Area, path: &Path) -> Option<File> {
        let areas = self.areas.lock().unwrap();
        let stored = areas[area as usize].get(path)?;
        Some(File::new(path.clone(), stored.contents.clone(), stored.stat))
    }

    pub(crate) fn stat(&self, area: Area, path: &Path) -> Option<Stat> {
        let areas = self.areas.lock().unwrap();
        let stored = areas[area as usize].get(path)?;
        Some(stored.stat)
    }

    pub(crate) fn list(&self, area: Area, prefix: &Prefix) -> Vec<Path> {
        let areas = self.areas.lock().unwrap();
        areas[area as usize].keys().filter(|path| prefix.covers(path)).cloned().collect()
    }

    /// Applies every write and delete at once, under the one lock.
    pub(crate) fn commit(&self, request: CommitRequest) -> CommitOutcome {
        let CommitRequest { timestamp, mut staged } = request;
        let mut areas = self.areas.lock().unwrap();
        let files = &mut areas[staged.area as usize];
        staged.expand_prefix_deletes(files.keys());
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
                    files.insert(path.clone(), Stored { contents, stat });
                    outcome.changes.push(RawChange { path, kind: ChangeKind::Changed });
                }
                Action::Delete => {
                    if files.remove(&path).is_some() {
                        outcome.changes.push(RawChange { path, kind: ChangeKind::Removed });
                    }
                }
            }
        }
        outcome
    }
}
