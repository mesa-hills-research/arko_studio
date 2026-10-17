//! Interaction tests of the composer and the session's dialogs, on the fake
//! backend's sample sessions (which show, but don't run).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::SystemTime;

use gpui_kit::component::WindowExt as _;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AnyWindowHandle, ClipboardItem, Entity, TestAppContext};

use crate::backend::FakeBackend;
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
    act(cx, window, |window, cx| {
        window.click("credentials-save", cx)
    });
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
