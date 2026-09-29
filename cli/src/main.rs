//! `tidings`: read, write and watch a tidings Store from the terminal, one command at a time or
//! in a shell that keeps the Store open.

mod command;
mod failure;
mod location;
mod output;
mod shell;

use std::io::ErrorKind;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use tidings::Area;

use crate::command::{AreaName, Session, StoreCommand};
use crate::failure::Failure;
use crate::location::StoreArgs;
use crate::output::{Output, print_stdout};

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
    /// Work on the Store directly: read, write and watch its Files, or keep it open in a shell
    #[command(subcommand)]
    Store(StoreGroup),
}

/// The commands under `tidings store`: a one-shot command, or the shell.
#[derive(Debug, Subcommand)]
enum StoreGroup {
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
    let result = match cli.command {
        Command::Store(StoreGroup::Shell) => shell::run(&runtime, &cli.store, cli.json),
        Command::Store(StoreGroup::OneShot(command)) => {
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
