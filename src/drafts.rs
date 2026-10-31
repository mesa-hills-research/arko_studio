//! What the user put in each session's composer and hasn't sent: kept
//! when they switch sessions, and the text across restarts too, in
//! `<arko folder>/studio-drafts.json`. Typing changes them in memory, and
//! the file is written after a pause, on a background thread, never per key.
//! A draft's images are kept in memory only, beside its text, so they're
//! gone when Studio quits.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui_kit::{App, AppContext as _, BorrowAppContext as _, Global, Task};
use serde::{Deserialize, Serialize};

use crate::composer::{Attached, Outgoing};

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

/// Puts what a composer holds into the drafts.
type Keeper = Box<dyn Fn(&mut App)>;

/// Every unsent draft.
pub struct Drafts {
    /// The text of each, which is saved.
    drafts: BTreeMap<String, String>,
    /// The images of each, kept in memory only.
    images: BTreeMap<String, Vec<Attached>>,
    path: Option<PathBuf>,
    changed: bool,
    saving: Option<Task<()>>,
    /// Put what composers hold into the drafts before the last save at
    /// quit. One whose composer is gone does nothing.
    keepers: Vec<Keeper>,
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
        images: BTreeMap::new(),
        path,
        changed: false,
        saving: None,
        keepers: Vec::new(),
    });
    cx.on_app_quit(|cx| {
        // The last keystrokes, which a composer keeps only once typing rests.
        let keepers = std::mem::take(&mut cx.global_mut::<Drafts>().keepers);
        for keep in &keepers {
            keep(cx);
        }
        let unsaved = unsaved(cx);
        async move {
            if let Some((path, drafts)) = unsaved
                && let Err(error) = write(&path, drafts)
            {
                tracing::error!("couldn't save {} ({error})", path.display());
            }
        }
    })
    .detach();
}

/// Has `keep` put what a composer holds into the drafts before the app
/// quits.
pub fn keep_at_quit(keep: impl Fn(&mut App) + 'static, cx: &mut App) {
    cx.global_mut::<Drafts>().keepers.push(Box::new(keep));
}

/// The draft of `key` (a session id or [`NEW_SESSION`]), as the composer
/// restores it.
pub fn get(key: &str, cx: &App) -> Option<Outgoing> {
    let drafts = cx.global::<Drafts>();
    let text = drafts.drafts.get(key);
    let images = drafts.images.get(key);
    if text.is_none() && images.is_none() {
        return None;
    }
    Some(Outgoing {
        text: text.cloned().unwrap_or_default(),
        images: images.cloned().unwrap_or_default(),
    })
}

/// Keeps `draft` as the draft of `key`: its text saved, its images in
/// memory. A draft with neither words nor images forgets it.
pub fn set(key: &str, draft: &Outgoing, cx: &mut App) {
    let text = &draft.text;
    let drafts = cx.global::<Drafts>();
    let same = match drafts.drafts.get(key) {
        Some(kept) => kept == text,
        None => text.trim().is_empty(),
    };
    let images = (!draft.images.is_empty()).then(|| draft.images.clone());
    cx.update_global::<Drafts, _>(|drafts, _| match images {
        Some(images) => {
            drafts.images.insert(key.to_string(), images);
        }
        None => {
            drafts.images.remove(key);
        }
    });
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

/// Keeps `text`, with no images, as the draft of `key`.
pub fn set_text(key: &str, text: &str, cx: &mut App) {
    set(
        key,
        &Outgoing {
            text: text.to_string(),
            images: Vec::new(),
        },
        cx,
    );
}

/// The drafts to write, when they changed since the last save.
fn unsaved(cx: &App) -> Option<(PathBuf, BTreeMap<String, String>)> {
    let drafts = cx.global::<Drafts>();
    let path = drafts.path.clone()?;
    drafts.changed.then(|| (path, drafts.drafts.clone()))
}

fn write(path: &Path, drafts: BTreeMap<String, String>) -> std::io::Result<()> {
    let text = serde_json::to_string_pretty(&DraftsFile { drafts })?;
    crate::prefs::write_atomically(path, &text)
}

fn schedule_save(cx: &mut App) {
    if cx.global::<Drafts>().path.is_none() {
        return;
    }
    // Replacing the task drops the one waiting: only a pause saves.
    let task = cx.spawn(async move |cx| {
        cx.background_executor().timer(SAVE_DELAY).await;
        let Some((path, drafts)) = cx.update(|cx| {
            let unsaved = unsaved(cx);
            cx.global_mut::<Drafts>().changed = false;
            unsaved
        }) else {
            return;
        };
        // Written as JSON off the UI thread.
        let written = cx
            .background_spawn({
                let path = path.clone();
                async move { write(&path, drafts) }
            })
            .await;
        if let Err(error) = written {
            tracing::error!("couldn't save {} ({error})", path.display());
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
        cx.update(|cx| set_text("s1", "Count the", cx));
        cx.update(|cx| set_text("s1", "Count the pears", cx));
        cx.run_until_parked();
        assert!(!path.exists(), "written before the pause");
        cx.executor().advance_clock(SAVE_DELAY);
        cx.run_until_parked();
        assert_eq!(
            load(&path).get("s1").map(String::as_str),
            Some("Count the pears")
        );
        // An emptied draft is forgotten.
        cx.update(|cx| set_text("s1", "  ", cx));
        cx.executor().advance_clock(SAVE_DELAY);
        cx.run_until_parked();
        assert!(load(&path).is_empty());
        assert!(cx.read(|cx| get("s1", cx)).is_none());
    }

    /// A draft's images are kept beside its text, in memory only: a draft
    /// of an image alone is kept, and the file never has it.
    #[gpui_kit::test]
    async fn a_drafts_images_are_kept_in_memory_only(cx: &mut gpui_kit::TestAppContext) {
        let folder = tempfile::tempdir().unwrap();
        let path = file(folder.path());
        cx.update(|cx| init(BTreeMap::new(), Some(path.clone()), cx));
        let image = Attached::new(
            gpui_kit::ImageFormat::Png,
            crate::testing::PNG.to_vec(),
            Some("shot.png".into()),
        );
        let draft = Outgoing {
            text: String::new(),
            images: vec![image.clone()],
        };
        cx.update(|cx| set("s1", &draft, cx));
        cx.executor().advance_clock(SAVE_DELAY);
        cx.run_until_parked();
        let kept = cx.read(|cx| get("s1", cx)).unwrap();
        assert!(kept.text.is_empty());
        assert!(kept.images.len() == 1 && kept.images[0].is(&image));
        assert!(load(&path).is_empty());
        // Emptied, it's forgotten.
        cx.update(|cx| set("s1", &Outgoing::default(), cx));
        assert!(cx.read(|cx| get("s1", cx)).is_none());
    }
}
