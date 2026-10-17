//! Golden screenshots of Arko Studio's window, rendered headlessly by the GPUI
//! fork's gpui-pre-screenshot on Mesa's software Vulkan driver, in Work Sans
//! and JetBrains Mono. The goldens live in `tests/screenshots`. On a mismatch
//! the actual image and a diff go to `tests/screenshots/failures`, and
//! `UPDATE_GOLDENS=1` rewrites the goldens.
#![cfg(target_os = "linux")]

use std::sync::Arc;
use std::time::SystemTime;

use arko_studio::backend::FakeBackend;
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

/// The workers panel with workers in each state: two running (one past an
/// hour), one waiting, one that finished and one that failed a moment ago,
/// and one finished long ago in the collapsed "Finished" group.
fn workers_window(mode: ThemeMode) -> Screenshot {
    use arko::remote::{AgentStatus, WorkerState, WorkerSummary};
    use arko_studio::workers::WorkerTimes;
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
    let mut times = WorkerTimes::default();
    for worker in &workers {
        times.observe(worker);
    }
    let agent = |id: &str, tokens, window| AgentStatus {
        id: id.into(),
        context_tokens: Some(tokens),
        context_window: Some(window),
        handover_at: None,
        compacts: false,
    };
    let source = PanelSource {
        workers,
        times,
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
