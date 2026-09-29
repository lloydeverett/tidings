//! Settling local changes in a Working copy: `discard` throws the person's side away and takes
//! the Store's version, and `resolve` takes what is in the folder as the merge of a Diverged Path,
//! against the Store's version in `theirs`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path as FsPath, PathBuf};

use tidings::{File, Path, Revision, Store};

use super::record::{Base, Record};
use super::scan::{self, Scan};
use super::{
    Blocked, Local, LocalChange, NamedPath, Naming, RECORD_DIRECTORY, Selection, SyncEvent, Target,
    WorkingCopy, blocking_directory, theirs_file,
};
use crate::failure::Failure;

/// What `discard` did: nothing, if there were no local changes to throw away.
#[derive(Debug)]
pub struct DiscardReport {
    /// Each Path whose local change was thrown away, in order.
    pub discarded: Vec<Discarded>,
    /// Each `theirs` file that couldn't be removed, as `sync` reports it. Their Paths are no
    /// longer Diverged all the same.
    pub events: Vec<SyncEvent>,
}

/// A Path whose local change `discard` threw away.
#[derive(Debug)]
pub struct Discarded {
    /// The Path.
    pub path: Path,
    /// What was thrown away.
    pub change: DiscardedChange,
    /// What the folder holds there now.
    pub outcome: DiscardOutcome,
}

/// What the folder holds at a Path `discard` threw a local change away at.
#[derive(Debug, Clone, Copy)]
pub enum DiscardOutcome {
    /// The Store's version, of this Revision, which is the new Base.
    TookStoresVersion(Revision),
    /// Nothing, since the Store has no File there: the local file was removed.
    Removed,
    /// Nothing, as the Store has no File there either.
    Absent,
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
    /// Each `theirs` file that couldn't be removed, as `sync` reports it. Their Paths are no
    /// longer Diverged all the same.
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
    /// A file with no Base is left alone unless it is named itself, not only a directory it is in,
    /// since the Store never had it: then it is removed, or replaced if the Store has a File there
    /// now. Files whose names can't be Paths are never touched, nor are ignored ones. Afterwards,
    /// every stale `theirs` file is removed, as [`WorkingCopy::remove_stale_theirs`] says.
    /// Refused, changing nothing, if a named path covers nothing, or names an ignored file or one
    /// whose name can't be a Path, or if what is in the folder keeps the Store's version of any
    /// Path from being put there.
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
            selection.check_each_names_something(&scan, &lock.record, Naming::Discard)?;
            refuse_unfit_names(&scan, &selection)?;
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
            let outcome = match &store_file {
                Some(file) => DiscardOutcome::TookStoresVersion(file.revision()),
                None if matches!(local, Local::File(_) | Local::Other) => DiscardOutcome::Removed,
                None => DiscardOutcome::Absent,
            };
            if !matches!(outcome, DiscardOutcome::Absent) {
                // Between checking the directories and applying through them, another process
                // could swap one for a symlink, as for `sync`, which isn't guarded against.
                if let Err(failure) = self.apply(&path, &local, store_file.as_ref()) {
                    // What was applied so far is recorded, since the folder holds it.
                    self.save(record)?;
                    return Err(failure);
                }
            }
            record.set_base(&path, store_file.as_ref().map(Base::of_file));
            record.divergences.remove(&path);
            report.discarded.push(Discarded { path, change, outcome });
        }
        report.events = self.remove_stale_theirs(record);
        // The folder is changed first and the record saved after.
        self.save(record)?;
        Ok(report)
    }

    /// Takes what is in the folder at each of `named`, files in the folder relative to `current`,
    /// the current directory, as the merge of that Diverged Path: its Base becomes the Store's
    /// version recorded for its `theirs` file, the one it was merged with, or it has no Base if
    /// the Store had removed the File. The Divergence is cleared, the folder left as it is, and
    /// every stale `theirs` file removed, as [`WorkingCopy::remove_stale_theirs`] says. So the
    /// next commit goes through, unless the Store has changed the File again since, which it
    /// doesn't hide: the commit is then a Conflict.
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
            let base = match revision {
                Some(revision) => Some(self.base_of(store, &path, revision).await?),
                None => None,
            };
            record.set_base(&path, base);
            report.resolved.push(Resolved { path, revision });
        }
        report.events = self.remove_stale_theirs(record);
        self.save(record)?;
        Ok(report)
    }

    /// The Base of the Diverged `path` at `revision`, the Revision recorded for its `theirs` file,
    /// with the hash of its contents: those the Store holds, if it is still at `revision`, or else
    /// those of the `theirs` file, which `sync` writes whenever it records a new Revision for it.
    /// (Only a crash between the two, or the person editing it, leaves it holding other contents,
    /// and then a local file holding them counts as unchanged since a Base that is older than they
    /// are, so the Store's version is put over it, which loses nothing it doesn't have.) If there
    /// is no `theirs` file either, the contents aren't known, so any local file counts as changed
    /// since the Base: were it taken as unchanged, `sync` would put the Store's newer version over
    /// it.
    async fn base_of(
        &self,
        store: &Store,
        path: &Path,
        revision: Revision,
    ) -> Result<Base, Failure> {
        let store_file = store.read(self.area, path).await?;
        let contents = match store_file.filter(|file| file.revision() == revision) {
            Some(file) => Some(file.contents().to_owned()),
            None => self.read_theirs(path),
        };
        Ok(match contents {
            Some(contents) => Base::of(revision, &contents),
            None => Base { revision, hash: None },
        })
    }

    /// What `path`'s `theirs` file holds, if it is a regular file holding text, reached through no
    /// symlink.
    fn read_theirs(&self, path: &Path) -> Option<String> {
        let at = self.folder.join(theirs_file(path));
        let blocked = blocking_directory(&self.folder.join(RECORD_DIRECTORY), &at).ok()?;
        let is_file = fs::symlink_metadata(&at).is_ok_and(|metadata| metadata.is_file());
        if blocked.is_some() || !is_file {
            return None;
        }
        fs::read_to_string(&at).ok()
    }
}

/// Fails if `selection` names a file itself whose name in the folder can't be a Path, which
/// `discard` never touches, so that it isn't taken for one with nothing to discard.
fn refuse_unfit_names(scan: &Scan, selection: &Selection) -> Result<(), Failure> {
    let Selection::Named(named) = selection else { return Ok(()) };
    for NamedPath { given, target } in named {
        let Target::Under(name) = target else { continue };
        if name.path().is_some() {
            continue;
        }
        if let Some(unfit) = scan.invalid.iter().find(|unfit| unfit.name == *name) {
            return Err(Failure::error(format!(
                "{}: {}, so it is no File's and discard never touches it: rename or remove it \
                 yourself",
                given.display(),
                unfit.reason
            )));
        }
    }
    Ok(())
}

/// Each Path in `scan` or `record` whose local change `discard` throws away, of those `selection`
/// covers, with what that change is: each that is modified, deleted, invalid or Diverged, and each
/// file with no Base that `selection` names itself.
fn discardable(
    scan: &Scan,
    record: &Record,
    selection: &Selection,
) -> BTreeMap<Path, DiscardedChange> {
    let has_base = |path: &Path| record.bases.contains_key(path);
    let chosen_at = |path: &Path| {
        selection.covers(path.as_str()) && (has_base(path) || selection.names_itself(path.as_str()))
    };
    let mut chosen = BTreeMap::new();
    for (path, difference) in scan.changes() {
        if chosen_at(path) {
            chosen.insert(path.clone(), DiscardedChange::Local(difference.local_change()));
        }
    }
    for path in scan.invalid.iter().filter_map(|unfit| unfit.name.path()) {
        if chosen_at(&path) {
            chosen.insert(path, DiscardedChange::Invalid);
        }
    }
    for path in record.divergences.keys().filter(|path| selection.covers(path.as_str())) {
        chosen.insert(path.clone(), DiscardedChange::Diverged);
    }
    chosen
}
