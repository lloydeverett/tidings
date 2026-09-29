//! What commands print: for a person to read, or as JSON with `--json`.

use std::io::{self, Write};
use std::path::Path as FsPath;

use serde_json::{Value, json};
use tidings::{
    Area, Change, ChangeKind, Committed, FeedItem, File, Origin, Path, Prefix, PrefixRevision, Stat,
};

use crate::working_copy::{
    Blocked, CommitReport, DiscardReport, Discarded, DiscardedChange, DivergedPath, LocalChange,
    LocalName, PathStatus, ResolveReport, Resolved, StatusReport, SyncEvent, Unfit,
};

/// What a command gives, for [`Output`] to print.
pub enum Report {
    /// Nothing to print.
    Nothing,
    /// A File that was read.
    File(File),
    /// What `stat` gave for the File at the Path.
    Stat(Path, Stat),
    /// The Paths `list` gave.
    Paths(Vec<Path>),
    /// What `stat-prefix` gave for the Prefix in the Area.
    PrefixRevision(Area, Prefix, PrefixRevision),
    /// A successful Commit.
    Committed(Committed),
    /// Something was added to the open Staging, which now holds `count` things.
    Staged { area: Area, count: usize },
    /// `edit` changed nothing, so nothing was committed.
    Unchanged,
    /// The editor `edit` ran quit with a failure, so the edit was dropped.
    Cancelled,
    /// What `commit` committed from a Working copy.
    WorkingCopyCommit(CommitReport),
    /// What `status` found in a Working copy.
    WorkingCopyStatus(StatusReport),
    /// What `discard` threw away in a Working copy.
    WorkingCopyDiscard(DiscardReport),
    /// What `resolve` settled in a Working copy.
    WorkingCopyResolve(ResolveReport),
}

/// How to print.
#[derive(Debug, Clone, Copy)]
pub struct Output {
    /// `--json`.
    pub json: bool,
    /// Whether a prompt follows what is printed, in the interactive shell.
    pub at_prompt: bool,
}

impl Output {
    /// Prints `report` on stdout, or a note about it on stderr.
    pub fn print(self, report: &Report) -> io::Result<()> {
        if self.json {
            if let Some(value) = as_json(report) {
                return print_stdout(&format!("{value}\n"));
            }
            return Ok(());
        }
        match report {
            Report::Nothing | Report::Staged { .. } => Ok(()),
            // Exactly as stored, apart from at the prompt, where a last line without a newline
            // would be overwritten by the prompt.
            Report::File(file) => {
                let contents = file.contents();
                if self.at_prompt && !contents.is_empty() && !contents.ends_with('\n') {
                    print_stdout(&format!("{contents}\n"))
                } else {
                    print_stdout(contents)
                }
            }
            Report::Stat(_, stat) => print_stdout(&format!(
                "revision {}\nmodified {}\n",
                stat.revision(),
                stat.modified()
            )),
            Report::Paths(paths) => {
                print_stdout(&paths.iter().map(|path| format!("{path}\n")).collect::<String>())
            }
            Report::PrefixRevision(_, _, revision) => print_stdout(&format!("{revision}\n")),
            Report::Committed(committed) => {
                let lines = committed.revisions().iter();
                print_stdout(
                    &lines
                        .map(|(path, revision)| format!("{revision}  {path}\n"))
                        .collect::<String>(),
                )
            }
            Report::Unchanged => {
                eprintln!("unchanged: nothing was committed");
                Ok(())
            }
            Report::Cancelled => {
                eprintln!("cancelled: the editor quit with a failure, so the edit was dropped");
                Ok(())
            }
            Report::WorkingCopyCommit(report) if report.changes.is_empty() => {
                eprintln!("nothing to commit");
                Ok(())
            }
            // Each change committed, then anything reconciling after a `Pending` Commit found, as
            // `sync` prints it.
            Report::WorkingCopyCommit(report) => {
                print_stdout(
                    &report
                        .changes
                        .iter()
                        .map(|change| format!("{} {}\n", change_name(change.change), change.path))
                        .chain(report.events.iter().map(|event| format!("{}\n", event_line(event))))
                        .collect::<String>(),
                )?;
                if report.pending {
                    eprintln!("{PENDING_NOTE}");
                }
                Ok(())
            }
            // Each Path, then whether `sync` is running.
            Report::WorkingCopyStatus(report) => {
                let mut lines: Vec<String> =
                    report.paths.iter().map(|path| status_parts(path).line()).collect();
                if lines.is_empty() {
                    lines.push("nothing to commit".to_owned());
                }
                let sync = if report.syncing { "sync is running" } else { "sync isn't running" };
                lines.push(sync.to_owned());
                print_stdout(&lines.iter().map(|line| format!("{line}\n")).collect::<String>())
            }
            Report::WorkingCopyDiscard(report) if report.discarded.is_empty() => {
                eprintln!("nothing to discard");
                Ok(())
            }
            // Each Path, then each `theirs` file that couldn't be removed, as `sync` prints it.
            Report::WorkingCopyDiscard(report) => print_stdout(
                &report
                    .discarded
                    .iter()
                    .map(|discarded| discarded_parts(discarded).line())
                    .chain(report.events.iter().map(event_line))
                    .map(|line| format!("{line}\n"))
                    .collect::<String>(),
            ),
            Report::WorkingCopyResolve(report) => print_stdout(
                &report
                    .resolved
                    .iter()
                    .map(|resolved| resolved_parts(resolved).line())
                    .chain(report.events.iter().map(event_line))
                    .map(|line| format!("{line}\n"))
                    .collect::<String>(),
            ),
        }
    }

    /// The lines that show `item` for the Areas in `areas`, or every Area if it is empty.
    pub fn feed_lines(self, item: &FeedItem, areas: &[Area]) -> Vec<String> {
        let shown = |area: Area| areas.is_empty() || areas.contains(&area);
        match item {
            FeedItem::Changes(changes) => changes
                .iter()
                .filter(|change| shown(change.area))
                .map(|change| self.change_line(change))
                .collect(),
            FeedItem::Resync(area) if shown(*area) => vec![if self.json {
                json!({"resync": area_name(*area)}).to_string()
            } else {
                format!("resync {}", area_name(*area))
            }],
            _ => Vec::new(),
        }
    }

    fn change_line(self, change: &Change) -> String {
        let origin = match change.origin {
            Origin::Local => "local",
            Origin::External => "external",
        };
        let kind = match change.kind {
            ChangeKind::Changed => "changed",
            ChangeKind::Removed => "removed",
        };
        let area = area_name(change.area);
        if self.json {
            json!({"origin": origin, "kind": kind, "area": area, "path": change.path.as_str()})
                .to_string()
        } else {
            format!("{origin} {kind} {area} {}", change.path)
        }
    }
}

/// How `sync` prints what it does.
#[derive(Debug, Clone, Copy)]
pub struct SyncOutput {
    /// How to print each line, as JSON or for a person.
    pub output: Output,
    /// `--quiet`: print only what needs the person's attention.
    pub quiet: bool,
}

impl SyncOutput {
    /// Prints what `sync` did, as one line, unless it is quiet and this needs no attention.
    pub fn print(self, event: &SyncEvent) -> io::Result<()> {
        let needs_attention =
            matches!(event, SyncEvent::Resync | SyncEvent::Diverged(_) | SyncEvent::Error { .. });
        if self.quiet && !needs_attention {
            return Ok(());
        }
        let line = if self.output.json { event_json(event).to_string() } else { event_line(event) };
        print_stdout(&format!("{line}\n"))
    }
}

/// The JSON object that says what `sync` did, or found, as in `{"event": "created", "path": "a"}`.
pub fn event_json(event: &SyncEvent) -> Value {
    EventParts::of(event).json()
}

/// The line for a person that says what `sync` did, or found, as in "diverged a: removed in the
/// Store".
pub fn event_line(event: &SyncEvent) -> String {
    EventParts::of(event).line()
}

/// What an event says, to print as a line for a person or as JSON.
pub struct EventParts<'a> {
    /// Its name, as in `diverged`.
    pub name: &'static str,
    /// What it is about, if anything.
    pub subject: Option<Subject<'a>>,
    /// What else it says, if anything.
    pub message: Option<String>,
    /// For a *diverged* event only: the file with the Store's version, or `None` if the Store
    /// removed it, so that another program following the Working copy can find it.
    pub theirs: Option<Option<&'a FsPath>>,
}

impl<'a> EventParts<'a> {
    /// What `event`, from `sync`, says.
    pub fn of(event: &'a SyncEvent) -> EventParts<'a> {
        let parts = |name, path: Option<&'a Path>, message| EventParts {
            name,
            subject: path.map(Subject::Path),
            message,
            theirs: None,
        };
        match event {
            SyncEvent::Created(path) => parts("created", Some(path), None),
            SyncEvent::Updated(path) => parts("updated", Some(path), None),
            SyncEvent::Removed(path) => parts("removed", Some(path), None),
            SyncEvent::Diverged(diverged) => EventParts::diverged(diverged),
            SyncEvent::Resolved(path) => parts("resolved", Some(path), None),
            SyncEvent::Error { path, message } => parts("error", Some(path), Some(message.clone())),
            SyncEvent::Resync => parts("resync", None, None),
            SyncEvent::CaughtUp => parts("caught-up", None, None),
        }
    }

    /// What a *diverged* event says about `diverged`, a Diverged Path.
    pub fn diverged(diverged: &'a DivergedPath) -> EventParts<'a> {
        let DivergedPath { path, theirs_file, blocked } = diverged;
        EventParts {
            name: "diverged",
            subject: Some(Subject::Path(path)),
            message: Some(diverged_message(theirs_file.as_deref(), blocked.as_ref())),
            theirs: Some(theirs_file.as_deref()),
        }
    }

    /// What an *invalid* event says about `unfit`, a file that can't become a File.
    pub fn invalid(unfit: &'a Unfit) -> EventParts<'a> {
        EventParts {
            name: "invalid",
            subject: Some(Subject::Local(&unfit.name)),
            message: Some(unfit.reason.clone()),
            theirs: None,
        }
    }

    /// As a JSON object, as in `{"event": "created", "path": "a"}`.
    pub fn json(&self) -> Value {
        let mut object = serde_json::Map::new();
        object.insert("event".to_owned(), json!(self.name));
        if let Some(subject) = self.subject {
            object.insert("path".to_owned(), json!(subject.as_str()));
        }
        if let Some(message) = &self.message {
            object.insert("message".to_owned(), json!(message));
        }
        if let Some(theirs) = self.theirs {
            object.insert("theirs".to_owned(), json!(theirs.map(FsPath::to_string_lossy)));
        }
        Value::Object(object)
    }

    /// As a line for a person, as in "diverged a: removed in the Store".
    pub fn line(&self) -> String {
        // For a person, a name is words.
        let mut line = self.name.replace('-', " ");
        if let Some(subject) = self.subject {
            line.push_str(&format!(" {}", subject.as_str()));
        }
        if let Some(message) = &self.message {
            line.push_str(&format!(": {message}"));
        }
        line
    }
}

/// What an event is about.
#[derive(Clone, Copy)]
pub enum Subject<'a> {
    /// A Path.
    Path(&'a Path),
    /// For an *invalid* event only: a file in the folder, which may not be a valid Path, given
    /// where a Path would be.
    Local(&'a LocalName),
}

impl Subject<'_> {
    /// The Path, or the file's name in the folder, as text.
    fn as_str(&self) -> &str {
        match self {
            Subject::Path(path) => path.as_str(),
            Subject::Local(name) => name.as_str(),
        }
    }
}

/// What a *diverged* line says: where the Store's version is, or that the Store removed it, after
/// what in the folder kept it from being applied, if anything.
fn diverged_message(theirs_file: Option<&FsPath>, blocked: Option<&Blocked>) -> String {
    let store_side = match theirs_file {
        Some(theirs_file) => format!("the Store's version is in {}", theirs_file.display()),
        None => "removed in the Store".to_owned(),
    };
    match blocked {
        Some(blocked) => format!("{}; {store_side}", blocked.reason()),
        None => store_side,
    }
}

/// What `status` says about one Path, or one file in the folder, as `sync` gives an event: named
/// `modified`, `added`, `deleted`, `diverged` or `invalid`.
fn status_parts(status: &PathStatus) -> EventParts<'_> {
    match status {
        PathStatus::Changed(path, change) => EventParts {
            name: change_name(*change),
            subject: Some(Subject::Path(path)),
            message: None,
            theirs: None,
        },
        PathStatus::Diverged(diverged) => EventParts::diverged(diverged),
        PathStatus::Invalid(unfit) => EventParts::invalid(unfit),
    }
}

/// What `discard` says about one Path, as in "discarded a: took the Store's version".
fn discarded_parts(discarded: &Discarded) -> EventParts<'_> {
    let message = match (discarded.revision, discarded.removed) {
        (Some(_), _) => "took the Store's version",
        (None, true) => "removed, since the Store has no File there",
        (None, false) => "the Store has no File there either",
    };
    EventParts {
        name: "discarded",
        subject: Some(Subject::Path(&discarded.path)),
        message: Some(message.to_owned()),
        theirs: None,
    }
}

/// What `resolve` says about one Path, as in "resolved a: its Base is now …".
fn resolved_parts(resolved: &Resolved) -> EventParts<'_> {
    let message = match resolved.revision {
        Some(_) => "its Base is now the Store's version it was merged with",
        None => "it has no Base, since the Store had removed it",
    };
    EventParts {
        name: "resolved",
        subject: Some(Subject::Path(&resolved.path)),
        message: Some(message.to_owned()),
        theirs: None,
    }
}

/// What `discard` threw away, by the name `status` gives it.
fn discarded_change_name(change: DiscardedChange) -> &'static str {
    match change {
        DiscardedChange::Local(change) => change_name(change),
        DiscardedChange::Invalid => "invalid",
        DiscardedChange::Diverged => "diverged",
    }
}

/// `report` as JSON, or `None` if it has nothing to show.
fn as_json(report: &Report) -> Option<Value> {
    Some(match report {
        Report::Nothing => return None,
        Report::File(file) => json!({
            "path": file.path().as_str(),
            "contents": file.contents(),
            "revision": file.revision().to_string(),
            "modified": file.modified().to_string(),
        }),
        Report::Stat(path, stat) => json!({
            "path": path.as_str(),
            "revision": stat.revision().to_string(),
            "modified": stat.modified().to_string(),
        }),
        Report::Paths(paths) => json!(paths.iter().map(Path::as_str).collect::<Vec<_>>()),
        Report::PrefixRevision(area, prefix, revision) => json!({
            "area": area_name(*area),
            "prefix": prefix.as_str(),
            "revision": revision.to_string(),
        }),
        Report::Committed(committed) => json!({
            "timestamp": committed.timestamp().to_string(),
            "revisions": committed
                .revisions()
                .iter()
                .map(|(path, revision)| (path.as_str().to_owned(), json!(revision.to_string())))
                .collect::<serde_json::Map<_, _>>(),
        }),
        Report::Staged { area, count } => {
            json!({"staged": {"area": area_name(*area), "count": count}})
        }
        Report::Unchanged => json!({"unchanged": true}),
        Report::Cancelled => json!({"cancelled": true}),
        Report::WorkingCopyCommit(report) => json!({
            "committed": report
                .changes
                .iter()
                .map(|change| json!({
                    "path": change.path.as_str(),
                    "change": change_name(change.change),
                    "revision": change.revision.map(|revision| revision.to_string()),
                }))
                .collect::<Vec<_>>(),
            "pending": report.pending,
            "events": report.events.iter().map(event_json).collect::<Vec<_>>(),
        }),
        Report::WorkingCopyStatus(report) => json!({
            "paths": report.paths.iter().map(|path| status_parts(path).json()).collect::<Vec<_>>(),
            "syncing": report.syncing,
        }),
        Report::WorkingCopyDiscard(report) => json!({
            "discarded": report
                .discarded
                .iter()
                .map(|discarded| json!({
                    "path": discarded.path.as_str(),
                    "change": discarded_change_name(discarded.change),
                    "revision": discarded.revision.map(|revision| revision.to_string()),
                }))
                .collect::<Vec<_>>(),
            "events": report.events.iter().map(event_json).collect::<Vec<_>>(),
        }),
        Report::WorkingCopyResolve(report) => json!({
            "resolved": report
                .resolved
                .iter()
                .map(|resolved| json!({
                    "path": resolved.path.as_str(),
                    "revision": resolved.revision.map(|revision| revision.to_string()),
                }))
                .collect::<Vec<_>>(),
            "events": report.events.iter().map(event_json).collect::<Vec<_>>(),
        }),
    })
}

/// What `commit` says after a Commit that gave `Pending`.
const PENDING_NOTE: &str = "the Commit happened, but isn't finished yet: a File in the Store \
                            couldn't be replaced, and the next Commit to the Area, or opening the \
                            Store, finishes it";

/// A local change's name, as `commit` prints it.
fn change_name(change: LocalChange) -> &'static str {
    match change {
        LocalChange::Added => "added",
        LocalChange::Modified => "modified",
        LocalChange::Deleted => "deleted",
    }
}

/// Writes `text` to stdout straight away, so that a line reaches a pipe as soon as it's printed.
pub fn print_stdout(text: &str) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    stdout.write_all(text.as_bytes())?;
    stdout.flush()
}

/// An Area's name on the command line.
pub fn area_name(area: Area) -> &'static str {
    match area {
        Area::Config => "config",
        Area::Data => "data",
        Area::Cache => "cache",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::working_copy::{CommitReport, CommittedChange};

    /// A Commit that gave `Pending` is printed with the events reconciling after it gave, such as
    /// a Path another Commit made Diverged in between, which no test through the binary can time.
    #[test]
    fn a_pending_commits_events_are_printed_with_it() {
        let path = Path::new("app.toml").unwrap();
        let report = CommitReport {
            changes: vec![CommittedChange {
                path: path.clone(),
                change: LocalChange::Modified,
                revision: None,
            }],
            pending: true,
            events: vec![SyncEvent::Diverged(DivergedPath {
                path,
                theirs_file: Some(".tidings/theirs/app.toml".into()),
                blocked: None,
            })],
        };
        assert_eq!(
            as_json(&Report::WorkingCopyCommit(report)),
            Some(json!({
                "committed": [{"path": "app.toml", "change": "modified", "revision": null}],
                "pending": true,
                "events": [{
                    "event": "diverged",
                    "path": "app.toml",
                    "message": "the Store's version is in .tidings/theirs/app.toml",
                    "theirs": ".tidings/theirs/app.toml",
                }],
            }))
        );
    }
}
