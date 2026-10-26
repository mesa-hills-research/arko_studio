//! How sessions are named, as Settings reads and changes it. The ways to
//! ask a model for a session's name and their order are arko's setting,
//! kept in Arko's folder where the terminal app reads it too.

use std::path::Path;

use gpui_kit::{App, AppContext as _};

use super::SharedBackend;

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

/// The writes of the setting, kept apart from Settings so that the last
/// change is written whether or not Settings is still open. One write runs
/// at a time, and changes that come meanwhile leave only the latest to be
/// written next, so what is saved is what was chosen last.
/// Hears how the last write ended.
type Heard = Box<dyn FnOnce(&Result<(), String>, &mut App)>;

#[derive(Default)]
struct NameWrites {
    /// The latest list asked for that no write has taken yet.
    pending: Option<Vec<Way>>,
    running: bool,
    /// Who wants to hear how the writes ended, when the last one has.
    then: Vec<Heard>,
}

impl gpui_kit::Global for NameWrites {}

/// Writes `ways` through the backend, off the UI thread, after the write
/// that is running, if one is, and in place of any that waits. `then` hears
/// how the last write ended, once none is left to run.
pub fn write_names(
    ways: Vec<Way>,
    then: impl FnOnce(&Result<(), String>, &mut App) + 'static,
    cx: &mut App,
) {
    let writes = cx.default_global::<NameWrites>();
    writes.pending = Some(ways);
    writes.then.push(Box::new(then));
    if !writes.running {
        writes.running = true;
        write_next(cx);
    }
}

fn write_next(cx: &mut App) {
    let Some(ways) = cx.global_mut::<NameWrites>().pending.take() else {
        return;
    };
    let backend = cx.global::<SharedBackend>().clone();
    let writing = cx.background_spawn(async move { backend.set_session_names(&ways) });
    cx.spawn(async move |cx| {
        let result = writing.await;
        cx.update(|cx| {
            if cx.global::<NameWrites>().pending.is_some() {
                write_next(cx);
                return;
            }
            let writes = cx.global_mut::<NameWrites>();
            writes.running = false;
            for then in std::mem::take(&mut writes.then) {
                then(&result, cx);
            }
        });
    })
    .detach();
}
