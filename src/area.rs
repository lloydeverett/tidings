/// One of the three independent parts of a [`Store`](crate::Store).
///
/// Each Area is separate from the others in the same way the platform's config, data and cache
/// directories are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Area {
    /// Settings, which people may also edit by hand.
    Config,
    /// The application's own data.
    Data,
    /// Files that may disappear at any time, because the user or the OS cleared them.
    Cache,
}

impl Area {
    /// Every Area, in order.
    pub(crate) const ALL: [Area; 3] = [Area::Config, Area::Data, Area::Cache];
}
