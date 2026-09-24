use std::collections::BTreeMap;

use crate::{Area, IntoPath, Path, Result};

/// An owned set of staged writes for one Area. Nothing happens until it is committed with
/// [`Store::commit`](crate::Store::commit).
///
/// A Staging is an ordinary value, built without the Store. Dropping it without committing
/// writes nothing.
#[derive(Debug)]
#[must_use = "a Staging writes nothing until it is committed"]
pub struct Staging {
    area: Area,
    writes: BTreeMap<Path, String>,
}

impl Staging {
    /// An empty Staging for `area`.
    pub fn new(area: Area) -> Staging {
        Staging { area, writes: BTreeMap::new() }
    }

    /// The Area this Staging writes to.
    pub fn area(&self) -> Area {
        self.area
    }

    /// Stages writing `contents` to `path`. A later write to the same Path replaces this one.
    ///
    /// Gives [`Error::InvalidPath`](crate::Error::InvalidPath) if `path` is not allowed.
    pub fn write(&mut self, path: impl IntoPath, contents: impl Into<String>) -> Result<&mut Self> {
        self.writes.insert(path.into_path()?, contents.into());
        Ok(self)
    }

    pub(crate) fn into_writes(self) -> BTreeMap<Path, String> {
        self.writes
    }
}
