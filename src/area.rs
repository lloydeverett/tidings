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
    /// The Area's name in lower case, which names its directory under a Root override, and its
    /// database.
    #[cfg(feature = "sqlite")]
    pub(crate) fn name(self) -> &'static str {
        match self {
            Area::Config => "config",
            Area::Data => "data",
            Area::Cache => "cache",
        }
    }
}

/// One `T` for each Area. The only place that knows how Areas map to positions.
#[derive(Debug, Default)]
pub(crate) struct PerArea<T>([T; 3]);

impl<T> PerArea<T> {
    const AREAS: [Area; 3] = [Area::Config, Area::Data, Area::Cache];

    /// One `T` for each Area, made by `make`, or the first error it gives.
    #[cfg(feature = "sqlite")]
    pub(crate) fn try_from_fn(
        mut make: impl FnMut(Area) -> crate::Result<T>,
    ) -> crate::Result<PerArea<T>> {
        let [config, data, cache] = Self::AREAS;
        Ok(PerArea([make(config)?, make(data)?, make(cache)?]))
    }

    pub(crate) fn get(&self, area: Area) -> &T {
        &self.0[area as usize]
    }

    pub(crate) fn get_mut(&mut self, area: Area) -> &mut T {
        &mut self.0[area as usize]
    }

    /// Each Area with its `T`, in order of Area.
    pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = (Area, &mut T)> {
        Self::AREAS.into_iter().zip(&mut self.0)
    }
}
