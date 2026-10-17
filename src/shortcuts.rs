//! The keyboard shortcuts sheet (Mod+/, Help > Keyboard Shortcuts): every
//! key chord of the app by group, searchable, in a dialog. It's made from the
//! command table the key bindings come from, so it lists exactly what works.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, WindowExt as _, h_flex,
    kbd::Kbd, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::commands::{Group, commands};

/// A row of the sheet: a command and its chords.
pub struct Shortcut {
    pub group: Group,
    pub name: &'static str,
    /// The action's name, as GPUI knows it ("app::NewSession").
    pub action: &'static str,
    pub keys: Vec<Keystroke>,
}

impl Shortcut {
    /// Whether the row matches what the user typed: its name, its group or
    /// its keys as the keycaps show them.
    fn matches(&self, query: &str) -> bool {
        let query = query.trim().to_lowercase();
        query.is_empty()
            || self.name.to_lowercase().contains(&query)
            || self.group.title().to_lowercase().contains(&query)
            || self
                .keys
                .iter()
                .any(|key| Kbd::format(key).to_lowercase().contains(&query))
    }
}

/// Every command with a key chord, in the table's order.
pub fn shortcuts() -> Vec<Shortcut> {
    commands()
        .into_iter()
        .filter(|command| !command.keys.is_empty())
        .map(|command| Shortcut {
            group: command.group,
            name: command.name,
            action: command.action.name(),
            keys: command.keystrokes(),
        })
        .collect()
}

/// The sheet's content: a search field over the grouped rows.
pub struct ShortcutsView {
    filter: Entity<InputState>,
    shortcuts: Vec<Shortcut>,
    _filter: Subscription,
}

impl ShortcutsView {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Search shortcuts"));
        let subscription = cx.subscribe(&filter, |_, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        });
        Self {
            filter,
            shortcuts: shortcuts(),
            _filter: subscription,
        }
    }
}

/// Opens the sheet over `window`.
pub fn open(window: &mut Window, cx: &mut App) {
    let view = cx.new(|cx| ShortcutsView::new(window, cx));
    let focus_on_mount = Rc::new(Cell::new(true));
    window.open_dialog(cx, move |dialog, _, _| {
        let view = view.clone();
        let focus_on_mount = focus_on_mount.clone();
        dialog
            .title("Keyboard Shortcuts")
            .w(px(520.))
            .content(move |content, window, cx| {
                if focus_on_mount.replace(false) {
                    let filter = view.read(cx).filter.clone();
                    window.defer(cx, move |window, cx| {
                        filter.update(cx, |filter, cx| filter.focus(window, cx));
                    });
                }
                content.child(view.clone())
            })
    });
}

impl Render for ShortcutsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let query = self.filter.read(cx).value().to_string();
        let groups: Vec<(Group, Vec<(usize, &Shortcut)>)> = Group::ALL
            .iter()
            .map(|&group| {
                let rows = self
                    .shortcuts
                    .iter()
                    .enumerate()
                    .filter(|(_, shortcut)| shortcut.group == group && shortcut.matches(&query))
                    .collect();
                (group, rows)
            })
            .filter(|(_, rows): &(Group, Vec<_>)| !rows.is_empty())
            .collect();
        let empty = groups.is_empty();
        v_flex()
            .id("shortcuts")
            .test_support()
            .gap_3()
            .child(
                Input::new(&self.filter)
                    .small()
                    .cleanable(true)
                    .prefix(Icon::new(IconName::Search).text_color(theme.muted_foreground)),
            )
            .child(
                v_flex()
                    .id("shortcut-groups")
                    .max_h(rems(26.))
                    .overflow_y_scroll()
                    .gap_4()
                    .children(groups.into_iter().map(|(group, rows)| {
                        v_flex()
                            .gap_0p5()
                            .child(
                                div()
                                    .pb_1()
                                    .text_xs()
                                    .font_medium()
                                    .text_color(theme.muted_foreground)
                                    .child(group.title()),
                            )
                            // Ctrl+F is a keymap's own there.
                            .when(group == Group::General && !cfg!(target_os = "macos"), |this| {
                                this.child(
                                    div()
                                        .pb_1()
                                        .text_xs()
                                        .text_color(theme.muted_foreground)
                                        .child(
                                            "With the Emacs or Vim keymap, Ctrl+F moves the caret in the composer. F3 opens Find from anywhere.",
                                        ),
                                )
                            })
                            .children(rows.into_iter().map(|(index, shortcut)| {
                                h_flex()
                                    .id(("shortcut", index))
                                    .test_support()
                                    .min_h_7()
                                    .gap_3()
                                    .justify_between()
                                    .text_sm()
                                    .child(div().min_w_0().truncate().child(shortcut.name))
                                    .child(
                                        h_flex()
                                            .flex_none()
                                            .gap_1()
                                            .children(shortcut.keys.iter().cloned().map(Kbd::new)),
                                    )
                            }))
                    }))
                    .when(empty, |this| {
                        this.child(
                            div()
                                .py_4()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child(format!("No shortcut matches “{}”.", query.trim())),
                        )
                    }),
            )
    }
}

#[cfg(test)]
mod tests {
    // Named, not `super::*`: GPUI's glob brings a `test` macro that would
    // shadow Rust's.
    use super::shortcuts;

    #[test]
    fn rows_are_found_by_name_group_or_key() {
        let shortcuts = shortcuts();
        let found = |query: &str| -> Vec<&str> {
            shortcuts
                .iter()
                .filter(|shortcut| shortcut.matches(query))
                .map(|shortcut| shortcut.name)
                .collect()
        };
        assert_eq!(found("zoom"), ["Zoom In", "Zoom Out"]);
        assert!(found("composer").contains(&"Stop"));
        assert!(found("f2").contains(&"Rename Session…"));
        assert!(found("nothing like this").is_empty());
    }
}
