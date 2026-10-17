//! The feed's updates on their way to a transcript: drained off the UI
//! thread as they come, held until the transcript's next frame, and applied
//! then, all at once.

use std::sync::{Arc, Mutex};

use arko::remote::{Activity, Feed, Snapshot, Update};
use gpui_kit::{AppContext as _, Context, Task};
use tokio::sync::Notify;

/// What came since the transcript last drew.
#[derive(Default)]
pub(crate) struct Pending {
    pub updates: Vec<Update>,
    /// The activity as of the latest `Update::Activity` (which carries none).
    pub activity: Option<Activity>,
}

impl Pending {
    fn is_empty(&self) -> bool {
        self.updates.is_empty() && self.activity.is_none()
    }

    /// Adds `update`, joining text appended to the update before it.
    fn push(&mut self, update: Update) {
        if let Update::Append { id, text } = &update
            && let Some(last) = self.updates.last_mut()
        {
            match last {
                Update::Append {
                    id: last_id,
                    text: last_text,
                } if last_id == id => {
                    last_text.push_str(text);
                    return;
                }
                Update::Item(item) | Update::Change(item) if item.id == *id => {
                    item.text.push_str(text);
                    return;
                }
                _ => {}
            }
        }
        self.updates.push(update);
    }
}

/// A transcript's subscription to a feed.
pub(crate) struct Inbox {
    pending: Arc<Mutex<Pending>>,
    _drain: Task<()>,
    _wake: Task<()>,
}

impl Inbox {
    /// Subscribes to `feed`: returns the history so far, and an inbox that
    /// asks `cx`'s entity to draw again (once) when updates come.
    pub fn subscribe<T: 'static>(feed: &Feed, cx: &mut Context<T>) -> (Snapshot, Self) {
        let (snapshot, mut batches) = feed.subscribe_batched();
        let pending = Arc::new(Mutex::new(Pending::default()));
        let notify = Arc::new(Notify::new());
        let drain = cx.background_spawn({
            let pending = pending.clone();
            let notify = notify.clone();
            let feed = feed.clone();
            async move {
                while let Some(batch) = batches.next().await {
                    let activity = batch
                        .iter()
                        .any(|update| matches!(update, Update::Activity))
                        .then(|| feed.activity());
                    let wake = {
                        let mut pending = pending.lock().unwrap_or_else(|p| p.into_inner());
                        let was_empty = pending.is_empty();
                        for update in batch {
                            if !matches!(update, Update::Activity) {
                                pending.push(update);
                            }
                        }
                        if activity.is_some() {
                            pending.activity = activity;
                        }
                        was_empty && !pending.is_empty()
                    };
                    // One wake while updates wait: the frame that takes them
                    // takes all that came meanwhile.
                    if wake {
                        notify.notify_one();
                    }
                }
            }
        });
        let wake = cx.spawn(async move |this, cx| {
            loop {
                notify.notified().await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        });
        (
            snapshot,
            Self {
                pending,
                _drain: drain,
                _wake: wake,
            },
        )
    }

    /// Takes what came since the last call.
    pub fn take(&self) -> Pending {
        std::mem::take(&mut *self.pending.lock().unwrap_or_else(|p| p.into_inner()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arko::remote::{Item, Kind};

    #[test]
    fn text_appended_while_waiting_joins_the_update_before() {
        let mut pending = Pending::default();
        let mut item = Item::new(Kind::Text, "a");
        item.id = 1;
        pending.push(Update::Item(item));
        pending.push(Update::Append {
            id: 1,
            text: "b".into(),
        });
        pending.push(Update::Append {
            id: 2,
            text: "c".into(),
        });
        pending.push(Update::Append {
            id: 2,
            text: "d".into(),
        });
        assert_eq!(pending.updates.len(), 2);
        assert!(matches!(&pending.updates[0], Update::Item(item) if item.text == "ab"));
        assert!(matches!(&pending.updates[1], Update::Append { id: 2, text } if text == "cd"));
    }
}
