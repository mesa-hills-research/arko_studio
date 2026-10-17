//! Settings typed as text: a short field checked and saved once the typing
//! rests, or on Enter or when it loses focus. Saved when it reads, else
//! saying why under it while the setting stays as it was.

use std::time::Duration;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::setting::{SettingField, SettingItem};
use gpui_kit::component::{ActiveTheme as _, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::prefs::{self, Prefs};

/// How long a field waits after the last keystroke before it checks and
/// saves what's typed: "0." on the way to 0.0.0.0 isn't an error yet, and
/// 9, 90 and 900 aren't saved on the way to 9000.
pub const TYPING_REST: Duration = Duration::from_millis(600);

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
    /// Checks what's typed once the typing rests.
    pending: Option<Task<()>>,
}

impl<T: Clone + PartialEq + 'static> TextSetting<T> {
    /// A field showing the preference as it is, with `placeholder` for the
    /// default. Its owner hears its input with [`subscribe`] and calls
    /// [`Self::follow`] on the preferences' changes.
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
            pending: None,
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

    /// What's typed changed: it's checked once the typing rests
    /// ([`TYPING_REST`]). An error shown goes as soon as it reads. `field`
    /// finds this setting in its page `P`.
    fn changed<P: 'static>(&mut self, field: fn(&mut P) -> &mut Self, cx: &mut Context<P>) {
        let text = self.input.read(cx).value();
        if self.error.is_some() && (self.kind.parse)(&text).is_ok() {
            self.error = None;
            cx.notify();
        }
        self.pending = Some(cx.spawn(async move |page, cx| {
            cx.background_executor().timer(TYPING_REST).await;
            page.update(cx, |page, cx| {
                if field(page).check(cx) {
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    /// Checks what's typed now, the typing over (Enter, the field left,
    /// Settings closed): saved when it reads, else kept with why not.
    /// Whether the reason shown changed.
    pub fn commit(&mut self, cx: &mut App) -> bool {
        if self.pending.take().is_none() {
            return false;
        }
        self.check(cx)
    }

    fn check(&mut self, cx: &mut App) -> bool {
        let text = self.input.read(cx).value();
        let before = self.error.clone();
        match (self.kind.parse)(&text) {
            Ok(value) => {
                self.error = None;
                self.seen = value.clone();
                let set = self.kind.set;
                prefs::update(cx, |prefs| set(prefs, value));
            }
            Err(why) => self.error = Some(why.into()),
        }
        self.error != before
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
            self.pending = None;
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
                    // The size of the other controls in Settings.
                    .child(Input::new(&input).w(rems(9.)))
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

/// Has page `P` hear what's typed in its setting `field`: checked and
/// saved once the typing rests, at once on Enter or when the field loses
/// focus. The page draws again only when the reason shown under the field
/// changes, not on each keystroke.
pub fn subscribe<P: 'static, T: Clone + PartialEq + 'static>(
    field: fn(&mut P) -> &mut TextSetting<T>,
    input: &Entity<InputState>,
    cx: &mut Context<P>,
) -> Subscription {
    cx.subscribe(input, move |page, _, event: &InputEvent, cx| match event {
        InputEvent::Change => field(page).changed(field, cx),
        InputEvent::PressEnter { .. } | InputEvent::Blur => {
            if field(page).commit(cx) {
                cx.notify();
            }
        }
        InputEvent::Focus => {}
    })
}
