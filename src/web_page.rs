//! A session's web page: arko serves it (the terminal's `/remote`) so that a
//! phone or another computer can follow the session and take part in it.
//! This module turns it on and off with arko's own commands, and shows its
//! links, the best of them as a QR code.

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

/// What the page lets others do, in a sentence.
pub const WHAT_IT_ALLOWS: &str =
    "Anyone with one of these links can follow this session and send it messages.";

/// The command that serves the page on `bind` (`/remote`'s argument: an
/// address, a port or both, empty for the default).
pub fn share_command(bind: &str) -> String {
    let bind = bind.trim();
    if bind.is_empty() {
        "/remote on".to_string()
    } else {
        format!("/remote {bind}")
    }
}

/// The command that stops serving the page.
pub const STOP_COMMAND: &str = "/remote off";

/// The command that gives the session a new key. The page goes on with it,
/// and the old links stop working.
pub const NEW_KEY_COMMAND: &str = "/remote new-key";

/// Serves the session's page on `bind` (see [`share_command`]).
pub fn share(session: &Entity<LiveSession>, bind: &str, cx: &mut App) -> Result<(), String> {
    send(session, share_command(bind), cx)
}

/// Stops serving the session's page: its links stop working.
pub fn stop(session: &Entity<LiveSession>, cx: &mut App) -> Result<(), String> {
    send(session, STOP_COMMAND.to_string(), cx)
}

/// Gives the session a new key, which ends the page's open sessions.
pub fn new_key(session: &Entity<LiveSession>, cx: &mut App) -> Result<(), String> {
    send(session, NEW_KEY_COMMAND.to_string(), cx)
}

fn send(session: &Entity<LiveSession>, command: String, cx: &mut App) -> Result<(), String> {
    let input = arko::Input::Message(arko::Message {
        text: command,
        images: Vec::new(),
    });
    session.update(cx, |session, cx| session.send(input, cx))
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

/// The link the QR code shows: the first one another device can open, else
/// the first.
pub fn qr_link(page: &Page) -> Option<&Link> {
    page.links
        .iter()
        .find(|link| !link.loopback)
        .or_else(|| page.links.first())
}

/// The QR codes drawn lately, by the text they hold: a page's links change
/// only with a new key or address.
#[derive(Default)]
struct QrImages(VecDeque<(String, Option<Arc<Image>>)>);

impl Global for QrImages {}

const KEPT_QR_IMAGES: usize = 4;

/// `text` as a QR code image (dark on light, whatever the theme, as readers
/// expect), made once and kept for the next frames.
pub fn qr_image(text: &str, cx: &mut App) -> Option<Arc<Image>> {
    let images = cx.default_global::<QrImages>();
    if let Some((_, image)) = images.0.iter().find(|(kept, _)| kept == text) {
        return image.clone();
    }
    let image = arko::remote::qr_svg(text)
        .map(|svg| Arc::new(Image::from_bytes(ImageFormat::Svg, svg.into_bytes())));
    images.0.push_front((text.to_string(), image.clone()));
    images.0.truncate(KEPT_QR_IMAGES);
    image
}

/// The page's links, each with a copy button, the best one as a QR code,
/// what the page allows, and Stop sharing and New key. Without a session
/// (a preview) the buttons do nothing.
#[derive(IntoElement)]
pub struct WebPagePanel {
    page: Page,
    session: Option<Entity<LiveSession>>,
}

impl WebPagePanel {
    pub fn new(page: &Page) -> Self {
        Self {
            page: page.clone(),
            session: None,
        }
    }

    /// The session whose page it is: its buttons act on it.
    pub fn session(mut self, session: &Entity<LiveSession>) -> Self {
        self.session = Some(session.clone());
        self
    }
}

impl RenderOnce for WebPagePanel {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let qr = qr_link(&self.page).map(|link| link.url.clone());
        let qr = qr.and_then(|url| qr_image(&url, cx));
        let theme = cx.theme();
        let mono = theme.mono_font_family.clone();
        let _ = window;
        let session = self.session.clone();
        let act = move |command: fn(&Entity<LiveSession>, &mut App) -> Result<(), String>| {
            let session = session.clone();
            move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                if let Some(session) = &session
                    && let Err(why) = command(session, cx)
                {
                    use gpui_kit::component::WindowExt as _;
                    window.push_notification(why, cx);
                }
            }
        };
        v_flex()
            .id("web-page-panel")
            .test_support()
            .w(rems(20.))
            .gap_3()
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_sm()
                            .font_medium()
                            .child("The web page is shared"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(WHAT_IT_ALLOWS),
                    ),
            )
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
                                .accessibility_label("Copy the link"),
                        )
                }),
            ))
            .when_some(qr, |this, qr| {
                this.child(
                    v_flex()
                        .items_center()
                        .gap_1()
                        .child(
                            div()
                                .id("web-page-qr")
                                .test_support()
                                .size(rems(9.))
                                .child(img(qr).size_full().rounded(theme.radius)),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child("The first link, for a phone’s camera"),
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
            })
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("web-page-new-key")
                            .ghost()
                            .small()
                            .label("New key")
                            .tooltip("A new key: these links stop working")
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
    use super::{
        KEPT_QR_IMAGES, NEW_KEY_COMMAND, STOP_COMMAND, link_label, qr_image, qr_link,
        share_command, shown_url,
    };
    use arko::remote::{Link, LinkKind, Page};
    use gpui_kit::ImageFormat;
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
    fn sharing_sends_the_terminals_commands() {
        assert_eq!(share_command(""), "/remote on");
        assert_eq!(share_command(" 0.0.0.0:9000 "), "/remote 0.0.0.0:9000");
        assert_eq!(STOP_COMMAND, "/remote off");
        assert_eq!(NEW_KEY_COMMAND, "/remote new-key");
        for command in [
            share_command("9000"),
            STOP_COMMAND.into(),
            NEW_KEY_COMMAND.into(),
        ] {
            assert!(arko::commands::find(&command).is_some(), "{command}");
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
        assert_eq!(qr_link(&page(vec![local.clone()])), Some(&local));
        assert_eq!(qr_link(&page(Vec::new())), None);
    }

    #[gpui_kit::test]
    fn a_qr_code_is_made_once_per_link(cx: &mut gpui_kit::TestAppContext) {
        let first = cx.update(|cx| qr_image("http://10.0.0.2:8765/#key=abc", cx).unwrap());
        let again = cx.update(|cx| qr_image("http://10.0.0.2:8765/#key=abc", cx).unwrap());
        assert!(Arc::ptr_eq(&first, &again));
        assert_eq!(first.format, ImageFormat::Svg);
        for n in 0..KEPT_QR_IMAGES {
            cx.update(|cx| qr_image(&format!("http://10.0.0.2:{n}/"), cx));
        }
        let later = cx.update(|cx| qr_image("http://10.0.0.2:8765/#key=abc", cx).unwrap());
        assert!(!Arc::ptr_eq(&first, &later), "only the latest few are kept");
    }
}
