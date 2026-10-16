//! Actions, key bindings and the menus.
//!
//! macOS shows the menus in the system menu bar. Windows and Linux show them
//! in the title bar, through GPUI Kit's `AppMenuBar`.

use gpui_kit::component::{GlobalState, input};
use gpui_kit::*;

use crate::{APP_NAME, chrome};

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
    ]
);

pub fn init(cx: &mut App) {
    // App-wide actions. The window actions are handled in `Workspace`.
    cx.on_action(|_: &Quit, cx| chrome::quit(cx));
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());

    // `secondary` is Cmd on macOS and Ctrl elsewhere. Windows and Linux close
    // windows with Alt+F4 through the system, which runs the close check.
    cx.bind_keys([
        KeyBinding::new("secondary-q", Quit, None),
        KeyBinding::new("secondary-,", OpenSettings, None),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-w", CloseWindow, None),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-m", Minimize, None),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-h", Hide, None),
        #[cfg(target_os = "macos")]
        KeyBinding::new("alt-cmd-h", HideOthers, None),
    ]);

    cx.set_menus(menus());
    // AppMenuBar draws the menus it finds here.
    GlobalState::global_mut(cx).set_app_menus(menus().into_iter().map(Menu::owned).collect());
}

fn menus() -> Vec<Menu> {
    if cfg!(target_os = "macos") {
        vec![
            // macOS titles the first menu with the app's name.
            Menu::new(APP_NAME).items([
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
            ]),
            Menu::new("File").items([MenuItem::action("Close Window", CloseWindow)]),
            edit_menu(),
            // A menu named "Window" also lists the open windows.
            Menu::new("Window").items([
                MenuItem::action("Minimize", Minimize),
                MenuItem::action("Zoom", Zoom),
            ]),
        ]
    } else {
        let quit = if cfg!(target_os = "windows") {
            "Exit"
        } else {
            "Quit"
        };
        vec![
            Menu::new("File").items([
                MenuItem::action("Settings…", OpenSettings),
                MenuItem::separator(),
                MenuItem::action(quit, Quit),
            ]),
            edit_menu(),
            Menu::new("Help").items([MenuItem::action(format!("About {APP_NAME}"), About)]),
        ]
    }
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
    ])
}
