use jiff::Timestamp;

use crate::{Path, Revision};

/// A File as it was read: its Path, its text contents, when it was last modified, and its
/// Revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct File {
    path: Path,
    contents: String,
    modified: Timestamp,
    revision: Revision,
}

impl File {
    pub(crate) fn new(
        path: Path,
        contents: String,
        modified: Timestamp,
        revision: Revision,
    ) -> File {
        File { path, contents, modified, revision }
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
        self.modified
    }

    /// The Revision of the File as it was read.
    pub fn revision(&self) -> Revision {
        self.revision
    }
}
