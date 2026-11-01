//! How sessions are named, as Settings reads and changes it. The ways to
//! ask a model for a session's name and their order are arko's setting,
//! kept in Arko's folder where the terminal app reads it too.

use std::path::Path;

use gpui_kit::App;

pub use arko::session_names::Way;

/// How new sessions are named now.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SessionNames {
    /// The ways turned on, in the order they are tried. None means the
    /// first words of the first message.
    pub ways: Vec<Way>,
    /// What each way needs that is missing now, if anything.
    pub missing: Vec<(Way, Option<String>)>,
}

impl SessionNames {
    /// The setting in Arko's folder `root`, and what each way lacks. It
    /// runs `claude`, so call it off the UI thread.
    pub fn read(root: &Path) -> Self {
        Self {
            ways: arko::session_names::load(root),
            missing: arko::session_names::missing(root),
        }
    }

    /// What `way` lacks now, in arko's words.
    pub fn lacks(&self, way: Way) -> Option<&str> {
        self.missing
            .iter()
            .find(|(missing, _)| *missing == way)
            .and_then(|(_, reason)| reason.as_deref())
    }
}

/// Writes `ways` through the backend, off the UI thread, after the write
/// that is running, if one is, and in place of any that waits. `then` hears
/// how the last write ended, once none is left to run.
pub fn write_names(
    ways: Vec<Way>,
    then: impl FnOnce(&Result<(), String>, &mut App) + 'static,
    cx: &mut App,
) {
    super::write_latest(
        ways,
        |backend, ways| backend.set_session_names(&ways),
        |_| {},
        then,
        cx,
    );
}
