//! Scanning a Working copy's folder: which files in it can become Files, which can't and why, and
//! which the ignore file, `.tidings/ignore`, leaves out.
//!
//! The ignore file is in `.gitignore` syntax, read afresh by each scan, and matched as git matches
//! one: a file in an ignored directory is ignored, whatever patterns later re-include it. It only
//! applies to files with no Base: anything the Store holds is always synced and tracked.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fmt;
use std::fs::{self, FileType};
use std::io::{self, ErrorKind, Write};
use std::path::{Path as FsPath, PathBuf};

use atomic_write_file::AtomicWriteFile;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use tidings::{InvalidPathReason, Path};

use super::record::Base;
use super::{RECORD_DIRECTORY, failed_at};
use crate::failure::Failure;

/// The ignore file in [`RECORD_DIRECTORY`], in `.gitignore` syntax: a file with no Base that
/// matches it is left out of a commit.
pub(super) const IGNORE_FILE: &str = "ignore";

/// What the ignore file of a new Working copy holds: the files editors and operating systems leave
/// in a folder.
const DEFAULT_IGNORE: &str = "\
# Files with no Base that match a pattern here, in .gitignore syntax, are left out of
# `tidings commit`. A File the Store holds is always synced and committed, even if it matches.
.*.sw?
*~
4913
.DS_Store
Thumbs.db
.#*
";

/// What scanning the folder found, outside `.tidings/`.
#[derive(Default)]
pub(super) struct Scan {
    /// Each file that can become a File, with its contents.
    pub(super) files: BTreeMap<Path, String>,
    /// Each file that can't become a File, in order of name.
    pub(super) invalid: Vec<Unfit>,
    /// Each file with no Base that the ignore file leaves out, in order.
    pub(super) ignored: Vec<LocalName>,
}

/// Something in the folder that can't become a File: a file whose name or contents can't be a
/// File's, a symlink, or a special file.
#[derive(Debug)]
pub struct Unfit {
    /// Its name in the folder.
    pub name: LocalName,
    /// Why it can't become a File, as in "is a symlink".
    pub reason: String,
}

/// The name of a file or directory in the folder, relative to it, with `/` between names, as in
/// `themes/dark.toml`, and any part that isn't UTF-8 shown with U+FFFD. It spells a Path only if
/// the name is a valid one.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct LocalName(String);

impl LocalName {
    /// The name of `relative`, a file or directory under the folder given relative to it, or
    /// `None` if it is in `.tidings/`, which never holds Files.
    pub(super) fn of(relative: &FsPath) -> Option<LocalName> {
        let parts: Vec<&OsStr> = relative.components().map(|part| part.as_os_str()).collect();
        if parts.first().is_some_and(|first| is_record_directory(first)) {
            return None;
        }
        let parts: Vec<_> = parts.iter().map(|part| part.to_string_lossy()).collect();
        Some(LocalName(parts.join("/")))
    }

    /// The name as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for LocalName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Whether `name`, of something directly in the folder, is the record's directory: matched in any
/// letter case, since the folder may be on a filesystem that ignores it.
pub(super) fn is_record_directory(name: &OsStr) -> bool {
    name.to_str().is_some_and(|name| name.eq_ignore_ascii_case(RECORD_DIRECTORY))
}

/// Where the Working copy that is `folder` keeps its ignore file.
fn ignore_file(folder: &FsPath) -> PathBuf {
    folder.join(RECORD_DIRECTORY).join(IGNORE_FILE)
}

/// Saves the ignore file of a new Working copy in `folder`, holding [`DEFAULT_IGNORE`], unless
/// there is one already, which only a `sync` that stopped before saving the record, or the person,
/// could have written.
pub(super) fn create_ignore_file(folder: &FsPath) -> Result<(), Failure> {
    let file = ignore_file(folder);
    if fs::symlink_metadata(&file).is_ok() {
        return Ok(());
    }
    // Whole or not at all, so that it never holds only some of the patterns.
    let write = || -> io::Result<()> {
        let mut atomic = AtomicWriteFile::open(&file)?;
        atomic.write_all(DEFAULT_IGNORE.as_bytes())?;
        atomic.commit()
    };
    write().map_err(failed_at(&file))
}

/// Every file in `folder`, outside `.tidings/`: with its contents, if it can become a File, or
/// else why not. A file with no Base in `bases` that the ignore file matches is only named, as
/// ignored. Empty directories hold no files, so they are never found.
pub(super) fn scan(folder: &FsPath, bases: &BTreeMap<Path, Base>) -> Result<Scan, Failure> {
    let ignore = read_ignore_file(folder)?;
    let mut scanner = Scanner { folder, bases, ignore, scan: Scan::default() };
    scanner.scan_directory(FsPath::new(""), false)?;
    let mut scan = scanner.scan;
    scan.invalid.sort_unstable_by(|a, b| a.name.cmp(&b.name));
    scan.ignored.sort_unstable();
    Ok(scan)
}

/// The ignore file of `folder`, read afresh, as a matcher for files anywhere in the folder. A
/// missing one, as when the person removed it, matches nothing. Fails if it is anything but a
/// regular file, since it isn't followed, and if any line of it isn't UTF-8 or isn't a valid
/// pattern, naming the line, since leaving the pattern out could commit files the person meant to
/// leave out. A byte order mark at its start is skipped, as git skips it.
fn read_ignore_file(folder: &FsPath) -> Result<Gitignore, Failure> {
    let file = ignore_file(folder);
    let refuse = |why: &dyn fmt::Display| Failure::error(format!("{}{why}", file.display()));
    let mut builder = GitignoreBuilder::new(folder);
    match fs::symlink_metadata(&file) {
        Ok(metadata) if metadata.is_file() => {}
        Ok(metadata) if metadata.is_symlink() => {
            return Err(refuse(&": is a symlink, which isn't followed: make it a regular file"));
        }
        Ok(_) => return Err(refuse(&": isn't a regular file")),
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Gitignore::empty()),
        Err(error) => return Err(failed_at(&file)(error)),
    }
    let bytes = fs::read(&file).map_err(failed_at(&file))?;
    let text = String::from_utf8(bytes).map_err(|error| {
        let valid = &error.as_bytes()[..error.utf8_error().valid_up_to()];
        let number = valid.iter().filter(|&&byte| byte == b'\n').count() + 1;
        refuse(&format!(":{number}: isn't UTF-8 text: save the ignore file as UTF-8"))
    })?;
    // As git does, and as `GitignoreBuilder::add` would, but `add_line` doesn't.
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    for (index, line) in text.lines().enumerate() {
        let number = index + 1;
        builder.add_line(None, line).map_err(|error| refuse(&format!(":{number}: {error}")))?;
    }
    builder.build().map_err(|error| refuse(&format!(": {error}")))
}

/// What a file's name in the folder is as a Path.
enum AsPath {
    /// It is a valid Path.
    Valid(Path),
    /// It isn't UTF-8.
    NotUtf8,
    /// It isn't a valid Path, for this reason.
    Invalid(InvalidPathReason),
}

/// Scans one folder, gathering what it finds.
struct Scanner<'a> {
    /// The folder of the Working copy being scanned.
    folder: &'a FsPath,
    /// The Base of each Path, which the ignore file never applies to.
    bases: &'a BTreeMap<Path, Base>,
    /// The ignore file, as a matcher.
    ignore: Gitignore,
    /// What it has found so far.
    scan: Scan,
}

impl Scanner<'_> {
    /// Adds every file under `directory`, relative to the folder, to the scan, as [`scan`] says.
    /// `ignored` says whether the ignore file leaves out `directory` or one it is in, which leaves
    /// out everything under it, as in git.
    fn scan_directory(&mut self, directory: &FsPath, ignored: bool) -> Result<(), Failure> {
        let at = self.folder.join(directory);
        for entry in fs::read_dir(&at).map_err(failed_at(&at))? {
            let entry = entry.map_err(failed_at(&at))?;
            let relative = directory.join(entry.file_name());
            let Some(name) = LocalName::of(&relative) else { continue };
            let file_type = entry.file_type().map_err(failed_at(&at))?;
            let is_dir = file_type.is_dir();
            let ignored =
                ignored || self.ignore.matched(self.folder.join(&relative), is_dir).is_ignore();
            if is_dir {
                self.scan_directory(&relative, ignored)?;
            } else {
                self.scan_file(&relative, name, file_type, ignored)?;
            }
        }
        Ok(())
    }

    /// Adds the file `relative` to the folder, named `name`, of `file_type`, to the scan, unless it
    /// has no Base and is `ignored`, which is noted instead.
    fn scan_file(
        &mut self,
        relative: &FsPath,
        name: LocalName,
        file_type: FileType,
        ignored: bool,
    ) -> Result<(), Failure> {
        let as_path = as_path(relative, &name)?;
        let has_base = matches!(&as_path, AsPath::Valid(path) if self.bases.contains_key(path));
        if ignored && !has_base {
            self.scan.ignored.push(name);
            return Ok(());
        }
        let reason = match as_path {
            AsPath::NotUtf8 => "has a name that isn't UTF-8".to_owned(),
            AsPath::Invalid(reason) => format!("isn't a valid Path: {reason}"),
            AsPath::Valid(_) if file_type.is_symlink() => "is a symlink".to_owned(),
            AsPath::Valid(_) if !file_type.is_file() => "isn't a regular file".to_owned(),
            AsPath::Valid(path) => {
                let at = self.folder.join(relative);
                match fs::read_to_string(&at) {
                    Ok(contents) => {
                        self.scan.files.insert(path, contents);
                        return Ok(());
                    }
                    Err(error) if error.kind() == ErrorKind::InvalidData => {
                        "isn't UTF-8 text".to_owned()
                    }
                    Err(error) => return Err(failed_at(&at)(error)),
                }
            }
        };
        self.scan.invalid.push(Unfit { name, reason });
        Ok(())
    }
}

/// What `relative`, a file in the folder named `name`, is as a Path.
fn as_path(relative: &FsPath, name: &LocalName) -> Result<AsPath, Failure> {
    if relative.to_str().is_none() {
        return Ok(AsPath::NotUtf8);
    }
    match Path::new(name.as_str()) {
        Ok(path) => Ok(AsPath::Valid(path)),
        Err(tidings::Error::InvalidPath { reason, .. }) => Ok(AsPath::Invalid(reason)),
        Err(error) => Err(Failure::from(error).in_context(name)),
    }
}
