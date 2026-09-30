//! The record, `.tidings/working-copy`: which Store a Working copy belongs to, each Path's Base,
//! and which Paths are Diverged.
//!
//! It is plain text, in the style of the filesystem journal (ADR 0005): a first line that names
//! the format and its version, then a line for each item, with fields separated by tabs, and `\`,
//! tabs and line breaks escaped:
//! - `store` and the Store's Location, which is absolute;
//! - `backend` and the Backend;
//! - `base`, a Path, its Base Revision, and the hash of the contents last written to or read from
//!   the folder for that Base, in hexadecimal;
//! - `diverged`, a Diverged Path, and the Revision of the Store's File in `theirs` and the hash of
//!   that File's contents, which are both left out if the Store has no File there.
//!
//! This is version 1; any other version is refused. So is a record in the shape version 1 had
//! while a Store held Areas, with `root` or `identity`, and `area`: it is in a format this version
//! doesn't know.
//!
//! It is always replaced whole, with `atomic-write-file`, which writes a new file, forces it to
//! disk and renames it over the record.

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::{Path as FsPath, PathBuf};

use atomic_write_file::AtomicWriteFile;
use tidings::{File, Path, Revision};

use crate::failure::Failure;
use crate::location::{BackendName, StoreAddress};

/// The first line of every record this version writes, which names its format.
const FORMAT: &str = "tidings working-copy 1";

/// What the first line of a record in any version starts with.
const FORMAT_NAME: &str = "tidings working-copy ";

/// A Working copy's record.
#[derive(Debug)]
pub struct Record {
    /// The Store the Working copy belongs to.
    pub store: StoreAddress,
    /// The Base of each Path that has one.
    pub bases: BTreeMap<Path, Base>,
    /// Each Diverged Path, which may or may not have a Base.
    pub divergences: BTreeMap<Path, Divergence>,
}

/// A Path's Base, and the hash of its contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Base {
    pub revision: Revision,
    /// The hash of the Base's contents.
    pub hash: Hash,
}

impl Base {
    /// The Base of the Revision `revision`, which holds `contents`.
    pub fn of(revision: Revision, contents: &str) -> Base {
        Base { revision, hash: Hash::of(contents) }
    }

    /// The Base of the Store's File `file`: its Revision, which holds its contents.
    pub fn of_file(file: &File) -> Base {
        Base::of(file.revision(), file.contents())
    }

    /// Whether `contents` are the Base's.
    pub fn holds(&self, contents: &str) -> bool {
        self.hash == Hash::of(contents)
    }
}

/// That a Path is Diverged, and the Store's File that `theirs` holds for it, if the Store has one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Divergence {
    /// The Store's File in `theirs`, as the Base `resolve` takes: its Revision, and the hash of
    /// its contents, computed from the Store's File when it was put in `theirs`, never read back
    /// from `theirs`, which the person may have merged in. `None` if the Store has no File there.
    pub theirs: Option<Base>,
}

impl Divergence {
    /// The Revision of the Store's File in `theirs`, or `None` if the Store has no File there.
    pub fn theirs_revision(&self) -> Option<Revision> {
        self.theirs.map(|theirs| theirs.revision)
    }
}

/// The hash of a file's contents (XXH3), which tells whether a local file still holds what the
/// Working copy last wrote to or read from it. It is never compared with a Revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hash(u128);

impl Hash {
    /// The hash of `contents`.
    pub fn of(contents: &str) -> Hash {
        Hash(xxhash_rust::xxh3::xxh3_128(contents.as_bytes()))
    }

    /// The hash as the record writes it, in hexadecimal.
    fn to_text(self) -> String {
        format!("{:032x}", self.0)
    }

    /// The hash the record wrote as `text`.
    fn parse(text: &str) -> Result<Hash, String> {
        u128::from_str_radix(text, 16).map(Hash).map_err(|_| bad("hash", text))
    }
}

/// Whether `file` is a record, of this version or another: whether it starts with the line that
/// names the format.
pub fn is_record(file: &FsPath) -> bool {
    std::fs::read(file).is_ok_and(|bytes| bytes.starts_with(FORMAT_NAME.as_bytes()))
}

impl Record {
    /// A record with no Bases yet.
    pub fn new(store: StoreAddress) -> Record {
        Record { store, bases: BTreeMap::new(), divergences: BTreeMap::new() }
    }

    /// Makes `base` the Base of `path`, or leaves `path` with no Base if `base` is `None`, as when
    /// the Store has no File there.
    pub fn set_base(&mut self, path: &Path, base: Option<Base>) {
        match base {
            Some(base) => {
                self.bases.insert(path.clone(), base);
            }
            None => {
                self.bases.remove(path);
            }
        }
    }

    /// Reads the record in `file`.
    pub fn read(file: &FsPath) -> Result<Record, Failure> {
        let text = std::fs::read_to_string(file)
            .map_err(|error| Failure::error(format!("can't read {}: {error}", file.display())))?;
        Record::parse(&text)
            .map_err(|problem| Failure::error(format!("{} {problem}", file.display())))
    }

    /// Writes the record, whole, to `file`, and forces it to disk.
    pub fn write(&self, file: &FsPath) -> Result<(), Failure> {
        let text = self.to_text();
        let write = || -> io::Result<()> {
            let mut atomic = AtomicWriteFile::open(file)?;
            atomic.write_all(text.as_bytes())?;
            atomic.commit()
        };
        write().map_err(|error| Failure::error(format!("can't write {}: {error}", file.display())))
    }

    /// The record as the text of its file.
    fn to_text(&self) -> String {
        let mut text = format!("{FORMAT}\n");
        let mut line = |fields: &[&str]| {
            let fields: Vec<String> = fields.iter().map(|field| escape(field)).collect();
            text.push_str(&fields.join("\t"));
            text.push('\n');
        };
        // A Location that isn't UTF-8 can't be given on the command line anyway.
        line(&["store", &self.store.location.to_string_lossy()]);
        line(&["backend", &self.store.backend.to_string()]);
        for (path, base) in &self.bases {
            line(&["base", path.as_str(), &base.revision.to_string(), &base.hash.to_text()]);
        }
        for (path, divergence) in &self.divergences {
            match &divergence.theirs {
                Some(theirs) => {
                    let (revision, hash) = (theirs.revision.to_string(), theirs.hash.to_text());
                    line(&["diverged", path.as_str(), &revision, &hash]);
                }
                None => line(&["diverged", path.as_str()]),
            }
        }
        text
    }

    /// Parses a record, or says what is wrong with it.
    fn parse(text: &str) -> Result<Record, String> {
        let mut lines = text.lines();
        match lines.next() {
            Some(FORMAT) => {}
            Some(first) if first.starts_with(FORMAT_NAME) => {
                return Err(format!("is in a format this version doesn't know: {first:?}"));
            }
            _ => return Err("is not a Working copy's record".to_owned()),
        }
        let (mut location, mut backend) = (None, None);
        let (mut bases, mut divergences) = (BTreeMap::new(), BTreeMap::new());
        let parse_path = |path: &str| Path::new(path).map_err(|error| error.to_string());
        for line in lines {
            let fields: Vec<String> = line.split('\t').map(unescape).collect();
            let fields: Vec<&str> = fields.iter().map(String::as_str).collect();
            match fields[..] {
                ["store", store] => location = Some(PathBuf::from(store)),
                ["backend", name] => backend = Some(parse_backend(name)?),
                [old @ ("root" | "identity" | "area"), ..] => {
                    return Err(format!(
                        "is in a format this version doesn't know: it has a line {old:?}, from \
                         before a Store was one Location"
                    ));
                }
                ["base", path, revision, hash] => {
                    bases.insert(parse_path(path)?, parse_base(revision, hash)?);
                }
                ["diverged", path] => {
                    divergences.insert(parse_path(path)?, Divergence { theirs: None });
                }
                ["diverged", path, revision, hash] => {
                    let theirs = Some(parse_base(revision, hash)?);
                    divergences.insert(parse_path(path)?, Divergence { theirs });
                }
                _ => return Err(format!("has a line it can't read: {line:?}")),
            }
        }
        let missing = |what: &str| format!("doesn't say {what}");
        let store = StoreAddress {
            location: location.ok_or_else(|| missing("where the Store is"))?,
            backend: backend.ok_or_else(|| missing("the Backend"))?,
        };
        Ok(Record { store, bases, divergences })
    }
}

/// The Base the record gives as `revision` and `hash`.
fn parse_base(revision: &str, hash: &str) -> Result<Base, String> {
    let revision = revision.parse().map_err(|_| bad("Revision", revision))?;
    Ok(Base { revision, hash: Hash::parse(hash)? })
}

/// The Backend named `name`, which can't be the memory Backend.
fn parse_backend(name: &str) -> Result<BackendName, String> {
    match parse_value(name)? {
        BackendName::Memory => Err(bad("Backend", name)),
        backend => Ok(backend),
    }
}

/// `name` as a value of a command-line argument.
fn parse_value<T: clap::ValueEnum>(name: &str) -> Result<T, String> {
    T::from_str(name, false).map_err(|_| bad("name", name))
}

/// What is wrong with a record that has `text` where it should have a `what`.
fn bad(what: &str, text: &str) -> String {
    format!("has a {what} it can't read: {text:?}")
}

/// `field` with `\`, tabs and line breaks escaped, so that it fits in one field of a line.
fn escape(field: &str) -> String {
    let mut escaped = String::with_capacity(field.len());
    for c in field.chars() {
        match c {
            '\\' => escaped.push_str("\\\\"),
            '\t' => escaped.push_str("\\t"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            c => escaped.push(c),
        }
    }
    escaped
}

/// A field [`escape`] wrote, as it was.
fn unescape(field: &str) -> String {
    let mut unescaped = String::with_capacity(field.len());
    let mut chars = field.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            unescaped.push(c);
            continue;
        }
        match chars.next() {
            Some('t') => unescaped.push('\t'),
            Some('n') => unescaped.push('\n'),
            Some('r') => unescaped.push('\r'),
            Some(other) => unescaped.push(other),
            None => {}
        }
    }
    unescaped
}
