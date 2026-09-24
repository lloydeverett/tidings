use std::fmt;

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
        Revision(xxhash_rust::xxh3::xxh3_128(contents.as_bytes()))
    }
}

impl fmt::Debug for Revision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Revision({:032x})", self.0)
    }
}
