//! The commands that work on a Store, both as one-shot commands and in the shell, and the
//! session that runs them.

use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;

use clap::{Args, Subcommand, ValueEnum};
use tidings::{Area, Precondition, Prefix, PrefixRevision, Revision, Staging, Store};

use crate::edit;
use crate::failure::Failure;
use crate::output::{Report, area_name};

/// The commands shared by one-shot mode and the shell.
#[derive(Debug, Subcommand)]
pub enum StoreCommand {
    /// Print a File's contents, exactly as stored
    Read { area: AreaName, path: String },
    /// Print a File's Revision and when it was last modified
    Stat { area: AreaName, path: String },
    /// Print the Paths of the Files under a Prefix, in order
    List {
        area: AreaName,
        /// Empty, for the whole Area, or ending in `/`.
        #[arg(default_value = "")]
        prefix: String,
    },
    /// Print the Prefix Revision of everything under a Prefix
    ///
    /// The shell remembers it for `require-prefix`.
    StatPrefix {
        area: AreaName,
        /// Empty, for the whole Area, or ending in `/`.
        #[arg(default_value = "")]
        prefix: String,
    },
    /// Write a File, and print its new Revision
    Write {
        area: AreaName,
        path: String,
        #[command(flatten)]
        contents: ContentsArgs,
        #[command(flatten)]
        precondition: PreconditionArgs,
    },
    /// Delete a File, if there is one
    Delete {
        area: AreaName,
        path: String,
        #[command(flatten)]
        precondition: PreconditionArgs,
    },
    /// Delete every File under a Prefix
    DeletePrefix {
        area: AreaName,
        /// Ending in `/`, or empty for the whole Area.
        prefix: String,
    },
    /// Edit a File in $VISUAL or $EDITOR, and write it back unless it changed meanwhile
    ///
    /// A missing File starts empty, and is written only if it is still missing. On a Conflict,
    /// the edited text is kept in a temporary file, which is named.
    Edit { area: AreaName, path: String },
}

/// An Area, as it is named on the command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum AreaName {
    Config,
    Data,
    Cache,
}

impl From<AreaName> for Area {
    fn from(name: AreaName) -> Area {
        match name {
            AreaName::Config => Area::Config,
            AreaName::Data => Area::Data,
            AreaName::Cache => Area::Cache,
        }
    }
}

/// Where `write` takes the contents from: stdin, unless one of these is given.
#[derive(Debug, Args)]
#[group(multiple = false)]
pub struct ContentsArgs {
    /// The contents. In the shell, `\n`, `\t` and `\\` in them are a newline, a tab and a
    /// backslash.
    #[arg(long)]
    contents: Option<String>,
    /// A file holding the contents.
    #[arg(long, value_name = "FILE")]
    from: Option<PathBuf>,
}

/// What a write or delete requires of the File there, or nothing.
#[derive(Debug, Args)]
#[group(multiple = false)]
pub struct PreconditionArgs {
    /// Only if there is no File there.
    #[arg(long)]
    if_absent: bool,
    /// Only if the File is unchanged since it had this Revision.
    #[arg(long, value_name = "REVISION")]
    if_revision: Option<Revision>,
}

impl PreconditionArgs {
    fn precondition(&self) -> Precondition {
        match (self.if_absent, self.if_revision) {
            (true, _) => Precondition::Absent,
            (false, Some(revision)) => Precondition::UnchangedSince(revision),
            (false, None) => Precondition::Any,
        }
    }
}

/// A Store and what the commands run on it keep between them: in one-shot mode for one command,
/// in the shell for the whole session.
pub struct Session {
    store: Store,
    /// Whether this is the shell, where stdin is the commands, not a File's contents.
    shell: bool,
    /// The Staging opened with `stage`, which writes and deletes add to instead of committing.
    staging: Option<OpenStaging>,
    /// The last Prefix Revision `stat-prefix` gave for each Area and Prefix.
    prefix_revisions: HashMap<(Area, Prefix), PrefixRevision>,
}

/// The shell's open Staging.
pub struct OpenStaging {
    staging: Staging,
    /// How many things were staged, for the prompt.
    count: usize,
}

impl Session {
    pub fn new(store: Store, shell: bool) -> Session {
        Session { store, shell, staging: None, prefix_revisions: HashMap::new() }
    }

    /// The open Staging's Area, and how many things it holds.
    pub fn staging(&self) -> Option<(Area, usize)> {
        self.staging.as_ref().map(|open| (open.staging.area(), open.count))
    }

    pub async fn run(&mut self, command: StoreCommand) -> Result<Report, Failure> {
        match command {
            StoreCommand::Read { area, path } => {
                let area = area.into();
                match self.store.read(area, path.as_str()).await? {
                    Some(file) => Ok(Report::File(file)),
                    None => Err(Failure::missing(area, &path)),
                }
            }
            StoreCommand::Stat { area, path } => {
                let area = area.into();
                match self.store.stat(area, path.as_str()).await? {
                    Some(stat) => Ok(Report::Stat(tidings::Path::new(path)?, stat)),
                    None => Err(Failure::missing(area, &path)),
                }
            }
            StoreCommand::List { area, prefix } => {
                Ok(Report::Paths(self.store.list(area.into(), prefix).await?))
            }
            StoreCommand::StatPrefix { area, prefix } => {
                let (area, prefix) = (area.into(), Prefix::new(prefix)?);
                let revision = self.store.stat_prefix(area, prefix.clone()).await?;
                self.prefix_revisions.insert((area, prefix.clone()), revision.clone());
                Ok(Report::PrefixRevision(area, prefix, revision))
            }
            StoreCommand::Write { area, path, contents, precondition } => {
                let contents = self.contents(contents)?;
                let precondition = precondition.precondition();
                self.stage_or_commit(area.into(), |staging| {
                    staging.write_requiring(path, contents, precondition).map(drop)
                })
                .await
            }
            StoreCommand::Delete { area, path, precondition } => {
                let precondition = precondition.precondition();
                self.stage_or_commit(area.into(), |staging| {
                    staging.delete_requiring(path, precondition).map(drop)
                })
                .await
            }
            StoreCommand::DeletePrefix { area, prefix } => {
                self.stage_or_commit(area.into(), |staging| staging.delete_prefix(prefix).map(drop))
                    .await
            }
            StoreCommand::Edit { area, path } => edit::edit(self, area.into(), &path).await,
        }
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    /// Adds what `stage` stages to the open Staging, which must be for `area`, or, with none
    /// open, commits it straight away.
    pub async fn stage_or_commit(
        &mut self,
        area: Area,
        stage: impl FnOnce(&mut Staging) -> tidings::Result<()>,
    ) -> Result<Report, Failure> {
        match &mut self.staging {
            Some(open) => {
                check_area(&open.staging, area)?;
                stage(&mut open.staging)?;
                open.count += 1;
                Ok(Report::Staged { area, count: open.count })
            }
            None => {
                let mut staging = Staging::new(area);
                stage(&mut staging)?;
                Ok(Report::Committed(self.store.commit(staging).await?))
            }
        }
    }

    /// `stage`: opens a Staging for `area`.
    pub fn open_staging(&mut self, area: Area) -> Result<Report, Failure> {
        if let Some((open, _)) = self.staging() {
            return Err(Failure::error(format!(
                "a Staging for {} is open already: commit or discard it first",
                area_name(open)
            )));
        }
        self.staging = Some(OpenStaging { staging: Staging::new(area), count: 0 });
        Ok(Report::Nothing)
    }

    /// `require`: requires `precondition` of the File at `path` in the open Staging.
    pub fn require(&mut self, path: &str, precondition: Precondition) -> Result<Report, Failure> {
        let open = self.open()?;
        open.staging.require(path, precondition)?;
        open.count += 1;
        Ok(Report::Staged { area: open.staging.area(), count: open.count })
    }

    /// `require-prefix`: requires everything under `prefix` to be unchanged since the last
    /// `stat-prefix` of it, in the open Staging's Area.
    pub fn require_prefix(&mut self, prefix: String) -> Result<Report, Failure> {
        let prefix = Prefix::new(prefix)?;
        let area = self.open()?.staging.area();
        let Some(revision) = self.prefix_revisions.get(&(area, prefix.clone())) else {
            return Err(Failure::error(format!(
                "no Prefix Revision of {} {:?} to require: run `stat-prefix {} {}` first",
                area_name(area),
                prefix.as_str(),
                area_name(area),
                prefix.as_str(),
            )));
        };
        let revision = revision.clone();
        let open = self.open()?;
        open.staging.require_prefix(prefix, revision)?;
        open.count += 1;
        Ok(Report::Staged { area, count: open.count })
    }

    /// `commit`: commits the open Staging, which is closed whether or not the Commit succeeds.
    pub async fn commit(&mut self) -> Result<Report, Failure> {
        self.open()?;
        let open = self.staging.take().expect("checked above");
        Ok(Report::Committed(self.store.commit(open.staging).await?))
    }

    /// `discard`: closes the open Staging without committing it.
    pub fn discard(&mut self) -> Result<Report, Failure> {
        self.open()?;
        self.staging = None;
        Ok(Report::Nothing)
    }

    /// The open Staging, or an error if there is none.
    fn open(&mut self) -> Result<&mut OpenStaging, Failure> {
        self.staging
            .as_mut()
            .ok_or_else(|| Failure::error("no Staging is open: open one with `stage`"))
    }

    /// The contents `write` is to write.
    fn contents(&self, args: ContentsArgs) -> Result<String, Failure> {
        match (args.contents, args.from) {
            (Some(contents), _) if self.shell => Ok(unescape(&contents)),
            (Some(contents), _) => Ok(contents),
            (None, Some(from)) => std::fs::read_to_string(&from)
                .map_err(|error| Failure::error(format!("can't read {}: {error}", from.display()))),
            (None, None) if self.shell => {
                Err(Failure::error("give the contents with --contents or --from"))
            }
            (None, None) => {
                let mut contents = String::new();
                std::io::stdin()
                    .read_to_string(&mut contents)
                    .map_err(|error| Failure::error(format!("can't read stdin: {error}")))?;
                Ok(contents)
            }
        }
    }
}

/// Fails unless `staging` is for `area`.
fn check_area(staging: &Staging, area: Area) -> Result<(), Failure> {
    if staging.area() == area {
        return Ok(());
    }
    Err(Failure::error(format!(
        "the open Staging is for {}, not {}: commit or discard it first",
        area_name(staging.area()),
        area_name(area),
    )))
}

/// `text` with `\n`, `\t` and `\\` made a newline, a tab and a backslash. Any other backslash
/// stays as it is.
fn unescape(text: &str) -> String {
    let mut unescaped = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            unescaped.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => unescaped.push('\n'),
            Some('t') => unescaped.push('\t'),
            Some('\\') => unescaped.push('\\'),
            Some(other) => unescaped.extend(['\\', other]),
            None => unescaped.push('\\'),
        }
    }
    unescaped
}
