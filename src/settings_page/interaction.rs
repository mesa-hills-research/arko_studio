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

use super::{
    ACCOUNTS_PAGE, ADVANCED_PAGE, APPEARANCE_PAGE, EXTENSIONS_PAGE, INPUT_PAGE, NOTIFICATIONS_PAGE,
    SESSIONS_PAGE, SettingsPage, WEB_PAGE_PAGE,
};
use crate::Startup;
use crate::backend::account_words::LoginState;
use crate::backend::{Backend as _, FakeBackend, Mode, ModelChoice, ModelChoices, ModelGroup};
use crate::prefs;
use crate::tests::{act, open};
use crate::workspace::{self, Workspace};

/// The app on the sample sessions, with `settings`. Dialogs open and
/// close without fading, so a click lands on the dialog that's there.
fn start_with(cx: &mut TestAppContext, settings: prefs::Prefs) -> Arc<FakeBackend> {
    start_on(
        cx,
        FakeBackend::sample("/arko", SystemTime::now()),
        settings,
    )
}

/// The app on `backend`, with `settings`, as [`start_with`].
pub(super) fn start_on(
    cx: &mut TestAppContext,
    backend: FakeBackend,
    mut settings: prefs::Prefs,
) -> Arc<FakeBackend> {
    settings.appearance.motion = prefs::Motion::Off;
    let backend = Arc::new(backend);
    cx.update(|cx| crate::init(Startup::for_tests(backend.clone()).with_prefs(settings), cx));
    backend
}

/// A window `height` tall on Settings' page `page`. A dialog opened in a
/// window much taller than the test screen can't be clicked.
pub(super) fn settings_window(
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
        assert_eq!(view.draft_model(cx).as_deref(), Some("claude-sonnet-5-5"));
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

/// Last used, the default, starts a new session in the mode the last one
/// started or switched to, and in Agent before any.
#[gpui_kit::test]
fn last_used_starts_a_new_session_in_the_last_mode(cx: &mut TestAppContext) {
    let backend = start_with(cx, prefs::Prefs::default());
    assert_eq!(
        cx.read(|cx| prefs::get(cx).new_sessions.mode),
        prefs::NewSessionMode::Last
    );
    let (_, first) = open(cx, 1300.);
    let mode = cx.read(|cx| first.read(cx).session_view().read(cx).draft().mode);
    assert_eq!(mode, Mode::Agent);
    assert_eq!(backend.defaults().mode, Mode::Agent);
    cx.update(|cx| {
        prefs::update(cx, |prefs| {
            prefs.new_sessions.last_mode = Some(prefs::NewSessionMode::Chat)
        })
    });
    let (_, second) = open(cx, 1300.);
    let mode = cx.read(|cx| second.read(cx).session_view().read(cx).draft().mode);
    assert_eq!(mode, Mode::Chat);
    assert_eq!(backend.defaults().mode, Mode::Chat);
    // A mode chosen in Settings wins over the last one.
    cx.update(|cx| {
        prefs::update(cx, |prefs| {
            prefs.new_sessions.mode = prefs::NewSessionMode::Agent
        })
    });
    let (_, third) = open(cx, 1300.);
    let mode = cx.read(|cx| third.read(cx).session_view().read(cx).draft().mode);
    assert_eq!(mode, Mode::Agent);
}

/// The default model and effort, changed in Settings while a window is
/// open: its new session's chip names them, and the session starts on
/// them.
#[gpui_kit::test]
fn the_chip_follows_the_defaults_set_in_settings(cx: &mut TestAppContext) {
    start_with(cx, prefs::Prefs::default());
    let (window, workspace) = open(cx, 1300.);
    let view = cx.read(|cx| workspace.read(cx).session_view().clone());
    let chip = |cx: &mut TestAppContext| cx.read(|cx| view.read(cx).chip_label(cx));
    assert_eq!(chip(cx), "Agent · Opus 5.5");
    cx.update(|cx| {
        prefs::update(cx, |prefs| {
            prefs.new_sessions.model = Some("claude-sonnet-5-5".into());
            prefs.new_sessions.effort = Some("high".into());
        })
    });
    act(cx, window, |_, _| {});
    assert_eq!(chip(cx), "Agent · Sonnet 5.5 · high");
    act(cx, window, |window, cx| window.input("Hello", cx));
    act(cx, window, |window, cx| window.press("enter", cx));
    let started = cx.read(|cx| view.read(cx).session(cx).map(|session| session.model));
    assert_eq!(
        started.map(|model| arko::models::display_name(&model)),
        Some("Sonnet 5.5".to_string())
    );
}

/// "Needs an OpenRouter key" in the composer's model list goes once the
/// key is added in Settings: the composer lists its models again.
#[gpui_kit::test]
fn a_key_added_in_settings_makes_its_models_usable(cx: &mut TestAppContext) {
    let group = |name: &str, needs: Option<&str>, models: &[(&str, &str)]| ModelGroup {
        name: name.into(),
        needs: needs.map(str::to_string),
        models: models
            .iter()
            .map(|(name, alias)| ModelChoice {
                name: (*name).into(),
                alias: (*alias).into(),
            })
            .collect(),
    };
    let backend = FakeBackend::sample("/arko", SystemTime::now())
        .with_models(ModelChoices {
            groups: vec![
                group(
                    "Anthropic API, billed per token",
                    None,
                    &[("claude-opus-5-5", "opus")],
                ),
                group(
                    "OpenRouter, billed per token",
                    Some("an OpenRouter key"),
                    &[("openrouter:moonshotai/kimi-k3", "kimi")],
                ),
            ],
            default: Some("claude-opus-5-5".into()),
            note: None,
        })
        .with_groups_following_keys();
    start_on(cx, backend, prefs::Prefs::default());
    let (window, _, _) = settings_window(cx, ACCOUNTS_PAGE, 1400.);
    act(cx, window, |window, cx| window.press("escape", cx));
    // Whether the composer's model list says the OpenRouter models need
    // their key.
    let needs_key = |cx: &mut TestAppContext| {
        act(cx, window, |window, cx| window.click("mode-model", cx));
        let mut shown = false;
        act(cx, window, |window, cx| {
            shown = window.try_find(("model-needs", 1usize)).is_some();
            window.press("escape", cx);
        });
        shown
    };
    assert!(needs_key(cx));
    act(cx, window, |window, cx| {
        window.dispatch_action(Box::new(crate::menus::OpenAccounts), cx)
    });
    act(cx, window, |window, cx| window.click("key-openrouter", cx));
    act(cx, window, |window, cx| {
        window.input("sk-or-v1-0123456789abcdef", cx)
    });
    act(cx, window, |window, cx| window.click("key-save", cx));
    act(cx, window, |window, cx| window.press("escape", cx));
    assert!(!needs_key(cx));
}

/// A key is added in a masked field, replaced, and removed after a
/// confirmation, all through the backend. Keys Arko didn't save can't be
/// removed here.
#[gpui_kit::test]
fn api_keys_are_added_and_removed_through_the_backend(cx: &mut TestAppContext) {
    let backend = start_with(cx, prefs::Prefs::default());
    let (window, _, _) = settings_window(cx, ACCOUNTS_PAGE, 1400.);
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

/// Enter in the key's field saves the key, as Save does. With nothing
/// pasted, the dialog stays open and says so.
#[gpui_kit::test]
fn enter_in_the_key_field_saves_the_key(cx: &mut TestAppContext) {
    let backend = start_with(cx, prefs::Prefs::default());
    let (window, _, _) = settings_window(cx, ACCOUNTS_PAGE, 1400.);
    act(cx, window, |window, cx| window.click("key-openrouter", cx));
    act(cx, window, |window, cx| window.press("enter", cx));
    act(cx, window, |window, cx| {
        assert!(window.has_active_dialog(cx));
        assert!(window.try_find("key-error").is_some());
    });
    act(cx, window, |window, cx| {
        window.input("sk-or-v1-0123456789abcdef", cx)
    });
    act(cx, window, |window, cx| window.press("enter", cx));
    assert_eq!(
        backend.added_keys(),
        [(
            "openrouter".to_string(),
            "sk-or-v1-0123456789abcdef".to_string()
        )]
    );
    act(cx, window, |window, cx| {
        assert!(!window.has_active_dialog(cx));
    });
}

/// Check again reads Claude Code again. A chosen auth.json joins the ChatGPT
/// logins, which the page lists in order with Make first and Remove, and the
/// last one removed goes back to Codex's own.
#[gpui_kit::test]
fn logins_are_checked_and_chosen(cx: &mut TestAppContext) {
    let backend = start_with(cx, prefs::Prefs::default());
    let (window, _, settings) = settings_window(cx, ACCOUNTS_PAGE, 2400.);
    let read = backend.accounts_read();
    act(cx, window, |window, cx| {
        window.click("check-claude-code", cx)
    });
    assert_eq!(backend.accounts_read(), read + 1);
    let page = cx.read(|cx| settings.read(cx).accounts().clone());
    let logins = |cx: &mut TestAppContext| -> Vec<(String, LoginState)> {
        cx.read(|cx| {
            let accounts = page.read(cx).accounts().unwrap();
            accounts
                .chatgpt
                .iter()
                .map(|login| {
                    let path = login.path.as_ref().unwrap();
                    (path.display().to_string(), login.state.clone())
                })
                .collect()
        })
    };
    act(cx, window, |window, cx| {
        super::accounts::add_login(&page, "/home/ada/notes.txt".into(), window, cx)
    });
    act(cx, window, |window, cx| {
        assert_eq!(window.notifications(cx).len(), 1, "says why not");
        super::accounts::add_login(&page, "/home/ada/.codex/auth.json".into(), window, cx)
    });
    act(cx, window, |window, cx| {
        super::accounts::add_login(&page, "/home/ada/.codex-2/auth.json".into(), window, cx)
    });
    let ready = LoginState::Ready;
    assert_eq!(
        logins(cx),
        [
            ("/home/ada/.codex/auth.json".to_string(), ready.clone()),
            ("/home/ada/.codex-2/auth.json".to_string(), ready.clone()),
        ]
    );
    // The second takes over first, and the one before it goes second.
    act(cx, window, |window, cx| {
        window.click(("first-login", 1usize), cx)
    });
    assert_eq!(logins(cx)[0].0, "/home/ada/.codex-2/auth.json");
    act(cx, window, |window, _| {
        assert!(window.try_find(("first-login", 0usize)).is_none());
    });
    // Removing the last chosen one goes back to Codex's own.
    act(cx, window, |window, cx| {
        window.click(("remove-login", 0usize), cx)
    });
    act(cx, window, |window, cx| {
        window.click(("remove-login", 0usize), cx)
    });
    assert_eq!(
        logins(cx),
        [(
            "/home/ada/.codex/auth.json".to_string(),
            LoginState::Missing
        )]
    );
    let source = cx.read(|cx| page.read(cx).accounts().unwrap().chatgpt[0].source);
    assert_eq!(source, crate::backend::account_words::LoginSource::Default);
    act(cx, window, |window, _| {
        assert!(window.try_find(("remove-login", 0usize)).is_none());
    });
}

/// With no ChatGPT login at all the page still offers Add and no Remove.
#[gpui_kit::test]
fn the_accounts_page_copes_with_no_chatgpt_login(cx: &mut TestAppContext) {
    let backend = start_with(cx, prefs::Prefs::default());
    backend.set_chatgpt_logins(Vec::new());
    let (window, _, _) = settings_window(cx, ACCOUNTS_PAGE, 2400.);
    act(cx, window, |window, _| {
        assert!(window.try_find("choose-login").is_some());
        assert!(window.try_find(("remove-login", 0usize)).is_none());
    });
}

/// Lets the typing rest: what's typed in Settings is checked and saved.
fn rest(cx: &mut TestAppContext) {
    cx.executor().advance_clock(super::fields::TYPING_REST);
    cx.run_until_parked();
}

/// Sizes and times are saved once they read as arko reads them and the
/// typing rests. Until then the field says why, and the setting stays. A
/// reset empties the field.
#[gpui_kit::test]
fn typed_session_defaults_are_checked_and_saved(cx: &mut TestAppContext) {
    let backend = start_with(cx, prefs::Prefs::default());
    let (window, _, settings) = settings_window(cx, SESSIONS_PAGE, 2400.);
    let page = cx.read(|cx| settings.read(cx).sessions().clone());
    act(cx, window, |window, cx| window.click("handover-at", cx));
    act(cx, window, |window, cx| window.input("5k", cx));
    // Not while typing: it may go on to 50k.
    assert_eq!(cx.read(|cx| page.read(cx).errors())[0], None);
    rest(cx);
    let errors = cx.read(|cx| page.read(cx).errors());
    assert!(errors[0].is_some());
    assert_eq!(cx.read(|cx| prefs::get(cx).new_sessions.handover_at), None);
    act(cx, window, |window, _| {
        assert!(window.try_find("handover-at-error").is_some());
    });
    act(cx, window, |window, cx| window.press("secondary-a", cx));
    act(cx, window, |window, cx| window.input("500k", cx));
    // The reason goes as soon as what's typed reads, the saving waits.
    assert_eq!(cx.read(|cx| page.read(cx).errors())[0], None);
    assert_eq!(cx.read(|cx| prefs::get(cx).new_sessions.handover_at), None);
    rest(cx);
    assert_eq!(
        cx.read(|cx| prefs::get(cx).new_sessions.handover_at),
        Some(500_000)
    );
    assert_eq!(backend.defaults().handover_at, Some(500_000));

    act(cx, window, |window, cx| {
        window.click("background-after", cx)
    });
    act(cx, window, |window, cx| window.input("2h", cx));
    // Enter checks it at once.
    act(cx, window, |window, cx| window.press("enter", cx));
    assert!(cx.read(|cx| page.read(cx).errors())[2].is_some());
    act(cx, window, |window, cx| window.press("secondary-a", cx));
    act(cx, window, |window, cx| window.input("off", cx));
    rest(cx);
    assert_eq!(
        cx.read(|cx| prefs::get(cx).new_sessions.background_after),
        Some(0)
    );

    // Monitor wakes takes a day at most, auto, always, or off.
    act(cx, window, |window, cx| window.click("monitor-wakes", cx));
    act(cx, window, |window, cx| window.input("25h", cx));
    act(cx, window, |window, cx| window.press("enter", cx));
    assert!(cx.read(|cx| page.read(cx).errors())[3].is_some());
    act(cx, window, |window, cx| window.press("secondary-a", cx));
    act(cx, window, |window, cx| window.input("always", cx));
    rest(cx);
    assert_eq!(
        cx.read(|cx| prefs::get(cx).new_sessions.monitor_wakes.clone()),
        Some("always".into())
    );
    assert_eq!(
        backend.defaults().monitor_wakes,
        Some(arko::monitors::Wakes::Always)
    );

    act(cx, window, |window, cx| {
        window.click("chat-instructions", cx)
    });
    act(cx, window, |window, cx| window.input("Answer briefly.", cx));
    assert_eq!(
        cx.read(|cx| prefs::get(cx).new_sessions.chat_instructions.clone()),
        ""
    );
    // Another setting changing meanwhile leaves what's typed alone.
    cx.update(|cx| prefs::update(cx, |prefs| prefs.appearance.render_markdown = false));
    rest(cx);
    assert_eq!(
        cx.read(|cx| prefs::get(cx).new_sessions.chat_instructions.clone()),
        "Answer briefly."
    );
    // Leaving the field saves at once: the window's next frame tells the
    // field, while the window is active.
    act(cx, window, |window, _| window.activate_window());
    act(cx, window, |window, cx| window.input(" Always.", cx));
    act(cx, window, |window, cx| window.click("handover-at", cx));
    act(cx, window, |_, _| {});
    assert_eq!(
        cx.read(|cx| prefs::get(cx).new_sessions.chat_instructions.clone()),
        "Answer briefly. Always."
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

/// Sessions lists who chooses the workers' models, worker billing, the
/// daily API budget and how deep workers go in Arko's words, shows the
/// saved choice, and the choice made there is what the backend starts new
/// sessions with.
#[gpui_kit::test]
fn the_worker_settings_are_chosen_and_passed_on(cx: &mut TestAppContext) {
    use arko::WorkerBilling;

    let mut settings = prefs::Prefs::default();
    settings.new_sessions.worker_models = arko::ModelChoice::Profiles;
    let backend = start_with(cx, settings);
    let (window, _, _) = settings_window(cx, SESSIONS_PAGE, 900.);
    // The group's items: handover size, workers' handover size, models for
    // workers, worker billing, daily API budget, worker depth.
    let label = |cx: &mut TestAppContext, item: usize| {
        let mut label = None;
        act(cx, window, |window, _| {
            label = window
                .within("group-0")
                .within(format!("item-{item}"))
                .find("btn")
                .label()
                .map(str::to_string);
        });
        label
    };
    let budget_on = |cx: &mut TestAppContext| {
        let mut checked = None;
        act(cx, window, |window, _| {
            checked = window
                .within("group-0")
                .within("item-4")
                .find("check")
                .checked();
        });
        checked.unwrap()
    };
    let started_with = || {
        let mut options = arko::Options::default();
        backend.defaults().apply(&mut options, Some(Mode::Agent));
        (
            options.worker_models,
            options.worker_billing,
            options.api_budget,
            options.worker_depth,
        )
    };
    assert_eq!(label(cx, 2).as_deref(), Some("Profiles only"));
    // Billing shows Arko's default, and leaves it to Arko.
    assert_eq!(label(cx, 3).as_deref(), Some("Ask before API"));
    assert!(!budget_on(cx));
    assert_eq!(label(cx, 5).as_deref(), Some("2"));
    assert_eq!(
        started_with(),
        (Some(arko::ModelChoice::Profiles), None, None, Some(2))
    );
    // Choosing from the menus: Arko's labels, in its order.
    for (item, choice) in [(2, 2usize), (3, 0usize), (5, 3usize)] {
        act(cx, window, |window, cx| {
            window
                .within("group-0")
                .within(format!("item-{item}"))
                .click("btn", cx)
        });
        act(cx, window, |window, cx| {
            window.within("popup-menu").click(choice, cx)
        });
    }
    act(cx, window, |window, cx| {
        window.within("group-0").within("item-4").click("check", cx)
    });
    assert_eq!(
        cx.read(|cx| prefs::get(cx).new_sessions.worker_models),
        arko::ModelChoice::Session
    );
    assert_eq!(
        cx.read(|cx| prefs::get(cx).new_sessions.worker_billing),
        Some(WorkerBilling::Plans)
    );
    assert_eq!(
        cx.read(|cx| prefs::get(cx).new_sessions.api_budget),
        Some(true)
    );
    assert_eq!(cx.read(|cx| prefs::get(cx).new_sessions.worker_depth), 4);
    assert_eq!(label(cx, 2).as_deref(), Some("Session’s worker model"));
    assert_eq!(label(cx, 3).as_deref(), Some("Plans only"));
    assert!(budget_on(cx));
    assert_eq!(label(cx, 5).as_deref(), Some("4"));
    assert_eq!(
        started_with(),
        (
            Some(arko::ModelChoice::Session),
            Some(WorkerBilling::Plans),
            Some(Some(arko::workers::DEFAULT_BUDGET)),
            Some(4)
        )
    );
    // A choice made elsewhere shows at once.
    cx.update(|cx| {
        prefs::update(cx, |prefs| {
            prefs.new_sessions.worker_models = arko::ModelChoice::Agent;
            prefs.new_sessions.worker_billing = Some(WorkerBilling::Api);
            prefs.new_sessions.api_budget = Some(false);
            prefs.new_sessions.worker_depth = 1;
        })
    });
    act(cx, window, |_, _| {});
    assert_eq!(label(cx, 2).as_deref(), Some("Main agent chooses"));
    assert_eq!(label(cx, 3).as_deref(), Some("API allowed"));
    assert!(!budget_on(cx));
    assert_eq!(label(cx, 5).as_deref(), Some("1"));
}

/// The chat instructions type as Settings › Input says, as it changes
/// with Settings open.
#[gpui_kit::test]
fn the_chat_instructions_follow_the_input_settings(cx: &mut TestAppContext) {
    start_with(cx, prefs::Prefs::default());
    let (_, _, settings) = settings_window(cx, SESSIONS_PAGE, 900.);
    let page = cx.read(|cx| settings.read(cx).sessions().clone());
    let typing = |cx: &mut TestAppContext| cx.read(|cx| page.read(cx).instructions_input(cx));
    assert_eq!(
        typing(cx),
        (
            true,
            crate::composer::text_keymap(prefs::Keymap::Standard),
            true
        )
    );
    cx.update(|cx| {
        prefs::update(cx, |prefs| {
            prefs.input.smooth_typing = false;
            prefs.input.keymap = prefs::Keymap::Vim;
            prefs.input.spell_check = false;
        })
    });
    assert_eq!(
        typing(cx),
        (
            false,
            crate::composer::text_keymap(prefs::Keymap::Vim),
            false
        )
    );
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

/// A moment after the app starts, the fonts Settings lists are read in the
/// background, so the first opening of Settings shows its font pickers at
/// once, as the later ones do.
#[gpui_kit::test]
fn the_fonts_settings_lists_are_read_a_moment_after_the_start(cx: &mut TestAppContext) {
    start_with(cx, prefs::Prefs::default());
    let (window, workspace) = open(cx, 1300.);
    cx.update(crate::fonts::warm_up);
    cx.run_until_parked();
    assert!(
        cx.read(crate::fonts::picker_catalog).is_none(),
        "not while the app starts"
    );
    cx.executor().advance_clock(crate::fonts::WARM_UP_AFTER);
    cx.run_until_parked();
    assert!(cx.read(crate::fonts::picker_catalog).is_some());
    cx.update_window(window, |_, window, cx| {
        window.dispatch_action(Box::new(crate::menus::OpenSettings), cx)
    })
    .unwrap();
    let settings = cx.read(|cx| workspace.read(cx).settings().unwrap().clone());
    assert!(cx.read(|cx| settings.read(cx).font_pickers().is_some()));
}

/// Monospace only narrows the code font's list, and starts on. The
/// interface font has no use for it and doesn't offer it.
#[gpui_kit::test]
fn only_the_code_font_offers_monospace_only(cx: &mut TestAppContext) {
    start_with(cx, prefs::Prefs::default());
    let (window, _, settings) = settings_window(cx, APPEARANCE_PAGE, 1200.);
    assert!(cx.read(|cx| settings.read(cx).font_pickers().is_some()));
    act(cx, window, |window, _| {
        // A second checkbox, the interface font's, would make the ID
        // ambiguous. This one is on, as only the code font's starts.
        let monospace = window.find("font-picker-monospace");
        assert_eq!(monospace.checked(), Some(true));
    });
}

/// "Needs an OpenAI key" in the composer's model list opens Models &
/// accounts on the OpenAI key's row, with the keyboard there.
#[gpui_kit::test]
fn a_model_that_needs_a_key_opens_its_row(cx: &mut TestAppContext) {
    start_with(cx, prefs::Prefs::default());
    let (window, workspace) = open(cx, 1300.);
    let view = cx.read(|cx| workspace.read(cx).session_view().clone());
    cx.update(|cx| {
        view.update(cx, |_, cx| {
            cx.emit(crate::session_view::SessionViewEvent::OpenAccounts(Some(
                "OpenAI, with your API key".into(),
            )))
        })
    });
    act(cx, window, |_, _| {});
    let settings = cx.read(|cx| workspace.read(cx).settings().unwrap().clone());
    let (page, target, focus) = cx.read(|cx| {
        let accounts = settings.read(cx).accounts().read(cx);
        (
            settings.read(cx).page(),
            accounts.target(),
            accounts.focus_of("openai").unwrap(),
        )
    });
    assert_eq!(page, 2);
    assert_eq!(target, Some("openai"));
    act(cx, window, |window, _| {
        assert!(focus.is_focused(window));
        assert!(window.try_find("account-openai").is_some());
    });
    // Enter on the row does what its button does: the key's dialog.
    act(cx, window, |window, cx| window.press("enter", cx));
    act(cx, window, |window, cx| {
        assert!(window.has_active_dialog(cx));
        assert!(window.try_find("key-form").is_some());
    });
    assert_eq!(
        super::accounts::account_for("Claude Code, on your Claude plan"),
        Some("claude-code")
    );
    assert_eq!(
        super::accounts::account_for("OpenAI, on your ChatGPT plan"),
        Some("chatgpt")
    );
    assert_eq!(
        super::accounts::account_for("Google Gemini, billed per token"),
        Some("gemini")
    );
    assert_eq!(super::accounts::account_for("Somewhere else"), None);
}

/// The web page's address and port are checked as typed, and every
/// session serves its page there from the start once that's on.
#[gpui_kit::test]
fn the_web_page_address_and_port_are_checked_and_used(cx: &mut TestAppContext) {
    let backend = start_with(cx, prefs::Prefs::default());
    let (window, _, settings) = settings_window(cx, WEB_PAGE_PAGE, 2400.);
    let page = cx.read(|cx| settings.read(cx).web_page().clone());
    act(cx, window, |window, cx| {
        window.click("web-page-address", cx)
    });
    // On the way to 0.0.0.0, "0." is no error yet.
    act(cx, window, |window, cx| window.input("0.", cx));
    assert_eq!(cx.read(|cx| page.read(cx).errors())[0], None);
    act(cx, window, |window, cx| window.press("secondary-a", cx));
    act(cx, window, |window, cx| window.input("my-laptop", cx));
    rest(cx);
    assert!(cx.read(|cx| page.read(cx).errors())[0].is_some());
    act(cx, window, |window, cx| window.press("secondary-a", cx));
    act(cx, window, |window, cx| window.input("127.0.0.1", cx));
    assert_eq!(cx.read(|cx| page.read(cx).errors())[0], None);
    act(cx, window, |window, cx| window.click("web-page-port", cx));
    act(cx, window, |window, cx| window.input("70000", cx));
    rest(cx);
    assert!(cx.read(|cx| page.read(cx).errors())[1].is_some());
    act(cx, window, |window, cx| window.press("secondary-a", cx));
    // 9, 90 and 900 aren't saved on the way to 9000.
    for key in ["9", "0", "0", "0"] {
        act(cx, window, |window, cx| window.input(key, cx));
        assert_eq!(cx.read(|cx| prefs::get(cx).web_page.port), None);
    }
    rest(cx);
    cx.read(|cx| {
        let web = &prefs::get(cx).web_page;
        assert_eq!(web.address.as_deref(), Some("127.0.0.1"));
        assert_eq!(web.port, Some(9000));
        assert_eq!(web.bind_argument(), "127.0.0.1:9000");
    });
    assert_eq!(backend.defaults().web_page, None, "not shared by default");
    cx.update(|cx| prefs::update(cx, |prefs| prefs.web_page.share_by_default = true));
    let bind = backend.defaults().web_page.unwrap();
    assert_eq!(bind.ip.to_string(), "127.0.0.1");
    assert_eq!(bind.port, Some(9000));
}

/// An open session's changes that don't start or stop its web page (a
/// reply streaming in) leave Settings as it is drawn.
#[gpui_kit::test]
fn a_sessions_updates_leave_settings_as_drawn(cx: &mut TestAppContext) {
    let backend = start_with(cx, prefs::Prefs::default());
    let (window, _, settings) = settings_window(cx, WEB_PAGE_PAGE, 900.);
    let entry = backend.sessions().unwrap().remove(0);
    let session = cx.update(|cx| crate::live::LiveSession::resume(&entry, cx));
    let builds = |cx: &mut TestAppContext| cx.read(|cx| settings.read(cx).builds);
    let frame = |cx: &mut TestAppContext| {
        cx.run_until_parked();
        cx.update_window(window, |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
    };
    frame(cx);
    let before = builds(cx);
    for _ in 0..5 {
        cx.update(|cx| session.update(cx, |_, cx| cx.notify()));
        frame(cx);
    }
    assert_eq!(builds(cx), before);
}

/// A project's remembered answer about its MCP servers can be forgotten,
/// so its next session asks again.
#[gpui_kit::test]
fn a_projects_mcp_answer_is_forgotten(cx: &mut TestAppContext) {
    use crate::backend::extensions::Approval;
    start_with(cx, prefs::Prefs::default());
    let (window, _, settings) = settings_window(cx, EXTENSIONS_PAGE, 2400.);
    let page = cx.read(|cx| settings.read(cx).extensions().clone());
    let approval = |cx: &mut TestAppContext| {
        cx.read(|cx| page.read(cx).extensions().unwrap().projects[0].approval)
    };
    assert_eq!(approval(cx), Approval::Approved);
    act(cx, window, |window, cx| {
        window.click(("forget-approval", 0usize), cx)
    });
    assert_eq!(approval(cx), Approval::NotAsked);
    act(cx, window, |window, cx| {
        assert_eq!(window.notifications(cx).len(), 1);
        assert!(window.try_find(("forget-approval", 0usize)).is_none());
    });
}

/// The switches for Arko's built-in skills, all of them and each by name,
/// reach the options of the sessions opened after, and switching back on
/// is passed on too.
#[gpui_kit::test]
fn the_built_in_skill_switches_reach_the_sessions_options(cx: &mut TestAppContext) {
    let backend = start_with(cx, prefs::Prefs::default());
    let (window, _, _) = settings_window(cx, EXTENSIONS_PAGE, 2400.);
    // Left to Arko until chosen.
    assert_eq!(backend.defaults().builtin_skills, None);
    assert_eq!(backend.defaults().builtin_skills_off, None);
    let on = |cx: &mut TestAppContext, id: &'static str| {
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            window.try_find(id).and_then(|found| found.checked())
        })
        .unwrap()
    };
    assert_eq!(on(cx, "builtin-skills"), Some(true));
    assert_eq!(on(cx, "builtin-skill-user-facing-prose"), Some(true));
    act(cx, window, |window, cx| {
        window.click("builtin-skill-repo-docs-style", cx)
    });
    assert_eq!(
        backend.defaults().builtin_skills_off,
        Some(vec!["repo-docs-style".to_string()])
    );
    assert_eq!(backend.defaults().builtin_skills, None);
    assert_eq!(on(cx, "builtin-skill-repo-docs-style"), Some(false));
    assert_eq!(on(cx, "builtin-skill-user-facing-prose"), Some(true));
    act(cx, window, |window, cx| window.click("builtin-skills", cx));
    assert_eq!(backend.defaults().builtin_skills, Some(false));
    assert_eq!(on(cx, "builtin-skills"), Some(false));
    // Switched back on, each is passed on, so a resumed session that kept
    // them off gets them again.
    act(cx, window, |window, cx| window.click("builtin-skills", cx));
    act(cx, window, |window, cx| {
        window.click("builtin-skill-repo-docs-style", cx)
    });
    assert_eq!(backend.defaults().builtin_skills, Some(true));
    assert_eq!(backend.defaults().builtin_skills_off, Some(Vec::new()));
    assert_eq!(on(cx, "builtin-skill-repo-docs-style"), Some(true));
}

/// MCP config files added under Advanced reach every session's options,
/// and Remove takes one out. Extensions lists what's there as it changes.
#[gpui_kit::test]
fn mcp_config_files_are_added_and_removed(cx: &mut TestAppContext) {
    let backend = start_with(cx, prefs::Prefs::default());
    let (window, _, settings) = settings_window(cx, ADVANCED_PAGE, 2400.);
    let extensions = cx.read(|cx| settings.read(cx).extensions().clone());
    let listed = |cx: &mut TestAppContext| {
        cx.read(|cx| {
            let found = extensions.read(cx).extensions().unwrap();
            found.files.iter().filter(|file| !file.arko).count()
        })
    };
    let file = std::path::PathBuf::from("/home/ada/mcp/work.json");
    cx.update(|cx| {
        settings.update(cx, |settings, cx| {
            settings.add_mcp_configs(vec![file.clone(), file.clone()], cx)
        })
    });
    assert_eq!(
        backend.defaults().mcp_config,
        std::slice::from_ref(&file),
        "once"
    );
    act(cx, window, |_, _| {});
    assert_eq!(listed(cx), 1);
    act(cx, window, |window, cx| {
        window.click(("remove-mcp-config", 0usize), cx)
    });
    assert!(backend.defaults().mcp_config.is_empty());
    assert!(cx.read(|cx| prefs::get(cx).advanced.mcp_config.is_empty()));
    assert_eq!(listed(cx), 0);
}

/// Add… under Advanced takes MCP config files only: another file is
/// refused with why, and the others are added.
#[gpui_kit::test]
fn only_mcp_config_files_are_added(cx: &mut TestAppContext) {
    let backend = start_with(cx, prefs::Prefs::default());
    let (window, _, settings) = settings_window(cx, ADVANCED_PAGE, 2400.);
    let temp = tempfile::tempdir().unwrap();
    let good = temp.path().join("servers.json");
    let notes = temp.path().join("notes.txt");
    std::fs::write(
        &good,
        r#"{"mcpServers": {"files": {"command": "mcp-files"}}}"#,
    )
    .unwrap();
    std::fs::write(&notes, "Buy pears.").unwrap();
    act(cx, window, |window, cx| {
        settings.update(cx, |settings, cx| {
            settings.add_checked_mcp_configs(vec![notes.clone(), good.clone()], window, cx)
        })
    });
    assert_eq!(backend.defaults().mcp_config, [good]);
    act(cx, window, |window, cx| {
        assert_eq!(window.notifications(cx).len(), 1, "says why not");
    });
}

/// The MCP config files are removed one by one, with Remove. The
/// Advanced page's reset leaves the list, which would take a file dialog
/// per file to make again.
#[gpui_kit::test]
fn advanced_resets_keep_the_mcp_config_files(cx: &mut TestAppContext) {
    let mut settings = prefs::Prefs::default();
    settings.advanced.mcp_config = vec![std::path::PathBuf::from("/home/ada/mcp/work.json")];
    start_with(cx, settings);
    let (window, _, _) = settings_window(cx, ADVANCED_PAGE, 900.);
    act(cx, window, |window, _| {
        assert!(window.try_find(("remove-mcp-config", 0usize)).is_some());
        assert!(window.try_find("reset").is_none(), "nothing else to reset");
    });
}

/// Moving or resizing the window changes its placement in the preferences
/// many times a second: the pages aren't built again for it. A preference
/// they show still shows at once.
#[gpui_kit::test]
fn a_resize_leaves_the_pages_as_they_are(cx: &mut TestAppContext) {
    start_with(cx, prefs::Prefs::default());
    let (window, _, settings) = settings_window(cx, APPEARANCE_PAGE, 900.);
    let builds = |cx: &mut TestAppContext| cx.read(|cx| settings.read(cx).builds);
    // A frame as the platform asks for one: only what changed draws again.
    let frame = |cx: &mut TestAppContext| {
        cx.run_until_parked();
        cx.update_window(window, |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
    };
    frame(cx);
    let before = builds(cx);
    for step in 0..5 {
        cx.update(|cx| {
            prefs::update(cx, |prefs| {
                prefs.window = Some(prefs::WindowPlacement {
                    x: 10. * step as f32,
                    y: 0.,
                    width: 1300. + step as f32,
                    height: 900.,
                    maximized: false,
                });
            })
        });
        frame(cx);
    }
    assert_eq!(builds(cx), before);
    cx.update(|cx| prefs::update(cx, |prefs| prefs.appearance.render_markdown = false));
    frame(cx);
    assert!(builds(cx) > before);
}

/// The choices on Input, Models & accounts and Sessions stay inside their
/// rows from 460 to 1300 pixels wide: beside their titles where they fit,
/// under them where they don't, and across a narrow page.
#[gpui_kit::test]
fn the_choices_stay_inside_their_rows(cx: &mut TestAppContext) {
    use gpui_kit::{ElementId, Window, base::test_support::snapshots};
    start_with(cx, prefs::Prefs::default());
    let (window, _, settings) = settings_window(cx, INPUT_PAGE, 1200.);
    let show = |cx: &mut TestAppContext, page: usize| {
        cx.update(|cx| settings.update(cx, |settings, cx| settings.show_page(page, cx)));
        act(cx, window, |_, _| {});
    };
    // The dropdowns' buttons on the page shown, as their right edges:
    // the kit's and OpenRouter's menus.
    let menus = [
        "btn",
        "openrouter-region",
        "openrouter-sort",
        "openrouter-precision",
    ]
    .map(|id| ElementId::Name(id.into()));
    let choices = |window: &Window| {
        snapshots(window)
            .into_iter()
            .filter(|element| {
                element
                    .path()
                    .last()
                    .is_some_and(|last| menus.contains(last))
                    && element
                        .path()
                        .contains(&ElementId::Name("settings-page".into()))
            })
            .map(|element| element.bounds().right())
            .collect::<Vec<_>>()
    };
    // How many choices each page had in view, over all the widths: a
    // narrow page can push them below the window.
    let mut checked = [0; 3];
    for width in (460..=1300).step_by(40) {
        cx.simulate_window_resize(window, size(px(width as f32), px(1200.)));
        show(cx, INPUT_PAGE);
        // The paste number field ends where the rows end, in either layout.
        let mut inset = px(0.);
        act(cx, window, |window, _| {
            let page = window.find("settings-page").bounds();
            let edge = window.within("group-2").find("increment").bounds().right();
            inset = page.right() - edge;
        });
        for (ix, page) in [INPUT_PAGE, ACCOUNTS_PAGE, SESSIONS_PAGE]
            .into_iter()
            .enumerate()
        {
            show(cx, page);
            act(cx, window, |window, _| {
                let edge = window.find("settings-page").bounds().right() - inset;
                let choices = choices(window);
                checked[ix] += choices.len();
                for right in choices {
                    assert!(
                        right <= edge + px(1.),
                        "a choice on page {page} at {width} px ends at {right:?}, past {edge:?}"
                    );
                }
            });
        }
    }
    assert!(checked.iter().all(|&count| count > 20), "{checked:?}");
}

/// A preference changed outside Settings shows at once in its group, which
/// otherwise draws again only when what it shows changes.
#[gpui_kit::test]
fn a_group_shows_a_preference_changed_elsewhere(cx: &mut TestAppContext) {
    start_with(cx, prefs::Prefs::default());
    // Notifications: When a turn ends, When a question arrives, When a
    // worker reports or fails, Cache reminders, Sound.
    let (window, _, _) = settings_window(cx, NOTIFICATIONS_PAGE, 900.);
    let sound = |cx: &mut TestAppContext| {
        let mut checked = None;
        act(cx, window, |window, _| {
            checked = window.within("item-4").find("check").checked();
        });
        checked.unwrap()
    };
    let on = sound(cx);
    for _ in 0..2 {
        let now = sound(cx);
        cx.update(|cx| prefs::update(cx, |prefs| prefs.notifications.sound = !now));
        assert_eq!(sound(cx), !now);
    }
    assert_eq!(sound(cx), on);
}

/// The models of the model settings' tests: the sample's Anthropic models,
/// and Kimi K3 on OpenRouter.
pub(super) fn openrouter_models() -> ModelChoices {
    let choice = |name: &str, alias: &str| ModelChoice {
        name: name.into(),
        alias: alias.into(),
    };
    ModelChoices {
        groups: vec![
            ModelGroup {
                name: "Anthropic API, billed per token".into(),
                needs: None,
                models: vec![
                    choice("claude-opus-5-5", "opus"),
                    choice("claude-sonnet-5-5", "sonnet"),
                ],
            },
            ModelGroup {
                name: "OpenRouter, billed per token".into(),
                needs: None,
                models: vec![choice("openrouter:moonshotai/kimi-k3", "")],
            },
        ],
        default: Some("claude-opus-5-5".into()),
        note: None,
    }
}

/// Model and Default workers' model are found by typing, and a model
/// none of the groups lists is chosen in OpenRouter's catalog, which opens
/// on what was typed.
#[gpui_kit::test]
fn the_model_settings_find_a_model_and_browse_openrouter(cx: &mut TestAppContext) {
    let backend = FakeBackend::sample("/arko", SystemTime::now())
        .with_models(openrouter_models())
        .with_catalog(crate::backend::sample_catalog(SystemTime::now()));
    let backend = start_on(cx, backend, prefs::Prefs::default());
    let (window, _, _) = settings_window(cx, ACCOUNTS_PAGE, 2800.);
    let pick = super::profiles_tests::pick_model;
    pick(cx, window, "new-sessions-model-button", "sonnet");
    assert_eq!(
        cx.read(|cx| prefs::get(cx).new_sessions.model.clone())
            .as_deref(),
        Some("claude-sonnet-5-5")
    );
    act(cx, window, |window, _| {
        assert_eq!(
            window.find("new-sessions-model-button").label(),
            Some("Model, Sonnet 5.5 · Anthropic API")
        );
    });
    // GLM 5 isn't listed: Enter takes the one row left, which opens the
    // catalog on "glm", and Enter there picks it.
    pick(cx, window, "new-sessions-worker-model-button", "glm");
    act(cx, window, |window, cx| {
        assert!(window.has_active_dialog(cx));
        assert!(window.try_find("openrouter-browser").is_some());
        window.press("enter", cx);
    });
    act(cx, window, |window, cx| {
        assert!(!window.has_active_dialog(cx));
    });
    assert_eq!(
        cx.read(|cx| prefs::get(cx).new_sessions.worker_model.clone())
            .as_deref(),
        Some("openrouter:z-ai/glm-5")
    );
    assert_eq!(backend.catalog_fetches(), 0, "the sample was fresh");
    act(cx, window, |window, _| {
        assert_eq!(
            window.find("new-sessions-worker-model-button").label(),
            Some("Default workers’ model, GLM 5 · OpenRouter")
        );
    });
}

/// OpenRouter's routing settings are saved through the backend as they're
/// changed, zero data retention turning "No data collection" on for good,
/// and Refresh fetches the models again.
#[gpui_kit::test]
fn the_openrouter_settings_are_saved_through_the_backend(cx: &mut TestAppContext) {
    use arko::openrouter::{Precision, Region, Sort};
    let backend = FakeBackend::sample("/arko", SystemTime::now())
        .with_catalog(crate::backend::sample_catalog(SystemTime::now()));
    let backend = start_on(cx, backend, prefs::Prefs::default());
    let (window, _, settings) = settings_window(cx, ACCOUNTS_PAGE, 2400.);
    let part = cx.read(|cx| settings.read(cx).accounts().read(cx).openrouter().clone());
    let words = |cx: &mut TestAppContext| cx.read(|cx| part.read(cx).models_words());
    assert_eq!(words(cx), "6 with tool use, updated 1 minute ago");

    act(cx, window, |window, cx| window.click("openrouter-zdr", cx));
    assert!(backend.openrouter().zdr());
    assert!(cx.read(|cx| part.read(cx).settings().unwrap().data_collection_implied()));
    // Region, Providers by and Precision, from their menus: each choice
    // in arko's order.
    for (menu, choice) in [
        ("openrouter-region", 3usize),
        ("openrouter-sort", 2),
        ("openrouter-precision", 2),
    ] {
        act(cx, window, |window, cx| window.click(menu, cx));
        act(cx, window, |window, cx| {
            window.within("popup-menu").click(choice, cx)
        });
    }
    let saved = backend.openrouter();
    assert_eq!(saved.region(), Region::UsOrEu);
    assert_eq!(saved.sort(), Sort::Throughput);
    assert_eq!(saved.precision(), Precision::Bf16);
    assert!(saved.zdr(), "the changes before stay");

    // The provider lists are saved on Enter, as Arko reads them.
    let (only, _) = cx.read(|cx| part.read(cx).inputs());
    act(cx, window, |window, cx| {
        only.update(cx, |only, cx| only.focus(window, cx))
    });
    act(cx, window, |window, cx| {
        window.input("Anthropic, google-vertex", cx)
    });
    act(cx, window, |window, cx| window.press("enter", cx));
    assert_eq!(backend.openrouter().only(), ["anthropic", "google-vertex"]);

    act(cx, window, |window, cx| {
        window.click("openrouter-refresh", cx)
    });
    assert_eq!(backend.catalog_fetches(), 1);
    assert_eq!(words(cx), "6 with tool use, updated just now");
}

/// The Settings page's OpenRouter part, in `window`.
fn openrouter_part(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
) -> Entity<super::openrouter::OpenRouterPart> {
    let part = cx
        .update_window(window, |_, window, cx| {
            cx.new(|cx| super::openrouter::OpenRouterPart::new(window, cx))
        })
        .unwrap();
    cx.run_until_parked();
    part
}

/// Two windows' OpenRouter settings, each read before the other's change,
/// each change one control: both changes are kept, zero data retention
/// included, and each window shows them. So does a change made elsewhere,
/// which the next change here keeps.
#[gpui_kit::test]
fn openrouter_changes_from_two_windows_keep_each_other(cx: &mut TestAppContext) {
    use arko::openrouter::{Region, Sort};
    let backend = start_on(
        cx,
        FakeBackend::sample("/arko", SystemTime::now()),
        prefs::Prefs::default(),
    );
    let (window, _) = open(cx, 1300.);
    let (first, second) = (openrouter_part(cx, window), openrouter_part(cx, window));
    cx.update(|cx| {
        first.update(cx, |part, cx| {
            part.change(|settings| settings.set_zdr(true), cx)
        });
        second.update(cx, |part, cx| {
            part.change(|settings| settings.set_region(Region::Us), cx)
        });
    });
    cx.run_until_parked();
    let saved = backend.openrouter();
    assert!(saved.zdr());
    assert_eq!(saved.region(), Region::Us);
    for part in [&first, &second] {
        assert_eq!(
            cx.read(|cx| part.read(cx).settings().cloned()),
            Some(saved.clone())
        );
    }
    // The terminal app sets the providers' order; a region chosen here
    // keeps it.
    backend.change_openrouter_elsewhere(|settings| settings.set_sort(Sort::Latency));
    cx.update(|cx| {
        first.update(cx, |part, cx| {
            part.change(|settings| settings.set_region(Region::Eu), cx)
        })
    });
    cx.run_until_parked();
    let saved = backend.openrouter();
    assert_eq!((saved.sort(), saved.region()), (Sort::Latency, Region::Eu));
    assert!(saved.zdr());
}

/// A save that fails says why, and what shows is what's saved: the
/// settings as they were when nothing else waits, else what the later
/// change saved.
#[gpui_kit::test]
fn a_failed_openrouter_save_shows_what_is_saved(cx: &mut TestAppContext) {
    use arko::openrouter::{Region, Sort};
    let backend = start_on(
        cx,
        FakeBackend::sample("/arko", SystemTime::now()),
        prefs::Prefs::default(),
    );
    let (window, _) = open(cx, 1300.);
    let part = openrouter_part(cx, window);
    let shown = |cx: &mut TestAppContext| cx.read(|cx| part.read(cx).settings().cloned().unwrap());
    let problem = |cx: &mut TestAppContext| cx.read(|cx| part.read(cx).problem().is_some());
    // Alone, it goes back to the settings saved.
    backend.fail_openrouter_saves(1);
    cx.update(|cx| part.update(cx, |part, cx| part.change(|s| s.set_zdr(true), cx)));
    cx.run_until_parked();
    assert!(!shown(cx).zdr());
    assert!(!backend.openrouter().zdr());
    assert!(problem(cx));
    // With a change after it waiting, that one's save shows.
    backend.fail_openrouter_saves(1);
    cx.update(|cx| {
        part.update(cx, |part, cx| {
            part.change(|s| s.set_region(Region::Eu), cx);
            part.change(|s| s.set_sort(Sort::Price), cx);
        })
    });
    cx.run_until_parked();
    let saved = backend.openrouter();
    assert_eq!((saved.region(), saved.sort()), (Region::Any, Sort::Price));
    assert_eq!(shown(cx), saved);
}

/// The model lists' rows and OpenRouter's controls have names a screen
/// reader says: each row its model, way and state, each control its field
/// and value.
#[gpui_kit::test]
fn the_model_rows_and_openrouter_controls_have_names(cx: &mut TestAppContext) {
    use gpui_kit::base::test_support::snapshots;
    let backend = FakeBackend::sample("/arko", SystemTime::now()).with_models(openrouter_models());
    start_on(cx, backend, prefs::Prefs::default());
    let (window, _, _) = settings_window(cx, ACCOUNTS_PAGE, 2800.);
    act(cx, window, |window, _| {
        assert_eq!(
            window.find("openrouter-zdr").label(),
            Some("Zero data retention")
        );
        assert_eq!(
            window.find("openrouter-data-collection").label(),
            Some("No data collection")
        );
        assert_eq!(
            window.find("openrouter-region").label(),
            Some("Region, Any")
        );
        assert_eq!(
            window.find("openrouter-sort").label(),
            Some("Providers by, Default")
        );
        assert_eq!(
            window.find("openrouter-precision").label(),
            Some("Precision, Any")
        );
        for (field, name) in [
            ("openrouter-only", "Only these providers"),
            ("openrouter-ignore", "Never these providers"),
        ] {
            let field = gpui_kit::ElementId::Name(field.into());
            assert!(
                snapshots(window)
                    .iter()
                    .any(|element| element.path().contains(&field) && element.label() == Some(name)),
                "{name}"
            );
        }
    });
    act(cx, window, |window, cx| {
        window.click("new-sessions-model-button", cx)
    });
    // A model billed per token is heard with its billing, as Arko says it.
    let billed = |row: &str, name: &str| match openrouter_models().billing(name) {
        Some(billing) => format!("{row}, {billing}"),
        None => row.to_string(),
    };
    let opus = billed("Opus 5.5, Anthropic API", "claude-opus-5-5");
    let kimi = billed("Kimi K3, OpenRouter", "openrouter:moonshotai/kimi-k3");
    assert!(opus.starts_with("Opus 5.5, Anthropic API, API, $"));
    act(cx, window, |window, _| {
        let list = window.within("new-sessions-model");
        assert_eq!(list.find("model-row-0-0").label(), Some("Last used"));
        assert_eq!(list.find("model-row-1-0").label(), Some(opus.as_str()));
        assert_eq!(list.find("model-row-2-0").label(), Some(kimi.as_str()));
        assert_eq!(
            list.find("model-row-2-1").label(),
            Some("All OpenRouter models")
        );
    });
}
