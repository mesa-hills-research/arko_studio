//! Keep-warm for every session in Settings › Sessions › Prompt cache, for
//! up to some hours after the last message, and a limit on what its
//! refreshes cost. It is arko's setting in Arko's folder, which the terminal and the
//! web page change too, and it's off unless the user turns it on. Studio
//! alone writes it for its sessions. A choice is saved through the app's
//! queue of writes, off the UI thread, which goes on after Settings closes,
//! and each save that goes through has every open session read it again and
//! take it, saving nothing. The page shows the choice being saved, and once
//! the saves end, what the file holds and why a save failed.

use arko::keep_warm::{self, Limits, SETTING_DOLLARS, SETTING_HOURS};
use gpui_kit::component::setting::{SettingField, SettingGroup, SettingItem};
use gpui_kit::component::{ActiveTheme as _, h_flex};
use gpui_kit::*;

use super::SessionsPage;
use crate::backend::{SharedBackend, observe_saves, save_state, write_latest};
use crate::live::LiveSessions;
use crate::workers::curly;

/// What the group shows and the writes it waits for.
#[derive(Default)]
pub(super) struct KeepWarm {
    /// The setting as last read or chosen, `None` while off.
    setting: Option<Limits>,
    /// It shows a choice whose save hasn't ended.
    saving: bool,
    /// Why the last save failed.
    error: Option<SharedString>,
    /// The reading of the file as the page opens, until a choice replaces it.
    reading: Option<Task<()>>,
}

/// Each open session reads the setting again and takes it, saving nothing.
fn reload_sessions(cx: &mut App) {
    for session in LiveSessions::all(cx) {
        session
            .read(cx)
            .request(|reply| arko::Request::ReloadKeepWarmSetting { reply }, cx)
            .detach();
    }
}

/// The hours as the menu says them: "Off", "1 hour", "15 hours".
fn hours_label(hours: u32) -> SharedString {
    match hours {
        0 => "Off".into(),
        1 => "1 hour".into(),
        hours => format!("{hours} hours").into(),
    }
}

/// The dollars as the menu says them: "None", "$5.00".
fn dollars_label(dollars: f64) -> SharedString {
    if dollars > 0.0 {
        format!("${dollars:.2}").into()
    } else {
        "None".into()
    }
}

/// The setting with `hours` (0 for off), keeping its dollars.
pub fn with_hours(setting: Option<Limits>, hours: u32) -> Option<Limits> {
    (hours > 0).then(|| {
        let mut limits = Limits::default();
        limits.idle_hours = Some(hours);
        limits.dollars = setting.and_then(|setting| setting.dollars);
        limits
    })
}

/// The setting with `dollars` (0 for none), keeping its hours.
pub fn with_dollars(setting: Option<Limits>, dollars: f64) -> Option<Limits> {
    setting.map(|mut limits| {
        limits.dollars = (dollars > 0.0).then_some(dollars);
        limits
    })
}

/// The menu's steps (arko's, as the terminal's settings step through
/// them), with `current` among them where a command set another.
fn steps<T: PartialEq + Copy>(steps: &[T], current: T) -> Vec<T> {
    let mut steps = steps.to_vec();
    if !steps.contains(&current) {
        steps.push(current);
    }
    steps
}

impl SessionsPage {
    /// Follows the app's saves of the setting.
    pub(super) fn observe_keep_warm(cx: &mut Context<Self>) -> Subscription {
        observe_saves::<Option<Limits>, _>(cx, |page, cx| page.follow_keep_warm_saves(cx))
    }

    /// Shows the choice being saved. Once the saves end, shows what the file
    /// holds, and why the last save failed if it did.
    fn follow_keep_warm_saves(&mut self, cx: &mut Context<Self>) {
        let state = save_state::<Option<Limits>>(cx);
        match state.pending {
            Some(setting) => {
                self.keep_warm.reading = None;
                self.keep_warm.setting = setting;
                self.keep_warm.saving = true;
                self.keep_warm.error = None;
            }
            None if self.keep_warm.saving => self.read_keep_warm(cx),
            None => {}
        }
        cx.notify();
    }

    /// Reads the setting from Arko's folder, in the background, or takes
    /// the choice whose save hasn't ended, with why the last save failed.
    pub(super) fn read_keep_warm(&mut self, cx: &mut Context<Self>) {
        let state = save_state::<Option<Limits>>(cx);
        self.keep_warm.error = state.error.as_deref().map(|why| curly(why).into());
        if let Some(setting) = state.pending {
            self.keep_warm.setting = setting;
            self.keep_warm.saving = true;
            return;
        }
        self.keep_warm.saving = false;
        let backend = cx.global::<SharedBackend>().clone();
        let read = cx.background_spawn(async move { keep_warm::load(backend.data_root()) });
        self.keep_warm.reading = Some(cx.spawn(async move |this, cx| {
            let setting = read.await;
            this.update(cx, |this, cx| {
                this.keep_warm.setting = setting;
                cx.notify();
            })
            .ok();
        }));
    }

    /// Sets keep-warm for every session to `setting`, or off.
    pub fn set_keep_warm(&mut self, setting: Option<Limits>, cx: &mut Context<Self>) {
        self.keep_warm.reading = None;
        self.keep_warm.setting = setting;
        self.keep_warm.saving = true;
        self.keep_warm.error = None;
        write_latest(
            setting,
            |backend, setting| keep_warm::save(backend.data_root(), setting),
            reload_sessions,
            cx,
        );
        cx.notify();
    }

    /// The setting as the page shows it (tests).
    #[cfg(test)]
    pub fn keep_warm_setting(&self) -> Option<Limits> {
        self.keep_warm.setting
    }

    /// Why the last save failed, as the page says it (tests).
    #[cfg(test)]
    pub fn keep_warm_error(&self) -> Option<SharedString> {
        self.keep_warm.error.clone()
    }

    pub(super) fn keep_warm_group(&self, this: &Entity<Self>) -> SettingGroup {
        let setting = self.keep_warm.setting;
        let shown = (
            setting.and_then(|setting| setting.idle_hours),
            setting.and_then(|setting| setting.dollars),
            self.keep_warm.error.as_deref(),
        );
        let hours = setting.and_then(|setting| setting.idle_hours).unwrap_or(0);
        let (get, set) = (this.clone(), this.clone());
        let mut items = vec![
            SettingItem::new(
                keep_warm::NAME,
                SettingField::dropdown(
                    steps(&SETTING_HOURS, hours)
                        .into_iter()
                        .map(|hours| (hours.to_string().into(), hours_label(hours)))
                        .collect(),
                    move |cx: &App| {
                        let setting = get.read(cx).keep_warm.setting;
                        let hours = setting.and_then(|setting| setting.idle_hours);
                        hours.unwrap_or(0).to_string().into()
                    },
                    move |value: SharedString, cx: &mut App| {
                        if let Ok(hours) = value.parse() {
                            set.update(cx, |page, cx| {
                                page.set_keep_warm(with_hours(page.keep_warm.setting, hours), cx)
                            });
                        }
                    },
                )
                .default_value("0"),
            )
            .description(keep_warm::setting_sentence(setting).replace('\'', "’"))
            .keywords(["prompt cache", "keep-warm", "refresh", "idle", "hours"]),
        ];
        if let Some(limits) = setting {
            let dollars = limits.dollars.unwrap_or(0.0);
            let (get, set) = (this.clone(), this.clone());
            items.push(
                SettingItem::new(
                    "Keep-warm limit",
                    SettingField::dropdown(
                        steps(&SETTING_DOLLARS, dollars)
                            .into_iter()
                            .map(|dollars| (dollars.to_string().into(), dollars_label(dollars)))
                            .collect(),
                        move |cx: &App| {
                            let setting = get.read(cx).keep_warm.setting;
                            let dollars = setting.and_then(|setting| setting.dollars);
                            dollars.unwrap_or(0.0).to_string().into()
                        },
                        move |value: SharedString, cx: &mut App| {
                            if let Ok(dollars) = value.parse() {
                                set.update(cx, |page, cx| {
                                    page.set_keep_warm(
                                        with_dollars(page.keep_warm.setting, dollars),
                                        cx,
                                    )
                                });
                            }
                        },
                    )
                    .default_value("0"),
                )
                .description(if limits.dollars.is_some() {
                    "Keep-warm stops before a session’s refreshes cost more at API prices, estimated on a plan."
                } else {
                    "Only the hours stop keep-warm."
                })
                .keywords(["prompt cache", "keep-warm", "dollars", "budget", "plan"]),
            );
        }
        if let Some(error) = self.keep_warm.error.clone() {
            items.push(SettingItem::render(move |_, _, cx| {
                h_flex()
                    .id("keep-warm-error")
                    .test_support()
                    .text_sm()
                    .text_color(cx.theme().danger)
                    .child(error.clone())
            }));
        }
        SettingGroup::new()
            .title("Prompt cache")
            .key(super::super::group_key("Prompt cache", &shown))
            .items(items)
    }
}

#[cfg(test)]
mod tests {
    use super::{steps, with_dollars, with_hours};

    /// Hours keep the dollars, and off drops both. Dollars need hours, and
    /// none clears them.
    #[test]
    fn hours_and_dollars_change_one_at_a_time() {
        let on = with_hours(None, 15).unwrap();
        assert_eq!((on.idle_hours, on.dollars), (Some(15), None));
        let limited = with_dollars(Some(on), 5.0).unwrap();
        assert_eq!((limited.idle_hours, limited.dollars), (Some(15), Some(5.0)));
        let longer = with_hours(Some(limited), 24).unwrap();
        assert_eq!((longer.idle_hours, longer.dollars), (Some(24), Some(5.0)));
        assert_eq!(with_hours(Some(longer), 0), None);
        assert_eq!(with_dollars(None, 5.0), None);
        assert_eq!(with_dollars(Some(longer), 0.0).unwrap().dollars, None);
        // A value a command set shows among the steps.
        assert_eq!(steps(&[0, 1, 8], 3), vec![0, 1, 8, 3]);
        assert_eq!(steps(&[0, 1, 8], 8), vec![0, 1, 8]);
    }

    /// The page shows the setting in Arko's folder. A choice is saved there,
    /// by the app alone, and each open session then takes it again from the
    /// file, saving nothing.
    #[gpui_kit::test]
    fn a_choice_is_saved_once_and_open_sessions_reload_it(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::AppContext as _;

        use crate::backend::FakeBackend;
        use crate::live::{LiveSession, LiveSessions};
        use crate::settings_page::SESSIONS_PAGE;
        use crate::settings_page::interaction::{settings_window, start_on};

        let temp = tempfile::tempdir().unwrap();
        arko::keep_warm::save(temp.path(), with_dollars(with_hours(None, 15), 5.0)).unwrap();
        start_on(
            cx,
            FakeBackend::empty(temp.path()),
            crate::prefs::Prefs::default(),
        );
        let (_, _, settings) = settings_window(cx, SESSIONS_PAGE, 4000.);
        let page = cx.read(|cx| settings.read(cx).sessions().clone());
        cx.run_until_parked();
        let shown = cx.read(|cx| page.read(cx).keep_warm_setting()).unwrap();
        assert_eq!((shown.idle_hours, shown.dollars), (Some(15), Some(5.0)));

        let session = cx.new(|cx| LiveSession::for_tests("s1", cx));
        cx.update(|cx| LiveSessions::add_for_tests(session.clone(), cx));
        page.update(cx, |page, cx| {
            page.set_keep_warm(with_hours(page.keep_warm_setting(), 8), cx)
        });
        cx.run_until_parked();
        let saved = arko::keep_warm::load(temp.path()).unwrap();
        assert_eq!((saved.idle_hours, saved.dollars), (Some(8), Some(5.0)));
        let requests = session.update(cx, |session, _| session.take_requests());
        assert!(
            matches!(
                requests.as_slice(),
                [arko::Request::ReloadKeepWarmSetting { .. }]
            ),
            "{requests:?}"
        );
        assert!(
            cx.read(|cx| session.read(cx).queued().is_empty()),
            "no command"
        );

        page.update(cx, |page, cx| page.set_keep_warm(None, cx));
        cx.run_until_parked();
        assert_eq!(arko::keep_warm::load(temp.path()), None);
        assert_eq!(cx.read(|cx| page.read(cx).keep_warm_setting()), None);
    }

    /// Settings opened again while a save is on its way hears how it ended.
    /// A failed save says why, and the page shows what the file holds.
    #[gpui_kit::test]
    fn settings_opened_during_a_save_hears_that_it_failed(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::AppContext as _;
        use gpui_kit::test::TestWindowExt as _;

        use crate::backend::FakeBackend;
        use crate::settings_page::SESSIONS_PAGE;
        use crate::settings_page::interaction::{settings_window, start_on};

        let temp = tempfile::tempdir().unwrap();
        // A folder where the file goes, so the save fails.
        std::fs::create_dir(temp.path().join("keep-warm.json")).unwrap();
        start_on(
            cx,
            FakeBackend::empty(temp.path()),
            crate::prefs::Prefs::default(),
        );
        let (window, workspace, settings) = settings_window(cx, SESSIONS_PAGE, 4000.);
        let page = cx.read(|cx| settings.read(cx).sessions().clone());
        cx.run_until_parked();
        cx.update_window(window, |_, window, cx| {
            page.update(cx, |page, cx| page.set_keep_warm(with_hours(None, 12), cx));
            window.press("escape", cx);
        })
        .unwrap();
        drop((page, settings));
        cx.update_window(window, |_, window, cx| window.press("secondary-,", cx))
            .unwrap();
        let reopened = cx.read(|cx| workspace.read(cx).settings().unwrap().clone());
        let page = cx.read(|cx| reopened.read(cx).sessions().clone());
        let shown = cx.read(|cx| page.read(cx).keep_warm_setting());
        assert_eq!(shown.and_then(|setting| setting.idle_hours), Some(12));
        cx.run_until_parked();
        assert_eq!(arko::keep_warm::load(temp.path()), None);
        let error = cx.read(|cx| page.read(cx).keep_warm_error());
        assert!(
            error.is_some_and(|why| why.starts_with("Couldn’t save the keep-warm setting")),
            "the reopened page says the save failed"
        );
        assert_eq!(cx.read(|cx| page.read(cx).keep_warm_setting()), None);
    }

    /// A save that went through reaches the open sessions also when a later
    /// one fails, and the page shows the saved value with why the later one
    /// failed.
    #[gpui_kit::test]
    fn a_save_that_went_through_reaches_the_sessions_before_a_later_failure(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use gpui_kit::AppContext as _;

        use crate::backend::FakeBackend;
        use crate::live::{LiveSession, LiveSessions};
        use crate::settings_page::SESSIONS_PAGE;
        use crate::settings_page::interaction::{settings_window, start_on};

        let temp = tempfile::tempdir().unwrap();
        start_on(
            cx,
            FakeBackend::empty(temp.path()),
            crate::prefs::Prefs::default(),
        );
        let (_, _, settings) = settings_window(cx, SESSIONS_PAGE, 4000.);
        let page = cx.read(|cx| settings.read(cx).sessions().clone());
        cx.run_until_parked();
        let session = cx.new(|cx| LiveSession::for_tests("s1", cx));
        cx.update(|cx| LiveSessions::add_for_tests(session.clone(), cx));
        page.update(cx, |page, cx| {
            page.set_keep_warm(with_hours(None, 1), cx);
            crate::backend::write_latest::<Option<arko::keep_warm::Limits>>(
                with_hours(None, 12),
                |_, _| Err("The disk is full".into()),
                |_| {},
                cx,
            );
        });
        cx.run_until_parked();
        assert_eq!(
            arko::keep_warm::load(temp.path()).and_then(|setting| setting.idle_hours),
            Some(1)
        );
        let requests = session.update(cx, |session, _| session.take_requests());
        assert!(
            matches!(
                requests.as_slice(),
                [arko::Request::ReloadKeepWarmSetting { .. }]
            ),
            "{requests:?}"
        );
        let shown = cx.read(|cx| page.read(cx).keep_warm_setting());
        assert_eq!(shown.and_then(|setting| setting.idle_hours), Some(1));
        let error = cx.read(|cx| page.read(cx).keep_warm_error());
        assert_eq!(error.as_deref(), Some("The disk is full"));
    }

    /// The last choice is written also when Settings closes before the
    /// write ends.
    #[gpui_kit::test]
    fn the_last_choice_is_written_after_settings_closes(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::AppContext as _;
        use gpui_kit::test::TestWindowExt as _;

        use crate::backend::FakeBackend;
        use crate::settings_page::SESSIONS_PAGE;
        use crate::settings_page::interaction::{settings_window, start_on};

        let temp = tempfile::tempdir().unwrap();
        start_on(
            cx,
            FakeBackend::empty(temp.path()),
            crate::prefs::Prefs::default(),
        );
        let (window, workspace, settings) = settings_window(cx, SESSIONS_PAGE, 4000.);
        let page = cx.read(|cx| settings.read(cx).sessions().clone());
        cx.run_until_parked();
        cx.update_window(window, |_, window, cx| {
            page.update(cx, |page, cx| {
                page.set_keep_warm(with_hours(None, 1), cx);
                page.set_keep_warm(with_hours(None, 12), cx);
            });
            window.press("escape", cx);
        })
        .unwrap();
        assert!(
            cx.read(|cx| workspace.read(cx).settings().is_none()),
            "closed"
        );
        drop((page, settings));
        // Opened again before the save ends, Settings shows what's saving.
        cx.update_window(window, |_, window, cx| window.press("secondary-,", cx))
            .unwrap();
        let reopened = cx.read(|cx| workspace.read(cx).settings().unwrap().clone());
        let page = cx.read(|cx| reopened.read(cx).sessions().clone());
        let shown = cx.read(|cx| page.read(cx).keep_warm_setting());
        assert_eq!(shown.and_then(|setting| setting.idle_hours), Some(12));
        cx.run_until_parked();
        let saved = arko::keep_warm::load(temp.path()).unwrap();
        assert_eq!(saved.idle_hours, Some(12));
    }
}
