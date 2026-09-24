use crate::Revision;

/// Something a Staging requires of a File when it is committed. If it doesn't hold, the Commit
/// writes nothing and fails with [`Error::Conflict`](crate::Error::Conflict).
///
/// A Staging can also require everything under a Prefix to be unchanged, with a
/// [`PrefixRevision`](crate::PrefixRevision) and
/// [`Staging::require_prefix`](crate::Staging::require_prefix).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Precondition {
    /// Whatever is there. Requires nothing, so it never fails.
    #[default]
    Any,
    /// There is no File at the Path.
    Absent,
    /// The File has the contents it had at this Revision. A File that was removed, or changed
    /// and not changed back, fails it.
    UnchangedSince(Revision),
}

impl Precondition {
    /// Whether it holds for a File whose Revision is `current`, or `None` if there is no File.
    pub(crate) fn holds(self, current: Option<Revision>) -> bool {
        match self {
            Precondition::Any => true,
            Precondition::Absent => current.is_none(),
            Precondition::UnchangedSince(revision) => current == Some(revision),
        }
    }
}
