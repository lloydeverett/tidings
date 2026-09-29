//! What commands print: for a person to read, or as JSON with `--json`.

use std::io::{self, Write};

use serde_json::{Value, json};
use tidings::{
    Area, Change, ChangeKind, Committed, FeedItem, File, Origin, Path, Prefix, PrefixRevision, Stat,
};

use crate::working_copy::{CommitReport, LocalChange, SyncEvent};

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
            Report::WorkingCopyCommit(report) => print_stdout(
                &report
                    .changes
                    .iter()
                    .map(|change| format!("{} {}\n", change_name(change.change), change.path))
                    .collect::<String>(),
            ),
        }
    }

    /// Prints what `sync` did, as one line.
    pub fn print_sync_event(self, event: &SyncEvent) -> io::Result<()> {
        let (name, path) = match event {
            SyncEvent::Created(path) => ("created", Some(path)),
            SyncEvent::Updated(path) => ("updated", Some(path)),
            SyncEvent::Removed(path) => ("removed", Some(path)),
            SyncEvent::CaughtUp => ("caught-up", None),
        };
        let line = match (self.json, path) {
            (true, Some(path)) => json!({"event": name, "path": path.as_str()}).to_string(),
            (true, None) => json!({"event": name}).to_string(),
            (false, Some(path)) => format!("{name} {path}"),
            // For a person, a name is words.
            (false, None) => name.replace('-', " "),
        };
        print_stdout(&format!("{line}\n"))
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
        }),
    })
}

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
