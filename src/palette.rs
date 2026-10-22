//! The command palette (Mod+K, also Mod+Shift+P): every command of the app,
//! the Settings pages and the sessions, found by typing, in a dialog.
//!
//! Its rows carry no action of their own: the dialog sits outside the
//! window's content, where the window's commands aren't handled. Choosing a
//! row closes the dialog and runs the row in the window, from where the
//! keyboard was before.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::component::command::{Command, CommandGroup, CommandItem, CommandState};
use gpui_kit::component::{ActiveTheme as _, IndexPath, WindowExt as _, h_flex, kbd::Kbd};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::backend::{SessionEntry, SessionId};
use crate::commands::{Group, commands};
use crate::settings_page;

/// What a row of the palette does.
pub enum Target {
    /// Runs a command.
    Action(Box<dyn Action>),
    /// Opens Settings on a page, by its place in the list of pages.
    SettingsPage(usize),
    /// Opens a session.
    Session(SessionId),
    /// One of Arko's commands (`/cost`), as if typed in the composer: it
    /// runs at once, or waits there for its arguments.
    Slash(&'static str),
}

/// A row: its label, more words it's found by, and what it does.
pub struct Row {
    pub label: SharedString,
    pub keywords: Vec<SharedString>,
    pub target: Target,
}

/// A titled group of rows.
pub struct Section {
    pub title: &'static str,
    pub rows: Vec<Row>,
}

/// The palette's sections: the commands by group, Arko's own commands,
/// Settings' pages and the sessions.
pub fn sections(sessions: &[SessionEntry]) -> Vec<Section> {
    let commands = commands();
    let mut sections: Vec<Section> = Group::ALL
        .iter()
        .map(|&group| {
            let mut rows: Vec<Row> = Vec::new();
            for command in commands
                .iter()
                .filter(|command| command.group == group && command.in_palette)
            {
                if rows.iter().all(|row| row.label != command.name) {
                    rows.push(Row {
                        label: command.name.into(),
                        keywords: Vec::new(),
                        target: Target::Action(command.action.boxed_clone()),
                    });
                }
            }
            Section {
                title: group.title(),
                rows,
            }
        })
        .collect();
    sections.push(arko_commands());
    sections.push(Section {
        title: "Settings",
        rows: settings_page::PAGES
            .iter()
            .enumerate()
            .map(|(page, title)| Row {
                label: SharedString::from(format!("Settings › {title}")),
                keywords: vec!["preferences".into()],
                target: Target::SettingsPage(page),
            })
            .collect(),
    });
    let mut sessions: Vec<&SessionEntry> = sessions.iter().collect();
    sessions.sort_by_key(|session| std::cmp::Reverse(session.activity()));
    sections.push(Section {
        title: "Go to Session",
        rows: sessions
            .into_iter()
            .map(|session| Row {
                label: session.title.clone().into(),
                keywords: session
                    .workspace
                    .iter()
                    .filter_map(|folder| folder.file_name())
                    .map(|name| SharedString::from(name.to_string_lossy().into_owned()))
                    .chain(session.conversations.iter().map(|c| c.title.clone().into()))
                    .collect(),
                target: Target::Session(session.id.clone()),
            })
            .collect(),
    });
    sections.retain(|section| !section.rows.is_empty());
    sections
}

/// Arko's commands, as the composer's slash menu lists them: each found by
/// its name and what it does.
fn arko_commands() -> Section {
    Section {
        title: "Arko Commands",
        rows: arko::commands::COMMANDS
            .iter()
            .filter(|command| command.runs == arko::commands::Runs::Session)
            .map(|command| Row {
                label: command.name.into(),
                keywords: vec![command.about.into()],
                target: Target::Slash(command.name),
            })
            .collect(),
    }
}

/// Opens the palette over `window`. `run` gets the chosen row's target once
/// the dialog has closed.
pub fn open(
    sections: Vec<Section>,
    run: impl Fn(Target, &mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    let state = cx.new(|cx| CommandState::new(window, cx));
    let sections = Rc::new(sections);
    let run = Rc::new(run);
    let focus_on_mount = Rc::new(Cell::new(true));
    window.open_dialog(cx, move |dialog, _, _| {
        let state = state.clone();
        let sections = sections.clone();
        let run = run.clone();
        let focus_on_mount = focus_on_mount.clone();
        dialog
            .close_button(false)
            .p_0()
            .w(px(560.))
            .content(move |content, window, cx| {
                if focus_on_mount.replace(false) {
                    let state = state.clone();
                    window.defer(cx, move |window, cx| {
                        state.read(cx).focus_handle(cx).focus(window, cx);
                    });
                }
                let chosen = sections.clone();
                let run = run.clone();
                let palette = sections.iter().fold(
                    Command::new(&state)
                        .bordered(false)
                        .placeholder("Search commands, settings and sessions")
                        .max_h(px(400.)),
                    |palette, section| palette.group(group(section)),
                );
                content.child(
                    div()
                        .id("command-palette")
                        .test_support()
                        .child(palette.on_confirm(move |index: IndexPath, window, cx| {
                            let Some(row) = chosen
                                .get(index.section)
                                .and_then(|section| section.rows.get(index.row))
                            else {
                                return;
                            };
                            let target = match &row.target {
                                Target::Action(action) => Target::Action(action.boxed_clone()),
                                Target::SettingsPage(page) => Target::SettingsPage(*page),
                                Target::Session(id) => Target::Session(id.clone()),
                                Target::Slash(name) => Target::Slash(name),
                            };
                            window.close_dialog(cx);
                            run(target, window, cx);
                        })),
                )
            })
    });
}

/// A section as a group of rows, each with its command's shortcut on a
/// keycap at the right.
fn group(section: &Section) -> CommandGroup {
    CommandGroup::new()
        .label(section.title)
        .items(section.rows.iter().map(|row| {
            let label = row.label.clone();
            let action = match &row.target {
                Target::Action(action) => Some(Rc::new(action.boxed_clone())),
                _ => None,
            };
            CommandItem::new()
                .label(row.label.clone())
                .keywords(row.keywords.clone())
                .child(move |window, cx| {
                    let muted = cx.theme().muted_foreground;
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_2()
                        .items_center()
                        .child(div().flex_1().min_w_0().truncate().child(label.clone()))
                        .when_some(
                            action.as_ref().and_then(|action| {
                                Kbd::global_binding_for_action(action.as_ref().as_ref(), window)
                            }),
                            |this, kbd| this.child(div().flex_none().text_color(muted).child(kbd)),
                        )
                })
        }))
}

#[cfg(test)]
mod tests {
    // Named, not `super::*`: GPUI's glob brings a `test` macro that would
    // shadow Rust's.
    use super::{Target, sections};
    use crate::backend::{Backend as _, FakeBackend};
    use std::time::SystemTime;

    #[test]
    fn the_palette_offers_commands_settings_and_sessions() {
        let sessions = FakeBackend::sample("/arko", SystemTime::now())
            .sessions()
            .unwrap();
        let sections = sections(&sessions);
        let titles: Vec<&str> = sections.iter().map(|section| section.title).collect();
        assert_eq!(
            titles,
            [
                "General",
                "Sessions",
                "Composer",
                "Transcript",
                "Notifications",
                "View",
                "Arko Commands",
                "Settings",
                "Go to Session"
            ]
        );
        let labels = |title: &str| -> Vec<String> {
            sections
                .iter()
                .find(|section| section.title == title)
                .unwrap()
                .rows
                .iter()
                .map(|row| row.label.to_string())
                .collect()
        };
        // The palette leaves itself out, and keys that move around a list.
        assert!(!labels("General").contains(&"Command Palette…".to_string()));
        assert!(
            !labels("Sessions")
                .iter()
                .any(|label| label.contains("List"))
        );
        // Each command once.
        let rename = labels("Sessions")
            .iter()
            .filter(|label| *label == "Rename Session…")
            .count();
        assert_eq!(rename, 1);
        assert_eq!(labels("Settings")[0], "Settings › Appearance");
        // Arko's commands, those of the full-screen terminal view left out.
        assert!(labels("Arko Commands").contains(&"/cost".to_string()));
        assert!(!labels("Arko Commands").contains(&"/watch".to_string()));
        // The most recent session first.
        assert_eq!(
            labels("Go to Session")[0],
            "Split the engine out of main.rs"
        );
        let last = sections.last().unwrap().rows.last().unwrap();
        assert!(matches!(last.target, Target::Session(_)));
    }
}
