//! Spell checking and word suggestions for the composer, from GPUI Kit's
//! writing tools. The vocabulary is Arko Studio's own, in
//! `<arko folder>/studio-writing/`, and learns only from what the user types
//! here: never pasted text, program text or the agents' replies.

use std::path::Path;

use gpui_kit::writing::{SuggestionMode, Writing, WritingOptions};
use gpui_kit::{App, Global};

use crate::prefs;

/// The folder in Arko's folder that holds the vocabulary and the words the
/// user added to the dictionary.
pub const FOLDER: &str = "studio-writing";

/// The writing aids every composer shares.
pub struct WritingAids(pub Writing);

impl Global for WritingAids {}

/// Starts loading the dictionary and the vocabulary from Arko's folder
/// `arko` (on a background thread), or keeps them in memory (tests).
pub fn init(arko: Option<&Path>, cx: &mut App) {
    let writing = match arko {
        Some(arko) => Writing::load(WritingOptions::new().with_data_dir(arko.join(FOLDER)), cx),
        None => Writing::in_memory(),
    };
    cx.set_global(WritingAids(writing));
    follow_prefs(cx);
    cx.observe_global::<prefs::PrefsStore>(follow_prefs)
        .detach();
    // Cmd+Right (Ctrl+Right) takes a suggestion's next word.
    cx.bind_keys(gpui_kit::writing::accept_word_bindings());
    // What was learned is kept as the app quits.
    cx.on_app_quit(|cx| cx.global::<WritingAids>().0.save_in_background(cx))
        .detach();
}

/// How eagerly suggestions show, as Settings › Input says.
pub fn mode(suggestions: prefs::Suggestions) -> SuggestionMode {
    match suggestions {
        prefs::Suggestions::Off => SuggestionMode::Off,
        prefs::Suggestions::Quiet => SuggestionMode::Quiet,
        prefs::Suggestions::Eager => SuggestionMode::Eager,
    }
}

fn follow_prefs(cx: &mut App) {
    let wanted = mode(prefs::get(cx).input.suggestions);
    let writing = &cx.global::<WritingAids>().0;
    if writing.suggestion_mode() != wanted {
        writing.set_suggestion_mode(wanted);
    }
}

/// Saves what the suggestions learned, on a background thread.
pub fn save(cx: &App) {
    cx.global::<WritingAids>().0.save_in_background(cx).detach();
}
