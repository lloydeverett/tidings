use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;

use xxhash_rust::xxh3::{Xxh3, xxh3_128};

use crate::Path;

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
/// when they cover the same Paths with the same Revisions.
#[derive(Clone)]
pub struct PrefixRevision {
    /// A 128-bit hash (XXH3) over each Path and its Revision, in order of Path.
    hash: u128,
    /// What the hash was taken over. It is kept only so that a Conflict can name the Paths that
    /// differ, and is never shown.
    files: Arc<[(Path, Revision)]>,
}

impl PrefixRevision {
    /// The Prefix Revision of these Files, given in order of Path.
    pub(crate) fn of(files: Vec<(Path, Revision)>) -> PrefixRevision {
        let mut hasher = Xxh3::new();
        for (path, revision) in &files {
            hasher.update(path.as_str().as_bytes());
            // A Path can't contain a NUL, so it ends the Path unambiguously.
            hasher.update(&[0]);
            hasher.update(&revision.0.to_le_bytes());
        }
        PrefixRevision { hash: hasher.digest128(), files: files.into() }
    }

    /// The Paths added, removed or changed between `self` and `now`.
    pub(crate) fn differences<'a>(&'a self, now: &'a PrefixRevision) -> BTreeSet<Path> {
        if self == now {
            return BTreeSet::new();
        }
        let by_path = |files: &'a [(Path, Revision)]| -> BTreeMap<&'a Path, &'a Revision> {
            files.iter().map(|(path, revision)| (path, revision)).collect()
        };
        let (before, after) = (by_path(&self.files), by_path(&now.files));
        let paths = before.keys().chain(after.keys());
        paths
            .filter(|path| before.get(*path) != after.get(*path))
            .map(|path| (*path).clone())
            .collect()
    }
}

impl PartialEq for PrefixRevision {
    fn eq(&self, other: &Self) -> bool {
        self.hash == other.hash
    }
}

impl Eq for PrefixRevision {}

impl std::hash::Hash for PrefixRevision {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.hash.hash(state);
    }
}

impl fmt::Debug for PrefixRevision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PrefixRevision({:032x})", self.hash)
    }
}
