//! Backend markers (ADR 0007): each Area's directory records which Backend holds it, in
//! `.tidings/backend`, as `fs` or `sqlite` and a newline, so that a Store on another Backend
//! refuses to open it, rather than taking the other Backend's files for its own or missing its
//! Files.
//!
//! **Unmarked Areas.** An Area whose directory, `.tidings/` or marker doesn't exist is unmarked,
//! and the first Store to open it marks it for its Backend, adopting whatever it holds. A marker
//! that names no Backend, or can't be read, is an error, and is never overwritten.
//!
//! **Opening.** A Store reads all three Areas' markers before it marks any, so that it fails
//! without changing anything if one belongs to another Backend. It then marks the unmarked ones,
//! in order of Area, each only if no marker has appeared since. A Store that finds one has
//! reads it, and fails if it names another Backend. So when Stores on two Backends open an
//! unmarked location at once, the one that marks the config Area gets the location, and the other
//! fails before it marks anything else.
//!
//! **Clearing.** A Cache cleared by the user or the OS loses its marker with everything else. It
//! is marked again the next time a Store opens, and meanwhile the other Areas' markers still keep
//! out the other Backend.

use std::fs;
use std::io::{self, Write};
use std::path::{Path as FsPath, PathBuf};
use std::time::Duration;

use super::{failed, sync_directory};
use crate::area::PerArea;
use crate::path::RESERVED;
use crate::{Area, BackendKind, Error, Result};

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

/// Where the marker of the Area whose root is `root` is.
fn marker_path(root: &FsPath) -> PathBuf {
    root.join(RESERVED).join(MARKER)
}

/// The Backend each Area's marker names, or `None` where it has none, given each Area's
/// directory. Changes nothing.
fn read_all(directories: &PerArea<PathBuf>) -> Result<PerArea<Option<BackendKind>>> {
    PerArea::try_from_fn(|area| read(&marker_path(directories.get(area))))
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
                "{} is empty: if no Store is opening the Area, remove it, and a Store will mark \
                 the Area again",
                path.display(),
            );
            return Err(Error::backend(message));
        }
        let message = format!("{} names no Backend: {other:?}", path.display());
        return Err(Error::backend(message));
    }
}

/// The one Backend the Areas in `directories` are marked for, or `None` if none is marked. Gives
/// [`Error::MixedBackends`] if they are marked for different Backends. Changes nothing.
pub(crate) fn detect(directories: &PerArea<PathBuf>) -> Result<Option<BackendKind>> {
    let marked: Vec<(Area, BackendKind)> = read_all(directories)?
        .iter()
        .filter_map(|(area, marked)| marked.map(|kind| (area, kind)))
        .collect();
    match marked.first() {
        None => Ok(None),
        Some(&(_, first)) if marked.iter().all(|&(_, kind)| kind == first) => Ok(Some(first)),
        Some(_) => Err(Error::MixedBackends { marked }),
    }
}

/// Marks every Area in `directories` for `kind`, as the module's doc describes, making each
/// directory and its `.tidings/` where they don't exist. Gives the Areas that were unmarked, and
/// so have been adopted, or [`Error::WrongBackend`] for the first Area marked for another Backend.
pub(crate) fn claim(directories: &PerArea<PathBuf>, kind: BackendKind) -> Result<Vec<Area>> {
    let markers = read_all(directories)?;
    for (area, marked) in markers.iter() {
        check(area, *marked, kind)?;
    }
    let mut adopted = Vec::new();
    for (area, marked) in markers.iter() {
        if marked.is_none() && mark(area, directories.get(area), kind)? {
            adopted.push(area);
        }
    }
    Ok(adopted)
}

/// Fails unless `marked`, `area`'s marker, is unmarked or names `kind`.
fn check(area: Area, marked: Option<BackendKind>, kind: BackendKind) -> Result<()> {
    match marked {
        Some(found) if found != kind => Err(Error::WrongBackend { area, found }),
        _ => Ok(()),
    }
}

/// Marks `area`, whose root is `root`, for `kind`, unless a marker has appeared since it was
/// read, which must then name `kind`. Gives whether it marked it.
fn mark(area: Area, root: &FsPath, kind: BackendKind) -> Result<bool> {
    let directory = root.join(RESERVED);
    fs::create_dir_all(&directory).map_err(|error| failed(&directory, error))?;
    let path = directory.join(MARKER);
    let created = fs::OpenOptions::new().write(true).create_new(true).open(&path);
    let mut file = match created {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            check(area, read(&path)?, kind)?;
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
