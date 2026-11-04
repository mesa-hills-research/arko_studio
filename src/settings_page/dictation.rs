//! Dictation's cleanup in Settings › Input, what dictation takes out of
//! English speech. It is Arko's setting in `dictation.json` in Arko's
//! folder, which the terminal follows too, and each recording reads it as it
//! starts. A choice is saved through the app's queue of writes, off the UI
//! thread. The page shows the choice being saved, and once the saves end,
//! what the file holds and why a save failed.

use arko::dictation::settings::{self, Cleanup, Settings};
use gpui_kit::component::setting::{SettingField, SettingGroup, SettingItem};
use gpui_kit::component::{ActiveTheme as _, h_flex};
use gpui_kit::*;

use super::SettingsPage;
use crate::backend::{SharedBackend, observe_saves, save_state, write_latest};
use crate::workers::curly;

/// What the setting says under its name.
const DESCRIPTION: &str = "Light takes um, uh and stutters out of English dictation. Medium and Full also take out repeats and false starts, and Full catches the most. With Medium or Full, words settle when you pause.";

/// What the group shows and the writes it waits for.
pub(super) struct CleanupSetting {
    /// The level as last read or chosen.
    level: Cleanup,
    /// It shows a choice whose save hasn't ended.
    saving: bool,
    /// Why the last save failed.
    error: Option<SharedString>,
    /// The reading of the file as the page opens, until a choice replaces it.
    reading: Option<Task<()>>,
}

impl Default for CleanupSetting {
    fn default() -> Self {
        Self {
            level: Cleanup::Off,
            saving: false,
            error: None,
            reading: None,
        }
    }
}

/// A level as the menu says it.
fn label(level: Cleanup) -> &'static str {
    match level {
        Cleanup::Off => "Off",
        Cleanup::Light => "Light",
        Cleanup::Medium => "Medium",
        Cleanup::Full => "Full",
    }
}

impl SettingsPage {
    /// Follows the app's saves of the setting.
    pub(super) fn observe_cleanup(cx: &mut Context<Self>) -> Subscription {
        observe_saves::<Cleanup, _>(cx, |page, cx| page.follow_cleanup_saves(cx))
    }

    /// Shows the choice being saved. Once the saves end, shows what the file
    /// holds, and why the last save failed if it did.
    fn follow_cleanup_saves(&mut self, cx: &mut Context<Self>) {
        let state = save_state::<Cleanup>(cx);
        match state.pending {
            Some(level) => {
                self.cleanup.reading = None;
                self.cleanup.level = level;
                self.cleanup.saving = true;
                self.cleanup.error = None;
            }
            None if self.cleanup.saving => self.read_cleanup(cx),
            None => {}
        }
        cx.notify();
    }

    /// Reads the setting from Arko's folder, in the background, or takes
    /// the choice whose save hasn't ended, with why the last save failed.
    pub(super) fn read_cleanup(&mut self, cx: &mut Context<Self>) {
        let state = save_state::<Cleanup>(cx);
        self.cleanup.error = state.error.as_deref().map(|why| curly(why).into());
        if let Some(level) = state.pending {
            self.cleanup.level = level;
            self.cleanup.saving = true;
            return;
        }
        self.cleanup.saving = false;
        let backend = cx.global::<SharedBackend>().clone();
        let read =
            cx.background_spawn(async move { Settings::load(backend.data_root()).cleanup() });
        self.cleanup.reading = Some(cx.spawn(async move |this, cx| {
            let level = read.await;
            this.update(cx, |this, cx| {
                this.cleanup.level = level;
                cx.notify();
            })
            .ok();
        }));
    }

    /// Sets what dictation takes out of English speech.
    pub fn set_cleanup(&mut self, level: Cleanup, cx: &mut Context<Self>) {
        self.cleanup.reading = None;
        self.cleanup.level = level;
        self.cleanup.saving = true;
        self.cleanup.error = None;
        write_latest(
            level,
            |backend, level| {
                settings::set_cleanup(backend.data_root(), level)
                    .map_err(|error| format!("Couldn’t save the dictation settings ({error})."))
            },
            |_| {},
            cx,
        );
        cx.notify();
    }

    /// The level as the page shows it (tests).
    #[cfg(test)]
    pub fn cleanup(&self) -> Cleanup {
        self.cleanup.level
    }

    /// Why the last save failed, as the page says it (tests).
    #[cfg(test)]
    pub fn cleanup_error(&self) -> Option<SharedString> {
        self.cleanup.error.clone()
    }

    pub(super) fn cleanup_group(&self, this: &Entity<Self>) -> SettingGroup {
        let shown = (self.cleanup.level.name(), self.cleanup.error.as_deref());
        let (get, set) = (this.clone(), this.clone());
        let mut items = vec![
            SettingItem::new(
                "Cleanup",
                SettingField::dropdown(
                    Cleanup::ALL
                        .into_iter()
                        .map(|level| (level.name().into(), label(level).into()))
                        .collect(),
                    move |cx: &App| get.read(cx).cleanup.level.name().into(),
                    move |value: SharedString, cx: &mut App| {
                        if let Some(level) = Cleanup::named(&value) {
                            set.update(cx, |page, cx| page.set_cleanup(level, cx));
                        }
                    },
                )
                .default_value(Cleanup::Off.name()),
            )
            .description(DESCRIPTION)
            .keywords(["dictation", "speech", "voice", "fillers", "um"]),
        ];
        if let Some(error) = self.cleanup.error.clone() {
            items.push(SettingItem::render(move |_, _, cx| {
                h_flex()
                    .id("cleanup-error")
                    .test_support()
                    .text_sm()
                    .text_color(cx.theme().danger)
                    .child(error.clone())
            }));
        }
        SettingGroup::new()
            .title("Dictation")
            .key(super::group_key("Dictation", &shown))
            .items(items)
    }
}

#[cfg(test)]
mod tests {
    use arko::dictation::settings::{Cleanup, FILE};

    use crate::backend::FakeBackend;
    use crate::settings_page::INPUT_PAGE;
    use crate::settings_page::interaction::{settings_window, start_on};

    /// The page shows the level `dictation.json` holds, and a choice saves
    /// it there, keeping the file's other settings.
    #[gpui_kit::test]
    fn the_cleanup_is_read_and_saved_in_arkos_folder(cx: &mut gpui_kit::TestAppContext) {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join(FILE);
        std::fs::write(&file, r#"{"model": "parakeet", "cleanup": "light"}"#).unwrap();
        start_on(
            cx,
            FakeBackend::empty(temp.path()),
            crate::prefs::Prefs::default(),
        );
        let (_, _, settings) = settings_window(cx, INPUT_PAGE, 4000.);
        cx.run_until_parked();
        assert_eq!(cx.read(|cx| settings.read(cx).cleanup()), Cleanup::Light);
        settings.update(cx, |page, cx| page.set_cleanup(Cleanup::Full, cx));
        cx.run_until_parked();
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(
            saved,
            serde_json::json!({"model": "parakeet", "cleanup": "full"})
        );
        assert_eq!(cx.read(|cx| settings.read(cx).cleanup()), Cleanup::Full);
        assert_eq!(cx.read(|cx| settings.read(cx).cleanup_error()), None);
    }

    /// A file that isn't JSON stays as it is, and the page says why the
    /// choice wasn't saved and shows what the file means, no cleanup.
    #[gpui_kit::test]
    fn a_file_that_isnt_json_is_left_alone(cx: &mut gpui_kit::TestAppContext) {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join(FILE);
        std::fs::write(&file, "{\"model\": ").unwrap();
        start_on(
            cx,
            FakeBackend::empty(temp.path()),
            crate::prefs::Prefs::default(),
        );
        let (_, _, settings) = settings_window(cx, INPUT_PAGE, 4000.);
        cx.run_until_parked();
        settings.update(cx, |page, cx| page.set_cleanup(Cleanup::Full, cx));
        cx.run_until_parked();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "{\"model\": ");
        assert_eq!(
            cx.read(|cx| settings.read(cx).cleanup_error()).as_deref(),
            Some("Couldn’t save the dictation settings (dictation.json isn’t valid JSON).")
        );
        assert_eq!(cx.read(|cx| settings.read(cx).cleanup()), Cleanup::Off);
    }
}
