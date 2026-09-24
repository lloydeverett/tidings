//! The journal of ADR 0005: `.tidings/journal`, which says what a filesystem Commit is doing, so
//! that one a crash interrupts can be finished or discarded the next time the Area is locked.
//!
//! It lists each temporary file with the File it replaces, where the write goes after following
//! any symlink, and each Path the Commit deletes. The directories to make and remove follow from
//! those. It is in one of two states:
//! - **prepared**: the temporary files may be partly written, and the Commit hasn't happened. It
//!   is discarded: its temporary files are removed.
//! - **committed**: every temporary file is written and on disk, and the Commit has happened. It
//!   is finished: the deletes are made and the temporary files renamed. Each step can be done
//!   again after a crash part way through, so finishing twice is the same as finishing once.
//!
//! Each state is written with `atomic-write-file`, which writes a new file, forces it to disk and
//! renames it over the journal, so the journal is always whole. It is plain text: a first line
//! that names the format, a line with the state, then a line for each temporary file
//! (`replace`, the temporary file, the File it replaces) and each delete (`remove`, the Path),
//! with fields separated by tabs, and `\`, tabs and line breaks escaped.

use std::collections::BTreeSet;
use std::fs;
use std::io::{self, Write};
use std::path::{Path as FsPath, PathBuf};

use atomic_write_file::AtomicWriteFile;

use super::{failed, is_absent};
use crate::{Error, Path, Result};

/// The first line of every journal, which names its format.
const FORMAT: &str = "tidings journal 1";

/// A Commit in progress, as the journal records it.
#[derive(Debug)]
pub(super) struct Journal {
    state: State,
    /// Each temporary file, in order of the Path it writes.
    pub(super) replaces: Vec<Replace>,
    /// Each Path the Commit deletes.
    removes: Vec<Path>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Prepared,
    Committed,
}

/// A temporary file, and the File it replaces once the Commit has happened.
#[derive(Debug)]
pub(super) struct Replace {
    pub(super) temporary: PathBuf,
    /// Where the write goes: the File at the Path, or the File a symlink there points to.
    pub(super) target: PathBuf,
}

/// Finishes or discards the Commit in the journal in `tidings`, if there is one, for the Area
/// whose root is `root`. It must be called with the Area locked.
pub(super) fn recover(root: &FsPath, tidings: &FsPath) -> Result<()> {
    let Some(journal) = Journal::read(tidings)? else { return Ok(()) };
    match journal.state {
        State::Prepared => {
            tracing::debug!("discarding a Commit to {} a crash interrupted", root.display());
            journal.discard(tidings)
        }
        State::Committed => {
            tracing::debug!("finishing a Commit to {} a crash interrupted", root.display());
            journal.finish(root, tidings)
        }
    }
}

impl Journal {
    /// A journal of a Commit that replaces Files with `replaces` and deletes `removes`, before
    /// any temporary file is written.
    pub(super) fn prepared(replaces: Vec<Replace>, removes: Vec<Path>) -> Journal {
        Journal { state: State::Prepared, replaces, removes }
    }

    /// Writes the journal, whole, to `tidings`, and forces it to disk.
    pub(super) fn write(&self, tidings: &FsPath) -> Result<()> {
        let path = tidings.join("journal");
        let text = self.to_text()?;
        let write = || -> io::Result<()> {
            let mut file = AtomicWriteFile::open(&path)?;
            file.write_all(text.as_bytes())?;
            file.commit()
        };
        write().map_err(|error| failed(&path, error))
    }

    /// Writes the journal as `committed`: once this returns, the Commit has happened.
    pub(super) fn commit(&mut self, tidings: &FsPath) -> Result<()> {
        self.state = State::Committed;
        self.write(tidings)
    }

    /// Discards the Commit: removes its temporary files, then the journal.
    pub(super) fn discard(&self, tidings: &FsPath) -> Result<()> {
        for Replace { temporary, .. } in &self.replaces {
            match fs::remove_file(temporary) {
                Ok(()) => {}
                Err(error) if is_absent(&error) => {}
                Err(error) => return Err(failed(temporary, error)),
            }
        }
        remove(tidings)
    }

    /// Finishes the Commit in the Area whose root is `root`, then removes the journal:
    /// 1. deletes each File, and removes each directory that leaves empty, so that a File can
    ///    take the directory's name (`d/e` to `d`);
    /// 2. makes the directory each write goes in, now that a File that had its name is gone (`a`
    ///    to `a/b`), removes an empty directory that has the File's name, and renames the
    ///    temporary file over the File;
    /// 3. forces each directory changed to disk, so that the Commit is on disk before the journal
    ///    is removed.
    pub(super) fn finish(&self, root: &FsPath, tidings: &FsPath) -> Result<()> {
        let mut changed = BTreeSet::new();
        for path in &self.removes {
            let file = root.join(path.as_str());
            // Finishing again, the File may be gone already, or be a directory a write made.
            match fs::symlink_metadata(&file) {
                Ok(metadata) if !metadata.is_dir() => {
                    fs::remove_file(&file).map_err(|error| failed(&file, error))?;
                    changed.extend(file.parent().map(FsPath::to_path_buf));
                }
                Ok(_) => {}
                Err(error) if is_absent(&error) => {}
                Err(error) => return Err(failed(&file, error)),
            }
            remove_emptied_directories(root, &file, &mut changed);
        }
        for Replace { temporary, target } in &self.replaces {
            if let Some(directory) = target.parent() {
                fs::create_dir_all(directory).map_err(|error| failed(directory, error))?;
            }
            // A directory the deletes left with nothing but empty directories in it.
            if fs::symlink_metadata(target).is_ok_and(|metadata| metadata.is_dir()) {
                remove_empty_directories(target).map_err(|error| failed(target, error))?;
            }
            match fs::rename(temporary, target) {
                Ok(()) => {
                    changed.extend(temporary.parent().map(FsPath::to_path_buf));
                    changed.extend(target.parent().map(FsPath::to_path_buf));
                }
                // Finishing again, it may be renamed already.
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(failed(temporary, error)),
            }
        }
        for directory in &changed {
            sync_directory(directory)?;
        }
        remove(tidings)
    }

    /// Reads the journal in `tidings`, if there is one.
    fn read(tidings: &FsPath) -> Result<Option<Journal>> {
        let path = tidings.join("journal");
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if is_absent(&error) => return Ok(None),
            Err(error) => return Err(failed(&path, error)),
        };
        let unreadable = |why: &str| {
            Error::backend(format!("the journal {} can't be read: {why}", path.display()))
        };
        let mut lines = text.lines();
        if lines.next() != Some(FORMAT) {
            return Err(unreadable("it isn't in a format this version of tidings knows"));
        }
        let state = match lines.next() {
            Some("prepared") => State::Prepared,
            Some("committed") => State::Committed,
            _ => return Err(unreadable("it has no state")),
        };
        let mut journal = Journal { state, replaces: Vec::new(), removes: Vec::new() };
        for line in lines {
            let fields: Vec<String> = line.split('\t').map(unescape).collect();
            match fields.as_slice() {
                [kind, temporary, target] if kind == "replace" => {
                    let (temporary, target) = (temporary.into(), target.into());
                    journal.replaces.push(Replace { temporary, target });
                }
                [kind, path] if kind == "remove" => {
                    let path =
                        Path::new(path.as_str()).map_err(|_| unreadable("a Path is invalid"))?;
                    journal.removes.push(path);
                }
                _ => return Err(unreadable(&format!("{line:?} isn't an item it can have"))),
            }
        }
        Ok(Some(journal))
    }

    /// The journal as the text [`read`](Self::read) reads.
    fn to_text(&self) -> Result<String> {
        let state = match self.state {
            State::Prepared => "prepared",
            State::Committed => "committed",
        };
        let mut text = format!("{FORMAT}\n{state}\n");
        for Replace { temporary, target } in &self.replaces {
            text += &format!("replace\t{}\t{}\n", escaped(temporary)?, escaped(target)?);
        }
        for path in &self.removes {
            text += &format!("remove\t{}\n", escape(path.as_str()));
        }
        Ok(text)
    }
}

/// Removes the journal in `tidings`.
///
/// That isn't forced to disk, which would cost every Commit another wait for the disk. If a power
/// cut loses it, the Commit is finished again when the Area is next locked, which changes nothing,
/// since every step of finishing can be done twice. Only a File that another program made since
/// at a Path the Commit deleted would be deleted again. The next Commit's journal replaces this
/// one on disk before that Commit changes anything.
fn remove(tidings: &FsPath) -> Result<()> {
    let path = tidings.join("journal");
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if is_absent(&error) => Ok(()),
        Err(error) => Err(failed(&path, error)),
    }
}

/// Removes the directory `file` was in, in the Area whose root is `root`, if that left it empty,
/// then the one above it if that leaves it empty, and so on up to the root, which stays. Each
/// directory that changed is added to `changed`.
fn remove_emptied_directories(root: &FsPath, file: &FsPath, changed: &mut BTreeSet<PathBuf>) {
    let mut directory = file.parent();
    while let Some(emptied) = directory.filter(|directory| directory.starts_with(root)) {
        if emptied == root {
            break;
        }
        match fs::remove_dir(emptied) {
            Ok(()) => {
                changed.remove(emptied);
                changed.extend(emptied.parent().map(FsPath::to_path_buf));
            }
            // Finishing again, it may be removed already.
            Err(error) if is_absent(&error) => {}
            // It isn't empty, or can't be removed: it stays, and so does everything above it.
            Err(_) => break,
        }
        directory = emptied.parent();
    }
}

/// Removes `directory`, which holds nothing but directories that hold nothing. It fails if there
/// is anything else in it.
fn remove_empty_directories(directory: &FsPath) -> io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            remove_empty_directories(&entry.path())?;
        }
    }
    fs::remove_dir(directory)
}

/// Forces the entries of `directory` to disk, where the platform can, so that a rename or delete
/// in it survives a power cut. A directory that is gone has nothing to force.
fn sync_directory(directory: &FsPath) -> Result<()> {
    #[cfg(unix)]
    match fs::File::open(directory).and_then(|opened| opened.sync_all()) {
        Ok(()) => {}
        Err(error) if is_absent(&error) => {}
        Err(error) => return Err(failed(directory, error)),
    }
    #[cfg(not(unix))]
    let _ = directory;
    Ok(())
}

/// `path` for the journal, escaped. It must be valid Unicode, since the journal is text.
fn escaped(path: &FsPath) -> Result<String> {
    let text = path.to_str().ok_or_else(|| {
        Error::backend(format!("{} isn't valid Unicode, so it can't be journaled", path.display()))
    })?;
    Ok(escape(text))
}

/// `text` with `\`, tabs and line breaks escaped, so that it is one field of one line.
fn escape(text: &str) -> String {
    text.replace('\\', "\\\\").replace('\t', "\\t").replace('\n', "\\n").replace('\r', "\\r")
}

/// The text [`escape`] escaped.
fn unescape(field: &str) -> String {
    let mut text = String::with_capacity(field.len());
    let mut chars = field.chars();
    while let Some(c) = chars.next() {
        let unescaped = if c == '\\' {
            match chars.next() {
                Some('t') => '\t',
                Some('n') => '\n',
                Some('r') => '\r',
                Some(other) => other,
                None => '\\',
            }
        } else {
            c
        };
        text.push(unescaped);
    }
    text
}
