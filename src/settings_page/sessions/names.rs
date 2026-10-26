//! Settings › Sessions › Names: whether a new session is named by the
//! first words of its first message or by a model, and for a model, the
//! ways to ask one, each on or off, in the order they are tried. It is
//! arko's setting in Arko's folder, which the terminal app reads too, so it
//! is read and written through the backend, off the UI thread.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::setting::{SettingField, SettingGroup, SettingItem};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, IconName, Sizable as _, h_flex};
use gpui_kit::*;

use super::SessionsPage;
use crate::backend::{SessionNames, SharedBackend, Way};
use crate::workers::curly;

/// The dropdown's two values.
const WORDS: &str = "words";
const MODEL: &str = "model";

/// What the group shows and the writes in flight.
pub(super) struct Names {
    /// What the backend said last, `None` until it did.
    read: Option<SessionNames>,
    /// The four ways as the list shows them, in order, each on or off.
    list: Vec<(Way, bool)>,
    /// Whether a model names sessions.
    by_model: bool,
    /// Why the last write failed.
    error: Option<SharedString>,
    /// A write is in flight, and a change came while it was.
    saving: bool,
    again: bool,
    /// How many times the setting was read: only the last reading counts.
    reads: usize,
}

impl Names {
    pub(super) fn new() -> Self {
        Self {
            read: None,
            list: arko::session_names::arranged(&arko::session_names::default_ways()),
            by_model: false,
            error: None,
            saving: false,
            again: false,
            reads: 0,
        }
    }
}

impl SessionsPage {
    /// Reads the setting and what each way lacks, in the background.
    pub(super) fn refresh_names(&mut self, cx: &mut Context<Self>) {
        self.names.reads += 1;
        let reading = self.names.reads;
        let backend = cx.global::<SharedBackend>().clone();
        let read = cx.background_spawn(async move { backend.session_names() });
        cx.spawn(async move |this, cx| {
            let read = read.await;
            this.update(cx, |this, cx| {
                if this.names.reads == reading {
                    this.adopt_names(read);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Shows what the backend read. The list stays as the user arranged it
    /// while a write is in flight.
    fn adopt_names(&mut self, read: SessionNames) {
        let names = &mut self.names;
        if !names.saving {
            names.by_model = !read.ways.is_empty();
            if names.by_model {
                names.list = arko::session_names::arranged(&read.ways);
            }
        }
        names.read = Some(read);
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

    /// Writes the setting through the backend. A change that comes during a
    /// write is written after it, so the last one stays.
    fn save_names(&mut self, cx: &mut Context<Self>) {
        cx.notify();
        if self.names.saving {
            self.names.again = true;
            return;
        }
        self.names.saving = true;
        self.names.error = None;
        let ways = self.names_ways();
        let backend = cx.global::<SharedBackend>().clone();
        let saving = cx.background_spawn(async move { backend.set_session_names(&ways) });
        cx.spawn(async move |this, cx| {
            let saved = saving.await;
            this.update(cx, |this, cx| {
                this.names.saving = false;
                match saved {
                    Ok(()) if std::mem::take(&mut this.names.again) => this.save_names(cx),
                    Ok(()) => {}
                    Err(why) => {
                        this.names.again = false;
                        this.names.error = Some(curly(&why).into());
                        // What the file holds is what shows.
                        this.refresh_names(cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
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
