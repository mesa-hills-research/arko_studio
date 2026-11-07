//! The changes to OpenRouter's routing settings asked for in this app. Each
//! is made to `openrouter.json` as it is when the change is written, since
//! another window or the terminal app may have changed other settings. They
//! are written one at a time and in order, whether or not Settings is still
//! open, and each write that goes through tells every window
//! ([`OpenRouterSaved`]). A read of the settings that began before a change
//! was asked for or written may hold them as they were, so it shows only if
//! [`OpenRouterRead::is_current`] says so.

use std::collections::VecDeque;

use gpui_kit::{App, AppContext as _, BorrowAppContext as _, Context, Subscription};

use super::{OpenRouterChange, OpenRouterSaved, SharedBackend};

/// The changes asked for and the write that runs.
#[derive(Default)]
struct OpenRouterChanges {
    /// The changes no write has taken yet, in the order asked.
    waiting: VecDeque<OpenRouterChange>,
    /// A change is being written or waits.
    running: bool,
    /// Why the last write failed, until a write goes through.
    error: Option<String>,
    /// Counts the changes asked for and the writes that ended, which an
    /// [`OpenRouterRead`] compares.
    changes: u64,
}

impl gpui_kit::Global for OpenRouterChanges {}

/// Makes `change` to OpenRouter's settings, after the changes asked for
/// before it.
pub fn change_openrouter(change: OpenRouterChange, cx: &mut App) {
    cx.default_global::<OpenRouterChanges>();
    let start = cx.update_global::<OpenRouterChanges, _>(|changes, _| {
        changes.waiting.push_back(change);
        changes.changes += 1;
        !std::mem::replace(&mut changes.running, true)
    });
    if start {
        write_next(cx);
    }
}

/// Whether a change asked for in this app isn't written yet.
pub fn openrouter_changing(cx: &App) -> bool {
    cx.try_global::<OpenRouterChanges>()
        .is_some_and(|changes| changes.running)
}

/// Why the last write of a change failed, until a write goes through.
pub fn openrouter_change_error(cx: &App) -> Option<String> {
    cx.try_global::<OpenRouterChanges>()?.error.clone()
}

/// A read of OpenRouter's settings begun at one moment, which tells whether
/// what it finds may show.
#[must_use]
pub struct OpenRouterRead {
    changes: u64,
}

/// Begins a read of OpenRouter's settings, as the read off the UI thread
/// starts.
pub fn begin_openrouter_read(cx: &App) -> OpenRouterRead {
    OpenRouterRead {
        changes: cx
            .try_global::<OpenRouterChanges>()
            .map_or(0, |changes| changes.changes),
    }
}

impl OpenRouterRead {
    /// Whether what the read found may show. It may if no change was asked
    /// for or written since the read began, and none waits.
    pub fn is_current(self, cx: &App) -> bool {
        cx.try_global::<OpenRouterChanges>()
            .is_none_or(|changes| changes.changes == self.changes && !changes.running)
    }
}

/// Calls `changed` whenever a write of the changes starts or ends.
pub fn observe_openrouter_changes<V: 'static>(
    cx: &mut Context<V>,
    changed: impl FnMut(&mut V, &mut Context<V>) + 'static,
) -> Subscription {
    cx.observe_global::<OpenRouterChanges>(changed)
}

fn write_next(cx: &mut App) {
    let Some(change) = cx.global_mut::<OpenRouterChanges>().waiting.pop_front() else {
        return;
    };
    let backend = cx.global::<SharedBackend>().clone();
    let writing = cx.background_spawn(async move { backend.change_openrouter(change) });
    cx.spawn(async move |cx| {
        let saved = writing.await;
        cx.update(|cx| {
            let more = cx.update_global::<OpenRouterChanges, _>(|changes, _| {
                changes.error = saved.as_ref().err().cloned();
                changes.changes += 1;
                changes.running = !changes.waiting.is_empty();
                changes.running
            });
            if let Ok(saved) = saved {
                OpenRouterSaved::publish(saved, cx);
            }
            if more {
                write_next(cx);
            }
        });
    })
    .detach();
}
