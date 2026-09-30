//! `edit`: a File edited in the person's editor, and written back only if nobody changed it
//! meanwhile.

use std::io::Write;
use std::process::Command;

use tidings::Precondition;

use super::{Session, keep_edit};
use crate::failure::Failure;
use crate::output::Report;

impl Session {
    /// Edits the File at `path`, or a new one if there is none, then writes it back,
    /// requiring it to be unchanged since it was read, or still absent. In an open Staging, the
    /// write is staged instead. If the text wasn't changed, or the editor quit with a failure,
    /// nothing is.
    ///
    /// If the write fails, now or when the Staging is committed, the edited text is kept in the
    /// temporary file it was edited in, which the failure names.
    pub(super) async fn edit(&mut self, path: &str) -> Result<Report, Failure> {
        let editor = editor()?;
        let file = self.store.read(path).await?;
        let original = file.as_ref().map_or("", |file| file.contents());

        let mut buffer =
            tempfile::Builder::new().prefix("tidings-edit-").suffix(&extension(path)).tempfile()?;
        buffer.write_all(original.as_bytes())?;
        buffer.flush()?;

        let (program, arguments) = editor.split_first().expect("checked not empty");
        let status =
            Command::new(program).args(arguments).arg(buffer.path()).status().map_err(|error| {
                Failure::error(format!("can't run the editor {program:?}: {error}"))
            })?;
        if !status.success() {
            return Ok(Report::Cancelled);
        }
        let edited = std::fs::read_to_string(buffer.path())
            .map_err(|error| Failure::error(format!("can't read back the edited text: {error}")))?;
        if edited == original {
            return Ok(Report::Unchanged);
        }

        let buffer = buffer.into_temp_path();
        let written = self
            .stage_or_commit(|staging| match &file {
                Some(file) => {
                    staging.write_back(file, edited);
                    Ok(())
                }
                None => staging.write_requiring(path, edited, Precondition::Absent).map(drop),
            })
            .await;
        match written {
            Ok(report @ Report::Staged { .. }) => {
                let open = self.staging.as_mut().expect("the edit was staged in it");
                open.edits.push(buffer);
                Ok(report)
            }
            Ok(report) => Ok(report),
            Err(failure) => Err(keep_edit(failure, buffer)),
        }
    }
}

/// The editor command: `$VISUAL`, or else `$EDITOR`, split as a shell would.
fn editor() -> Result<Vec<String>, Failure> {
    let unset = || Failure::error("set $VISUAL or $EDITOR to the editor to use");
    let variable = ["VISUAL", "EDITOR"]
        .into_iter()
        .find_map(|name| std::env::var(name).ok().filter(|value| !value.trim().is_empty()))
        .ok_or_else(unset)?;
    let words = shell_words::split(&variable)
        .map_err(|error| Failure::error(format!("can't split the editor command: {error}")))?;
    if words.is_empty() {
        return Err(unset());
    }
    Ok(words)
}

/// The extension of `path`'s last name, with its `.`, so the editor can tell the File's type.
fn extension(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => format!(".{extension}"),
        _ => String::new(),
    }
}
