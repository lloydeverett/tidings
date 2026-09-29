//! `tidings`: read, write and watch a tidings Store from the terminal, one command at a time or
//! in a shell that keeps the Store open.

mod command;
mod failure;
mod location;
mod output;
mod shell;
mod working_copy;

use std::io::{self, ErrorKind};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use tidings::Area;

use crate::command::{AreaName, Session, StoreCommand};
use crate::failure::Failure;
use crate::location::StoreArgs;
use crate::output::{Output, Report, SyncOutput, print_stdout};
use crate::working_copy::WorkingCopy;

/// Read, write and watch a tidings Store.
///
/// Exits with 0 on success, 2 if `store read` or `store stat` finds no File, 3 on a Conflict, and
/// 1 on any other failure.
#[derive(Debug, Parser)]
#[command(name = "tidings", version)]
struct Cli {
    #[command(flatten)]
    store: StoreArgs,

    /// Print JSON instead of text for a person to read.
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

/// A top-level command.
#[derive(Debug, Subcommand)]
enum Command {
    /// Make a folder a Working copy of an Area, and keep it in step with the Store until Ctrl-C
    ///
    /// The folder must be empty or missing, or a Working copy of the Area, which is resumed: the
    /// Store flags are needed only to make a new one. Files the Store adds, changes or removes
    /// appear in it, but a file changed locally is left alone: if the Store changes it too, it is
    /// Diverged, and the Store's version is put in `.tidings/theirs/` to merge against. Nothing in
    /// the folder reaches the Store until `commit`.
    Sync {
        area: AreaName,
        /// The folder. The current directory if left out.
        folder: Option<PathBuf>,
        /// Print only divergences, resyncs and errors.
        #[arg(long)]
        quiet: bool,
    },
    /// Commit every local change in a Working copy, all together or not at all
    ///
    /// Each change requires the File to be unchanged in the Store since the Working copy last
    /// took it from, or committed it to, the Store, or to be still absent for a new file.
    Commit {
        #[command(flatten)]
        working_copy: WorkingCopyArgs,
    },
    /// Work on the Store directly: read, write and watch its Files, or keep it open in a shell
    #[command(subcommand)]
    Store(StoreSubcommand),
}

/// Which Working copy a command acts on.
#[derive(Debug, Args)]
struct WorkingCopyArgs {
    /// The Working copy's folder. Found from the current directory, or a folder above it, if
    /// left out.
    #[arg(short = 'C', value_name = "FOLDER")]
    folder: Option<PathBuf>,
}

impl WorkingCopyArgs {
    /// Opens the Working copy.
    fn open(&self) -> Result<WorkingCopy, Failure> {
        match &self.folder {
            Some(folder) => WorkingCopy::open(folder),
            None => WorkingCopy::find(&std::env::current_dir()?),
        }
    }
}

/// The commands under `tidings store`: a one-shot command, or the shell.
#[derive(Debug, Subcommand)]
enum StoreSubcommand {
    #[command(flatten)]
    OneShot(OneShot),
    /// Keep the Store open and type commands, building up Stagings over several of them
    ///
    /// Reads a script from stdin when it isn't a terminal, stopping at the first failure.
    Shell,
}

/// A command that opens the Store, does one thing, and exits.
#[derive(Debug, Subcommand)]
enum OneShot {
    #[command(flatten)]
    Store(StoreCommand),
    /// Print the Changes to the Areas named, or to every Area, until Ctrl-C
    Watch { areas: Vec<AreaName> },
}

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            let _ = error.print();
            // clap's own code for a usage error is 2, which here means a missing File.
            return if error.use_stderr() { ExitCode::from(1) } else { ExitCode::SUCCESS };
        }
    };
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("tidings: can't start the async runtime: {error}");
            return ExitCode::from(1);
        }
    };
    let output = Output { json: cli.json, at_prompt: false };
    let result = match cli.command {
        Command::Sync { area, folder, quiet } => {
            let output = SyncOutput { output, quiet };
            runtime.block_on(sync(&cli.store, output, area, folder))
        }
        Command::Commit { working_copy } => {
            runtime.block_on(commit(&cli.store, output, &working_copy))
        }
        Command::Store(StoreSubcommand::Shell) => shell::run(&runtime, &cli.store, cli.json),
        Command::Store(StoreSubcommand::OneShot(command)) => {
            runtime.block_on(one_shot(&cli.store, cli.json, command))
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(failure) => {
            eprintln!("tidings: {failure}");
            failure.exit_code()
        }
    }
}

/// Opens the Store `store` chooses, and runs `command` on it.
async fn one_shot(store: &StoreArgs, json: bool, command: OneShot) -> Result<(), Failure> {
    let opened = store.open(false).await?;
    let output = Output { json, at_prompt: false };
    match command {
        OneShot::Store(command) => {
            let report = Session::new(opened.store, false).run(command).await?;
            Ok(output.print(&report)?)
        }
        OneShot::Watch { areas } => {
            eprintln!("watching {}", opened.description);
            let areas: Vec<Area> = areas.into_iter().map(Area::from).collect();
            let mut feed = opened.feed;
            let mut ctrl_c = std::pin::pin!(tokio::signal::ctrl_c());
            loop {
                let item = tokio::select! {
                    item = feed.next() => item,
                    _ = &mut ctrl_c => return Ok(()),
                };
                // The feed ends only once the Store is dropped, which it isn't until here.
                let Some(item) = item else { return Ok(()) };
                for line in output.feed_lines(&item, &areas) {
                    match print_stdout(&format!("{line}\n")) {
                        // Whatever read the lines has gone.
                        Err(error) if error.kind() == ErrorKind::BrokenPipe => return Ok(()),
                        printed => printed?,
                    }
                }
            }
        }
    }
}

/// `sync`: makes `folder` a Working copy of `area`, or resumes the one it is, and keeps it in step
/// with the Store until Ctrl-C.
async fn sync(
    store: &StoreArgs,
    output: SyncOutput,
    area: AreaName,
    folder: Option<PathBuf>,
) -> Result<(), Failure> {
    let ctrl_c = listen_for_ctrl_c()?;
    let folder = match folder {
        Some(folder) => folder,
        None => std::env::current_dir()?,
    };
    let report = |event: &_| output.print(event);
    WorkingCopy::sync(&folder, Area::from(area), store, ctrl_c, report).await
}

/// `commit`: commits every local change in the Working copy. Any Store flags in `store` must
/// match its record.
async fn commit(
    store: &StoreArgs,
    output: Output,
    working_copy: &WorkingCopyArgs,
) -> Result<(), Failure> {
    let working_copy = working_copy.open()?;
    let opened = working_copy.open_store(store).await?;
    let report = working_copy.commit(&opened.store).await?;
    Ok(output.print(&Report::WorkingCopyCommit(report))?)
}

/// Starts listening for Ctrl-C now, and gives what finishes once it is pressed. Until it is, Ctrl-C
/// doesn't stop the process. (`tokio::signal::ctrl_c` only starts listening once it is awaited.)
#[cfg(unix)]
fn listen_for_ctrl_c() -> io::Result<impl Future<Output = ()>> {
    use tokio::signal::unix::{SignalKind, signal};
    let mut interrupts = signal(SignalKind::interrupt())?;
    Ok(async move {
        interrupts.recv().await;
    })
}

/// Starts listening for Ctrl-C now, and gives what finishes once it is pressed. Until it is, Ctrl-C
/// doesn't stop the process. (`tokio::signal::ctrl_c` only starts listening once it is awaited.)
#[cfg(windows)]
fn listen_for_ctrl_c() -> io::Result<impl Future<Output = ()>> {
    let mut interrupts = tokio::signal::windows::ctrl_c()?;
    Ok(async move {
        interrupts.recv().await;
    })
}
