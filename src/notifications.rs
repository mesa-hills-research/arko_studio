//! The notification history: what Arko Studio told the user about their
//! sessions (turns that ended, questions, cache reminders), kept across
//! restarts in `<arko folder>/studio-notifications.json`. It holds the
//! latest [`KEPT`], each with the session's title and what was said, so the
//! text stays when the session is gone. Changes are written after a pause,
//! on a background thread, never on the UI thread.

use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui_kit::{App, AppContext as _, BorrowAppContext as _, Global, Task};
use serde::{Deserialize, Serialize};

use crate::backend::SessionId;

/// How many notifications the history keeps.
pub const KEPT: usize = 200;

/// How long changes rest before they are written.
pub const SAVE_DELAY: Duration = Duration::from_secs(1);

/// The history file in Arko's folder.
pub fn file(arko: &Path) -> PathBuf {
    arko.join("studio-notifications.json")
}

/// One notification in the history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Entry {
    /// Tells it from the others while the app runs.
    pub id: u64,
    /// The session it was about, when it had an id.
    pub session: Option<String>,
    /// The session's title when it was notified.
    pub title: String,
    /// What it said, a line break between lines.
    pub message: String,
    /// When, in Unix milliseconds.
    pub at: i64,
    /// The user has seen it, or opened its session.
    pub read: bool,
}

impl Default for Entry {
    fn default() -> Self {
        Self {
            id: 0,
            session: None,
            title: String::new(),
            message: String::new(),
            at: 0,
            read: true,
        }
    }
}

impl Entry {
    /// The session it was about.
    pub fn session_id(&self) -> Option<SessionId> {
        self.session.clone().map(SessionId)
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct HistoryFile {
    entries: Vec<serde_json::Value>,
}

/// Every notification kept, oldest first.
pub struct NotificationHistory {
    entries: Vec<Entry>,
    next_id: u64,
    path: Option<PathBuf>,
    changed: bool,
    saving: Option<Task<()>>,
}

impl Global for NotificationHistory {}

/// Reads the history in `path` (before the app starts). A missing file
/// gives none, and so does one that isn't JSON. Entries that can't be read
/// are left out.
pub fn load(path: &Path) -> Vec<Entry> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) => {
            if error.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!("couldn't read {} ({error})", path.display());
            }
            return Vec::new();
        }
    };
    let file = match serde_json::from_str::<HistoryFile>(&text) {
        Ok(file) => file,
        Err(error) => {
            tracing::warn!("{} is damaged ({error}), starting empty", path.display());
            return Vec::new();
        }
    };
    let mut entries: Vec<Entry> = file
        .entries
        .into_iter()
        .filter_map(|entry| serde_json::from_value(entry).ok())
        .collect();
    // Numbered afresh, so that a damaged file can't repeat a number.
    for (index, entry) in entries.iter_mut().enumerate() {
        entry.id = index as u64 + 1;
    }
    trim(&mut entries);
    entries
}

fn trim(entries: &mut Vec<Entry>) {
    let excess = entries.len().saturating_sub(KEPT);
    entries.drain(..excess);
}

/// Keeps `entries`, saved to `path` (none: in memory only).
pub fn init(mut entries: Vec<Entry>, path: Option<PathBuf>, cx: &mut App) {
    trim(&mut entries);
    let next_id = entries.iter().map(|entry| entry.id).max().unwrap_or(0) + 1;
    cx.set_global(NotificationHistory {
        entries,
        next_id,
        path,
        changed: false,
        saving: None,
    });
    cx.on_app_quit(|cx| {
        let unsaved = unsaved(cx);
        async move {
            if let Some((path, entries)) = unsaved
                && let Err(error) = write(&path, &entries)
            {
                tracing::error!("couldn't save {} ({error})", path.display());
            }
        }
    })
    .detach();
}

/// Every notification, oldest first.
pub fn entries(cx: &App) -> &[Entry] {
    &cx.global::<NotificationHistory>().entries
}

/// How many notifications the user hasn't seen.
pub fn unread(cx: &App) -> usize {
    entries(cx).iter().filter(|entry| !entry.read).count()
}

/// Adds a notification about the session `session` titled `title`, which
/// says `message`, and gives its number. `read` when the user is looking at
/// the session as it comes.
pub fn record(
    session: Option<&SessionId>,
    title: &str,
    message: &str,
    read: bool,
    cx: &mut App,
) -> u64 {
    let at = crate::cache_reminders::now(cx);
    let id = cx.update_global::<NotificationHistory, _>(|history, _| {
        let id = history.next_id;
        history.next_id += 1;
        history.entries.push(Entry {
            id,
            session: session.map(|session| session.0.clone()),
            title: title.to_string(),
            message: message.to_string(),
            at,
            read,
        });
        trim(&mut history.entries);
        history.changed = true;
        id
    });
    schedule_save(cx);
    id
}

/// Marks what `change` picks as read. Nothing happens (and nothing is
/// written) when none of it is unread.
fn mark(cx: &mut App, change: impl Fn(&Entry) -> bool) {
    if !entries(cx).iter().any(|entry| !entry.read && change(entry)) {
        return;
    }
    cx.update_global::<NotificationHistory, _>(|history, _| {
        for entry in history.entries.iter_mut().filter(|entry| change(entry)) {
            entry.read = true;
        }
        history.changed = true;
    });
    schedule_save(cx);
}

/// Marks the notification `id` read.
pub fn mark_read(id: u64, cx: &mut App) {
    mark(cx, |entry| entry.id == id);
}

/// Marks what was said of the session `session` read: the user opened it.
pub fn mark_session_read(session: &SessionId, cx: &mut App) {
    mark(cx, |entry| {
        entry.session.as_deref() == Some(session.as_str())
    });
}

/// Marks every notification read.
pub fn mark_all_read(cx: &mut App) {
    mark(cx, |_| true);
}

/// Forgets every notification.
pub fn clear(cx: &mut App) {
    if entries(cx).is_empty() {
        return;
    }
    cx.update_global::<NotificationHistory, _>(|history, _| {
        history.entries.clear();
        history.changed = true;
    });
    schedule_save(cx);
}

/// The entries to write, when they changed since the last save.
fn unsaved(cx: &App) -> Option<(PathBuf, Vec<Entry>)> {
    let history = cx.global::<NotificationHistory>();
    let path = history.path.clone()?;
    history.changed.then(|| (path, history.entries.clone()))
}

fn write(path: &Path, entries: &[Entry]) -> std::io::Result<()> {
    let entries = entries
        .iter()
        .filter_map(|entry| serde_json::to_value(entry).ok())
        .collect();
    let text = serde_json::to_string_pretty(&HistoryFile { entries })?;
    crate::prefs::write_atomically(path, &text)
}

fn schedule_save(cx: &mut App) {
    if cx.global::<NotificationHistory>().path.is_none() {
        return;
    }
    // Replacing the task drops the one waiting: only a pause saves.
    let task = cx.spawn(async move |cx| {
        cx.background_executor().timer(SAVE_DELAY).await;
        let Some((path, entries)) = cx.update(|cx| {
            let unsaved = unsaved(cx);
            cx.global_mut::<NotificationHistory>().changed = false;
            unsaved
        }) else {
            return;
        };
        // Written as JSON off the UI thread.
        let written = cx
            .background_spawn({
                let path = path.clone();
                async move { write(&path, &entries) }
            })
            .await;
        if let Err(error) = written {
            tracing::error!("couldn't save {} ({error})", path.display());
            cx.update(|cx| cx.global_mut::<NotificationHistory>().changed = true);
        }
    });
    cx.global_mut::<NotificationHistory>().saving = Some(task);
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::TestAppContext;

    fn entry(id: u64, session: &str, read: bool) -> Entry {
        Entry {
            id,
            session: Some(session.into()),
            title: format!("Session {session}"),
            message: "Arko finished its turn.".into(),
            at: 1_000 * id as i64,
            read,
        }
    }

    #[gpui_kit::test]
    fn what_is_recorded_is_unread_until_the_user_looks(cx: &mut TestAppContext) {
        cx.update(|cx| init(Vec::new(), None, cx));
        let (first, second) = cx.update(|cx| {
            let one = SessionId("s1".into());
            let two = SessionId("s2".into());
            let first = record(Some(&one), "Fonts", "Arko finished its turn.", false, cx);
            let second = record(Some(&two), "Naming", "Arko asks “Tabs?”", false, cx);
            // In sight as it came.
            record(
                Some(&one),
                "Fonts",
                "The prompt cache expires in a minute",
                true,
                cx,
            );
            (first, second)
        });
        assert_ne!(first, second);
        cx.read(|cx| {
            assert_eq!(entries(cx).len(), 3);
            assert_eq!(unread(cx), 2);
            assert_eq!(entries(cx)[0].title, "Fonts");
            assert_eq!(entries(cx)[0].session.as_deref(), Some("s1"));
        });
        cx.update(|cx| mark_read(first, cx));
        assert_eq!(cx.read(unread), 1);
        // Opening a session reads what was said of it.
        cx.update(|cx| mark_session_read(&SessionId("s2".into()), cx));
        assert_eq!(cx.read(unread), 0);
    }

    #[gpui_kit::test]
    fn mark_all_read_and_clear_empty_the_unread_count_and_the_list(cx: &mut TestAppContext) {
        let entries_in = (1..=3).map(|id| entry(id, "s1", false)).collect();
        cx.update(|cx| init(entries_in, None, cx));
        assert_eq!(cx.read(unread), 3);
        cx.update(mark_all_read);
        cx.read(|cx| {
            assert_eq!(unread(cx), 0);
            assert_eq!(entries(cx).len(), 3);
        });
        cx.update(clear);
        assert!(cx.read(|cx| entries(cx).is_empty()));
        // A new one gets a number no earlier one had.
        let id = cx.update(|cx| record(None, "New session", "Arko asks you something.", false, cx));
        assert_eq!(id, 4);
    }

    #[gpui_kit::test]
    fn the_history_keeps_the_latest_200(cx: &mut TestAppContext) {
        cx.update(|cx| init(Vec::new(), None, cx));
        for index in 0..=KEPT {
            cx.update(|cx| {
                record(None, "Fonts", &format!("Message {index}"), true, cx);
            });
        }
        cx.read(|cx| {
            assert_eq!(entries(cx).len(), KEPT);
            assert_eq!(entries(cx)[0].message, "Message 1");
            assert_eq!(entries(cx)[KEPT - 1].message, format!("Message {KEPT}"));
        });
    }

    /// Notifications are saved once after a pause, off the UI thread, and
    /// come back as they were when the app starts again.
    #[gpui_kit::test]
    async fn the_history_is_saved_after_a_pause_and_read_back(cx: &mut TestAppContext) {
        let folder = tempfile::tempdir().unwrap();
        let path = file(folder.path());
        cx.update(|cx| init(Vec::new(), Some(path.clone()), cx));
        let id = cx.update(|cx| {
            let session = SessionId("s1".into());
            record(
                Some(&session),
                "Fonts",
                "Arko finished its turn.",
                false,
                cx,
            );
            record(
                Some(&session),
                "Fonts",
                "Arko asks “Tabs or spaces?”",
                false,
                cx,
            )
        });
        cx.run_until_parked();
        assert!(!path.exists(), "written before the pause");
        cx.executor().advance_clock(SAVE_DELAY);
        cx.run_until_parked();
        let saved = load(&path);
        assert_eq!(saved.len(), 2);
        assert_eq!(saved[1].message, "Arko asks “Tabs or spaces?”");
        assert_eq!(saved[1].session_id(), Some(SessionId("s1".into())));
        assert!(!saved[1].read);
        // What was read stays read.
        cx.update(|cx| mark_read(id, cx));
        cx.executor().advance_clock(SAVE_DELAY);
        cx.run_until_parked();
        let saved = load(&path);
        assert_eq!(
            saved.iter().map(|entry| entry.read).collect::<Vec<_>>(),
            [false, true]
        );
        // A second app run starts from the file.
        cx.update(|cx| init(saved, Some(path.clone()), cx));
        assert_eq!(cx.read(unread), 1);
        let next = cx.update(|cx| record(None, "Fonts", "Another", false, cx));
        assert_eq!(next, 3);
    }

    /// A missing file, a damaged one and one with bad entries give what can
    /// be read, never a failure.
    #[test]
    fn a_missing_or_damaged_file_gives_what_can_be_read() {
        let folder = tempfile::tempdir().unwrap();
        let path = file(folder.path());
        assert!(load(&path).is_empty());
        std::fs::write(&path, "{ not json").unwrap();
        assert!(load(&path).is_empty());
        std::fs::write(&path, "[1, 2]").unwrap();
        assert!(load(&path).is_empty());
        std::fs::write(&path, "").unwrap();
        assert!(load(&path).is_empty());
        // Entries that read are kept, the rest are dropped, a missing field
        // takes its default, and the numbers are made afresh.
        std::fs::write(
            &path,
            r#"{"entries":[
                {"id":7,"session":"s1","title":"Fonts","message":"Done","at":5,"read":false},
                "not an entry",
                {"id":7,"title":"Naming","message":"Asks","at":6},
                {"id":9,"at":"yesterday"}
            ]}"#,
        )
        .unwrap();
        let loaded = load(&path);
        assert_eq!(loaded.len(), 2);
        assert_eq!(
            loaded.iter().map(|entry| entry.id).collect::<Vec<_>>(),
            [1, 2]
        );
        assert!(!loaded[0].read);
        assert_eq!(loaded[1].session, None);
        assert!(loaded[1].read);
        // Never more than the latest 200.
        let many: Vec<Entry> = (1..=KEPT as u64 + 20)
            .map(|id| entry(id, "s1", true))
            .collect();
        write(&path, &many).unwrap();
        let loaded = load(&path);
        assert_eq!(loaded.len(), KEPT);
        assert_eq!(loaded[0].at, 21_000);
    }
}
