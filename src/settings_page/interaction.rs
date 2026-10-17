//! Settings in the app's real window, driven by clicks and keys on GPUI's
//! test platform, on the fake backend: nothing touches Arko's folder.

use std::sync::Arc;
use std::time::SystemTime;

use gpui_kit::TestAppContext;

use crate::Startup;
use crate::backend::{FakeBackend, Mode};
use crate::prefs;
use crate::tests::open;

fn start_with(cx: &mut TestAppContext, settings: prefs::Prefs) -> Arc<FakeBackend> {
    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    cx.update(|cx| crate::init(Startup::for_tests(backend.clone()).with_prefs(settings), cx));
    backend
}

/// The default mode and model set in Settings are what a new session's
/// composer starts with, and the backend has the preferences as they
/// change.
#[gpui_kit::test]
fn a_new_session_starts_as_settings_say(cx: &mut TestAppContext) {
    let mut settings = prefs::Prefs::default();
    settings.new_sessions.mode = prefs::NewSessionMode::Chat;
    settings.new_sessions.model = Some("claude-sonnet-5-5".into());
    let backend = start_with(cx, settings);
    let (_, workspace) = open(cx, 1300.);
    cx.read(|cx| {
        let view = workspace.read(cx).session_view().read(cx);
        assert_eq!(view.draft().mode, Mode::Chat);
        assert_eq!(view.draft_model().as_deref(), Some("claude-sonnet-5-5"));
    });
    assert_eq!(backend.defaults().mode, Mode::Chat);
    cx.update(|cx| prefs::update(cx, |prefs| prefs.new_sessions.handover_at = Some(400_000)));
    assert_eq!(backend.defaults().handover_at, Some(400_000));
    // A window's draft keeps the mode it has: only the next one changes.
    cx.update(|cx| {
        prefs::update(cx, |prefs| {
            prefs.new_sessions.mode = prefs::NewSessionMode::Agent
        })
    });
    let mode = cx.read(|cx| workspace.read(cx).session_view().read(cx).draft().mode);
    assert_eq!(mode, Mode::Chat);
    let (_, second) = open(cx, 1300.);
    let mode = cx.read(|cx| second.read(cx).session_view().read(cx).draft().mode);
    assert_eq!(mode, Mode::Agent);
}
