//! `tidings shell`: one Store kept open while commands are typed, or read from a script.

use std::io::{self, BufRead, IsTerminal};
use std::pin::pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use clap::{Parser, Subcommand, ValueEnum};
use rustyline::error::ReadlineError;
use rustyline::{DefaultEditor, ExternalPrinter};
use tidings::{ChangeFeed, FeedItem, Precondition};
use tokio::runtime::Runtime;

use crate::command::{AreaName, Session, StoreCommand};
use crate::failure::Failure;
use crate::location::{BackendName, StoreArgs};
use crate::output::{Output, Report, area_name};

/// One line typed in the shell.
#[derive(Debug, Parser)]
#[command(
    no_binary_name = true,
    name = "tidings shell",
    override_usage = "<COMMAND> [ARGS]...",
    disable_version_flag = true,
    help_template = "{all-args}"
)]
struct ShellLine {
    #[command(subcommand)]
    command: ShellCommand,
}

/// The commands the shell takes: those one-shot mode takes, apart from `watch`, and its own.
#[derive(Debug, Subcommand)]
enum ShellCommand {
    #[command(flatten)]
    Store(StoreCommand),
    /// Open a Staging for an Area, to build up a Commit
    ///
    /// `write`, `delete`, `delete-prefix`, `edit`, `require` and `require-prefix` add to it,
    /// instead of committing, until `commit` or `discard`.
    Stage { area: AreaName },
    /// Require a File to be absent, or unchanged since a Revision, in the open Staging
    Require {
        path: String,
        #[arg(value_name = "absent|REVISION", value_parser = parse_required)]
        precondition: Precondition,
    },
    /// Require a Prefix to be unchanged since its last `stat-prefix`, in the open Staging
    RequirePrefix {
        #[arg(default_value = "")]
        prefix: String,
    },
    /// Commit the open Staging, all or nothing
    ///
    /// The Staging is closed even if the Commit fails.
    Commit,
    /// Close the open Staging without committing it
    Discard,
    /// Print the Changes on the Change feed as they arrive, or stop
    Feed { state: Switch },
    /// Leave the shell, discarding an open Staging
    #[command(alias = "quit")]
    Exit,
}

/// `on` or `off`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Switch {
    On,
    Off,
}

/// Parses what `require` requires: `absent`, or a Revision.
fn parse_required(text: &str) -> Result<Precondition, String> {
    if text == "absent" {
        return Ok(Precondition::Absent);
    }
    text.parse()
        .map(Precondition::UnchangedSince)
        .map_err(|_| "expected `absent` or a Revision of 32 hexadecimal digits".to_owned())
}

/// Runs the shell on the Store `args` chooses: interactive on a terminal, or a script from stdin.
pub fn run(runtime: &Runtime, args: &StoreArgs, json: bool) -> Result<(), Failure> {
    let opened = runtime.block_on(args.open(true))?;
    let mut shell = Shell {
        session: Session::new(opened.store, true),
        output: Output { json, shell: true },
        backend: opened.backend,
    };
    if io::stdin().is_terminal() {
        eprintln!(
            "tidings shell on {}: `help` lists the commands, `exit` or Ctrl-D leaves",
            opened.description,
        );
        shell.interactive(runtime, opened.feed)
    } else {
        shell.script(runtime, opened.feed)
    }
}

/// The shell's state, and how it prints.
struct Shell {
    session: Session,
    output: Output,
    /// For the prompt.
    backend: BackendName,
}

/// Whether to go on reading commands after one.
enum Flow {
    Continue,
    /// `exit`.
    Exit,
}

impl Shell {
    /// Reads commands with line editing, printing Changes as they arrive, until `exit` or
    /// Ctrl-D. A failed command is reported and the shell carries on.
    fn interactive(&mut self, runtime: &Runtime, feed: ChangeFeed) -> Result<(), Failure> {
        let mut editor =
            DefaultEditor::new().map_err(|error| Failure::error(format!("{error}")))?;
        let printer =
            editor.create_external_printer().map_err(|error| Failure::error(format!("{error}")))?;
        let feed_lines =
            Arc::new(Mutex::new(FeedLines { on: true, at_prompt: false, held: Vec::new() }));
        runtime.spawn(print_feed(feed, self.output, Arc::clone(&feed_lines), printer));
        // Ctrl-C while a command or the editor runs leaves the shell open. At the prompt, the
        // line editor reads it as a key instead.
        runtime.spawn(async { while tokio::signal::ctrl_c().await.is_ok() {} });
        loop {
            feed_lines.lock().unwrap().release_held();
            let line = editor.readline(&self.prompt());
            feed_lines.lock().unwrap().at_prompt = false;
            let line = match line {
                Ok(line) => line,
                Err(ReadlineError::Interrupted) => continue,
                Err(ReadlineError::Eof) => break,
                Err(error) => return Err(Failure::error(format!("can't read a line: {error}"))),
            };
            let _ = editor.add_history_entry(line.as_str());
            match self.execute(runtime, &line, |on| feed_lines.lock().unwrap().switch(on)) {
                Ok(Flow::Continue) => {}
                Ok(Flow::Exit) => break,
                Err(failure) => eprintln!("error: {failure}"),
            }
        }
        self.leave();
        Ok(())
    }

    /// Runs the commands on stdin, one per line, with no prompt, stopping at the first that
    /// fails with its failure. Changes are printed on stderr after each command, once `feed on`.
    fn script(&mut self, runtime: &Runtime, mut feed: ChangeFeed) -> Result<(), Failure> {
        let mut on = false;
        let mut result = Ok(());
        for (number, line) in io::stdin().lock().lines().enumerate() {
            let line =
                line.map_err(|error| Failure::error(format!("can't read stdin: {error}")))?;
            let flow = self.execute(runtime, &line, |switch| on = switch);
            for item in std::iter::from_fn(|| next_ready(&mut feed)) {
                if on {
                    self.output.feed_lines(&item, &[]).iter().for_each(|line| eprintln!("{line}"));
                }
            }
            match flow {
                Ok(Flow::Continue) => {}
                Ok(Flow::Exit) => break,
                Err(failure) => {
                    result = Err(failure.in_context(format!("line {}", number + 1)));
                    break;
                }
            }
        }
        self.leave();
        result
    }

    /// Runs one line. `switch_feed` turns printing Changes on or off.
    fn execute(
        &mut self,
        runtime: &Runtime,
        line: &str,
        switch_feed: impl FnOnce(bool),
    ) -> Result<Flow, Failure> {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            return Ok(Flow::Continue);
        }
        let words = shell_words::split(line)
            .map_err(|error| Failure::error(format!("can't split the line into words: {error}")))?;
        let command = match ShellLine::try_parse_from(words) {
            Ok(line) => line.command,
            Err(error) if !error.use_stderr() => {
                // Help, which clap prints on stdout, in colour on a terminal.
                error.print()?;
                return Ok(Flow::Continue);
            }
            Err(error) => {
                let message = error.render().to_string();
                let message = message.trim_end();
                return Err(Failure::error(message.strip_prefix("error: ").unwrap_or(message)));
            }
        };
        let report = match command {
            ShellCommand::Store(command) => runtime.block_on(self.session.run(command))?,
            ShellCommand::Stage { area } => self.session.open_staging(area.into())?,
            ShellCommand::Require { path, precondition } => {
                self.session.require(&path, precondition)?
            }
            ShellCommand::RequirePrefix { prefix } => self.session.require_prefix(prefix)?,
            ShellCommand::Commit => runtime.block_on(self.session.commit())?,
            ShellCommand::Discard => self.session.discard()?,
            ShellCommand::Feed { state } => {
                switch_feed(state == Switch::On);
                Report::Nothing
            }
            ShellCommand::Exit => return Ok(Flow::Exit),
        };
        self.output.print(&report)?;
        Ok(Flow::Continue)
    }

    /// `tidings[fs]> `, or with a Staging open, `tidings[fs data +2]> `.
    fn prompt(&self) -> String {
        match self.session.staging() {
            Some((area, count)) => {
                format!("tidings[{} {} +{count}]> ", self.backend, area_name(area))
            }
            None => format!("tidings[{}]> ", self.backend),
        }
    }

    /// Says so if an open Staging is being discarded as the shell ends.
    fn leave(&self) {
        if let Some((area, count)) = self.session.staging() {
            eprintln!("discarded the open Staging for {} ({count} staged)", area_name(area));
        }
    }
}

/// The Change feed's lines while the shell is interactive. They are printed straight away at the
/// prompt, and otherwise held until it is back, so they don't land in a command's output or
/// in the editor `edit` runs.
struct FeedLines {
    /// `feed on`.
    on: bool,
    /// Whether the shell is waiting at the prompt.
    at_prompt: bool,
    /// The lines that arrived while a command ran.
    held: Vec<String>,
}

impl FeedLines {
    /// Prints the lines held while a command ran, now that the prompt is back.
    fn release_held(&mut self) {
        for line in self.held.drain(..) {
            println!("{line}");
        }
        self.at_prompt = true;
    }

    /// `feed on` or `feed off`. Lines held while it was on are dropped when it goes off.
    fn switch(&mut self, on: bool) {
        self.on = on;
        if !on {
            self.held.clear();
        }
    }
}

/// Prints each item on `feed`, as [`FeedLines`] says, until the feed ends.
async fn print_feed(
    mut feed: ChangeFeed,
    output: Output,
    feed_lines: Arc<Mutex<FeedLines>>,
    mut printer: impl ExternalPrinter + Send + 'static,
) {
    while let Some(item) = feed.next().await {
        let mut lines = feed_lines.lock().unwrap();
        if !lines.on {
            continue;
        }
        for line in output.feed_lines(&item, &[]) {
            if lines.at_prompt {
                let _ = printer.print(format!("{line}\n"));
            } else {
                lines.held.push(line);
            }
        }
    }
}

/// The next item on `feed` if one is there already, without waiting.
fn next_ready(feed: &mut ChangeFeed) -> Option<FeedItem> {
    let next = pin!(feed.next());
    match next.poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(item) => item,
        Poll::Pending => None,
    }
}
