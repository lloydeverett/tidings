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

#[derive(Debug, Default)]
struct Areas {
    config: BTreeMap<Path, Stored>,
    data: BTreeMap<Path, Stored>,
    cache: BTreeMap<Path, Stored>,
}

/// A File as the memory Backend keeps it.
#[derive(Debug)]
struct Stored {
    contents: String,
    modified: Timestamp,
    revision: Revision,
}

impl Areas {
    fn get(&self, area: Area) -> &BTreeMap<Path, Stored> {
        match area {
            Area::Config => &self.config,
            Area::Data => &self.data,
            Area::Cache => &self.cache,
        }
    }

    fn get_mut(&mut self, area: Area) -> &mut BTreeMap<Path, Stored> {
        match area {
            Area::Config => &mut self.config,
            Area::Data => &mut self.data,
            Area::Cache => &mut self.cache,
        }
    }
}

impl MemoryBackend {
    pub(crate) fn read(&self, area: Area, path: &Path) -> Option<File> {
        let areas = self.areas.lock().unwrap();
        let stored = areas.get(area).get(path)?;
        Some(File::new(path.clone(), stored.contents.clone(), stored.modified, stored.revision))
    }

    /// Applies every write at once, under the one lock.
    pub(crate) fn commit(&self, request: CommitRequest) -> Written {
        let CommitRequest { area, timestamp, writes } = request;
        let mut areas = self.areas.lock().unwrap();
        let files = areas.get_mut(area);
        let mut written = Written::new();
        for (path, contents) in writes {
            let revision = Revision::of(&contents);
            files.insert(path.clone(), Stored { contents, modified: timestamp, revision });
            written.insert(path, revision);
        }
        written
    }
}
