use std::fmt;

use relative_path::{Component, RelativePath};

use crate::{Error, Result};

/// A File's name within its Area, known to be valid.
///
/// A Path follows the strictest platform's rules, on every Backend, so a Path that works on one
/// platform works on all of them. It is relative and `/`-separated, with no empty, `.` or `..`
/// segments. Each segment is a name Windows accepts, the whole Path is in Unicode NFC form, and
/// nothing is under `.tidings/`, which tidings keeps for itself. [`InvalidPathReason`] lists the
/// rules.
///
/// It can only be made by validating a string, with [`Path::new`] or through [`IntoPath`].
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

/// The top-level name tidings keeps its own bookkeeping under, in upper case.
const RESERVED_UPPERCASE: &str = ".TIDINGS";

/// Why `path` is refused, if it is.
pub(crate) fn refusal(path: &str) -> Option<InvalidPathReason> {
    let segments = RelativePath::new(path).components();
    if path.is_empty() {
        Some(InvalidPathReason::Empty)
    } else if path.starts_with('/') {
        Some(InvalidPathReason::NotRelative)
    } else if segments.clone().count() != path.split('/').count() {
        // relative-path skips empty segments, so each one shows up here as a segment missing.
        Some(InvalidPathReason::EmptySegment)
    } else if segments.clone().any(|segment| !matches!(segment, Component::Normal(_))) {
        Some(InvalidPathReason::DotSegment)
    } else if segments.clone().any(|segment| !is_portable_name(segment.as_str())) {
        Some(InvalidPathReason::UnportableName)
    } else if segments.clone().next().is_some_and(|first| is_reserved(first.as_str())) {
        Some(InvalidPathReason::Reserved)
    } else if !unicode_normalization::is_nfc(path) {
        Some(InvalidPathReason::NotNfc)
    } else {
        None
    }
}

/// Whether every platform accepts `name` as one segment of a Path, judged by the strictest rules
/// there are: Windows'. sanitize-filename does the checking, with the supplement below.
fn is_portable_name(name: &str) -> bool {
    is_sanitized_for_windows(name)
        && !name.chars().any(char::is_control)
        && !is_device_name_sanitize_filename_misses(name)
}

fn is_sanitized_for_windows(name: &str) -> bool {
    let windows_rules = sanitize_filename::OptionsForCheck { windows: true, truncate: true };
    sanitize_filename::is_sanitized_with_options(name, windows_rules)
}

// The supplement: names Windows refuses that sanitize-filename (0.6, and 0.7.0-beta too) lets
// through. Only these documented gaps are covered here; everything else is the crate's. Control
// characters are covered above with `char::is_control`, because the crate misses DEL (U+007F).

/// Device names Windows reserves that sanitize-filename doesn't list: the ports with superscript
/// digits, and the console's own names. In upper case.
const DEVICE_NAMES_SANITIZE_FILENAME_MISSES: &[&str] = &[
    "COM\u{b9}",
    "COM\u{b2}",
    "COM\u{b3}",
    "LPT\u{b9}",
    "LPT\u{b2}",
    "LPT\u{b3}",
    "CONIN$",
    "CONOUT$",
];

/// Whether Windows reads `name` as a device, in a way sanitize-filename misses. Windows ignores an
/// extension and any spaces before it, so `NUL.txt` and `NUL .txt` are both `NUL`; the crate knows
/// only the first. So its check is repeated on the name as Windows reads it.
fn is_device_name_sanitize_filename_misses(name: &str) -> bool {
    let device = name.split_once('.').map_or(name, |(base, _)| base).trim_end_matches(' ');
    DEVICE_NAMES_SANITIZE_FILENAME_MISSES.contains(&device.to_uppercase().as_str())
        || !is_sanitized_for_windows(device)
}

/// Whether `name`, as a Path's first segment, is tidings' own. Letter case is ignored, because on a
/// case-insensitive filesystem `.Tidings` is the same directory. Comparing in upper case, as NTFS
/// does, also catches letters such as `ſ` that only match once uppercased.
fn is_reserved(name: &str) -> bool {
    name.to_uppercase() == RESERVED_UPPERCASE
}

/// Which rule a refused Path or Prefix breaks, as given by [`Error::InvalidPath`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum InvalidPathReason {
    /// The Path is the empty string.
    Empty,
    /// The Path starts with `/`.
    NotRelative,
    /// The Path has an empty segment, as in `a//b` or `a/`.
    EmptySegment,
    /// The Path has a `.` or `..` segment.
    DotSegment,
    /// A segment is not a name every platform accepts. Windows' rules decide: no reserved names
    /// such as `CON`, `aux.txt` or `COM¹`, none of `\ : * ? " < > |`, no control characters, no
    /// trailing dot or space, and at most 255 bytes.
    UnportableName,
    /// The Path is not in Unicode NFC form. macOS treats different forms of the same character as
    /// one name, so only one form is allowed.
    NotNfc,
    /// The Path is under `.tidings/`, which tidings keeps for itself.
    Reserved,
    /// A Prefix other than the empty one doesn't end with `/`.
    NoTrailingSlash,
}

impl fmt::Display for InvalidPathReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            InvalidPathReason::Empty => "it is empty",
            InvalidPathReason::NotRelative => "it is not relative",
            InvalidPathReason::EmptySegment => "it has an empty segment",
            InvalidPathReason::DotSegment => "it has a `.` or `..` segment",
            InvalidPathReason::UnportableName => "a segment is not a name every platform accepts",
            InvalidPathReason::NotNfc => "it is not in Unicode NFC form",
            InvalidPathReason::Reserved => {
                "it is under `.tidings/`, which tidings keeps for itself"
            }
            InvalidPathReason::NoTrailingSlash => "it is a Prefix that doesn't end with `/`",
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
