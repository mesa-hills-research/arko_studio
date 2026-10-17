//! What the user typed in each session's composer and hasn't sent: kept
//! when they switch sessions and across restarts, in
//! `<arko folder>/studio-drafts.json`. Typing changes them in memory, and
//! the file is written after a pause, on a background thread, never per key.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui_kit::{App, AppContext as _, BorrowAppContext as _, Global, Task};
use serde::{Deserialize, Serialize};

/// How long typing pauses before the drafts are written.
pub const SAVE_DELAY: Duration = Duration::from_secs(1);

/// The draft of a new session, before it has an id.
pub const NEW_SESSION: &str = "new";

/// The drafts file in Arko's folder.
pub fn file(arko: &Path) -> PathBuf {
    arko.join("studio-drafts.json")
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct DraftsFile {
    /// By session id, or [`NEW_SESSION`].
    drafts: BTreeMap<String, String>,
}

/// Every unsent draft.
pub struct Drafts {
    drafts: BTreeMap<String, String>,
    path: Option<PathBuf>,
    changed: bool,
    saving: Option<Task<()>>,
}

impl Global for Drafts {}

/// Reads the drafts in `path` (before the app starts), or none.
pub fn load(path: &Path) -> BTreeMap<String, String> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<DraftsFile>(&text).ok())
        .map(|file| file.drafts)
        .unwrap_or_default()
}

/// Keeps `drafts`, saved to `path` (none: in memory only).
pub fn init(drafts: BTreeMap<String, String>, path: Option<PathBuf>, cx: &mut App) {
    cx.set_global(Drafts {
        drafts,
        path,
        changed: false,
        saving: None,
    });
    cx.on_app_quit(|cx| {
        let unsaved = unsaved(cx);
        async move {
            if let Some((path, text)) = unsaved
                && let Err(error) = crate::prefs::write_atomically(&path, &text)
            {
                tracing::error!("couldn't save {}: {error}", path.display());
            }
        }
    })
    .detach();
}

/// The draft of `key` (a session id or [`NEW_SESSION`]).
pub fn get(key: &str, cx: &App) -> Option<String> {
    cx.global::<Drafts>().drafts.get(key).cloned()
}

/// Keeps `text` as the draft of `key`. Empty text forgets it.
pub fn set(key: &str, text: &str, cx: &mut App) {
    let drafts = cx.global::<Drafts>();
    let same = match drafts.drafts.get(key) {
        Some(draft) => draft == text,
        None => text.trim().is_empty(),
    };
    if same {
        return;
    }
    cx.update_global::<Drafts, _>(|drafts, _| {
        if text.trim().is_empty() {
            drafts.drafts.remove(key);
        } else {
            drafts.drafts.insert(key.to_string(), text.to_string());
        }
        drafts.changed = true;
    });
    schedule_save(cx);
}

fn unsaved(cx: &App) -> Option<(PathBuf, String)> {
    let drafts = cx.global::<Drafts>();
    let path = drafts.path.clone()?;
    if !drafts.changed {
        return None;
    }
    let file = DraftsFile {
        drafts: drafts.drafts.clone(),
    };
    serde_json::to_string_pretty(&file)
        .ok()
        .map(|text| (path, text))
}

fn schedule_save(cx: &mut App) {
    if cx.global::<Drafts>().path.is_none() {
        return;
    }
    // Replacing the task drops the one waiting: only a pause saves.
    let task = cx.spawn(async move |cx| {
        cx.background_executor().timer(SAVE_DELAY).await;
        let Some((path, text)) = cx.update(|cx| {
            let unsaved = unsaved(cx);
            cx.global_mut::<Drafts>().changed = false;
            unsaved
        }) else {
            return;
        };
        let written = cx
            .background_spawn({
                let path = path.clone();
                async move { crate::prefs::write_atomically(&path, &text) }
            })
            .await;
        if let Err(error) = written {
            tracing::error!("couldn't save {}: {error}", path.display());
            cx.update(|cx| cx.global_mut::<Drafts>().changed = true);
        }
    });
    cx.global_mut::<Drafts>().saving = Some(task);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui_kit::test]
    async fn drafts_are_saved_once_after_a_pause(cx: &mut gpui_kit::TestAppContext) {
        let folder = tempfile::tempdir().unwrap();
        let path = file(folder.path());
        cx.update(|cx| init(BTreeMap::new(), Some(path.clone()), cx));
        cx.update(|cx| set("s1", "Count the", cx));
        cx.update(|cx| set("s1", "Count the pears", cx));
        cx.run_until_parked();
        assert!(!path.exists(), "written before the pause");
        cx.executor().advance_clock(SAVE_DELAY);
        cx.run_until_parked();
        assert_eq!(
            load(&path).get("s1").map(String::as_str),
            Some("Count the pears")
        );
        // An emptied draft is forgotten.
        cx.update(|cx| set("s1", "  ", cx));
        cx.executor().advance_clock(SAVE_DELAY);
        cx.run_until_parked();
        assert!(load(&path).is_empty());
        assert_eq!(cx.read(|cx| get("s1", cx)), None);
    }
}
