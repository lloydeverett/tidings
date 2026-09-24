use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::backend::AreaState;
use crate::path::letter_case_key;
use crate::{
    Area, Error, File, IntoPath, IntoPrefix, InvalidPathReason, Path, Precondition, Prefix,
    PrefixRevision, Result,
};

/// An owned set of staged writes and deletes for one Area. Nothing happens until it is committed
/// with [`Store::commit`](crate::Store::commit).
///
/// A Staging is an ordinary value, built without the Store. Dropping it without committing
/// writes nothing.
///
/// A Commit can be made to depend on what was read, with a [`Precondition`] on a File it writes,
/// deletes or only [`require`](Self::require)s, and with a [`PrefixRevision`] on everything under
/// a Prefix. A Staging with none of them depends on nothing but the Paths it writes and deletes.
#[derive(Debug)]
#[must_use = "a Staging writes nothing until it is committed"]
pub struct Staging {
    staged: Staged,
}

/// Everything a Staging holds. The Staging builds it, and the Commit hands it to the Backend.
#[derive(Debug)]
pub(crate) struct Staged {
    pub(crate) area: Area,
    /// What to do to each Path. It wins over `prefix_deletes`, because anything staged under a
    /// Prefix before the Prefix delete was dropped from here.
    pub(crate) actions: BTreeMap<Path, Action>,
    /// Prefixes to delete everything under, expanded when the Commit runs.
    prefix_deletes: BTreeSet<Prefix>,
    /// Every Precondition staged other than *any*, on a Path written, deleted or only required.
    /// None is ever dropped: a later write or delete replaces the action for a Path, but not what
    /// the Staging required of it.
    requirements: Vec<(Path, Precondition)>,
    /// Each Prefix that must be unchanged since a Prefix Revision.
    prefix_requirements: Vec<(Prefix, PrefixRevision)>,
}

/// What a Staging does to one Path.
#[derive(Debug)]
pub(crate) enum Action {
    Write(String),
    Delete,
}

impl Staging {
    /// An empty Staging for `area`.
    pub fn new(area: Area) -> Staging {
        Staging {
            staged: Staged {
                area,
                actions: BTreeMap::new(),
                prefix_deletes: BTreeSet::new(),
                requirements: Vec::new(),
                prefix_requirements: Vec::new(),
            },
        }
    }

    /// The Area this Staging writes to.
    pub fn area(&self) -> Area {
        self.staged.area
    }

    /// Stages writing `contents` to `path`. It replaces anything staged for the same Path before,
    /// apart from any Precondition staged on it, which must still hold.
    ///
    /// Gives [`Error::InvalidPath`](crate::Error::InvalidPath) if `path` is not allowed.
    pub fn write(&mut self, path: impl IntoPath, contents: impl Into<String>) -> Result<&mut Self> {
        self.write_requiring(path, contents, Precondition::Any)
    }

    /// Stages writing `contents` to `path`, as [`write`](Self::write) does, if `precondition`
    /// holds for the File there when the Commit is made. If it doesn't, the Commit writes nothing
    /// and fails with [`Error::Conflict`](crate::Error::Conflict).
    ///
    /// The Precondition stays even if something staged later replaces this write.
    pub fn write_requiring(
        &mut self,
        path: impl IntoPath,
        contents: impl Into<String>,
        precondition: Precondition,
    ) -> Result<&mut Self> {
        Ok(self.stage(path.into_path()?, Action::Write(contents.into()), precondition))
    }

    /// Stages writing `contents` back to a File that was read, if it is unchanged since: the
    /// write requires [`Precondition::UnchangedSince`] the File's Revision. So an edit someone
    /// made after the read is never overwritten: the Commit fails with a Conflict instead.
    pub fn write_back(&mut self, file: &File, contents: impl Into<String>) -> &mut Self {
        let precondition = Precondition::UnchangedSince(file.revision());
        self.stage(file.path().clone(), Action::Write(contents.into()), precondition)
    }

    /// Stages deleting the File at `path`. It replaces anything staged for the same Path before,
    /// apart from any Precondition staged on it, which must still hold. If there is no File there
    /// when the Commit is made, the delete does nothing.
    ///
    /// Deleting one Path and writing another in the same Staging renames a File, all-or-nothing.
    ///
    /// Gives [`Error::InvalidPath`](crate::Error::InvalidPath) if `path` is not allowed.
    pub fn delete(&mut self, path: impl IntoPath) -> Result<&mut Self> {
        self.delete_requiring(path, Precondition::Any)
    }

    /// Stages deleting the File at `path`, as [`delete`](Self::delete) does, if `precondition`
    /// holds for the File there when the Commit is made. If it doesn't, the Commit writes nothing
    /// and fails with [`Error::Conflict`](crate::Error::Conflict).
    ///
    /// The Precondition stays even if something staged later replaces this delete.
    pub fn delete_requiring(
        &mut self,
        path: impl IntoPath,
        precondition: Precondition,
    ) -> Result<&mut Self> {
        Ok(self.stage(path.into_path()?, Action::Delete, precondition))
    }

    /// Stages deleting every File under `prefix`. Which Files those are is decided when the
    /// Commit is made, so it includes Files added after this call. The empty Prefix deletes
    /// everything in the Area.
    ///
    /// It replaces anything staged under `prefix` before, apart from the Preconditions staged
    /// there, which must still hold. A write or delete under `prefix` staged after it still
    /// happens.
    ///
    /// Gives [`Error::InvalidPath`](crate::Error::InvalidPath) if `prefix` is not allowed.
    pub fn delete_prefix(&mut self, prefix: impl IntoPrefix) -> Result<&mut Self> {
        let prefix = prefix.into_prefix()?;
        self.staged.actions.retain(|path, _| !prefix.covers(path));
        self.staged.prefix_deletes.insert(prefix);
        Ok(self)
    }

    /// Adds `precondition` on the File at `path`, which the Staging need not write or delete. If
    /// it doesn't hold when the Commit is made, the Commit writes nothing and fails with
    /// [`Error::Conflict`](crate::Error::Conflict). Every Precondition staged on a Path must hold.
    ///
    /// Use it for a File the Commit was worked out from: requiring it
    /// [`UnchangedSince`](Precondition::UnchangedSince) the Revision that was read makes the
    /// Commit fail if the File changed in the meantime.
    ///
    /// Gives [`Error::InvalidPath`](crate::Error::InvalidPath) if `path` is not allowed.
    pub fn require(
        &mut self,
        path: impl IntoPath,
        precondition: Precondition,
    ) -> Result<&mut Self> {
        self.staged.add_requirement(path.into_path()?, precondition);
        Ok(self)
    }

    /// Requires everything under `prefix` to be unchanged since `prefix_revision`, which
    /// [`Store::stat_prefix`](crate::Store::stat_prefix) gave for the same Area and Prefix. If a
    /// File under `prefix` was added, removed or changed since, including Files never read, the
    /// Commit writes nothing and fails with [`Error::Conflict`](crate::Error::Conflict), naming
    /// those Files.
    ///
    /// Gives [`Error::InvalidPath`](crate::Error::InvalidPath) if `prefix` is not allowed.
    pub fn require_prefix(
        &mut self,
        prefix: impl IntoPrefix,
        prefix_revision: PrefixRevision,
    ) -> Result<&mut Self> {
        self.staged.prefix_requirements.push((prefix.into_prefix()?, prefix_revision));
        Ok(self)
    }

    fn stage(&mut self, path: Path, action: Action, precondition: Precondition) -> &mut Self {
        self.staged.add_requirement(path.clone(), precondition);
        self.staged.actions.insert(path, action);
        self
    }

    pub(crate) fn into_staged(self) -> Staged {
        self.staged
    }
}

impl Staged {
    fn add_requirement(&mut self, path: Path, precondition: Precondition) {
        // *Any* requires nothing, so a Staging without Preconditions has nothing to check.
        if precondition != Precondition::Any {
            self.requirements.push((path, precondition));
        }
    }

    /// Checks every Precondition against `area`, as it is when the Commit runs. A Backend calls
    /// this under its lock, before it writes anything. Gives [`Error::Conflict`] with every Path
    /// where a Precondition fails. A Staging with no Preconditions reads nothing.
    pub(crate) fn check_preconditions(&self, area: &impl AreaState) -> Result<()> {
        let mut conflicts = BTreeSet::new();
        for (path, precondition) in &self.requirements {
            if !precondition.holds(area.revision(path)?) {
                conflicts.insert(path.clone());
            }
        }
        for (prefix, prefix_revision) in &self.prefix_requirements {
            let now = PrefixRevision::of(area.revisions_under(prefix)?);
            conflicts.extend(prefix_revision.differences(&now));
        }
        if conflicts.is_empty() {
            Ok(())
        } else {
            Err(Error::Conflict { paths: conflicts.into_iter().collect() })
        }
    }

    /// Turns the Prefix deletes into deletes of the Paths under them, given every Path that
    /// exists in the Area. A Backend calls this under its lock, when the Commit runs. A Path
    /// staged after the Prefix delete keeps its own action.
    pub(crate) fn expand_prefix_deletes<'a>(
        &mut self,
        existing: impl IntoIterator<Item = &'a Path>,
    ) {
        let prefixes = std::mem::take(&mut self.prefix_deletes);
        for path in existing {
            if prefixes.iter().any(|prefix| prefix.covers(path)) {
                self.actions.entry(path.clone()).or_insert(Action::Delete);
            }
        }
    }

    /// Refuses a write that would create a Path differing only in letter case from another Path
    /// in the Area after the Commit, or put it under a Prefix that does. `existing` is every Path
    /// in the Area. A Backend calls this under its lock, after expanding the Prefix deletes, so
    /// that a Path deleted in the same Commit doesn't count.
    pub(crate) fn refuse_letter_case_clashes<'a>(
        &'a self,
        existing: impl IntoIterator<Item = &'a Path>,
    ) -> Result<()> {
        let written = self.actions.iter().filter(|(_, action)| matches!(action, Action::Write(_)));
        let mut written = written.map(|(path, _)| path).peekable();
        if written.peek().is_none() {
            return Ok(());
        }
        let deleted = |path: &Path| matches!(self.actions.get(path), Some(Action::Delete));
        let mut names = HashMap::new();
        for path in existing.into_iter().filter(|path| !deleted(path)) {
            for name in names_in(path) {
                names.insert(letter_case_key(name), name);
            }
        }
        for path in written {
            for name in names_in(path) {
                match names.entry(letter_case_key(name)) {
                    Entry::Vacant(entry) => {
                        entry.insert(name);
                    }
                    Entry::Occupied(other) if *other.get() != name => {
                        let path = path.as_str().to_owned();
                        return Err(Error::InvalidPath {
                            path,
                            reason: InvalidPathReason::LetterCaseClash,
                        });
                    }
                    Entry::Occupied(_) => {}
                }
            }
        }
        Ok(())
    }
}

/// Every name `path` puts in the Area: each Prefix it is under, other than the empty one, and the
/// Path itself. For `a/b/c.txt`, that is `a/`, `a/b/` and `a/b/c.txt`.
fn names_in(path: &Path) -> impl Iterator<Item = &str> {
    let path = path.as_str();
    let prefixes = path.match_indices('/').map(|(slash, _)| &path[..=slash]);
    prefixes.chain(std::iter::once(path))
}
