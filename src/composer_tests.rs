//! Interaction tests of the composer and the session's dialogs, on the fake
//! backend's sample sessions (which show, but don't run).

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::SystemTime;

use gpui_kit::component::WindowExt as _;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AnyWindowHandle, AppContext as _, ClipboardItem, Entity, TestAppContext};

use crate::backend::FakeBackend;
use crate::composer::ComposerEvent;
use crate::live::{PromptAnswer, PromptKind};
use crate::prefs::{self, SendKey};
use crate::tests::{act, open, open_title, row_of};
use crate::workspace::Workspace;
use crate::{Startup, drafts};

fn start(cx: &mut TestAppContext) -> (Arc<FakeBackend>, AnyWindowHandle, Entity<Workspace>) {
    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    cx.update(|cx| crate::init(Startup::for_tests(backend.clone()), cx));
    let (window, workspace) = open(cx, 1300.);
    (backend, window, workspace)
}

fn composer_text(workspace: &Entity<Workspace>, cx: &mut TestAppContext) -> String {
    cx.read(|cx| {
        workspace
            .read(cx)
            .session_view()
            .read(cx)
            .composer()
            .read(cx)
            .text(cx)
    })
}

fn open_row(
    window: AnyWindowHandle,
    workspace: &Entity<Workspace>,
    title: &str,
    cx: &mut TestAppContext,
) {
    let row = row_of(workspace, title, cx);
    act(cx, window, |window, cx| window.click(("row", row), cx));
}

#[gpui_kit::test]
fn each_session_keeps_its_draft(cx: &mut TestAppContext) {
    let (_, window, workspace) = start(cx);
    act(cx, window, |window, cx| {
        window.input("For the new session", cx)
    });
    open_row(window, &workspace, "Split the engine out of main.rs", cx);
    assert_eq!(composer_text(&workspace, cx), "");
    act(cx, window, |window, cx| window.input("About the split", cx));
    open_row(window, &workspace, "Naming a desktop app", cx);
    assert_eq!(composer_text(&workspace, cx), "");
    open_row(window, &workspace, "Split the engine out of main.rs", cx);
    assert_eq!(composer_text(&workspace, cx), "About the split");
    act(cx, window, |window, cx| window.press("secondary-n", cx));
    assert_eq!(composer_text(&workspace, cx), "For the new session");
    // They're kept for the next run too.
    assert_eq!(
        cx.read(|cx| drafts::get("20261016-091200-a1b2", cx))
            .as_deref(),
        Some("About the split")
    );
}

#[gpui_kit::test]
fn closing_the_window_keeps_what_was_just_typed(cx: &mut TestAppContext) {
    let (_, window, _) = start(cx);
    // Closed before typing rests, when the draft would be kept anyway.
    act(cx, window, |window, cx| window.input("Half a thought", cx));
    cx.update_window(window, |_, window, _| window.remove_window())
        .unwrap();
    cx.run_until_parked();
    assert_eq!(
        cx.read(|cx| drafts::get(drafts::NEW_SESSION, cx))
            .as_deref(),
        Some("Half a thought")
    );
}

#[gpui_kit::test]
fn a_saved_draft_comes_back_at_start(cx: &mut TestAppContext) {
    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    let drafts = BTreeMap::from([(
        drafts::NEW_SESSION.to_string(),
        "Half a thought".to_string(),
    )]);
    cx.update(|cx| crate::init(Startup::for_tests(backend).with_drafts(drafts), cx));
    let (_, workspace) = open(cx, 1300.);
    assert_eq!(composer_text(&workspace, cx), "Half a thought");
}

#[gpui_kit::test]
fn up_and_down_walk_the_history_at_the_edges(cx: &mut TestAppContext) {
    let (_, window, workspace) = start(cx);
    cx.update(|cx| {
        crate::history::add("First message", cx);
        crate::history::add("Second message", cx);
    });
    act(cx, window, |window, cx| window.input("Draft", cx));
    act(cx, window, |window, cx| window.press("up", cx));
    assert_eq!(composer_text(&workspace, cx), "Second message");
    act(cx, window, |window, cx| window.press("up", cx));
    assert_eq!(composer_text(&workspace, cx), "First message");
    act(cx, window, |window, cx| window.press("up", cx));
    assert_eq!(
        composer_text(&workspace, cx),
        "First message",
        "stops at the oldest"
    );
    act(cx, window, |window, cx| window.press("down", cx));
    assert_eq!(composer_text(&workspace, cx), "Second message");
    act(cx, window, |window, cx| window.press("down", cx));
    assert_eq!(composer_text(&workspace, cx), "Draft", "back to the draft");
}

/// ↑ and ↓ walk the input history from the empty composer with every
/// keymap, in Vim's normal and insert modes alike.
#[gpui_kit::test]
fn empty_history_arrows_work_in_every_keymap_and_both_vim_modes(cx: &mut TestAppContext) {
    use crate::prefs::Keymap as InputKeymap;
    let (_, window, workspace) = start(cx);
    cx.update(|cx| {
        crate::history::add("First message", cx);
        crate::history::add("Second message", cx);
    });
    for (keymap, insert) in [
        (InputKeymap::Standard, false),
        (InputKeymap::Emacs, false),
        (InputKeymap::Vim, false),
        (InputKeymap::Vim, true),
    ] {
        cx.update(|cx| prefs::update(cx, |prefs| prefs.input.keymap = keymap));
        act(cx, window, |window, cx| {
            if keymap == InputKeymap::Vim {
                window.press("escape", cx);
                if insert {
                    window.press("i", cx);
                }
            }
        });
        act(cx, window, |window, cx| window.press("up", cx));
        assert_eq!(
            composer_text(&workspace, cx),
            "Second message",
            "{keymap:?} insert {insert}"
        );
        act(cx, window, |window, cx| window.press("up", cx));
        assert_eq!(composer_text(&workspace, cx), "First message");
        act(cx, window, |window, cx| window.press("down", cx));
        act(cx, window, |window, cx| window.press("down", cx));
        assert_eq!(composer_text(&workspace, cx), "");
    }
    // After a count, Vim's ↑ stays a motion.
    act(cx, window, |window, cx| {
        window.press("escape", cx);
        window.press("2", cx);
        window.press("up", cx);
    });
    assert_eq!(composer_text(&workspace, cx), "");
}

#[gpui_kit::test]
fn a_long_paste_becomes_a_token_that_sends_whole(cx: &mut TestAppContext) {
    let (_, window, workspace) = start(cx);
    let pasted: String = (1..=30).map(|line| format!("line {line}\n")).collect();
    cx.update(|cx| cx.write_to_clipboard(ClipboardItem::new_string(pasted.clone())));
    act(cx, window, |window, cx| window.input("Look: ", cx));
    act(cx, window, |window, cx| window.press("ctrl-v", cx));
    let composer = cx.read(|cx| {
        workspace
            .read(cx)
            .session_view()
            .read(cx)
            .composer()
            .clone()
    });
    let (tokens, text, expanded) = cx.read(|cx| {
        let composer = composer.read(cx);
        (
            composer.textarea().read(cx).tokens().len(),
            composer.text(cx),
            composer.expanded_text(cx),
        )
    });
    assert_eq!(tokens, 1);
    assert_eq!(text, "Look: [Pasted 30 lines #1]");
    assert_eq!(expanded, format!("Look: {pasted}"));
    // A short paste stays text.
    cx.update(|cx| cx.write_to_clipboard(ClipboardItem::new_string("two\nlines".into())));
    act(cx, window, |window, cx| window.press("ctrl-v", cx));
    assert_eq!(
        cx.read(|cx| composer.read(cx).textarea().read(cx).tokens().len()),
        1
    );
    assert!(composer_text(&workspace, cx).ends_with("two\nlines"));
}

#[gpui_kit::test]
fn the_send_key_follows_settings_and_mod_enter_always_sends(cx: &mut TestAppContext) {
    let (_, window, workspace) = start(cx);
    cx.update(|cx| prefs::update(cx, |prefs| prefs.input.send_key = SendKey::ModEnter));
    act(cx, window, |window, cx| window.input("One", cx));
    act(cx, window, |window, cx| window.press("enter", cx));
    act(cx, window, |window, cx| window.input("Two", cx));
    assert_eq!(
        composer_text(&workspace, cx),
        "One\nTwo",
        "Enter adds a line"
    );
    assert_eq!(open_title(&workspace, cx), None);
    act(cx, window, |window, cx| window.press("secondary-enter", cx));
    // Sent: a new (sample) session starts with it, and the composer empties.
    assert!(open_title(&workspace, cx).is_some());
    assert_eq!(composer_text(&workspace, cx), "");

    cx.update(|cx| prefs::update(cx, |prefs| prefs.input.send_key = SendKey::ShiftEnter));
    act(cx, window, |window, cx| window.press("secondary-n", cx));
    act(cx, window, |window, cx| window.input("Three", cx));
    act(cx, window, |window, cx| window.press("enter", cx));
    assert_eq!(composer_text(&workspace, cx), "Three\n");
    act(cx, window, |window, cx| window.press("shift-enter", cx));
    assert_eq!(composer_text(&workspace, cx), "");
}

#[gpui_kit::test]
fn the_slash_menu_takes_tab_and_enter_and_mod_enter_sends_as_typed(cx: &mut TestAppContext) {
    let (_, window, workspace) = start(cx);
    let composer = cx.read(|cx| {
        workspace
            .read(cx)
            .session_view()
            .read(cx)
            .composer()
            .clone()
    });
    let sent: Rc<RefCell<Vec<String>>> = Rc::default();
    let _subscription = cx.update(|cx| {
        let sent = sent.clone();
        cx.subscribe(&composer, move |_, event, _| {
            if let ComposerEvent::Submit(outgoing) = event {
                sent.borrow_mut().push(outgoing.text.clone());
            }
        })
    });
    // Types `text` into an empty composer, workers r1, r10 and r11 running.
    let type_text = |text: &str, cx: &mut TestAppContext| {
        let reading = Arc::new(arko::remote::Reading {
            workers_running: vec![
                ("r1".into(), None),
                ("r10".into(), None),
                ("r11".into(), None),
            ],
            ..arko::remote::Reading::default()
        });
        cx.update(|cx| {
            composer.update(cx, |composer, _| {
                composer.set_session_state(reading, false, true)
            })
        });
        act(cx, window, |window, cx| {
            composer.update(cx, |composer, cx| composer.set_text("", window, cx))
        });
        act(cx, window, |window, cx| window.input(text, cx));
    };

    // Tab and Enter take the highlighted word.
    type_text("/stop-workers r", cx);
    act(cx, window, |window, cx| window.press("enter", cx));
    assert_eq!(composer_text(&workspace, cx), "/stop-workers r1 ");
    type_text("/stop-workers r1", cx);
    act(cx, window, |window, cx| window.press("tab", cx));
    assert_eq!(composer_text(&workspace, cx), "/stop-workers r10 ");
    assert!(sent.borrow().is_empty());
    // A command typed in full goes as typed, though r10 and r11 are offered.
    type_text("/stop-workers r1", cx);
    assert!(cx.read(|cx| composer.read(cx).menu().is_some()));
    act(cx, window, |window, cx| window.press("enter", cx));
    assert_eq!(*sent.borrow(), ["/stop-workers r1"]);
    // Mod+Enter sends what is typed, whatever the menu offers.
    type_text("/stop-workers r", cx);
    act(cx, window, |window, cx| window.press("secondary-enter", cx));
    assert_eq!(
        sent.borrow().last().map(String::as_str),
        Some("/stop-workers r")
    );
}

#[gpui_kit::test]
fn told_not_to_ask_a_switch_to_agent_still_asks_for_the_folder(cx: &mut TestAppContext) {
    let (_, window, workspace) = start(cx);
    cx.update(|cx| prefs::update(cx, |prefs| prefs.sessions.ask_before_switch = false));
    open_row(window, &workspace, "Naming a desktop app", cx);
    let view = cx.read(|cx| workspace.read(cx).session_view().clone());
    act(cx, window, |window, cx| {
        view.update(cx, |view, cx| {
            view.choose_mode(crate::backend::Mode::Agent, window, cx)
        })
    });
    act(cx, window, |window, cx| {
        assert!(window.try_find("switch-mode").is_some());
        assert!(window.try_find("switch-folder").is_some());
        assert!(window.try_find("dont-ask-switch").is_none());
        window.within("dialog").click("cancel", cx);
    });
}

#[gpui_kit::test]
fn spell_check_and_suggestions_follow_settings(cx: &mut TestAppContext) {
    let (_, _, workspace) = start(cx);
    let textarea = cx.read(|cx| {
        workspace
            .read(cx)
            .session_view()
            .read(cx)
            .composer()
            .read(cx)
            .textarea()
            .clone()
    });
    let mode = |cx: &mut TestAppContext| {
        cx.read(|cx| {
            cx.global::<crate::writing::WritingAids>()
                .0
                .suggestion_mode()
        })
    };
    assert!(cx.read(|cx| textarea.read(cx).is_spell_checking()));
    assert_eq!(mode(cx), gpui_kit::writing::SuggestionMode::Quiet);
    cx.update(|cx| {
        prefs::update(cx, |prefs| {
            prefs.input.spell_check = false;
            prefs.input.suggestions = prefs::Suggestions::Off;
        })
    });
    assert!(!cx.read(|cx| textarea.read(cx).is_spell_checking()));
    assert_eq!(mode(cx), gpui_kit::writing::SuggestionMode::Off);
}

#[gpui_kit::test]
fn the_placeholder_and_send_button_follow_the_state(cx: &mut TestAppContext) {
    let (_, window, workspace) = start(cx);
    let view = cx.read(|cx| workspace.read(cx).session_view().clone());
    assert_eq!(cx.read(|cx| view.read(cx).placeholder(cx)), "Message Arko");
    act(cx, window, |window, _| {
        assert!(window.try_find("send").is_some());
        assert!(window.try_find("stop").is_none());
    });
    // A sample session doesn't run: sending says so, and keeps the text.
    open_row(window, &workspace, "Split the engine out of main.rs", cx);
    act(cx, window, |window, cx| window.input("Hello", cx));
    act(cx, window, |window, cx| window.press("enter", cx));
    assert_eq!(composer_text(&workspace, cx), "Hello");
    act(cx, window, |window, cx| {
        assert_eq!(window.notifications(cx).len(), 1)
    });
}

/// Claude Mythos 5.1 has an (i) after its name in the model list, and its
/// tooltip says it needs access first. Its row is as tall as the others,
/// and the models without a note have none.
#[gpui_kit::test]
fn the_model_list_says_what_access_mythos_needs(cx: &mut TestAppContext) {
    use crate::backend::{ModelChoice, ModelChoices, ModelGroup};
    let models = [
        ("cc:claude-opus-5-5", "cc:opus"),
        ("cc:claude-mythos-5-1", "cc:mythos"),
    ];
    let backend = FakeBackend::sample("/arko", SystemTime::now()).with_models(ModelChoices {
        groups: vec![ModelGroup {
            name: "Claude Code, on your Claude plan".into(),
            needs: None,
            models: models
                .iter()
                .map(|(name, alias)| ModelChoice {
                    name: (*name).into(),
                    alias: (*alias).into(),
                })
                .collect(),
        }],
        default: Some("cc:claude-opus-5-5".into()),
        note: None,
    });
    cx.update(|cx| crate::init(Startup::for_tests(Arc::new(backend)), cx));
    let (window, _) = open(cx, 1300.);
    act(cx, window, |window, cx| window.click("mode-model", cx));
    let info = cx
        .update_window(window, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("model-note-0-0").is_none());
            let height = |id: &'static str| window.find(id).bounds().size.height;
            assert_eq!(height("model-0-1"), height("model-0-0"));
            window.find("model-note-0-1").bounds().center()
        })
        .unwrap();
    let cx = &mut gpui_kit::VisualTestContext::from_window(window, cx);
    assert!(cx.debug_bounds("tooltip").is_none());
    cx.simulate_mouse_move(info, None, gpui_kit::Modifiers::default());
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(2));
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.debug_bounds("tooltip").is_some());
}

#[gpui_kit::test]
fn the_mode_chip_lists_the_models_and_picks_one(cx: &mut TestAppContext) {
    let (_, window, workspace) = start(cx);
    let view = cx.read(|cx| workspace.read(cx).session_view().clone());
    act(cx, window, |window, cx| window.click("mode-model", cx));
    act(cx, window, |window, cx| {
        assert!(window.try_find("model-list").is_some());
        // The second model of the one group: Sonnet.
        window.click("model-0-1", cx);
    });
    assert_eq!(
        cx.read(|cx| view.read(cx).draft().model.clone()).as_deref(),
        Some("claude-sonnet-5-5")
    );
    assert_eq!(
        cx.read(|cx| view.read(cx).chip_label(cx)),
        "Agent · Sonnet 5.5"
    );
    // `/model` in a new session's composer picks it too.
    act(cx, window, |window, cx| {
        window.input("/model claude-opus-5-5 high", cx)
    });
    act(cx, window, |window, cx| window.press("escape", cx));
    act(cx, window, |window, cx| window.press("enter", cx));
    assert_eq!(
        cx.read(|cx| view.read(cx).chip_label(cx)),
        "Agent · Opus 5.5 · high"
    );
    assert_eq!(open_title(&workspace, cx), None, "no session started");
    // A name that isn't listed is refused, and stays to be put right.
    act(cx, window, |window, cx| {
        window.input("/model claude-sonnet-5", cx)
    });
    act(cx, window, |window, cx| window.press("escape", cx));
    act(cx, window, |window, cx| window.press("enter", cx));
    assert_eq!(composer_text(&workspace, cx), "/model claude-sonnet-5");
    assert_eq!(
        cx.read(|cx| view.read(cx).draft().model.clone()).as_deref(),
        Some("claude-opus-5-5")
    );
    assert_eq!(open_title(&workspace, cx), None);
    // Any of OpenRouter's models is taken by its slug, listed or not.
    let composer = cx.read(|cx| view.read(cx).composer().clone());
    act(cx, window, |window, cx| {
        composer.update(cx, |composer, cx| composer.set_text("", window, cx))
    });
    act(cx, window, |window, cx| {
        window.input("/model openrouter:z-ai/glm-5", cx)
    });
    act(cx, window, |window, cx| window.press("escape", cx));
    act(cx, window, |window, cx| window.press("enter", cx));
    assert_eq!(
        cx.read(|cx| view.read(cx).draft().model.clone()).as_deref(),
        Some("openrouter:z-ai/glm-5")
    );
    assert_eq!(composer_text(&workspace, cx), "");
    assert_eq!(open_title(&workspace, cx), None);
}

#[gpui_kit::test]
fn missing_credentials_are_entered_in_a_dialog(cx: &mut TestAppContext) {
    let (backend, window, workspace) = start(cx);
    open_row(window, &workspace, "Split the engine out of main.rs", cx);
    let session = cx.read(|cx| workspace.read(cx).live_session().cloned().unwrap());
    let need = || {
        PromptKind::Credentials(arko::Need::AnthropicKey {
            model: "claude-opus-5-5".into(),
        })
    };
    let mut answer = cx.update(|cx| session.update(cx, |session, cx| session.ask(need(), cx)));
    act(cx, window, |window, cx| {
        assert!(window.has_active_dialog(cx));
        assert!(window.try_find("credentials-form").is_some());
    });
    // Saving with nothing typed says so and keeps asking.
    act(cx, window, |window, cx| {
        window.click("credentials-save", cx)
    });
    act(cx, window, |window, _| {
        assert!(window.try_find("credentials-error").is_some())
    });
    act(cx, window, |window, cx| {
        window.input("sk-ant-test-0123456789", cx)
    });
    // Enter in the field saves, as the button does.
    act(cx, window, |window, cx| window.press("enter", cx));
    assert_eq!(
        backend.added_keys(),
        [(
            "anthropic".to_string(),
            "sk-ant-test-0123456789".to_string()
        )]
    );
    assert_eq!(answer.try_recv(), Ok(PromptAnswer::Again));
    act(cx, window, |window, cx| {
        assert!(!window.has_active_dialog(cx))
    });

    // Closed without an answer, it waits: the strip brings it back, and
    // Cancel tells the engine.
    let mut answer = cx.update(|cx| session.update(cx, |session, cx| session.ask(need(), cx)));
    act(cx, window, |window, cx| window.close_dialog(cx));
    act(cx, window, |window, cx| window.click("prompt-show", cx));
    act(cx, window, |window, cx| {
        window.click("credentials-cancel", cx)
    });
    assert_eq!(answer.try_recv(), Ok(PromptAnswer::Cancel));
    act(cx, window, |window, _| {
        assert!(window.try_find("prompt-strip").is_none())
    });
}

#[gpui_kit::test]
fn claude_code_is_checked_again_from_its_dialog(cx: &mut TestAppContext) {
    let (_, window, workspace) = start(cx);
    open_row(window, &workspace, "Split the engine out of main.rs", cx);
    let session = cx.read(|cx| workspace.read(cx).live_session().cloned().unwrap());
    let mut answer = cx.update(|cx| {
        session.update(cx, |session, cx| {
            session.ask(
                PromptKind::Credentials(arko::Need::ClaudeCode {
                    model: "cc:claude-opus-5-5".into(),
                    problem: arko::accounts::Problem::Missing,
                }),
                cx,
            )
        })
    });
    act(cx, window, |window, cx| {
        assert!(
            window.try_find("credentials-save").is_none(),
            "no key to enter"
        );
        window.click("credentials-again", cx);
    });
    assert_eq!(answer.try_recv(), Ok(PromptAnswer::Again));
}

#[gpui_kit::test]
fn ctrl_b_says_when_no_command_runs(cx: &mut TestAppContext) {
    let (_, window, workspace) = start(cx);
    open_row(window, &workspace, "Split the engine out of main.rs", cx);
    act(cx, window, |window, cx| window.press("ctrl-b", cx));
    act(cx, window, |window, cx| {
        assert_eq!(window.notifications(cx).len(), 1)
    });
}

#[gpui_kit::test]
fn closing_a_sample_session_shows_a_new_one(cx: &mut TestAppContext) {
    let (_, window, workspace) = start(cx);
    open_row(window, &workspace, "Split the engine out of main.rs", cx);
    assert!(open_title(&workspace, cx).is_some());
    act(cx, window, |window, cx| {
        window.dispatch_action(Box::new(crate::menus::CloseSession), cx)
    });
    assert_eq!(open_title(&workspace, cx), None);
    // And it can open again.
    open_row(window, &workspace, "Split the engine out of main.rs", cx);
    assert!(open_title(&workspace, cx).is_some());
}

#[gpui_kit::test]
fn an_arko_command_from_the_palette_waits_in_the_composer(cx: &mut TestAppContext) {
    let (_, window, workspace) = start(cx);
    let run = |name: &'static str, cx: &mut TestAppContext| {
        act(cx, window, |window, cx| {
            workspace.update(cx, |workspace, cx| {
                workspace.run(crate::palette::Target::Slash(name), None, window, cx)
            })
        });
    };
    // A command with arguments waits for them, its words offered.
    run("/outside", cx);
    assert_eq!(composer_text(&workspace, cx), "/outside ");
    act(cx, window, |window, _| {
        assert!(window.try_find("slash-menu").is_some())
    });
    // Without a session, one without arguments waits for the first message.
    run("/cost", cx);
    assert_eq!(composer_text(&workspace, cx), "/cost");
}

#[gpui_kit::test]
fn a_pasted_image_waits_as_a_tile_until_sent_or_removed(cx: &mut TestAppContext) {
    let (_, window, workspace) = start(cx);
    let image =
        gpui_kit::Image::from_bytes(gpui_kit::ImageFormat::Png, crate::testing::PNG.to_vec());
    cx.update(|cx| cx.write_to_clipboard(ClipboardItem::new_image(&image)));
    act(cx, window, |window, cx| window.press("ctrl-v", cx));
    let composer = cx.read(|cx| {
        workspace
            .read(cx)
            .session_view()
            .read(cx)
            .composer()
            .clone()
    });
    assert_eq!(cx.read(|cx| composer.read(cx).images().len()), 1);
    assert_eq!(composer_text(&workspace, cx), "", "no text pasted");
    act(cx, window, |window, _| {
        assert!(window.try_find("composer-images").is_some());
        // With an image, there's something to send.
        assert!(window.try_find("send").is_some());
    });
    // Its remove control shows on hover.
    let tile: gpui_kit::ElementId = ("image", 0usize).into();
    let remove = gpui_kit::ElementId::from((tile, "remove"));
    act(cx, window, |window, cx| {
        let tiles = window.find("composer-images").bounds();
        let over = tiles.origin + gpui_kit::point(gpui_kit::px(12.), gpui_kit::px(12.));
        window.dispatch_event(
            gpui_kit::PlatformInput::MouseMove(gpui_kit::MouseMoveEvent {
                position: over,
                pressed_button: None,
                modifiers: gpui_kit::Modifiers::default(),
            }),
            cx,
        );
    });
    act(cx, window, |window, cx| window.click(remove, cx));
    assert!(cx.read(|cx| composer.read(cx).images().is_empty()));
    act(cx, window, |window, _| {
        assert!(window.try_find("composer-images").is_none())
    });
}

#[gpui_kit::test]
fn an_image_the_model_cant_take_is_refused_before_sending(cx: &mut TestAppContext) {
    // A 1×1 GIF, which Gemini doesn't take.
    const GIF: &[u8] = b"GIF89a\x01\x00\x01\x00\x80\x00\x00\xff\xff\xff\x00\x00\x00!\xf9\x04\x01\x00\x00\x00\x00,\x00\x00\x00\x00\x01\x00\x01\x00\x00\x02\x02D\x01\x00;";
    let (_, window, workspace) = start(cx);
    let view = cx.read(|cx| workspace.read(cx).session_view().clone());
    let composer = cx.read(|cx| view.read(cx).composer().clone());
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.set_draft_model("gemini-3.1-pro-preview".into(), None, cx)
        });
        composer.update(cx, |composer, cx| {
            composer.attach_image(
                crate::composer::Attached::new(
                    gpui_kit::ImageFormat::Gif,
                    GIF.to_vec(),
                    Some("dot.gif".into()),
                ),
                cx,
            )
        });
    });
    let refusal = cx.read(|cx| composer.read(cx).image_refusal());
    assert!(
        refusal.as_deref().is_some_and(|why| why.contains("GIF")),
        "{refusal:?}"
    );
    act(cx, window, |window, cx| window.input("What is this?", cx));
    act(cx, window, |window, cx| window.press("enter", cx));
    // Nothing went: the text and the image stay for the user to change.
    assert_eq!(open_title(&workspace, cx), None);
    assert_eq!(composer_text(&workspace, cx), "What is this?");
    assert_eq!(cx.read(|cx| composer.read(cx).images().len()), 1);
    // A model that takes it clears the warning.
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.set_draft_model("claude-opus-5-5".into(), None, cx)
        })
    });
    assert_eq!(cx.read(|cx| composer.read(cx).image_refusal()), None);
}

#[test]
fn image_files_are_known_by_their_extension() {
    use crate::composer::image_format;
    use std::path::Path;
    assert_eq!(
        image_format(Path::new("a/shot.PNG")),
        Some(gpui_kit::ImageFormat::Png)
    );
    assert_eq!(
        image_format(Path::new("photo.jpeg")),
        Some(gpui_kit::ImageFormat::Jpeg)
    );
    assert_eq!(image_format(Path::new("notes.md")), None);
    assert_eq!(image_format(Path::new("Makefile")), None);
}

#[gpui_kit::test]
fn at_mentions_a_file_of_the_agents_folder_as_a_token(cx: &mut TestAppContext) {
    let folder = tempfile::tempdir().unwrap();
    let root = folder.path().to_path_buf();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/parse.rs"), "").unwrap();
    std::fs::write(root.join("src/main.rs"), "").unwrap();
    std::fs::write(root.join("README.md"), "").unwrap();
    let (_, window, workspace) = start(cx);
    let view = cx.read(|cx| workspace.read(cx).session_view().clone());
    cx.update(|cx| view.update(cx, |view, cx| view.set_draft_folder(root.clone(), cx)));
    let composer = cx.read(|cx| view.read(cx).composer().clone());
    act(cx, window, |window, cx| window.input("Look at @par", cx));
    // The folder is listed off the UI thread, then the menu offers it.
    crate::testing::wait_until(cx, "the files", |cx| {
        cx.read(|cx| composer.read(cx).menu().is_some())
    });
    let (kind, first) = cx.read(|cx| {
        let menu = composer.read(cx).menu().unwrap();
        (menu.kind, menu.items[0].insert.clone())
    });
    assert_eq!(kind, crate::composer::MenuKind::Mention);
    assert_eq!(first, "src/parse.rs");
    act(cx, window, |window, cx| window.press("enter", cx));
    // A token showing the file's name, which sends as its path.
    let (tokens, text) = cx.read(|cx| {
        let composer = composer.read(cx);
        (
            composer.textarea().read(cx).tokens().len(),
            composer.expanded_text(cx),
        )
    });
    assert_eq!(tokens, 1);
    assert_eq!(text, "Look at src/parse.rs ");
    // A chat mentions nothing.
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.set_draft_mode(crate::backend::Mode::Chat, cx)
        })
    });
    act(cx, window, |window, cx| window.input("and @mai", cx));
    cx.run_until_parked();
    assert!(cx.read(|cx| composer.read(cx).menu().is_none()));
}

/// Enter or Tab pressed right after a quick change, before the background
/// ranking of the new words came, chooses among the files for what is
/// typed now, never the earlier words' file still on screen.
#[gpui_kit::test]
fn a_fast_enter_or_tab_chooses_the_file_for_what_is_typed_now(cx: &mut TestAppContext) {
    let folder = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(folder.path().join("src")).unwrap();
    for name in ["main.rs", "parse.rs"] {
        std::fs::write(folder.path().join("src").join(name), "").unwrap();
    }
    let (_, window, workspace) = start(cx);
    let view = cx.read(|cx| workspace.read(cx).session_view().clone());
    view.update(cx, |view, cx| {
        view.set_draft_folder(folder.path().into(), cx)
    });
    let composer = cx.read(|cx| view.read(cx).composer().clone());
    let sent: Rc<RefCell<Vec<String>>> = Rc::default();
    let _subscription = cx.update(|cx| {
        let sent = sent.clone();
        cx.subscribe(&composer, move |_, event, _| {
            if let ComposerEvent::Submit(outgoing) = event {
                sent.borrow_mut().push(outgoing.text.clone());
            }
        })
    });
    let first = |cx: &mut TestAppContext| {
        cx.read(|cx| {
            composer
                .read(cx)
                .menu()
                .and_then(|menu| menu.items.first().map(|item| item.insert.clone()))
        })
    };
    for key in ["enter", "tab"] {
        act(cx, window, |window, cx| window.input("See @ma", cx));
        crate::testing::wait_until(cx, "the first query", |cx| first(cx).is_some());
        // A quick change, then the key, before anything runs in the
        // background: the earlier words' list is still on screen.
        cx.update_window(window, |_, window, cx| {
            window.press("backspace", cx);
            window.press("backspace", cx);
            window.input("pa", cx);
        })
        .unwrap();
        assert_eq!(first(cx).as_deref(), Some("src/main.rs"));
        cx.update_window(window, |_, window, cx| window.press(key, cx))
            .unwrap();
        cx.run_until_parked();
        let text = cx.read(|cx| composer.read(cx).expanded_text(cx));
        assert_eq!(text, "See src/parse.rs ", "{key}");
        assert!(sent.borrow().is_empty(), "{key} sent {:?}", sent.borrow());
        act(cx, window, |window, cx| {
            composer.update(cx, |composer, cx| composer.set_text("", window, cx))
        });
    }
}

/// The files for an @-mention are ranked off the UI thread: until the new
/// ranking comes, the list of what was typed before stays, and the latest
/// words' files are the ones that show.
#[gpui_kit::test]
fn an_at_mentions_files_are_ranked_off_the_ui_thread_latest_first(cx: &mut TestAppContext) {
    let folder = tempfile::tempdir().unwrap();
    let root = folder.path().to_path_buf();
    std::fs::create_dir_all(root.join("src")).unwrap();
    for file in ["src/main.rs", "src/parse.rs", "README.md"] {
        std::fs::write(root.join(file), "").unwrap();
    }
    let (_, window, workspace) = start(cx);
    let view = cx.read(|cx| workspace.read(cx).session_view().clone());
    cx.update(|cx| view.update(cx, |view, cx| view.set_draft_folder(root.clone(), cx)));
    let composer = cx.read(|cx| view.read(cx).composer().clone());
    act(cx, window, |window, cx| window.input("see @ma", cx));
    crate::testing::wait_until(cx, "the files", |cx| {
        cx.read(|cx| composer.read(cx).menu().is_some())
    });
    let first = |cx: &mut TestAppContext| {
        cx.read(|cx| {
            composer
                .read(cx)
                .menu()
                .map(|menu| (menu.items[0].insert.clone(), menu.range.clone()))
        })
    };
    assert_eq!(first(cx), Some(("src/main.rs".to_string(), 4..7)));
    // Two quick changes, nothing run in the background yet: the list stays.
    cx.update_window(window, |_, window, cx| {
        window.press("backspace", cx);
        window.press("backspace", cx);
        window.input("pa", cx);
    })
    .unwrap();
    assert_eq!(first(cx), Some(("src/main.rs".to_string(), 4..7)));
    cx.run_until_parked();
    assert_eq!(first(cx), Some(("src/parse.rs".to_string(), 4..7)));
}

/// An ordinary word after an "@" can hold a file's letters here and there
/// ("@here" in heart-pulse.svg): Enter and Shift+Enter keep their usual
/// meaning then, and Tab or a move in the list takes the file.
#[gpui_kit::test]
fn a_weak_file_match_leaves_enter_to_send(cx: &mut TestAppContext) {
    let folder = tempfile::tempdir().unwrap();
    let root = folder.path().to_path_buf();
    std::fs::create_dir_all(root.join("icons")).unwrap();
    for file in ["icons/heart-pulse.svg", "README.md"] {
        std::fs::write(root.join(file), "").unwrap();
    }
    let (_, window, workspace) = start(cx);
    let view = cx.read(|cx| workspace.read(cx).session_view().clone());
    cx.update(|cx| view.update(cx, |view, cx| view.set_draft_folder(root.clone(), cx)));
    let composer = cx.read(|cx| view.read(cx).composer().clone());
    let sent: Rc<RefCell<Vec<String>>> = Rc::default();
    let _subscription = cx.update(|cx| {
        let sent = sent.clone();
        cx.subscribe(&composer, move |_, event, _| {
            if let ComposerEvent::Submit(outgoing) = event {
                sent.borrow_mut().push(outgoing.text.clone());
            }
        })
    });
    let menu = |cx: &mut TestAppContext| {
        cx.read(|cx| {
            composer
                .read(cx)
                .menu()
                .map(|menu| (menu.items[0].insert.clone(), menu.takes_enter))
        })
    };
    act(cx, window, |window, cx| window.input("Ping @here", cx));
    crate::testing::wait_until(cx, "the files", |cx| menu(cx).is_some());
    assert_eq!(menu(cx), Some(("icons/heart-pulse.svg".to_string(), false)));
    // Shift+Enter breaks the line, as without the list.
    act(cx, window, |window, cx| window.press("shift-enter", cx));
    assert_eq!(composer_text(&workspace, cx), "Ping @here\n");
    act(cx, window, |window, cx| window.press("backspace", cx));
    crate::testing::wait_until(cx, "the files", |cx| menu(cx).is_some());
    // Enter sends the words as typed.
    act(cx, window, |window, cx| window.press("enter", cx));
    assert_eq!(*sent.borrow(), ["Ping @here"]);
    // A move in the list, and Enter takes the file.
    act(cx, window, |window, cx| window.input("Look at @here", cx));
    crate::testing::wait_until(cx, "the files", |cx| menu(cx).is_some());
    act(cx, window, |window, cx| window.press("down", cx));
    act(cx, window, |window, cx| window.press("up", cx));
    act(cx, window, |window, cx| window.press("enter", cx));
    assert_eq!(sent.borrow().len(), 1);
    assert!(
        cx.read(|cx| composer.read(cx).expanded_text(cx))
            .contains("icons/heart-pulse.svg")
    );
}

/// Two files of one name show their folders in the composer, and a path
/// with spaces goes to the model in backticks.
#[gpui_kit::test]
fn a_mentions_token_tells_files_of_one_name_apart(cx: &mut TestAppContext) {
    let folder = tempfile::tempdir().unwrap();
    let root = folder.path().to_path_buf();
    for dir in ["src/a", "src/b", "docs"] {
        std::fs::create_dir_all(root.join(dir)).unwrap();
    }
    for file in ["src/a/mod.rs", "src/b/mod.rs", "docs/my notes.md"] {
        std::fs::write(root.join(file), "").unwrap();
    }
    let (_, window, workspace) = start(cx);
    let view = cx.read(|cx| workspace.read(cx).session_view().clone());
    cx.update(|cx| view.update(cx, |view, cx| view.set_draft_folder(root.clone(), cx)));
    let composer = cx.read(|cx| view.read(cx).composer().clone());
    let has_menu = |cx: &mut TestAppContext| cx.read(|cx| composer.read(cx).menu().is_some());
    act(cx, window, |window, cx| window.input("See @a/mod", cx));
    crate::testing::wait_until(cx, "the files", has_menu);
    act(cx, window, |window, cx| window.press("enter", cx));
    act(cx, window, |window, cx| window.input("and (@notes", cx));
    crate::testing::wait_until(cx, "the files", has_menu);
    act(cx, window, |window, cx| window.press("enter", cx));
    let (labels, text) = cx.read(|cx| {
        let composer = composer.read(cx);
        let labels: Vec<String> = composer
            .textarea()
            .read(cx)
            .tokens()
            .iter()
            .map(|span| span.token().label().to_string())
            .collect();
        (labels, composer.expanded_text(cx))
    });
    assert_eq!(labels, ["a/mod.rs", "my notes.md"]);
    assert_eq!(text, "See src/a/mod.rs and (`docs/my notes.md` ");
}

/// The first "@" lists the folder off the UI thread: the list says so
/// meanwhile, and shows the files once they come.
#[gpui_kit::test]
fn the_first_at_says_the_files_are_being_listed(cx: &mut TestAppContext) {
    let folder = tempfile::tempdir().unwrap();
    let root = folder.path().to_path_buf();
    std::fs::write(root.join("README.md"), "").unwrap();
    let (_, window, workspace) = start(cx);
    let view = cx.read(|cx| workspace.read(cx).session_view().clone());
    cx.update(|cx| view.update(cx, |view, cx| view.set_draft_folder(root.clone(), cx)));
    let composer = cx.read(|cx| view.read(cx).composer().clone());
    let menu = |cx: &mut TestAppContext| {
        cx.read(|cx| {
            composer
                .read(cx)
                .menu()
                .map(|menu| (menu.note.clone(), menu.items.len()))
        })
    };
    cx.update_window(window, |_, window, cx| window.input("See @", cx))
        .unwrap();
    assert_eq!(menu(cx), Some((Some("Listing files…".into()), 0)));
    // Enter sends as typed meanwhile, and Tab chooses nothing.
    let takes_enter = cx.read(|cx| composer.read(cx).menu().map(|menu| menu.takes_enter));
    assert_eq!(takes_enter, Some(false));
    crate::testing::wait_until(cx, "the files", |cx| {
        menu(cx).is_some_and(|(note, items)| note.is_none() && items == 1)
    });
}

/// Branch from Here also works for a session this app hasn't opened.
#[gpui_kit::test]
fn branch_from_here_accepts_a_closed_session(cx: &mut TestAppContext) {
    let (_, window, workspace) = start(cx);
    let heard = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    cx.update(|cx| {
        let sidebar = workspace.read(cx).sidebar().clone();
        let heard = heard.clone();
        cx.subscribe(
            &sidebar,
            move |_, event: &crate::sidebar::SidebarEvent, _| {
                heard.borrow_mut().push(event.clone())
            },
        )
        .detach();
    });
    let row = row_of(&workspace, "Split the engine out of main.rs", cx);
    act(cx, window, |window, cx| {
        window.right_click(("row", row), cx)
    });
    act(cx, window, |window, cx| {
        let branch = window.within("popup-menu").find(2usize);
        assert_eq!(branch.label(), Some("Branch from Here"));
        window.within("popup-menu").click(2usize, cx);
    });
    assert!(
        heard
            .borrow()
            .iter()
            .any(|event| matches!(event, crate::sidebar::SidebarEvent::Branch(_))),
        "{:?}",
        heard.borrow()
    );
}

/// A right-click in the composer opens the menu the kit draws in the
/// theme, which every platform shows the same, with the field's items.
#[gpui_kit::test]
fn the_composer_menu_is_the_drawn_one(cx: &mut TestAppContext) {
    use gpui_kit::component::input::ContextMenuStyle;

    let (_, window, _) = start(cx);
    assert_eq!(
        cx.read(ContextMenuStyle::app_default),
        Some(ContextMenuStyle::Drawn)
    );
    act(cx, window, |window, cx| window.input("Some words", cx));
    act(cx, window, |window, cx| {
        window.right_click("composer-field", cx)
    });
    act(cx, window, |window, _| {
        assert!(
            window.try_find("popup-menu").is_some(),
            "the drawn menu opened"
        );
        let labels: Vec<String> = (0..12usize)
            .filter_map(|ix| window.within("popup-menu").try_find(ix))
            .filter_map(|item| item.label().map(str::to_string))
            .collect();
        assert!(labels.iter().any(|label| label == "Paste"), "{labels:?}");
    });
}
