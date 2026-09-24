use std::fmt;

use crate::path::refusal;
use crate::{Error, InvalidPathReason, Result};

/// The leading part of a Path, up to a `/`, known to be valid. It names a group of Files, such as
/// every File under `themes/`.
///
/// A Prefix is either empty, meaning the whole Area, or a valid Path followed by `/`. It is kept
/// in that form, with its `/`, so it can't be mistaken for the Path of a File, and so it matches
/// whole segments only: `themes/` covers `themes/dark.toml` but not `themes2/dark.toml`.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Prefix(String);

impl Prefix {
    /// Validates `prefix`, giving [`Error::InvalidPath`] if it is not allowed. Apart from its
    /// trailing `/`, it must follow the same rules as a [`Path`](crate::Path).
    pub fn new(prefix: impl Into<String>) -> Result<Prefix> {
        let prefix = prefix.into();
        if let Some(reason) = prefix_refusal(&prefix) {
            return Err(Error::InvalidPath { path: prefix, reason });
        }
        Ok(Prefix(prefix))
    }

    /// The Prefix as a string: empty, or ending with `/`.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Why `prefix` is refused, if it is.
fn prefix_refusal(prefix: &str) -> Option<InvalidPathReason> {
    if prefix.is_empty() {
        return None;
    }
    match prefix.strip_suffix('/') {
        None => Some(InvalidPathReason::NoTrailingSlash),
        // Just `/`: the root, not a part of the Area.
        Some("") => Some(InvalidPathReason::NotRelative),
        Some(path) => refusal(path),
    }
}

impl fmt::Debug for Prefix {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

impl fmt::Display for Prefix {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Anything an operation accepts as a Prefix: a [`Prefix`] that is already valid, or a string
/// that is validated when the operation is called.
pub trait IntoPrefix: sealed::Sealed {
    /// Validates `self` as a Prefix.
    fn into_prefix(self) -> Result<Prefix>;
}

impl IntoPrefix for Prefix {
    fn into_prefix(self) -> Result<Prefix> {
        Ok(self)
    }
}

impl IntoPrefix for &Prefix {
    fn into_prefix(self) -> Result<Prefix> {
        Ok(self.clone())
    }
}

impl IntoPrefix for &str {
    fn into_prefix(self) -> Result<Prefix> {
        Prefix::new(self)
    }
}

impl IntoPrefix for String {
    fn into_prefix(self) -> Result<Prefix> {
        Prefix::new(self)
    }
}

impl IntoPrefix for &String {
    fn into_prefix(self) -> Result<Prefix> {
        Prefix::new(self.as_str())
    }
}

mod sealed {
    pub trait Sealed {}
    impl Sealed for super::Prefix {}
    impl Sealed for &super::Prefix {}
    impl Sealed for &str {}
    impl Sealed for String {}
    impl Sealed for &String {}
}
