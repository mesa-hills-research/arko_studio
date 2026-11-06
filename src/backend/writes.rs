//! The writes of the settings kept in Arko's folder, kept apart from
//! Settings so that the last change is written whether or not Settings is
//! still open. Each kind of setting has its own queue. One write of it runs
//! at a time, and changes that come meanwhile leave only the latest to be
//! written next, so what is saved is what was chosen last.
//!
//! A setting's saves are the app's, and anything that shows the setting
//! observes them ([`observe_saves`], [`save_state`]): the choice being saved,
//! why the last save failed, and how many went through. Each save that goes
//! through runs its setting's follow-up, also when a later one waits.

use gpui_kit::{App, AppContext as _, BorrowAppContext as _, Context, Subscription};

use super::SharedBackend;

/// How a setting is written through the backend, off the UI thread.
pub type Write<T> = fn(&SharedBackend, T) -> Result<(), String>;

/// What follows a save that went through, such as telling the sessions.
pub type Saved = fn(&mut App);

/// The saves of the setting `T`.
struct Writes<T> {
    /// The latest value asked for that no write has taken yet, how it's
    /// written and what follows its save.
    pending: Option<(T, Write<T>, Saved)>,
    /// The latest value asked for, until the writes end.
    latest: Option<T>,
    running: bool,
    /// Why the last save failed, until a choice or a save after it.
    error: Option<String>,
    /// How many saves went through.
    saved: u64,
}

impl<T> Default for Writes<T> {
    fn default() -> Self {
        Self {
            pending: None,
            latest: None,
            running: false,
            error: None,
            saved: 0,
        }
    }
}

impl<T: 'static> gpui_kit::Global for Writes<T> {}

/// What the app knows of the saves of the setting `T`.
#[derive(Debug, Clone, PartialEq)]
pub struct SaveState<T> {
    /// The latest choice, while its save hasn't ended.
    pub pending: Option<T>,
    /// Why the last save failed, until a choice or a save after it.
    pub error: Option<String>,
    /// How many saves went through.
    pub saved: u64,
}

/// The saves of the setting `T` now.
pub fn save_state<T: Clone + 'static>(cx: &App) -> SaveState<T> {
    match cx.try_global::<Writes<T>>() {
        Some(writes) => SaveState {
            pending: writes.latest.clone(),
            error: writes.error.clone(),
            saved: writes.saved,
        },
        None => SaveState {
            pending: None,
            error: None,
            saved: 0,
        },
    }
}

/// Calls `changed` whenever the saves of the setting `T` change.
pub fn observe_saves<T: 'static, V: 'static>(
    cx: &mut Context<V>,
    changed: impl FnMut(&mut V, &mut Context<V>) + 'static,
) -> Subscription {
    cx.observe_global::<Writes<T>>(changed)
}

/// Writes `value` with `write`, after the write of the same setting that is
/// running, if one is, and in place of any that waits. `saved` follows if
/// its save goes through. How the saves go is the setting's
/// [`save_state`].
pub fn write_latest<T: Clone + Send + 'static>(
    value: T,
    write: Write<T>,
    saved: Saved,
    cx: &mut App,
) {
    cx.default_global::<Writes<T>>();
    let start = cx.update_global::<Writes<T>, _>(|writes, _| {
        writes.latest = Some(value.clone());
        writes.error = None;
        writes.pending = Some((value, write, saved));
        !std::mem::replace(&mut writes.running, true)
    });
    if start {
        write_next::<T>(cx);
    }
}

fn write_next<T: Clone + Send + 'static>(cx: &mut App) {
    let Some((value, write, saved)) = cx.global_mut::<Writes<T>>().pending.take() else {
        return;
    };
    let backend = cx.global::<SharedBackend>().clone();
    let writing = cx.background_spawn(async move { write(&backend, value) });
    cx.spawn(async move |cx| {
        let result = writing.await;
        cx.update(|cx| {
            cx.update_global::<Writes<T>, _>(|writes, _| match &result {
                Ok(()) => {
                    writes.saved += 1;
                    writes.error = None;
                }
                Err(why) => writes.error = Some(why.clone()),
            });
            if result.is_ok() {
                saved(cx);
            }
            // Asked after the follow-up, which may ask for another save.
            let more = cx.update_global::<Writes<T>, _>(|writes, _| {
                let more = writes.pending.is_some();
                if !more {
                    writes.running = false;
                    writes.latest = None;
                }
                more
            });
            if more {
                write_next::<T>(cx);
            }
        });
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::SystemTime;

    use gpui_kit::TestAppContext;

    use super::*;
    use crate::backend::FakeBackend;

    fn backend(cx: &mut App) {
        let backend = FakeBackend::sample("/arko", SystemTime::now());
        cx.set_global(SharedBackend(Arc::new(backend)));
    }

    /// A setting whose first save's follow-up asks for another save.
    #[derive(Clone)]
    struct FollowedUp(u32);

    static FOLLOWED_UP: Mutex<Vec<u32>> = Mutex::new(Vec::new());

    fn write_followed_up(_: &SharedBackend, value: FollowedUp) -> Result<(), String> {
        FOLLOWED_UP.lock().unwrap().push(value.0);
        Ok(())
    }

    fn ask_once_more(cx: &mut App) {
        if FOLLOWED_UP.lock().unwrap().len() == 1 {
            write_latest(FollowedUp(2), write_followed_up, ask_once_more, cx);
        }
    }

    #[gpui_kit::test]
    fn a_save_a_follow_up_asks_for_is_written(cx: &mut TestAppContext) {
        cx.update(|cx| {
            backend(cx);
            write_latest(FollowedUp(1), write_followed_up, ask_once_more, cx);
        });
        cx.run_until_parked();
        assert_eq!(*FOLLOWED_UP.lock().unwrap(), [1, 2]);
        assert!(cx.read(save_state::<FollowedUp>).pending.is_none());
    }
}
