//! The commands that work on a Store, both as one-shot commands and in the shell, and the
//! session that runs them.

mod edit;

use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;

use clap::{Args, Subcommand};
use tempfile::TempPath;
use tidings::{Precondition, Prefix, PrefixRevision, Revision, Staging, Store};

use crate::failure::Failure;
use crate::output::Report;

/// The commands shared by one-shot mode and the shell.
#[derive(Debug, Subcommand)]
pub enum StoreCommand {
    /// Print a File's contents, exactly as stored
    Read { path: String },
    /// Print a File's Revision and when it was last modified
    Stat { path: String },
    /// Print the Paths of the Files under a Prefix, in order
    List {
        /// Empty, for the whole Store, or ending in `/`.
        #[arg(default_value = "")]
        prefix: String,
    },
    /// Print the Prefix Revision of everything under a Prefix
    ///
    /// The shell remembers it for `require-prefix`.
    StatPrefix {
        /// Empty, for the whole Store, or ending in `/`.
        #[arg(default_value = "")]
        prefix: String,
    },
    /// Write a File, and print its new Revision
    Write {
        path: String,
        #[command(flatten)]
        contents: ContentsArgs,
        #[command(flatten)]
        precondition: PreconditionArgs,
    },
    /// Delete a File, if there is one
    Delete {
        path: String,
        #[command(flatten)]
        precondition: PreconditionArgs,
    },
    /// Delete every File under a Prefix
    DeletePrefix {
        /// Ending in `/`, or empty for the whole Store.
        prefix: String,
    },
    /// Edit a File in $VISUAL or $EDITOR, and write it back unless it changed meanwhile
    ///
    /// A missing File starts empty, and is written only if it is still missing. Quitting the
    /// editor with a failure, as with `:cq` in vim, cancels the edit. On a Conflict, the edited
    /// text is kept in a temporary file, which is named.
    Edit { path: String },
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
    /// The last Prefix Revision `stat-prefix` gave for each Prefix.
    prefix_revisions: HashMap<Prefix, PrefixRevision>,
}

/// The shell's open Staging.
struct OpenStaging {
    staging: Staging,
    /// How many things were staged, for the prompt.
    count: usize,
    /// The temporary files holding the text of each `edit` staged, so that it can be kept if the
    /// Commit fails. Dropping them removes them.
    edits: Vec<TempPath>,
}

impl OpenStaging {
    fn new() -> OpenStaging {
        OpenStaging { staging: Staging::new(), count: 0, edits: Vec::new() }
    }

    /// Stages what `stage` does, and counts it.
    fn add(
        &mut self,
        stage: impl FnOnce(&mut Staging) -> tidings::Result<()>,
    ) -> Result<Report, Failure> {
        stage(&mut self.staging)?;
        self.count += 1;
        Ok(Report::Staged { count: self.count })
    }
}

impl Session {
    /// A session on `store`, in the shell if `shell`.
    pub fn new(store: Store, shell: bool) -> Session {
        Session { store, shell, staging: None, prefix_revisions: HashMap::new() }
    }

    /// How many things the open Staging holds, if one is open.
    pub fn staged_count(&self) -> Option<usize> {
        self.staging.as_ref().map(|open| open.count)
    }

    /// Runs `command`.
    pub async fn run(&mut self, command: StoreCommand) -> Result<Report, Failure> {
        match command {
            StoreCommand::Read { path } => match self.store.read(path.as_str()).await? {
                Some(file) => Ok(Report::File(file)),
                None => Err(Failure::missing(&path)),
            },
            StoreCommand::Stat { path } => match self.store.stat(path.as_str()).await? {
                Some(stat) => Ok(Report::Stat(tidings::Path::new(path)?, stat)),
                None => Err(Failure::missing(&path)),
            },
            StoreCommand::List { prefix } => Ok(Report::Paths(self.store.list(prefix).await?)),
            StoreCommand::StatPrefix { prefix } => {
                let prefix = Prefix::new(prefix)?;
                let revision = self.store.stat_prefix(prefix.clone()).await?;
                self.prefix_revisions.insert(prefix.clone(), revision.clone());
                Ok(Report::PrefixRevision(prefix, revision))
            }
            StoreCommand::Write { path, contents, precondition } => {
                let contents = self.contents(contents)?;
                let precondition = precondition.precondition();
                self.stage_or_commit(|staging| {
                    staging.write_requiring(path, contents, precondition).map(drop)
                })
                .await
            }
            StoreCommand::Delete { path, precondition } => {
                let precondition = precondition.precondition();
                self.stage_or_commit(|staging| {
                    staging.delete_requiring(path, precondition).map(drop)
                })
                .await
            }
            StoreCommand::DeletePrefix { prefix } => {
                self.stage_or_commit(|staging| staging.delete_prefix(prefix).map(drop)).await
            }
            StoreCommand::Edit { path } => self.edit(&path).await,
        }
    }

    /// Adds what `stage` stages to the open Staging, or, with none open, commits it straight
    /// away.
    async fn stage_or_commit(
        &mut self,
        stage: impl FnOnce(&mut Staging) -> tidings::Result<()>,
    ) -> Result<Report, Failure> {
        match &mut self.staging {
            Some(open) => open.add(stage),
            None => {
                let mut staging = Staging::new();
                stage(&mut staging)?;
                Ok(Report::Committed(self.store.commit(staging).await?))
            }
        }
    }

    /// `stage`: opens a Staging.
    pub fn open_staging(&mut self) -> Result<Report, Failure> {
        if self.staging.is_some() {
            return Err(Failure::error("a Staging is open already: commit or discard it first"));
        }
        self.staging = Some(OpenStaging::new());
        Ok(Report::Nothing)
    }

    /// `require`: requires `precondition` of the File at `path` in the open Staging.
    pub fn require(&mut self, path: &str, precondition: Precondition) -> Result<Report, Failure> {
        self.open()?.add(|staging| staging.require(path, precondition).map(drop))
    }

    /// `require-prefix`: requires everything under `prefix` to be unchanged since the last
    /// `stat-prefix` of it, in the open Staging.
    pub fn require_prefix(&mut self, prefix: String) -> Result<Report, Failure> {
        let prefix = Prefix::new(prefix)?;
        self.require_open()?;
        let Some(revision) = self.prefix_revisions.get(&prefix) else {
            return Err(Failure::error(format!(
                "no Prefix Revision of {:?} to require: run `stat-prefix {}` first",
                prefix.as_str(),
                prefix.as_str(),
            )));
        };
        let revision = revision.clone();
        self.open()?.add(|staging| staging.require_prefix(prefix, revision).map(drop))
    }

    /// `commit`: commits the open Staging, which is closed whether or not the Commit succeeds.
    /// If it fails, the text of each `edit` staged is kept.
    pub async fn commit(&mut self) -> Result<Report, Failure> {
        self.require_open()?;
        let open = self.staging.take().expect("checked above");
        match self.store.commit(open.staging).await {
            Ok(committed) => Ok(Report::Committed(committed)),
            Err(error) => {
                let failure = Failure::from(error);
                Err(open.edits.into_iter().fold(failure, keep_edit))
            }
        }
    }

    /// `discard`: closes the open Staging without committing it.
    pub fn discard(&mut self) -> Result<Report, Failure> {
        self.require_open()?;
        self.staging = None;
        Ok(Report::Nothing)
    }

    /// The open Staging, or an error if there is none.
    fn open(&mut self) -> Result<&mut OpenStaging, Failure> {
        self.staging.as_mut().ok_or_else(no_staging_open)
    }

    /// An error if no Staging is open.
    fn require_open(&self) -> Result<(), Failure> {
        match self.staging {
            Some(_) => Ok(()),
            None => Err(no_staging_open()),
        }
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

/// `failure` with a note of where the edited text in `edit` is kept, now that it won't be
/// written.
fn keep_edit(failure: Failure, edit: TempPath) -> Failure {
    match edit.keep() {
        Ok(kept) => failure.with_note(format!("your edit is kept in {}", kept.display())),
        Err(error) => failure.with_note(format!("your edit couldn't be kept: {error}")),
    }
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

/// The error for a command that needs an open Staging when none is open.
fn no_staging_open() -> Failure {
    Failure::error("no Staging is open: open one with `stage`")
}
