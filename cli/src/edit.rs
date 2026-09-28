//! `edit`: a File edited in the person's editor, and written back only if nobody changed it
//! meanwhile.

use std::io::Write;
use std::process::Command;

use tidings::{Area, Precondition};

use crate::command::Session;
use crate::failure::Failure;
use crate::output::{Report, area_name};

/// Edits the File at `path` in `area`, or a new one if there is none, then writes it back,
/// requiring it to be unchanged since it was read, or still absent. In an open Staging, the
/// write is staged instead. If the text wasn't changed, nothing is.
///
/// If the write fails, the edited text is kept in the temporary file it was edited in, which the
/// failure names.
pub async fn edit(session: &mut Session, area: Area, path: &str) -> Result<Report, Failure> {
    let editor = editor()?;
    let file = session.store().read(area, path).await?;
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
        return Err(Failure::error(format!("the editor {program:?} failed ({status})")));
    }
    let edited = std::fs::read_to_string(buffer.path())
        .map_err(|error| Failure::error(format!("can't read back the edited text: {error}")))?;
    if edited == original {
        return Ok(Report::Unchanged);
    }

    let written = session
        .stage_or_commit(area, |staging| match &file {
            Some(file) => {
                staging.write_back(file, edited);
                Ok(())
            }
            None => staging.write_requiring(path, edited, Precondition::Absent).map(drop),
        })
        .await;
    written.map_err(|failure| match buffer.keep() {
        Ok((_, kept)) => failure.and(format!(
            "your edit of {} {path} is kept in {}",
            area_name(area),
            kept.display()
        )),
        Err(error) => failure.and(format!("and your edit couldn't be kept: {error}")),
    })
}

/// The editor command: `$VISUAL`, or else `$EDITOR`, split as a shell would.
fn editor() -> Result<Vec<String>, Failure> {
    let variable = ["VISUAL", "EDITOR"]
        .into_iter()
        .find_map(|name| std::env::var(name).ok().filter(|value| !value.trim().is_empty()))
        .ok_or_else(|| Failure::error("set $VISUAL or $EDITOR to the editor to use"))?;
    let words = shell_words::split(&variable)
        .map_err(|error| Failure::error(format!("can't split the editor command: {error}")))?;
    if words.is_empty() {
        return Err(Failure::error("set $VISUAL or $EDITOR to the editor to use"));
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
