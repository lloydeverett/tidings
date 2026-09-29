//! Settling local changes in a Working copy: `discard` throws the person's side away and takes
//! the Store's version, and `resolve` takes what is in the folder as the merge of a Diverged Path,
//! against the Store's version in `theirs`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path as FsPath, PathBuf};

use tidings::{File, Path, Revision, Store};

use super::record::{Base, Record};
use super::scan::{self, Scan};
use super::{Blocked, Local, LocalChange, NamedPath, Selection, SyncEvent, Target, WorkingCopy};
use crate::failure::Failure;

/// What `discard` did: nothing, if there were no local changes to throw away.
#[derive(Debug)]
pub struct DiscardReport {
    /// Each Path whose local change was thrown away, in order.
    pub discarded: Vec<Discarded>,
    /// Each `theirs` file that couldn't be removed, as `sync` reports it. The Paths are no longer
    /// Diverged all the same.
    pub events: Vec<SyncEvent>,
}

/// A Path whose local change `discard` threw away.
#[derive(Debug)]
pub struct Discarded {
    /// The Path.
    pub path: Path,
    /// What was thrown away.
    pub change: DiscardedChange,
    /// The Revision of the Store's version the folder now holds, which is the new Base, or `None`
    /// if the Store has no File there.
    pub revision: Option<Revision>,
    /// Whether a local file was removed, since the Store has no File there.
    pub removed: bool,
}

/// What `discard` threw away at a Path.
#[derive(Debug, Clone, Copy)]
pub enum DiscardedChange {
    /// A local change, as `status` lists it.
    Local(LocalChange),
    /// A file that can't become a File.
    Invalid,
    /// The person's side of a Diverged Path.
    Diverged,
}

/// What `resolve` did.
#[derive(Debug)]
pub struct ResolveReport {
    /// Each Path that is no longer Diverged, in order.
    pub resolved: Vec<Resolved>,
    /// Each `theirs` file that couldn't be removed, as `sync` reports it. The Paths are no longer
    /// Diverged all the same.
    pub events: Vec<SyncEvent>,
}

/// A Path `resolve` settled.
#[derive(Debug)]
pub struct Resolved {
    /// The Path, which is no longer Diverged.
    pub path: Path,
    /// Its new Base: the Revision of the Store's version it was merged with, or `None` if the
    /// Store had removed the File.
    pub revision: Option<Revision>,
}

/// What `discard` will do at one Path, once it knows nothing blocks it.
struct Discarding {
    /// The Path.
    path: Path,
    /// What is thrown away there.
    change: DiscardedChange,
    /// What the Store holds there now.
    store_file: Option<File>,
    /// What the folder holds there now.
    local: Local,
}

impl WorkingCopy {
    /// Throws away the local changes at each Path that is modified, deleted, invalid or Diverged,
    /// or only at those at or under `named`, files or directories in the folder, relative to
    /// `current`, the current directory: the folder gets the Store's version as it is now,
    /// written or removed as `sync` would, which becomes the Base, and any Divergence is cleared
    /// and its `theirs` file removed.
    ///
    /// A file with no Base is left alone unless it is named, since the Store never had it: then
    /// it is removed, or replaced if the Store has a File there now. Files whose names can't be
    /// Paths are never touched, nor are ignored ones. Refused, changing nothing, if a named path
    /// covers nothing, or names an ignored file, or if what is in the folder keeps the Store's
    /// version of any Path from being put there.
    pub async fn discard(
        &self,
        store: &Store,
        named: &[PathBuf],
        current: &FsPath,
    ) -> Result<DiscardReport, Failure> {
        let mut lock = self.lock()?;
        let selection = self.select(named, current)?;
        let chosen = {
            let scan = scan::scan(&self.folder, &lock.record.bases)?;
            selection.check_each_names_something(&scan, &lock.record, "discard")?;
            discardable(&scan, &lock.record, &selection)
        };
        let mut discarding = Vec::new();
        let mut blocked = Vec::new();
        for (path, change) in chosen {
            let store_file = store.read(self.area, &path).await?;
            let on_the_way = self.blocking_directory(&path)?;
            // Under a symlink or a file, nothing in the folder can be at the Path.
            let local = if on_the_way.is_some() { Local::Absent } else { self.local(&path)? };
            match (&store_file, on_the_way, &local) {
                (Some(_), Some(on_the_way), _) => blocked.push((path, on_the_way)),
                (Some(_), None, Local::Directory) => {
                    blocked.push((path.clone(), Blocked::Directory(path)));
                }
                _ => discarding.push(Discarding { path, change, store_file, local }),
            }
        }
        if !blocked.is_empty() {
            return Err(Failure::blocked(blocked));
        }
        let record = &mut lock.record;
        let mut report = DiscardReport { discarded: Vec::new(), events: Vec::new() };
        for Discarding { path, change, store_file, local } in discarding {
            let removed = store_file.is_none() && matches!(local, Local::File(_) | Local::Other);
            if store_file.is_some() || removed {
                // Between checking the directories and applying through them, another process
                // could swap one for a symlink, as for `sync`, which isn't guarded against.
                if let Err(failure) = self.apply(&path, &local, store_file.as_ref()) {
                    // What was applied so far is recorded, since the folder holds it.
                    self.save(record)?;
                    return Err(failure);
                }
            }
            match &store_file {
                Some(file) => {
                    record.bases.insert(path.clone(), Base::of(file.revision(), file.contents()));
                }
                None => {
                    record.bases.remove(&path);
                }
            }
            record.divergences.remove(&path);
            report.events.extend(self.remove_theirs(&path));
            let revision = store_file.as_ref().map(File::revision);
            report.discarded.push(Discarded { path, change, revision, removed });
        }
        // The folder is changed first and the record saved after.
        self.save(record)?;
        Ok(report)
    }

    /// Takes what is in the folder at each of `named`, files in the folder relative to `current`,
    /// the current directory, as the merge of that Diverged Path: its Base becomes the Store's
    /// version recorded for its `theirs` file, the one it was merged with, or it has no Base if
    /// the Store had removed the File. The Divergence is cleared, the `theirs` file removed, and
    /// the folder left as it is. So the next commit goes through, unless the Store has changed
    /// the File again since, which it doesn't hide: the commit is then a Conflict.
    ///
    /// Refused, resolving nothing, unless each of `named` is a Diverged Path; a directory isn't
    /// one, so that nothing is marked resolved by mistake.
    pub async fn resolve(
        &self,
        store: &Store,
        named: &[PathBuf],
        current: &FsPath,
    ) -> Result<ResolveReport, Failure> {
        let mut lock = self.lock()?;
        let record = &mut lock.record;
        let named = match self.select(named, current)? {
            Selection::Named(named) => named,
            Selection::All => return Err(Failure::error("name each Diverged Path to resolve")),
        };
        let mut paths = BTreeSet::new();
        let mut refused = Vec::new();
        for NamedPath { given, target } in named {
            let path = match target {
                Target::Under(name) => name.path(),
                Target::Folder => None,
            };
            match path.filter(|path| record.divergences.contains_key(path)) {
                Some(path) => {
                    paths.insert(path);
                }
                None => refused.push(given.display().to_string()),
            }
        }
        if !refused.is_empty() {
            return Err(Failure::error(format!(
                "can't resolve what isn't Diverged: {}: `tidings status` lists each Diverged Path",
                refused.join(", ")
            )));
        }
        let mut report = ResolveReport { resolved: Vec::new(), events: Vec::new() };
        for path in paths {
            let Some(divergence) = record.divergences.remove(&path) else { continue };
            let revision = divergence.theirs_revision;
            match revision {
                Some(revision) => {
                    let base = self.base_of(store, &path, revision).await?;
                    record.bases.insert(path.clone(), base);
                }
                None => {
                    record.bases.remove(&path);
                }
            }
            report.events.extend(self.remove_theirs(&path));
            report.resolved.push(Resolved { path, revision });
        }
        self.save(record)?;
        Ok(report)
    }

    /// The Base of `path` at `revision`, with the hash of its contents if the Store still holds
    /// them. If it doesn't, they aren't known, so any local file counts as changed since the
    /// Base: were it taken as unchanged, `sync` would put the Store's newer version over it.
    async fn base_of(
        &self,
        store: &Store,
        path: &Path,
        revision: Revision,
    ) -> Result<Base, Failure> {
        let store_file = store.read(self.area, path).await?;
        Ok(match store_file.filter(|file| file.revision() == revision) {
            Some(file) => Base::of(revision, file.contents()),
            None => Base { revision, hash: None },
        })
    }

    /// Removes `path`'s `theirs` file, if there is one, as [`WorkingCopy::settle_theirs`] does,
    /// giving the error event reporting why it couldn't, if it couldn't.
    fn remove_theirs(&self, path: &Path) -> Option<SyncEvent> {
        let failure = self.settle_theirs(path, None, true).err()?;
        Some(SyncEvent::Error { path: path.clone(), message: failure.to_string() })
    }
}

/// Each Path in `scan` or `record` whose local change `discard` throws away, of those `selection`
/// covers, with what that change is: each that is modified, deleted, invalid or Diverged, and each
/// file with no Base if `selection` names paths.
fn discardable(
    scan: &Scan,
    record: &Record,
    selection: &Selection,
) -> BTreeMap<Path, DiscardedChange> {
    let named = matches!(selection, Selection::Named(_));
    let mut chosen = BTreeMap::new();
    for (path, difference) in scan.changes() {
        let change = difference.local_change();
        if selection.covers(path.as_str()) && (named || change != LocalChange::Added) {
            chosen.insert(path.clone(), DiscardedChange::Local(change));
        }
    }
    for path in scan.invalid.iter().filter_map(|unfit| unfit.name.path()) {
        if selection.covers(path.as_str()) && (named || record.bases.contains_key(&path)) {
            chosen.insert(path, DiscardedChange::Invalid);
        }
    }
    for path in record.divergences.keys().filter(|path| selection.covers(path.as_str())) {
        chosen.insert(path.clone(), DiscardedChange::Diverged);
    }
    chosen
}
