//! The light or dark theme setting: a button naming the chosen theme, which
//! opens a searchable list. Resting on a theme in the list shows it (after
//! [`PREVIEW_REST`]), Enter or a click keeps it, and Esc or a click outside
//! goes back to the chosen one. The first time the list opens it says that
//! previews can flash, and offers to turn them off.

use gpui_kit::component::list::{List, ListDelegate, ListItem, ListState};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::{
    ActiveTheme as _, IconName, IndexPath, Sizable as _, StyledExt as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::appearance::{self, Catalog, PREVIEW_REST};
use crate::prefs;

/// What previews do, said before the first one.
pub const FLASH_NOTICE: &str = "Resting on a theme in the list shows it. Moving between light and dark themes makes the whole window flash. If flashing light affects you, choose No previews. Settings can change this later.";

pub struct ThemePicker {
    /// Picks the dark theme (else the light one).
    dark: bool,
    list: Entity<ListState<ThemeList>>,
    open: bool,
    _preview: Option<Task<()>>,
}

impl ThemePicker {
    pub fn new(dark: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let picker = cx.entity().downgrade();
        let list = cx.new(|cx| {
            ListState::new(
                ThemeList {
                    picker,
                    dark,
                    names: Vec::new(),
                    shown: Vec::new(),
                    selected: None,
                    quiet: false,
                    query: String::new(),
                },
                window,
                cx,
            )
            .searchable(true)
        });
        Self {
            dark,
            list,
            open: false,
            _preview: None,
        }
    }

    /// The theme this setting holds.
    fn chosen(&self, cx: &App) -> String {
        let appearance = &prefs::get(cx).appearance;
        if self.dark {
            appearance.dark_theme.clone()
        } else {
            appearance.light_theme.clone()
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Opens the list on the chosen theme, or closes it, going back to the
    /// chosen theme.
    pub fn set_open(&mut self, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        if open == self.open {
            return;
        }
        self.open = open;
        self._preview = None;
        if open {
            let chosen = self.chosen(cx);
            let names: Vec<SharedString> = cx
                .global::<Catalog>()
                .of_mode(self.dark)
                .map(|theme| theme.name.clone())
                .collect();
            let selected = names
                .iter()
                .position(|name| name.eq_ignore_ascii_case(&chosen));
            self.list.update(cx, |list, cx| {
                // Moving the selection here isn't the user resting on a theme.
                list.delegate_mut().quiet = true;
                let delegate = list.delegate_mut();
                delegate.shown = (0..names.len()).collect();
                delegate.names = names;
                delegate.selected = selected;
                if !list.delegate().query.is_empty() {
                    list.set_query("", window, cx);
                }
                let index = selected.map(IndexPath::new);
                list.set_selected_index(index, window, cx);
                if let Some(index) = index {
                    list.scroll_to_item(index, ScrollStrategy::Center, window, cx);
                }
                list.delegate_mut().quiet = false;
            });
        } else if appearance::previewing(cx).is_some() {
            appearance::preview(None, cx);
        }
        cx.notify();
    }

    /// The selection moved to `name`: show it once it rests there.
    fn rest_on(&mut self, name: SharedString, cx: &mut Context<Self>) {
        let appearance = &prefs::get(cx).appearance;
        if !appearance.theme_previews || !appearance.flash_notice {
            return;
        }
        self._preview = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(PREVIEW_REST).await;
            this.update(cx, |this, cx| {
                if this.open {
                    appearance::preview(Some(&name), cx);
                }
            })
            .ok();
        }));
    }

    /// Keeps `name` and closes the list.
    fn choose(&mut self, name: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let dark = self.dark;
        prefs::update(cx, |prefs| {
            if dark {
                prefs.appearance.dark_theme = name.to_string();
            } else {
                prefs.appearance.light_theme = name.to_string();
            }
        });
        self.set_open(false, window, cx);
    }

    fn answer_notice(&mut self, previews: bool, window: &mut Window, cx: &mut Context<Self>) {
        prefs::update(cx, |prefs| {
            prefs.appearance.flash_notice = true;
            prefs.appearance.theme_previews = previews;
        });
        self.list.update(cx, |list, cx| list.focus(window, cx));
        cx.notify();
    }

    fn render_notice(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("flash-notice")
            .test_support()
            .w(rems(20.))
            .gap_3()
            .child(div().text_sm().font_medium().child("Theme previews"))
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(FLASH_NOTICE),
            )
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("no-previews")
                            .outline()
                            .small()
                            .label("No previews")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.answer_notice(false, window, cx)
                            })),
                    )
                    .child(
                        Button::new("preview-themes")
                            .primary()
                            .small()
                            .label("Preview themes")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.answer_notice(true, window, cx)
                            })),
                    ),
            )
    }
}

impl Render for ThemePicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let notice = !prefs::get(cx).appearance.flash_notice;
        let picker = cx.entity().downgrade();
        let id = if self.dark {
            "dark-theme"
        } else {
            "light-theme"
        };
        Popover::new(id)
            .anchor(Anchor::TopRight)
            .open(self.open)
            .on_open_change(move |open, window, cx| {
                picker
                    .update(cx, |picker, cx| picker.set_open(*open, window, cx))
                    .ok();
            })
            .when(!notice, |this| {
                this.track_focus(&self.list.read(cx).focus_handle(cx))
            })
            .trigger(
                Button::new(SharedString::from(format!("{id}-button")))
                    .outline()
                    .small()
                    .label(self.chosen(cx))
                    .dropdown_caret(true),
            )
            .map(|this| {
                if notice {
                    this.child(self.render_notice(cx))
                } else {
                    this.child(
                        div()
                            .w(rems(18.))
                            .h(rems(20.))
                            .child(List::new(&self.list).small()),
                    )
                }
            })
    }
}

/// The themes of one mode, filtered by the search.
pub struct ThemeList {
    picker: WeakEntity<ThemePicker>,
    dark: bool,
    names: Vec<SharedString>,
    /// Indexes into `names` that match the search.
    shown: Vec<usize>,
    /// The selected theme, an index into `names`.
    selected: Option<usize>,
    /// The picker moves the selection itself: no preview.
    quiet: bool,
    /// The search, as typed.
    query: String,
}

impl ThemeList {
    fn name(&self, index: IndexPath) -> Option<SharedString> {
        self.shown
            .get(index.row)
            .and_then(|at| self.names.get(*at))
            .cloned()
    }
}

impl ListDelegate for ThemeList {
    type Item = ListItem;

    fn perform_search(
        &mut self,
        query: &str,
        _: &mut Window,
        _: &mut Context<ListState<Self>>,
    ) -> Task<()> {
        self.query = query.to_string();
        let query = query.to_lowercase();
        self.shown = self
            .names
            .iter()
            .enumerate()
            .filter(|(_, name)| name.to_lowercase().contains(&query))
            .map(|(index, _)| index)
            .collect();
        Task::ready(())
    }

    fn items_count(&self, _: usize, _: &App) -> usize {
        self.shown.len()
    }

    fn render_item(
        &mut self,
        index: IndexPath,
        _: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<Self::Item> {
        let name = self.name(index)?;
        let appearance = &prefs::get(cx).appearance;
        let chosen = if self.dark {
            &appearance.dark_theme
        } else {
            &appearance.light_theme
        };
        Some(
            ListItem::new(index.row)
                .child(name.clone())
                .confirmed(name.eq_ignore_ascii_case(chosen))
                .check_icon(IconName::Check),
        )
    }

    fn set_selected_index(
        &mut self,
        index: Option<IndexPath>,
        _: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) {
        let row = index.and_then(|index| self.shown.get(index.row).copied());
        if row == self.selected {
            return;
        }
        self.selected = row;
        if self.quiet {
            return;
        }
        if let Some(name) = index.and_then(|index| self.name(index)) {
            // Deferred: the picker may be the one moving the selection.
            let picker = self.picker.clone();
            cx.defer(move |cx| {
                picker
                    .update(cx, |picker, cx| picker.rest_on(name, cx))
                    .ok();
            });
        }
    }

    fn confirm(&mut self, _: bool, window: &mut Window, cx: &mut Context<ListState<Self>>) {
        let Some(name) = self.selected.and_then(|at| self.names.get(at)).cloned() else {
            return;
        };
        let picker = self.picker.clone();
        window.defer(cx, move |window, cx| {
            picker
                .update(cx, |picker, cx| picker.choose(name, window, cx))
                .ok();
        });
    }

    fn cancel(&mut self, window: &mut Window, cx: &mut Context<ListState<Self>>) {
        let picker = self.picker.clone();
        window.defer(cx, move |window, cx| {
            picker
                .update(cx, |picker, cx| picker.set_open(false, window, cx))
                .ok();
        });
    }
}
