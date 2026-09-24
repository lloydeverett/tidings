use jiff::Timestamp;

use crate::{Path, Revision};

/// A File as it was read: its Path, its text contents, when it was last modified, and its
/// Revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct File {
    path: Path,
    contents: String,
    stat: Stat,
}

impl File {
    pub(crate) fn new(path: Path, contents: String, stat: Stat) -> File {
        File { path, contents, stat }
    }

    /// The File's Path within its Area.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The File's text contents.
    pub fn contents(&self) -> &str {
        &self.contents
    }

    /// When the File was last modified. Every File in a Commit gets the same time.
    pub fn modified(&self) -> Timestamp {
        self.stat.modified()
    }

    /// The Revision of the File as it was read.
    pub fn revision(&self) -> Revision {
        self.stat.revision()
    }
}

/// What [`Store::stat`](crate::Store::stat) tells about a File without loading its contents:
/// when it was last modified, and its Revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stat {
    modified: Timestamp,
    revision: Revision,
}

impl Stat {
    pub(crate) fn new(modified: Timestamp, revision: Revision) -> Stat {
        Stat { modified, revision }
    }

    /// When the File was last modified. Every File in a Commit gets the same time.
    pub fn modified(&self) -> Timestamp {
        self.modified
    }

    /// The File's current Revision.
    pub fn revision(&self) -> Revision {
        self.revision
    }
}
