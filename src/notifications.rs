//! The notification history: what Arko Studio told the user about their
//! sessions (turns that ended, questions, cache reminders), kept across
//! restarts in `<arko folder>/studio-notifications.json`. It holds the
//! latest [`KEPT`], each with the session's title and what was said, so the
//! text stays when the session is gone. Changes are written after a pause,
//! on a background thread, never on the UI thread.
//!
//! The file also holds which sessions have news, the ones the sidebar marks
//! with a dot: a turn ended or a question came while the user was looking at
//! another session. It is read the way the unread notifications are (opening
//! the session clears both), and it doesn't depend on which notifications
//! Settings has on.

use std::collections::BTreeMap;
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
    /// When each session with news got it, by session id.
    news: serde_json::Value,
}

/// What the history file held.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Saved {
    pub entries: Vec<Entry>,
    /// The sessions with news and when (Unix milliseconds) it came.
    pub news: BTreeMap<String, i64>,
}

/// Every notification kept, oldest first, and the sessions with news.
pub struct NotificationHistory {
    entries: Vec<Entry>,
    news: BTreeMap<String, i64>,
    next_id: u64,
    path: Option<PathBuf>,
    changed: bool,
    saving: Option<Task<()>>,
}

impl Global for NotificationHistory {}

/// Reads the history in `path` (before the app starts). A missing file
/// gives none, and so does one that isn't JSON. Entries and sessions that
/// can't be read are left out.
pub fn load(path: &Path) -> Saved {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) => {
            if error.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!("couldn't read {} ({error})", path.display());
            }
            return Saved::default();
        }
    };
    let file = match serde_json::from_str::<HistoryFile>(&text) {
        Ok(file) => file,
        Err(error) => {
            tracing::warn!("{} is damaged ({error}), starting empty", path.display());
            return Saved::default();
        }
    };
    let mut news: BTreeMap<String, i64> = file
        .news
        .as_object()
        .map(|news| {
            news.iter()
                .filter_map(|(session, at)| Some((session.clone(), at.as_i64()?)))
                .collect()
        })
        .unwrap_or_default();
    trim_news(&mut news);
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
    Saved { entries, news }
}

fn trim(entries: &mut Vec<Entry>) {
    let excess = entries.len().saturating_sub(KEPT);
    entries.drain(..excess);
}

/// Keeps the sessions whose news is newest, [`KEPT`] at the most.
fn trim_news(news: &mut BTreeMap<String, i64>) {
    if news.len() > KEPT {
        let mut by_time: Vec<_> = news.iter().map(|(id, at)| (*at, id.clone())).collect();
        by_time.sort();
        for (_, id) in by_time.into_iter().take(news.len() - KEPT) {
            news.remove(&id);
        }
    }
}

/// Keeps what was `saved`, written to `path` (none: in memory only).
pub fn init(mut saved: Saved, path: Option<PathBuf>, cx: &mut App) {
    trim(&mut saved.entries);
    trim_news(&mut saved.news);
    let next_id = saved
        .entries
        .iter()
        .map(|entry| entry.id)
        .max()
        .unwrap_or(0)
        + 1;
    cx.set_global(NotificationHistory {
        entries: saved.entries,
        news: saved.news,
        next_id,
        path,
        changed: false,
        saving: None,
    });
    cx.on_app_quit(|cx| {
        let unsaved = unsaved(cx);
        async move {
            if let Some((path, saved)) = unsaved
                && let Err(error) = write(&path, &saved)
            {
                tracing::error!("couldn't save {} ({error})", path.display());
            }
        }
    })
    .detach();
}

/// Every notification, oldest first.
pub fn entries(cx: &App) -> &[Entry] {
    cx.try_global::<NotificationHistory>()
        .map_or(&[], |history| &history.entries)
}

/// Whether the session `session` has news the user hasn't seen: a turn
/// ended or a question came while they were looking at another session.
pub fn has_news(session: &SessionId, cx: &App) -> bool {
    cx.try_global::<NotificationHistory>()
        .is_some_and(|history| history.news.contains_key(session.as_str()))
}

/// Says the session `session` has news since the user last looked at it.
pub fn mark_news(session: &SessionId, cx: &mut App) {
    let at = crate::cache_reminders::now(cx);
    cx.update_global::<NotificationHistory, _>(|history, _| {
        history.news.insert(session.0.clone(), at);
        trim_news(&mut history.news);
        history.changed = true;
    });
    schedule_save(cx);
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

/// Marks the notification `id` read. (It doesn't take its session's news
/// away, opening the session does.)
pub fn mark_read(id: u64, cx: &mut App) {
    let Some(history) = cx.try_global::<NotificationHistory>() else {
        return;
    };
    if !history
        .entries
        .iter()
        .any(|entry| entry.id == id && !entry.read)
    {
        return;
    }
    cx.update_global::<NotificationHistory, _>(|history, _| {
        for entry in history.entries.iter_mut().filter(|entry| entry.id == id) {
            entry.read = true;
        }
        history.changed = true;
    });
    schedule_save(cx);
}

/// Marks what was said of the session `session` read and takes its news
/// away: the user opened it. Nothing is written when there was none.
pub fn mark_session_read(session: &SessionId, cx: &mut App) {
    let Some(history) = cx.try_global::<NotificationHistory>() else {
        return;
    };
    let said = |entry: &Entry| entry.session.as_deref() == Some(session.as_str());
    if !history.news.contains_key(session.as_str())
        && !history
            .entries
            .iter()
            .any(|entry| !entry.read && said(entry))
    {
        return;
    }
    cx.update_global::<NotificationHistory, _>(|history, _| {
        for entry in history.entries.iter_mut().filter(|entry| said(entry)) {
            entry.read = true;
        }
        history.news.remove(session.as_str());
        history.changed = true;
    });
    schedule_save(cx);
}

/// Marks every notification read, and takes every session's news away.
pub fn mark_all_read(cx: &mut App) {
    let Some(history) = cx.try_global::<NotificationHistory>() else {
        return;
    };
    if history.news.is_empty() && history.entries.iter().all(|entry| entry.read) {
        return;
    }
    cx.update_global::<NotificationHistory, _>(|history, _| {
        for entry in &mut history.entries {
            entry.read = true;
        }
        history.news.clear();
        history.changed = true;
    });
    schedule_save(cx);
}

/// Forgets the notifications and news of one session.
pub fn clear_session(session: &SessionId, cx: &mut App) {
    let Some(history) = cx.try_global::<NotificationHistory>() else {
        return;
    };
    if !history.news.contains_key(session.as_str())
        && !history
            .entries
            .iter()
            .any(|entry| entry.session.as_deref() == Some(session.as_str()))
    {
        return;
    }
    cx.update_global::<NotificationHistory, _>(|history, _| {
        history
            .entries
            .retain(|entry| entry.session.as_deref() != Some(session.as_str()));
        history.news.remove(session.as_str());
        history.changed = true;
    });
    schedule_save(cx);
}

/// Forgets every notification, and every session's news.
pub fn clear(cx: &mut App) {
    let Some(history) = cx.try_global::<NotificationHistory>() else {
        return;
    };
    if history.entries.is_empty() && history.news.is_empty() {
        return;
    }
    cx.update_global::<NotificationHistory, _>(|history, _| {
        history.entries.clear();
        history.news.clear();
        history.changed = true;
    });
    schedule_save(cx);
}

/// What to write, when it changed since the last save.
fn unsaved(cx: &App) -> Option<(PathBuf, Saved)> {
    let history = cx.global::<NotificationHistory>();
    let path = history.path.clone()?;
    history.changed.then(|| {
        let saved = Saved {
            entries: history.entries.clone(),
            news: history.news.clone(),
        };
        (path, saved)
    })
}

fn write(path: &Path, saved: &Saved) -> std::io::Result<()> {
    let entries = saved
        .entries
        .iter()
        .filter_map(|entry| serde_json::to_value(entry).ok())
        .collect();
    let news = serde_json::to_value(&saved.news)?;
    let text = serde_json::to_string_pretty(&HistoryFile { entries, news })?;
    crate::prefs::write_atomically(path, &text)
}

fn schedule_save(cx: &mut App) {
    if cx.global::<NotificationHistory>().path.is_none() {
        return;
    }
    // Replacing the task drops the one waiting: only a pause saves.
    let task = cx.spawn(async move |cx| {
        cx.background_executor().timer(SAVE_DELAY).await;
        let Some((path, saved)) = cx.update(|cx| {
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
                async move { write(&path, &saved) }
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
        cx.update(|cx| init(Saved::default(), None, cx));
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
        cx.update(|cx| {
            init(
                Saved {
                    entries: entries_in,
                    ..Saved::default()
                },
                None,
                cx,
            )
        });
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
    fn clearing_one_session_keeps_other_notifications_and_news(cx: &mut TestAppContext) {
        let folder = tempfile::tempdir().unwrap();
        let path = file(folder.path());
        let (one, two) = (SessionId("s1".into()), SessionId("s2".into()));
        cx.update(|cx| {
            init(Saved::default(), Some(path.clone()), cx);
            record(Some(&one), "One", "Finished", false, cx);
            record(Some(&two), "Two", "Finished", false, cx);
            record(None, "New session", "Finished", false, cx);
            mark_news(&one, cx);
            mark_news(&two, cx);
            clear_session(&one, cx);
        });
        cx.run_until_parked();
        cx.executor().advance_clock(SAVE_DELAY);
        cx.run_until_parked();
        let saved = load(&path);
        assert_eq!(saved.entries.len(), 2);
        assert_eq!(saved.entries[0].session.as_deref(), Some("s2"));
        assert_eq!(saved.entries[1].session, None);
        assert_eq!(saved.news.keys().collect::<Vec<_>>(), ["s2"]);
        assert_eq!(cx.read(unread), 2);
    }

    #[gpui_kit::test]
    fn the_history_keeps_the_latest_200(cx: &mut TestAppContext) {
        cx.update(|cx| init(Saved::default(), None, cx));
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
    /// come back as they were when the app starts again, with the sessions
    /// that had news.
    #[gpui_kit::test]
    async fn the_history_is_saved_after_a_pause_and_read_back(cx: &mut TestAppContext) {
        let folder = tempfile::tempdir().unwrap();
        let path = file(folder.path());
        cx.update(|cx| init(Saved::default(), Some(path.clone()), cx));
        let id = cx.update(|cx| {
            let session = SessionId("s1".into());
            record(
                Some(&session),
                "Fonts",
                "Arko finished its turn.",
                false,
                cx,
            );
            mark_news(&session, cx);
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
        assert_eq!(saved.entries.len(), 2);
        assert_eq!(saved.entries[1].message, "Arko asks “Tabs or spaces?”");
        assert_eq!(saved.entries[1].session_id(), Some(SessionId("s1".into())));
        assert!(!saved.entries[1].read);
        assert_eq!(saved.news.keys().collect::<Vec<_>>(), ["s1"]);
        // What was read stays read.
        cx.update(|cx| mark_read(id, cx));
        cx.executor().advance_clock(SAVE_DELAY);
        cx.run_until_parked();
        let saved = load(&path);
        assert_eq!(
            saved
                .entries
                .iter()
                .map(|entry| entry.read)
                .collect::<Vec<_>>(),
            [false, true]
        );
        // A second app run starts from the file.
        cx.update(|cx| init(saved, Some(path.clone()), cx));
        assert_eq!(cx.read(unread), 1);
        assert!(cx.read(|cx| has_news(&SessionId("s1".into()), cx)));
        let next = cx.update(|cx| record(None, "Fonts", "Another", false, cx));
        assert_eq!(next, 3);
    }

    /// A session has news from when something happened in it out of sight
    /// until the user opens it, which reads what was said of it too. Reading
    /// all, or clearing, takes every session's news away.
    #[gpui_kit::test]
    fn news_marks_a_session_until_it_is_opened(cx: &mut TestAppContext) {
        cx.update(|cx| init(Saved::default(), None, cx));
        let (one, two) = (SessionId("s1".into()), SessionId("s2".into()));
        let news =
            |cx: &mut TestAppContext, session: &SessionId| cx.read(|cx| has_news(session, cx));
        assert!(!news(cx, &one));
        cx.update(|cx| {
            mark_news(&one, cx);
            mark_news(&two, cx);
            record(Some(&one), "Fonts", "Arko finished its turn.", false, cx);
        });
        assert!(news(cx, &one) && news(cx, &two));
        // A notification read by itself leaves its session's news.
        cx.update(|cx| mark_read(1, cx));
        assert!(news(cx, &one));
        cx.update(|cx| mark_session_read(&one, cx));
        assert!(!news(cx, &one) && news(cx, &two));
        assert_eq!(cx.read(unread), 0);
        cx.update(mark_all_read);
        assert!(!news(cx, &two));
        cx.update(|cx| mark_news(&two, cx));
        cx.update(clear);
        assert!(!news(cx, &two));
    }

    /// A missing file, a damaged one and one with bad entries give what can
    /// be read, never a failure.
    #[test]
    fn a_missing_or_damaged_file_gives_what_can_be_read() {
        let folder = tempfile::tempdir().unwrap();
        let path = file(folder.path());
        assert_eq!(load(&path), Saved::default());
        for damaged in ["{ not json", "[1, 2]", ""] {
            std::fs::write(&path, damaged).unwrap();
            assert_eq!(load(&path), Saved::default(), "{damaged}");
        }
        // Entries that read are kept, the rest are dropped, a missing field
        // takes its default, and the numbers are made afresh.
        std::fs::write(
            &path,
            r#"{"entries":[
                {"id":7,"session":"s1","title":"Fonts","message":"Done","at":5,"read":false},
                "not an entry",
                {"id":7,"title":"Naming","message":"Asks","at":6},
                {"id":9,"at":"yesterday"}
            ],
            "news":{"s1":5,"s2":"yesterday","s3":null}}"#,
        )
        .unwrap();
        let loaded = load(&path);
        assert_eq!(loaded.entries.len(), 2);
        assert_eq!(
            loaded
                .entries
                .iter()
                .map(|entry| entry.id)
                .collect::<Vec<_>>(),
            [1, 2]
        );
        assert!(!loaded.entries[0].read);
        assert_eq!(loaded.entries[1].session, None);
        assert!(loaded.entries[1].read);
        // Sessions whose news can't be read are left out.
        assert_eq!(
            loaded.news.into_iter().collect::<Vec<_>>(),
            [("s1".into(), 5)]
        );
        // News that isn't a map is none.
        std::fs::write(&path, r#"{"news":[1,2]}"#).unwrap();
        assert!(load(&path).news.is_empty());
        // Never more than the latest 200.
        let many: Vec<Entry> = (1..=KEPT as u64 + 20)
            .map(|id| entry(id, "s1", true))
            .collect();
        let news = (0..KEPT as i64 + 5)
            .map(|at| (format!("s{at}"), at))
            .collect();
        write(
            &path,
            &Saved {
                entries: many,
                news,
            },
        )
        .unwrap();
        let loaded = load(&path);
        assert_eq!(loaded.entries.len(), KEPT);
        assert_eq!(loaded.entries[0].at, 21_000);
        assert_eq!(loaded.news.len(), KEPT);
        assert!(!loaded.news.contains_key("s0"), "the oldest news goes");
    }
}
