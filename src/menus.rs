//! Actions, key bindings and the menus.
//!
//! macOS shows the menus in the system menu bar. Windows and Linux show them
//! in the title bar, through GPUI Kit's `AppMenuBar`. `secondary` in a key
//! binding is Cmd on macOS and Ctrl elsewhere (the design's Mod).

use gpui_kit::component::{GlobalState, input};
use gpui_kit::*;

use crate::{APP_NAME, appearance, chrome, prefs};

actions!(
    app,
    [
        About,
        OpenSettings,
        CloseWindow,
        Minimize,
        Zoom,
        Hide,
        HideOthers,
        ShowAll,
        Quit,
        NewWindow,
        NewSession,
        ExportMarkdown,
        Find,
        FindNext,
        FindPrevious,
        ToggleSidebar,
        ToggleWorkersPanel,
        FocusSessionFilter,
        CommandPalette,
        CycleView,
        ViewFull,
        ViewQuiet,
        ViewMessages,
        ZoomIn,
        ZoomOut,
        ResetZoom,
        SessionInfo,
        RenameSession,
        DeleteSession,
        BranchSession,
        StopTurn,
        MoveToBackground,
        HandOverNow,
        ExpandComposer,
        KeyboardShortcuts,
        OpenLogsFolder,
        OpenDataFolder,
        CloseSession,
        OpenAccounts,
        ShareWebPage,
        StopSharingWebPage,
        RefreshCache,
    ]
);

pub fn init(cx: &mut App) {
    // App-wide actions. The window actions are handled in `Workspace`.
    cx.on_action(|_: &Quit, cx| chrome::quit(cx));
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    cx.on_action(|_: &NewWindow, cx| {
        crate::workspace::open(cx);
    });
    cx.on_action(|_: &ZoomIn, cx| appearance::zoom(1, cx));
    cx.on_action(|_: &ZoomOut, cx| appearance::zoom(-1, cx));
    cx.on_action(|_: &ResetZoom, cx| appearance::zoom(0, cx));
    cx.on_action(|_: &OpenLogsFolder, cx| {
        if let Some(folder) = crate::paths::logs_folder() {
            let _ = std::fs::create_dir_all(&folder);
            cx.open_with_system(&folder);
        }
    });
    cx.on_action(|_: &OpenDataFolder, cx| {
        let root = cx
            .global::<crate::backend::SharedBackend>()
            .data_root()
            .to_path_buf();
        cx.open_with_system(&root);
    });

    // Windows and Linux close windows with Alt+F4 through the system, which
    // runs the close check.
    cx.bind_keys(crate::commands::key_bindings());
    set_menus(cx);
    // The menus show the sidebar's and the panel's state, so they follow it.
    cx.observe_global::<prefs::PrefsStore>(|cx| {
        if menu_state(cx) != cx.global::<MenuState>().0 {
            set_menus(cx);
        }
    })
    .detach();
}

/// What the menus show of the app's state.
#[derive(Clone, Copy, PartialEq, Default)]
pub(crate) struct MenuStateKey {
    sidebar_open: bool,
    view: prefs::TranscriptView,
}

/// The state the menus were last built for. Title bars observe it to draw
/// the menus again.
#[derive(Default)]
pub struct MenuState(MenuStateKey);

impl Global for MenuState {}

fn menu_state(cx: &App) -> MenuStateKey {
    MenuStateKey {
        sidebar_open: prefs::get(cx).sidebar.open,
        view: prefs::get(cx).view,
    }
}

/// A menu item with a check mark when `checked`.
fn checked(name: &str, action: impl Action, checked: bool) -> MenuItem {
    MenuItem::Action {
        name: name.to_string().into(),
        action: Box::new(action),
        os_action: None,
        checked,
        disabled: false,
    }
}

/// Builds the menus for the system menu bar (macOS) and the title bars.
pub fn set_menus(cx: &mut App) {
    let state = menu_state(cx);
    cx.set_global(MenuState(state));
    cx.set_menus(menus(state));
    // AppMenuBar draws the menus it finds here.
    GlobalState::global_mut(cx).set_app_menus(menus(state).into_iter().map(Menu::owned).collect());
}

pub(crate) fn menus(state: MenuStateKey) -> Vec<Menu> {
    let mac = cfg!(target_os = "macos");
    let mut menus = Vec::new();
    if mac {
        // macOS titles the first menu with the app's name.
        menus.push(Menu::new(APP_NAME).items([
            MenuItem::action(format!("About {APP_NAME}"), About),
            MenuItem::separator(),
            MenuItem::action("Settings…", OpenSettings),
            MenuItem::separator(),
            MenuItem::os_submenu("Services", SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action(format!("Hide {APP_NAME}"), Hide),
            MenuItem::action("Hide Others", HideOthers),
            MenuItem::action("Show All", ShowAll),
            MenuItem::separator(),
            MenuItem::action(format!("Quit {APP_NAME}"), Quit),
        ]));
    }
    menus.push(file_menu(mac));
    menus.push(edit_menu());
    menus.push(view_menu(state));
    menus.push(session_menu());
    menus.push(window_menu(mac));
    menus.push(help_menu(mac));
    menus
}

fn file_menu(mac: bool) -> Menu {
    let mut items = vec![
        MenuItem::action("New Session", NewSession),
        MenuItem::action("New Window", NewWindow),
    ];
    if mac {
        items.extend([
            MenuItem::separator(),
            MenuItem::action("Close Window", CloseWindow),
        ]);
    } else {
        let quit = if cfg!(target_os = "windows") {
            "Exit"
        } else {
            "Quit"
        };
        items.extend([
            MenuItem::separator(),
            MenuItem::action("Settings…", OpenSettings),
            MenuItem::separator(),
            MenuItem::action(quit, Quit),
        ]);
    }
    Menu::new("File").items(items)
}

/// Text editing, with the OS actions that also reach system panels such as
/// the macOS open dialog.
fn edit_menu() -> Menu {
    Menu::new("Edit").items([
        MenuItem::os_action("Undo", input::Undo, OsAction::Undo),
        MenuItem::os_action("Redo", input::Redo, OsAction::Redo),
        MenuItem::separator(),
        MenuItem::os_action("Cut", input::Cut, OsAction::Cut),
        MenuItem::os_action("Copy", input::Copy, OsAction::Copy),
        MenuItem::os_action("Paste", input::Paste, OsAction::Paste),
        MenuItem::os_action("Select All", input::SelectAll, OsAction::SelectAll),
        MenuItem::separator(),
        MenuItem::action("Find…", Find),
    ])
}

fn view_menu(state: MenuStateKey) -> Menu {
    let sidebar = if state.sidebar_open {
        "Hide Sidebar"
    } else {
        "Show Sidebar"
    };
    Menu::new("View").items([
        MenuItem::action(sidebar, ToggleSidebar),
        MenuItem::action("Workers Panel", ToggleWorkersPanel),
        MenuItem::separator(),
        checked(
            "Full Transcript",
            ViewFull,
            state.view == prefs::TranscriptView::Full,
        ),
        checked(
            "Quiet Transcript",
            ViewQuiet,
            state.view == prefs::TranscriptView::Quiet,
        ),
        checked(
            "Messages Only",
            ViewMessages,
            state.view == prefs::TranscriptView::Messages,
        ),
        MenuItem::action("Next Transcript View", CycleView),
        MenuItem::separator(),
        MenuItem::action("Zoom In", ZoomIn),
        MenuItem::action("Zoom Out", ZoomOut),
        MenuItem::action("Actual Size", ResetZoom),
        MenuItem::separator(),
        MenuItem::action("Command Palette…", CommandPalette),
        MenuItem::action("Go to Session…", FocusSessionFilter),
    ])
}

fn session_menu() -> Menu {
    Menu::new("Session").items([
        MenuItem::action("Session Info…", SessionInfo),
        MenuItem::action("Rename…", RenameSession),
        MenuItem::action("Branch from Here", BranchSession),
        MenuItem::action("Export as Markdown…", ExportMarkdown),
        MenuItem::separator(),
        MenuItem::action("Stop", StopTurn),
        MenuItem::action("Move Command to Background", MoveToBackground),
        MenuItem::action("Hand Over Now", HandOverNow),
        MenuItem::action("Refresh Cache", RefreshCache),
        MenuItem::separator(),
        MenuItem::action("Share Web Page", ShareWebPage),
        MenuItem::action("Stop Sharing Web Page", StopSharingWebPage),
        MenuItem::separator(),
        MenuItem::action("Close Session", CloseSession),
        MenuItem::action("Delete Session…", DeleteSession),
    ])
}

/// A menu named "Window" also lists the open windows on macOS.
fn window_menu(mac: bool) -> Menu {
    let zoom = if mac { "Zoom" } else { "Maximize" };
    Menu::new("Window").items([
        MenuItem::action("Minimize", Minimize),
        MenuItem::action(zoom, Zoom),
        MenuItem::separator(),
        MenuItem::action("New Window", NewWindow),
    ])
}

fn help_menu(mac: bool) -> Menu {
    let mut items = vec![
        MenuItem::action("Keyboard Shortcuts", KeyboardShortcuts),
        MenuItem::separator(),
        MenuItem::action("Open Arko’s Folder", OpenDataFolder),
        MenuItem::action("Open Logs Folder", OpenLogsFolder),
    ];
    if !mac {
        items.extend([
            MenuItem::separator(),
            MenuItem::action(format!("About {APP_NAME}"), About),
        ]);
    }
    Menu::new("Help").items(items)
}

#[cfg(test)]
mod tests {
    // Named, not `super::*`: GPUI's glob brings a `test` macro that would
    // shadow Rust's.
    use super::{MenuStateKey, menus};
    use crate::APP_NAME;
    use gpui_kit::{Menu, MenuItem};

    fn names(menu: &Menu) -> Vec<String> {
        menu.items
            .iter()
            .filter_map(|item| match item {
                MenuItem::Action { name, .. } => Some(name.to_string()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_menus_are_in_the_designed_order() {
        let menus = menus(MenuStateKey {
            sidebar_open: true,
            ..MenuStateKey::default()
        });
        let titles: Vec<_> = menus.iter().map(|menu| menu.name.to_string()).collect();
        let expected = ["File", "Edit", "View", "Session", "Window", "Help"];
        if cfg!(target_os = "macos") {
            assert_eq!(titles[0], APP_NAME);
            assert_eq!(&titles[1..], expected);
        } else {
            assert_eq!(titles, expected);
        }
    }

    #[test]
    fn the_view_menu_names_what_the_sidebar_item_does() {
        let open = menus(MenuStateKey {
            sidebar_open: true,
            ..MenuStateKey::default()
        });
        let closed = menus(MenuStateKey::default());
        let view = |menus: &[Menu]| {
            names(menus.iter().find(|menu| menu.name == "View").unwrap())[0].clone()
        };
        assert_eq!(view(&open), "Hide Sidebar");
        assert_eq!(view(&closed), "Show Sidebar");
    }

    #[test]
    fn items_that_open_a_dialog_end_in_an_ellipsis() {
        for menu in menus(MenuStateKey::default()) {
            for name in names(&menu) {
                let opens_more = [
                    "Settings", "Export", "Find", "Rename", "Delete", "Info", "Palette", "Go to",
                ]
                .iter()
                .any(|word| name.contains(word));
                assert_eq!(name.ends_with('…'), opens_more, "{name}");
            }
        }
    }
}
