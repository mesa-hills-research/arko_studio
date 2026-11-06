//! Settings › Sessions › Names: whether a new session is named by the
//! first words of its first message or by a model, and for a model, the
//! ways to ask one, each on or off, in the order they are tried. It is
//! arko's setting in Arko's folder, which the terminal app reads too, so it
//! is read and written through the backend, off the UI thread. Its saves are
//! the app's, so the page shows the choice being saved, also when Settings
//! opens again while it is, and hears how the saves end.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::setting::{SettingField, SettingGroup, SettingItem};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, IconName, Sizable as _, h_flex};
use gpui_kit::*;

use super::SessionsPage;
use crate::backend::{
    Read, SessionNames, SharedBackend, Way, begin_read, observe_saves, save_state, write_names,
};
use crate::workers::curly;

/// The dropdown's two values.
const WORDS: &str = "words";
const MODEL: &str = "model";

/// What the group shows and the writes it waits for.
pub(super) struct Names {
    /// What the backend said last, `None` until it did.
    read: Option<SessionNames>,
    /// The four ways as the list shows them, in order, each on or off.
    list: Vec<(Way, bool)>,
    /// Whether a model names sessions.
    by_model: bool,
    /// Why the last save failed.
    error: Option<SharedString>,
    /// A save of the setting hasn't ended. The backend's queue
    /// (`write_names`) runs them, also after this page is gone.
    saving: bool,
    /// The reading of the setting under way. A newer one replaces it.
    reading: Option<Task<()>>,
}

impl Names {
    pub(super) fn new() -> Self {
        Self {
            read: None,
            list: arko::session_names::arranged(&arko::session_names::default_ways()),
            by_model: false,
            error: None,
            saving: false,
            reading: None,
        }
    }
}

impl SessionsPage {
    /// Follows the app's saves of the setting.
    pub(super) fn observe_names(cx: &mut Context<Self>) -> Subscription {
        observe_saves::<Vec<Way>, _>(cx, |page, cx| page.follow_names_saves(cx))
    }

    /// Takes the saves as they are when the page opens. A choice being saved
    /// shows, and so does why the last save failed.
    pub(super) fn open_names(&mut self, cx: &mut Context<Self>) {
        let state = save_state::<Vec<Way>>(cx);
        self.names.error = state.error.as_deref().map(|why| curly(why).into());
        if let Some(ways) = state.pending {
            self.names.saving = true;
            self.show_ways(&ways);
        }
        self.refresh_names(cx);
    }

    /// Follows the app's saves of the setting, whichever page asked. A
    /// choice being saved shows. Once the saves end, what the file holds
    /// shows, with why the last save failed if it did.
    fn follow_names_saves(&mut self, cx: &mut Context<Self>) {
        let state = save_state::<Vec<Way>>(cx);
        if let Some(ways) = state.pending {
            self.names.saving = true;
            self.names.error = None;
            // This page's choice, or another page's.
            self.show_ways(&ways);
            cx.notify();
            return;
        }
        if !std::mem::take(&mut self.names.saving) {
            return;
        }
        self.names.error = state.error.as_deref().map(|why| curly(why).into());
        self.refresh_names(cx);
        cx.notify();
    }

    /// Shows `ways` turned on, in their order. A list that turns on the same
    /// ways keeps its order.
    fn show_ways(&mut self, ways: &[Way]) {
        if self.names_ways() == ways {
            return;
        }
        let names = &mut self.names;
        names.by_model = !ways.is_empty();
        if names.by_model {
            names.list = arko::session_names::arranged(ways);
        }
    }

    /// Reads the setting and what each way lacks, in the background.
    pub(super) fn refresh_names(&mut self, cx: &mut Context<Self>) {
        let began = begin_read::<Vec<Way>>(cx);
        let backend = cx.global::<SharedBackend>().clone();
        let read = cx.background_spawn(async move { backend.session_names() });
        self.names.reading = Some(cx.spawn(async move |this, cx| {
            let read = read.await;
            this.update(cx, |this, cx| {
                this.adopt_names(read, began, cx);
                cx.notify();
            })
            .ok();
        }));
    }

    /// Shows what the backend read in the reading `began`. What each way
    /// lacks always shows. The ways are taken only if no save of them was
    /// asked for or ended since, here or in another window, and none waits,
    /// since the reading may be of the list before the change, and showing
    /// it would undo the change on screen.
    fn adopt_names(&mut self, read: SessionNames, began: Read<Vec<Way>>, cx: &App) {
        if began.is_current(cx) {
            self.show_ways(&read.ways);
        }
        self.names.read = Some(read);
    }

    /// What the reading `began` read, arriving now (tests).
    #[cfg(test)]
    pub fn adopt_names_read(
        &mut self,
        read: SessionNames,
        began: Read<Vec<Way>>,
        cx: &mut Context<Self>,
    ) {
        self.adopt_names(read, began, cx);
        cx.notify();
    }

    /// The ways turned on, in order, as the setting holds them.
    fn names_ways(&self) -> Vec<Way> {
        let names = &self.names;
        if !names.by_model {
            return Vec::new();
        }
        names
            .list
            .iter()
            .filter(|(_, on)| *on)
            .map(|(way, _)| *way)
            .collect()
    }

    /// Chooses the first words or a model. A model starts with the two
    /// ways that use a plan on, unless some are on already.
    pub fn set_by_model(&mut self, by_model: bool, cx: &mut Context<Self>) {
        let names = &mut self.names;
        if names.by_model == by_model {
            return;
        }
        names.by_model = by_model;
        if by_model && names.list.iter().all(|(_, on)| !on) {
            names.list = arko::session_names::arranged(&arko::session_names::default_ways());
        }
        self.save_names(cx);
    }

    /// Turns the way at `at` in the list on or off.
    pub fn set_way(&mut self, at: usize, on: bool, cx: &mut Context<Self>) {
        if let Some(row) = self.names.list.get_mut(at) {
            row.1 = on;
            self.save_names(cx);
        }
    }

    /// Moves the way at `at` up or down the list, past its neighbour.
    pub fn move_way(&mut self, at: usize, up: bool, cx: &mut Context<Self>) {
        let to = if up { at.checked_sub(1) } else { Some(at + 1) };
        if let Some(to) = to.filter(|to| *to < self.names.list.len() && at < self.names.list.len())
        {
            self.names.list.swap(at, to);
            self.save_names(cx);
        }
    }

    /// Asks the backend's queue to write the setting. It writes the latest
    /// list asked for, also when this page is gone by then (Settings was
    /// closed), and every page showing it hears how the saves end.
    fn save_names(&mut self, cx: &mut Context<Self>) {
        let names = &mut self.names;
        names.saving = true;
        names.error = None;
        write_names(self.names_ways(), cx);
        cx.notify();
    }

    /// The setting's group.
    pub(super) fn names_group(&self, this: &Entity<Self>) -> SettingGroup {
        let names = &self.names;
        let shown = (
            names.by_model,
            &names.list,
            names.read.as_ref().map(|read| &read.missing),
            names.error.as_deref(),
        );
        SettingGroup::new()
            .title("Names")
            .key(super::super::group_key("Names", &shown))
            .items(self.names_items(this))
    }

    /// The group's rows. The first chooses the first words or a model. With
    /// a model, the ways follow, then a line that says the first words come
    /// last, and a failed write is said at the end.
    pub fn names_items(&self, this: &Entity<Self>) -> Vec<SettingItem> {
        let names = &self.names;
        let mut items = vec![self.names_choice(this)];
        if names.by_model {
            let missing = |way| {
                names
                    .read
                    .as_ref()
                    .and_then(|read| read.lacks(way))
                    .map(curly)
            };
            for (at, (way, on)) in names.list.iter().enumerate() {
                items.push(way_item(
                    this,
                    at,
                    names.list.len(),
                    *way,
                    *on,
                    missing(*way),
                ));
            }
            items.push(SettingItem::render(|_, _, cx| {
                div()
                    .id("names-fallback")
                    .test_support()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("Then the first words.")
            }));
        }
        if let Some(error) = names.error.clone() {
            items.push(SettingItem::render(move |_, _, cx| {
                div()
                    .id("names-error")
                    .test_support()
                    .text_sm()
                    .text_color(cx.theme().danger)
                    .child(error.clone())
            }));
        }
        items
    }

    /// How the setting shows (tests): whether a model names sessions, the
    /// ways in order with whether each is on, and why the last write
    /// failed.
    #[cfg(test)]
    pub fn naming(&self) -> (bool, Vec<(Way, bool)>, Option<String>) {
        let names = &self.names;
        (
            names.by_model,
            names.list.clone(),
            names.error.as_ref().map(|error| error.to_string()),
        )
    }

    /// The row that chooses the first words or a model.
    fn names_choice(&self, this: &Entity<Self>) -> SettingItem {
        let (shown, chosen) = (this.clone(), this.clone());
        SettingItem::new(
            "Session names",
            SettingField::dropdown(
                vec![
                    (WORDS.into(), "First words".into()),
                    (MODEL.into(), "Named by a model".into()),
                ],
                move |cx: &App| {
                    if shown.read(cx).names.by_model {
                        MODEL.into()
                    } else {
                        WORDS.into()
                    }
                },
                move |value: SharedString, cx: &mut App| {
                    chosen.update(cx, |page, cx| page.set_by_model(value == MODEL, cx));
                },
            )
            .default_value(WORDS),
        )
        .description(
            "With a model, a session’s first message goes to the provider that names it. Until it answers, and when no way works, the first words are the name.",
        )
        .keywords(["name", "title", "model", "haiku", "luna", "rename", "first message"])
    }
}

/// The row of the way at `at` of `count`, with its checkbox and buttons
/// that move it, and what it lacks now under it.
fn way_item(
    this: &Entity<SessionsPage>,
    at: usize,
    count: usize,
    way: Way,
    on: bool,
    lacks: Option<String>,
) -> SettingItem {
    let page = this.clone();
    let label = way.label();
    let item = SettingItem::new(
        label,
        SettingField::render(move |_, _, _| {
            let (toggle, up, down) = (page.clone(), page.clone(), page.clone());
            h_flex()
                .gap_1()
                .child(
                    Checkbox::new(("name-way", at))
                        .checked(on)
                        .accessibility_label(format!("Name sessions with {label}"))
                        .on_click(move |on, _, cx| {
                            let on = *on;
                            toggle.update(cx, |page, cx| page.set_way(at, on, cx));
                        }),
                )
                .child(
                    Button::new(("name-up", at))
                        .ghost()
                        .small()
                        .icon(IconName::ArrowUp)
                        .disabled(at == 0)
                        .accessibility_label(format!("Try {label} earlier"))
                        .on_click(move |_, _, cx| {
                            up.update(cx, |page, cx| page.move_way(at, true, cx));
                        }),
                )
                .child(
                    Button::new(("name-down", at))
                        .ghost()
                        .small()
                        .icon(IconName::ArrowDown)
                        .disabled(at + 1 == count)
                        .accessibility_label(format!("Try {label} later"))
                        .on_click(move |_, _, cx| {
                            down.update(cx, |page, cx| page.move_way(at, false, cx));
                        }),
                )
        }),
    )
    .keywords([
        "name",
        "title",
        "model",
        "claude",
        "chatgpt",
        "anthropic",
        "openai",
    ]);
    match lacks {
        Some(lacks) => item.description(lacks),
        None => item,
    }
}
