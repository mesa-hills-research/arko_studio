//! Settings typed as text: a short field checked as it's typed, saved when
//! it reads, else saying why under it while the setting stays as it was.

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::setting::{SettingField, SettingItem};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::prefs::{self, Prefs};

/// How a typed setting reads and writes its preference.
pub struct TextKind<T> {
    pub get: fn(&Prefs) -> Option<T>,
    pub set: fn(&mut Prefs, Option<T>),
    /// The text as the setting: `None` for an empty field (the default),
    /// else the value, or why it isn't one.
    pub parse: fn(&str) -> Result<Option<T>, String>,
    pub show: fn(&T) -> String,
}

/// A setting typed as text.
pub struct TextSetting<T> {
    input: Entity<InputState>,
    error: Option<SharedString>,
    /// The value the field last showed or saved: a change to the setting
    /// from elsewhere (a reset) shows in the field.
    seen: Option<T>,
    kind: TextKind<T>,
}

impl<T: Clone + PartialEq + 'static> TextSetting<T> {
    /// A field showing the preference as it is, with `placeholder` for the
    /// default. Its owner calls [`Self::changed`] on its input's changes
    /// ([`Self::input`]) and [`Self::follow`] on the preferences'.
    pub fn new(kind: TextKind<T>, placeholder: &str, window: &mut Window, cx: &mut App) -> Self {
        let value = (kind.get)(prefs::get(cx));
        let text = value.as_ref().map(kind.show).unwrap_or_default();
        let placeholder = placeholder.to_string();
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(placeholder)
                .default_value(text)
        });
        Self {
            input,
            error: None,
            seen: value,
            kind,
        }
    }

    pub fn input(&self) -> &Entity<InputState> {
        &self.input
    }

    /// Why what's typed isn't saved, if it isn't.
    pub fn error(&self) -> Option<SharedString> {
        self.error.clone()
    }

    pub fn typed(&self, cx: &App) -> SharedString {
        self.input.read(cx).value()
    }

    /// Reads what's typed: saved when it reads, else kept with why not.
    pub fn changed(&mut self, cx: &mut App) {
        let text = self.input.read(cx).value();
        match (self.kind.parse)(&text) {
            Ok(value) => {
                self.error = None;
                self.seen = value.clone();
                let set = self.kind.set;
                prefs::update(cx, |prefs| set(prefs, value));
            }
            Err(why) => self.error = Some(why.into()),
        }
    }

    /// Shows the setting as it is now, when it changed elsewhere.
    pub fn follow(&mut self, window: &mut Window, cx: &mut App) {
        let value = (self.kind.get)(prefs::get(cx));
        if value != self.seen {
            let text = value.as_ref().map(self.kind.show).unwrap_or_default();
            self.seen = value;
            self.error = None;
            self.input
                .update(cx, |input, cx| input.set_value(text, window, cx));
        }
    }

    /// The setting's row: `title`, the field (found in tests by `id`) and
    /// why what's typed isn't saved. Its reset goes back to the default.
    pub fn item(&self, title: &'static str, id: &'static str) -> SettingItem {
        let input = self.input.clone();
        let error = self.error.clone();
        let (get, set) = (self.kind.get, self.kind.set);
        SettingItem::new(
            title,
            SettingField::render(move |options, _, cx| {
                let danger = cx.theme().danger;
                // Beside its title, or under it where the page is narrow.
                let beside = options.layout() == Axis::Horizontal;
                v_flex()
                    .id(id)
                    .test_support()
                    .when(beside, |this| this.items_end())
                    .gap_1()
                    .child(Input::new(&input).small().w(rems(9.)))
                    .when_some(error.clone(), |this, error| {
                        this.child(
                            div()
                                .id(SharedString::from(format!("{id}-error")))
                                .test_support()
                                .max_w(rems(16.))
                                .text_xs()
                                .text_color(danger)
                                .child(error),
                        )
                    })
            })
            .on_reset(
                move |cx| get(prefs::get(cx)).is_some(),
                move |_, cx| prefs::update(cx, |prefs| set(prefs, None)),
            ),
        )
    }
}

/// Whether `event` is a change of what's typed.
pub fn is_change(event: &InputEvent) -> bool {
    matches!(event, InputEvent::Change)
}
