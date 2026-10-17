//! The title bar: GPUI Kit's `TitleBar` with the menus (Windows and Linux),
//! the open session's title and folder, which open its info, and the
//! layout buttons at the right end.

use gpui_kit::component::{
    ActiveTheme as _, GlobalState, IconName, Selectable as _, Sizable as _, TITLE_BAR_HEIGHT,
    TitleBar,
    button::{Button, ButtonVariants as _},
    h_flex,
    menu::AppMenuBar,
    popover::Popover,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::live::LiveSession;
use crate::session_info::SessionInfo;
use crate::{chrome, menus};

/// The title bar of one window. It re-renders only when its own state
/// changes, never with the transcript or the composer.
pub struct AppTitleBar {
    menu_bar: Entity<AppMenuBar>,
    title: Option<SharedString>,
    folder: Option<SharedString>,
    sidebar_open: bool,
    /// The workers panel's button: shown for a session that has or had
    /// workers, and whether the panel shows.
    workers: Option<bool>,
    /// The session's info, which the title opens.
    info: Entity<SessionInfo>,
    info_open: bool,
    _menus: Subscription,
}

impl AppTitleBar {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let menu_bar = AppMenuBar::new(cx);
        // The menus change with the app's state (Show or Hide Sidebar).
        let menus = cx.observe_global::<menus::MenuState>(|this, cx| {
            this.menu_bar.update(cx, |menu_bar, cx| menu_bar.reload(cx));
        });
        Self {
            menu_bar,
            title: None,
            folder: None,
            sidebar_open: true,
            workers: None,
            info: cx.new(|cx| SessionInfo::new(window, cx)),
            info_open: false,
            _menus: menus,
        }
    }

    /// The session whose title shows: its info opens from it.
    pub fn set_live_session(
        &mut self,
        session: Option<Entity<LiveSession>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if session.is_none() {
            self.set_info_open(false, cx);
        }
        self.info
            .update(cx, |info, cx| info.set_session(session, window, cx));
        cx.notify();
    }

    /// Opens the session's info (Session › Info), or closes it.
    pub fn toggle_info(&mut self, cx: &mut Context<Self>) {
        let has_session = self.info.read(cx).session().is_some();
        self.set_info_open(!self.info_open && has_session, cx);
    }

    fn set_info_open(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.info_open != open {
            self.info.update(cx, |info, cx| info.set_open(open, cx));
            self.info_open = open;
            cx.notify();
        }
    }

    pub fn is_info_open(&self) -> bool {
        self.info_open
    }

    pub fn info(&self) -> &Entity<SessionInfo> {
        &self.info
    }

    /// Shows the open session's title and the name of its folder.
    pub fn set_session(
        &mut self,
        title: Option<SharedString>,
        folder: Option<SharedString>,
        cx: &mut Context<Self>,
    ) {
        if (&self.title, &self.folder) != (&title, &folder) {
            self.title = title;
            self.folder = folder;
            cx.notify();
        }
    }

    pub fn set_sidebar_open(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.sidebar_open != open {
            self.sidebar_open = open;
            cx.notify();
        }
    }

    /// Offers the workers panel's button (`Some`, with whether the panel
    /// shows), or leaves it out.
    pub fn set_workers(&mut self, workers: Option<bool>, cx: &mut Context<Self>) {
        if self.workers != workers {
            self.workers = workers;
            cx.notify();
        }
    }
}

/// The narrowest the session's title gets, in rems, while the window has
/// room for it (or as wide as the title is, when that is less).
const TITLE_FLOOR: f32 = 7.;
/// The narrowest the folder's name gets before the title is cut short.
const FOLDER_FLOOR: f32 = 4.;
/// The bar's inset at its start, before the menus.
const BAR_INSET: Pixels = px(12.);
/// The folder gives up its room this many times faster than the title, so
/// it reaches its floor before the title gets shorter.
const FOLDER_SHRINK: f32 = 10_000.;

/// How the title bar shares its width between the menus and the title.
struct Room {
    /// The most the menus take. They scroll sideways in it.
    menus: Pixels,
    /// The narrowest the title gets.
    title: Pixels,
    /// The narrowest the folder gets, or `None` when there's no room for it.
    folder: Option<Pixels>,
}

impl AppTitleBar {
    /// Shares the bar's width. The menus get what the title and the folder
    /// leave at their floors, and the folder goes first when the window is
    /// too narrow for both floors.
    fn room(&self, window: &Window, cx: &App) -> Room {
        let rem = window.rem_size();
        let width = |text: &str| text_width(text, window, cx);
        let title = self
            .title
            .as_ref()
            .map_or(px(0.), |title| width(title).min(rem * TITLE_FLOOR));
        let folder = self
            .folder
            .as_ref()
            .map(|folder| width(&folder_label(folder)).min(rem * FOLDER_FLOOR));
        let gap = if self.title.is_some() && folder.is_some() {
            rem * 0.375
        } else {
            px(0.)
        };
        // Besides the menus and the words, the bar holds the window's
        // buttons, its inset, the room around the title and inside its
        // button, and the layout buttons with their gaps.
        let layout_buttons = if self.workers.is_some() { 2. } else { 1. };
        let fixed = window_buttons(window, cx) + BAR_INSET + rem * (3.5 + 1.625 * layout_buttons);
        let free = window.viewport_size().width - fixed;
        let floors = title + gap + folder.unwrap_or_default();
        let menus = whole_menus(free - floors, window, cx);
        let words = (free - menus).max(px(0.));
        let title = title.min(words);
        let folder = folder.filter(|folder| title + gap + *folder <= words);
        Room {
            menus,
            title,
            folder,
        }
    }

    /// The title and the folder: plain text, or, for an open session, a
    /// button that opens its info. The folder's name shrinks first, to its
    /// floor, then the title, each with an ellipsis.
    fn render_title(&self, room: &Room, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let words = h_flex()
            .min_w_0()
            .gap_1p5()
            .text_sm()
            .when_some(self.title.clone(), |this, title| {
                this.child(
                    div()
                        .id("session-title-text")
                        .test_support()
                        .min_w(room.title)
                        .truncate()
                        .child(title),
                )
            })
            .when_some(
                self.folder.as_ref().zip(room.folder),
                |this, (folder, floor)| {
                    this.child(
                        div()
                            .id("session-folder-text")
                            .test_support()
                            .min_w(floor)
                            .flex_shrink(FOLDER_SHRINK)
                            .truncate()
                            .text_color(muted)
                            .child(folder_label(folder)),
                    )
                },
            );
        let has_session = self.info.read(cx).session().is_some();
        if !has_session {
            return words.into_any_element();
        }
        let info = self.info.clone();
        let view = cx.entity().downgrade();
        // Clicks on it don't drag the window. The space around it does.
        title_bar_item(
            Popover::new("session-info-popover")
                .anchor(Anchor::TopCenter)
                // The button narrows with the title bar, and the title in it
                // ends with an ellipsis.
                .trigger_style(StyleRefinement::default().min_w_0())
                .open(self.info_open)
                .on_open_change(move |open, _, cx| {
                    let open = *open;
                    view.update(cx, |this, cx| this.set_info_open(open, cx))
                        .ok();
                })
                .trigger(
                    Button::new("session-title-button")
                        .ghost()
                        .small()
                        .max_w_full()
                        .tooltip_with_action("Session info", &menus::SessionInfo, None)
                        .child(words),
                )
                .content(move |_, _, _| info.clone()),
        )
        .min_w_0()
        .into_any_element()
    }
}

/// The width to give the menus out of `room`. That's all of it when every
/// title fits, else what the titles that fit whole take, and at least the
/// first title. A narrow window then shows whole titles, and the menus
/// after them scroll into view. macOS has its menus in its own menu bar.
fn whole_menus(room: Pixels, window: &Window, cx: &App) -> Pixels {
    if cfg!(target_os = "macos") {
        return px(0.);
    }
    let rem = window.rem_size();
    let mut filled = px(0.);
    for (ix, menu) in GlobalState::global(cx).app_menus().iter().enumerate() {
        // As GPUI Kit's menu bar draws a title (a compact small button) and
        // the gap before it, with a pixel to spare.
        let title = (text_width(&menu.name, window, cx) + rem * 0.75 + px(1.)).max(rem * 1.5);
        let gap = if ix == 0 { px(0.) } else { rem * 0.25 };
        if ix > 0 && filled + gap + title > room {
            return filled;
        }
        filled += gap + title;
    }
    filled.max(room)
}

/// The folder as the title bar shows it, after the title.
fn folder_label(folder: &str) -> SharedString {
    format!("· {folder}").into()
}

/// How wide `text` is in the title bar's small text, in whole pixels.
fn text_width(text: &str, window: &Window, cx: &App) -> Pixels {
    let mut style = window.text_style();
    style.font_family = cx.theme().font_family.clone();
    style.font_size = rems(0.875).into();
    let size = style.font_size.to_pixels(window.rem_size());
    window
        .text_system()
        .shape_line(
            SharedString::from(text.to_string()),
            size,
            &[style.to_run(text.len())],
            None,
        )
        .width
        .ceil()
}

/// How wide the window's own buttons are in the bar. These are the caption
/// buttons on Windows, the traffic lights' room on macOS, and on Linux the
/// buttons the desktop leaves to the app.
fn window_buttons(window: &Window, cx: &App) -> Pixels {
    if cfg!(target_os = "windows") {
        return px(46.) * 3.;
    }
    if cfg!(target_os = "macos") {
        return px(80.) - BAR_INSET;
    }
    if !matches!(window.window_decorations(), Decorations::Client { .. }) {
        return px(0.);
    }
    let Some(layout) = cx.button_layout() else {
        return px(0.);
    };
    let controls = window.window_controls();
    let shown = layout
        .left
        .iter()
        .chain(&layout.right)
        .flatten()
        .filter(|button| match button {
            WindowButton::Minimize => controls.minimize,
            WindowButton::Maximize => controls.maximize,
            WindowButton::Close => true,
        })
        .count();
    TITLE_BAR_HEIGHT * shown as f32
}

impl Render for AppTitleBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        crate::testing::rendered("AppTitleBar");
        let mac = cfg!(target_os = "macos");
        let room = self.room(window, cx);
        let title = self.render_title(&room, cx);
        TitleBar::new()
            // GPUI Kit's Linux close button calls `window.remove_window()`, which
            // skips the close check. This sends it through the check instead. Only
            // Linux uses it: macOS and Windows close through the system.
            .on_close_window(|_, window, cx| chrome::close_window(window, cx))
            // The menus, on Windows and Linux (macOS has them in its menu bar).
            .when(!mac, |this| {
                // They scroll sideways, in what the title leaves them.
                this.child(title_bar_item(self.menu_bar.clone()).max_w(room.menus))
            })
            // The session's title, centred on the window. It moves aside for
            // the menus when they leave too little room, and is cut short
            // when it doesn't fit between them and the layout buttons. The
            // bar stays a drag area around it.
            .center(
                h_flex()
                    .id("session-title")
                    .test_support()
                    .min_w_0()
                    .child(title),
            )
            // The layout buttons, together at the right end, before the
            // window buttons on Windows and Linux.
            .suffix(
                title_bar_item(
                    h_flex()
                        .id("layout-buttons")
                        .gap_0p5()
                        .child(layout_button(
                            "toggle-sidebar",
                            IconName::PanelLeft,
                            self.sidebar_open,
                            "Toggle sidebar",
                            Box::new(menus::ToggleSidebar),
                        ))
                        .when_some(self.workers, |this, open| {
                            this.child(layout_button(
                                "toggle-workers",
                                IconName::PanelRight,
                                open,
                                "Toggle workers panel",
                                Box::new(menus::ToggleWorkersPanel),
                            ))
                        }),
                )
                .mr_1(),
            )
    }
}

/// A button that shows or hides a pane, with the pane's shortcut in its
/// tooltip.
fn layout_button(
    id: &'static str,
    icon: IconName,
    selected: bool,
    label: &'static str,
    action: Box<dyn Action>,
) -> Button {
    let tooltip_action = action.boxed_clone();
    Button::new(id)
        .small()
        .ghost()
        .compact()
        .icon(icon)
        .selected(selected)
        .tooltip_with_action(label, tooltip_action.as_ref(), None)
        .accessibility_label(label)
        .on_click(move |_, window, cx| window.dispatch_action(action.boxed_clone(), cx))
}

/// Holds clickable title bar content: buttons, menus, tabs, search fields.
///
/// GPUI Kit's `TitleBar` makes the whole bar a drag area, and its buttons let
/// presses through to it. Without `occlude()` a click turns into a window
/// drag on Windows, and on macOS and Linux the window moves if the pointer
/// moves during the click. `occlude()` keeps the drag area from seeing
/// presses on this element. Text and empty space stay draggable.
pub fn title_bar_item(child: impl IntoElement) -> Div {
    h_flex().occlude().child(child)
}

#[cfg(test)]
mod tests {
    // Named, not `super::*`: GPUI's glob brings a `test` macro that would
    // shadow Rust's.
    use crate::Startup;
    use crate::tests::{act, open, row_of};
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{Bounds, Pixels, TestAppContext, px, size};
    use std::sync::Arc;
    use std::time::SystemTime;

    fn start(cx: &mut TestAppContext) {
        let backend = Arc::new(crate::backend::FakeBackend::sample(
            "/arko",
            SystemTime::now(),
        ));
        cx.update(|cx| crate::init(Startup::for_tests(backend), cx));
    }

    /// The bounds of the title's button, of the menus (none on macOS) and of
    /// the first layout button.
    fn title_bar(
        cx: &mut TestAppContext,
        window: gpui_kit::AnyWindowHandle,
    ) -> [Option<Bounds<Pixels>>; 3] {
        let mut bounds = [None; 3];
        act(cx, window, |window, _| {
            bounds = ["session-title-button", "app-menu-bar", "toggle-sidebar"]
                .map(|id| window.try_find(id).map(|element| element.bounds()));
        });
        bounds
    }

    /// The sample sessions, and three with long names. One has a long title
    /// in a long folder, one a short title in it, and one a long title in a
    /// short folder.
    /// All three had workers, so both layout buttons show.
    fn start_with_long_names(cx: &mut TestAppContext) {
        use crate::backend::{Backend as _, FakeBackend, SessionEntry, SessionId};
        let sample = FakeBackend::sample("/arko", SystemTime::now());
        let mut sessions = sample.sessions().unwrap();
        let first = sessions[0].clone();
        let session = |id: &str, title: &str, folder: &str| SessionEntry {
            id: SessionId(id.into()),
            title: title.into(),
            workspace: Some(std::path::Path::new("/home/ada/src").join(folder)),
            had_workers: true,
            ..first.clone()
        };
        sessions.extend([
            session("20261016-091300-e1e1", LONG_TITLE, LONG_FOLDER),
            session("20261016-091400-e2e2", "Notes", LONG_FOLDER),
            session("20261016-091500-e3e3", OTHER_LONG_TITLE, "arko"),
        ]);
        let backend =
            FakeBackend::new("/arko", sessions).with_default_model(Some("claude-opus-5-5".into()));
        cx.update(|cx| crate::init(Startup::for_tests(Arc::new(backend)), cx));
    }

    const LONG_TITLE: &str = "A long session title that should stay readable";
    const OTHER_LONG_TITLE: &str = "Another long session title in a short folder";
    const LONG_FOLDER: &str = "a-project-folder-with-a-very-long-descriptive-name";

    /// A long title and a long folder share what the menus and the layout
    /// buttons leave. The folder's name shrinks first, to its floor, then
    /// the title, each with an ellipsis, and neither spills out of the
    /// title's button. A short title or folder keeps its whole width.
    #[gpui_kit::test]
    fn the_folder_shrinks_before_the_title_and_neither_spills(cx: &mut TestAppContext) {
        start_with_long_names(cx);
        let (window, workspace) = open(cx, 1300.);
        // Opens the session titled `title`, then measures its title and
        // folder at `width`.
        let show = |title: &str, width: f32, cx: &mut TestAppContext| {
            cx.simulate_window_resize(window, size(px(1300.), px(720.)));
            let row = row_of(&workspace, title, cx);
            act(cx, window, |window, cx| window.click(("row", row), cx));
            cx.simulate_window_resize(window, size(px(width), px(720.)));
            let mut bounds = None;
            act(cx, window, |window, _| {
                let found = |id: &'static str| window.try_find(id).map(|element| element.bounds());
                let [title, folder, button, menus, layout] = [
                    "session-title-text",
                    "session-folder-text",
                    "session-title-button",
                    "app-menu-bar",
                    "toggle-sidebar",
                ]
                .map(found);
                let (title, button, layout) = (title.unwrap(), button.unwrap(), layout.unwrap());
                assert!(window.try_find("toggle-workers").is_some());
                // Nothing spills out of the button, or under the menus or
                // the layout buttons.
                let end = folder.map_or(title.right(), |folder| folder.right());
                assert!(
                    end <= button.right(),
                    "{width} {title:?} {folder:?} {button:?}"
                );
                if let Some(folder) = folder {
                    assert!(
                        title.right() <= folder.left(),
                        "{width} {title:?} {folder:?}"
                    );
                }
                assert!(
                    button.right() <= layout.left(),
                    "{width} {button:?} {layout:?}"
                );
                if let Some(menus) = menus {
                    assert!(
                        menus.right() <= button.left(),
                        "{width} {menus:?} {button:?}"
                    );
                }
                bounds = Some((title, folder, window.rem_size()));
            });
            bounds.unwrap()
        };
        let (title, folder, rem) = show(LONG_TITLE, 1300., cx);
        let (whole_title, whole_folder) = (title.size.width, folder.unwrap().size.width);
        assert!(
            whole_title > rem * 15. && whole_folder > rem * 15.,
            "{title:?} {folder:?}"
        );
        // The folder keeps 4 rems, and the title at least 7.
        for width in [600., 400.] {
            let (title, folder, _) = show(LONG_TITLE, width, cx);
            assert!(title.size.width < whole_title, "{width} {title:?}");
            // Windows' caption buttons leave too little room for the folder
            // at the narrowest.
            if cfg!(target_os = "linux") {
                let folder = folder.expect("room for the folder");
                assert!(
                    (folder.size.width - rem * 4.).abs() < px(1.),
                    "{width} {folder:?}"
                );
                assert!(title.size.width >= rem * 7. - px(1.), "{width} {title:?}");
            }
        }
        // A short title keeps its width, with the folder just after it.
        let (title, folder, _) = show("Notes", 400., cx);
        let folder = folder.unwrap();
        assert!(title.size.width < rem * 4., "{title:?}");
        assert!(
            folder.left() - title.right() < rem * 0.5,
            "{title:?} {folder:?}"
        );
        // A short folder keeps its width, and the title is cut short.
        let (whole, folder, _) = show(OTHER_LONG_TITLE, 1300., cx);
        let whole_folder = folder.unwrap().size.width;
        let (title, folder, _) = show(OTHER_LONG_TITLE, 600., cx);
        assert_eq!(folder.unwrap().size.width, whole_folder);
        assert!(title.size.width < whole.size.width, "{title:?}");
    }

    /// The session's title sits in the middle of the window rather than of
    /// the space between the menus and the layout buttons, and a narrow
    /// window cuts it short between them.
    #[gpui_kit::test]
    fn the_title_is_centred_on_the_window_and_cut_short_between_the_menus_and_buttons(
        cx: &mut TestAppContext,
    ) {
        start(cx);
        let (window, workspace) = open(cx, 1300.);
        let row = row_of(&workspace, "Fix the handover size for workers", cx);
        act(cx, window, |window, cx| window.click(("row", row), cx));

        let [title, menus, button] = title_bar(cx, window);
        let (wide, button) = (title.unwrap(), button.unwrap());
        assert!((wide.center().x - px(650.)).abs() < px(1.), "{wide:?}");
        assert!(wide.right() <= button.left());

        cx.simulate_window_resize(window, size(px(500.), px(720.)));
        let [title, narrow_menus, button] = title_bar(cx, window);
        let (title, button) = (title.unwrap(), button.unwrap());
        assert!(title.size.width < wide.size.width, "{title:?} {wide:?}");
        assert!(title.size.width > px(0.), "{title:?}");
        assert!(title.right() <= button.left(), "{title:?} {button:?}");
        assert_eq!(menus.is_some(), cfg!(not(target_os = "macos")));
        if let Some(menus) = narrow_menus {
            assert!(title.left() >= menus.right(), "{menus:?} {title:?}");
        }
    }
}
