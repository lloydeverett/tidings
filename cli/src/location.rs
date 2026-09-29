//! Which Store a command opens: the flags that choose one, and opening it.

use std::fmt;
use std::path::PathBuf;

use clap::{Args, ValueEnum};
use tidings::{AppIdentity, BackendKind, ChangeFeed, FsOptions, SqliteOptions, Store};

use crate::failure::Failure;

/// The flags that choose a Store, which every command takes.
#[derive(Debug, Args)]
pub struct StoreArgs {
    /// The directory holding the Store's Areas, as `config`, `data` and `cache` in it.
    #[arg(long, global = true, env = "TIDINGS_ROOT", value_name = "DIR")]
    root: Option<PathBuf>,

    /// The App identity whose standard directories hold the Store, as in `com.example.myapp`.
    #[arg(
        long,
        global = true,
        env = "TIDINGS_IDENTITY",
        value_name = "TLD.AUTHOR.APP",
        value_parser = Identity::parse,
    )]
    identity: Option<Identity>,

    /// The Backend that holds the Store. Found from the Store's location when left out.
    #[arg(long, global = true, env = "TIDINGS_BACKEND")]
    backend: Option<BackendName>,

    /// Make a new Store where there is none. Needs `--backend`.
    #[arg(long, global = true)]
    create: bool,
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

/// An App identity written as `tld.author.app`, in the order of a reverse domain name.
#[derive(Debug, Clone)]
pub struct Identity {
    text: String,
    identity: AppIdentity,
}

impl Identity {
    /// Parses `tld.author.app`: exactly three parts, none of them empty.
    pub fn parse(text: &str) -> Result<Identity, String> {
        let parts: Vec<&str> = text.split('.').collect();
        match parts[..] {
            [tld, author, app] if parts.iter().all(|part| !part.is_empty()) => {
                Ok(Identity { text: text.to_owned(), identity: AppIdentity::new(app, author, tld) })
            }
            _ => Err("an identity is written TLD.AUTHOR.APP, as in com.example.myapp".to_owned()),
        }
    }
}

impl fmt::Display for Identity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

/// Where a Store on the filesystem or SQLite is.
#[derive(Debug, Clone)]
pub enum StoreLocation {
    /// A Root override, always absolute, so that it means the same from any directory.
    Root(PathBuf),
    /// The standard directories of an App identity.
    Identity(Identity),
}

impl StoreLocation {
    /// For a person to read.
    fn description(&self) -> String {
        match self {
            StoreLocation::Root(root) => root.display().to_string(),
            StoreLocation::Identity(identity) => format!("the directories of {identity}"),
        }
    }

    /// The App identity to open the Store with.
    fn app_identity(&self) -> AppIdentity {
        match self {
            // The Root override replaces every directory the App identity would give, so any
            // identity will do.
            StoreLocation::Root(_) => AppIdentity::new("tidings", "tidings", "cli"),
            StoreLocation::Identity(identity) => identity.identity.clone(),
        }
    }

    /// The Root override, if there is one.
    fn root(&self) -> Option<&std::path::Path> {
        match self {
            StoreLocation::Root(root) => Some(root),
            StoreLocation::Identity(_) => None,
        }
    }

    /// Opens the Store here, on `backend`, or on the Backend found here if that is `None`.
    /// `create` makes a new Store where there is none.
    pub async fn open(
        &self,
        backend: Option<BackendName>,
        create: bool,
    ) -> Result<Opened, Failure> {
        let (identity, root) = (self.app_identity(), self.root());
        let detected = Store::detect(&identity, root).await?;
        let backend = match (detected, backend) {
            // A Backend that doesn't match gives the library's `WrongBackend` when it opens.
            (Some(_), Some(backend)) => backend,
            (None, Some(backend)) if create => backend,
            (Some(kind), None) => kind.into(),
            (None, None) if create => {
                return Err(Failure::error("--create needs --backend, to say which one to make"));
            }
            (None, _) => {
                return Err(Failure::error(format!(
                    "there is no Store at {}: pass --create and --backend to make one",
                    self.description(),
                )));
            }
        };
        let (store, feed) = match backend {
            BackendName::Fs => {
                let mut options = FsOptions::default();
                if let Some(root) = root {
                    options = options.root_override(root);
                }
                Store::open_fs(&identity, options).await?
            }
            BackendName::Sqlite => {
                let mut options = SqliteOptions::default();
                if let Some(root) = root {
                    options = options.root_override(root);
                }
                Store::open_sqlite(&identity, options).await?
            }
            BackendName::Memory => unreachable!("the memory Backend has no location"),
        };
        let description = format!("{} ({backend})", self.description());
        Ok(Opened { store, feed, backend, description })
    }
}

/// A Store the CLI opened, with its Change feed.
pub struct Opened {
    pub store: Store,
    /// The Store's one Change feed.
    pub feed: ChangeFeed,
    /// The Backend it was opened on, which may have been found from its location.
    pub backend: BackendName,
    /// Where the Store is and its Backend, for a person to read.
    pub description: String,
}

impl StoreArgs {
    /// Opens the Store the flags choose. `in_shell` allows the memory Backend, which is gone as
    /// soon as the command that opened it is.
    pub async fn open(&self, in_shell: bool) -> Result<Opened, Failure> {
        if self.backend == Some(BackendName::Memory) {
            return self.open_memory(in_shell);
        }
        self.location()?.open(self.backend, self.create).await
    }

    /// Opens a Store on the memory Backend, if `in_shell`, and nothing says where it is.
    fn open_memory(&self, in_shell: bool) -> Result<Opened, Failure> {
        if !in_shell {
            return Err(Failure::error(
                "the memory Backend lasts only as long as the command, so only \
                 `tidings store shell` can use it",
            ));
        }
        if self.root.is_some() || self.identity.is_some() || self.create {
            return Err(Failure::error(
                "the memory Backend has no location, and is always new: leave out --root, \
                 --identity and --create",
            ));
        }
        let (store, feed) = Store::open_memory();
        let description = "a new Store in memory".to_owned();
        Ok(Opened { store, feed, backend: BackendName::Memory, description })
    }

    /// Where the Store on the filesystem or SQLite is, from exactly one of `--root` and
    /// `--identity`.
    pub fn location(&self) -> Result<StoreLocation, Failure> {
        match (&self.root, &self.identity) {
            (Some(root), None) => match std::path::absolute(root) {
                Ok(root) => Ok(StoreLocation::Root(root)),
                Err(error) => {
                    Err(Failure::error(format!("can't use --root {}: {error}", root.display())))
                }
            },
            (None, Some(identity)) => Ok(StoreLocation::Identity(identity.clone())),
            (Some(_), Some(_)) => Err(Failure::error("give either --root or --identity, not both")),
            (None, None) => Err(Failure::error(
                "say where the Store is, with --root or --identity (or TIDINGS_ROOT or \
                 TIDINGS_IDENTITY)",
            )),
        }
    }
}
