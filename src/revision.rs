use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;

use xxhash_rust::xxh3::{Xxh3, xxh3_128};

use crate::{Area, Path, Prefix};

/// An opaque value identifying one state of a File, as returned when it is read.
///
/// Two Files with the same contents have the same Revision, on every Backend: a Revision is a
/// 128-bit hash (XXH3) of the contents. So a File that is changed and then changed back counts
/// as unchanged.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Revision(u128);

impl Revision {
    /// The Revision of a File with these contents.
    pub(crate) fn of(contents: &str) -> Revision {
        Revision(xxh3_128(contents.as_bytes()))
    }
}

impl fmt::Debug for Revision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Revision({:032x})", self.0)
    }
}

/// An opaque value identifying the state of everything under a Prefix: which Paths exist there,
/// and the Revision of each. It is given by [`Store::stat_prefix`](crate::Store::stat_prefix), and
/// [`Staging::require_prefix`](crate::Staging::require_prefix) makes a Commit depend on it.
///
/// Adding, removing or changing a File under the Prefix changes it. Two Prefix Revisions are equal
/// when they were taken for the same Area and Prefix, and cover the same Paths with the same
/// Revisions.
///
/// It keeps a list of the Paths it covers and their Revisions, so that a Conflict can name the
/// ones that changed. Holding one therefore costs memory in proportion to the number of Files
/// under its Prefix.
#[derive(Clone)]
pub struct PrefixRevision {
    /// The Area and Prefix it was taken for.
    area: Area,
    prefix: Prefix,
    /// A 128-bit hash (XXH3) over each Path and its Revision, in order of Path.
    hash: u128,
    /// What the hash was taken over. It is kept only so that a Conflict can name the Paths that
    /// differ, and is never shown to the app.
    files: Arc<[(Path, Revision)]>,
}

impl PrefixRevision {
    /// The Prefix Revision of `prefix` in `area`, given the Files under it in order of Path.
    pub(crate) fn of(area: Area, prefix: Prefix, files: Vec<(Path, Revision)>) -> PrefixRevision {
        PrefixRevision { area, prefix, hash: hash(&files), files: files.into() }
    }

    /// The Prefix it was taken for.
    pub(crate) fn prefix(&self) -> &Prefix {
        &self.prefix
    }

    /// Whether it was taken for `prefix` in `area`.
    pub(crate) fn is_for(&self, area: Area, prefix: &Prefix) -> bool {
        self.area == area && self.prefix == *prefix
    }

    /// The Paths added, removed or changed since, given the Files under its Prefix now, in order
    /// of Path.
    pub(crate) fn differences<'a>(&'a self, now: &'a [(Path, Revision)]) -> BTreeSet<Path> {
        if hash(now) == self.hash {
            return BTreeSet::new();
        }
        let by_path = |files: &'a [(Path, Revision)]| -> BTreeMap<&'a Path, &'a Revision> {
            files.iter().map(|(path, revision)| (path, revision)).collect()
        };
        let (before, after) = (by_path(&self.files), by_path(now));
        let paths = before.keys().chain(after.keys());
        paths
            .filter(|path| before.get(*path) != after.get(*path))
            .map(|path| (*path).clone())
            .collect()
    }
}

/// A 128-bit hash (XXH3) over each of `files` and its Revision, given in order of Path.
fn hash(files: &[(Path, Revision)]) -> u128 {
    let mut hasher = Xxh3::new();
    for (path, revision) in files {
        hasher.update(path.as_str().as_bytes());
        // A Path can't contain a NUL, so it ends the Path unambiguously.
        hasher.update(&[0]);
        hasher.update(&revision.0.to_le_bytes());
    }
    hasher.digest128()
}

impl PartialEq for PrefixRevision {
    fn eq(&self, other: &Self) -> bool {
        (self.area, &self.prefix, self.hash) == (other.area, &other.prefix, other.hash)
    }
}

impl Eq for PrefixRevision {}

impl std::hash::Hash for PrefixRevision {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        (self.area, &self.prefix, self.hash).hash(state);
    }
}

impl fmt::Debug for PrefixRevision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PrefixRevision({:?} {:?} {:032x})", self.area, self.prefix, self.hash)
    }
}
