//! The writes of the settings kept in Arko's folder, kept apart from
//! Settings so that the last change is written whether or not Settings is
//! still open. Each kind of setting has its own queue. One write of it runs
//! at a time, and changes that come meanwhile leave only the latest to be
//! written next, so what is saved is what was chosen last.

use gpui_kit::{App, AppContext as _};

use super::SharedBackend;

/// Hears how the last write ended.
type Heard = Box<dyn FnOnce(&Result<(), String>, &mut App)>;

/// How a setting is written through the backend, off the UI thread.
pub type Write<T> = fn(&SharedBackend, T) -> Result<(), String>;

/// The queue of the setting `T`.
struct Writes<T> {
    /// The latest value asked for that no write has taken yet, and how it's
    /// written.
    pending: Option<(T, Write<T>)>,
    /// The latest value asked for, until the writes end.
    latest: Option<T>,
    running: bool,
    /// Who wants to hear how the writes ended, when the last one has.
    then: Vec<Heard>,
}

impl<T> Default for Writes<T> {
    fn default() -> Self {
        Self {
            pending: None,
            latest: None,
            running: false,
            then: Vec::new(),
        }
    }
}

impl<T: 'static> gpui_kit::Global for Writes<T> {}

/// Writes `value` with `write`, after the write of the same setting that is
/// running, if one is, and in place of any that waits. `then` hears how the
/// last write ended, once none is left to run.
pub fn write_latest<T: Clone + Send + 'static>(
    value: T,
    write: Write<T>,
    then: impl FnOnce(&Result<(), String>, &mut App) + 'static,
    cx: &mut App,
) {
    let writes = cx.default_global::<Writes<T>>();
    writes.latest = Some(value.clone());
    writes.pending = Some((value, write));
    writes.then.push(Box::new(then));
    if !writes.running {
        writes.running = true;
        write_next::<T>(cx);
    }
}

/// The latest value of the setting `T` asked for, while its writes haven't
/// ended, which is what the setting is about to hold.
pub fn writing<T: Clone + 'static>(cx: &App) -> Option<T> {
    cx.try_global::<Writes<T>>()?.latest.clone()
}

fn write_next<T: Clone + Send + 'static>(cx: &mut App) {
    let Some((value, write)) = cx.global_mut::<Writes<T>>().pending.take() else {
        return;
    };
    let backend = cx.global::<SharedBackend>().clone();
    let writing = cx.background_spawn(async move { write(&backend, value) });
    cx.spawn(async move |cx| {
        let result = writing.await;
        cx.update(|cx| {
            if cx.global::<Writes<T>>().pending.is_some() {
                write_next::<T>(cx);
                return;
            }
            let writes = cx.global_mut::<Writes<T>>();
            writes.running = false;
            writes.latest = None;
            for then in std::mem::take(&mut writes.then) {
                then(&result, cx);
            }
        });
    })
    .detach();
}
