//! The App identity, and where it puts a Store's Areas on this platform. It belongs to no one
//! Backend: every Backend that keeps its Areas on disk uses it.

use std::path::PathBuf;

use etcetera::app_strategy::{AppStrategy, AppStrategyArgs, choose_app_strategy};

use crate::area::PerArea;
use crate::{Area, Error, Result};

/// Who an app is: its name, its author and its top-level domain. Together they decide where a
/// Store's Areas are on this platform: in the platform's standard config, data and cache
/// directories for the app, as the `etcetera` crate's `choose_app_strategy` finds them. Those are
/// the XDG directories on Linux and macOS (such as `~/.config/frobnicator`), and folders under
/// `AppData` on Windows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppIdentity {
    app_name: String,
    author: String,
    top_level_domain: String,
}

impl AppIdentity {
    /// The App identity of the app called `app_name`, by `author`, under `top_level_domain`, as in
    /// `AppIdentity::new("Frobnicator", "Acme Corp", "org")`.
    pub fn new(
        app_name: impl Into<String>,
        author: impl Into<String>,
        top_level_domain: impl Into<String>,
    ) -> AppIdentity {
        AppIdentity {
            app_name: app_name.into(),
            author: author.into(),
            top_level_domain: top_level_domain.into(),
        }
    }

    /// The directory each Area is in: the platform's standard config, data and cache directories
    /// for the app or, under a Root override, `config`, `data` and `cache` in it.
    pub(crate) fn area_directories(
        &self,
        root_override: Option<&std::path::Path>,
    ) -> Result<PerArea<PathBuf>> {
        if let Some(root) = root_override {
            return PerArea::try_from_fn(|area| Ok(root.join(area.name())));
        }
        let strategy = choose_app_strategy(AppStrategyArgs {
            top_level_domain: self.top_level_domain.clone(),
            author: self.author.clone(),
            app_name: self.app_name.clone(),
        })
        .map_err(Error::backend)?;
        PerArea::try_from_fn(|area| {
            Ok(match area {
                Area::Config => strategy.config_dir(),
                Area::Data => strategy.data_dir(),
                Area::Cache => strategy.cache_dir(),
            })
        })
    }
}
