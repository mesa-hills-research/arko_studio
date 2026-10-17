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
fn start_on(
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
    let (window, _, _) = settings_window(cx, 2, 1400.);
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

/// Enter in the key's field saves the key, as Save does. With nothing
/// pasted, the dialog stays open and says so.
#[gpui_kit::test]
fn enter_in_the_key_field_saves_the_key(cx: &mut TestAppContext) {
    let backend = start_with(cx, prefs::Prefs::default());
    let (window, _, _) = settings_window(cx, 2, 1400.);
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

/// Sizes and times are saved as soon as they read as arko reads them.
/// Until then the field says why, and the setting stays. A reset empties
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
    let (window, _, settings) = settings_window(cx, 5, 2400.);
    let page = cx.read(|cx| settings.read(cx).web_page().clone());
    act(cx, window, |window, cx| {
        window.click("web-page-address", cx)
    });
    act(cx, window, |window, cx| window.input("my-laptop", cx));
    assert!(cx.read(|cx| page.read(cx).errors())[0].is_some());
    act(cx, window, |window, cx| window.press("secondary-a", cx));
    act(cx, window, |window, cx| window.input("0.0.0.0", cx));
    assert_eq!(cx.read(|cx| page.read(cx).errors())[0], None);
    act(cx, window, |window, cx| window.click("web-page-port", cx));
    act(cx, window, |window, cx| window.input("70000", cx));
    assert!(cx.read(|cx| page.read(cx).errors())[1].is_some());
    act(cx, window, |window, cx| window.press("secondary-a", cx));
    act(cx, window, |window, cx| window.input("9000", cx));
    cx.read(|cx| {
        let web = &prefs::get(cx).web_page;
        assert_eq!(web.address, "0.0.0.0");
        assert_eq!(web.port, Some(9000));
        assert_eq!(web.bind_argument(), "0.0.0.0:9000");
    });
    assert_eq!(backend.defaults().web_page, None, "not shared by default");
    cx.update(|cx| prefs::update(cx, |prefs| prefs.web_page.share_by_default = true));
    let bind = backend.defaults().web_page.unwrap();
    assert_eq!(bind.ip.to_string(), "0.0.0.0");
    assert_eq!(bind.port, Some(9000));
}

/// A project's remembered answer about its MCP servers can be forgotten,
/// so its next session asks again.
#[gpui_kit::test]
fn a_projects_mcp_answer_is_forgotten(cx: &mut TestAppContext) {
    use crate::backend::extensions::Approval;
    start_with(cx, prefs::Prefs::default());
    let (window, _, settings) = settings_window(cx, 6, 2400.);
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

/// MCP config files added under Advanced reach every session's options,
/// and Remove takes one out. Extensions lists what's there as it changes.
#[gpui_kit::test]
fn mcp_config_files_are_added_and_removed(cx: &mut TestAppContext) {
    let backend = start_with(cx, prefs::Prefs::default());
    let (window, _, settings) = settings_window(cx, 7, 2400.);
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
    let (window, _, settings) = settings_window(cx, 7, 2400.);
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

/// Moving or resizing the window changes its placement in the preferences
/// many times a second: the pages aren't built again for it. A preference
/// they show still shows at once.
#[gpui_kit::test]
fn a_resize_leaves_the_pages_as_they_are(cx: &mut TestAppContext) {
    start_with(cx, prefs::Prefs::default());
    let (window, _, settings) = settings_window(cx, 0, 900.);
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
