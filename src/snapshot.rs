use crate::backend::BackendSnapshot;
use crate::{File, IntoPath, IntoPrefix, Path, Result, Stat};

/// A view of one Area as it stood when it was taken, from
/// [`Store::snapshot`](crate::Store::snapshot). Reading several Files through it never mixes the
/// results of different Commits: Commits made after it was taken don't show in it.
///
/// Holding a Snapshot doesn't hold up Commits. It doesn't keep the Store open either: the Change
/// feed still ends once every Store handle has been dropped, and the Snapshot can still be read.
#[derive(Debug)]
pub struct Snapshot {
    snapshot: BackendSnapshot,
}

impl Snapshot {
    pub(crate) fn new(snapshot: BackendSnapshot) -> Snapshot {
        Snapshot { snapshot }
    }

    /// Reads the File at `path` as it was, or gives `Ok(None)` if there was none.
    pub async fn read(&self, path: impl IntoPath) -> Result<Option<File>> {
        let path = path.into_path()?;
        self.snapshot.read(&path).await
    }

    /// Gives when the File at `path` was last modified and its Revision, as they were, or
    /// `Ok(None)` if there was no File there.
    pub async fn stat(&self, path: impl IntoPath) -> Result<Option<Stat>> {
        let path = path.into_path()?;
        self.snapshot.stat(&path).await
    }

    /// Lists the Paths of the Files that were under `prefix`, in order. The empty Prefix lists the
    /// whole Area.
    pub async fn list(&self, prefix: impl IntoPrefix) -> Result<Vec<Path>> {
        let prefix = prefix.into_prefix()?;
        self.snapshot.list(&prefix).await
    }
}
