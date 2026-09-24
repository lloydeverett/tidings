//! The memory Backend: every Area lives in process memory, shared by every clone of the Store.

use std::collections::BTreeMap;
use std::sync::Mutex;

use jiff::Timestamp;

use super::{CommitRequest, Written};
use crate::{Area, File, Path, Revision};

#[derive(Debug, Default)]
pub(crate) struct MemoryBackend {
    areas: Mutex<Areas>,
}

/// Each Area's Files, indexed by `Area as usize`.
type Areas = [BTreeMap<Path, Stored>; 3];

/// A File as the memory Backend keeps it.
#[derive(Debug)]
struct Stored {
    contents: String,
    modified: Timestamp,
    revision: Revision,
}

impl MemoryBackend {
    pub(crate) fn read(&self, area: Area, path: &Path) -> Option<File> {
        let areas = self.areas.lock().unwrap();
        let stored = areas[area as usize].get(path)?;
        Some(File::new(path.clone(), stored.contents.clone(), stored.modified, stored.revision))
    }

    /// Applies every write at once, under the one lock.
    pub(crate) fn commit(&self, request: CommitRequest) -> Written {
        let CommitRequest { area, timestamp, writes } = request;
        let mut areas = self.areas.lock().unwrap();
        let files = &mut areas[area as usize];
        let mut written = Written::new();
        for (path, contents) in writes {
            let revision = Revision::of(&contents);
            files.insert(path.clone(), Stored { contents, modified: timestamp, revision });
            written.insert(path, revision);
        }
        written
    }
}
