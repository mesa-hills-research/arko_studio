//! Golden screenshots of Arko Studio's window, rendered headlessly by the GPUI
//! fork's gpui-pre-screenshot on Mesa's software Vulkan driver, in Work Sans
//! and JetBrains Mono. The goldens live in `tests/screenshots`. On a mismatch
//! the actual image and a diff go to `tests/screenshots/failures`, and
//! `UPDATE_GOLDENS=1` rewrites the goldens.
#![cfg(target_os = "linux")]

use std::sync::Arc;
use std::time::SystemTime;

use arko_studio::backend::{Backend as _, FakeBackend, SessionEntry, SessionId};
use arko_studio::prefs::{self, Motion, Prefs, ThemeMode};
use arko_studio::{Startup, assets::Assets, fonts, workspace};
use gpui_kit::{px, size};
use gpui_screenshot::{Goldens, Screenshot, ScreenshotApp, Tolerance};

/// Mesa and LLVM releases round antialiased edges a level or two apart, so
/// each channel of each pixel may differ from its golden by up to 2.
fn goldens() -> Goldens {
    gpui_screenshot::goldens!().tolerance(Tolerance::channel(2))
}

/// A window as it opens, on a new session, `width` logical pixels wide, with
/// the sample sessions in the sidebar.
fn new_window(mode: ThemeMode, width: f32) -> Screenshot {
    let mut app = ScreenshotApp::with_assets(Assets).unwrap();
    let mut settings = Prefs::default();
    settings.appearance.theme_mode = mode;
    // The harness draws animations at their end, and so does the app then.
    settings.appearance.motion = Motion::Off;
    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    let startup = Startup::for_tests(backend)
        .with_prefs(settings)
        .with_fonts(fonts::bundled());
    app.update(|cx| {
        arko_studio::init(startup, cx);
        for family in [prefs::DEFAULT_UI_FONT, prefs::DEFAULT_CODE_FONT] {
            assert!(fonts::is_available(family, cx), "{family} isn't loaded");
        }
    });
    let window = app
        .open_window_with(size(px(width), px(800.)), 1., |options, cx| {
            workspace::open_window(options, cx).map(|(window, _)| window)
        })
        .unwrap();
    app.capture(window).unwrap()
}

/// A window `width` logical pixels wide on a session with a long title, in
/// a folder with a long name, that had workers (so both layout buttons
/// show). The folder's name gives way first, then the title.
fn long_title(mode: ThemeMode, width: f32) -> Screenshot {
    let mut app = ScreenshotApp::with_assets(Assets).unwrap();
    let mut settings = Prefs::default();
    settings.appearance.theme_mode = mode;
    settings.appearance.motion = Motion::Off;
    let mut sessions = FakeBackend::sample("/arko", SystemTime::now())
        .sessions()
        .unwrap();
    let id = SessionId("20261016-091300-e1e1".into());
    sessions.push(SessionEntry {
        id: id.clone(),
        title: "A long session title that should stay readable".into(),
        workspace: Some("/home/ada/src/a-project-folder-with-a-very-long-descriptive-name".into()),
        had_workers: true,
        ..sessions[0].clone()
    });
    let backend =
        FakeBackend::new("/arko", sessions).with_default_model(Some("claude-opus-5-5".into()));
    let startup = Startup::for_tests(Arc::new(backend))
        .with_prefs(settings)
        .with_fonts(fonts::bundled());
    app.update(|cx| arko_studio::init(startup, cx));
    let mut opened = None;
    let window = app
        .open_window_with(size(px(width), px(400.)), 1., |options, cx| {
            workspace::open_window(options, cx).map(|(window, workspace)| {
                opened = Some(workspace);
                window
            })
        })
        .unwrap();
    let workspace = opened.unwrap();
    app.update_window(window, |_, window, cx| {
        workspace.update(cx, |workspace, cx| workspace.open_session(id, window, cx));
    })
    .unwrap();
    app.run_until_parked();
    app.capture(window).unwrap()
}

#[test]
fn long_title_light() {
    goldens().assert("long-title-light", &long_title(ThemeMode::Light, 600.));
}
#[test]
fn long_title_dark() {
    goldens().assert("long-title-dark", &long_title(ThemeMode::Dark, 600.));
}
#[test]
fn minimum_title_light() {
    goldens().assert("minimum-title-light", &long_title(ThemeMode::Light, 400.));
}
#[test]
fn minimum_title_dark() {
    goldens().assert("minimum-title-dark", &long_title(ThemeMode::Dark, 400.));
}

#[test]
fn wide_light() {
    goldens().assert("wide-light", &new_window(ThemeMode::Light, 1300.));
}

#[test]
fn wide_dark() {
    goldens().assert("wide-dark", &new_window(ThemeMode::Dark, 1300.));
}

#[test]
fn half_light() {
    goldens().assert("half-light", &new_window(ThemeMode::Light, 900.));
}

#[test]
fn half_dark() {
    goldens().assert("half-dark", &new_window(ThemeMode::Dark, 900.));
}

#[test]
fn narrow_light() {
    goldens().assert("narrow-light", &new_window(ThemeMode::Light, 600.));
}

#[test]
fn narrow_dark() {
    goldens().assert("narrow-dark", &new_window(ThemeMode::Dark, 600.));
}

/// Settings' page `page` in a window `width` wide, on the sample sessions
/// and accounts.
fn settings_window(mode: ThemeMode, width: f32, page: usize) -> Screenshot {
    let mut app = ScreenshotApp::with_assets(Assets).unwrap();
    let mut settings = Prefs::default();
    settings.appearance.theme_mode = mode;
    settings.appearance.motion = Motion::Off;
    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    let startup = Startup::for_tests(backend)
        .with_prefs(settings)
        .with_fonts(fonts::bundled());
    app.update(|cx| arko_studio::init(startup, cx));
    let opened = std::cell::RefCell::new(None);
    let window = app
        .open_window_with(size(px(width), px(1200.)), 1., |options, cx| {
            workspace::open_window(options, cx).map(|(window, workspace)| {
                opened.replace(Some(workspace));
                window
            })
        })
        .unwrap();
    let workspace = opened.take().unwrap();
    app.update_window(window, |_, window, cx| {
        window.dispatch_action(Box::new(arko_studio::menus::OpenSettings), cx)
    })
    .unwrap();
    app.run_until_parked();
    app.update(|cx| {
        let settings = workspace.read(cx).settings().unwrap().clone();
        settings.update(cx, |settings, cx| settings.show_page(page, cx));
    });
    app.capture(window).unwrap()
}

#[test]
fn settings_appearance_light() {
    goldens().assert(
        "settings-appearance-light",
        &settings_window(ThemeMode::Light, 1300., 0),
    );
}

#[test]
fn settings_appearance_dark() {
    goldens().assert(
        "settings-appearance-dark",
        &settings_window(ThemeMode::Dark, 1300., 0),
    );
}

#[test]
fn settings_input_light() {
    goldens().assert(
        "settings-input-light",
        &settings_window(ThemeMode::Light, 1300., 1),
    );
}

#[test]
fn settings_input_dark() {
    goldens().assert(
        "settings-input-dark",
        &settings_window(ThemeMode::Dark, 1300., 1),
    );
}

/// Where a choice no longer fits beside its title: Send key goes under it.
#[test]
fn settings_input_medium_light() {
    goldens().assert(
        "settings-input-medium-light",
        &settings_window(ThemeMode::Light, 980., 1),
    );
}

#[test]
fn settings_input_medium_dark() {
    goldens().assert(
        "settings-input-medium-dark",
        &settings_window(ThemeMode::Dark, 980., 1),
    );
}

#[test]
fn settings_accounts_light() {
    goldens().assert(
        "settings-accounts-light",
        &settings_window(ThemeMode::Light, 1300., 2),
    );
}

#[test]
fn settings_accounts_dark() {
    goldens().assert(
        "settings-accounts-dark",
        &settings_window(ThemeMode::Dark, 1300., 2),
    );
}

#[test]
fn settings_sessions_light() {
    goldens().assert(
        "settings-sessions-light",
        &settings_window(ThemeMode::Light, 1300., 3),
    );
}

#[test]
fn settings_sessions_dark() {
    goldens().assert(
        "settings-sessions-dark",
        &settings_window(ThemeMode::Dark, 1300., 3),
    );
}

#[test]
fn settings_notifications_light() {
    goldens().assert(
        "settings-notifications-light",
        &settings_window(ThemeMode::Light, 1300., 4),
    );
}

#[test]
fn settings_notifications_dark() {
    goldens().assert(
        "settings-notifications-dark",
        &settings_window(ThemeMode::Dark, 1300., 4),
    );
}

#[test]
fn settings_web_page_light() {
    goldens().assert(
        "settings-web-page-light",
        &settings_window(ThemeMode::Light, 1300., 5),
    );
}

#[test]
fn settings_web_page_dark() {
    goldens().assert(
        "settings-web-page-dark",
        &settings_window(ThemeMode::Dark, 1300., 5),
    );
}

#[test]
fn settings_extensions_light() {
    goldens().assert(
        "settings-extensions-light",
        &settings_window(ThemeMode::Light, 1300., 6),
    );
}

#[test]
fn settings_extensions_dark() {
    goldens().assert(
        "settings-extensions-dark",
        &settings_window(ThemeMode::Dark, 1300., 6),
    );
}

#[test]
fn settings_advanced_light() {
    goldens().assert(
        "settings-advanced-light",
        &settings_window(ThemeMode::Light, 1300., 7),
    );
}

#[test]
fn settings_advanced_dark() {
    goldens().assert(
        "settings-advanced-dark",
        &settings_window(ThemeMode::Dark, 1300., 7),
    );
}

/// Under 480 px, each setting's control goes under its title.
#[test]
fn settings_sessions_narrow_light() {
    goldens().assert(
        "settings-sessions-narrow-light",
        &settings_window(ThemeMode::Light, 460., 3),
    );
}

#[test]
fn settings_sessions_narrow_dark() {
    goldens().assert(
        "settings-sessions-narrow-dark",
        &settings_window(ThemeMode::Dark, 460., 3),
    );
}

/// The app's icon, the image the transcript's tool call shows.
const PNG: &[u8] = include_bytes!("../packaging/linux/com.mesahillsresearch.ArkoStudio.png");

/// The transcript's feed, fed as a session feeds it.
mod feed {
    use std::collections::HashMap;

    use arko::images::ImageRef;
    use arko::remote::{CallStatus, Delivery, Feed, Kind, Level, Source, WorkerChange};
    use arko_agents::AgentEvent;
    use arko_workflows::WorkflowEvent;
    use serde_json::Value;
    use uuid::Uuid;

    fn event(event: AgentEvent) -> WorkflowEvent {
        WorkflowEvent {
            timestamp: chrono::Utc::now(),
            workflow_run_id: Uuid::nil(),
            agent_name: "main".into(),
            agent_run_id: Uuid::nil(),
            event,
        }
    }

    fn say(feed: &Feed, text: &str) {
        feed.event(
            &event(AgentEvent::AssistantResponseChunk(text.into())),
            None,
        );
    }

    fn call(feed: &Feed, tool: &str, parameters: &[(&str, Value)]) -> Uuid {
        let id = Uuid::new_v4();
        let parameters: HashMap<String, Value> = parameters
            .iter()
            .map(|(key, value)| (key.to_string(), value.clone()))
            .collect();
        feed.event(
            &event(AgentEvent::ToolCallFormatted {
                tool_call_id: id,
                tool_name: tool.into(),
                parameters,
            }),
            None,
        );
        id
    }

    fn result(feed: &Feed, id: Uuid, tool: &str, output: &str) {
        feed.event(
            &event(AgentEvent::ToolResult {
                tool_call_id: id,
                tool_name: tool.into(),
                output: output.into(),
            }),
            None,
        );
    }

    const ANSWER: &str = "The parser now reads its input as it arrives. Each chunk goes \
through `Parser::feed`, and a finished block is never parsed again.

## What changed

- `Parser::feed` takes a chunk and returns the blocks it completed.
- The open block keeps its state between chunks.

```rust
pub fn feed(&mut self, chunk: &str) -> Vec<Block> {
    self.pending.push_str(chunk);
    let mut done = Vec::new();
    while let Some(end) = self.block_end() {
        let text: String = self.pending.drain(..end).collect();
        done.push(Block::parse(&text));
    }
    done
}
```

The new test streams a document one byte at a time and compares the blocks with a \
parse of the whole text.";

    /// A reply still streaming, with a note the user typed meanwhile that
    /// the agent hasn't read yet.
    pub fn note_mid_reply() -> Feed {
        let feed = Feed::new("main");
        feed.user(
            "Make the parser streaming, and add a test that feeds it a byte at a time.",
            Source::App,
            Delivery::Turn,
        );
        feed.turn_started();
        say(
            &feed,
            "The parser now reads its input as it arrives. Each chunk go",
        );
        feed.note(
            "Keep `parse` as a wrapper for now.",
            Source::App,
            1,
            Vec::new(),
        );
        say(
            &feed,
            "es through `Parser::feed`, and a finished block is never parsed again.",
        );
        feed
    }

    /// A finished turn with each kind of row, and a second one running with
    /// an image a tool returned.
    pub fn representative() -> Feed {
        let feed = Feed::new("main");
        feed.user(
            "Make the parser streaming, and add a test that feeds it a byte at a time.",
            Source::App,
            Delivery::Turn,
        );
        feed.turn_started();
        feed.event(
            &event(AgentEvent::ReasoningChunk(
                "The parser takes a whole string today.".into(),
            )),
            None,
        );
        let read = call(
            &feed,
            "read_file",
            &[
                ("path", "src/parse.rs".into()),
                ("start_line", 1.into()),
                ("end_line", 120.into()),
            ],
        );
        result(&feed, read, "read_file", "1 | pub struct Parser {\n2 | }\n");
        let edit = call(
            &feed,
            "edit_file",
            &[
                ("path", "src/parse.rs".into()),
                (
                    "old_string",
                    "pub fn parse(text: &str) -> Vec<Block> {\n".into(),
                ),
                (
                    "new_string",
                    "pub fn feed(&mut self, chunk: &str) -> Vec<Block> {\n".into(),
                ),
            ],
        );
        result(
            &feed,
            edit,
            "edit_file",
            "Edited src/parse.rs at lines 40-40",
        );
        let test = call(&feed, "shell", &[("command", "cargo test -p parse".into())]);
        result(
            &feed,
            test,
            "shell",
            "exit code 101, 3.1 s\nrunning 12 tests\ntest streams_a_byte_at_a_time ... FAILED\n\
             test result: FAILED. 11 passed; 1 failed",
        );
        let again = call(&feed, "shell", &[("command", "cargo test -p parse".into())]);
        result(
            &feed,
            again,
            "shell",
            "exit code 0, 2.9 s\nrunning 12 tests\ntest result: ok. 12 passed; 0 failed",
        );
        say(&feed, ANSWER);
        feed.reply(ANSWER);
        feed.turn_ended("48 s · $0.212 (session $1.04) · context 61k", None);

        feed.user("Now fuzz it.", Source::App, Delivery::Turn);
        feed.turn_started();
        feed.worker(
            "r1",
            "Write a fuzz target for the parser",
            WorkerChange::Started,
        );
        feed.notice(Level::Warn, "rate limited, retrying in 4 s (attempt 1)");
        // A file read that showed an image (as a resumed session has it).
        let mut read = arko::remote::Item::new(Kind::Call, "read assets/icon.png");
        read.id = 1;
        read.tool = Some("read_file".into());
        read.status = Some(CallStatus::Ok);
        read.images = vec![ImageRef {
            id: 1,
            mime: "image/png".into(),
            label: "assets/icon.png".into(),
            bytes: super::PNG.len(),
        }];
        feed.restore(vec![read]);
        call(
            &feed,
            "shell",
            &[(
                "command",
                "cargo fuzz run parser -- -max_total_time=60".into(),
            )],
        );
        feed
    }
}

/// A note the agent hasn't read, outlined and muted below the reply that
/// goes on streaming above it.
fn pending_note_window(mode: ThemeMode) -> Screenshot {
    use arko_studio::transcript::{AgentFilter, TranscriptView};
    use gpui_kit::AppContext as _;

    let mut app = ScreenshotApp::with_assets(Assets).unwrap();
    let mut settings = Prefs::default();
    settings.appearance.theme_mode = mode;
    settings.appearance.motion = Motion::Off;
    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    let startup = Startup::for_tests(backend)
        .with_prefs(settings)
        .with_fonts(fonts::bundled());
    app.update(|cx| arko_studio::init(startup, cx));
    let feed = feed::note_mid_reply();
    let window = app
        .open_window_with(size(px(900.), px(300.)), 1., |options, cx| {
            gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| TranscriptView::new(&feed, AgentFilter::Main, window, cx))
            })
            .map(|(window, _)| window)
        })
        .unwrap();
    app.capture(window).unwrap()
}

#[test]
fn transcript_pending_note_light() {
    goldens().assert(
        "transcript-pending-note-light",
        &pending_note_window(ThemeMode::Light),
    );
}

#[test]
fn transcript_pending_note_dark() {
    goldens().assert(
        "transcript-pending-note-dark",
        &pending_note_window(ThemeMode::Dark),
    );
}

/// The representative transcript alone in a window `width` wide, its
/// running times read at a fixed moment.
fn transcript_window(mode: ThemeMode, width: f32) -> Screenshot {
    use arko_studio::transcript::{AgentFilter, TranscriptView};
    use gpui_kit::AppContext as _;

    let mut app = ScreenshotApp::with_assets(Assets).unwrap();
    let mut settings = Prefs::default();
    settings.appearance.theme_mode = mode;
    settings.appearance.motion = Motion::Off;
    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    let startup = Startup::for_tests(backend)
        .with_prefs(settings)
        .with_fonts(fonts::bundled());
    app.update(|cx| arko_studio::init(startup, cx));
    let feed = feed::representative();
    // The edit and the failed test show their details.
    let items = feed.subscribe().0.items;
    let open: Vec<u64> = items
        .iter()
        .filter(|item| {
            item.tool.as_deref() == Some("edit_file")
                || item
                    .outcome
                    .as_deref()
                    .is_some_and(|o| o.starts_with("exit"))
        })
        .map(|item| item.id)
        .collect();
    // The call and the worker started a moment ago: they show 12 s.
    let now = chrono::Utc::now().timestamp_millis() + 12_000;
    let window = app
        .open_window_with(size(px(width), px(1500.)), 1., |options, cx| {
            gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| {
                    let mut view = TranscriptView::new(&feed, AgentFilter::Main, window, cx);
                    view.set_clock(move || now, cx);
                    view.set_image_lookup(|_| Some((PNG.into(), "image/png".into())), cx);
                    for item in open {
                        view.set_expanded(item, true, cx);
                    }
                    view
                })
            })
            .map(|(window, _)| window)
        })
        .unwrap();
    app.capture(window).unwrap()
}

#[test]
fn transcript_wide_light() {
    goldens().assert(
        "transcript-wide-light",
        &transcript_window(ThemeMode::Light, 1300.),
    );
}

#[test]
fn transcript_wide_dark() {
    goldens().assert(
        "transcript-wide-dark",
        &transcript_window(ThemeMode::Dark, 1300.),
    );
}

#[test]
fn transcript_narrow_light() {
    goldens().assert(
        "transcript-narrow-light",
        &transcript_window(ThemeMode::Light, 600.),
    );
}

#[test]
fn transcript_narrow_dark() {
    goldens().assert(
        "transcript-narrow-dark",
        &transcript_window(ThemeMode::Dark, 600.),
    );
}

/// A session opened again on a turn that ended, in a window `width` wide,
/// with the pointer on the row `hovered` (a row's name and its entry). The
/// turn ended with a reply, or without one, stopped.
fn ended_turn_window(
    mode: ThemeMode,
    width: f32,
    replied: bool,
    hovered: (&'static str, u64),
) -> Screenshot {
    use arko::remote::{Delivery, Feed, Item, Kind, Level, Source};
    use arko_studio::transcript::{AgentFilter, TranscriptView};
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{AppContext as _, Modifiers, MouseMoveEvent, PlatformInput, point};

    let mut app = ScreenshotApp::with_assets(Assets).unwrap();
    let mut settings = Prefs::default();
    settings.appearance.theme_mode = mode;
    settings.appearance.motion = Motion::Off;
    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    let startup = Startup::for_tests(backend)
        .with_prefs(settings)
        .with_fonts(fonts::bundled());
    app.update(|cx| arko_studio::init(startup, cx));
    let mut message = Item::new(Kind::User, "Name a fruit that keeps well.");
    message.message_index = Some(0);
    message.source = Some(Source::App);
    message.delivery = Some(Delivery::Turn);
    let ending = if replied {
        Item::new(
            Kind::Reply,
            "An apple. Kept cool and dry, it lasts for months.",
        )
    } else {
        Item::new(Kind::Notice, "Stopped.").level(Level::Warn)
    };
    let footer = Item::new(Kind::Turn, "6 s · $0.004 (session $0.01) · context 3k");
    let items = [message, ending, footer]
        .into_iter()
        .zip(1..)
        .map(|(mut item, id)| {
            item.id = id;
            item.turn = Some(1);
            item
        })
        .collect();
    let feed = Feed::new("main");
    feed.restore(items);
    let window = app
        .open_window_with(size(px(width), px(300.)), 1., |options, cx| {
            gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| TranscriptView::new(&feed, AgentFilter::Main, window, cx))
            })
            .map(|(window, _)| window)
        })
        .unwrap();
    app.run_until_parked();
    let _ = app.capture(window).unwrap();
    // The pointer at the row's left, in the window's margin, away from its
    // buttons.
    app.update_window(window, |_, window, cx| {
        let row = window.find(hovered).bounds();
        let position = point(row.left() + px(4.), row.bottom() - px(4.));
        window.dispatch_event(
            PlatformInput::MouseMove(MouseMoveEvent {
                position,
                pressed_button: None,
                modifiers: Modifiers::default(),
            }),
            cx,
        );
    })
    .unwrap();
    app.capture(window).unwrap()
}

/// The latest reply, under the pointer: Regenerate is alone at the right
/// end of its actions.
#[test]
fn transcript_regenerate_light() {
    goldens().assert(
        "transcript-regenerate-light",
        &ended_turn_window(ThemeMode::Light, 1300., true, ("actions", 2)),
    );
}

#[test]
fn transcript_regenerate_dark() {
    goldens().assert(
        "transcript-regenerate-dark",
        &ended_turn_window(ThemeMode::Dark, 1300., true, ("actions", 2)),
    );
}

#[test]
fn transcript_regenerate_narrow_light() {
    goldens().assert(
        "transcript-regenerate-narrow-light",
        &ended_turn_window(ThemeMode::Light, 460., true, ("actions", 2)),
    );
}

#[test]
fn transcript_regenerate_narrow_dark() {
    goldens().assert(
        "transcript-regenerate-narrow-dark",
        &ended_turn_window(ThemeMode::Dark, 460., true, ("actions", 2)),
    );
}

/// A turn stopped before it replied, its footer under the pointer:
/// Regenerate is at the line's right end, where the reply would be.
#[test]
fn transcript_stopped_light() {
    goldens().assert(
        "transcript-stopped-light",
        &ended_turn_window(ThemeMode::Light, 1300., false, ("turn", 3)),
    );
}

#[test]
fn transcript_stopped_dark() {
    goldens().assert(
        "transcript-stopped-dark",
        &ended_turn_window(ThemeMode::Dark, 1300., false, ("turn", 3)),
    );
}

#[test]
fn transcript_stopped_narrow_light() {
    goldens().assert(
        "transcript-stopped-narrow-light",
        &ended_turn_window(ThemeMode::Light, 460., false, ("turn", 3)),
    );
}

#[test]
fn transcript_stopped_narrow_dark() {
    goldens().assert(
        "transcript-stopped-narrow-dark",
        &ended_turn_window(ThemeMode::Dark, 460., false, ("turn", 3)),
    );
}

/// A user's message under the pointer: Edit, Branch and Copy in a row under
/// the bubble, at its right edge.
#[test]
fn transcript_message_actions_light() {
    goldens().assert(
        "transcript-message-actions-light",
        &ended_turn_window(ThemeMode::Light, 1300., true, ("user", 1)),
    );
}

#[test]
fn transcript_message_actions_dark() {
    goldens().assert(
        "transcript-message-actions-dark",
        &ended_turn_window(ThemeMode::Dark, 1300., true, ("user", 1)),
    );
}

#[test]
fn transcript_message_actions_narrow_light() {
    goldens().assert(
        "transcript-message-actions-narrow-light",
        &ended_turn_window(ThemeMode::Light, 460., true, ("user", 1)),
    );
}

#[test]
fn transcript_message_actions_narrow_dark() {
    goldens().assert(
        "transcript-message-actions-narrow-dark",
        &ended_turn_window(ThemeMode::Dark, 460., true, ("user", 1)),
    );
}

/// The representative transcript with a find going on: its matches marked,
/// the current one stronger, and the keyboard's ring on a row.
fn transcript_find_window(mode: ThemeMode) -> Screenshot {
    use arko_studio::transcript::{AgentFilter, TranscriptView};
    use gpui_kit::AppContext as _;
    use gpui_kit::test::TestWindowExt as _;

    let mut app = ScreenshotApp::with_assets(Assets).unwrap();
    let mut settings = Prefs::default();
    settings.appearance.theme_mode = mode;
    settings.appearance.motion = Motion::Off;
    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    let startup = Startup::for_tests(backend)
        .with_prefs(settings)
        .with_fonts(fonts::bundled());
    app.update(|cx| arko_studio::init(startup, cx));
    let feed = feed::representative();
    let now = chrono::Utc::now().timestamp_millis() + 12_000;
    let mut opened = None;
    let window = app
        .open_window_with(size(px(1300.), px(1500.)), 1., |options, cx| {
            gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| {
                    let mut view = TranscriptView::new(&feed, AgentFilter::Main, window, cx);
                    view.set_clock(move || now, cx);
                    view.set_image_lookup(|_| Some((PNG.into(), "image/png".into())), cx);
                    view
                })
            })
            .map(|(window, view)| {
                opened = Some(view);
                window
            })
        })
        .unwrap();
    let view = opened.unwrap();
    app.run_until_parked();
    app.update_window(window, |_, window, cx| {
        view.update(cx, |view, cx| {
            view.find("parse", cx);
            view.focus_rows(window, cx);
        });
        // From the keyboard: its ring shows.
        window.press("up", cx);
    })
    .unwrap();
    app.capture(window).unwrap()
}

#[test]
fn transcript_find_light() {
    goldens().assert(
        "transcript-find-light",
        &transcript_find_window(ThemeMode::Light),
    );
}

#[test]
fn transcript_find_dark() {
    goldens().assert(
        "transcript-find-dark",
        &transcript_find_window(ThemeMode::Dark),
    );
}

/// The composer of an agent's new session, with the files an "@" mentions
/// listed above it.
fn mention_window(mode: ThemeMode) -> Screenshot {
    use gpui_kit::test::TestWindowExt as _;

    // A folder of a fixed name: the window shows it.
    let root = std::env::temp_dir().join("arko-mention-list");
    let _ = std::fs::remove_dir_all(&root);
    for dir in ["src/parser", "docs"] {
        std::fs::create_dir_all(root.join(dir)).unwrap();
    }
    for file in [
        "README.md",
        "src/parse.rs",
        "src/parser/mod.rs",
        "src/main.rs",
        "docs/parsing.md",
    ] {
        std::fs::write(root.join(file), "").unwrap();
    }
    let mut app = ScreenshotApp::with_assets(Assets).unwrap();
    let mut settings = Prefs::default();
    settings.appearance.theme_mode = mode;
    settings.appearance.motion = Motion::Off;
    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    let startup = Startup::for_tests(backend)
        .with_prefs(settings)
        .with_fonts(fonts::bundled());
    app.update(|cx| arko_studio::init(startup, cx));
    let mut opened = None;
    let window = app
        .open_window_with(size(px(900.), px(600.)), 1., |options, cx| {
            workspace::open_window(options, cx).map(|(window, workspace)| {
                opened = Some(workspace);
                window
            })
        })
        .unwrap();
    let workspace = opened.unwrap();
    app.update_window(window, |_, window, cx| {
        let view = workspace.read(cx).session_view().clone();
        view.update(cx, |view, cx| {
            view.set_draft_folder(root.clone(), cx);
            view.focus_composer(window, cx);
        });
        window.input("Look at the @pars", cx);
    })
    .unwrap();
    // The folder is listed and ranked off the UI thread.
    app.run_until_parked();
    app.capture(window).unwrap()
}

#[test]
fn mention_list_light() {
    goldens().assert("mention-list-light", &mention_window(ThemeMode::Light));
}

#[test]
fn mention_list_dark() {
    goldens().assert("mention-list-dark", &mention_window(ThemeMode::Dark));
}

/// The workers panel with workers in each state: two running (one past an
/// hour), one waiting, one that finished and one that failed a moment ago,
/// and one finished long ago in the collapsed "Finished" group.
fn workers_window(mode: ThemeMode) -> Screenshot {
    use arko::remote::{AgentStatus, WorkerState, WorkerSummary};
    use arko_studio::workers_panel::{PanelSource, WorkersPanel};
    use gpui_kit::AppContext as _;

    let mut app = ScreenshotApp::with_assets(Assets).unwrap();
    let mut settings = Prefs::default();
    settings.appearance.theme_mode = mode;
    settings.appearance.motion = Motion::Off;
    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    let startup = Startup::for_tests(backend)
        .with_prefs(settings)
        .with_fonts(fonts::bundled());
    app.update(|cx| arko_studio::init(startup, cx));
    const NOW: i64 = 1_760_000_000_000;
    const MINUTE: i64 = 60_000;
    let worker =
        |id: &str, title: &str, state, calls, started, ended, last: Option<&str>| WorkerSummary {
            id: id.into(),
            title: title.into(),
            state,
            tool_calls: calls,
            started_at: started,
            ended_at: ended,
            ran_ms: started.zip(ended).map_or(0, |(s, e)| (e - s).max(0) as u64),
            running_since: started.filter(|_| state == WorkerState::Running),
            last: last.map(Into::into),
            report: None,
        };
    let mut finished = worker(
        "r4",
        "Update the changelog",
        WorkerState::Finished,
        7,
        Some(NOW - 6 * MINUTE),
        Some(NOW - 2 * MINUTE),
        None,
    );
    finished.report = Some("Wrote the changelog for 0.4.".into());
    let workers = vec![
        worker(
            "r1",
            "Fix the flaky parser test",
            WorkerState::Running,
            18,
            Some(NOW - 4 * MINUTE - 12_000),
            None,
            Some("$ cargo test -p parse"),
        ),
        worker(
            "r2",
            "Profile the renderer under load",
            WorkerState::Running,
            142,
            Some(NOW - 65 * MINUTE),
            None,
            Some("read src/render/scene.rs:120-180"),
        ),
        worker(
            "r3",
            "Write the migration guide",
            WorkerState::Queued,
            0,
            None,
            None,
            None,
        ),
        finished,
        worker(
            "r5",
            "Bisect the slowdown",
            WorkerState::Failed,
            3,
            Some(NOW - 3 * MINUTE),
            Some(NOW - MINUTE),
            Some("the model failed: overloaded"),
        ),
        worker(
            "r6",
            "Rename the config keys",
            WorkerState::Finished,
            11,
            Some(NOW - 40 * MINUTE),
            Some(NOW - 20 * MINUTE),
            None,
        ),
    ];
    let agent = |id: &str, tokens, window| AgentStatus {
        id: id.into(),
        context_tokens: Some(tokens),
        context_window: Some(window),
        handover_at: None,
        compacts: false,
    };
    let source = PanelSource {
        workers,
        agents: vec![
            agent("r1", 88_000, 200_000),
            agent("r2", 152_000, 1_000_000),
        ],
        feed: None,
        images: None,
        key: None,
    };
    let window = app
        .open_window_with(size(px(352.), px(420.)), 1., |options, cx| {
            gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| {
                    let mut panel = WorkersPanel::new(window, cx);
                    panel.set_source(source, cx);
                    panel.set_clock(|| NOW, cx);
                    panel
                })
            })
            .map(|(window, _)| window)
        })
        .unwrap();
    app.capture(window).unwrap()
}

#[test]
fn workers_light() {
    goldens().assert("workers-light", &workers_window(ThemeMode::Light));
}

#[test]
fn workers_dark() {
    goldens().assert("workers-dark", &workers_window(ThemeMode::Dark));
}

/// The dialog that asks before an edit in an agent session removes the
/// messages after the message, over a new window.
fn edit_dialog_window(mode: ThemeMode) -> Screenshot {
    use arko_studio::editing::{self, Change};

    let mut app = ScreenshotApp::with_assets(Assets).unwrap();
    let mut settings = Prefs::default();
    settings.appearance.theme_mode = mode;
    settings.appearance.motion = Motion::Off;
    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    let startup = Startup::for_tests(backend)
        .with_prefs(settings)
        .with_fonts(fonts::bundled());
    app.update(|cx| arko_studio::init(startup, cx));
    let window = app
        .open_window_with(size(px(900.), px(560.)), 1., |options, cx| {
            workspace::open_window(options, cx).map(|(window, _)| window)
        })
        .unwrap();
    app.update_window(window, |_, window, cx| {
        editing::confirm(Change::Edit, 3, true, window, cx, |_, _, _| {})
    })
    .unwrap();
    app.capture(window).unwrap()
}

#[test]
fn edit_dialog_light() {
    goldens().assert("edit-dialog-light", &edit_dialog_window(ThemeMode::Light));
}

#[test]
fn edit_dialog_dark() {
    goldens().assert("edit-dialog-dark", &edit_dialog_window(ThemeMode::Dark));
}

/// About Arko Studio over a window `width` × `height` logical pixels, with
/// the built-in fonts, so all three credits show. `scroll` turns the mouse
/// wheel that many pixels down over the dialog.
fn about_window(mode: ThemeMode, width: f32, height: f32, scroll: f32) -> Screenshot {
    use gpui_kit::{InputEvent as _, ScrollDelta, ScrollWheelEvent, point};

    let mut app = ScreenshotApp::with_assets(Assets).unwrap();
    let mut settings = Prefs::default();
    settings.appearance.theme_mode = mode;
    settings.appearance.motion = Motion::Off;
    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    let startup = Startup::for_tests(backend)
        .with_prefs(settings)
        .with_fonts(fonts::bundled());
    app.update(|cx| arko_studio::init(startup, cx));
    let window = app
        .open_window_with(size(px(width), px(height)), 1., |options, cx| {
            workspace::open_window(options, cx).map(|(window, _)| window)
        })
        .unwrap();
    app.update_window(window, |_, window, cx| arko_studio::about::open(window, cx))
        .unwrap();
    if scroll > 0. {
        // A frame first, so the wheel finds the dialog's body.
        app.capture(window).unwrap();
        app.update_window(window, |_, window, cx| {
            window.dispatch_event(
                ScrollWheelEvent {
                    position: point(px(width / 2.), px(height / 2.)),
                    delta: ScrollDelta::Pixels(point(px(0.), px(-scroll))),
                    ..Default::default()
                }
                .to_platform_input(),
                cx,
            );
        })
        .unwrap();
        app.run_until_parked();
    }
    app.capture(window).unwrap()
}

#[test]
fn about_light() {
    goldens().assert(
        "about-light",
        &about_window(ThemeMode::Light, 900., 560., 0.),
    );
}

#[test]
fn about_dark() {
    goldens().assert("about-dark", &about_window(ThemeMode::Dark, 900., 560., 0.));
}

/// At the window's smallest size the dialog narrows with it, and the
/// credits wrap inside its padding.
#[test]
fn about_narrow_light() {
    goldens().assert(
        "about-narrow-light",
        &about_window(ThemeMode::Light, 400., 300., 0.),
    );
}

#[test]
fn about_narrow_dark() {
    goldens().assert(
        "about-narrow-dark",
        &about_window(ThemeMode::Dark, 400., 300., 0.),
    );
}

/// The dialog's body scrolls to the rest of the credits and the links when
/// the window is too short for it.
#[test]
fn about_narrow_scrolled_light() {
    goldens().assert(
        "about-narrow-scrolled-light",
        &about_window(ThemeMode::Light, 400., 300., 400.),
    );
}

#[test]
fn about_narrow_scrolled_dark() {
    goldens().assert(
        "about-narrow-scrolled-dark",
        &about_window(ThemeMode::Dark, 400., 300., 400.),
    );
}

/// The status bar's figures, at a fixed moment.
mod status {
    use std::time::Duration;

    use arko::remote::{
        Activity, CostPart, LastRequest, Link, LinkKind, Page, PlanStatus, PlanWindow, Reading,
    };

    /// The moment the bars are drawn at, in Unix milliseconds.
    pub const NOW: i64 = 1_800_000_000_000;

    fn lan(url: &str, interface: &str) -> Link {
        Link {
            url: url.into(),
            interface: Some(interface.into()),
            kind: LinkKind::Lan,
            loopback: false,
        }
    }

    /// A page shared on every interface.
    pub fn page() -> Page {
        Page {
            addr: "0.0.0.0:8765".parse().unwrap(),
            links: vec![
                lan(
                    "http://192.168.1.20:8765/#key=Xq3v9TzK1bR7mWc2LpYd8sNf",
                    "wlan0",
                ),
                Link {
                    url: "http://100.101.7.12:8765/#key=Xq3v9TzK1bR7mWc2LpYd8sNf".into(),
                    interface: Some("tailscale0".into()),
                    kind: LinkKind::Tailscale,
                    loopback: false,
                },
                Link {
                    url: "http://127.0.0.1:8765/#key=Xq3v9TzK1bR7mWc2LpYd8sNf".into(),
                    interface: None,
                    kind: LinkKind::Loopback,
                    loopback: true,
                },
            ],
            note: None,
        }
    }

    /// A session at work: two workers, outside access, the web page, a plan
    /// near its 5-hour limit, a cost and a context of 142k.
    pub fn busy() -> Reading {
        let mut reading = Reading {
            cost: 1.2,
            context_tokens: Some(142_318),
            context_window: Some(1_000_000),
            workers_running: vec!["r1".into(), "r2".into()],
            page: Some(page()),
            plans: vec![PlanStatus {
                plan: "Claude plan".into(),
                windows: vec![
                    PlanWindow {
                        label: "5 h".into(),
                        used_percent: 84.,
                        resets_at: None,
                    },
                    PlanWindow {
                        label: "7 d".into(),
                        used_percent: 41.,
                        resets_at: None,
                    },
                ],
                text: String::new(),
            }],
            last_request: Some(LastRequest {
                at: NOW,
                model: "claude-opus-5-5".into(),
                sizes: Some(Default::default()),
                bytes_per_token: 4.,
            }),
            ..Reading::default()
        };
        reading.settings.outside = true;
        reading.settings.handover_at = 950_000;
        reading.costs.main = CostPart {
            billed: 1.0,
            list_price: 1.0,
            input_tokens: 420_000,
            output_tokens: 18_000,
            calls: 14,
        };
        reading.costs.workers = CostPart {
            billed: 0.2,
            list_price: 0.2,
            input_tokens: 90_000,
            output_tokens: 4_000,
            calls: 6,
        };
        if let Some(sizes) = reading
            .last_request
            .as_mut()
            .and_then(|request| request.sizes.as_mut())
        {
            sizes.system_prompt = 30_000;
            sizes.tool_definitions = 42_000;
            sizes.user_text = 60_000;
            sizes.assistant_text = 120_000;
            sizes.tool_calls = 40_000;
            sizes.tool_results = 270_000;
            sizes.image_tokens = 3_000;
        }
        reading.cache.read = Some(140_100);
        reading.cache.written = Some(2_218);
        reading
    }

    /// A turn running for 1:23.
    pub fn working() -> Activity {
        Activity {
            working: true,
            since: Some(NOW - 83_000),
            ..Activity::default()
        }
    }

    /// An idle session on a plan near its handover size, its 5-minute cache
    /// in its last minute.
    pub fn idle() -> Reading {
        let mut reading = Reading {
            api_price: Some(3.4),
            context_tokens: Some(871_204),
            context_window: Some(1_000_000),
            ..Reading::default()
        };
        reading.settings.handover_at = 950_000;
        reading.costs.main = CostPart {
            billed: 0.,
            list_price: 3.4,
            input_tokens: 2_400_000,
            output_tokens: 31_000,
            calls: 22,
        };
        reading.cache.lifetime = Some(Duration::from_secs(300));
        reading.cache.refreshed_at = Some(NOW - 258_000);
        reading.cache.tokens = Some(871_204);
        reading.cache.on_plan = Some("your Claude plan");
        reading
    }
}

/// A status bar alone in a window `width` wide.
fn status_window(
    mode: ThemeMode,
    width: f32,
    reading: arko::remote::Reading,
    activity: arko::remote::Activity,
) -> Screenshot {
    use arko_studio::layout::{NARROW_BELOW, WidthClass};
    use arko_studio::status_bar::StatusBar;
    use gpui_kit::AppContext as _;

    let mut app = ScreenshotApp::with_assets(Assets).unwrap();
    let mut settings = Prefs::default();
    settings.appearance.theme_mode = mode;
    settings.appearance.motion = Motion::Off;
    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    let startup = Startup::for_tests(backend)
        .with_prefs(settings)
        .with_fonts(fonts::bundled());
    app.update(|cx| arko_studio::init(startup, cx));
    let window = app
        .open_window_with(size(px(width), px(28.)), 1., |options, cx| {
            gpui_kit::open_window(options, cx, |_, cx| {
                cx.new(|cx| {
                    let mut bar =
                        StatusBar::fixed(reading, activity, std::rc::Rc::new(|| status::NOW));
                    if width < NARROW_BELOW {
                        bar.set_width(WidthClass::Narrow, cx);
                    }
                    bar
                })
            })
            .map(|(window, _)| window)
        })
        .unwrap();
    app.capture(window).unwrap()
}

#[test]
fn status_busy_light() {
    goldens().assert(
        "status-busy-light",
        &status_window(ThemeMode::Light, 1300., status::busy(), status::working()),
    );
}

#[test]
fn status_busy_dark() {
    goldens().assert(
        "status-busy-dark",
        &status_window(ThemeMode::Dark, 1300., status::busy(), status::working()),
    );
}

#[test]
fn status_idle_light() {
    goldens().assert(
        "status-idle-light",
        &status_window(ThemeMode::Light, 1300., status::idle(), Default::default()),
    );
}

#[test]
fn status_idle_dark() {
    goldens().assert(
        "status-idle-dark",
        &status_window(ThemeMode::Dark, 1300., status::idle(), Default::default()),
    );
}

#[test]
fn status_narrow_light() {
    goldens().assert(
        "status-narrow-light",
        &status_window(ThemeMode::Light, 600., status::busy(), status::working()),
    );
}

#[test]
fn status_narrow_dark() {
    goldens().assert(
        "status-narrow-dark",
        &status_window(ThemeMode::Dark, 600., status::busy(), status::working()),
    );
}

/// The status bar's popovers side by side: the context's, the cost's and
/// the web page's.
struct StatusPopovers {
    reading: Arc<arko::remote::Reading>,
}

impl gpui_kit::Render for StatusPopovers {
    fn render(
        &mut self,
        _: &mut gpui_kit::Window,
        cx: &mut gpui_kit::Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        use arko::details::{CostReport, CostShare};
        use arko_studio::context_ring::ContextDetails;
        use arko_studio::status_bar::CostDetails;
        use arko_studio::web_page::WebPagePanel;
        use gpui_kit::component::{ActiveTheme as _, h_flex};
        use gpui_kit::{IntoElement as _, ParentElement as _, Styled as _, div};

        let theme = cx.theme();
        let card = |content: gpui_kit::AnyElement| {
            div()
                .p_3()
                .bg(theme.popover)
                .text_color(theme.popover_foreground)
                .border_1()
                .border_color(theme.border)
                .rounded(theme.radius_lg)
                .child(content)
        };
        let share = |billed: f64, input: usize, output: usize| CostShare {
            billed,
            api: billed,
            input,
            output,
            plan: None,
        };
        let cost = CostReport {
            main: share(1.0, 420_000, 18_000),
            workers: share(0.2, 90_000, 4_000),
            plans: Vec::new(),
            embeddings: Some((120_000, 0.01)),
        };
        h_flex()
            .size_full()
            .items_start()
            .gap_4()
            .p_4()
            .bg(theme.background)
            .text_color(theme.foreground)
            .font_family(theme.font_family.clone())
            .child(card(
                ContextDetails::new(&self.reading, status::NOW).into_any_element(),
            ))
            .child(card(CostDetails::new(Some(cost)).into_any_element()))
            .child(card(
                WebPagePanel::new(&status::page())
                    .explained()
                    .into_any_element(),
            ))
    }
}

fn status_popovers(mode: ThemeMode) -> Screenshot {
    use gpui_kit::AppContext as _;

    let mut app = ScreenshotApp::with_assets(Assets).unwrap();
    let mut settings = Prefs::default();
    settings.appearance.theme_mode = mode;
    settings.appearance.motion = Motion::Off;
    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    let startup = Startup::for_tests(backend)
        .with_prefs(settings)
        .with_fonts(fonts::bundled());
    app.update(|cx| arko_studio::init(startup, cx));
    let reading = Arc::new(status::busy());
    let window = app
        .open_window_with(size(px(1100.), px(640.)), 1., |options, cx| {
            gpui_kit::open_window(options, cx, |_, cx| cx.new(|_| StatusPopovers { reading }))
                .map(|(window, _)| window)
        })
        .unwrap();
    app.capture(window).unwrap()
}

#[test]
fn status_popovers_light() {
    goldens().assert("status-popovers-light", &status_popovers(ThemeMode::Light));
}

#[test]
fn status_popovers_dark() {
    goldens().assert("status-popovers-dark", &status_popovers(ThemeMode::Dark));
}

/// The last cache reminder, at 0:42 before the cache lapses, over an empty
/// window.
fn cache_reminder(mode: ThemeMode) -> Screenshot {
    use arko_studio::cache_reminders::{Countdown, ReminderClock, last_reminder};
    use gpui_kit::AppContext as _;
    use gpui_kit::component::WindowExt as _;

    let mut app = ScreenshotApp::with_assets(Assets).unwrap();
    let mut settings = Prefs::default();
    settings.appearance.theme_mode = mode;
    settings.appearance.motion = Motion::Off;
    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    let startup = Startup::for_tests(backend)
        .with_prefs(settings)
        .with_fonts(fonts::bundled());
    app.update(|cx| {
        arko_studio::init(startup, cx);
        cx.set_global(ReminderClock(std::rc::Rc::new(|| status::NOW)));
    });
    let window = app
        .open_window_with(size(px(560.), px(240.)), 1., |options, cx| {
            gpui_kit::open_window(options, cx, |_, cx| cx.new(|_| gpui_kit::Empty))
                .map(|(window, _)| window)
        })
        .unwrap();
    app.update(|cx| {
        window
            .update(cx, |_, window, cx| {
                let countdown = cx.new(|cx| {
                    Countdown::new(
                        "Split the engine out of main.rs".into(),
                        "Refresh ≈ $0.04 · Letting it expire re-caches ≈ $0.90".into(),
                        status::NOW + 42_000,
                        cx,
                    )
                });
                window.push_notification(last_reminder(1, countdown, |_, _| {}), cx);
            })
            .unwrap();
    });
    app.capture(window).unwrap()
}

#[test]
fn cache_reminder_light() {
    goldens().assert("cache-reminder-light", &cache_reminder(ThemeMode::Light));
}

#[test]
fn cache_reminder_dark() {
    goldens().assert("cache-reminder-dark", &cache_reminder(ThemeMode::Dark));
}

/// A session's info, as its popover shows it.
struct InfoCard {
    info: gpui_kit::Entity<arko_studio::session_info::SessionInfo>,
}

impl gpui_kit::Render for InfoCard {
    fn render(
        &mut self,
        _: &mut gpui_kit::Window,
        cx: &mut gpui_kit::Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        use gpui_kit::component::ActiveTheme as _;
        use gpui_kit::{ParentElement as _, Styled as _, div};

        let theme = cx.theme();
        div()
            .size_full()
            .p_4()
            .bg(theme.background)
            .text_color(theme.foreground)
            .font_family(theme.font_family.clone())
            .child(
                div()
                    .p_3()
                    .bg(theme.popover)
                    .text_color(theme.popover_foreground)
                    .border_1()
                    .border_color(theme.border)
                    .rounded(theme.radius_lg)
                    .child(self.info.clone()),
            )
    }
}

fn session_info(mode: ThemeMode) -> Screenshot {
    use arko::details::{
        HandoverFacts, McpFacts, McpServerFacts, ModelFacts, SessionFacts, SkillFacts, SkillsFacts,
        WorkersFacts,
    };
    use arko_studio::backend::Mode;
    use arko_studio::session_info::{Facts, Preview, SessionInfo};
    use gpui_kit::AppContext as _;
    use std::path::PathBuf;

    let mut app = ScreenshotApp::with_assets(Assets).unwrap();
    let mut settings = Prefs::default();
    settings.appearance.theme_mode = mode;
    settings.appearance.motion = Motion::Off;
    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    let startup = Startup::for_tests(backend)
        .with_prefs(settings)
        .with_fonts(fonts::bundled());
    app.update(|cx| arko_studio::init(startup, cx));

    let mut reading = arko::remote::Reading {
        context_window: Some(1_000_000),
        ..Default::default()
    };
    reading.settings.outside = true;
    reading.settings.handover_at = 950_000;
    reading.settings.workers_handover_at = 200_000;
    reading.settings.background_after = 600;
    reading.settings.text_replies = true;
    reading.settings.extra_rules = arko::ExtraRules::Small;
    reading.settings.worker_project_instructions = true;
    reading.models.started = true;
    reading.models.has_workers = true;
    let preview = Preview {
        title: "Split the engine out of main.rs".into(),
        mode: Mode::Agent,
        workspace: Some(PathBuf::from("/home/ada/src/arko")),
        id: "20261016-091200-a1b2".into(),
        dir: Some(PathBuf::from(
            "/home/ada/.local/share/arko/sessions/20261016-091200-a1b2",
        )),
        reading: Arc::new(reading),
        tools: vec![
            "2 MCP servers".into(),
            "2 skills".into(),
            "indexes arko (code, monitored), docs (docs)".into(),
        ],
    };
    let model = |shown: &str| ModelFacts {
        model: "claude-opus-5-5".into(),
        shown: shown.into(),
        effort: arko::Effort::High,
        fast: false,
        cache_1h: false,
    };
    let facts = Facts {
        session: SessionFacts {
            id: preview.id.clone(),
            dir: preview.dir.clone().unwrap(),
            workspace: PathBuf::from("/home/ada/src/arko"),
            title: Some(preview.title.clone()),
            created: chrono::DateTime::from_timestamp(1_800_000_000, 0).unwrap(),
            main: model("claude-opus-5-5 (high effort) on the Anthropic API"),
            workers: Some(WorkersFacts {
                model: model("claude-sonnet-5-5 (high effort) on the Anthropic API"),
                max_running: None,
            }),
            handover: HandoverFacts {
                main_at: 950_000,
                main_compacts: false,
                workers_at: 200_000,
                workers_compact: false,
            },
            text_replies: true,
            extra_rules: arko::ExtraRules::Small,
            cc_compact: false,
            project_files: arko::ProjectFiles::One,
            project_workers: Some(true),
            outside: true,
            background_after: 600,
            show_thinking: false,
            hide_thinking: false,
        },
        mcp: McpFacts {
            servers: vec![
                McpServerFacts {
                    name: "files".into(),
                    shown: "files-server".into(),
                    file: PathBuf::from("/home/ada/src/arko/.mcp.json"),
                    tools: Ok(vec!["read".into(), "write".into(), "list".into()]),
                },
                McpServerFacts {
                    name: "browser".into(),
                    shown: "browser-server".into(),
                    file: PathBuf::from("/home/ada/src/arko/.mcp.json"),
                    tools: Err("no such program: browser-server".into()),
                },
            ],
            ..McpFacts::default()
        },
        skills: Ok(SkillsFacts {
            skills: ["release-notes", "docs-style"]
                .into_iter()
                .map(|name| SkillFacts {
                    name: name.into(),
                    description: String::new(),
                    always: false,
                })
                .collect(),
            folders: Vec::new(),
            own_folder: PathBuf::new(),
        }),
    };
    let window = app
        .open_window_with(size(px(560.), px(1300.)), 1., |options, cx| {
            gpui_kit::open_window(options, cx, |window, cx| {
                let info = cx.new(|cx| {
                    let mut info = SessionInfo::new(window, cx);
                    info.set_preview(preview, facts, window, cx);
                    info
                });
                cx.new(|_| InfoCard { info })
            })
            .map(|(window, _)| window)
        })
        .unwrap();
    app.capture(window).unwrap()
}

#[test]
fn session_info_light() {
    goldens().assert("session-info-light", &session_info(ThemeMode::Light));
}

#[test]
fn session_info_dark() {
    goldens().assert("session-info-dark", &session_info(ThemeMode::Dark));
}

/// The open session's name being edited in the sidebar. The field alone shows
/// focus, and the row keeps its selected look without a ring of its own.
fn renaming(mode: ThemeMode) -> Screenshot {
    use arko_studio::backend::SessionId;
    use arko_studio::menus;
    use gpui_kit::test::TestWindowExt as _;

    let mut app = ScreenshotApp::with_assets(Assets).unwrap();
    let mut settings = Prefs::default();
    settings.appearance.theme_mode = mode;
    settings.appearance.motion = Motion::Off;
    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    let startup = Startup::for_tests(backend)
        .with_prefs(settings)
        .with_fonts(fonts::bundled());
    app.update(|cx| arko_studio::init(startup, cx));
    let mut workspace = None;
    let window = app
        .open_window_with(size(px(900.), px(360.)), 1., |options, cx| {
            workspace::open_window(options, cx).map(|(window, opened)| {
                workspace = Some(opened);
                window
            })
        })
        .unwrap();
    let workspace = workspace.unwrap();
    app.update_window(window, |_, window, cx| {
        window.activate_window();
        let fonts = SessionId("20261016-080000-a7b8".into());
        workspace.update(cx, |workspace, cx| {
            workspace.open_session(fonts, window, cx)
        });
    })
    .unwrap();
    app.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.dispatch_action(Box::new(menus::FocusSessionFilter), cx);
    })
    .unwrap();
    for key in ["down", "f2"] {
        app.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            window.press(key, cx);
        })
        .unwrap();
    }
    app.capture(window).unwrap()
}

#[test]
fn renaming_light() {
    goldens().assert("renaming-light", &renaming(ThemeMode::Light));
}

#[test]
fn renaming_dark() {
    goldens().assert("renaming-dark", &renaming(ThemeMode::Dark));
}
