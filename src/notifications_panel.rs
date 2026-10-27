//! The notifications panel: what Arko Studio told the user about their
//! sessions, newest first, opened on demand at the right (docked on wide
//! windows, a sheet on narrower ones) from the status bar's unread count,
//! the View menu, the command palette or Mod+Shift+M.
//!
//! Each row gives the session's title, the time and what was said, with a
//! marker while unread. Opening a row shows its session. A row whose session
//! is gone keeps its text and has nothing to open. The panel is a list as
//! the sessions sidebar is, and nothing in it is announced as it changes.

use std::collections::HashSet;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, StyledExt as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::backend::SessionId;
use crate::live::LiveSessions;
use crate::notifications::{self, Entry, NotificationHistory};
use crate::sidebar::Sidebar;
use crate::{figures, menus};

/// The key context of the list.
pub const LIST_CONTEXT: &str = "NotificationsList";

/// How often the times on show are written again while the panel shows.
const TICK: Duration = Duration::from_secs(30);

actions!(
    notifications_panel,
    [
        SelectPrevious,
        SelectNext,
        SelectFirst,
        SelectLast,
        OpenSelected,
        /// From the list back to the composer.
        Back,
    ]
);

/// What the panel asks of the window.
#[derive(Debug, Clone, PartialEq)]
pub enum NotificationsPanelEvent {
    /// Show this session.
    Open(SessionId),
    /// Esc in the list: the keyboard goes back to the composer.
    Leave,
}

/// Returns the time, in Unix milliseconds.
type Clock = Rc<dyn Fn() -> i64>;

pub struct NotificationsPanel {
    /// Tells which sessions still exist.
    sidebar: Entity<Sidebar>,
    focus_handle: FocusHandle,
    /// The notification the keyboard is on.
    cursor: Option<u64>,
    shown: bool,
    scroll: ScrollHandle,
    clock: Option<Clock>,
    /// Draws the times again while the panel shows.
    ticker: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<NotificationsPanelEvent> for NotificationsPanel {}

impl Focusable for NotificationsPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl NotificationsPanel {
    /// A panel over the notification history, opening the sessions
    /// `sidebar` lists and those running.
    pub fn new(sidebar: Entity<Sidebar>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let subscriptions = vec![
            cx.observe_global::<NotificationHistory>(|_, cx| cx.notify()),
            cx.observe(&sidebar, |_, _, cx| cx.notify()),
            cx.on_focus_in(&focus_handle, window, |this, _, cx| {
                if this.cursor.is_none() {
                    this.cursor = notifications::entries(cx).last().map(|entry| entry.id);
                    cx.notify();
                }
            }),
        ];
        Self {
            sidebar,
            focus_handle,
            cursor: None,
            shown: false,
            scroll: ScrollHandle::new(),
            clock: None,
            ticker: None,
            _subscriptions: subscriptions,
        }
    }

    /// Says whether the panel shows: its times move on only then.
    pub fn set_shown(&mut self, shown: bool, cx: &mut Context<Self>) {
        if self.shown == shown {
            return;
        }
        self.shown = shown;
        self.ticker = shown.then(|| {
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(TICK).await;
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
                        break;
                    }
                }
            })
        });
        cx.notify();
    }

    /// Reads the time from `clock` instead of the system's: for tests and
    /// screenshots, whose times must not change.
    #[doc(hidden)]
    pub fn set_clock(&mut self, clock: impl Fn() -> i64 + 'static, cx: &mut Context<Self>) {
        self.clock = Some(Rc::new(clock));
        cx.notify();
    }

    fn now(&self, cx: &App) -> i64 {
        match &self.clock {
            Some(clock) => clock(),
            None => crate::cache_reminders::now(cx),
        }
    }

    /// Gives the list the keyboard.
    pub fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.cursor.is_none() {
            self.cursor = notifications::entries(cx).last().map(|entry| entry.id);
            cx.notify();
        }
        self.focus_handle.focus(window, cx);
    }

    pub fn contains_focus(&self, window: &Window, cx: &App) -> bool {
        self.focus_handle.contains_focused(window, cx)
    }

    /// The ids of the notifications, newest first.
    fn order(&self, cx: &App) -> Vec<u64> {
        notifications::entries(cx)
            .iter()
            .rev()
            .map(|entry| entry.id)
            .collect()
    }

    /// The sessions that can still be shown: those the sidebar lists and
    /// those running (a new one may not be listed yet).
    fn existing(&self, cx: &App) -> HashSet<SessionId> {
        let mut known: HashSet<SessionId> = self
            .sidebar
            .read(cx)
            .sessions()
            .iter()
            .map(|entry| entry.id.clone())
            .collect();
        known.extend(LiveSessions::ids(cx));
        known
    }

    /// The session to show for `entry`, when it still exists.
    fn target(&self, entry: &Entry, cx: &App) -> Option<SessionId> {
        let id = entry.session_id()?;
        self.existing(cx).contains(&id).then_some(id)
    }

    fn open(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(entry) = notifications::entries(cx)
            .iter()
            .find(|entry| entry.id == id)
            .cloned()
        else {
            return;
        };
        if let Some(session) = self.target(&entry, cx) {
            cx.emit(NotificationsPanelEvent::Open(session));
        }
    }

    // ----- the keyboard -----

    fn move_cursor(&mut self, step: isize, cx: &mut Context<Self>) {
        let order = self.order(cx);
        if order.is_empty() {
            return;
        }
        let last = order.len() as isize - 1;
        let at = self
            .cursor
            .and_then(|cursor| order.iter().position(|id| *id == cursor));
        let next = match at {
            None if step > 0 => 0,
            None => last,
            Some(at) => (at as isize + step).rem_euclid(last + 1),
        };
        self.select(next as usize, &order, cx);
    }

    fn select(&mut self, index: usize, order: &[u64], cx: &mut Context<Self>) {
        self.cursor = order.get(index).copied();
        self.scroll.scroll_to_item(index);
        cx.notify();
    }

    fn select_previous(&mut self, _: &SelectPrevious, _: &mut Window, cx: &mut Context<Self>) {
        self.move_cursor(-1, cx);
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        self.move_cursor(1, cx);
    }

    fn select_first(&mut self, _: &SelectFirst, _: &mut Window, cx: &mut Context<Self>) {
        let order = self.order(cx);
        self.select(0, &order, cx);
    }

    fn select_last(&mut self, _: &SelectLast, _: &mut Window, cx: &mut Context<Self>) {
        let order = self.order(cx);
        self.select(order.len().saturating_sub(1), &order, cx);
    }

    fn open_selected(&mut self, _: &OpenSelected, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(cursor) = self.cursor {
            self.open(cursor, cx);
        }
    }

    fn back(&mut self, _: &Back, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(NotificationsPanelEvent::Leave);
    }

    // ----- drawing -----

    fn render_header(&self, cx: &App) -> impl IntoElement {
        let theme = cx.theme();
        h_flex()
            .flex_none()
            .h_10()
            .pl_3()
            .pr_1()
            .gap_1()
            .border_b_1()
            .border_color(theme.sidebar_border)
            .child(
                div()
                    .id("notifications-title")
                    .test_support()
                    .flex_1()
                    .text_sm()
                    .font_medium()
                    .child("Notifications"),
            )
            .child(close_button())
    }

    /// Mark all read and Clear all, over the list.
    fn render_actions(&self, unread: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        h_flex()
            .flex_none()
            .px_2()
            .py_1()
            .gap_1()
            .border_b_1()
            .border_color(theme.sidebar_border)
            .child(
                Button::new("notifications-mark-read")
                    .ghost()
                    .xsmall()
                    .label("Mark all read")
                    .disabled(unread == 0)
                    .on_click(|_, _, cx| notifications::mark_all_read(cx)),
            )
            .child(
                Button::new("notifications-clear")
                    .ghost()
                    .xsmall()
                    .label("Clear all")
                    .on_click(cx.listener(|this, _, _, cx| {
                        notifications::clear(cx);
                        this.cursor = None;
                    })),
            )
    }

    /// A notification's row: the session's title and `when` it came, with
    /// what was said under them. The marker, and the title in a heavier
    /// weight, show that it is unread.
    fn render_row(
        &self,
        index: usize,
        entry: &Entry,
        openable: bool,
        selected: bool,
        when: String,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let unread = !entry.read;
        let title = if entry.title.is_empty() {
            "Session".to_string()
        } else {
            entry.title.clone()
        };
        let label = row_label(&title, &entry.message, &when, unread);
        let id = entry.id;
        let lines = entry
            .message
            .lines()
            .map(str::to_string)
            .collect::<Vec<_>>();
        let (info, muted, ring, hover) = (
            theme.info,
            theme.muted_foreground,
            theme.ring,
            theme.sidebar_accent,
        );
        let hover_text = theme.sidebar_accent_foreground;
        h_flex()
            .id(("notification", index))
            .test_support()
            .role(Role::ListItem)
            .aria_label(SharedString::from(label))
            .w_full()
            .px_2()
            .py_1p5()
            .gap_2()
            .items_start()
            .rounded(theme.radius)
            .border_1()
            .border_color(theme.transparent)
            .when(selected, |this| this.border_color(ring))
            .when(openable, |this| {
                this.cursor_pointer()
                    .hover(|this| this.bg(hover.opacity(0.8)).text_color(hover_text))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.cursor = Some(id);
                        this.focus_handle.focus(window, cx);
                        this.open(id, cx);
                    }))
            })
            .child(
                // The unread marker, in the space it takes when it isn't
                // there, so the titles line up.
                div()
                    .id(("notification-unread", index))
                    .test_support()
                    .flex_none()
                    .mt(rems(0.4))
                    .size_2()
                    .rounded_full()
                    .when(unread, |this| this.bg(info)),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .whitespace_normal()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_sm()
                                    .when(unread, |this| this.font_semibold())
                                    .child(title),
                            )
                            .child(div().flex_none().text_xs().text_color(muted).child(when)),
                    )
                    .children(lines.into_iter().enumerate().map(|(line, text)| {
                        div()
                            .text_xs()
                            .when(!unread || line > 0, |this| this.text_color(muted))
                            .child(text)
                    })),
            )
            .into_any_element()
    }

    fn render_list(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let empty = notifications::entries(cx).is_empty();
        if empty {
            return div()
                .id("notifications-empty")
                .test_support()
                .flex_1()
                .p_3()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child("No notifications")
                .into_any_element();
        }
        let now = self.now(cx);
        let times = figures::Times::of(cx);
        let focused = self.focus_handle.contains_focused(window, cx);
        let entries: Vec<Entry> = notifications::entries(cx).iter().rev().cloned().collect();
        let existing = self.existing(cx);
        let rows: Vec<AnyElement> = entries
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                let openable = entry
                    .session_id()
                    .is_some_and(|session| existing.contains(&session));
                let selected = focused && self.cursor == Some(entry.id);
                let when = figures::ago(entry.at, now, &times);
                self.render_row(index, entry, openable, selected, when, cx)
            })
            .collect();
        v_flex()
            .id("notifications-list")
            .test_support()
            .key_context(LIST_CONTEXT)
            .track_focus(&self.focus_handle)
            .role(Role::List)
            .aria_label(SharedString::from("Notifications"))
            .on_action(cx.listener(Self::select_previous))
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_first))
            .on_action(cx.listener(Self::select_last))
            .on_action(cx.listener(Self::open_selected))
            .on_action(cx.listener(Self::back))
            .flex_1()
            .min_h_0()
            .p_1()
            .gap_0p5()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .children(rows)
            .into_any_element()
    }
}

/// What a screen reader says of a row: the session, what was said, when,
/// and whether it is unread.
pub fn row_label(title: &str, message: &str, when: &str, unread: bool) -> String {
    let said = message.lines().next().unwrap_or("");
    let mut label = format!("{title}, {said}, {when}");
    if unread {
        label.push_str(", unread");
    }
    label
}

/// The close button of the panel's header.
fn close_button() -> Button {
    Button::new("close-notifications")
        .ghost()
        .xsmall()
        .compact()
        .icon(gpui_kit::component::IconName::Close)
        .tooltip_with_action("Close", &menus::ToggleNotifications, None)
        .accessibility_label("Close notifications")
        .on_click(|_, window, cx| window.dispatch_action(Box::new(menus::ToggleNotifications), cx))
}

impl Render for NotificationsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        crate::testing::rendered("NotificationsPanel");
        let theme = cx.theme();
        let (background, foreground) = (theme.sidebar, theme.sidebar_foreground);
        let entries = notifications::entries(cx);
        let unread = notifications::unread(cx);
        let any = !entries.is_empty();
        v_flex()
            .id("notifications-panel")
            .test_support()
            .size_full()
            .bg(background)
            .text_color(foreground)
            .child(self.render_header(cx))
            .when(any, |this| this.child(self.render_actions(unread, cx)))
            .child(self.render_list(window, cx))
    }
}

#[cfg(test)]
mod tests {
    // Named, not `super::*`: GPUI's glob brings a `test` macro that would
    // shadow Rust's.
    use super::{NotificationsPanel, row_label};
    use crate::Startup;
    use crate::backend::FakeBackend;
    use crate::cache_reminders::ReminderClock;
    use crate::notifications::{self, Entry};
    use crate::tests::{act, open, open_title};
    use crate::workspace::Workspace;
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, TestAppContext};
    use std::sync::Arc;
    use std::time::SystemTime;

    const NOW: i64 = 1_800_000_000_000;
    const MINUTE: i64 = 60_000;
    const SPLIT: &str = "20261016-091200-a1b2";
    const FONTS: &str = "20261016-080000-a7b8";
    const GONE: &str = "20260101-000000-dead";

    /// The row `index` of the list, newest first.
    fn row(index: usize) -> (&'static str, usize) {
        ("notification", index)
    }

    fn entry(id: u64, session: &str, title: &str, message: &str, ago: i64, read: bool) -> Entry {
        Entry {
            id,
            session: Some(session.into()),
            title: title.into(),
            message: message.into(),
            at: NOW - ago,
            read,
        }
    }

    /// Four notifications, the oldest read: a turn of the first sample
    /// session, a reminder about the sixth, a question from a session that
    /// has since been deleted, and a second turn.
    fn history() -> Vec<Entry> {
        vec![
            entry(
                1,
                SPLIT,
                "Split the engine out of main.rs",
                "Arko finished its turn.",
                2 * 60 * MINUTE,
                true,
            ),
            entry(
                2,
                FONTS,
                "Variable fonts and the wght axis",
                "The prompt cache expires in 5 minutes\nLetting it expire re-caches ≈ $0.90",
                20 * MINUTE,
                false,
            ),
            entry(
                3,
                GONE,
                "Old session",
                "Arko asks “Tabs or spaces?”",
                3 * MINUTE,
                false,
            ),
            entry(
                4,
                SPLIT,
                "Split the engine out of main.rs",
                "Arko finished its turn.",
                10_000,
                false,
            ),
        ]
    }

    fn start(entries: Vec<Entry>, width: f32, cx: &mut TestAppContext) -> Panel {
        let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
        cx.update(|cx| {
            crate::init(Startup::for_tests(backend).with_notifications(entries), cx);
            cx.set_global(ReminderClock(std::rc::Rc::new(|| NOW)));
        });
        let (window, workspace) = open(cx, width);
        cx.update_window(window, |_, window, _| window.activate_window())
            .unwrap();
        let panel = cx.read(|cx| workspace.read(cx).notifications_panel().clone());
        Panel {
            window,
            workspace,
            panel,
        }
    }

    struct Panel {
        window: AnyWindowHandle,
        workspace: Entity<Workspace>,
        panel: Entity<NotificationsPanel>,
    }

    impl Panel {
        fn open(&self, cx: &mut TestAppContext) {
            act(cx, self.window, |window, cx| {
                window.dispatch_action(Box::new(crate::menus::ToggleNotifications), cx)
            });
        }

        fn is_open(&self, cx: &mut TestAppContext) -> bool {
            cx.read(|cx| self.workspace.read(cx).is_notifications_open())
        }

        fn shows(&self, cx: &mut TestAppContext, id: impl Into<gpui_kit::ElementId>) -> bool {
            let id = id.into();
            cx.update_window(self.window, |_, window, cx| {
                window.render_frame(cx);
                window.try_find(id).is_some()
            })
            .unwrap()
        }

        fn label(&self, cx: &mut TestAppContext, id: impl Into<gpui_kit::ElementId>) -> String {
            let id = id.into();
            cx.update_window(self.window, |_, window, cx| {
                window.render_frame(cx);
                window
                    .find(id)
                    .label()
                    .map(String::from)
                    .unwrap_or_default()
            })
            .unwrap()
        }

        fn click(&self, cx: &mut TestAppContext, id: impl Into<gpui_kit::ElementId>) {
            let id = id.into();
            act(cx, self.window, |window, cx| window.click(id, cx));
        }

        fn press(&self, cx: &mut TestAppContext, key: &str) {
            act(cx, self.window, |window, cx| window.press(key, cx));
        }
    }

    #[test]
    fn a_row_is_named_for_a_screen_reader_by_its_session_message_and_time() {
        assert_eq!(
            row_label("Fonts", "Arko finished its turn.", "3 min ago", true),
            "Fonts, Arko finished its turn., 3 min ago, unread"
        );
        assert_eq!(
            row_label(
                "Fonts",
                "The prompt cache expires in a minute\nRefresh ≈ $0.04",
                "14:05",
                false
            ),
            "Fonts, The prompt cache expires in a minute, 14:05"
        );
    }

    /// With nothing unread the status bar has no button. A count shows
    /// while something is unread, and it opens the panel.
    #[gpui_kit::test]
    fn the_status_bar_shows_a_count_only_while_something_is_unread(cx: &mut TestAppContext) {
        let read: Vec<Entry> = history()
            .into_iter()
            .map(|entry| Entry {
                read: true,
                ..entry
            })
            .collect();
        let panel = start(read, 1300., cx);
        assert!(!panel.shows(cx, "status-notifications"));
        cx.update(|cx| {
            notifications::record(None, "Fonts", "Arko finished its turn.", false, cx);
            notifications::record(None, "Fonts", "Arko asks you something.", false, cx);
        });
        cx.run_until_parked();
        assert!(panel.shows(cx, "status-notifications"));
        assert_eq!(
            panel.label(cx, "status-notifications"),
            "2 unread notifications"
        );
        panel.click(cx, "status-notifications");
        assert!(panel.is_open(cx));
        assert!(panel.shows(cx, "notifications-panel"));
        // Reading it all takes the count away, and the panel stays.
        panel.click(cx, "notifications-mark-read");
        assert!(!panel.shows(cx, "status-notifications"));
        assert!(panel.is_open(cx));
    }

    /// Each row names the session and says what was said and when, newest
    /// first, and the unread ones say so.
    #[gpui_kit::test]
    fn rows_name_their_session_message_and_time(cx: &mut TestAppContext) {
        let panel = start(history(), 1300., cx);
        panel.open(cx);
        assert_eq!(
            panel.label(cx, row(0)),
            "Split the engine out of main.rs, Arko finished its turn., Just now, unread"
        );
        assert_eq!(
            panel.label(cx, row(1)),
            "Old session, Arko asks “Tabs or spaces?”, 3 min ago, unread"
        );
        assert_eq!(
            panel.label(cx, row(2)),
            "Variable fonts and the wght axis, The prompt cache expires in 5 minutes, 20 min ago, unread"
        );
        let oldest = panel.label(cx, row(3));
        assert!(oldest.starts_with("Split the engine out of main.rs, Arko finished"));
        assert!(!oldest.ends_with("unread"), "{oldest}");
        assert!(!panel.shows(cx, row(4)));
        assert!(!panel.shows(cx, "notifications-empty"));
    }

    /// Clicking a row shows its session and reads what was said of it, and
    /// the rows of other sessions stay unread.
    #[gpui_kit::test]
    fn clicking_a_row_shows_its_session_and_reads_it(cx: &mut TestAppContext) {
        let panel = start(history(), 1300., cx);
        panel.open(cx);
        assert_eq!(open_title(&panel.workspace, cx), None);
        panel.click(cx, row(2));
        assert_eq!(
            open_title(&panel.workspace, cx).as_deref(),
            Some("Variable fonts and the wght axis")
        );
        let unread = cx.read(|cx| {
            notifications::entries(cx)
                .iter()
                .filter(|entry| !entry.read)
                .map(|entry| entry.id)
                .collect::<Vec<_>>()
        });
        assert_eq!(unread, [3, 4]);
        // The panel stays where it was docked, for the next one.
        assert!(panel.is_open(cx));
    }

    /// A notification of a session that is gone keeps its text and has
    /// nothing to open.
    #[gpui_kit::test]
    fn a_row_of_a_deleted_session_keeps_its_text_and_opens_nothing(cx: &mut TestAppContext) {
        let panel = start(history(), 1300., cx);
        panel.open(cx);
        panel.click(cx, row(1));
        assert_eq!(open_title(&panel.workspace, cx), None);
        assert_eq!(cx.read(notifications::unread), 3, "still unread");
        assert_eq!(
            panel.label(cx, row(1)),
            "Old session, Arko asks “Tabs or spaces?”, 3 min ago, unread"
        );
    }

    /// The keyboard goes through the rows, opens one with Enter, and Esc
    /// gives the keyboard back to the composer.
    #[gpui_kit::test]
    fn the_keyboard_moves_through_the_rows_and_opens(cx: &mut TestAppContext) {
        let panel = start(history(), 1300., cx);
        // Mod+Shift+M opens the panel with the keyboard in its list.
        panel.press(cx, "secondary-shift-m");
        assert!(panel.is_open(cx));
        assert!(
            cx.update_window(panel.window, |_, window, cx| {
                panel.panel.read(cx).contains_focus(window, cx)
            })
            .unwrap()
        );
        // On the newest, then down two, to the oldest's neighbour.
        panel.press(cx, "down");
        panel.press(cx, "down");
        panel.press(cx, "enter");
        assert_eq!(
            open_title(&panel.workspace, cx).as_deref(),
            Some("Variable fonts and the wght axis")
        );
        // Esc from the list goes back to the composer.
        panel.press(cx, "secondary-shift-m");
        panel.press(cx, "escape");
        let in_panel = cx
            .update_window(panel.window, |_, window, cx| {
                panel.panel.read(cx).contains_focus(window, cx)
            })
            .unwrap();
        assert!(!in_panel);
    }

    /// Mod+Shift+M comes from a letter key, which the platforms send with
    /// Shift as it reads. A second press closes the panel.
    #[gpui_kit::test]
    fn mod_shift_m_toggles_the_panel(cx: &mut TestAppContext) {
        use gpui_kit::{Keystroke, Modifiers};

        let panel = start(history(), 1300., cx);
        let secondary = if cfg!(target_os = "macos") {
            Modifiers::command()
        } else {
            Modifiers::control()
        };
        let delivered = Keystroke {
            modifiers: Modifiers {
                shift: true,
                ..secondary
            },
            key: "m".into(),
            key_char: None,
        };
        act(cx, panel.window, |window, cx| {
            window.dispatch_keystroke(delivered.clone(), cx);
        });
        assert!(panel.is_open(cx));
        act(cx, panel.window, |window, cx| {
            window.dispatch_keystroke(delivered, cx);
        });
        assert!(!panel.is_open(cx));
    }

    /// Mark all read and Clear all, and the empty list's one line.
    #[gpui_kit::test]
    fn mark_all_read_and_clear_all(cx: &mut TestAppContext) {
        let panel = start(history(), 1300., cx);
        panel.open(cx);
        panel.click(cx, "notifications-mark-read");
        assert_eq!(cx.read(notifications::unread), 0);
        assert!(!panel.label(cx, row(0)).ends_with("unread"));
        panel.click(cx, "notifications-clear");
        assert!(cx.read(|cx| notifications::entries(cx).is_empty()));
        assert!(panel.shows(cx, "notifications-empty"));
        // Nothing to mark or clear: the buttons go with the list.
        assert!(!panel.shows(cx, "notifications-mark-read"));
        assert!(!panel.shows(cx, "notifications-clear"));
    }

    /// A narrow window shows the panel as a sheet over the window, which
    /// Esc closes.
    #[gpui_kit::test]
    fn a_narrow_window_shows_the_panel_as_a_sheet_that_esc_closes(cx: &mut TestAppContext) {
        let panel = start(history(), 600., cx);
        panel.open(cx);
        assert!(panel.is_open(cx));
        assert!(panel.shows(cx, row(0)));
        panel.press(cx, "escape");
        assert!(!panel.is_open(cx));
    }

    /// A docked notifications panel and the workers panel share the place at
    /// the right, so opening one closes the other.
    #[gpui_kit::test]
    fn the_panels_at_the_right_take_turns(cx: &mut TestAppContext) {
        let panel = start(history(), 1300., cx);
        panel.open(cx);
        assert!(panel.is_open(cx));
        act(cx, panel.window, |window, cx| {
            window.dispatch_action(Box::new(crate::menus::ToggleWorkersPanel), cx)
        });
        assert!(!panel.is_open(cx));
        assert!(cx.read(|cx| panel.workspace.read(cx).is_workers_open()));
        panel.open(cx);
        assert!(panel.is_open(cx));
        assert!(!cx.read(|cx| panel.workspace.read(cx).is_workers_open()));
    }

    /// The commands are in the palette and the View menu.
    #[test]
    fn the_commands_are_in_the_palette_and_the_menu() {
        let sections = crate::palette::sections(&[]);
        let notifications = sections
            .iter()
            .find(|section| section.title == "Notifications")
            .expect("a section");
        let labels: Vec<_> = notifications
            .rows
            .iter()
            .map(|row| row.label.to_string())
            .collect();
        assert_eq!(
            labels,
            [
                "Toggle Notifications",
                "Mark All Notifications Read",
                "Clear Notifications"
            ]
        );
    }
}
