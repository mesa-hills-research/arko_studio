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

/// What the page lets others do, for Settings › Web page.
pub const WHAT_IT_ALLOWS: &str = "Anyone with a session’s link can follow it from a phone or another computer, and send it messages, answers and commands. Each link holds the session’s key, and New key ends the old links. The page isn’t encrypted: share it on networks you trust.";

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

/// The link the QR code shows: the first one another device can open.
/// `None` when only this computer can open the page.
pub fn qr_link(page: &Page) -> Option<&Link> {
    page.links.iter().find(|link| !link.loopback)
}

/// What one of the page's links lets someone do.
pub const ACT_IN_IT: &str =
    "follow this session and act in it: send messages and commands, answer questions, stop a turn.";

/// Who can open a page other devices reach.
pub const ON_THIS_NETWORK: &str =
    "Devices on this network can open the page with these links, which hold the session’s key.";

/// The page isn't encrypted.
pub const NOT_ENCRYPTED: &str = "The page isn’t encrypted: share it only on networks you trust.";

/// What the panel says the page allows: who can open it, what a link lets
/// them do, and, for a page other devices reach, that it isn't encrypted.
pub fn explanation(page: &Page) -> String {
    if qr_link(page).is_some() {
        format!("{ON_THIS_NETWORK} Anyone with one can {ACT_IN_IT} {NOT_ENCRYPTED}")
    } else {
        format!("Anyone with one of these links can {ACT_IN_IT}")
    }
}

/// What the panel says when only this computer can open the page.
pub const THIS_COMPUTER_ONLY: &str = "Only this computer can open these links. For a phone or another computer, share the page on your network in Settings › Web page (Tailscale’s too), or reach it through an SSH tunnel.";

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
        let qr = qr.and_then(|(url, label)| Some((qr_image(&url, cx)?, label)));
        let theme = cx.theme();
        let mono = theme.mono_font_family.clone();
        // As tall as the window allows: the links and the QR code scroll,
        // the buttons stay.
        let tallest = window.viewport_size().height * 0.8;
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
                                .accessibility_label("Copy the link"),
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
                                .child(THIS_COMPUTER_ONLY),
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
            .when_some(qr, |this, (qr, label)| {
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
        // A phone can't open this computer's own address.
        assert_eq!(qr_link(&page(vec![local.clone()])), None);
        assert_eq!(qr_link(&page(Vec::new())), None);
        // On the network, the panel says who can open the page and that
        // it isn't encrypted.
        let shared = super::explanation(&page(vec![local.clone(), lan.clone()]));
        assert!(shared.starts_with(super::ON_THIS_NETWORK), "{shared}");
        assert!(shared.ends_with(super::NOT_ENCRYPTED), "{shared}");
        let local = super::explanation(&page(vec![local]));
        assert!(local.starts_with("Anyone with one of these links can follow"));
        assert!(!local.contains(super::NOT_ENCRYPTED));
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
