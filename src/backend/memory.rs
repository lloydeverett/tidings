//! The memory Backend: every Area lives in process memory, shared by every clone of the Store.

use std::collections::BTreeMap;
use std::sync::Mutex;

use jiff::Timestamp;

use super::{Applied, CommitRequest};
use crate::staging::Staged;
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
    modified: Timestamp,
    revision: Revision,
}

impl MemoryBackend {
    pub(crate) fn read(&self, area: Area, path: &Path) -> Option<File> {
        let areas = self.areas.lock().unwrap();
        let stored = areas[area as usize].get(path)?;
        Some(File::new(path.clone(), stored.contents.clone(), stored.modified, stored.revision))
    }

    pub(crate) fn stat(&self, area: Area, path: &Path) -> Option<Stat> {
        let areas = self.areas.lock().unwrap();
        let stored = areas[area as usize].get(path)?;
        Some(Stat::new(stored.modified, stored.revision))
    }

    pub(crate) fn list(&self, area: Area, prefix: &Prefix) -> Vec<Path> {
        let areas = self.areas.lock().unwrap();
        areas[area as usize].keys().filter(|path| prefix.covers(path)).cloned().collect()
    }

    /// Applies every write and delete at once, under the one lock.
    pub(crate) fn commit(&self, request: CommitRequest) -> Applied {
        let CommitRequest { area, timestamp, mut staged, prefix_deletes } = request;
        let mut areas = self.areas.lock().unwrap();
        let files = &mut areas[area as usize];
        for prefix in &prefix_deletes {
            for path in files.keys().filter(|path| prefix.covers(path)) {
                staged.entry(path.clone()).or_insert(Staged::Delete);
            }
        }
        let mut applied = Applied::default();
        for (path, staged) in staged {
            match staged {
                Staged::Write(contents) => {
                    let revision = Revision::of(&contents);
                    applied.revisions.insert(path.clone(), revision);
                    // A write that changes nothing is left out: the File keeps its time.
                    if files.get(&path).is_some_and(|stored| stored.revision == revision) {
                        continue;
                    }
                    files.insert(path.clone(), Stored { contents, modified: timestamp, revision });
                    applied.changes.push((path, ChangeKind::Changed));
                }
                Staged::Delete => {
                    if files.remove(&path).is_some() {
                        applied.changes.push((path, ChangeKind::Removed));
                    }
                }
            }
        }
        applied
    }
}
