//! Arko serves a session's web page so a phone or another computer can
//! follow the session and take part in it. This module asks the engine to
//! share it and shows its links, the best of them as a QR code.

use std::collections::VecDeque;
use std::sync::Arc;

use arko::remote::{Link, LinkKind, Page};
use gpui_kit::component::clipboard::Clipboard;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, StyledExt as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::live::LiveSession;

/// What a session's web page lets others do, wherever the app offers it
/// (the status bar's popover, Settings › Web page).
pub const WHAT_IT_ALLOWS: &str = "Anyone with a session’s link can follow the session and act in it, sending messages and commands, answering questions and stopping turns. Each link holds the session’s key, and New key ends the old links. The page isn’t encrypted, so share it only on networks you trust.";

/// Serves the page on an explicit address, or the network by default.
pub fn share(
    session: &Entity<LiveSession>,
    bind: Option<String>,
    cx: &App,
) -> Task<Result<Page, String>> {
    session
        .read(cx)
        .request(|reply| arko::Request::ShareWebPage { bind, reply }, cx)
}

/// Stops serving the session's page, ending its links.
pub fn stop(session: &Entity<LiveSession>, window: &mut Window, cx: &mut App) {
    let answer = session
        .read(cx)
        .request(|reply| arko::Request::StopWebPage { reply }, cx);
    notify_error(answer, window, cx);
}

/// Gives the session a new key, which ends the page's open sessions.
pub fn new_key(session: &Entity<LiveSession>, window: &mut Window, cx: &mut App) {
    let answer = session
        .read(cx)
        .request(|reply| arko::Request::NewWebPageKey { reply }, cx);
    notify_error(answer, window, cx);
}

fn notify_error<T: Send + 'static>(
    answer: Task<Result<T, String>>,
    window: &mut Window,
    cx: &mut App,
) {
    window
        .spawn(cx, async move |cx| {
            if let Err(why) = answer.await {
                cx.update(|window, cx| {
                    use gpui_kit::component::WindowExt as _;
                    window.push_notification(
                        gpui_kit::component::notification::Notification::error(why),
                        cx,
                    );
                })
                .ok();
            }
        })
        .detach();
}

/// The network a link is on, and its interface: "Local network · wlan0",
/// "Tailscale", "This computer".
pub fn link_label(link: &Link) -> String {
    let kind = match link.kind {
        _ if link.loopback => return "This computer".to_string(),
        LinkKind::Lan => "Local network",
        LinkKind::Tailscale => "Tailscale",
        LinkKind::Vpn => "VPN",
        LinkKind::Loopback => return "This computer".to_string(),
        LinkKind::Other => "Other network",
    };
    match &link.interface {
        Some(interface) => format!("{kind} · {interface}"),
        None => kind.to_string(),
    }
}

/// A link as it is shown: its address, without the key in its fragment.
pub fn shown_url(link: &Link) -> &str {
    link.url.split("/#").next().unwrap_or(&link.url)
}

/// The link the QR code shows: the first one another device can open.
/// `None` when only this computer can open the page.
pub fn qr_link(page: &Page) -> Option<&Link> {
    page.links.iter().find(|link| !link.loopback)
}

/// Who can open a page other devices reach.
pub const ON_THIS_NETWORK: &str = "Devices on this network can open the page with these links.";

/// What the panel says the page allows: for a page other devices reach,
/// who can open it, then what the page lets others do.
pub fn explanation(page: &Page) -> String {
    if qr_link(page).is_some() {
        format!("{ON_THIS_NETWORK} {WHAT_IT_ALLOWS}")
    } else {
        WHAT_IT_ALLOWS.to_string()
    }
}

/// What the panel says when only this computer can open the page.
pub const HOW_TO_REACH_IT: &str = "Only this computer can open these links. For a phone or another computer, share the page on your network in Settings › Web page (Tailscale’s too), or reach it through an SSH tunnel.";

/// A QR code's text and the device pixels of its modules.
type QrKey = (String, u32);

/// The QR codes drawn lately, by the text they hold and the device pixels
/// of a module: a page's links change only with a new key or address.
#[derive(Default)]
struct QrImages(VecDeque<(QrKey, Option<Arc<RenderImage>>)>);

impl Global for QrImages {}

const KEPT_QR_IMAGES: usize = 4;

/// The light margin around a QR code, in modules: the standard's.
const QR_QUIET: i32 = 4;

/// `text` as a QR code's pixels, `module` device pixels a module, dark on
/// light whatever the theme (as readers expect), with its margin.
fn qr_pixels(text: &str, module: u32) -> Option<RenderImage> {
    use qrcodegen::{QrCode, QrCodeEcc};

    let code = QrCode::encode_text(text, QrCodeEcc::Low).ok()?;
    let modules = (code.size() + 2 * QR_QUIET) as u32;
    let side = modules * module;
    let image = image::RgbaImage::from_fn(side, side, |x, y| {
        let (x, y) = (
            (x / module) as i32 - QR_QUIET,
            (y / module) as i32 - QR_QUIET,
        );
        let shade = if code.get_module(x, y) { 0 } else { 255 };
        image::Rgba([shade, shade, shade, 255])
    });
    Some(RenderImage::new(vec![image::Frame::new(image)]))
}

/// How many modules `text`'s QR code is across, its margin included.
fn qr_modules(text: &str) -> Option<u32> {
    let code = qrcodegen::QrCode::encode_text(text, qrcodegen::QrCodeEcc::Low).ok()?;
    Some((code.size() + 2 * QR_QUIET) as u32)
}

/// `text` as a QR code as large as fits `room` (logical pixels) with each
/// module whole device pixels, so its edges stay sharp, and its size in
/// logical pixels. Made once and kept for the next frames. The images let
/// go leave the window too.
pub fn qr_image(
    text: &str,
    room: Pixels,
    window: &mut Window,
    cx: &mut App,
) -> Option<(Arc<RenderImage>, Pixels)> {
    let scale = window.scale_factor();
    let modules = qr_modules(text)?;
    let module = ((f32::from(room) * scale) as u32 / modules).max(1);
    let shown = px((modules * module) as f32 / scale);
    let key = (text.to_string(), module);
    let images = cx.default_global::<QrImages>();
    if let Some((_, image)) = images.0.iter().find(|(kept, _)| *kept == key) {
        return image.clone().map(|image| (image, shown));
    }
    let image = qr_pixels(text, module).map(Arc::new);
    images.0.push_front((key, image.clone()));
    let gone: Vec<_> = images
        .0
        .drain(KEPT_QR_IMAGES.min(images.0.len())..)
        .filter_map(|(_, image)| image)
        .collect();
    for image in gone {
        window.drop_image(image).ok();
    }
    image.map(|image| (image, shown))
}

/// What a click on a button runs.
type OnClick = std::rc::Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// The page's links, each with a copy button, the best one as a QR code,
/// what the page allows, and Stop sharing and New key. Without a session
/// (a preview) the buttons do nothing.
#[derive(IntoElement)]
pub struct WebPagePanel {
    page: Page,
    session: Option<Entity<LiveSession>>,
    explained: bool,
    on_settings: Option<OnClick>,
}

impl WebPagePanel {
    pub fn new(page: &Page) -> Self {
        Self {
            page: page.clone(),
            session: None,
            explained: false,
            on_settings: None,
        }
    }

    /// Says what the page lets others do ([`explanation`]), where nothing
    /// around the panel does.
    pub fn explained(mut self) -> Self {
        self.explained = true;
        self
    }

    /// The session whose page it is: its buttons act on it.
    pub fn session(mut self, session: &Entity<LiveSession>) -> Self {
        self.session = Some(session.clone());
        self
    }

    /// Offers Settings › Web page, through `open`, when only this computer
    /// can open the page.
    pub fn on_settings(
        mut self,
        open: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_settings = Some(std::rc::Rc::new(open));
        self
    }
}

impl RenderOnce for WebPagePanel {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let qr = qr_link(&self.page).map(|link| (link.url.clone(), link_label(link)));
        let room = window.rem_size() * 9.;
        let qr = qr.and_then(|(url, label)| Some((qr_image(&url, room, window, cx)?, label)));
        let theme = cx.theme();
        let mono = theme.mono_font_family.clone();
        // As tall as the window allows: the links and the QR code scroll,
        // the buttons stay.
        let tallest = window.viewport_size().height * 0.8;
        let session = self.session.clone();
        let act = move |command: fn(&Entity<LiveSession>, &mut Window, &mut App)| {
            let session = session.clone();
            move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                if let Some(session) = &session {
                    command(session, window, cx);
                }
            }
        };
        let body = v_flex()
            .id("web-page-body")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .gap_3()
            .child(v_flex().id("web-page-links").gap_1().children(
                self.page.links.iter().enumerate().map(|(ix, link)| {
                    h_flex()
                        .id(("web-page-link", ix))
                        .gap_2()
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .child(div().text_xs().truncate().child(link_label(link)))
                                .child(
                                    div()
                                        .text_xs()
                                        .font_family(mono.clone())
                                        .text_color(theme.muted_foreground)
                                        .truncate()
                                        .child(shown_url(link).to_string()),
                                ),
                        )
                        .child(
                            Clipboard::new(("copy-web-page-link", ix))
                                .value(link.url.clone())
                                .tooltip("Copy the link")
                                .accessibility_label(format!("Copy the {} link", link_label(link))),
                        )
                }),
            ))
            .when(qr.is_none() && !self.page.links.is_empty(), |this| {
                this.child(
                    v_flex()
                        .id("web-page-this-computer")
                        .test_support()
                        .gap_2()
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(HOW_TO_REACH_IT),
                        )
                        .when_some(self.on_settings.clone(), |this, open| {
                            this.child(
                                h_flex().child(
                                    Button::new("web-page-settings")
                                        .outline()
                                        .small()
                                        .label("Web Page Settings")
                                        .on_click(move |event, window, cx| open(event, window, cx)),
                                ),
                            )
                        }),
                )
            })
            .when_some(qr, |this, ((qr, shown), label)| {
                this.child(
                    v_flex()
                        .items_center()
                        .gap_1()
                        .child(
                            div()
                                .id("web-page-qr")
                                .test_support()
                                .size(rems(9.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(img(qr).size(shown).rounded(theme.radius)),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(format!("{label}, for a phone’s camera")),
                        ),
                )
            })
            .when_some(self.page.note.clone(), |this, note| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(crate::backend::sentence(&note)),
                )
            });
        v_flex()
            .id("web-page-panel")
            .test_support()
            .w(rems(20.))
            .max_h(tallest)
            .gap_3()
            .child(
                v_flex()
                    .flex_none()
                    .gap_1()
                    .child(
                        div()
                            .text_sm()
                            .font_medium()
                            .child("The web page is shared"),
                    )
                    .when(self.explained, |this| {
                        this.child(
                            div()
                                .id("web-page-allows")
                                .test_support()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(explanation(&self.page)),
                        )
                    }),
            )
            .child(body)
            .child(
                h_flex()
                    .flex_none()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("web-page-new-key")
                            .ghost()
                            .small()
                            .label("New key")
                            .tooltip("A new key ends these links")
                            .disabled(self.session.is_none())
                            .on_click(act(new_key)),
                    )
                    .child(
                        Button::new("web-page-stop")
                            .outline()
                            .small()
                            .label("Stop sharing")
                            .disabled(self.session.is_none())
                            .on_click(act(stop)),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    // Named, not `super::*`: GPUI's glob brings a `test` macro that would
    // shadow Rust's.
    use super::{KEPT_QR_IMAGES, link_label, qr_image, qr_link, shown_url};
    use arko::remote::{Link, LinkKind, Page};
    use gpui_kit::px;
    use std::sync::Arc;

    fn link(url: &str, interface: Option<&str>, kind: LinkKind, loopback: bool) -> Link {
        Link {
            url: url.into(),
            interface: interface.map(Into::into),
            kind,
            loopback,
        }
    }

    #[test]
    fn links_name_their_network_and_hide_the_key() {
        let lan = link(
            "http://192.168.1.20:8765/#key=secret",
            Some("wlan0"),
            LinkKind::Lan,
            false,
        );
        assert_eq!(link_label(&lan), "Local network · wlan0");
        assert_eq!(shown_url(&lan), "http://192.168.1.20:8765");
        let local = link(
            "http://127.0.0.1:8765/#key=secret",
            None,
            LinkKind::Loopback,
            true,
        );
        assert_eq!(link_label(&local), "This computer");
        let tailnet = link(
            "http://100.64.0.2:8765/#key=k",
            None,
            LinkKind::Tailscale,
            false,
        );
        assert_eq!(link_label(&tailnet), "Tailscale");
    }

    #[test]
    fn the_qr_code_shows_a_link_other_devices_can_open() {
        let local = link("http://127.0.0.1:1/#key=k", None, LinkKind::Loopback, true);
        let lan = link(
            "http://10.0.0.2:1/#key=k",
            Some("eth0"),
            LinkKind::Lan,
            false,
        );
        let page = |links: Vec<Link>| Page {
            addr: "0.0.0.0:1".parse().unwrap(),
            links,
            note: None,
        };
        assert_eq!(qr_link(&page(vec![local.clone(), lan.clone()])), Some(&lan));
        // A phone can't open this computer's own address.
        assert_eq!(qr_link(&page(vec![local.clone()])), None);
        assert_eq!(qr_link(&page(Vec::new())), None);
        // On the network, the panel says who can open the page and that
        // it isn't encrypted.
        let shared = super::explanation(&page(vec![local.clone(), lan.clone()]));
        assert!(shared.starts_with(super::ON_THIS_NETWORK), "{shared}");
        assert!(shared.ends_with(super::WHAT_IT_ALLOWS), "{shared}");
        // Only this computer opens it: what it allows, the same text as
        // Settings › Web page has.
        assert_eq!(
            super::explanation(&page(vec![local])),
            super::WHAT_IT_ALLOWS
        );
    }

    #[gpui_kit::test]
    fn a_qr_code_is_made_once_per_link(cx: &mut gpui_kit::TestAppContext) {
        let window = cx.add_empty_window();
        let link = "http://10.0.0.2:8765/#key=abc";
        let made = |cx: &mut gpui_kit::VisualTestContext, text: &str| {
            cx.update(|window, cx| qr_image(text, px(144.), window, cx).unwrap())
        };
        let (first, shown) = made(window, link);
        let (again, _) = made(window, link);
        assert!(Arc::ptr_eq(&first, &again));
        // Whole device pixels a module, as large as fits the room: drawn at
        // its own size, never stretched.
        let scale = window.update(|window, _| window.scale_factor());
        let width = first.size(0).width.0 as u32;
        assert_eq!(width % super::qr_modules(link).unwrap(), 0);
        assert_eq!(shown, px(width as f32 / scale));
        assert!(shown <= px(144.) && shown > px(144. * 0.8), "{shown:?}");
        for n in 0..KEPT_QR_IMAGES {
            made(window, &format!("http://10.0.0.2:{n}/"));
        }
        let (later, _) = made(window, link);
        assert!(!Arc::ptr_eq(&first, &later), "only the latest few are kept");
    }

    /// Each module is a square of whole pixels, dark on light, inside the
    /// standard's margin.
    #[test]
    fn a_qr_codes_modules_are_whole_pixels() {
        let image = super::qr_pixels("http://192.168.1.12:8765/#key=abc", 4).unwrap();
        let bytes = image.as_bytes(0).unwrap();
        let side = image.size(0).width.0 as usize;
        assert_eq!(side, 37 * 4);
        let shade = |x: usize, y: usize| bytes[(y * side + x) * 4];
        // The margin is light, the finder pattern's corner dark, and its
        // edge is sharp: no grey between.
        assert_eq!(shade(15, 15), 255);
        assert_eq!(shade(16, 16), 0);
        assert_eq!(shade(19, 19), 0);
        assert!(
            bytes
                .chunks(4)
                .all(|pixel| pixel[0] == 0 || pixel[0] == 255)
        );
    }
}
