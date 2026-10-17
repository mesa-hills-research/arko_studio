//! Settings in the app's real window, driven by clicks and keys on GPUI's
//! test platform, on the fake backend: nothing touches Arko's folder.

use std::sync::Arc;
use std::time::SystemTime;

use gpui_kit::component::WindowExt as _;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, Entity, Point, TestAppContext, WindowBounds,
    WindowOptions, px, size,
};

use super::SettingsPage;
use crate::Startup;
use crate::backend::account_words::LoginState;
use crate::backend::{Backend as _, FakeBackend, Mode};
use crate::prefs;
use crate::tests::{act, open};
use crate::workspace::{self, Workspace};

/// The app on the sample sessions, with `settings`. Dialogs open and
/// close without fading, so a click lands on the dialog that's there.
fn start_with(cx: &mut TestAppContext, mut settings: prefs::Prefs) -> Arc<FakeBackend> {
    settings.appearance.motion = prefs::Motion::Off;
    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    cx.update(|cx| crate::init(Startup::for_tests(backend.clone()).with_prefs(settings), cx));
    backend
}

/// A window `height` tall on Settings' page `page`. A dialog opened in a
/// window much taller than the test screen can't be clicked.
fn settings_window(
    cx: &mut TestAppContext,
    page: usize,
    height: f32,
) -> (AnyWindowHandle, Entity<Workspace>, Entity<SettingsPage>) {
    let (window, workspace) = cx.update(|cx| {
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: Point::default(),
                size: size(px(1300.), px(height)),
            })),
            ..Default::default()
        };
        workspace::open_window(options, cx).unwrap()
    });
    cx.run_until_parked();
    act(cx, window, |window, cx| window.press("secondary-,", cx));
    let settings = cx.read(|cx| workspace.read(cx).settings().unwrap().clone());
    cx.update(|cx| settings.update(cx, |settings, cx| settings.show_page(page, cx)));
    // The page reads the accounts on a background thread.
    act(cx, window, |_, _| {});
    (window, workspace, settings)
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

/// A key is added in a masked field, replaced, and removed after a
/// confirmation, all through the backend; keys Arko didn't save can't be
/// removed here.
#[gpui_kit::test]
fn api_keys_are_added_and_removed_through_the_backend(cx: &mut TestAppContext) {
    let backend = start_with(cx, prefs::Prefs::default());
    let (window, _, _) = settings_window(cx, 2, 1400.);
    act(cx, window, |window, _| {
        assert!(window.try_find("key-openrouter").is_some());
        // The OpenAI key comes from the environment: only Replace.
        assert!(window.try_find("remove-openai").is_none());
        assert!(window.try_find("remove-anthropic").is_some());
    });
    act(cx, window, |window, cx| window.click("key-openrouter", cx));
    act(cx, window, |window, cx| {
        assert!(window.has_active_dialog(cx));
        window.input("short", cx);
    });
    act(cx, window, |window, cx| window.click("key-save", cx));
    act(cx, window, |window, _| {
        assert!(window.try_find("key-error").is_some());
    });
    act(cx, window, |window, cx| window.press("secondary-a", cx));
    act(cx, window, |window, cx| {
        window.input("sk-or-v1-0123456789abcdef", cx)
    });
    act(cx, window, |window, cx| window.click("key-save", cx));
    let added = backend.added_keys();
    assert_eq!(added.last().map(|(id, _)| id.as_str()), Some("openrouter"));
    act(cx, window, |window, cx| {
        assert!(!window.has_active_dialog(cx));
        assert_eq!(window.notifications(cx).len(), 1);
        // Saved in Arko's folder now: it can be removed.
        assert!(window.try_find("remove-openrouter").is_some());
    });
    act(cx, window, |window, cx| {
        window.click("remove-openrouter", cx)
    });
    assert!(
        cx.read(|_| backend
            .accounts()
            .key("openrouter")
            .unwrap()
            .masked
            .is_some()),
        "asks first"
    );
    act(cx, window, |window, cx| {
        window.within("dialog").click("ok", cx)
    });
    assert_eq!(backend.accounts().key("openrouter").unwrap().masked, None);
    act(cx, window, |window, _| {
        assert!(window.try_find("remove-openrouter").is_none());
    });
}

/// Check again reads Claude Code again, and a chosen auth.json becomes the
/// ChatGPT login.
#[gpui_kit::test]
fn logins_are_checked_and_chosen(cx: &mut TestAppContext) {
    let backend = start_with(cx, prefs::Prefs::default());
    let (window, _, settings) = settings_window(cx, 2, 2400.);
    let read = backend.accounts_read();
    act(cx, window, |window, cx| {
        window.click("check-claude-code", cx)
    });
    assert_eq!(backend.accounts_read(), read + 1);
    let page = cx.read(|cx| settings.read(cx).accounts().clone());
    act(cx, window, |window, cx| {
        super::accounts::use_login(&page, "/home/ada/notes.txt".into(), window, cx)
    });
    act(cx, window, |window, cx| {
        assert_eq!(window.notifications(cx).len(), 1, "says why not");
        super::accounts::use_login(&page, "/home/ada/.codex/auth.json".into(), window, cx)
    });
    let state = cx.read(|cx| page.read(cx).accounts().unwrap().chatgpt.state.clone());
    assert_eq!(state, LoginState::Ready);
}

/// Sizes and times are saved as soon as they read as arko reads them;
/// until then the field says why, and the setting stays. A reset empties
/// the field.
#[gpui_kit::test]
fn typed_session_defaults_are_checked_and_saved(cx: &mut TestAppContext) {
    let backend = start_with(cx, prefs::Prefs::default());
    let (window, _, settings) = settings_window(cx, 3, 2400.);
    let page = cx.read(|cx| settings.read(cx).sessions().clone());
    act(cx, window, |window, cx| window.click("handover-at", cx));
    act(cx, window, |window, cx| window.input("5k", cx));
    let errors = cx.read(|cx| page.read(cx).errors());
    assert!(errors[0].is_some());
    assert_eq!(cx.read(|cx| prefs::get(cx).new_sessions.handover_at), None);
    act(cx, window, |window, _| {
        assert!(window.try_find("handover-at-error").is_some());
    });
    act(cx, window, |window, cx| window.press("secondary-a", cx));
    act(cx, window, |window, cx| window.input("500k", cx));
    assert_eq!(
        cx.read(|cx| prefs::get(cx).new_sessions.handover_at),
        Some(500_000)
    );
    assert_eq!(backend.defaults().handover_at, Some(500_000));
    assert_eq!(cx.read(|cx| page.read(cx).errors())[0], None);

    act(cx, window, |window, cx| {
        window.click("background-after", cx)
    });
    act(cx, window, |window, cx| window.input("2h", cx));
    assert!(cx.read(|cx| page.read(cx).errors())[2].is_some());
    act(cx, window, |window, cx| window.press("secondary-a", cx));
    act(cx, window, |window, cx| window.input("off", cx));
    assert_eq!(
        cx.read(|cx| prefs::get(cx).new_sessions.background_after),
        Some(0)
    );

    act(cx, window, |window, cx| {
        window.click("chat-instructions", cx)
    });
    act(cx, window, |window, cx| window.input("Answer briefly.", cx));
    assert_eq!(
        cx.read(|cx| prefs::get(cx).new_sessions.chat_instructions.clone()),
        "Answer briefly."
    );

    // Reset, as the page's reset does it: the fields follow.
    cx.update(|cx| {
        prefs::update(cx, |prefs| {
            prefs.new_sessions.handover_at = None;
            prefs.new_sessions.chat_instructions.clear();
        })
    });
    act(cx, window, |_, _| {});
    let typed = cx.read(|cx| page.read(cx).typed(cx));
    assert_eq!(typed[0].as_ref(), "");
    assert_eq!(typed[2].as_ref(), "off");
}

/// Opening Settings reads nothing on the UI thread: the installed fonts
/// and the accounts are read in the background while the page shows, and
/// the fonts once for the app.
#[gpui_kit::test]
fn opening_settings_leaves_the_reading_to_the_background(cx: &mut TestAppContext) {
    let backend = start_with(cx, prefs::Prefs::default());
    let (window, workspace) = open(cx, 1300.);
    cx.update_window(window, |_, window, cx| {
        window.dispatch_action(Box::new(crate::menus::OpenSettings), cx)
    })
    .unwrap();
    let settings = cx.read(|cx| workspace.read(cx).settings().unwrap().clone());
    cx.read(|cx| {
        let settings = settings.read(cx);
        assert!(
            settings.font_pickers().is_none(),
            "fonts read on the UI thread"
        );
        assert!(settings.accounts().read(cx).accounts().is_none());
    });
    assert_eq!(backend.accounts_read(), 0);
    act(cx, window, |window, _| {
        assert!(window.try_find("settings-page").is_some());
    });
    cx.read(|cx| {
        let settings = settings.read(cx);
        assert!(settings.font_pickers().is_some());
        assert!(settings.accounts().read(cx).accounts().is_some());
    });
    // Opened again, the pickers have the fonts at once.
    act(cx, window, |window, cx| window.press("escape", cx));
    cx.update_window(window, |_, window, cx| {
        window.dispatch_action(Box::new(crate::menus::OpenSettings), cx)
    })
    .unwrap();
    let settings = cx.read(|cx| workspace.read(cx).settings().unwrap().clone());
    assert!(cx.read(|cx| settings.read(cx).font_pickers().is_some()));
}
