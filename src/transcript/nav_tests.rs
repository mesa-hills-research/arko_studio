//! The transcript from the keyboard, on a real feed in a test window.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::SystemTime;

use arko::remote::{Delivery, Feed, Source};
use arko_agents::AgentEvent;
use arko_workflows::WorkflowEvent;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, Entity, Point, TestAppContext, WindowBounds,
    WindowOptions, px, size,
};
use serde_json::Value;
use uuid::Uuid;

use crate::Startup;
use crate::backend::FakeBackend;
use crate::transcript::model::RowKey;
use crate::transcript::{AgentFilter, TranscriptEvent, TranscriptView};

fn event(event: AgentEvent) -> WorkflowEvent {
    WorkflowEvent {
        timestamp: chrono::Utc::now(),
        workflow_run_id: Uuid::nil(),
        agent_name: "main".into(),
        agent_run_id: Uuid::nil(),
        event,
    }
}

fn command(feed: &Feed, line: &str) {
    let id = Uuid::new_v4();
    let parameters: HashMap<String, Value> = [("command".to_string(), Value::from(line))]
        .into_iter()
        .collect();
    feed.event(
        &event(AgentEvent::ToolCallFormatted {
            tool_call_id: id,
            tool_name: "shell".into(),
            parameters,
        }),
        None,
    );
    feed.event(
        &event(AgentEvent::ToolResult {
            tool_call_id: id,
            tool_name: "shell".into(),
            output: "exit code 0, 0.1 s\nok".into(),
        }),
        None,
    );
}

/// A message, a thinking, a command and a reply.
fn short() -> Feed {
    let feed = Feed::new("main");
    feed.user("Run the tests.", Source::App, Delivery::Turn);
    feed.turn_started();
    feed.event(
        &event(AgentEvent::ReasoningChunk("The tests first.".into())),
        None,
    );
    command(&feed, "cargo test");
    feed.event(
        &event(AgentEvent::AssistantResponseChunk("All pass.".into())),
        None,
    );
    feed.reply("All pass.");
    feed.turn_ended("3 s", None);
    feed
}

/// Forty messages: more than a screen.
fn long() -> Feed {
    let feed = Feed::new("main");
    for n in 0..40 {
        feed.user(&format!("Message {n}."), Source::App, Delivery::Note);
    }
    feed
}

struct Rows {
    window: AnyWindowHandle,
    view: Entity<TranscriptView>,
    events: Rc<RefCell<Vec<TranscriptEvent>>>,
}

fn open(cx: &mut TestAppContext, feed: Feed, height: f32) -> Rows {
    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    cx.update(|cx| crate::init(Startup::for_tests(backend), cx));
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds {
            origin: Point::default(),
            size: size(px(800.), px(height)),
        })),
        ..Default::default()
    };
    let (window, view) = cx
        .update(|cx| {
            gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| TranscriptView::new(&feed, AgentFilter::Main, window, cx))
            })
        })
        .unwrap();
    cx.update_window(window, |_, window, _| window.activate_window())
        .unwrap();
    let events: Rc<RefCell<Vec<TranscriptEvent>>> = Rc::default();
    cx.update(|cx| {
        let events = events.clone();
        cx.subscribe(&view, move |_, event: &TranscriptEvent, _| {
            events.borrow_mut().push(event.clone())
        })
        .detach();
    });
    let rows = Rows {
        window,
        view,
        events,
    };
    rows.draw(cx);
    rows
}

impl Rows {
    fn draw(&self, cx: &mut TestAppContext) {
        for _ in 0..4 {
            cx.run_until_parked();
            cx.update_window(self.window, |_, window, cx| window.render_frame(cx))
                .unwrap();
        }
        cx.run_until_parked();
    }

    fn focus(&self, cx: &mut TestAppContext) {
        let view = self.view.clone();
        cx.update_window(self.window, |_, window, cx| {
            view.update(cx, |view, cx| view.focus_rows(window, cx))
        })
        .unwrap();
        self.draw(cx);
    }

    fn press(&self, cx: &mut TestAppContext, key: &str) {
        cx.update_window(self.window, |_, window, cx| window.press(key, cx))
            .unwrap();
        self.draw(cx);
    }

    fn cursor(&self, cx: &mut TestAppContext) -> Option<RowKey> {
        cx.read(|cx| self.view.read(cx).cursor)
    }

    fn cursor_index(&self, cx: &mut TestAppContext) -> Option<usize> {
        cx.read(|cx| self.view.read(cx).cursor_index())
    }

    fn rows(&self, cx: &mut TestAppContext) -> Vec<RowKey> {
        cx.read(|cx| self.view.read(cx).rows.iter().map(|row| row.key).collect())
    }
}

#[gpui_kit::test]
fn the_keyboard_moves_through_the_rows_and_opens_them(cx: &mut TestAppContext) {
    let rows = open(cx, short(), 900.);
    rows.focus(cx);
    // It starts on the last row on screen: the turn's footer.
    let keys = rows.rows(cx);
    assert_eq!(rows.cursor(cx), keys.last().copied());
    // Up onto the reply's actions: they show, as under the pointer.
    rows.press(cx, "up");
    let Some(RowKey::Actions(reply)) = rows.cursor(cx) else {
        panic!("on the reply's actions: {:?}", rows.cursor(cx));
    };
    assert_eq!(cx.read(|cx| rows.view.read(cx).hovered), Some(reply));
    // Up to the call, which Enter opens.
    while !matches!(rows.cursor(cx), Some(RowKey::Call(_))) {
        rows.press(cx, "up");
    }
    let Some(RowKey::Call(call)) = rows.cursor(cx) else {
        unreachable!()
    };
    rows.press(cx, "enter");
    assert_eq!(
        cx.read(|cx| rows.view.read(cx).ui.get(&call).and_then(|ui| ui.expanded)),
        Some(true)
    );
    // Up to the thinking: Enter opens it, and on its text closes it again.
    rows.press(cx, "up");
    let Some(RowKey::Thinking(thinking)) = rows.cursor(cx) else {
        panic!("on the thinking: {:?}", rows.cursor(cx));
    };
    rows.press(cx, "enter");
    let shows = |cx: &mut TestAppContext| {
        rows.rows(cx)
            .iter()
            .any(|key| matches!(key, RowKey::Block { id, .. } if *id == thinking))
    };
    assert!(shows(cx));
    rows.press(cx, "down");
    assert!(matches!(rows.cursor(cx), Some(RowKey::Block { id, .. }) if id == thinking));
    rows.press(cx, "enter");
    assert!(!shows(cx));
    assert_eq!(rows.cursor(cx), Some(RowKey::Thinking(thinking)));
    rows.press(cx, "up");
    assert!(matches!(rows.cursor(cx), Some(RowKey::User(_))));
    // Esc gives the keyboard back.
    rows.press(cx, "escape");
    assert_eq!(rows.events.borrow().last(), Some(&TranscriptEvent::Leave));
}

#[gpui_kit::test]
fn the_arrows_start_from_the_screen_after_it_scrolled(cx: &mut TestAppContext) {
    let rows = open(cx, long(), 400.);
    rows.focus(cx);
    let bottom = rows.cursor_index(cx).unwrap();
    assert_eq!(bottom, rows.rows(cx).len() - 1, "the latest row");
    // Home scrolls to the start: Down goes on from the top of the screen.
    rows.press(cx, "home");
    rows.press(cx, "down");
    assert_eq!(rows.cursor_index(cx), Some(0));
    rows.press(cx, "down");
    assert_eq!(rows.cursor_index(cx), Some(1));
    // A page down, and Down takes the first row on screen.
    rows.press(cx, "pagedown");
    rows.press(cx, "down");
    let after = rows.cursor_index(cx).unwrap();
    assert!(after > 2, "{after}");
    // End scrolls to the latest: Up takes the last row on screen.
    rows.press(cx, "end");
    rows.press(cx, "up");
    assert_eq!(rows.cursor_index(cx), Some(bottom));
    // Up and up: the rows above scroll into view.
    for _ in 0..30 {
        rows.press(cx, "up");
    }
    assert_eq!(rows.cursor_index(cx), Some(bottom - 30));
    let shown = cx.read(|cx| rows.view.read(cx).list.shows_whole(bottom - 30));
    assert!(shown);
}

/// On the real engine: F6 takes the keyboard from the composer to the
/// transcript's rows, and Esc brings it back.
#[gpui_kit::test]
fn f6_goes_to_the_rows_and_esc_comes_back(cx: &mut TestAppContext) {
    use crate::backend::ArkoBackend;
    use crate::testing::{Mock, reply, wait_until};
    use crate::tests::{act, open};
    use gpui_kit::Focusable as _;

    let mock = Mock::start(|_, _| reply("a1", "Seven pears."));
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("arko");
    let work = temp.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    cx.executor().allow_parking();
    let backend = Arc::new(ArkoBackend::new(&root).with_configure(mock.configure()));
    cx.update(|cx| crate::init(Startup::for_tests(backend), cx));
    let (window, workspace) = open(cx, 1300.);
    let view = cx.read(|cx| workspace.read(cx).session_view().clone());
    cx.update(|cx| view.update(cx, |view, cx| view.set_draft_folder(work.clone(), cx)));
    act(cx, window, |window, cx| {
        window.input("Count the pears.", cx)
    });
    act(cx, window, |window, cx| window.press("enter", cx));
    wait_until(cx, "the reply in the transcript", |cx| {
        cx.update_window(window, |_, window, cx| window.render_frame(cx))
            .unwrap();
        cx.read(|cx| {
            view.read(cx).transcript(cx).is_some_and(|transcript| {
                transcript.read(cx).row_count() >= 2 && transcript.read(cx).busy_reason().is_none()
            })
        })
    });
    let transcript = cx.read(|cx| view.read(cx).transcript(cx).cloned().unwrap());
    act(cx, window, |window, cx| window.press("f6", cx));
    act(cx, window, |window, cx| {
        assert!(
            transcript
                .read(cx)
                .focus_handle(cx)
                .contains_focused(window, cx)
        );
    });
    assert!(cx.read(|cx| transcript.read(cx).cursor.is_some()));
    act(cx, window, |window, cx| window.press("escape", cx));
    act(cx, window, |window, cx| {
        assert!(view.read(cx).composer().read(cx).is_focused(window, cx));
    });
    act(cx, window, |window, cx| {
        let sessions = crate::live::LiveSessions::all(cx);
        crate::quit::close(sessions, false, window, cx);
    });
    wait_until(cx, "the session to end", |cx| {
        cx.read(|cx| crate::live::LiveSessions::all(cx).is_empty())
    });
}
