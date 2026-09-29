//! The record, `.tidings/working-copy`: which Store and Area a Working copy belongs to, and each
//! Path's Base.
//!
//! It is plain text, in the style of the filesystem journal (ADR 0005): a first line that names
//! the format, then a line for each item, with fields separated by tabs, and `\`, tabs and line
//! breaks escaped:
//! - `root` and the absolute Root override, or `identity` and the App identity;
//! - `backend` and the Backend;
//! - `area` and the Area;
//! - `base`, a Path, its Base Revision, and the hash of the contents last written to or read from
//!   the folder for that Base, in hexadecimal.
//!
//! It is always replaced whole, with `atomic-write-file`, which writes a new file, forces it to
//! disk and renames it over the record.

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::{Path as FsPath, PathBuf};

use atomic_write_file::AtomicWriteFile;
use tidings::{Area, Path, Revision};

use crate::command::AreaName;
use crate::failure::Failure;
use crate::location::{BackendName, Identity, StoreAddress, StoreLocation};
use crate::output::area_name;

/// The first line of every record, which names its format.
const FORMAT: &str = "tidings working-copy 1";

/// What the first line of a record in any version starts with.
const FORMAT_NAME: &str = "tidings working-copy ";

/// A Working copy's record.
#[derive(Debug)]
pub struct Record {
    /// The Store the Working copy belongs to.
    pub store: StoreAddress,
    pub area: Area,
    /// The Base of each Path that has one.
    pub bases: BTreeMap<Path, Base>,
}

/// A Path's Base, and the hash of its contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Base {
    pub revision: Revision,
    pub hash: Hash,
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
}

/// Whether `file` is a record, of this version or another: whether it starts with the line that
/// names the format.
pub fn is_record(file: &FsPath) -> bool {
    std::fs::read(file).is_ok_and(|bytes| bytes.starts_with(FORMAT_NAME.as_bytes()))
}

impl Record {
    /// A record with no Bases yet.
    pub fn new(store: StoreAddress, area: Area) -> Record {
        Record { store, area, bases: BTreeMap::new() }
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
        match &self.store.location {
            StoreLocation::Root(root) => {
                // A Root override that isn't UTF-8 can't be given on the command line anyway.
                line(&["root", &root.to_string_lossy()]);
            }
            StoreLocation::Identity(identity) => line(&["identity", &identity.to_string()]),
        }
        line(&["backend", &self.store.backend.to_string()]);
        line(&["area", area_name(self.area)]);
        for (path, base) in &self.bases {
            let revision = base.revision.to_string();
            let hash = format!("{:032x}", base.hash.0);
            line(&["base", path.as_str(), &revision, &hash]);
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
        let (mut location, mut backend, mut area) = (None, None, None);
        let mut bases = BTreeMap::new();
        for line in lines {
            let fields: Vec<String> = line.split('\t').map(unescape).collect();
            let fields: Vec<&str> = fields.iter().map(String::as_str).collect();
            match fields[..] {
                ["root", root] => location = Some(StoreLocation::Root(PathBuf::from(root))),
                ["identity", identity] => {
                    location = Some(StoreLocation::Identity(Identity::parse(identity)?));
                }
                ["backend", name] => backend = Some(parse_backend(name)?),
                ["area", name] => area = Some(Area::from(parse_value::<AreaName>(name)?)),
                ["base", path, revision, hash] => {
                    let path = Path::new(path).map_err(|error| error.to_string())?;
                    let revision = revision.parse().map_err(|_| bad("Revision", revision))?;
                    let hash = u128::from_str_radix(hash, 16).map_err(|_| bad("hash", hash))?;
                    bases.insert(path, Base { revision, hash: Hash(hash) });
                }
                _ => return Err(format!("has a line it can't read: {line:?}")),
            }
        }
        let missing = |what: &str| format!("doesn't say {what}");
        let store = StoreAddress {
            location: location.ok_or_else(|| missing("where the Store is"))?,
            backend: backend.ok_or_else(|| missing("the Backend"))?,
        };
        Ok(Record { store, area: area.ok_or_else(|| missing("the Area"))?, bases })
    }
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
