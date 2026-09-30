//! Text files for an application, in as many Stores as it wants, each in a directory it chooses,
//! held by the Backend it chooses: the filesystem, SQLite or memory. Writes happen only through
//! all-or-nothing Commits of a [`Staging`], and every Change is announced on the Store's
//! [`ChangeFeed`].
//!
//! The [`Store`] is async, on tokio. With the `blocking` feature, `blocking::Store` offers the
//! same for synchronous code.
//!
//! The terms used throughout (Store, Location, Path, Staging, Commit, Change...) are defined in the
//! crate's `CONTEXT.md`.
//!
//! # Opening Stores
//!
//! A Store is one Location, a directory the app passes in, and an app opens as many as it needs.
//! Nothing ties them together: each has its own Files, Change feed and Resyncs, and its own
//! Backend. tidings doesn't pick directories. For the platform's standard ones, use a crate such
//! as [`etcetera`](https://docs.rs/etcetera) and pass what it gives. Here config is on the
//! filesystem, where people can edit it, and data is in SQLite, which has Snapshots:
//!
//! ```no_run
//! use etcetera::{AppStrategy, AppStrategyArgs, choose_app_strategy};
//! use tidings::{FeedItem, FsOptions, SqliteOptions, Staging, Store};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let dirs = choose_app_strategy(AppStrategyArgs {
//!         top_level_domain: "com".to_owned(),
//!         author: "Example".to_owned(),
//!         app_name: "My App".to_owned(),
//!     })?;
//!     let (config, mut config_feed) =
//!         Store::open_fs(dirs.config_dir(), FsOptions::default()).await?;
//!     let (data, mut data_feed) =
//!         Store::open_sqlite(dirs.data_dir(), SqliteOptions::default()).await?;
//!
//!     if let Some(settings) = config.read("settings.toml").await? {
//!         println!("{}", settings.contents());
//!     }
//!     let mut staging = Staging::new();
//!     staging.write("notes/first.md", "# First\n")?;
//!     data.commit(staging).await?;
//!
//!     // Each Store has its own Change feed. Wait on them together with ordinary tokio tools.
//!     loop {
//!         tokio::select! {
//!             Some(item) = config_feed.next() => match item {
//!                 FeedItem::Changes(changes) => println!("config changed: {changes:?}"),
//!                 FeedItem::Resync => println!("read all the config again"),
//!             },
//!             Some(item) = data_feed.next() => match item {
//!                 FeedItem::Changes(changes) => println!("data changed: {changes:?}"),
//!                 FeedItem::Resync => println!("read all the data again"),
//!             },
//!             else => break,
//!         }
//!     }
//!     Ok(())
//! }
//! ```
//!
//! A Store can't be opened inside another Store's Location or a Working copy, nor at a Working
//! copy's folder: see [`Error::NestedLocation`] and [`Error::LocationIsWorkingCopy`]. The same
//! Location can be opened twice, on the same Backend: each Store sees the other's Commits as
//! external. Any Location may vanish, as a cache's does when the OS clears it: the Store makes it
//! again and sends a [`FeedItem::Resync`].

mod backend;
#[cfg(feature = "blocking")]
pub mod blocking;
mod change;
mod committed;
mod error;
mod file;
mod path;
mod precondition;
mod prefix;
mod revision;
mod snapshot;
mod staging;
mod store;

pub use backend::BackendKind;
#[cfg(feature = "fs")]
pub use backend::fs::FsOptions;
#[cfg(all(feature = "fs", feature = "testing"))]
pub use backend::fs::{FailurePoint, Pause};
#[cfg(feature = "sqlite")]
pub use backend::sqlite::SqliteOptions;
pub use change::{Change, ChangeFeed, ChangeKind, FeedItem, Origin};
pub use committed::Committed;
pub use error::{Error, Result};
pub use file::{File, Stat};
pub use path::{IntoPath, InvalidPathReason, Path};
pub use precondition::Precondition;
pub use prefix::{IntoPrefix, Prefix};
pub use revision::{ParseRevisionError, PrefixRevision, Revision};
pub use snapshot::Snapshot;
pub use staging::Staging;
pub use store::Store;

/// Compiles the README's example, so it can't drift from the API.
#[cfg(all(doctest, feature = "fs", feature = "sqlite"))]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;
