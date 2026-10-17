//! The feed's history and updates on their way to a transcript: taken off
//! the UI thread as they come, held until the transcript's next frame, and
//! applied then, all at once (or a moment later, by a transcript that
//! isn't drawn).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use arko::remote::{Activity, Feed, Snapshot, Update};
use gpui_kit::{AppContext as _, Context, Task};
use tokio::sync::Notify;

use super::TranscriptView;

/// What came since the transcript last drew.
#[derive(Default)]
pub(crate) struct Pending {
    /// The history so far, once, as the subscription starts: the updates
    /// that follow it come after it.
    pub snapshot: Option<Snapshot>,
    pub updates: Vec<Update>,
    /// The activity as of the latest `Update::Activity` (which carries none).
    pub activity: Option<Activity>,
}

impl Pending {
    fn is_empty(&self) -> bool {
        self.snapshot.is_none() && self.updates.is_empty() && self.activity.is_none()
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

/// How long updates wait for a frame before a transcript that isn't drawn
/// (one kept for a session not on screen) takes them anyway.
pub(super) const CATCH_UP: Duration = Duration::from_millis(500);

/// A transcript's subscription to a feed.
pub(crate) struct Inbox {
    pending: Arc<Mutex<Pending>>,
    _drain: Task<()>,
    _wake: Task<()>,
    _catch_up: Task<()>,
}

impl Inbox {
    /// Subscribes to `feed`: an inbox that gets the history so far and then
    /// the updates, and asks the transcript to draw again (once) when they
    /// come. Subscribing copies the history under the feed's lock, so it
    /// happens on a background thread too.
    ///
    /// A transcript that doesn't draw within [`CATCH_UP`] takes the
    /// updates then: the model's work is quick, and the frame that shows
    /// it again has no long backlog to go through.
    pub fn subscribe(feed: &Feed, cx: &mut Context<TranscriptView>) -> Self {
        let pending = Arc::new(Mutex::new(Pending::default()));
        // Woken when updates come to an empty inbox: one to draw, one to
        // catch up if no frame comes.
        let notify = Arc::new(Notify::new());
        let catch_up = Arc::new(Notify::new());
        let drain = cx.background_spawn({
            let pending = pending.clone();
            let notify = notify.clone();
            let catch_up = catch_up.clone();
            let feed = feed.clone();
            async move {
                let (snapshot, mut batches) = feed.subscribe_batched();
                pending.lock().unwrap_or_else(|p| p.into_inner()).snapshot = Some(snapshot);
                notify.notify_one();
                catch_up.notify_one();
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
                        catch_up.notify_one();
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
        let catch_up = cx.spawn(async move |this, cx| {
            loop {
                catch_up.notified().await;
                let Ok(drawn) = this.read_with(cx, |this, _| this.renders) else {
                    break;
                };
                cx.background_executor().timer(CATCH_UP).await;
                let caught_up = this.update(cx, |this, cx| {
                    if this.renders == drawn {
                        this.apply_pending(cx);
                    }
                });
                if caught_up.is_err() {
                    break;
                }
            }
        });
        Self {
            pending,
            _drain: drain,
            _wake: wake,
            _catch_up: catch_up,
        }
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
