//! Interaction tests: the app's real windows, driven by clicks and keys on
//! GPUI's test platform, on the fake backend's sample sessions.

use std::sync::Arc;
use std::time::SystemTime;

use gpui_kit::component::{Theme, ThemeRegistry, WindowExt as _};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, Bounds, Entity, Point, TestAppContext, Window,
    WindowBounds, WindowOptions, px, size,
};

use crate::appearance::PREVIEW_REST;
use crate::backend::{Backend as _, FakeBackend};
use crate::prefs::{self, ThemeMode};
use crate::sidebar::Row;
use crate::workspace::{self, Workspace};
use crate::{Startup, menus};

const SPLIT: &str = "20261016-091200-a1b2";
const FONTS: &str = "20261016-080000-a7b8";

fn start(cx: &mut TestAppContext) -> Arc<FakeBackend> {
    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    cx.update(|cx| crate::init(Startup::for_tests(backend.clone()), cx));
    backend
}

pub(crate) fn open(cx: &mut TestAppContext, width: f32) -> (AnyWindowHandle, Entity<Workspace>) {
    let opened = cx.update(|cx| {
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: Point::default(),
                size: size(px(width), px(720.)),
            })),
            ..Default::default()
        };
        workspace::open_window(options, cx).unwrap()
    });
    // The sessions are listed on a background thread.
    cx.run_until_parked();
    opened
}

/// Runs `act` in the window after drawing a frame, then lets the work it
/// started finish.
pub(crate) fn act(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
    act: impl FnOnce(&mut Window, &mut App),
) {
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        act(window, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

pub(crate) fn open_title(workspace: &Entity<Workspace>, cx: &mut TestAppContext) -> Option<String> {
    cx.read(|cx| {
        workspace
            .read(cx)
            .session_view()
            .read(cx)
            .session(cx)
            .map(|session| session.title)
    })
}

pub(crate) fn row_of(workspace: &Entity<Workspace>, title: &str, cx: &mut TestAppContext) -> usize {
    cx.read(|cx| {
        workspace
            .read(cx)
            .sidebar()
            .read(cx)
            .rows()
            .iter()
            .position(|row| matches!(row, Row::Session { title: t, .. } if t == title))
            .unwrap_or_else(|| panic!("no row {title}"))
    })
}

#[gpui_kit::test]
fn clicking_a_session_opens_it(cx: &mut TestAppContext) {
    start(cx);
    let (window, workspace) = open(cx, 1300.);
    assert_eq!(open_title(&workspace, cx), None);
    let row = row_of(&workspace, "Split the engine out of main.rs", cx);
    act(cx, window, |window, cx| window.click(("row", row), cx));
    assert_eq!(
        open_title(&workspace, cx).as_deref(),
        Some("Split the engine out of main.rs")
    );
    let active = cx.read(|cx| {
        let sidebar = workspace.read(cx).sidebar().read(cx);
        sidebar
            .rows()
            .iter()
            .any(|row| matches!(row, Row::Session { id, .. } if id.as_str() == SPLIT))
    });
    assert!(active);
}

#[gpui_kit::test]
fn the_keyboard_moves_through_the_list_and_opens(cx: &mut TestAppContext) {
    start(cx);
    let (window, workspace) = open(cx, 1300.);
    // Mod+P focuses the filter; Down goes into the list, on its first session.
    act(cx, window, |window, cx| {
        window.dispatch_action(Box::new(menus::FocusSessionFilter), cx)
    });
    act(cx, window, |window, cx| window.press("down", cx));
    act(cx, window, |window, cx| window.press("enter", cx));
    assert_eq!(
        open_title(&workspace, cx).as_deref(),
        Some("Variable fonts and the wght axis")
    );
    // Opening moves the focus to the composer: back to the list, one down.
    act(cx, window, |window, cx| {
        window.dispatch_action(Box::new(menus::FocusSessionFilter), cx)
    });
    act(cx, window, |window, cx| window.press("down", cx));
    act(cx, window, |window, cx| window.press("down", cx));
    act(cx, window, |window, cx| window.press("enter", cx));
    assert_eq!(
        open_title(&workspace, cx).as_deref(),
        Some("Naming a desktop app")
    );
}

#[gpui_kit::test]
fn the_filter_narrows_the_list_and_enter_opens_the_first_match(cx: &mut TestAppContext) {
    start(cx);
    let (window, workspace) = open(cx, 1300.);
    act(cx, window, |window, cx| {
        window.dispatch_action(Box::new(menus::FocusSessionFilter), cx)
    });
    act(cx, window, |window, cx| window.input("golden", cx));
    let titles = cx.read(|cx| workspace.read(cx).sidebar().read(cx).rows().len());
    assert_eq!(titles, 2, "the group and its one session");
    act(cx, window, |window, cx| window.press("enter", cx));
    assert_eq!(
        open_title(&workspace, cx),
        None,
        "that session is held elsewhere"
    );
    act(cx, window, |window, cx| {
        assert_eq!(window.notifications(cx).len(), 1);
    });
    // Esc clears the filter.
    act(cx, window, |window, cx| {
        window.dispatch_action(Box::new(menus::FocusSessionFilter), cx)
    });
    act(cx, window, |window, cx| window.press("escape", cx));
    let rows = cx.read(|cx| workspace.read(cx).sidebar().read(cx).rows().len());
    assert_eq!(rows, 8);
}

#[gpui_kit::test]
fn left_goes_up_to_the_group_and_closes_it(cx: &mut TestAppContext) {
    start(cx);
    let (window, workspace) = open(cx, 1300.);
    act(cx, window, |window, cx| {
        window.dispatch_action(Box::new(menus::FocusSessionFilter), cx)
    });
    act(cx, window, |window, cx| window.press("down", cx));
    act(cx, window, |window, cx| window.press("left", cx));
    act(cx, window, |window, cx| window.press("left", cx));
    let collapsed = cx.read(|cx| prefs::get(cx).sidebar.collapsed.clone());
    assert_eq!(collapsed, [crate::sidebar::CHATS]);
    let rows = cx.read(|cx| workspace.read(cx).sidebar().read(cx).rows().to_vec());
    assert!(matches!(
        &rows[0],
        Row::Group {
            collapsed: true,
            ..
        }
    ));
    assert!(
        matches!(&rows[1], Row::Group { .. }),
        "the chats are hidden"
    );
    // Right opens it again.
    act(cx, window, |window, cx| window.press("right", cx));
    assert!(cx.read(|cx| prefs::get(cx).sidebar.collapsed.is_empty()));
}

#[gpui_kit::test]
fn f2_renames_a_session_in_place(cx: &mut TestAppContext) {
    let backend = start(cx);
    let (window, _) = open(cx, 1300.);
    act(cx, window, |window, cx| {
        window.dispatch_action(Box::new(menus::FocusSessionFilter), cx)
    });
    act(cx, window, |window, cx| window.press("down", cx));
    act(cx, window, |window, cx| window.press("f2", cx));
    // The name is selected, so typing replaces it.
    act(cx, window, |window, cx| window.input("Fonts", cx));
    act(cx, window, |window, cx| window.press("enter", cx));
    let sessions = backend.sessions().unwrap();
    let renamed = sessions
        .iter()
        .find(|entry| entry.id.as_str() == FONTS)
        .unwrap();
    assert_eq!(renamed.title, "Fonts");
}

#[gpui_kit::test]
fn escape_cancels_a_rename(cx: &mut TestAppContext) {
    let backend = start(cx);
    let (window, _) = open(cx, 1300.);
    act(cx, window, |window, cx| {
        window.dispatch_action(Box::new(menus::FocusSessionFilter), cx)
    });
    act(cx, window, |window, cx| window.press("down", cx));
    act(cx, window, |window, cx| window.press("f2", cx));
    act(cx, window, |window, cx| window.input("Nope", cx));
    act(cx, window, |window, cx| window.press("escape", cx));
    let sessions = backend.sessions().unwrap();
    let kept = sessions
        .iter()
        .find(|entry| entry.id.as_str() == FONTS)
        .unwrap();
    assert_eq!(kept.title, "Variable fonts and the wght axis");
}

#[gpui_kit::test]
fn delete_asks_first_and_then_removes_the_session(cx: &mut TestAppContext) {
    let backend = start(cx);
    let (window, workspace) = open(cx, 1300.);
    act(cx, window, |window, cx| {
        window.dispatch_action(Box::new(menus::FocusSessionFilter), cx)
    });
    act(cx, window, |window, cx| window.press("down", cx));
    act(cx, window, |window, cx| window.press("delete", cx));
    assert_eq!(
        backend.sessions().unwrap().len(),
        5,
        "nothing deleted before the answer"
    );
    act(cx, window, |window, cx| {
        assert!(window.has_active_dialog(cx));
        window.within("dialog").click("ok", cx);
    });
    let sessions = backend.sessions().unwrap();
    assert!(sessions.iter().all(|entry| entry.id.as_str() != FONTS));
    let rows = cx.read(|cx| workspace.read(cx).sidebar().read(cx).rows().len());
    assert_eq!(rows, 7);
}

#[gpui_kit::test]
fn a_session_open_elsewhere_isnt_opened_and_says_why(cx: &mut TestAppContext) {
    start(cx);
    let (window, workspace) = open(cx, 1300.);
    let row = row_of(&workspace, "Golden screenshots on Linux", cx);
    act(cx, window, |window, cx| window.click(("row", row), cx));
    assert_eq!(open_title(&workspace, cx), None);
    act(cx, window, |window, cx| {
        assert_eq!(window.notifications(cx).len(), 1)
    });
}

#[gpui_kit::test]
fn the_sidebar_snaps_closed_and_the_menu_says_so(cx: &mut TestAppContext) {
    start(cx);
    let (window, workspace) = open(cx, 1300.);
    assert!(cx.read(|cx| workspace.read(cx).is_sidebar_docked(cx)));
    act(cx, window, |window, cx| {
        window.dispatch_action(Box::new(menus::ToggleSidebar), cx)
    });
    assert!(!cx.read(|cx| workspace.read(cx).is_sidebar_docked(cx)));
    assert!(!cx.read(|cx| prefs::get(cx).sidebar.open));
    act(cx, window, |window, cx| {
        assert!(window.try_find("sidebar-pane").is_none());
        window.press("secondary-\\", cx);
    });
    assert!(cx.read(|cx| workspace.read(cx).is_sidebar_docked(cx)));
}

#[gpui_kit::test]
fn a_narrow_window_shows_the_sidebar_as_a_sheet_that_closes_on_a_choice(cx: &mut TestAppContext) {
    start(cx);
    let (window, workspace) = open(cx, 600.);
    assert_eq!(
        cx.read(|cx| workspace.read(cx).width_class()),
        crate::layout::WidthClass::Narrow
    );
    assert!(!cx.read(|cx| workspace.read(cx).is_sidebar_docked(cx)));
    act(cx, window, |window, cx| {
        assert!(window.try_find("sidebar-pane").is_none());
        window.dispatch_action(Box::new(menus::ToggleSidebar), cx);
    });
    act(cx, window, |window, cx| {
        assert!(window.has_active_sheet(cx))
    });
    let row = row_of(&workspace, "Split the engine out of main.rs", cx);
    act(cx, window, |window, cx| {
        window.within("sheet").click(("row", row), cx)
    });
    assert_eq!(
        open_title(&workspace, cx).as_deref(),
        Some("Split the engine out of main.rs")
    );
    act(cx, window, |window, cx| {
        assert!(!window.has_active_sheet(cx))
    });
}

/// A session of the workspace in a window `width` wide, and the number of
/// notifications the window shows.
fn session_in_workspace(
    cx: &mut TestAppContext,
    width: f32,
) -> (
    AnyWindowHandle,
    Entity<Workspace>,
    Entity<crate::live::LiveSession>,
) {
    use crate::live::LiveSession;

    start(cx);
    let (window, workspace) = open(cx, width);
    let session = cx.new(|cx| LiveSession::for_tests("s1", cx));
    act(cx, window, |window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.show_session(session.clone(), window, cx)
        })
    });
    (window, workspace, session)
}

fn notifications(cx: &mut TestAppContext, window: AnyWindowHandle) -> usize {
    cx.update_window(window, |_, window, cx| window.notifications(cx).len())
        .unwrap()
}

/// Types `text` in the composer and sends it.
fn send_text(cx: &mut TestAppContext, window: AnyWindowHandle, text: &str) {
    act(cx, window, |window, cx| window.input(text, cx));
    act(cx, window, |window, cx| window.press("enter", cx));
}

/// The messages the session has queued, as typed.
fn queued_messages(
    session: &Entity<crate::live::LiveSession>,
    cx: &mut TestAppContext,
) -> Vec<String> {
    cx.read(|cx| {
        session
            .read(cx)
            .queued()
            .iter()
            .filter_map(|input| match input {
                arko::Input::Message(message) => Some(message.text.clone()),
                _ => None,
            })
            .collect()
    })
}

/// Where the workers panel is a sheet, a worker's Message closes the sheet,
/// which covers the composer. A message the user then sends shows only in
/// that worker's transcript, which is out of sight, so a notification says
/// it went. Messages to the main agent show in the transcript, with no
/// notification.
fn a_message_to_a_worker_from_a_sheet_says_it_went(cx: &mut TestAppContext, width: f32) {
    use crate::workers_panel::WorkersPanelEvent;

    let (window, workspace, session) = session_in_workspace(cx, width);
    assert!(!cx.read(|cx| workspace.read(cx).width_class().docks_panel()));
    act(cx, window, |window, cx| {
        window.dispatch_action(Box::new(menus::ToggleWorkersPanel), cx)
    });
    act(cx, window, |window, cx| {
        assert!(window.has_active_sheet(cx))
    });
    // r1's Message, as the panel's button gives it.
    let panel = cx.read(|cx| workspace.read(cx).workers_panel().clone());
    act(cx, window, |_, cx| {
        panel.update(cx, |_, cx| cx.emit(WorkersPanelEvent::Message("r1".into())))
    });
    act(cx, window, |window, cx| {
        assert!(!window.has_active_sheet(cx))
    });
    send_text(cx, window, "Use tabs.");
    assert_eq!(queued_messages(&session, cx), ["/message r1 Use tabs."]);
    assert_eq!(notifications(cx, window), 1);
    // Back to the main agent: its messages need no word.
    act(cx, window, |window, cx| window.press("escape", cx));
    let view = cx.read(|cx| workspace.read(cx).session_view().clone());
    assert_eq!(cx.read(|cx| view.read(cx).target().cloned()), None);
    send_text(cx, window, "And the tests.");
    assert_eq!(cx.read(|cx| session.read(cx).queued().len()), 2);
    assert_eq!(notifications(cx, window), 1);
}

#[gpui_kit::test]
fn a_message_to_a_worker_from_a_sheet_says_it_went_in_a_medium_window(cx: &mut TestAppContext) {
    a_message_to_a_worker_from_a_sheet_says_it_went(cx, 900.);
}

#[gpui_kit::test]
fn a_message_to_a_worker_from_a_sheet_says_it_went_in_a_narrow_window(cx: &mut TestAppContext) {
    a_message_to_a_worker_from_a_sheet_says_it_went(cx, 600.);
}

/// Where the workers panel docks, a message to a worker says it went unless
/// the panel shows that worker's transcript. It says so while the panel is
/// closed, and while the panel shows another worker.
#[gpui_kit::test]
fn a_message_to_a_worker_says_it_went_unless_its_transcript_shows(cx: &mut TestAppContext) {
    use crate::workers_panel::{PanelSource, WorkersPanelEvent};
    use arko::remote::{Feed, WorkerChange};

    let (window, workspace, session) = session_in_workspace(cx, 1300.);
    assert!(cx.read(|cx| workspace.read(cx).width_class().docks_panel()));
    let panel = cx.read(|cx| workspace.read(cx).workers_panel().clone());
    let message = |cx: &mut TestAppContext, worker: &str| {
        let worker = worker.to_string();
        act(cx, window, |_, cx| {
            panel.update(cx, |_, cx| {
                cx.emit(WorkersPanelEvent::Message(worker.into()))
            })
        });
    };

    // The panel is closed.
    assert!(!cx.read(|cx| workspace.read(cx).is_workers_open()));
    message(cx, "r1");
    send_text(cx, window, "Use tabs.");
    assert_eq!(notifications(cx, window), 1);

    // It shows r2's transcript, not r1's.
    let feed = Feed::new("main");
    feed.turn_started();
    feed.worker("r1", "Fix the parser", WorkerChange::Started);
    feed.worker("r2", "Write the changelog", WorkerChange::Started);
    act(cx, window, |window, cx| {
        window.dispatch_action(Box::new(menus::ToggleWorkersPanel), cx)
    });
    assert!(cx.read(|cx| workspace.read(cx).is_workers_open()));
    // The panel shows `worker` from the feed above. (The session's own
    // source has no feed, and its key is the one the panel keeps its
    // transcripts for.)
    let key = (
        session.entity_id(),
        cx.read(|cx| session.read(cx).opening()),
    );
    let show = |cx: &mut TestAppContext, worker: &str| {
        let (feed, shown) = (feed.clone(), worker.to_string());
        act(cx, window, |window, cx| {
            panel.update(cx, |panel, cx| {
                panel.set_source(
                    PanelSource {
                        workers: feed.subscribe().0.workers,
                        feed: Some(feed),
                        key: Some(key),
                        ..PanelSource::default()
                    },
                    cx,
                );
                panel.show_worker(shown.into(), window, cx);
            })
        });
        assert_eq!(
            cx.read(|cx| panel.read(cx).open_worker().cloned()),
            Some(worker.to_string().into())
        );
    };
    show(cx, "r2");
    message(cx, "r1");
    send_text(cx, window, "And the tests.");
    assert_eq!(notifications(cx, window), 2);

    // It shows r1's, so the message is in sight and needs no word.
    show(cx, "r1");
    message(cx, "r1");
    send_text(cx, window, "Thanks.");
    assert_eq!(
        queued_messages(&session, cx),
        [
            "/message r1 Use tabs.",
            "/message r1 And the tests.",
            "/message r1 Thanks."
        ]
    );
    assert_eq!(notifications(cx, window), 2);
}

/// The confirmation names the worker and reads as a sentence.
#[test]
fn the_confirmation_of_a_message_to_a_worker_reads_as_a_sentence() {
    let text = workspace::sent_to("r1");
    assert_eq!(text, "Sent to r1.");
    assert!(!text.contains([':', ';']), "{text}");
}

#[gpui_kit::test]
fn dragging_the_sidebars_edge_resizes_it_and_remembers(cx: &mut TestAppContext) {
    start(cx);
    let (window, workspace) = open(cx, 1300.);
    act(cx, window, |window, cx| {
        let handle = window.find("sidebar-resize").bounds();
        let from = handle.center();
        window.drag(from, Point::new(px(320.), from.y), cx);
    });
    let width = cx.read(|cx| prefs::get(cx).sidebar.width);
    assert!((width - 20.).abs() < 0.2, "{width} rem");
    // Held to its range.
    act(cx, window, |window, cx| {
        let from = window.find("sidebar-resize").bounds().center();
        window.drag(from, Point::new(px(40.), from.y), cx);
    });
    assert_eq!(
        cx.read(|cx| prefs::get(cx).sidebar.width),
        prefs::MIN_SIDEBAR_WIDTH
    );
    assert!(cx.read(|cx| workspace.read(cx).is_sidebar_docked(cx)));
}

#[gpui_kit::test]
fn settings_open_in_the_window_and_escape_goes_back(cx: &mut TestAppContext) {
    start(cx);
    let (window, workspace) = open(cx, 1300.);
    act(cx, window, |window, cx| window.press("secondary-,", cx));
    assert!(cx.read(|cx| workspace.read(cx).settings().is_some()));
    act(cx, window, |window, cx| {
        assert!(window.try_find("settings-page").is_some());
        assert!(window.try_find("session-view").is_none());
        window.press("escape", cx);
    });
    assert!(cx.read(|cx| workspace.read(cx).settings().is_none()));
}

#[gpui_kit::test]
fn the_theme_follows_the_mode_and_keeps_the_zoom(cx: &mut TestAppContext) {
    start(cx);
    let (window, _) = open(cx, 1300.);
    let name = |cx: &mut TestAppContext| cx.read(|cx| Theme::global(cx).theme_name().to_string());
    cx.update(|cx| prefs::update(cx, |prefs| prefs.appearance.theme_mode = ThemeMode::Dark));
    assert_eq!(name(cx), prefs::DEFAULT_DARK_THEME);
    act(cx, window, |window, cx| window.press("secondary-=", cx));
    act(cx, window, |window, cx| window.press("secondary-=", cx));
    assert_eq!(cx.read(|cx| prefs::get(cx).appearance.zoom), 1.25);
    assert_eq!(cx.read(|cx| Theme::global(cx).font_size), px(20.));
    cx.update(|cx| prefs::update(cx, |prefs| prefs.appearance.theme_mode = ThemeMode::Light));
    assert_eq!(name(cx), prefs::DEFAULT_LIGHT_THEME);
    assert_eq!(
        cx.read(|cx| Theme::global(cx).font_size),
        px(20.),
        "kept across themes"
    );
    act(cx, window, |window, cx| window.press("secondary-0", cx));
    assert_eq!(cx.read(|cx| Theme::global(cx).font_size), px(16.));
}

#[gpui_kit::test]
fn turning_code_ligatures_off_reaches_the_code_font_and_outlasts_a_theme_change(
    cx: &mut TestAppContext,
) {
    start(cx);
    let _ = open(cx, 1300.);
    // The theme's copy, which the app's own code text reads, and the one
    // GPUI Kit's text views read.
    let features = |cx: &mut TestAppContext| {
        cx.read(|cx| {
            (
                Theme::global(cx).mono_font_features.clone(),
                gpui_kit::base::Theme::global(cx)
                    .tokens
                    .typography
                    .mono_features
                    .clone(),
            )
        })
    };
    let font_defaults = (
        gpui_kit::FontFeatures::default(),
        gpui_kit::FontFeatures::default(),
    );
    assert_eq!(features(cx), font_defaults, "on by default");

    cx.update(|cx| prefs::update(cx, |prefs| prefs.appearance.code_ligatures = false));
    let off = crate::fonts::no_ligatures();
    assert_eq!(features(cx), (off.clone(), off.clone()));
    cx.update(|cx| prefs::update(cx, |prefs| prefs.appearance.theme_mode = ThemeMode::Dark));
    assert_eq!(features(cx), (off.clone(), off), "kept across themes");

    cx.update(|cx| prefs::update(cx, |prefs| prefs.appearance.code_ligatures = true));
    assert_eq!(features(cx), font_defaults);
}

#[gpui_kit::test]
fn system_mode_follows_the_systems_appearance(cx: &mut TestAppContext) {
    start(cx);
    let _ = open(cx, 1300.);
    let name = |cx: &mut TestAppContext| cx.read(|cx| Theme::global(cx).theme_name().to_string());
    cx.update(|cx| crate::appearance::set_system_appearance(gpui_kit::WindowAppearance::Dark, cx));
    assert_eq!(name(cx), prefs::DEFAULT_DARK_THEME);
    cx.update(|cx| crate::appearance::set_system_appearance(gpui_kit::WindowAppearance::Light, cx));
    assert_eq!(name(cx), prefs::DEFAULT_LIGHT_THEME);
}

#[gpui_kit::test]
fn the_theme_lists_hold_the_bundled_themes_by_mode(cx: &mut TestAppContext) {
    start(cx);
    let light = cx.read(|cx| crate::appearance::theme_names(false, cx));
    let dark = cx.read(|cx| crate::appearance::theme_names(true, cx));
    assert!(light.iter().any(|name| name == prefs::DEFAULT_LIGHT_THEME));
    assert!(dark.iter().any(|name| name == prefs::DEFAULT_DARK_THEME));
    assert!(!light.iter().any(|name| name == prefs::DEFAULT_DARK_THEME));
    // GPUI Kit's own pair stays out of them.
    assert_eq!(
        light.len() + dark.len(),
        crate::appearance::parse_bundled().len()
    );
}

#[gpui_kit::test]
fn a_theme_that_changes_while_in_use_shows_at_once(cx: &mut TestAppContext) {
    start(cx);
    cx.update(|cx| prefs::update(cx, |prefs| prefs.appearance.theme_mode = ThemeMode::Light));
    // As when the user edits their copy of the theme in the themes folder.
    cx.update(|cx| {
        let theme = crate::appearance::find(prefs::DEFAULT_LIGHT_THEME, cx).unwrap();
        let mut edited = (*theme).clone();
        edited.radius = Some(13);
        ThemeRegistry::global_mut(cx).add_bundled_themes([edited]);
    });
    cx.run_until_parked();
    assert_eq!(cx.read(|cx| Theme::global(cx).radius), px(13.));
}

#[gpui_kit::test]
fn the_theme_list_asks_once_then_previews_after_a_rest(cx: &mut TestAppContext) {
    start(cx);
    let (window, workspace) = open(cx, 1300.);
    cx.update(|cx| prefs::update(cx, |prefs| prefs.appearance.theme_mode = ThemeMode::Dark));
    act(cx, window, |window, cx| window.press("secondary-,", cx));
    let picker = cx.read(|cx| {
        workspace
            .read(cx)
            .settings()
            .unwrap()
            .read(cx)
            .dark_theme()
            .clone()
    });
    let name = |cx: &mut TestAppContext| cx.read(|cx| Theme::global(cx).theme_name().to_string());

    // The first time, the notice comes before any preview.
    act(cx, window, |window, cx| {
        window.click("dark-theme-button", cx)
    });
    act(cx, window, |window, cx| {
        assert!(window.try_find("flash-notice").is_some());
        window.click("preview-themes", cx);
    });
    assert!(cx.read(|cx| prefs::get(cx).appearance.flash_notice));

    // Moving rests, then shows the theme moved to.
    act(cx, window, |window, cx| window.press("down", cx));
    assert_eq!(name(cx), prefs::DEFAULT_DARK_THEME, "not before the rest");
    cx.executor().advance_clock(PREVIEW_REST);
    cx.run_until_parked();
    let previewed = name(cx);
    assert_ne!(previewed, prefs::DEFAULT_DARK_THEME);

    // Esc goes back to the chosen theme at once.
    act(cx, window, |window, cx| window.press("escape", cx));
    assert!(!cx.read(|cx| picker.read(cx).is_open()));
    assert_eq!(name(cx), prefs::DEFAULT_DARK_THEME);
    assert_eq!(
        cx.read(|cx| prefs::get(cx).appearance.dark_theme.clone()),
        prefs::DEFAULT_DARK_THEME
    );

    // Enter keeps the one the list rests on.
    act(cx, window, |window, cx| {
        window.click("dark-theme-button", cx)
    });
    act(cx, window, |window, cx| window.press("down", cx));
    act(cx, window, |window, cx| window.press("enter", cx));
    assert_eq!(
        cx.read(|cx| prefs::get(cx).appearance.dark_theme.clone()),
        previewed
    );
    assert_eq!(name(cx), previewed);
}

#[gpui_kit::test]
fn with_previews_off_moving_shows_nothing(cx: &mut TestAppContext) {
    start(cx);
    let (window, _) = open(cx, 1300.);
    cx.update(|cx| {
        prefs::update(cx, |prefs| {
            prefs.appearance.theme_mode = ThemeMode::Dark;
            prefs.appearance.flash_notice = true;
            prefs.appearance.theme_previews = false;
        })
    });
    act(cx, window, |window, cx| window.press("secondary-,", cx));
    act(cx, window, |window, cx| {
        window.click("dark-theme-button", cx)
    });
    act(cx, window, |window, cx| window.press("down", cx));
    cx.executor().advance_clock(PREVIEW_REST * 4);
    cx.run_until_parked();
    assert_eq!(
        cx.read(|cx| Theme::global(cx).theme_name().to_string()),
        prefs::DEFAULT_DARK_THEME
    );
}

#[gpui_kit::test]
fn new_session_leaves_the_open_one(cx: &mut TestAppContext) {
    start(cx);
    let (window, workspace) = open(cx, 1300.);
    let row = row_of(&workspace, "Split the engine out of main.rs", cx);
    act(cx, window, |window, cx| window.click(("row", row), cx));
    assert!(open_title(&workspace, cx).is_some());
    act(cx, window, |window, cx| window.press("secondary-n", cx));
    assert_eq!(open_title(&workspace, cx), None);
}

#[gpui_kit::test]
fn the_status_bar_keeps_its_height_with_nothing_to_show(cx: &mut TestAppContext) {
    start(cx);
    let (window, _) = open(cx, 1300.);
    act(cx, window, |window, _| {
        assert_eq!(window.find("status-bar").bounds().size.height, px(28.));
    });
}

#[gpui_kit::test]
fn new_session_lines_up_with_the_rows_below(cx: &mut TestAppContext) {
    start(cx);
    let (window, workspace) = open(cx, 1300.);
    let row = row_of(&workspace, "Split the engine out of main.rs", cx);
    act(cx, window, |window, cx| {
        let button = window.find("new-session").bounds();
        let group = window.find(("row", 0usize)).bounds();
        assert_eq!(button.left(), group.left());
        assert_eq!(button.size.width, group.size.width);
        window.click(("row", row), cx);
    });
    assert!(open_title(&workspace, cx).is_some());
    act(cx, window, |window, cx| window.click("new-session", cx));
    assert_eq!(open_title(&workspace, cx), None);
}

#[gpui_kit::test]
fn the_layout_buttons_sit_together_and_the_workers_one_comes_with_workers(cx: &mut TestAppContext) {
    start(cx);
    let (window, workspace) = open(cx, 1300.);
    act(cx, window, |window, cx| {
        assert!(
            window.try_find("toggle-workers").is_none(),
            "a new session has no workers"
        );
        window.click("toggle-sidebar", cx);
    });
    assert!(!cx.read(|cx| prefs::get(cx).sidebar.open));
    act(cx, window, |window, cx| window.click("toggle-sidebar", cx));
    assert!(cx.read(|cx| prefs::get(cx).sidebar.open));

    let row = row_of(&workspace, "Fix the handover size for workers", cx);
    act(cx, window, |window, cx| window.click(("row", row), cx));
    act(cx, window, |window, cx| {
        // At the right end, after the session's title.
        let sidebar = window.find("toggle-sidebar").bounds();
        let workers = window.find("toggle-workers").bounds();
        let title = window.find("session-title").bounds();
        assert!(sidebar.left() >= title.right() && workers.left() > sidebar.left());
        window.click("toggle-workers", cx);
    });
    assert!(cx.read(|cx| workspace.read(cx).is_workers_open()));
}

#[gpui_kit::test]
fn a_new_session_picks_chat_or_agent_in_its_empty_transcript(cx: &mut TestAppContext) {
    start(cx);
    let (window, workspace) = open(cx, 1300.);
    let view = cx.read(|cx| workspace.read(cx).session_view().clone());
    let line = |cx: &mut TestAppContext| cx.read(|cx| view.read(cx).draft_line());
    act(cx, window, |window, _| {
        assert!(window.try_find("empty-mode-switch").is_some());
        assert!(window.try_find("empty-mode-about").is_some());
    });
    act(cx, window, |window, cx| window.click("empty-mode-chat", cx));
    assert_eq!(line(cx).as_deref(), Some("New chat"));
    // ←/→ switch while the switch has focus.
    act(cx, window, |window, cx| window.press("right", cx));
    assert_eq!(line(cx).as_deref(), Some("New agent session in ~/src/arko"));
    act(cx, window, |window, cx| window.press("left", cx));
    assert_eq!(line(cx).as_deref(), Some("New chat"));
    // An open session has it in the chip's popover only.
    let row = row_of(&workspace, "Split the engine out of main.rs", cx);
    act(cx, window, |window, cx| window.click(("row", row), cx));
    act(cx, window, |window, _| {
        assert!(window.try_find("empty-mode-switch").is_none())
    });
}

#[gpui_kit::test]
fn a_new_session_says_where_it_will_run_and_the_chip_switches_it(cx: &mut TestAppContext) {
    start(cx);
    let (window, workspace) = open(cx, 1300.);
    let view = cx.read(|cx| workspace.read(cx).session_view().clone());
    let line = |cx: &mut TestAppContext| cx.read(|cx| view.read(cx).draft_line());
    let chip = |cx: &mut TestAppContext| cx.read(|cx| view.read(cx).chip_label(cx));
    // The most recent agent session's folder, and the last session's model.
    assert_eq!(line(cx).as_deref(), Some("New agent session in ~/src/arko"));
    assert_eq!(chip(cx), "Agent · Opus 5.5");

    act(cx, window, |window, cx| window.click("mode-model", cx));
    act(cx, window, |window, cx| window.click("mode-chat", cx));
    assert_eq!(line(cx).as_deref(), Some("New chat"));
    assert_eq!(chip(cx), "Chat · Opus 5.5");
    act(cx, window, |window, cx| window.click("mode-agent", cx));
    act(cx, window, |window, cx| window.press("escape", cx));

    // Change… lists the recent folders.
    act(cx, window, |window, cx| window.click("change-folder", cx));
    act(cx, window, |window, cx| {
        window.within("popup-menu").click(1usize, cx)
    });
    assert_eq!(
        line(cx).as_deref(),
        Some("New agent session in ~/src/gpui_kit")
    );

    // An open session shows its transcript instead.
    let row = row_of(&workspace, "Split the engine out of main.rs", cx);
    act(cx, window, |window, cx| window.click(("row", row), cx));
    assert_eq!(line(cx), None);
}

/// Opens the command palette, types `query` and takes the first match.
fn run_from_palette(cx: &mut TestAppContext, window: AnyWindowHandle, query: &str) {
    act(cx, window, |window, cx| window.press("secondary-k", cx));
    act(cx, window, |window, cx| {
        assert!(window.try_find("command-palette").is_some());
        window.input(query, cx);
    });
    act(cx, window, |window, cx| window.press("enter", cx));
    act(cx, window, |window, _| {
        assert!(window.try_find("command-palette").is_none(), "closed");
    });
}

#[gpui_kit::test]
fn the_command_palette_runs_commands_in_the_window(cx: &mut TestAppContext) {
    start(cx);
    let (window, workspace) = open(cx, 1300.);
    run_from_palette(cx, window, "toggle sidebar");
    assert!(!cx.read(|cx| prefs::get(cx).sidebar.open));
    // Mod+Shift+P opens it too.
    act(cx, window, |window, cx| {
        window.press("secondary-shift-p", cx)
    });
    act(cx, window, |window, cx| {
        assert!(window.try_find("command-palette").is_some());
        window.press("escape", cx);
    });
    act(cx, window, |window, _| {
        assert!(window.try_find("command-palette").is_none())
    });
    run_from_palette(cx, window, "expand composer");
    assert!(cx.read(|cx| prefs::get(cx).input.composer_expanded));
    assert!(cx.read(|cx| workspace.read(cx).settings().is_none()));
}

#[gpui_kit::test]
fn the_command_palette_opens_settings_pages_and_sessions(cx: &mut TestAppContext) {
    start(cx);
    let (window, workspace) = open(cx, 1300.);
    run_from_palette(cx, window, "settings › notifications");
    let page = cx.read(|cx| {
        workspace
            .read(cx)
            .settings()
            .map(|settings| settings.read(cx).page())
    });
    assert_eq!(page, Some(4));
    run_from_palette(cx, window, "naming a desktop");
    assert_eq!(
        open_title(&workspace, cx).as_deref(),
        Some("Naming a desktop app")
    );
    assert!(cx.read(|cx| workspace.read(cx).settings().is_none()));
}

#[gpui_kit::test]
fn every_bound_key_is_on_the_shortcuts_sheet(cx: &mut TestAppContext) {
    start(cx);
    let sheet = crate::shortcuts::shortcuts();
    let keymap = cx.read(|cx| cx.key_bindings());
    let keymap = keymap.borrow();
    let ours = ["app::", "session_list::", "settings_page::"];
    let mut checked = 0;
    for binding in keymap.bindings() {
        let action = binding.action().name();
        if !ours.iter().any(|namespace| action.starts_with(namespace)) {
            continue;
        }
        let stroke = gpui_kit::AsKeystroke::as_keystroke(&binding.keystrokes()[0]).clone();
        // The sheet lists a chord with Shift on a symbol key as it reads
        // (Ctrl+Shift+\). The keymap has it as the platforms send it too.
        assert!(
            sheet.iter().any(|row| row.action == action
                && row.keys.iter().any(|key| {
                    key == &stroke
                        || crate::commands::delivered_form(&key.unparse())
                            .and_then(|form| gpui_kit::Keystroke::parse(&form).ok())
                            .as_ref()
                            == Some(&stroke)
                })),
            "{action} on {stroke:?} isn't on the sheet"
        );
        checked += 1;
    }
    assert!(checked > 30, "only {checked} bindings");
}

#[gpui_kit::test]
fn the_shortcuts_sheet_opens_on_mod_slash_and_searches(cx: &mut TestAppContext) {
    start(cx);
    let (window, _) = open(cx, 1300.);
    let sheet = crate::shortcuts::shortcuts();
    let row = |name: &str| sheet.iter().position(|row| row.name == name).unwrap();
    let (zoom_in, stop) = (row("Zoom In"), row("Stop"));
    act(cx, window, |window, cx| window.press("secondary-/", cx));
    act(cx, window, |window, cx| {
        assert!(window.try_find("shortcuts").is_some());
        assert!(window.try_find(("shortcut", zoom_in)).is_some());
        assert!(window.try_find(("shortcut", stop)).is_some());
        window.input("zoom", cx);
    });
    act(cx, window, |window, _| {
        assert!(window.try_find(("shortcut", zoom_in)).is_some());
        assert!(window.try_find(("shortcut", stop)).is_none());
    });
}

#[gpui_kit::test]
fn the_composer_follows_smooth_typing_and_keybindings(cx: &mut TestAppContext) {
    start(cx);
    let (_, workspace) = open(cx, 1300.);
    let composer = cx.read(|cx| {
        workspace
            .read(cx)
            .session_view()
            .read(cx)
            .composer()
            .read(cx)
            .textarea()
            .clone()
    });
    assert!(cx.read(|cx| composer.read(cx).has_smooth_caret()));
    cx.update(|cx| {
        prefs::update(cx, |prefs| {
            prefs.input.smooth_typing = false;
            prefs.input.keymap = prefs::Keymap::Emacs;
        })
    });
    assert!(!cx.read(|cx| composer.read(cx).has_smooth_caret()));
    assert_eq!(
        cx.read(|cx| composer.read(cx).current_keymap()),
        gpui_kit::component::input::Keymap::Emacs
    );
}

#[gpui_kit::test]
fn reset_preferences_asks_first_and_keeps_the_windows_place(cx: &mut TestAppContext) {
    start(cx);
    let (window, workspace) = open(cx, 1300.);
    cx.update(|cx| {
        prefs::update(cx, |prefs| {
            prefs.appearance.zoom = 1.5;
            prefs.input.keymap = prefs::Keymap::Vim;
            prefs.window = Some(prefs::WindowPlacement {
                x: 10.,
                y: 20.,
                width: 1300.,
                height: 720.,
                maximized: false,
            });
        })
    });
    act(cx, window, |window, cx| window.press("secondary-,", cx));
    cx.update(|cx| {
        let settings = workspace.read(cx).settings().unwrap().clone();
        settings.update(cx, |settings, cx| settings.show_page(7, cx));
    });
    act(cx, window, |window, cx| {
        // Advanced's first buttons show. Reset sits lower, under the fold of
        // the test's window, so its action goes in as its button sends it.
        assert!(window.try_find("open-data").is_some());
        window.dispatch_action(Box::new(crate::settings_page::ResetPreferences), cx);
    });
    assert_eq!(
        cx.read(|cx| prefs::get(cx).appearance.zoom),
        1.5,
        "asks first"
    );
    act(cx, window, |window, cx| {
        window.within("dialog").click("ok", cx)
    });
    cx.read(|cx| {
        let prefs = prefs::get(cx);
        assert_eq!(prefs.appearance.zoom, 1.0);
        assert_eq!(prefs.input.keymap, prefs::Keymap::Standard);
        assert_eq!(prefs.window.as_ref().map(|window| window.x), Some(10.));
    });
}

#[gpui_kit::test]
fn about_shows_the_version_license_and_what_the_app_includes(cx: &mut TestAppContext) {
    start(cx);
    let (window, _) = open(cx, 1300.);
    act(cx, window, |window, cx| {
        window.dispatch_action(Box::new(menus::About), cx)
    });
    act(cx, window, |window, _| {
        assert!(window.try_find("about").is_some());
        // The tests run without the built-in fonts.
        assert!(window.try_find("credit-fonts").is_none());
        assert!(window.try_find("credit-themes").is_some());
        assert!(window.try_find("credit-icons").is_some());
        assert!(window.try_find("source-link").is_some());
    });
}

#[gpui_kit::test]
fn a_theme_in_the_themes_folder_replaces_the_bundled_one(cx: &mut TestAppContext) {
    let folder = tempfile::tempdir().unwrap();
    std::fs::write(
        folder.path().join("mine.json"),
        r#"{ "name": "Mine", "themes": [
            { "name": "MACOS CLASSIC LIGHT", "mode": "light", "radius": 13 },
            { "name": "Mine Dark", "mode": "dark" }
        ] }"#,
    )
    .unwrap();
    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    let mut settings = prefs::Prefs::default();
    settings.appearance.theme_mode = ThemeMode::Light;
    let startup = Startup::for_tests(backend)
        .with_prefs(settings)
        .with_themes_folder(folder.path().to_path_buf());
    cx.update(|cx| crate::init(startup, cx));
    // The folder is read on a background thread.
    cx.run_until_parked();
    assert_eq!(cx.read(|cx| Theme::global(cx).radius), px(13.));
    let dark = cx.read(|cx| crate::appearance::theme_names(true, cx));
    assert!(dark.iter().any(|name| name == "Mine Dark"));
    let light = cx.read(|cx| crate::appearance::theme_names(false, cx));
    let same = light
        .iter()
        .filter(|name| name.eq_ignore_ascii_case(prefs::DEFAULT_LIGHT_THEME))
        .count();
    assert_eq!(same, 1, "replaced, not listed twice");
}

#[gpui_kit::test]
fn the_font_pickers_offer_only_what_the_settings_keep(cx: &mut TestAppContext) {
    start(cx);
    let (window, _) = open(cx, 1300.);
    act(cx, window, |window, cx| window.press("secondary-,", cx));
    act(cx, window, |window, _| {
        assert!(window.try_find("settings-page").is_some());
        // A family and a size: no weight, line height or features.
        assert!(window.try_find("font-picker-weight").is_none());
    });
}
