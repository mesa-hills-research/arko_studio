//! Interaction tests: the app's real windows, driven by clicks and keys on
//! GPUI's test platform, on the fake backend's sample sessions.

use std::sync::Arc;
use std::time::SystemTime;

use gpui_kit::component::{Theme, WindowExt as _};
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

fn open(cx: &mut TestAppContext, width: f32) -> (AnyWindowHandle, Entity<Workspace>) {
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
fn act(cx: &mut TestAppContext, window: AnyWindowHandle, act: impl FnOnce(&mut Window, &mut App)) {
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        act(window, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

fn open_title(workspace: &Entity<Workspace>, cx: &mut TestAppContext) -> Option<String> {
    cx.read(|cx| {
        workspace
            .read(cx)
            .session_view()
            .read(cx)
            .session()
            .map(|session| session.title.clone())
    })
}

fn row_of(workspace: &Entity<Workspace>, title: &str, cx: &mut TestAppContext) -> usize {
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
fn a_new_session_says_where_it_will_run_and_the_chip_switches_it(cx: &mut TestAppContext) {
    start(cx);
    let (window, workspace) = open(cx, 1300.);
    let view = cx.read(|cx| workspace.read(cx).session_view().clone());
    let line = |cx: &mut TestAppContext| cx.read(|cx| view.read(cx).draft_line());
    let chip = |cx: &mut TestAppContext| cx.read(|cx| view.read(cx).chip_label());
    // The most recent agent session's folder, and the last session's model.
    assert_eq!(line(cx).as_deref(), Some("New agent session in ~/src/arko"));
    assert_eq!(chip(cx), "Agent · claude-opus-5-5");

    act(cx, window, |window, cx| window.click("mode-model", cx));
    act(cx, window, |window, cx| window.click("mode-chat", cx));
    assert_eq!(line(cx).as_deref(), Some("New chat"));
    assert_eq!(chip(cx), "Chat · claude-opus-5-5");
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
