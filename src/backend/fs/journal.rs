//! The journal of ADR 0005: `.tidings/journal`, which says what a filesystem Commit is doing, so
//! that one a crash interrupts can be finished or discarded the next time the Area is locked.
//!
//! It lists each temporary file with the Path it writes and the File it replaces, and each Path
//! the Commit deletes with the Revision of the File it deletes. A File a write replaces is either
//! the File at its Path, whose directories finishing may make, or, for a write through a symlink,
//! the file the link points to, whose directory must exist. It is in one of two states:
//! - **prepared**: the temporary files may be partly written, and the Commit hasn't happened. It
//!   is discarded: its temporary files are removed.
//! - **committed**: every temporary file is written and on disk, and the Commit has happened. It
//!   is finished: the deletes are made and the temporary files renamed. Until it is, reads through
//!   tidings see the Area as it will be once it is ([`AsFinished`]).
//!
//! Finishing a Commit again, after a crash part way through or just before the journal was
//! removed, leaves the Area as finishing it once did:
//! - a temporary file that is gone was renamed already, so it is skipped, with the directories it
//!   needed;
//! - a delete is made only if the File is still there, under exactly its name, with the Revision
//!   it had when the Commit was planned. So a File moved onto the Path since is left alone,
//!   whether by this Commit, as when a rename changes only letter case on a filesystem that
//!   ignores it (`Foo` to `foo`), or by another program. Only a File another program wrote since,
//!   with the very same contents, would be deleted again;
//! - no two Paths of a Commit are the same file on disk (the Commit refuses that), so one Path's
//!   delete can't remove what another Path's write put there.
//!
//! Each state is written with `atomic-write-file`, which writes a new file, forces it to disk and
//! renames it over the journal, so the journal is always whole. It is plain text: a first line
//! that names the format, a line with the state, then a line for each item, with fields separated
//! by tabs, and `\`, tabs and line breaks escaped:
//! - `write`, the temporary file, and the Path whose File it replaces;
//! - `replace`, the temporary file, the Path it writes, and the file that Path's symlink points
//!   to, which it replaces;
//! - `remove`, the Path, and the Revision of the File it deletes, in hexadecimal.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Write};
use std::path::{Path as FsPath, PathBuf};

use atomic_write_file::AtomicWriteFile;

#[cfg(feature = "testing")]
use super::FailurePoint;
use jiff::Timestamp;

use super::{
    AreaRoot, failed, on_disk, present, present_at, read_file, remove_empty_directories, revisions,
};
use crate::backend::AreaState;
use crate::{Error, Path, Prefix, Result, Revision};

/// The first line of every journal, which names its format.
const FORMAT: &str = "tidings journal 2";

/// A Commit in progress, as the journal records it.
#[derive(Debug)]
pub(super) struct Journal {
    state: State,
    /// Each temporary file, in order of the Path it writes.
    replaces: Vec<Replace>,
    /// Each Path the Commit deletes.
    removes: Vec<Remove>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Prepared,
    Committed,
}

/// A temporary file, the Path it writes, and the File it replaces once the Commit has happened.
#[derive(Debug, Clone)]
pub(super) struct Replace {
    pub(super) path: Path,
    pub(super) temporary: PathBuf,
    pub(super) target: Target,
}

/// The File a temporary file replaces.
#[derive(Debug, Clone)]
pub(super) enum Target {
    /// The File at the Path written, whose directories finishing makes if they don't exist.
    AtPath,
    /// The file the symlink at the Path written points to, anywhere, in a directory that exists.
    Linked(PathBuf),
}

/// A Path a Commit deletes, and the Revision of the File there when the Commit was planned.
#[derive(Debug)]
pub(super) struct Remove {
    pub(super) path: Path,
    pub(super) revision: Revision,
}

/// An Area as reads through tidings see it: as finishing the Commit in its journal will leave it,
/// if the journal is committed. Each Path the Commit writes is its temporary file, until that is
/// renamed, and each Path it deletes is absent, if the File there is the one it deletes, as
/// finishing deletes only that one. With no committed journal, it is the Area as it is on disk.
pub(super) struct AsFinished<'a> {
    area: &'a AreaRoot,
    /// The temporary file of each Path the Commit writes.
    written: BTreeMap<Path, PathBuf>,
    /// Each Path the Commit deletes, with the Revision of the File it deletes.
    removed: BTreeMap<Path, Revision>,
}

impl Replace {
    /// Where the file this replaces is on disk, in the Area whose root is `root`.
    pub(super) fn on_disk(&self, root: &FsPath) -> PathBuf {
        match &self.target {
            Target::AtPath => on_disk(root, self.path.as_str()),
            Target::Linked(target) => target.clone(),
        }
    }
}

impl AsFinished<'_> {
    /// `area`, as finishing the Commit in its journal will leave it.
    pub(super) fn of(area: &AreaRoot) -> Result<AsFinished<'_>> {
        let mut finished = AsFinished { area, written: BTreeMap::new(), removed: BTreeMap::new() };
        if let Some(journal) = Journal::read(&area.tidings())?
            && journal.state == State::Committed
        {
            let written =
                journal.replaces.into_iter().map(|replace| (replace.path, replace.temporary));
            let removed = journal.removes.into_iter().map(|remove| (remove.path, remove.revision));
            (finished.written, finished.removed) = (written.collect(), removed.collect());
        }
        Ok(finished)
    }

    /// What [`AreaRoot::read`] gives for `path`, once the Commit is finished.
    pub(super) fn read(&self, path: &Path) -> Result<Option<(Vec<u8>, Timestamp)>> {
        // A temporary file that is gone was renamed over the File already.
        if let Some(temporary) = self.written.get(path)
            && let Some(read) = read_file(temporary)?
        {
            return Ok(Some(read));
        }
        let read = self.area.read(path)?;
        let removed = self.removed.get(path).is_some_and(|revision| {
            read.as_ref().is_some_and(|(contents, _)| Revision::of_bytes(contents) == *revision)
        });
        Ok(if removed { None } else { read })
    }

    /// What [`AreaRoot::paths_under`] gives for `prefix`, once the Commit is finished.
    pub(super) fn paths_under(&self, prefix: &Prefix) -> Result<Vec<Path>> {
        let mut paths = Vec::new();
        for path in self.area.paths_under(prefix)? {
            if !self.removed.contains_key(&path) || self.read(&path)?.is_some() {
                paths.push(path);
            }
        }
        let written = self.written.keys();
        paths.extend(written.filter(|path| path.as_str().starts_with(prefix.as_str())).cloned());
        paths.sort();
        paths.dedup();
        Ok(paths)
    }

    /// The Path and Revision of every File under `prefix`, in order of Path, once the Commit is
    /// finished.
    pub(super) fn revisions_under(&self, prefix: &Prefix) -> Result<Vec<(Path, Revision)>> {
        revisions(self.paths_under(prefix)?, |path| self.read(path))
    }
}

/// Whether the journal in `tidings` is `committed`.
pub(super) fn is_committed(tidings: &FsPath) -> Result<bool> {
    Ok(Journal::read(tidings)?.is_some_and(|journal| journal.state == State::Committed))
}

/// What [`recover`] did.
pub(super) enum Recovery {
    /// There was no Commit left in the journal, or there was and it is finished or discarded now.
    Done,
    /// The Commit left in the journal couldn't be finished or discarded, for this reason, even
    /// after trying again. It is still there.
    Left(Error),
}

/// Finishes or discards the Commit left in the journal of `area`, by a crash or by a Commit that
/// couldn't finish, if there is one. It must be called with the Area locked. It gives an error
/// only if the journal can't be read.
pub(super) fn recover(area: &AreaRoot) -> Result<Recovery> {
    let tidings = area.tidings();
    let Some(journal) = Journal::read(&tidings)? else { return Ok(Recovery::Done) };
    let recovered = match journal.state {
        State::Prepared => {
            tracing::debug!("discarding a Commit to {} that never happened", area.root.display());
            journal.discard(&tidings)
        }
        State::Committed => {
            tracing::debug!("finishing a Commit to {} left unfinished", area.root.display());
            area.finish(&journal, true)
        }
    };
    Ok(match recovered {
        Ok(()) => Recovery::Done,
        Err(error) => Recovery::Left(error),
    })
}

impl Journal {
    /// A journal of a Commit that replaces Files with `replaces` and deletes `removes`, before
    /// any temporary file is written.
    pub(super) fn prepared(replaces: Vec<Replace>, removes: Vec<Remove>) -> Journal {
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
            present_at(fs::remove_file(temporary), temporary)?;
        }
        remove(tidings)
    }

    /// Finishes the Commit in `area`, then removes the journal. `again` says whether this is
    /// finishing a Commit a crash interrupted, which may be partly finished already:
    /// 1. deletes each File, and removes each directory that leaves empty, so that a File can
    ///    take the directory's name (`d/e` to `d`). Finishing again, a File is deleted only if it
    ///    is still the one the Commit deletes;
    /// 2. for each temporary file not yet renamed, makes the directories its File goes in, now that
    ///    a File that had one of their names is gone (`a` to `a/b`), removes an empty directory
    ///    that has the File's name, and renames the temporary file over the File;
    /// 3. forces each directory changed or made to disk, so that the Commit is on disk before the
    ///    journal is removed.
    pub(super) fn finish(&self, area: &AreaRoot, again: bool) -> Result<()> {
        let root = &area.root;
        let mut changed = BTreeSet::new();
        for Remove { path, revision } in &self.removes {
            let file = on_disk(root, path.as_str());
            let there = present_at(fs::symlink_metadata(&file), &file)?;
            // A directory there now was made by one of the writes.
            let file_there = there.is_some_and(|there| !there.is_dir());
            if file_there && (!again || area.revision(path)? == Some(*revision)) {
                fs::remove_file(&file).map_err(|error| failed(&file, error))?;
                changed.extend(file.parent().map(FsPath::to_path_buf));
            }
            remove_emptied_directories(root, &file, &mut changed);
        }
        #[cfg(feature = "testing")]
        if !again {
            area.stop_at(FailurePoint::AfterDeletes)?;
        }
        #[cfg_attr(
            not(feature = "testing"),
            expect(clippy::unused_enumerate_index, reason = "the number names a failure point")
        )]
        for (_n, replace) in self.replaces.iter().enumerate() {
            let Replace { path, temporary, target } = replace;
            if present_at(fs::symlink_metadata(temporary), temporary)?.is_none() {
                continue;
            }
            if let Target::AtPath = target {
                area.make_directories(path, &mut changed)?;
            }
            let target = replace.on_disk(root);
            if fs::symlink_metadata(&target).is_ok_and(|there| there.is_dir()) {
                remove_empty_directories(&target).map_err(|error| failed(&target, error))?;
            }
            #[cfg(feature = "testing")]
            area.rename_fails(_n).map_err(|error| failed(temporary, error))?;
            fs::rename(temporary, &target).map_err(|error| failed(temporary, error))?;
            changed.extend(temporary.parent().map(FsPath::to_path_buf));
            changed.extend(target.parent().map(FsPath::to_path_buf));
            #[cfg(feature = "testing")]
            if !again {
                area.stop_at(FailurePoint::AfterRename(_n))?;
            }
        }
        for directory in &changed {
            sync_directory(directory)?;
        }
        remove(&area.tidings())
    }

    /// Reads the journal in `tidings`, if there is one.
    fn read(tidings: &FsPath) -> Result<Option<Journal>> {
        let path = tidings.join("journal");
        let Some(text) = present_at(fs::read_to_string(&path), &path)? else { return Ok(None) };
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
        let parse_path = |path: &str| Path::new(path).map_err(|_| unreadable("a Path is invalid"));
        for line in lines {
            let fields: Vec<String> = line.split('\t').map(unescape).collect();
            match fields.as_slice() {
                [kind, temporary, path] if kind == "write" => {
                    let (path, temporary) = (parse_path(path)?, temporary.into());
                    journal.replaces.push(Replace { path, temporary, target: Target::AtPath });
                }
                [kind, temporary, path, target] if kind == "replace" => {
                    let (path, temporary) = (parse_path(path)?, temporary.into());
                    let target = Target::Linked(target.into());
                    journal.replaces.push(Replace { path, temporary, target });
                }
                [kind, path, revision] if kind == "remove" => {
                    let revision = from_hex(revision).ok_or_else(|| {
                        unreadable(&format!("{revision:?} isn't a Revision it can have"))
                    })?;
                    journal.removes.push(Remove { path: parse_path(path)?, revision });
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
        for Replace { path, temporary, target } in &self.replaces {
            let (temporary, path) = (escaped(temporary)?, escape(path.as_str()));
            text += &match target {
                Target::AtPath => format!("write\t{temporary}\t{path}\n"),
                Target::Linked(target) => {
                    format!("replace\t{temporary}\t{path}\t{}\n", escaped(target)?)
                }
            };
        }
        for Remove { path, revision } in &self.removes {
            text += &format!("remove\t{}\t{}\n", escape(path.as_str()), to_hex(*revision));
        }
        Ok(text)
    }
}

/// Removes the journal in `tidings`.
///
/// That isn't forced to disk, which would cost every Commit another wait for the disk. If a power
/// cut loses it, the Commit is finished again when the Area is next locked, which leaves the Area
/// as it is, as the module's doc explains. The next Commit's journal replaces this one on disk
/// before that Commit changes anything.
fn remove(tidings: &FsPath) -> Result<()> {
    let path = tidings.join("journal");
    present_at(fs::remove_file(&path), &path)?;
    Ok(())
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
        match present(fs::remove_dir(emptied)) {
            Ok(Some(())) => {
                changed.remove(emptied);
                changed.extend(emptied.parent().map(FsPath::to_path_buf));
            }
            // Finishing again, it may be removed already.
            Ok(None) => {}
            // It isn't empty, or can't be removed: it stays, and so does everything above it.
            Err(_) => break,
        }
        directory = emptied.parent();
    }
}

/// Forces the entries of `directory` to disk, where the platform can, so that a rename, delete or
/// new directory in it survives a power cut. A directory that is gone has nothing to force.
fn sync_directory(directory: &FsPath) -> Result<()> {
    #[cfg(unix)]
    present_at(fs::File::open(directory).and_then(|opened| opened.sync_all()), directory)?;
    #[cfg(not(unix))]
    let _ = directory;
    Ok(())
}

/// `revision` in hexadecimal.
fn to_hex(revision: Revision) -> String {
    revision.to_bytes().iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The Revision [`to_hex`] wrote as `hex`.
fn from_hex(hex: &str) -> Option<Revision> {
    let mut bytes = [0; 16];
    if hex.len() != 2 * bytes.len() || !hex.is_ascii() {
        return None;
    }
    for (byte, digits) in bytes.iter_mut().zip(hex.as_bytes().chunks(2)) {
        *byte = u8::from_str_radix(std::str::from_utf8(digits).ok()?, 16).ok()?;
    }
    Some(Revision::from_bytes(bytes))
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
