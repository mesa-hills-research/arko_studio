//! The rows of the table of inputs against the reader's states (DESIGN.md
//! section 9) in Studio's window, at pixel ratios 1, 1.25, 1.5 and 2, with
//! moves in device pixels. Following leaves blank below a shorter end and
//! moves no line down, a scroll up that moves a line reads, a scroll down
//! that leaves the end in sight follows, End, Esc, the marker and sending
//! follow again, a press holds the view, a fold keeps the clicked line on its
//! device pixel row, what opens and closes puts every line back, another
//! view keeps the reader's block, and the marker counts the lines below the
//! bottom edge the reader hasn't seen.

use std::collections::HashMap;

use super::*;
use crate::transcript::list::Marker;

const SIZE: (f32, f32) = (900., 640.);

/// Each tracked line of the last frame, by its id, with its device row.
fn rows(run: &Run) -> HashMap<String, i32> {
    let last = run.last.as_ref().expect("a frame was drawn");
    ids(&last.lines)
        .into_iter()
        .map(|(id, ix)| (id, last.lines[ix].row))
        .collect()
}

/// The lines both frames show that moved, from where to where.
fn moved(before: &HashMap<String, i32>, after: &HashMap<String, i32>) -> Vec<(String, i32, i32)> {
    before
        .iter()
        .filter_map(|(id, from)| {
            let to = *after.get(id)?;
            (to != *from).then(|| (id.clone(), *from, to))
        })
        .collect()
}

impl Run<'_> {
    /// Draws the frames a change makes until they stop changing.
    fn calm(&mut self) {
        for _ in 0..4 {
            self.draw();
            let shot = self.shot();
            let same = self
                .last
                .as_ref()
                .is_some_and(|last| last.lines == shot.lines);
            self.last = Some(shot);
            if same {
                break;
            }
        }
    }

    fn following(&mut self) -> bool {
        let transcript = self.transcript().expect("the transcript shows");
        self.cx.update(|cx| transcript.read(cx).is_following(cx))
    }

    fn marker(&mut self) -> Option<Marker> {
        let transcript = self.transcript().expect("the transcript shows");
        self.cx.update(|cx| transcript.read(cx).new_below())
    }

    fn blank(&self) -> i32 {
        self.last.as_ref().map_or(0, |last| last.blank)
    }

    /// A reply of `n` paragraphs at the end, streamed whole.
    fn reply_of(&mut self, n: usize) {
        for _ in 0..n {
            let text = format!("{}\n\n", self.some(12, 10));
            say(&self.feed, "main", &text);
        }
        self.calm();
    }

    /// The wheel by `dy` device pixels, up when positive.
    fn wheel_device(&mut self, dy: f32) {
        let scale = self.scale;
        self.wheel(dy / scale);
        self.calm();
    }

    /// The left button down (`true`) or up at `at`.
    fn button(&mut self, at: Point<Pixels>, down: bool) {
        self.cx
            .update_window(self.window, |_, window, cx| {
                let event = if down {
                    MouseDownEvent {
                        button: MouseButton::Left,
                        position: at,
                        modifiers: Modifiers::default(),
                        click_count: 1,
                        first_mouse: false,
                    }
                    .to_platform_input()
                } else {
                    MouseUpEvent {
                        button: MouseButton::Left,
                        position: at,
                        modifiers: Modifiers::default(),
                        click_count: 1,
                    }
                    .to_platform_input()
                };
                window.dispatch_event(event, cx);
            })
            .unwrap();
    }

    fn drag_to(&mut self, at: Point<Pixels>) {
        self.cx
            .update_window(self.window, |_, window, cx| {
                window.dispatch_event(
                    MouseMoveEvent {
                        position: at,
                        pressed_button: Some(MouseButton::Left),
                        modifiers: Modifiers::default(),
                    }
                    .to_platform_input(),
                    cx,
                );
            })
            .unwrap();
    }

    /// A point on the transcript's text, in logical pixels.
    fn on_text(&self) -> Point<Pixels> {
        let last = self.last.as_ref().expect("a frame was drawn");
        let line = &last.lines[last.lines.len() / 2];
        point(
            px((line.x + 20) as f32 / self.scale),
            px((line.row - 4) as f32 / self.scale),
        )
    }

    /// Blank below the end, from a command at the end, open in the Full view
    /// with its output, closed by the reader while following.
    fn blank_by_fold(&mut self) {
        let command = format!("cargo test {}", self.words(1));
        let id = call(&self.feed, "main", "shell", &[("command", command.into())]);
        let output: Vec<String> = (0..8).map(|_| format!("{} ok", self.words(2))).collect();
        result(
            &self.feed,
            "main",
            id,
            "shell",
            &format!("exit code 0, 1.0 s\n{}", output.join("\n")),
        );
        self.calm();
        let item = super::super::tests::item_ids(&self.feed)
            .into_iter()
            .rev()
            .find(|(_, kind)| *kind == arko::remote::Kind::Call)
            .map(|(id, _)| id)
            .expect("the call shows");
        let toggle = ElementId::from(("call-toggle", item));
        let open = |run: &mut Self| {
            let toggle = toggle.clone();
            run.cx
                .update_window(run.window, |_, window, _| {
                    window.try_find(toggle).and_then(|found| found.expanded())
                })
                .ok()
                .flatten()
                .unwrap_or(false)
        };
        // (It opens first where it doesn't show open, and the view goes to
        // the end below it.)
        if !open(self) {
            let at = self.toggle_at(&toggle).expect("the command's line shows");
            self.click(at);
            self.calm();
            let transcript = self.transcript().unwrap();
            self.cx
                .update(|cx| transcript.update(cx, |view, cx| view.scroll_to_end(cx)));
            self.calm();
        }
        let at = self.toggle_at(&toggle).expect("the command's line shows");
        self.click(at);
        self.calm();
        assert!(!open(self), "the command shows open");
        if self.blank() == 0 {
            let transcript = self.transcript().unwrap();
            let (report, rows, top) = self.cx.update(|cx| {
                let view = transcript.read(cx);
                (
                    view.list.handle().report(),
                    view.rows.len(),
                    view.list.top(),
                )
            });
            panic!(
                "the fold closed with no blank below the end: {report:?}, {rows} rows, top {top:?}\n{}",
                self.last
                    .as_ref()
                    .unwrap()
                    .render(self.last.as_ref().unwrap())
            );
        }
    }

    /// The end gets shorter while "Working…" goes, as text comes after a
    /// pause (a reply, a pause with "Working…", and the reply going on with
    /// a word).
    fn working_goes(&mut self) {
        self.wait(2_500);
        self.calm();
        let more = format!(" {}", self.words(1));
        say(&self.feed, "main", &more);
        self.calm();
    }
}

/// Runs `test` at every pixel ratio.
fn each_scale(test: impl Fn(f32)) {
    for scale in SCALES {
        test(scale);
    }
}

/// Rule 1 and row 25: "Working…" going leaves blank below the end, and no
/// line moves. A scroll down then moves nothing and follows from there
/// (row 2), and what comes fills the blank first.
#[test]
fn a_shorter_end_leaves_blank_and_a_scroll_down_moves_nothing() {
    each_scale(|scale| {
        let mut meter = Meter::default();
        let mut run = Run::with_history(0, (SIZE, scale), 4, &mut meter);
        run.feed
            .user("Please w9001 w9002", Source::App, Delivery::Turn);
        run.feed.turn_started();
        run.reply_of(3);
        run.working_goes();
        assert!(run.following(), "at {scale}");
        let before = rows(&run);
        let blank = run.blank();
        // "Working…" shows, then goes as the text goes on.
        run.wait(2_500);
        run.calm();
        let more = format!(" {}", run.words(1));
        say(&run.feed, "main", &more);
        run.calm();
        let after = rows(&run);
        let down: Vec<_> = moved(&before, &after)
            .into_iter()
            .filter(|(_, from, to)| to > from)
            .collect();
        assert!(down.is_empty(), "at {scale}: lines went down {down:?}");
        assert!(run.blank() >= blank, "at {scale}");
        assert_eq!(run.marker(), None, "at {scale}");
        // A scroll down moves nothing and follows.
        let still = rows(&run);
        run.wheel_device(-30.);
        assert_eq!(rows(&run), still, "at {scale}");
        assert!(run.following(), "at {scale}");
    });
}

/// Rule 5 and row 1: a scroll up of a device pixel reads, output then goes
/// below the lines, and the marker counts the lines that came below the
/// bottom edge, rounded up (rule 8, PX9). Rows 4 and 6: End and the marker
/// follow again with the end on the bottom edge.
#[test]
fn a_scroll_up_reads_and_the_marker_counts_only_lines_below() {
    each_scale(|scale| {
        let mut meter = Meter::default();
        let mut run = Run::with_history(1, (SIZE, scale), 6, &mut meter);
        run.feed.user("Please w9101", Source::App, Delivery::Turn);
        run.feed.turn_started();
        run.reply_of(2);
        assert!(run.following());
        run.wheel_device(1.);
        assert!(!run.following(), "a device pixel up reads, at {scale}");
        let read = rows(&run);
        run.reply_of(2);
        assert!(moved(&read, &rows(&run)).is_empty(), "at {scale}");
        let Some(Marker::New(lines)) = run.marker() else {
            panic!("at {scale}: no count, {:?}", run.marker());
        };
        assert!(lines >= 2, "at {scale}: {lines}");
        // The same count in the next frame, once the rows are measured.
        run.calm();
        assert_eq!(run.marker(), Some(Marker::New(lines)), "at {scale}");
        // End follows at the end, with the blank closed.
        let transcript = run.transcript().unwrap();
        run.cx
            .update(|cx| transcript.update(cx, |view, cx| view.scroll_to_end(cx)));
        run.calm();
        assert!(run.following(), "at {scale}");
        assert_eq!(run.blank(), 0, "at {scale}");
        assert_eq!(run.marker(), None, "at {scale}");
    });
}

/// Row 4 and 5: Esc follows from where the end shows, and keeps the blank.
#[test]
fn esc_follows_from_where_the_end_shows() {
    each_scale(|scale| {
        let mut meter = Meter::default();
        let mut run = Run::with_history(0, (SIZE, scale), 4, &mut meter);
        run.feed.user("Please w9201", Source::App, Delivery::Turn);
        run.feed.turn_started();
        run.reply_of(2);
        run.blank_by_fold();
        assert!(run.following(), "a fold leaves the choice, at {scale}");
        run.wheel_device(2.);
        assert!(!run.following(), "at {scale}");
        let before = rows(&run);
        let blank = run.blank();
        let transcript = run.transcript().unwrap();
        run.cx
            .update(|cx| transcript.update(cx, |view, cx| view.follow_here(cx)));
        run.calm();
        assert!(run.following(), "at {scale}");
        assert_eq!(rows(&run), before, "at {scale}");
        assert_eq!(run.blank(), blank, "at {scale}");
    });
}

/// Rows 10 and 13: a press holds the view, output goes below the held
/// lines, and a plain click by a follower goes to the end at its release.
/// Row 14: a drag stays where it is.
#[test]
fn a_press_holds_the_view_and_a_plain_click_catches_up() {
    each_scale(|scale| {
        for drag in [false, true] {
            let mut meter = Meter::default();
            let mut run = Run::with_history(0, (SIZE, scale), 6, &mut meter);
            run.feed.user("Please w9301", Source::App, Delivery::Turn);
            run.feed.turn_started();
            run.reply_of(1);
            let at = run.on_text();
            run.button(at, true);
            run.calm();
            let held = rows(&run);
            run.reply_of(3);
            assert!(moved(&held, &rows(&run)).is_empty(), "held, at {scale}");
            if drag {
                run.drag_to(point(at.x + px(40.), at.y));
            }
            run.button(at, false);
            run.calm();
            assert!(run.following(), "at {scale}");
            if drag {
                assert!(
                    moved(&held, &rows(&run)).is_empty(),
                    "a drag stays, at {scale}"
                );
            } else {
                assert_eq!(run.blank(), 0, "the click caught up, at {scale}");
                assert_eq!(run.marker(), None, "at {scale}");
            }
        }
    });
}

/// Row 7: sending a message goes to the end, for a reader too.
#[test]
fn sending_a_message_goes_to_the_end() {
    each_scale(|scale| {
        let mut meter = Meter::default();
        let mut run = Run::with_history(0, (SIZE, scale), 8, &mut meter);
        run.wheel_device(400.);
        assert!(!run.following(), "at {scale}");
        let window = run.window;
        let workspace = run.workspace.clone();
        run.cx
            .update_window(window, |_, window, cx| {
                let view = workspace.read(cx).session_view().clone();
                view.update(cx, |view, cx| {
                    view.send(
                        crate::composer::Outgoing {
                            text: "And w9401".into(),
                            images: Vec::new(),
                        },
                        window,
                        cx,
                    )
                });
            })
            .unwrap();
        run.calm();
        assert!(run.following(), "at {scale}");
        assert_eq!(run.marker(), None, "at {scale}");
    });
}

/// Row 12 and PX10: a fold the reader opens or closes keeps its clicked line
/// on its device pixel row, following with blank below the end, following
/// with the end on the bottom edge, and reading, and the lines above it move
/// by a line at most.
#[test]
fn a_fold_keeps_its_clicked_line_on_its_row() {
    each_scale(|scale| {
        for state in 0..3 {
            let mut meter = Meter::default();
            let mut run = Run::with_history(0, (SIZE, scale), 4, &mut meter);
            run.feed.user("Please w9501", Source::App, Delivery::Turn);
            run.feed.turn_started();
            for _ in 0..3 {
                run.call();
            }
            run.reply_of(1);
            match state {
                0 => run.blank_by_fold(),
                1 => {}
                _ => run.wheel_device(30.),
            }
            let folds = run.folds();
            let Some((key, id, at)) = folds
                .into_iter()
                .find_map(|(key, id)| run.toggle_at(&id).map(|at| (key, id, at)))
            else {
                panic!("at {scale}: no fold on the screen");
            };
            let _ = (key, &id);
            let y = (f32::from(at.y) * scale).round() as i32;
            let before = rows(&run);
            let clicked = before
                .iter()
                .min_by_key(|(_, row)| (**row - y).abs())
                .map(|(id, row)| (id.clone(), *row))
                .unwrap();
            run.click(at);
            run.calm();
            let after = rows(&run);
            assert_eq!(
                after.get(&clicked.0),
                Some(&clicked.1),
                "at {scale}, state {state}: the clicked line moved"
            );
            let line = (32. * scale) as i32;
            for (id, from, to) in moved(&before, &after) {
                if from < clicked.1 {
                    assert!(
                        (to - from).abs() <= line,
                        "at {scale}, state {state}: {id} {from} → {to}"
                    );
                }
            }
            assert_eq!(run.following(), state < 2, "at {scale}, state {state}");
        }
    });
}

/// Rows 16 and 17, rule 7: the composer grows by three lines and shrinks
/// again, and the approval tray opens and closes, with blank below the end
/// and without, and every line comes back to its row.
#[test]
fn what_opens_and_closes_puts_every_line_back() {
    each_scale(|scale| {
        for (kind, blank) in [(2, false), (2, true), (0, false), (0, true)] {
            let mut meter = Meter::default();
            let mut run = Run::with_history(0, (SIZE, scale), 6, &mut meter);
            run.feed.user("Please w9601", Source::App, Delivery::Turn);
            run.feed.turn_started();
            run.reply_of(2);
            if blank {
                run.blank_by_fold();
            }
            let before = rows(&run);
            run.change(kind, true);
            run.calm();
            run.change(kind, false);
            run.calm();
            assert_eq!(rows(&run), before, "at {scale}, {kind}, blank {blank}");
            assert!(run.following(), "at {scale}");
        }
    });
}

/// Row 19 and rule 9: another view and back while reading keeps the block
/// the reader left on the top row, and following, the new view follows at
/// its end.
#[test]
fn another_view_keeps_the_readers_block_and_a_follower_follows() {
    each_scale(|scale| {
        let mut meter = Meter::default();
        let mut run = Run::with_history(0, (SIZE, scale), 10, &mut meter);
        run.wheel_device(600.);
        let before = rows(&run);
        for view in [View::Quiet, View::Messages, View::Full] {
            run.cx
                .update(|cx| prefs::update(cx, |prefs| prefs.view = view));
            run.calm();
        }
        assert_eq!(rows(&run), before, "at {scale}");
        assert!(!run.following(), "at {scale}");
        let transcript = run.transcript().unwrap();
        run.cx
            .update(|cx| transcript.update(cx, |view, cx| view.scroll_to_end(cx)));
        run.calm();
        run.cx
            .update(|cx| prefs::update(cx, |prefs| prefs.view = View::Messages));
        run.calm();
        assert!(run.following(), "at {scale}");
        assert_eq!(run.blank(), 0, "at {scale}");
    });
}

/// Rule 6: a follower's scroll down in the frame that output grows the end
/// keeps the end on the bottom edge, and the view follows on.
#[test]
fn a_followers_scroll_down_as_output_comes_keeps_following() {
    each_scale(|scale| {
        let mut meter = Meter::default();
        let mut run = Run::with_history(0, (SIZE, scale), 4, &mut meter);
        run.feed.user("Please w9901", Source::App, Delivery::Turn);
        run.feed.turn_started();
        run.reply_of(2);
        assert!(run.following(), "at {scale}");
        for _ in 0..3 {
            let command = format!("cargo test {}", run.words(1));
            call(&run.feed, "main", "shell", &[("command", command.into())]);
        }
        // (A device pixel down, less than the calls grow the end by, as Page
        // Down gives it. The wheel at the end goes past the list.)
        let transcript = run.transcript().unwrap();
        run.cx
            .update(|cx| transcript.read(cx).list.scroll_by(px(1. / scale)));
        run.calm();
        let followed = run.cx.update(|cx| transcript.read(cx).list.followed());
        assert!(run.following(), "at {scale}");
        assert!(followed, "at {scale}");
        assert_eq!(run.blank(), 0, "at {scale}");
        assert_eq!(run.marker(), None, "at {scale}");
    });
}

/// Row 18: another text size wraps the rows again with a follower's end on
/// the bottom edge, and the blank below it closes.
#[test]
fn another_text_size_brings_a_followers_end_to_the_bottom_edge() {
    each_scale(|scale| {
        let mut meter = Meter::default();
        let mut run = Run::with_history(0, (SIZE, scale), 6, &mut meter);
        run.feed.user("Please w9801", Source::App, Delivery::Turn);
        run.feed.turn_started();
        run.reply_of(2);
        run.blank_by_fold();
        assert!(run.blank() > 0, "at {scale}");
        run.cx
            .update(|cx| prefs::update(cx, |prefs| prefs.appearance.zoom = 0.75));
        run.calm();
        assert!(run.following(), "at {scale}");
        // (Within the device pixel the paddings above and below the rows
        // round to at this text size.)
        assert!(run.blank() <= 1, "at {scale}: {} blank", run.blank());
    });
}

/// Rule 9: the block another view keeps is the one on the top row as the
/// reader left it. One an earlier switch kept ends with the reader's scroll,
/// also a scroll drawn in the one frame before the next switch.
#[test]
fn another_view_keeps_the_block_on_the_top_row_now() {
    each_scale(|scale| {
        let mut meter = Meter::default();
        let mut run = Run::with_history(0, (SIZE, scale), 10, &mut meter);
        run.wheel_device(600.);
        assert!(!run.following(), "at {scale}");
        let trip = |run: &mut Run| {
            for view in [View::Quiet, View::Full] {
                run.cx
                    .update(|cx| prefs::update(cx, |prefs| prefs.view = view));
                run.calm();
            }
        };
        trip(&mut run);
        run.wheel(50. / scale);
        run.draw();
        run.last = Some(run.shot());
        let before = rows(&run);
        trip(&mut run);
        assert_eq!(rows(&run), before, "at {scale}");
    });
}

/// Draws the marker over a full last line, with output below the screen
/// ("↓ 6 new lines") and with none ("↓ end"), at pixel ratios 1 and 2, into
/// `MARKER_SHOTS` (by default `marker-shots`), for its rendering.
#[test]
#[ignore = "draws the marker's renderings"]
fn draw_the_marker() {
    use std::sync::Arc;
    use std::time::SystemTime;

    let dir = std::path::PathBuf::from(
        std::env::var("MARKER_SHOTS").unwrap_or_else(|_| "marker-shots".into()),
    );
    std::fs::create_dir_all(&dir).unwrap();
    for (scale, dark) in [(1., false), (2., false), (2., true)] {
        let mut app = gpui_screenshot::ScreenshotApp::with_assets(crate::assets::Assets).unwrap();
        let mut settings = prefs::Prefs::default();
        settings.appearance.motion = prefs::Motion::Off;
        if dark {
            settings.appearance.theme_mode = prefs::ThemeMode::Dark;
        }
        let backend = Arc::new(crate::backend::FakeBackend::sample(
            "/arko",
            SystemTime::now(),
        ));
        let startup = crate::Startup::for_tests(backend)
            .with_prefs(settings)
            .with_fonts(crate::fonts::bundled());
        app.update(|cx| crate::init(startup, cx));
        let mut opened = None;
        let window = app
            .open_window_with(size(px(900.), px(600.)), scale, |options, cx| {
                crate::workspace::open_window(options, cx).map(|(window, workspace)| {
                    opened = Some(workspace);
                    window
                })
            })
            .unwrap();
        let workspace = opened.unwrap();
        let feed = Feed::new("main");
        let session = app.update(|cx| cx.new(|cx| LiveSession::for_tests("s1", cx)));
        app.update(|cx| crate::live::LiveSessions::add_for_tests(session.clone(), cx));
        app.update(|cx| session.update(cx, |session, cx| session.follow_feed(feed.clone(), cx)));
        app.update_window(window, |_, window, cx| {
            workspace.update(cx, |workspace, cx| {
                workspace.show_session(session.clone(), window, cx)
            })
        })
        .unwrap();
        for turn in 0..4 {
            feed.user(
                &format!("Why does the list jump when a fold closes at the end? ({turn})"),
                Source::App,
                Delivery::Turn,
            );
            feed.turn_started();
            feed.reply(
                "The list pinned its last row to the bottom edge in every layout, so a shorter \
                 end moved every line down. Now the lines stay, and the blank below the end waits \
                 for what comes.\n\nA scroll up that moves a line reads, and a scroll down that \
                 reaches the end follows again.",
            );
            feed.turn_ended("12 s · $0.03", None);
        }
        feed.user("And the marker?", Source::App, Delivery::Turn);
        feed.turn_started();
        for _ in 0..8 {
            real::settle(&mut app, window);
        }
        // The reader scrolls up a little, and the reply streams below.
        let position = point(px(600.), px(300.));
        app.update_window(window, |_, window, cx| {
            window.dispatch_event(
                ScrollWheelEvent {
                    position,
                    delta: ScrollDelta::Pixels(point(px(0.), px(90.))),
                    ..Default::default()
                }
                .to_platform_input(),
                cx,
            );
        })
        .unwrap();
        real::settle(&mut app, window);
        let name = |what: &str| {
            let look = if dark { "dark" } else { "light" };
            dir.join(format!("marker-{what}-{look}-{scale}x.png"))
        };
        let shot = app.capture(window).unwrap();
        shot.image.save(name("end")).unwrap();
        say(
            &feed,
            "main",
            "The marker counts the lines that came below the bottom edge since the last frame \
             that followed, and only those the reader hasn't seen.\n\nIt says end when nothing \
             came, and a click on it goes to the end.",
        );
        for _ in 0..4 {
            real::settle(&mut app, window);
        }
        let shot = app.capture(window).unwrap();
        shot.image.save(name("new-lines")).unwrap();
    }
}
