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
    /// Only in `tidings shell`: a Store that lasts as long as the shell.
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
struct Identity {
    text: String,
    identity: AppIdentity,
}

impl Identity {
    fn parse(text: &str) -> Result<Identity, String> {
        let parts: Vec<&str> = text.split('.').collect();
        match parts[..] {
            [tld, author, app] if parts.iter().all(|part| !part.is_empty()) => {
                Ok(Identity { text: text.to_owned(), identity: AppIdentity::new(app, author, tld) })
            }
            _ => Err("an identity is written TLD.AUTHOR.APP, as in com.example.myapp".to_owned()),
        }
    }
}

/// A Store the CLI opened, with its Change feed.
pub struct Opened {
    pub store: Store,
    pub feed: ChangeFeed,
    pub backend: BackendName,
    /// Where the Store is, for a person to read.
    pub location: String,
}

/// Where a Store on the filesystem or SQLite is.
struct Location {
    identity: AppIdentity,
    root: Option<PathBuf>,
    /// For a person to read.
    description: String,
}

impl StoreArgs {
    /// Opens the Store the flags choose. `in_shell` allows the memory Backend, which is gone as
    /// soon as the command that opened it is.
    pub async fn open(&self, in_shell: bool) -> Result<Opened, Failure> {
        if self.backend == Some(BackendName::Memory) {
            return self.open_memory(in_shell);
        }
        let location = self.location()?;
        let detected = Store::detect(&location.identity, location.root.as_deref()).await?;
        let backend = match (detected, self.backend) {
            // A Backend that doesn't match gives the library's `WrongBackend` when it opens.
            (Some(_), Some(backend)) => backend,
            (None, Some(backend)) if self.create => backend,
            (Some(kind), None) => kind.into(),
            (None, None) if self.create => {
                return Err(Failure::error("--create needs --backend, to say which one to make"));
            }
            (None, _) => {
                return Err(Failure::error(format!(
                    "there is no Store at {}: pass --create and --backend to make one",
                    location.description,
                )));
            }
        };
        let (store, feed) = match backend {
            BackendName::Fs => {
                let mut options = FsOptions::default();
                if let Some(root) = &location.root {
                    options = options.root_override(root);
                }
                Store::open_fs(&location.identity, options).await?
            }
            BackendName::Sqlite => {
                let mut options = SqliteOptions::default();
                if let Some(root) = &location.root {
                    options = options.root_override(root);
                }
                Store::open_sqlite(&location.identity, options).await?
            }
            BackendName::Memory => unreachable!("opened above"),
        };
        Ok(Opened { store, feed, backend, location: location.description })
    }

    fn open_memory(&self, in_shell: bool) -> Result<Opened, Failure> {
        if !in_shell {
            return Err(Failure::error(
                "the memory Backend lasts only as long as the command, so only `tidings shell` \
                 can use it",
            ));
        }
        if self.root.is_some() || self.identity.is_some() {
            return Err(Failure::error(
                "the memory Backend has no location: leave out --root and --identity",
            ));
        }
        let (store, feed) = Store::open_memory();
        Ok(Opened { store, feed, backend: BackendName::Memory, location: "memory".to_owned() })
    }

    fn location(&self) -> Result<Location, Failure> {
        match (&self.root, &self.identity) {
            (Some(root), None) => Ok(Location {
                // The Root override replaces every directory the App identity would give, so
                // any identity will do.
                identity: AppIdentity::new("tidings", "tidings", "cli"),
                root: Some(root.clone()),
                description: root.display().to_string(),
            }),
            (None, Some(identity)) => Ok(Location {
                identity: identity.identity.clone(),
                root: None,
                description: format!("the directories of {}", identity.text),
            }),
            (Some(_), Some(_)) => Err(Failure::error("give either --root or --identity, not both")),
            (None, None) => Err(Failure::error(
                "say where the Store is, with --root or --identity (or TIDINGS_ROOT or \
                 TIDINGS_IDENTITY)",
            )),
        }
    }
}
