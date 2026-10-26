//! Settings › Models & accounts › OpenRouter: how OpenRouter routes every
//! request (zero data retention, data collection, region, the providers'
//! order, precision, and which providers), kept in `openrouter.json` in
//! Arko's folder for the terminal app too, and OpenRouter's list of
//! models. Everything is read and saved through the backend, on a
//! background thread.

use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::setting::{SettingField, SettingGroup, SettingItem};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex};
use gpui_kit::component::{switch::Switch, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use arko::openrouter::{Control, Precision, Region, Sort, slugs};

use super::fields::TYPING_REST;
use crate::backend::{Catalog, OpenRouterSettings, SharedBackend, sentence};
use crate::workers::curly;

/// What the group says before its rows.
const ABOUT: &str = "OpenRouter applies these to every request, along with your account’s privacy settings at openrouter.ai. Open sessions use a change from their next request.";

/// The group's state: the settings and the models as last read.
pub struct OpenRouterPart {
    settings: Option<OpenRouterSettings>,
    catalog: Option<Catalog>,
    /// The models are being fetched.
    fetching: bool,
    /// Why the settings couldn't be saved, when they couldn't.
    problem: Option<String>,
    only: Entity<InputState>,
    ignore: Entity<InputState>,
    _read: Option<Task<()>>,
    _save: Option<Task<()>>,
    _fetch: Option<Task<()>>,
    _typing: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

/// Which provider list a field holds.
#[derive(Clone, Copy, PartialEq)]
enum Providers {
    Only,
    Ignore,
}

impl OpenRouterPart {
    /// Reads the settings and the models arko keeps, and fetches the
    /// models when those are stale.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let field = |placeholder: &str, window: &mut Window, cx: &mut Context<Self>| {
            let placeholder = placeholder.to_string();
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
        };
        let only = field("Any", window, cx);
        let ignore = field("None", window, cx);
        let subscriptions = vec![
            cx.subscribe(&only, |this, _, event: &InputEvent, cx| {
                this.typed(Providers::Only, event, cx)
            }),
            cx.subscribe(&ignore, |this, _, event: &InputEvent, cx| {
                this.typed(Providers::Ignore, event, cx)
            }),
        ];
        let mut this = Self {
            settings: None,
            catalog: None,
            fetching: false,
            problem: None,
            only,
            ignore,
            _read: None,
            _save: None,
            _fetch: None,
            _typing: None,
            _subscriptions: subscriptions,
        };
        this.read(cx);
        this.fetch(false, cx);
        this
    }

    /// Reads the settings again, in the background (the terminal app may
    /// have changed them).
    pub fn read(&mut self, cx: &mut Context<Self>) {
        let backend = cx.global::<SharedBackend>().clone();
        let reading =
            cx.background_spawn(
                async move { (backend.openrouter(), backend.openrouter_models_kept()) },
            );
        self._read = Some(cx.spawn(async move |this, cx| {
            let (settings, catalog) = reading.await;
            this.update(cx, |this, cx| {
                this.settings = Some(settings);
                if this.catalog.is_none() || catalog.fetched.is_some() {
                    this.catalog = Some(catalog);
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Fetches OpenRouter's models in the background: when they're stale,
    /// or at once with `force`.
    pub fn fetch(&mut self, force: bool, cx: &mut Context<Self>) {
        self.fetching = true;
        cx.notify();
        let backend = cx.global::<SharedBackend>().clone();
        let fetching = cx.background_spawn(async move { backend.openrouter_models(force) });
        self._fetch = Some(cx.spawn(async move |this, cx| {
            let catalog = fetching.await;
            this.update(cx, |this, cx| {
                this.catalog = Some(catalog);
                this.fetching = false;
                cx.notify();
            })
            .ok();
        }));
    }

    pub fn settings(&self) -> Option<&OpenRouterSettings> {
        self.settings.as_ref()
    }

    /// Changes the settings with `change` and saves them, after the saves
    /// before it. A change to what rules models out fetches the models
    /// again.
    fn change(&mut self, change: impl FnOnce(&mut OpenRouterSettings), cx: &mut Context<Self>) {
        let Some(before) = self.settings.clone() else {
            return;
        };
        let mut settings = before.clone();
        change(&mut settings);
        if settings == before {
            return;
        }
        let refilter = settings.zdr() != before.zdr() || settings.region() != before.region();
        self.settings = Some(settings.clone());
        cx.notify();
        let backend = cx.global::<SharedBackend>().clone();
        let previous = self._save.take();
        self._save = Some(cx.spawn(async move |this, cx| {
            if let Some(previous) = previous {
                previous.await;
            }
            let saved = cx
                .background_spawn(async move { backend.set_openrouter(settings) })
                .await;
            this.update(cx, |this, cx| {
                match saved {
                    Ok(()) => {
                        this.problem = None;
                        if refilter {
                            this.fetch(false, cx);
                        }
                    }
                    Err(why) => {
                        this.problem = Some(curly(&why));
                        this.read(cx);
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// What's typed in a provider list changed or was left: saved once the
    /// typing rests, at once on Enter or when the field loses focus.
    fn typed(&mut self, which: Providers, event: &InputEvent, cx: &mut Context<Self>) {
        match event {
            InputEvent::Change => {
                self._typing = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(TYPING_REST).await;
                    this.update(cx, |this, cx| this.save_providers(which, cx))
                        .ok();
                }));
            }
            InputEvent::PressEnter { .. } | InputEvent::Blur => {
                self._typing = None;
                self.save_providers(which, cx);
            }
            InputEvent::Focus => {}
        }
    }

    fn save_providers(&mut self, which: Providers, cx: &mut Context<Self>) {
        let typed = self.input(which).read(cx).value();
        let providers = slugs(&typed);
        self.change(
            |settings| match which {
                Providers::Only => settings.set_only(providers),
                Providers::Ignore => settings.set_ignore(providers),
            },
            cx,
        );
    }

    fn input(&self, which: Providers) -> &Entity<InputState> {
        match which {
            Providers::Only => &self.only,
            Providers::Ignore => &self.ignore,
        }
    }

    /// The fields typed into: Only these providers and Never these
    /// providers (tests).
    #[cfg(test)]
    pub fn inputs(&self) -> (Entity<InputState>, Entity<InputState>) {
        (self.only.clone(), self.ignore.clone())
    }

    /// What the Models row says: how many, and when they were fetched.
    pub fn models_words(&self) -> String {
        let Some(catalog) = &self.catalog else {
            return "Loading…".into();
        };
        let mut words = match (catalog.models.len(), catalog.updated()) {
            (0, _) if self.fetching => "Loading…".to_string(),
            (0, _) if catalog.fetched.is_some() => {
                "None with tool use under these settings".to_string()
            }
            (0, _) => "Not loaded yet".to_string(),
            (count, Some(updated)) => format!("{count} with tool use, {updated}"),
            (count, None) => format!("{count} with tool use"),
        };
        if let Some(problem) = &catalog.problem {
            words = format!(
                "{}. {}",
                sentence(&words).trim_end_matches('.'),
                sentence(problem)
            );
            words = curly(&words);
        }
        words
    }

    /// The group, `focus` being the first row's, marked when `marked`
    /// (Settings opened from the composer's OpenRouter settings).
    pub fn group(
        &self,
        this: &Entity<Self>,
        focus: Option<FocusHandle>,
        marked: bool,
        cx: &App,
    ) -> SettingGroup {
        let Some(settings) = &self.settings else {
            return SettingGroup::new()
                .title("OpenRouter")
                .description(ABOUT)
                .item(
                    SettingItem::new("Routing", SettingField::render(|_, _, _| div()))
                        .description("Checking…"),
                );
        };
        let implied = settings.data_collection_implied();
        let description = match &self.problem {
            Some(problem) => format!("{} {ABOUT}", sentence(problem)),
            None => ABOUT.to_string(),
        };
        let (ring, radius) = (cx.theme().ring, cx.theme().radius);
        SettingGroup::new()
            .title("OpenRouter")
            .description(description)
            .items([
                SettingItem::new(Control::Zdr.label(), {
                    let (page, toggling) = (this.clone(), this.clone());
                    SettingField::render(move |_, _, cx| {
                        let on = page
                            .read(cx)
                            .settings
                            .as_ref()
                            .is_some_and(OpenRouterSettings::zdr);
                        let toggle = toggling.clone();
                        let switch = Switch::new("openrouter-zdr")
                            .checked(on)
                            .accessibility_label(Control::Zdr.label())
                            .on_click(move |on, _, cx| {
                                let on = *on;
                                toggle.update(cx, |part, cx| {
                                    part.change(|settings| settings.set_zdr(on), cx)
                                });
                            });
                        let keyed = toggling.clone();
                        // Settings opens here from the composer: the row is
                        // marked and has the keyboard.
                        h_flex()
                            .id("account-openrouter-settings")
                            .test_support()
                            .when_some(focus.clone(), |this, focus| {
                                this.track_focus(&focus).on_key_down(
                                    move |event: &KeyDownEvent, window, cx| {
                                        let keystroke = &event.keystroke;
                                        if focus.is_focused(window)
                                            && !keystroke.modifiers.modified()
                                            && matches!(keystroke.key.as_str(), "enter" | "space")
                                        {
                                            cx.stop_propagation();
                                            keyed.update(cx, |part, cx| {
                                                part.change(
                                                    |settings| settings.set_zdr(!settings.zdr()),
                                                    cx,
                                                )
                                            });
                                        }
                                    },
                                )
                            })
                            .rounded(radius)
                            .when(marked, |this| this.border_1().border_color(ring))
                            .child(switch)
                    })
                    .on_reset(
                        {
                            let page = this.clone();
                            move |cx| {
                                page.read(cx)
                                    .settings
                                    .as_ref()
                                    .is_some_and(OpenRouterSettings::zdr)
                            }
                        },
                        {
                            let page = this.clone();
                            move |_, cx| {
                                page.update(cx, |part, cx| {
                                    part.change(|settings| settings.set_zdr(false), cx)
                                })
                            }
                        },
                    )
                })
                .description(curly(Control::Zdr.about()))
                .keywords(["openrouter", "zdr", "privacy", "retention"]),
                SettingItem::new(
                    Control::DataCollection.label(),
                    self.switch(
                        this,
                        |settings| {
                            settings.deny_data_collection() || settings.data_collection_implied()
                        },
                        |settings, on| settings.set_deny_data_collection(on),
                    ),
                )
                .disabled(implied)
                .description(if implied {
                    "Zero data retention covers it.".to_string()
                } else {
                    curly(Control::DataCollection.about())
                })
                .keywords(["openrouter", "privacy", "training", "data collection"]),
                SettingItem::new(
                    Control::Region.label(),
                    self.dropdown(
                        this,
                        Region::ALL.iter().map(|region| (region.word(), region.label())),
                        |settings| settings.region().word(),
                        |settings, word| {
                            if let Some(region) = Region::ALL.into_iter().find(|r| r.word() == word)
                            {
                                settings.set_region(region)
                            }
                        },
                        Region::Any.word(),
                    ),
                )
                .description(
                    "Keeps requests in a region. It needs OpenRouter’s Business or Enterprise plan, and fewer models offer it. US or EU sends each model to the US when it’s offered there, else to the EU.",
                )
                .keywords(["openrouter", "region", "residency", "us", "eu"]),
                SettingItem::new(
                    Control::Sort.label(),
                    self.dropdown(
                        this,
                        Sort::ALL.iter().map(|sort| (sort.word(), sort.label())),
                        |settings| settings.sort().word(),
                        |settings, word| {
                            if let Some(sort) = Sort::ALL.into_iter().find(|s| s.word() == word) {
                                settings.set_sort(sort)
                            }
                        },
                        Sort::Default.word(),
                    ),
                )
                .description(curly(Control::Sort.about()))
                .keywords(["openrouter", "sort", "price", "throughput", "latency"]),
                SettingItem::new(
                    Control::Precision.label(),
                    self.dropdown(
                        this,
                        Precision::ALL
                            .iter()
                            .chain(
                                (settings.precision() == Precision::Custom)
                                    .then_some(&Precision::Custom),
                            )
                            .map(|precision| (precision.word(), precision.label())),
                        |settings| settings.precision().word(),
                        |settings, word| {
                            if let Some(precision) =
                                Precision::ALL.into_iter().find(|p| p.word() == word)
                            {
                                settings.set_precision(precision)
                            }
                        },
                        Precision::Any.word(),
                    ),
                )
                .description(curly(Control::Precision.about()))
                .keywords(["openrouter", "quantization", "precision", "fp8", "bf16"]),
                self.providers_item(this, Providers::Only)
                    .description(curly(Control::Only.about()))
                    .keywords(["openrouter", "providers", "only"]),
                self.providers_item(this, Providers::Ignore)
                    .description(curly(Control::Ignore.about()))
                    .keywords(["openrouter", "providers", "ignore", "never"]),
                SettingItem::new("Models", {
                    let page = this.clone();
                    let fetching = self.fetching;
                    SettingField::render(move |_, _, _| {
                        let page = page.clone();
                        Button::new("openrouter-refresh")
                            .outline()
                            .small()
                            .label("Refresh")
                            .loading(fetching)
                            .on_click(move |_, _, cx| {
                                page.update(cx, |part, cx| part.fetch(true, cx))
                            })
                    })
                })
                .description(self.models_words())
                .keywords(["openrouter", "models", "catalog", "refresh"]),
            ])
    }

    /// A switch over a flag of the settings.
    fn switch(
        &self,
        this: &Entity<Self>,
        get: fn(&OpenRouterSettings) -> bool,
        set: fn(&mut OpenRouterSettings, bool),
    ) -> SettingField<bool> {
        let (page, setting) = (this.clone(), this.clone());
        SettingField::switch(
            move |cx: &App| page.read(cx).settings.as_ref().is_some_and(get),
            move |on: bool, cx: &mut App| {
                setting.update(cx, |part, cx| part.change(|settings| set(settings, on), cx))
            },
        )
        .default_value(false)
    }

    /// A dropdown over `options` (word, label), its default `default`.
    fn dropdown<'a>(
        &self,
        this: &Entity<Self>,
        options: impl Iterator<Item = (&'a str, &'a str)>,
        get: fn(&OpenRouterSettings) -> &'static str,
        set: fn(&mut OpenRouterSettings, &str),
        default: &'static str,
    ) -> SettingField<SharedString> {
        let (page, setting) = (this.clone(), this.clone());
        let options = options
            .map(|(word, label)| (word.to_string().into(), label.to_string().into()))
            .collect();
        SettingField::dropdown(
            options,
            move |cx: &App| {
                page.read(cx)
                    .settings
                    .as_ref()
                    .map(get)
                    .unwrap_or(default)
                    .into()
            },
            move |word: SharedString, cx: &mut App| {
                setting.update(cx, |part, cx| {
                    part.change(|settings| set(settings, &word), cx)
                })
            },
        )
        .default_value(default)
    }

    /// A field for a list of provider slugs, separated by commas.
    fn providers_item(&self, this: &Entity<Self>, which: Providers) -> SettingItem {
        let (title, id) = match which {
            Providers::Only => (Control::Only.label(), "openrouter-only"),
            Providers::Ignore => (Control::Ignore.label(), "openrouter-ignore"),
        };
        let list = move |settings: &OpenRouterSettings| match which {
            Providers::Only => settings.only().to_vec(),
            Providers::Ignore => settings.ignore().to_vec(),
        };
        let input = self.input(which).clone();
        let page = this.clone();
        let (dirty, reset) = (this.clone(), this.clone());
        SettingItem::new(
            title,
            SettingField::render(move |_, window, cx| {
                // Shows the list as saved, unless it's being typed.
                let saved = page
                    .read(cx)
                    .settings
                    .as_ref()
                    .map(list)
                    .unwrap_or_default();
                let typed = input.read(cx).value();
                if slugs(&typed) != saved && !input.read(cx).focus_handle(cx).is_focused(window) {
                    input.update(cx, |input, cx| {
                        input.set_value(saved.join(", "), window, cx)
                    });
                }
                v_flex()
                    .id(id)
                    .test_support()
                    .child(Input::new(&input).small().w(rems(14.)))
            })
            .on_reset(
                move |cx| {
                    dirty
                        .read(cx)
                        .settings
                        .as_ref()
                        .is_some_and(|settings| !list(settings).is_empty())
                },
                move |_, cx| {
                    reset.update(cx, |part, cx| {
                        part.change(
                            |settings| match which {
                                Providers::Only => settings.set_only(Vec::new()),
                                Providers::Ignore => settings.set_ignore(Vec::new()),
                            },
                            cx,
                        )
                    })
                },
            ),
        )
    }
}
