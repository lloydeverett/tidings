//! Backend markers (ADR 0007): a Location records which Backend holds it, in `.tidings/backend`,
//! as `fs` or `sqlite` and a newline, so that a Store on another Backend refuses to open it,
//! rather than taking the other Backend's files for its own or missing its Files.
//!
//! **Unmarked Locations.** A Location whose directory, `.tidings/` or marker doesn't exist is
//! unmarked, and the first Store to open it marks it for its Backend, adopting whatever it holds.
//! A marker that names no Backend, or can't be read, is an error, and is never overwritten.
//!
//! **Opening.** A Store reads the marker before it makes anything, so that it fails without
//! changing anything if the Location belongs to another Backend. It then marks an unmarked
//! Location only if no marker has appeared since. A Store that finds one has reads it, and fails
//! if it names another Backend. So when Stores on two Backends open an unmarked Location at once,
//! the one that makes the marker gets it, and the other fails.
//!
//! **Nesting.** Before that, opening refuses a Location inside a directory holding a `.tidings/`,
//! another Store's Location or a Working copy ([`refuse_nested`]), so that two never claim the
//! same files.
//!
//! **Clearing.** A Location removed by the user or the OS loses its marker with everything else.
//! The Store open on it makes it and marks it again.

use std::fs;
use std::io::{self, Write};
use std::path::Component;
use std::path::{Path as FsPath, PathBuf};
use std::time::Duration;

use super::{failed, is_absent, sync_directory};
use crate::path::RESERVED;
use crate::{BackendKind, Error, Result};

/// The marker's name in [`RESERVED`].
const MARKER: &str = "backend";

/// How long to wait before reading again a marker found empty, each time it is, since another
/// Store may be writing it. Once they have all passed, it is an error.
const EMPTY_MARKER_DELAYS: [Duration; 4] = [
    Duration::from_millis(5),
    Duration::from_millis(20),
    Duration::from_millis(50),
    Duration::from_millis(200),
];

/// Where the marker of the Location `location` is.
fn marker_path(location: &FsPath) -> PathBuf {
    location.join(RESERVED).join(MARKER)
}

/// The Backend the marker at `path` names, or `None` if there is none.
///
/// It blocks, and sleeps while it waits for an empty marker to be written, so it runs only off
/// the async runtime: on tokio's blocking threads, or in the blocking API.
fn read(path: &FsPath) -> Result<Option<BackendKind>> {
    let mut delays = EMPTY_MARKER_DELAYS.into_iter();
    loop {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(failed(path, error)),
        };
        let other = text.trim_end();
        if let Some(kind) = BackendKind::named(other) {
            return Ok(Some(kind));
        }
        if other.is_empty() {
            // Another Store may have made it, and not yet written it.
            if let Some(delay) = delays.next() {
                std::thread::sleep(delay);
                continue;
            }
            let message = format!(
                "{} is empty: if no Store is opening the Location, remove it, and a Store will \
                 mark the Location again",
                path.display(),
            );
            return Err(Error::backend(message));
        }
        let message = format!("{} names no Backend: {other:?}", path.display());
        return Err(Error::backend(message));
    }
}

/// The Backend the Location `location` is marked for, or `None` if it isn't marked. Changes
/// nothing.
pub(crate) fn detect(location: &FsPath) -> Result<Option<BackendKind>> {
    read(&marker_path(location))
}

/// Marks the Location `location` for `kind`, as the module's doc describes, making it and its
/// `.tidings/` where they don't exist. Gives whether it was unmarked, and so has been adopted, or
/// [`Error::WrongBackend`], having changed nothing, if it is marked for another Backend.
pub(crate) fn claim(location: &FsPath, kind: BackendKind) -> Result<bool> {
    check(detect(location)?, kind)?;
    let directory = location.join(RESERVED);
    fs::create_dir_all(&directory).map_err(|error| failed(&directory, error))?;
    let path = directory.join(MARKER);
    let created = fs::OpenOptions::new().write(true).create_new(true).open(&path);
    let mut file = match created {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            check(read(&path)?, kind)?;
            return Ok(false);
        }
        Err(error) => return Err(failed(&path, error)),
    };
    let written = file.write_all(format!("{kind}\n").as_bytes());
    written.and_then(|()| file.sync_all()).map_err(|error| failed(&path, error))?;
    // So that the marker itself survives a power cut, not only what it holds.
    sync_directory(&directory)?;
    Ok(true)
}

/// Fails unless `marked`, a Location's marker, is unmarked or names `kind`.
fn check(marked: Option<BackendKind>, kind: BackendKind) -> Result<()> {
    match marked {
        Some(found) if found != kind => Err(Error::WrongBackend { found }),
        _ => Ok(()),
    }
}

/// Gives [`Error::NestedLocation`] if a directory above the Location `location` holds a
/// `.tidings/` directory, as another Store's Location and a Working copy do. Symlinks on the way
/// are resolved first, so that the Location is checked where it really is. The Location holding
/// one itself is usual. Changes nothing.
pub(crate) fn refuse_nested(location: &FsPath) -> Result<()> {
    let resolved = resolved(location)?;
    for outer in resolved.ancestors().skip(1) {
        if holds_tidings(outer)? {
            return Err(Error::NestedLocation { outer: outer.to_path_buf() });
        }
    }
    Ok(())
}

/// Whether `directory` holds a `.tidings/` directory, as a Store's Location and a Working copy do.
/// Nothing there, or a file where a directory on the way would be, is `false`, and any other
/// failure an error, so that a `.tidings/` that can't be looked at is never taken for none.
pub(crate) fn holds_tidings(directory: &FsPath) -> Result<bool> {
    let tidings = directory.join(RESERVED);
    match fs::metadata(&tidings) {
        Ok(metadata) => Ok(metadata.is_dir()),
        Err(error) if is_absent(&error) => Ok(false),
        Err(error) => Err(failed(&tidings, error)),
    }
}

/// `location`, made absolute, with every symlink on the way resolved, though it may not exist
/// yet: the nearest of it and its ancestors that exists, as [`fs::canonicalize`] gives it, then
/// the names after that, which can't be symlinks, since they don't exist.
fn resolved(location: &FsPath) -> Result<PathBuf> {
    let mut existing = std::path::absolute(location).map_err(|error| failed(location, error))?;
    let mut missing = Vec::new();
    let mut resolved = loop {
        match fs::canonicalize(&existing) {
            Ok(canonical) => break canonical,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let last = existing.components().next_back().map(Component::as_os_str);
                let Some(last) = last.map(ToOwned::to_owned) else {
                    return Err(failed(&existing, error));
                };
                if !existing.pop() {
                    return Err(failed(&existing, error));
                }
                missing.push(last);
            }
            Err(error) => return Err(failed(&existing, error)),
        }
    };
    for name in missing.into_iter().rev() {
        if name == ".." {
            resolved.pop();
        } else {
            resolved.push(name);
        }
    }
    Ok(resolved)
}
