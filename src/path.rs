use std::fmt;

use crate::{Error, Result};

/// A File's name within its Area, known to be valid.
///
/// A Path is relative and `/`-separated, with no empty segments. It can only be made by
/// validating a string, with [`Path::new`] or through [`IntoPath`].
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Path(String);

impl Path {
    /// Validates `path`, giving [`Error::InvalidPath`] if it is not allowed.
    pub fn new(path: impl Into<String>) -> Result<Path> {
        let path = path.into();
        if let Some(reason) = refusal(&path) {
            return Err(Error::InvalidPath { path, reason });
        }
        Ok(Path(path))
    }

    /// The Path as a string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Why `path` is refused, if it is.
fn refusal(path: &str) -> Option<InvalidPathReason> {
    if path.is_empty() {
        Some(InvalidPathReason::Empty)
    } else if path.starts_with('/') {
        Some(InvalidPathReason::NotRelative)
    } else if path.split('/').any(str::is_empty) {
        Some(InvalidPathReason::EmptySegment)
    } else {
        None
    }
}

/// Which rule a refused Path breaks, as given by [`Error::InvalidPath`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum InvalidPathReason {
    /// The Path is the empty string.
    Empty,
    /// The Path starts with `/`.
    NotRelative,
    /// The Path has an empty segment, as in `a//b` or `a/`.
    EmptySegment,
}

impl fmt::Display for InvalidPathReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            InvalidPathReason::Empty => "it is empty",
            InvalidPathReason::NotRelative => "it is not relative",
            InvalidPathReason::EmptySegment => "it has an empty segment",
        })
    }
}

impl fmt::Debug for Path {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

impl fmt::Display for Path {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Anything an operation accepts as a Path: a [`Path`] that is already valid, or a string that is
/// validated when the operation is called.
pub trait IntoPath: sealed::Sealed {
    /// Validates `self` as a Path.
    fn into_path(self) -> Result<Path>;
}

impl IntoPath for Path {
    fn into_path(self) -> Result<Path> {
        Ok(self)
    }
}

impl IntoPath for &Path {
    fn into_path(self) -> Result<Path> {
        Ok(self.clone())
    }
}

impl IntoPath for &str {
    fn into_path(self) -> Result<Path> {
        Path::new(self)
    }
}

impl IntoPath for String {
    fn into_path(self) -> Result<Path> {
        Path::new(self)
    }
}

impl IntoPath for &String {
    fn into_path(self) -> Result<Path> {
        Path::new(self.as_str())
    }
}

mod sealed {
    pub trait Sealed {}
    impl Sealed for super::Path {}
    impl Sealed for &super::Path {}
    impl Sealed for &str {}
    impl Sealed for String {}
    impl Sealed for &String {}
}
