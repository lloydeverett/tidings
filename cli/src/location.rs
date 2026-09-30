//! Which Store a command opens: the flags that choose one, and opening it.

use std::fmt;
use std::fs;
use std::path::{self, Path, PathBuf};

use clap::{Args, ValueEnum};
use tidings::{BackendKind, ChangeFeed, FsOptions, SqliteOptions, Store};

use crate::failure::Failure;

/// The flags that choose a Store, which every command takes.
#[derive(Debug, Args)]
pub struct StoreArgs {
    /// The Store's Location: the directory the app opens it at.
    #[arg(long, global = true, env = "TIDINGS_STORE", value_name = "DIR")]
    store: Option<PathBuf>,

    /// The Backend that holds the Store. Found from the Store's Location when left out.
    #[arg(long, global = true, env = "TIDINGS_BACKEND")]
    backend: Option<BackendName>,

    /// Make a new Store where there is none. Needs `--backend`.
    #[arg(long, global = true)]
    pub create: bool,
}

/// A Backend, as it is named on the command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum BackendName {
    Fs,
    Sqlite,
    /// Only in `tidings store shell`: a Store that lasts as long as the shell.
    Memory,
}

impl From<BackendKind> for BackendName {
    fn from(kind: BackendKind) -> BackendName {
        match kind {
            BackendKind::Fs => BackendName::Fs,
            BackendKind::Sqlite => BackendName::Sqlite,
        }
    }
}

impl fmt::Display for BackendName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.to_possible_value().unwrap().get_name())
    }
}

/// The Backend that holds the Store at `location`, or `None` if there is no Store there.
async fn detect(location: &Path) -> Result<Option<BackendName>, Failure> {
    let detected = Store::detect(location).await?;
    Ok(detected.map(BackendName::from))
}

/// The failure for there being no Store at `location`.
fn missing(location: &Path) -> Failure {
    Failure::error(format!(
        "there is no Store at {}: pass --create and --backend to make one",
        location.display(),
    ))
}

/// Which Store: its Location and the Backend that holds it, which is all it takes to open it
/// again. Two are equal when they name the same Store in the same way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreAddress {
    /// The Store's Location, always absolute, so that it means the same from any directory.
    pub location: PathBuf,
    /// The Backend that holds the Store, which is never the memory Backend, since a Store in
    /// memory has no address to open it by again.
    pub backend: BackendName,
}

impl StoreAddress {
    /// Opens the Store, or gives `None` if there is none there: it never makes one.
    pub async fn open(&self) -> Result<Option<Opened>, Failure> {
        if detect(&self.location).await?.is_none() {
            return Ok(None);
        }
        Ok(Some(self.open_or_make().await?))
    }

    /// Opens the Store, making a new one if there is none, without looking for it first.
    pub async fn open_or_make(&self) -> Result<Opened, Failure> {
        // A Backend that doesn't match the one found gives the library's `WrongBackend` when it
        // opens.
        let (store, feed) = match self.backend {
            BackendName::Fs => Store::open_fs(&self.location, FsOptions::default()).await?,
            BackendName::Sqlite => {
                Store::open_sqlite(&self.location, SqliteOptions::default()).await?
            }
            BackendName::Memory => unreachable!("the memory Backend has no Location"),
        };
        let description = self.to_string();
        Ok(Opened { store, feed, backend: self.backend, description })
    }
}

impl fmt::Display for StoreAddress {
    /// The Store's Location and its Backend, for a person to read.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.location.display(), self.backend)
    }
}

/// A Store the CLI opened, with its Change feed.
pub struct Opened {
    pub store: Store,
    /// The Store's one Change feed.
    pub feed: ChangeFeed,
    /// The Backend it was opened on, which may have been found from its Location.
    pub backend: BackendName,
    /// The Store's Location and its Backend, for a person to read.
    pub description: String,
}

impl StoreArgs {
    /// Opens the Store the flags choose. `in_shell` allows the memory Backend, which is gone as
    /// soon as the command that opened it is.
    pub async fn open(&self, in_shell: bool) -> Result<Opened, Failure> {
        if self.is_memory() {
            return self.open_memory(in_shell);
        }
        self.address().await?.open_or_make().await
    }

    /// The Location of the Store the flags choose for a new Working copy, and its Backend: the one
    /// `--backend` names, or the one found there. It can't be a Store in memory. The Store is
    /// looked for but neither opened nor made, so a Working copy refused after this leaves no
    /// Store behind.
    pub async fn address_for_working_copy(&self) -> Result<StoreAddress, Failure> {
        if self.is_memory() {
            return Err(memory_for_a_working_copy());
        }
        self.address().await
    }

    /// The Location of the Store the flags choose, which isn't one in memory, and its Backend: the
    /// one `--backend` names, or the one found there. Fails if there is no Store there and
    /// `--create` is left out, but never makes one.
    async fn address(&self) -> Result<StoreAddress, Failure> {
        let location = self.location()?;
        let backend = match (self.backend, detect(&location).await?) {
            (_, None) if !self.create => return Err(missing(&location)),
            (Some(backend), _) | (None, Some(backend)) => backend,
            (None, None) => {
                return Err(Failure::error("--create needs --backend, to say which one to make"));
            }
        };
        Ok(StoreAddress { location, backend })
    }

    /// Whether the flags choose the memory Backend.
    fn is_memory(&self) -> bool {
        self.backend == Some(BackendName::Memory)
    }

    /// Fails, naming the difference, unless each flag given agrees with `address`, the Store a
    /// Working copy belongs to. A flag left out agrees with anything. `--create` agrees with
    /// nothing, since the Working copy's Store is made only with the Working copy.
    pub fn check_matches(&self, address: &StoreAddress) -> Result<(), Failure> {
        if self.create {
            return Err(Failure::error(format!(
                "--create makes a new Store, but the Working copy's Store is {address} already: \
                 leave it out"
            )));
        }
        let mismatch = |given: String| {
            Failure::error(format!(
                "{given} doesn't match the Working copy's Store, {address}: leave it out, since \
                 the Working copy knows its Store"
            ))
        };
        if let Some(store) = &self.store
            && !same_directory(store, &address.location)
        {
            return Err(mismatch(format!("--store (or TIDINGS_STORE) {}", store.display())));
        }
        if let Some(backend) = self.backend
            && backend != address.backend
        {
            return Err(mismatch(format!("--backend (or TIDINGS_BACKEND) {backend}")));
        }
        Ok(())
    }

    /// Opens a Store on the memory Backend, if `in_shell`, and nothing says where it is.
    fn open_memory(&self, in_shell: bool) -> Result<Opened, Failure> {
        if !in_shell {
            return Err(memory_outside_the_shell());
        }
        if self.store.is_some() || self.create {
            return Err(Failure::error(
                "the memory Backend has no Location, and is always new: leave out --store and \
                 --create",
            ));
        }
        let (store, feed) = Store::open_memory();
        let description = "a new Store in memory".to_owned();
        Ok(Opened { store, feed, backend: BackendName::Memory, description })
    }

    /// The Location of the Store on the filesystem or SQLite, from `--store`, made absolute.
    pub fn location(&self) -> Result<PathBuf, Failure> {
        let Some(store) = &self.store else {
            return Err(Failure::error("say where the Store is, with --store (or TIDINGS_STORE)"));
        };
        path::absolute(store).map_err(|error| {
            Failure::error(format!("can't use --store {}: {error}", store.display()))
        })
    }
}

/// Whether `given`, as `--store` gave it, is the directory `recorded`, an absolute path: the same
/// path once made absolute, or the same directory once symlinks are followed.
fn same_directory(given: &Path, recorded: &Path) -> bool {
    if path::absolute(given).is_ok_and(|given| given == recorded) {
        return true;
    }
    match (fs::canonicalize(given), fs::canonicalize(recorded)) {
        (Ok(given), Ok(recorded)) => given == recorded,
        _ => false,
    }
}

/// The failure for the memory Backend outside `tidings store shell`.
fn memory_outside_the_shell() -> Failure {
    Failure::error(
        "the memory Backend lasts only as long as the command, so only `tidings store shell` can \
         use it",
    )
}

/// The failure for a Working copy of a Store on the memory Backend.
fn memory_for_a_working_copy() -> Failure {
    Failure::error("a Working copy can't be of a Store in memory: no other process could reach it")
}
