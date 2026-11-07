//! What the user put in each session's composer and hasn't sent: kept
//! when they switch sessions, and the text across restarts too, in
//! `<arko folder>/studio-drafts.json`. Typing changes them in memory, and
//! the file is written after a pause, on a background thread, never per key.
//! A draft's images are kept in memory only, beside its text, so they're
//! gone when Studio quits.
//!
//! The drafts are where a session's unsent words belong, whichever window
//! shows the session. A window keeps the words it shows ([`save`]), and
//! words given back go to the draft and into every window that shows the
//! session, so a window can't erase words it never had.

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
    /// What a send last took from each, which a window still showing it
    /// doesn't put back (in memory only).
    taken: BTreeMap<String, Outgoing>,
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
        taken: BTreeMap::new(),
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

/// Keeps `draft`, what a window holds now, as the draft of `key`. `kept` is
/// what the window held when it last took, kept or was given back the
/// draft, and `sent` is what a send just took from it. Of two windows'
/// different words, the last kept wins.
/// - Words a window shows are kept, unless it changed nothing since and
///   the stored draft has them already, or a send took them from it.
/// - An empty draft takes the stored one's place only when the stored one
///   is what the window held, or what its send just took. So a window that
///   never had another window's words can't erase them.
pub fn save(key: &str, draft: &Outgoing, kept: &Outgoing, sent: Option<&Outgoing>, cx: &mut App) {
    let stored = get(key, cx).unwrap_or_default();
    let keeps = if is_empty(draft) {
        same(&stored, kept) || sent.is_some_and(|sent| same(&stored, sent))
    } else if same(draft, kept) {
        let taken = cx.global::<Drafts>().taken.get(key);
        !has(&stored, draft) && !taken.is_some_and(|taken| same(taken, draft))
    } else {
        true
    };
    if !keeps {
        return;
    }
    if sent.is_some() && is_empty(draft) && !is_empty(&stored) {
        cx.global_mut::<Drafts>()
            .taken
            .insert(key.to_string(), stored);
    }
    set(key, draft, cx);
}

/// Puts `back`, the words of a send that didn't go, after the draft of
/// `key` ([`with_back`]).
pub fn give_back(key: &str, back: &Outgoing, cx: &mut App) {
    cx.global_mut::<Drafts>().taken.remove(key);
    let draft = with_back(get(key, cx).unwrap_or_default(), back);
    set(key, &draft, cx);
}

/// `draft` with `back`, words given back, after its own words unless it
/// holds them already, and with the images of `back` it lacks.
pub fn with_back(mut draft: Outgoing, back: &Outgoing) -> Outgoing {
    // Words joined before are one of the draft's parts between blank lines.
    let parts = format!("\n\n{}\n\n", draft.text.trim());
    if !parts.contains(&format!("\n\n{}\n\n", back.text.trim())) {
        draft.text = joined(&draft.text, &back.text);
    }
    for image in &back.images {
        if !draft.images.iter().any(|kept| kept.is(image)) {
            draft.images.push(image.clone());
        }
    }
    draft
}

/// Whether `a` and `b` hold the same text and the same images.
pub fn same(a: &Outgoing, b: &Outgoing) -> bool {
    a.text == b.text
        && a.images.len() == b.images.len()
        && a.images.iter().zip(&b.images).all(|(a, b)| a.is(b))
}

/// Whether `stored` has the words and the images of `draft` already.
fn has(stored: &Outgoing, draft: &Outgoing) -> bool {
    stored.text.contains(draft.text.trim())
        && draft
            .images
            .iter()
            .all(|image| stored.images.iter().any(|kept| kept.is(image)))
}

/// Whether `draft` has neither words nor images.
fn is_empty(draft: &Outgoing) -> bool {
    draft.text.trim().is_empty() && draft.images.is_empty()
}

/// `more` after `text`, a blank line between them when both have words.
fn joined(text: &str, more: &str) -> String {
    match (text.trim().is_empty(), more.trim().is_empty()) {
        (true, _) => more.to_string(),
        (false, true) => text.to_string(),
        (false, false) => format!("{}\n\n{more}", text.trim_end()),
    }
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

    fn text(text: &str) -> Outgoing {
        Outgoing {
            text: text.into(),
            images: Vec::new(),
        }
    }

    /// A window keeps the words it shows, the last kept winning, but none
    /// the draft has already or a send took, unless it changed them. It
    /// empties the draft only where it held it or its send took it.
    #[gpui_kit::test]
    fn a_window_keeps_its_words_and_empties_only_its_own(cx: &mut gpui_kit::TestAppContext) {
        cx.update(|cx| init(BTreeMap::new(), None, cx));
        let stored =
            |cx: &mut gpui_kit::TestAppContext| cx.read(|cx| get("s1", cx)).map(|draft| draft.text);
        let keep =
            |cx: &mut gpui_kit::TestAppContext, draft: &str, kept: &str, sent: Option<&str>| {
                let sent = sent.map(text);
                cx.update(|cx| save("s1", &text(draft), &text(kept), sent.as_ref(), cx));
                stored(cx)
            };
        assert_eq!(
            keep(cx, "Count the pears", "", None).as_deref(),
            Some("Count the pears")
        );
        assert_eq!(keep(cx, "Hello", "", None).as_deref(), Some("Hello"));
        // A window that never had them, untouched or sending other words.
        assert_eq!(keep(cx, "", "", None).as_deref(), Some("Hello"));
        assert_eq!(keep(cx, "", "", Some("Go on.")).as_deref(), Some("Hello"));
        // The window that held them sends them.
        assert_eq!(keep(cx, "", "Hello", Some("Hello")), None);
        // Words another window still shows are kept again, unless a send
        // took them.
        let pears = Some("Count the pears");
        assert_eq!(
            keep(cx, "Count the pears", "Count the pears", None).as_deref(),
            pears
        );
        assert_eq!(keep(cx, "", "Count the pears", pears), None);
        assert_eq!(keep(cx, "Count the pears", "Count the pears", None), None);
        // Words given back are no longer taken, and come back once.
        cx.update(|cx| give_back("s1", &text("Go on."), cx));
        cx.update(|cx| give_back("s1", &text("Go on."), cx));
        assert_eq!(stored(cx).as_deref(), Some("Go on."));
        let both = Some("Count the pears\n\nGo on.");
        assert_eq!(
            keep(cx, "Count the pears\n\nGo on.", "Go on.", None).as_deref(),
            both
        );
        // A window that holds only some of them, unchanged, keeps the rest.
        assert_eq!(keep(cx, "Go on.", "Go on.", None).as_deref(), both);
        // A changed one keeps its own, and an emptied one empties it.
        assert_eq!(keep(cx, "Go on", "Go on.", None).as_deref(), Some("Go on"));
        assert_eq!(keep(cx, "", "Go on", None), None);
    }

    /// Words given back come after a draft's own, once, as a part of it.
    #[test]
    fn words_given_back_join_a_draft_once() {
        let back = text("Go on.");
        let with = |draft: &str| with_back(text(draft), &back).text;
        assert_eq!(with(""), "Go on.");
        assert_eq!(with("Count the pears"), "Count the pears\n\nGo on.");
        assert_eq!(
            with("Count the pears\n\nGo on."),
            "Count the pears\n\nGo on."
        );
        assert_eq!(with("Go on. Then stop."), "Go on. Then stop.\n\nGo on.");
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
