//! Settings › Sessions › Names in the app's real window, driven by clicks on
//! GPUI's test platform, on the fake backend (the setting in memory) and on
//! the real one over a temporary folder.

use std::time::SystemTime;

use gpui_kit::component::setting::SettingItem;
use gpui_kit::component::text::Text;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AnyWindowHandle, Entity, TestAppContext};

use super::SESSIONS_PAGE;
use super::SessionsPage;
use super::interaction::{settings_window, start_on};
use crate::backend::{Backend as _, FakeBackend, Way};
use crate::prefs;
use crate::tests::act;

/// The app with `backend`, on Settings › Sessions.
fn sessions_page(
    cx: &mut TestAppContext,
    backend: FakeBackend,
) -> (
    std::sync::Arc<FakeBackend>,
    AnyWindowHandle,
    Entity<SessionsPage>,
) {
    let backend = start_on(cx, backend, prefs::Prefs::default());
    let (window, _, settings) = settings_window(cx, SESSIONS_PAGE, 4000.);
    let page = cx.read(|cx| settings.read(cx).sessions().clone());
    (backend, window, page)
}

fn sample() -> FakeBackend {
    FakeBackend::sample("/arko", SystemTime::now())
}

/// Each row as (title, description), the rows that aren't plain items
/// as `None`.
fn rows(
    cx: &mut TestAppContext,
    page: &Entity<SessionsPage>,
) -> Vec<Option<(String, Option<String>)>> {
    cx.read(|cx| {
        page.read(cx)
            .names_items(page)
            .into_iter()
            .map(|item| match item {
                SettingItem::Item {
                    title, description, ..
                } => Some((
                    title.to_string(),
                    description.map(|text| match text {
                        Text::String(text) => text.to_string(),
                        Text::TextView(_) => String::new(),
                    }),
                )),
                SettingItem::Element { .. } => None,
            })
            .collect()
    })
}

fn naming(
    cx: &mut TestAppContext,
    page: &Entity<SessionsPage>,
) -> (bool, Vec<(Way, bool)>, Option<String>) {
    cx.read(|cx| page.read(cx).naming())
}

fn choose(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
    page: &Entity<SessionsPage>,
    model: bool,
) {
    act(cx, window, |_, cx| {
        page.update(cx, |page, cx| page.set_by_model(model, cx))
    });
}

/// Sessions are named by the first words until a model is chosen, and
/// then the four ways show in their order.
#[gpui_kit::test]
fn the_first_words_name_sessions_until_a_model_is_chosen(cx: &mut TestAppContext) {
    let (backend, window, page) = sessions_page(cx, sample());
    let rows_now = rows(cx, &page);
    assert_eq!(rows_now.len(), 1, "only the choice: {rows_now:?}");
    let (title, description) = rows_now[0].clone().unwrap();
    assert_eq!(title, "Session names");
    assert!(description.unwrap().starts_with(
        "With a model, a session’s first message goes to the provider that names it."
    ),);
    act(cx, window, |window, _| {
        assert!(window.try_find(("name-way", 0usize)).is_none());
        assert!(window.try_find("names-fallback").is_none());
    });
    assert!(
        backend.saved_session_names().is_empty(),
        "nothing is written by looking"
    );
    let (by_model, _, _) = naming(cx, &page);
    assert!(!by_model);
}

/// Choosing a model turns on the two plans and writes them through the
/// backend. The four ways then show with what the others lack, and the
/// first words come last.
#[gpui_kit::test]
fn choosing_a_model_writes_the_two_plans_and_lists_the_four_ways(cx: &mut TestAppContext) {
    let (backend, window, page) = sessions_page(cx, sample());
    choose(cx, window, &page, true);
    assert_eq!(
        backend.saved_session_names(),
        [vec![Way::ClaudePlan, Way::ChatGptPlan]]
    );
    let rows_now = rows(cx, &page);
    let described = |at: usize| rows_now[at].clone().unwrap();
    assert_eq!(rows_now.len(), 6, "choice, four ways, the first words");
    assert_eq!(
        described(1),
        ("Haiku 5.5 on the Claude plan".to_string(), None)
    );
    assert_eq!(
        described(2),
        ("GPT-6 Luna on the ChatGPT plan".to_string(), None)
    );
    assert_eq!(
        described(3),
        (
            "Haiku 5.5 with the Anthropic API key".to_string(),
            Some("No Anthropic API key".to_string())
        )
    );
    assert_eq!(
        described(4),
        (
            "GPT-6 Luna with the OpenAI API key".to_string(),
            Some("No OpenAI API key".to_string())
        )
    );
    assert!(rows_now[5].is_none(), "the muted line is an element");
    act(cx, window, |window, _| {
        for at in 0..4usize {
            assert!(window.try_find(("name-way", at)).is_some());
        }
        assert!(window.try_find("names-fallback").is_some());
    });
    // Back to the first words writes none, and the list goes.
    choose(cx, window, &page, false);
    assert_eq!(backend.saved_session_names().last(), Some(&Vec::new()));
    act(cx, window, |window, _| {
        assert!(window.try_find(("name-way", 0usize)).is_none());
    });
    // A model again keeps the ways that were on.
    choose(cx, window, &page, true);
    assert_eq!(
        backend.saved_session_names().last(),
        Some(&vec![Way::ClaudePlan, Way::ChatGptPlan])
    );
}

/// A way that can't run says what it lacks, in the app's typography.
#[gpui_kit::test]
fn a_way_that_cant_run_says_why(cx: &mut TestAppContext) {
    let backend = sample().with_session_names(
        &[Way::ClaudePlan, Way::ChatGptPlan],
        &[
            (Way::ClaudePlan, "Claude Code isn't signed in"),
            (Way::ChatGptPlan, "No ChatGPT login"),
        ],
    );
    let (_, _, page) = sessions_page(cx, backend);
    let rows_now = rows(cx, &page);
    assert_eq!(
        rows_now[1].clone().unwrap().1.as_deref(),
        Some("Claude Code isn’t signed in")
    );
    assert_eq!(
        rows_now[2].clone().unwrap().1.as_deref(),
        Some("No ChatGPT login")
    );
    assert_eq!(rows_now[3].clone().unwrap().1, None);
}

/// A way is turned on and off with its checkbox, and moved with its
/// arrows. Each change writes the ways that are on, in order.
#[gpui_kit::test]
fn the_ways_are_turned_on_and_moved_and_each_change_is_written(cx: &mut TestAppContext) {
    let backend = sample().with_session_names(&[Way::ClaudePlan, Way::ChatGptPlan], &[]);
    let (backend, window, page) = sessions_page(cx, backend);
    let (by_model, list, _) = naming(cx, &page);
    assert!(by_model, "ways saved mean a model names sessions");
    assert_eq!(
        list,
        [
            (Way::ClaudePlan, true),
            (Way::ChatGptPlan, true),
            (Way::ClaudeKey, false),
            (Way::OpenAiKey, false),
        ]
    );
    // The Anthropic key on.
    act(cx, window, |window, cx| {
        window.click(("name-way", 2usize), cx)
    });
    // The ChatGPT plan first.
    act(cx, window, |window, cx| {
        window.click(("name-up", 1usize), cx)
    });
    // The Claude plan off.
    act(cx, window, |window, cx| {
        window.click(("name-way", 1usize), cx)
    });
    // The Anthropic key before what is left.
    act(cx, window, |window, cx| {
        window.click(("name-up", 2usize), cx)
    });
    assert_eq!(
        backend.saved_session_names(),
        [
            vec![Way::ClaudePlan, Way::ChatGptPlan, Way::ClaudeKey],
            vec![Way::ChatGptPlan, Way::ClaudePlan, Way::ClaudeKey],
            vec![Way::ChatGptPlan, Way::ClaudeKey],
            vec![Way::ChatGptPlan, Way::ClaudeKey],
        ]
    );
    let (_, list, _) = naming(cx, &page);
    assert_eq!(
        list,
        [
            (Way::ChatGptPlan, true),
            (Way::ClaudeKey, true),
            (Way::ClaudePlan, false),
            (Way::OpenAiKey, false),
        ]
    );
    // The ends have nowhere to go.
    act(cx, window, |window, cx| {
        window.click(("name-up", 0usize), cx)
    });
    act(cx, window, |window, cx| {
        window.click(("name-down", 3usize), cx)
    });
    assert_eq!(backend.saved_session_names().len(), 4);
    // A way moved down past the others.
    act(cx, window, |window, cx| {
        window.click(("name-down", 0usize), cx)
    });
    assert_eq!(
        backend.saved_session_names().last(),
        Some(&vec![Way::ClaudeKey, Way::ChatGptPlan])
    );
}

/// A write the backend refuses is said under the ways, and the list goes
/// back to what is saved.
#[gpui_kit::test]
fn a_refused_write_is_said_and_the_list_shows_what_is_saved(cx: &mut TestAppContext) {
    let backend =
        sample().refusing_session_names("Couldn't save the session names (Permission denied).");
    let (backend, window, page) = sessions_page(cx, backend);
    choose(cx, window, &page, true);
    let (by_model, _, error) = naming(cx, &page);
    assert_eq!(
        error.as_deref(),
        Some("Couldn’t save the session names (Permission denied).")
    );
    assert!(!by_model, "the file still names by the first words");
    assert!(backend.saved_session_names().is_empty());
    act(cx, window, |window, _| {
        assert!(window.try_find("names-error").is_some());
    });
}

/// The setting is arko's file in Arko's folder, which the terminal app
/// reads: the real backend writes it and arko reads it back.
#[test]
fn the_real_backend_writes_the_file_the_terminal_reads() {
    use crate::backend::ArkoBackend;
    let temp = tempfile::tempdir().unwrap();
    let backend = ArkoBackend::new(temp.path());
    backend
        .set_session_names(&[Way::OpenAiKey, Way::ClaudePlan])
        .unwrap();
    assert_eq!(
        arko::session_names::load(temp.path()),
        [Way::OpenAiKey, Way::ClaudePlan]
    );
    backend.set_session_names(&[]).unwrap();
    assert!(arko::session_names::load(temp.path()).is_empty());
    // A folder that can't be written says so.
    let file = temp.path().join("not-a-folder");
    std::fs::write(&file, "").unwrap();
    let why = ArkoBackend::new(&file)
        .set_session_names(&[Way::ClaudePlan])
        .unwrap_err();
    assert!(why.starts_with("Couldn't save the session names"), "{why}");
}
