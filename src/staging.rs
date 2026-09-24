use std::collections::{BTreeMap, BTreeSet};

use crate::{Area, IntoPath, IntoPrefix, Path, Prefix, Result};

/// An owned set of staged writes and deletes for one Area. Nothing happens until it is committed
/// with [`Store::commit`](crate::Store::commit).
///
/// A Staging is an ordinary value, built without the Store. Dropping it without committing
/// writes nothing.
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
            staged: Staged { area, actions: BTreeMap::new(), prefix_deletes: BTreeSet::new() },
        }
    }

    /// The Area this Staging writes to.
    pub fn area(&self) -> Area {
        self.staged.area
    }

    /// Stages writing `contents` to `path`. It replaces anything staged for the same Path before.
    ///
    /// Gives [`Error::InvalidPath`](crate::Error::InvalidPath) if `path` is not allowed.
    pub fn write(&mut self, path: impl IntoPath, contents: impl Into<String>) -> Result<&mut Self> {
        self.staged.actions.insert(path.into_path()?, Action::Write(contents.into()));
        Ok(self)
    }

    /// Stages deleting the File at `path`. It replaces anything staged for the same Path before.
    /// If there is no File there when the Commit is made, the delete does nothing.
    ///
    /// Deleting one Path and writing another in the same Staging renames a File, all-or-nothing.
    ///
    /// Gives [`Error::InvalidPath`](crate::Error::InvalidPath) if `path` is not allowed.
    pub fn delete(&mut self, path: impl IntoPath) -> Result<&mut Self> {
        self.staged.actions.insert(path.into_path()?, Action::Delete);
        Ok(self)
    }

    /// Stages deleting every File under `prefix`. Which Files those are is decided when the
    /// Commit is made, so it includes Files added after this call. The empty Prefix deletes
    /// everything in the Area.
    ///
    /// It replaces anything staged under `prefix` before. A write or delete under `prefix` staged
    /// after it still happens.
    ///
    /// Gives [`Error::InvalidPath`](crate::Error::InvalidPath) if `prefix` is not allowed.
    pub fn delete_prefix(&mut self, prefix: impl IntoPrefix) -> Result<&mut Self> {
        let prefix = prefix.into_prefix()?;
        self.staged.actions.retain(|path, _| !prefix.covers(path));
        self.staged.prefix_deletes.insert(prefix);
        Ok(self)
    }

    pub(crate) fn into_staged(self) -> Staged {
        self.staged
    }
}

impl Staged {
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
}
