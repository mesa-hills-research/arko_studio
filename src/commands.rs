//! Every command of the app in one table: its name, its group, its action and
//! its key chords. The key bindings, the command palette and the keyboard
//! shortcuts sheet are all made from it, so they can't disagree. The menus
//! dispatch the same actions (a test checks that each one is in the table).

use std::rc::Rc;

use gpui_kit::*;

use crate::composer::EditLastMessage;
use crate::menus::*;
use crate::settings_page::CloseSettings;
use crate::sidebar::{
    CancelRename, ClearFilter, Collapse, Expand, FocusList, OpenSelected, SelectFirst, SelectLast,
    SelectNext, SelectPrevious,
};
use crate::sidebar::{FILTER_CONTEXT, LIST_CONTEXT, RENAME_CONTEXT};
use crate::{notifications_panel, workers_panel};

/// The sections of the shortcuts sheet and the palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    General,
    Sessions,
    Composer,
    Transcript,
    Workers,
    Notifications,
    View,
}

impl Group {
    pub const ALL: [Group; 7] = [
        Group::General,
        Group::Sessions,
        Group::Composer,
        Group::Transcript,
        Group::Workers,
        Group::Notifications,
        Group::View,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Group::General => "General",
            Group::Sessions => "Sessions",
            Group::Composer => "Composer",
            Group::Transcript => "Transcript",
            Group::Workers => "Workers",
            Group::Notifications => "Notifications",
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

    /// Its chords, ready for the keymap. A chord that holds Shift on a key
    /// with a symbol on top is bound as the platforms send it too (see
    /// [`delivered_form`]), between two bindings of the chord as it reads.
    /// Menus and keycaps show one binding of an action. The kit takes the last
    /// and macOS's menu bar takes the first, so both show Ctrl+Shift+\.
    pub fn key_bindings(&self) -> Vec<KeyBinding> {
        self.keys
            .iter()
            .flat_map(|key| {
                let written = (*key).to_string();
                match delivered_form(key) {
                    Some(delivered) => vec![written.clone(), delivered, written],
                    None => vec![written],
                }
            })
            .map(|key| (self.bind)(&key, self.context))
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

/// The keys that carry two symbols on a US keyboard, with the one Shift makes.
const SHIFTED_SYMBOLS: [(&str, &str); 21] = [
    ("`", "~"),
    ("1", "!"),
    ("2", "@"),
    ("3", "#"),
    ("4", "$"),
    ("5", "%"),
    ("6", "^"),
    ("7", "&"),
    ("8", "*"),
    ("9", "("),
    ("0", ")"),
    ("-", "_"),
    ("=", "+"),
    ("[", "{"),
    ("]", "}"),
    ("\\", "|"),
    (";", ":"),
    ("'", "\""),
    (",", "<"),
    (".", ">"),
    ("/", "?"),
];

/// What the platforms send when `chord` holds Shift on one of the keys in
/// [`SHIFTED_SYMBOLS`]. They report the symbol alone, without the Shift
/// modifier, so Ctrl+Shift+\ arrives as `ctrl-|` and a binding written
/// `ctrl-shift-\` never matches it. `None` for a chord of any other kind.
pub fn delivered_form(chord: &str) -> Option<String> {
    let mut stroke = Keystroke::parse(chord).ok()?;
    if !stroke.modifiers.shift {
        return None;
    }
    let (_, symbol) = SHIFTED_SYMBOLS.iter().find(|(key, _)| *key == stroke.key)?;
    stroke.modifiers.shift = false;
    stroke.key = (*symbol).to_string();
    Some(stroke.unparse())
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
        // Free in every keymap on Linux and Windows. On macOS the default
        // keymap takes it to a line's start, which the empty composer
        // turns into this.
        Command::new(Composer, "Edit the Last Message", EditLastMessage)
            .keys(&["alt-up"])
            .context(crate::composer::CONTEXT)
            .not_in_palette(),
        Command::new(
            Composer,
            "Back to the Composer from a Question",
            crate::session_view::LeaveQuestion,
        )
        .keys(&["escape"])
        .context(crate::session_view::QUESTION_CONTEXT)
        .not_in_palette(),
        Command::new(Composer, "Expand Composer", ExpandComposer).keys(&["secondary-shift-e"]),
        Command::new(Composer, "Stop", StopTurn).keys(&["secondary-."]),
        // Esc stops a turn from the composer too (it handles that key itself).
        Command::new(Composer, "Move to Background", MoveToBackground).keys(&["ctrl-b"]),
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
        // The notifications panel's list, and what works on all of them.
        Command::new(Notifications, "Toggle Notifications", ToggleNotifications)
            .keys(&["secondary-shift-m"]),
        Command::new(
            Notifications,
            "Mark All Notifications Read",
            MarkNotificationsRead,
        ),
        Command::new(Notifications, "Clear Notifications", ClearNotifications),
        Command::new(
            Notifications,
            "Previous Notification",
            notifications_panel::SelectPrevious,
        )
        .keys(&["up"])
        .context(notifications_panel::LIST_CONTEXT)
        .not_in_palette(),
        Command::new(
            Notifications,
            "Next Notification",
            notifications_panel::SelectNext,
        )
        .keys(&["down"])
        .context(notifications_panel::LIST_CONTEXT)
        .not_in_palette(),
        Command::new(
            Notifications,
            "First Notification",
            notifications_panel::SelectFirst,
        )
        .keys(&["home"])
        .context(notifications_panel::LIST_CONTEXT)
        .not_in_palette(),
        Command::new(
            Notifications,
            "Last Notification",
            notifications_panel::SelectLast,
        )
        .keys(&["end"])
        .context(notifications_panel::LIST_CONTEXT)
        .not_in_palette(),
        Command::new(
            Notifications,
            "Open Notification",
            notifications_panel::OpenSelected,
        )
        .keys(&["enter"])
        .context(notifications_panel::LIST_CONTEXT)
        .not_in_palette(),
        Command::new(
            Notifications,
            "Back to the Composer",
            notifications_panel::Back,
        )
        .keys(&["escape"])
        .context(notifications_panel::LIST_CONTEXT)
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
    use super::{Group, SHIFTED_SYMBOLS, commands, delivered_form};
    use crate::menus::{MenuStateKey, menus};
    use gpui_kit::{Keystroke, MenuItem};

    #[test]
    fn a_chord_with_shift_on_a_symbol_key_is_delivered_as_the_symbol() {
        let secondary = if cfg!(target_os = "macos") {
            "cmd"
        } else {
            "ctrl"
        };
        let cases = [
            ("secondary-shift-\\", format!("{secondary}-|")),
            ("secondary-shift-=", format!("{secondary}-+")),
            ("secondary-shift--", format!("{secondary}-_")),
            ("secondary-shift-/", format!("{secondary}-?")),
            ("secondary-shift-1", format!("{secondary}-!")),
            ("alt-shift-[", "alt-{".to_string()),
            ("shift-,", "<".to_string()),
        ];
        for (chord, delivered) in cases {
            assert_eq!(delivered_form(chord), Some(delivered), "{chord}");
        }
        // Letters keep Shift, named keys keep it, and a chord without Shift
        // is sent as it reads.
        for chord in [
            "secondary-shift-p",
            "shift-tab",
            "shift-f3",
            "secondary-shift-enter",
            "secondary-\\",
            "secondary-+",
            "secondary-=",
            "secondary--",
        ] {
            assert_eq!(delivered_form(chord), None, "{chord}");
        }
        assert_eq!(delivered_form("shift"), None);
    }

    #[test]
    fn every_shifted_symbol_of_the_table_is_a_single_key_that_parses() {
        for (key, symbol) in SHIFTED_SYMBOLS {
            let stroke = Keystroke::parse(&format!("shift-{key}")).unwrap();
            assert_eq!(stroke.key, key);
            assert!(stroke.modifiers.shift);
            let delivered = delivered_form(&format!("ctrl-shift-{key}")).unwrap();
            let stroke = Keystroke::parse(&delivered).unwrap();
            assert_eq!(stroke.key, symbol);
            assert!(!stroke.modifiers.shift && stroke.modifiers.control);
        }
    }

    /// Every chord of the table that holds Shift on a key with a symbol on
    /// top is bound as the platforms send it too. The chord as it reads comes
    /// before and after, so it is the first and the last binding of its
    /// command, the two the menus and the palette show.
    #[test]
    fn a_shifted_symbol_chord_is_bound_as_delivered_and_as_it_reads() {
        let mut checked = 0;
        for command in commands() {
            let strokes: Vec<Keystroke> = command
                .key_bindings()
                .iter()
                .map(|binding| binding.keystrokes()[0].inner().clone())
                .collect();
            let mut expected = Vec::new();
            for chord in &command.keys {
                let written = Keystroke::parse(chord).unwrap();
                if let Some(delivered) = delivered_form(chord) {
                    expected.push(written.clone());
                    expected.push(Keystroke::parse(&delivered).unwrap());
                    checked += 1;
                }
                expected.push(written);
            }
            assert_eq!(strokes, expected, "{}", command.name);
        }
        assert!(checked > 0);
    }

    /// No two commands take the same chord in the same key context, so a
    /// chord does one thing wherever the keyboard is.
    #[test]
    fn no_chord_means_two_things_in_one_context() {
        let mut seen: Vec<(String, Option<&str>, &str)> = Vec::new();
        for command in commands() {
            for chord in &command.keys {
                let stroke = Keystroke::parse(chord).unwrap().unparse();
                if let Some((_, _, other)) = seen
                    .iter()
                    .find(|(seen, context, _)| *seen == stroke && *context == command.context)
                {
                    // The same command listed twice (a menu's and a
                    // list's, say) is one command.
                    assert_eq!(*other, command.name, "{chord} in {:?}", command.context);
                }
                seen.push((stroke, command.context, command.name));
            }
        }
    }

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
