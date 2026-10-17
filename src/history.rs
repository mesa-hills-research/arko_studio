//! The messages sent before, oldest first: arko's shared `history.txt` in
//! Arko's folder, so ↑ in the composer walks what was sent here and in the
//! terminal app. Read and written on a background thread, one write after
//! another.

use std::path::PathBuf;

use gpui_kit::{App, AppContext as _, BorrowAppContext as _, Global};
use tokio::sync::mpsc;

/// The most messages kept, as the terminal app keeps.
pub const HISTORY_SIZE: usize = 1000;

/// The history as last read, plus what was sent since.
pub struct InputHistory {
    entries: Vec<String>,
    /// To the task that adds entries to the file.
    writer: Option<mpsc::UnboundedSender<String>>,
}

impl Global for InputHistory {}

impl InputHistory {
    /// The messages, oldest first.
    pub fn entries(cx: &App) -> &[String] {
        &cx.global::<Self>().entries
    }

    /// Drops the oldest beyond [`HISTORY_SIZE`].
    fn trim(&mut self) {
        let excess = self.entries.len().saturating_sub(HISTORY_SIZE);
        self.entries.drain(..excess);
    }
}

/// Reads the history of Arko's folder `root` on a background thread. With
/// `None` it lives in memory (tests).
pub fn init(root: Option<PathBuf>, cx: &mut App) {
    let path = root.as_deref().map(arko::history::path_in);
    let writer = path.clone().map(|path| {
        let (writer, mut entries) = mpsc::unbounded_channel::<String>();
        cx.background_spawn(async move {
            while let Some(entry) = entries.recv().await {
                // Read again: the terminal app may have added to it meanwhile.
                arko::history::History::load(Some(path.clone())).add(&entry);
            }
        })
        .detach();
        writer
    });
    cx.set_global(InputHistory {
        entries: Vec::new(),
        writer,
    });
    if let Some(path) = path {
        let reading = cx.background_spawn(async move {
            arko::history::History::load(Some(path)).entries().to_vec()
        });
        cx.spawn(async move |cx| {
            let read = reading.await;
            cx.update(|cx| {
                cx.update_global::<InputHistory, _>(|history, _| {
                    // What was sent while it was read comes after.
                    let sent = std::mem::take(&mut history.entries);
                    history.entries = read;
                    history.entries.extend(sent);
                    history.trim();
                })
            });
        })
        .detach();
    }
}

/// Adds `entry` (not a repeat of the last), and to the file in the
/// background, as the terminal app does.
pub fn add(entry: &str, cx: &mut App) {
    if entry.trim().is_empty()
        || InputHistory::entries(cx)
            .last()
            .is_some_and(|last| last == entry)
    {
        return;
    }
    cx.update_global::<InputHistory, _>(|history, _| {
        history.entries.push(entry.to_string());
        history.trim();
        if let Some(writer) = &history.writer {
            let _ = writer.send(entry.to_string());
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{HISTORY_SIZE, InputHistory, add, init};

    #[gpui_kit::test]
    fn the_history_keeps_the_latest_messages(cx: &mut gpui_kit::TestAppContext) {
        cx.update(|cx| init(None, cx));
        for index in 0..=HISTORY_SIZE {
            cx.update(|cx| add(&format!("Message {index}"), cx));
        }
        cx.read(|cx| {
            let entries = InputHistory::entries(cx);
            assert_eq!(entries.len(), HISTORY_SIZE);
            assert_eq!(entries[0], "Message 1");
        });
    }
}
