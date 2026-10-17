//! Every command of the app in one table: its name, its group, its action and
//! its key chords. The key bindings, the command palette and the keyboard
//! shortcuts sheet are all made from it, so they can't disagree. The menus
//! dispatch the same actions (a test checks that each one is in the table).

use std::rc::Rc;

use gpui_kit::*;

use crate::menus::*;
use crate::settings_page::CloseSettings;
use crate::sidebar::{
    CancelRename, ClearFilter, Collapse, Expand, FocusList, OpenSelected, SelectFirst, SelectLast,
    SelectNext, SelectPrevious,
};
use crate::sidebar::{FILTER_CONTEXT, LIST_CONTEXT, RENAME_CONTEXT};
use crate::workers_panel;

/// The sections of the shortcuts sheet and the palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    General,
    Sessions,
    Composer,
    Transcript,
    Workers,
    View,
}

impl Group {
    pub const ALL: [Group; 6] = [
        Group::General,
        Group::Sessions,
        Group::Composer,
        Group::Transcript,
        Group::Workers,
        Group::View,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Group::General => "General",
            Group::Sessions => "Sessions",
            Group::Composer => "Composer",
            Group::Transcript => "Transcript",
            Group::Workers => "Workers",
            Group::View => "View",
        }
    }
}

/// Makes a key binding of a chord and a key context for a command's action.
type Bind = Rc<dyn Fn(&str, Option<&str>) -> KeyBinding>;

/// One command.
pub struct Command {
    pub name: &'static str,
    pub group: Group,
    pub action: Box<dyn Action>,
    /// Its key chords, in GPUI's notation: `secondary` is Cmd on macOS and
    /// Ctrl elsewhere.
    pub keys: Vec<&'static str>,
    /// The key context its chords work in, `None` for the whole window.
    pub context: Option<&'static str>,
    /// Whether the command palette offers it. Keys that only move around a
    /// list stay out of it.
    pub in_palette: bool,
    bind: Bind,
}

impl Command {
    fn new<A: Action + Clone>(group: Group, name: &'static str, action: A) -> Self {
        let bound = action.clone();
        Self {
            name,
            group,
            action: Box::new(action),
            keys: Vec::new(),
            context: None,
            in_palette: true,
            bind: Rc::new(move |key, context| KeyBinding::new(key, bound.clone(), context)),
        }
    }

    fn keys(mut self, keys: &[&'static str]) -> Self {
        self.keys.extend_from_slice(keys);
        self
    }

    fn context(mut self, context: &'static str) -> Self {
        self.context = Some(context);
        self
    }

    /// Leaves it out of the command palette.
    fn not_in_palette(mut self) -> Self {
        self.in_palette = false;
        self
    }

    /// Its chords, ready for the keymap.
    pub fn key_bindings(&self) -> Vec<KeyBinding> {
        self.keys
            .iter()
            .map(|key| (self.bind)(key, self.context))
            .collect()
    }

    /// Its chords as keystrokes, for keycaps.
    pub fn keystrokes(&self) -> Vec<Keystroke> {
        self.keys
            .iter()
            .filter_map(|key| Keystroke::parse(key).ok())
            .collect()
    }
}

/// The app's commands on this platform, in the order the sheet lists them.
pub fn commands() -> Vec<Command> {
    use Group::*;
    let mac = cfg!(target_os = "macos");
    let quit = if cfg!(target_os = "windows") {
        "Exit"
    } else {
        "Quit"
    };
    let mut commands = vec![
        Command::new(General, "Command Palette…", CommandPalette)
            .keys(&["secondary-k", "secondary-shift-p"])
            .not_in_palette(),
        Command::new(General, "Keyboard Shortcuts", KeyboardShortcuts).keys(&["secondary-/"]),
        Command::new(General, "Settings…", OpenSettings).keys(&["secondary-,"]),
        Command::new(General, "Models & Accounts…", OpenAccounts),
        Command::new(General, "Close Settings", CloseSettings)
            .keys(&["escape"])
            .context(crate::settings_page::CONTEXT)
            .not_in_palette(),
        Command::new(General, "New Window", NewWindow).keys(&["secondary-shift-n"]),
        Command::new(General, "Find…", Find).keys(&["secondary-f"]),
        // Also where Ctrl+F belongs to the composer's Emacs or Vim keymap:
        // F3 opens Find when it isn't open.
        Command::new(General, "Find Next", FindNext).keys(if mac {
            &["cmd-g", "f3"]
        } else {
            &["f3"]
        }),
        Command::new(General, "Find Previous", FindPrevious).keys(if mac {
            &["cmd-shift-g", "shift-f3"]
        } else {
            &["shift-f3"]
        }),
        Command::new(General, "Close Window", CloseWindow).keys(if mac { &["cmd-w"] } else { &[] }),
        Command::new(General, "Minimize", Minimize).keys(if mac { &["cmd-m"] } else { &[] }),
        Command::new(General, if mac { "Zoom" } else { "Maximize" }, Zoom),
        Command::new(General, "Open Arko’s Folder", OpenDataFolder),
        Command::new(General, "Open Logs Folder", OpenLogsFolder),
        Command::new(General, "About Arko Studio", About),
        Command::new(General, quit, Quit).keys(&["secondary-q"]),
        Command::new(Sessions, "New Session", NewSession).keys(&["secondary-n"]),
        Command::new(Sessions, "Go to Session…", FocusSessionFilter).keys(&["secondary-p"]),
        Command::new(Sessions, "Session Info…", SessionInfo),
        Command::new(Sessions, "Rename Session…", RenameSession),
        Command::new(Sessions, "Rename Session…", RenameSession)
            .keys(&["f2"])
            .context(LIST_CONTEXT)
            .not_in_palette(),
        Command::new(Sessions, "Branch from Here", BranchSession),
        Command::new(Sessions, "Hand Over Now", HandOverNow),
        Command::new(Sessions, "Refresh Cache", RefreshCache),
        Command::new(Sessions, "Share Web Page", ShareWebPage),
        Command::new(Sessions, "Stop Sharing Web Page", StopSharingWebPage),
        Command::new(Sessions, "Export as Markdown…", ExportMarkdown),
        Command::new(Sessions, "Close Session", CloseSession),
        Command::new(Sessions, "Delete Session…", DeleteSession),
        Command::new(Sessions, "Delete Session…", DeleteSession)
            .keys(&["delete", "backspace"])
            .context(LIST_CONTEXT)
            .not_in_palette(),
        Command::new(Sessions, "Previous in the List", SelectPrevious)
            .keys(&["up"])
            .context(LIST_CONTEXT)
            .not_in_palette(),
        Command::new(Sessions, "Next in the List", SelectNext)
            .keys(&["down"])
            .context(LIST_CONTEXT)
            .not_in_palette(),
        Command::new(Sessions, "First in the List", SelectFirst)
            .keys(&["home"])
            .context(LIST_CONTEXT)
            .not_in_palette(),
        Command::new(Sessions, "Last in the List", SelectLast)
            .keys(&["end"])
            .context(LIST_CONTEXT)
            .not_in_palette(),
        Command::new(Sessions, "Close Group or Conversations", Collapse)
            .keys(&["left"])
            .context(LIST_CONTEXT)
            .not_in_palette(),
        Command::new(Sessions, "Open Group or Conversations", Expand)
            .keys(&["right"])
            .context(LIST_CONTEXT)
            .not_in_palette(),
        Command::new(Sessions, "Open Session", OpenSelected)
            .keys(&["enter"])
            .context(LIST_CONTEXT)
            .not_in_palette(),
        Command::new(Sessions, "From the Filter to the List", FocusList)
            .keys(&["down"])
            .context(FILTER_CONTEXT)
            .not_in_palette(),
        Command::new(Sessions, "Clear the Filter", ClearFilter)
            .keys(&["escape"])
            .context(FILTER_CONTEXT)
            .not_in_palette(),
        Command::new(Sessions, "Cancel Renaming", CancelRename)
            .keys(&["escape"])
            .context(RENAME_CONTEXT)
            .not_in_palette(),
        Command::new(Composer, "Expand Composer", ExpandComposer).keys(&["secondary-shift-e"]),
        Command::new(Composer, "Stop", StopTurn).keys(&["secondary-."]),
        // Esc stops a turn from the composer too (it handles that key itself).
        Command::new(Composer, "Move Command to Background", MoveToBackground).keys(&["ctrl-b"]),
        Command::new(Transcript, "Full Transcript", ViewFull),
        Command::new(Transcript, "Quiet Transcript", ViewQuiet),
        Command::new(Transcript, "Messages Only", ViewMessages),
        Command::new(Transcript, "Next Transcript View", CycleView).keys(&["secondary-shift-o"]),
        Command::new(Transcript, "Copy", gpui_kit::component::input::Copy)
            .keys(&["secondary-c"])
            .context(crate::transcript::CONTEXT)
            .not_in_palette(),
        Command::new(
            Transcript,
            "Scroll Up a Page",
            crate::transcript::ScrollPageUp,
        )
        .keys(&["pageup"])
        .context(crate::transcript::CONTEXT)
        .not_in_palette(),
        Command::new(
            Transcript,
            "Scroll Down a Page",
            crate::transcript::ScrollPageDown,
        )
        .keys(&["pagedown"])
        .context(crate::transcript::CONTEXT)
        .not_in_palette(),
        Command::new(
            Transcript,
            "Scroll to the Start",
            crate::transcript::ScrollToStart,
        )
        .keys(&["home", "secondary-home"])
        .context(crate::transcript::CONTEXT)
        .not_in_palette(),
        Command::new(
            Transcript,
            "Jump to Latest",
            crate::transcript::ScrollToLatest,
        )
        .keys(&["end", "secondary-end"])
        .context(crate::transcript::CONTEXT)
        .not_in_palette(),
        Command::new(
            Transcript,
            "Focus Transcript",
            crate::transcript::FocusTranscript,
        )
        .keys(&["f6"]),
        Command::new(
            Transcript,
            "Previous Row",
            crate::transcript::SelectPrevious,
        )
        .keys(&["up"])
        .context(crate::transcript::CONTEXT)
        .not_in_palette(),
        Command::new(Transcript, "Next Row", crate::transcript::SelectNext)
            .keys(&["down"])
            .context(crate::transcript::CONTEXT)
            .not_in_palette(),
        Command::new(
            Transcript,
            "Open or Close Row",
            crate::transcript::ToggleRow,
        )
        .keys(&["enter"])
        .context(crate::transcript::CONTEXT)
        .not_in_palette(),
        Command::new(Transcript, "Back to the Composer", crate::transcript::Leave)
            .keys(&["escape"])
            .context(crate::transcript::CONTEXT)
            .not_in_palette(),
        // The workers panel's list, and a worker's view in it.
        Command::new(Workers, "Previous Worker", workers_panel::SelectPrevious)
            .keys(&["up"])
            .context(workers_panel::LIST_CONTEXT)
            .not_in_palette(),
        Command::new(Workers, "Next Worker", workers_panel::SelectNext)
            .keys(&["down"])
            .context(workers_panel::LIST_CONTEXT)
            .not_in_palette(),
        Command::new(Workers, "First Worker", workers_panel::SelectFirst)
            .keys(&["home"])
            .context(workers_panel::LIST_CONTEXT)
            .not_in_palette(),
        Command::new(Workers, "Last Worker", workers_panel::SelectLast)
            .keys(&["end"])
            .context(workers_panel::LIST_CONTEXT)
            .not_in_palette(),
        Command::new(Workers, "Next Worker, Then On", workers_panel::TabNext)
            .keys(&["tab"])
            .context(workers_panel::LIST_CONTEXT)
            .not_in_palette(),
        Command::new(
            Workers,
            "Previous Worker, Then Back",
            workers_panel::TabPrevious,
        )
        .keys(&["shift-tab"])
        .context(workers_panel::LIST_CONTEXT)
        .not_in_palette(),
        Command::new(Workers, "Open Worker", workers_panel::OpenSelected)
            .keys(&["enter"])
            .context(workers_panel::LIST_CONTEXT)
            .not_in_palette(),
        Command::new(Workers, "Back to the Composer", workers_panel::Back)
            .keys(&["escape"])
            .context(workers_panel::LIST_CONTEXT)
            .not_in_palette(),
        Command::new(Workers, "All Workers", workers_panel::Back)
            .keys(&["escape"])
            .context(workers_panel::WORKER_CONTEXT)
            .not_in_palette(),
        Command::new(View, "Toggle Sidebar", ToggleSidebar).keys(&["secondary-\\"]),
        Command::new(View, "Toggle Workers Panel", ToggleWorkersPanel)
            .keys(&["secondary-shift-\\"]),
        Command::new(View, "Zoom In", ZoomIn).keys(&["secondary-=", "secondary-+"]),
        Command::new(View, "Zoom Out", ZoomOut).keys(&["secondary--"]),
        Command::new(View, "Actual Size", ResetZoom).keys(&["secondary-0"]),
    ];
    if mac {
        commands.extend([
            Command::new(General, "Hide Arko Studio", Hide).keys(&["cmd-h"]),
            Command::new(General, "Hide Others", HideOthers).keys(&["alt-cmd-h"]),
            Command::new(General, "Show All", ShowAll),
        ]);
    }
    commands
}

/// The app's key bindings: every chord in the table.
pub fn key_bindings() -> Vec<KeyBinding> {
    commands().iter().flat_map(Command::key_bindings).collect()
}

#[cfg(test)]
mod tests {
    // Named, not `super::*`: GPUI's glob brings a `test` macro that would
    // shadow Rust's.
    use super::{Group, commands};
    use crate::menus::{MenuStateKey, menus};
    use gpui_kit::MenuItem;

    #[test]
    fn every_menu_action_is_a_command() {
        let names: Vec<&str> = commands()
            .iter()
            .map(|command| command.action.name())
            .collect();
        for menu in menus(MenuStateKey::default()) {
            for item in &menu.items {
                if let MenuItem::Action {
                    name,
                    action,
                    os_action: None,
                    ..
                } = item
                {
                    assert!(
                        names.contains(&action.name()),
                        "{name} ({}) isn't in the command table",
                        action.name()
                    );
                }
            }
        }
    }

    #[test]
    fn every_chord_parses_and_every_group_has_commands() {
        for command in commands() {
            assert_eq!(
                command.keystrokes().len(),
                command.keys.len(),
                "{}: {:?}",
                command.name,
                command.keys
            );
        }
        for group in Group::ALL {
            assert!(commands().iter().any(|command| command.group == group));
        }
    }
}
