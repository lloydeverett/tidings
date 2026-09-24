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
    area: Area,
    staged: BTreeMap<Path, Staged>,
    /// Prefixes to delete everything under. A write or delete staged after the Prefix delete is
    /// in `staged`, and wins over it for that Path.
    prefix_deletes: BTreeSet<Prefix>,
}

/// What a Staging does to one Path.
#[derive(Debug)]
pub(crate) enum Staged {
    Write(String),
    Delete,
}

impl Staging {
    /// An empty Staging for `area`.
    pub fn new(area: Area) -> Staging {
        Staging { area, staged: BTreeMap::new(), prefix_deletes: BTreeSet::new() }
    }

    /// The Area this Staging writes to.
    pub fn area(&self) -> Area {
        self.area
    }

    /// Stages writing `contents` to `path`. It replaces anything staged for the same Path before.
    ///
    /// Gives [`Error::InvalidPath`](crate::Error::InvalidPath) if `path` is not allowed.
    pub fn write(&mut self, path: impl IntoPath, contents: impl Into<String>) -> Result<&mut Self> {
        self.staged.insert(path.into_path()?, Staged::Write(contents.into()));
        Ok(self)
    }

    /// Stages deleting the File at `path`. It replaces anything staged for the same Path before.
    /// If there is no File there when the Commit is made, the delete does nothing.
    ///
    /// Deleting one Path and writing another in the same Staging renames a File, all-or-nothing.
    ///
    /// Gives [`Error::InvalidPath`](crate::Error::InvalidPath) if `path` is not allowed.
    pub fn delete(&mut self, path: impl IntoPath) -> Result<&mut Self> {
        self.staged.insert(path.into_path()?, Staged::Delete);
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
        self.staged.retain(|path, _| !prefix.covers(path));
        self.prefix_deletes.insert(prefix);
        Ok(self)
    }

    /// What is staged for each Path, and the Prefixes to delete everything under.
    pub(crate) fn into_parts(self) -> (BTreeMap<Path, Staged>, BTreeSet<Prefix>) {
        (self.staged, self.prefix_deletes)
    }
}
