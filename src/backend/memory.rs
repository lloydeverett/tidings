//! The memory Backend: every Area lives in process memory, shared by every clone of the Store.

use std::collections::BTreeMap;
use std::sync::Mutex;

use super::{AreaState, CommitOutcome, CommitRequest, RawChange};
use crate::area::PerArea;
use crate::staging::Action;
use crate::{Area, ChangeKind, File, Path, Prefix, PrefixRevision, Result, Revision, Stat};

#[derive(Debug, Default)]
pub(crate) struct MemoryBackend {
    areas: Mutex<PerArea<Files>>,
}

/// One Area's Files.
type Files = BTreeMap<Path, Stored>;

/// A File as the memory Backend keeps it.
#[derive(Debug)]
struct Stored {
    contents: String,
    stat: Stat,
}

impl MemoryBackend {
    pub(crate) fn read(&self, area: Area, path: &Path) -> Option<File> {
        let areas = self.areas.lock().unwrap();
        let stored = areas.get(area).get(path)?;
        Some(File::new(path.clone(), stored.contents.clone(), stored.stat))
    }

    pub(crate) fn stat(&self, area: Area, path: &Path) -> Option<Stat> {
        let areas = self.areas.lock().unwrap();
        let stored = areas.get(area).get(path)?;
        Some(stored.stat)
    }

    pub(crate) fn list(&self, area: Area, prefix: &Prefix) -> Vec<Path> {
        let areas = self.areas.lock().unwrap();
        areas.get(area).keys().filter(|path| prefix.covers(path)).cloned().collect()
    }

    pub(crate) fn stat_prefix(&self, area: Area, prefix: &Prefix) -> Result<PrefixRevision> {
        let areas = self.areas.lock().unwrap();
        let files = areas.get(area).revisions_under(prefix)?;
        Ok(PrefixRevision::of(area, prefix.clone(), files))
    }

    /// Checks every Precondition, then applies every write and delete at once, under the one
    /// lock.
    pub(crate) fn commit(&self, request: CommitRequest) -> Result<CommitOutcome> {
        let CommitRequest { timestamp, mut staged } = request;
        let mut areas = self.areas.lock().unwrap();
        let files = areas.get_mut(staged.area);
        staged.check_preconditions(files)?;
        staged.expand_prefix_deletes(files.keys());
        staged.refuse_clashing_paths(files.keys())?;
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
        Ok(outcome)
    }
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
