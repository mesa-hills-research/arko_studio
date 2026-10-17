//! Settings › Web page: whether sessions serve their web page as they
//! open, where they serve it, what it allows, and the pages served now,
//! each with its links, QR code, New key and Stop sharing.

use std::net::IpAddr;

use gpui_kit::component::setting::{SettingField, SettingGroup, SettingItem, SettingPage};
use gpui_kit::component::{ActiveTheme as _, IconName, StyledExt as _, v_flex};
use gpui_kit::*;

use super::fields::{TextKind, TextSetting, is_change};
use crate::live::{LiveSession, LiveSessions};
use crate::prefs::{self, DEFAULT_WEB_PAGE_ADDRESS};
use crate::web_page::WebPagePanel;

/// What the page lets others do, in a paragraph.
pub const WHAT_IT_ALLOWS: &str = "Anyone with a session’s link can follow it from a phone or another computer, and send it messages, answers and commands. Each link holds the session’s key, and New key ends the old links. The page isn’t encrypted: share it on networks you trust.";

/// The page's state: the address and port typed in, and the open sessions
/// it hears from (their pages start and stop).
pub struct WebPageSettings {
    address: TextSetting<String>,
    port: TextSetting<u16>,
    /// The preferences as the page last showed them.
    seen: prefs::Prefs,
    _subscriptions: Vec<Subscription>,
    _sessions: Vec<Subscription>,
}

impl WebPageSettings {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let address = TextSetting::new(
            TextKind {
                get: |prefs| {
                    let address = prefs.web_page.address.trim();
                    (!address.is_empty() && address != DEFAULT_WEB_PAGE_ADDRESS)
                        .then(|| address.to_string())
                },
                set: |prefs, address| {
                    prefs.web_page.address =
                        address.unwrap_or_else(|| DEFAULT_WEB_PAGE_ADDRESS.to_string())
                },
                parse: parse_address,
                show: |address| address.clone(),
            },
            DEFAULT_WEB_PAGE_ADDRESS,
            window,
            cx,
        );
        let port = TextSetting::new(
            TextKind {
                get: |prefs| prefs.web_page.port,
                set: |prefs, port| prefs.web_page.port = port,
                parse: parse_port,
                show: |port| port.to_string(),
            },
            &arko::remote::DEFAULT_PORT.to_string(),
            window,
            cx,
        );
        let subscriptions = vec![
            cx.subscribe(address.input(), |this, _, event, cx| {
                if is_change(event) {
                    this.address.changed(cx);
                    cx.notify();
                }
            }),
            cx.subscribe(port.input(), |this, _, event, cx| {
                if is_change(event) {
                    this.port.changed(cx);
                    cx.notify();
                }
            }),
            cx.observe_global_in::<prefs::PrefsStore>(window, |this, window, cx| {
                if !super::shown_changed(&mut this.seen, cx) {
                    return;
                }
                this.address.follow(window, cx);
                this.port.follow(window, cx);
                cx.notify();
            }),
            cx.observe_global::<LiveSessions>(|this, cx| {
                this.hear_sessions(cx);
                cx.notify();
            }),
        ];
        let mut this = Self {
            address,
            port,
            seen: super::shown_prefs(prefs::get(cx)),
            _subscriptions: subscriptions,
            _sessions: Vec::new(),
        };
        this.hear_sessions(cx);
        this
    }

    /// Hears from each open session, whose page may start or stop.
    fn hear_sessions(&mut self, cx: &mut Context<Self>) {
        self._sessions = LiveSessions::all(cx)
            .iter()
            .map(|session| cx.observe(session, |_, _, cx| cx.notify()))
            .collect();
    }

    /// Why the address and the port typed aren't saved, if they aren't.
    pub fn errors(&self) -> [Option<SharedString>; 2] {
        [self.address.error(), self.port.error()]
    }

    pub fn page(&self, cx: &App) -> SettingPage {
        let defaults = prefs::WebPagePrefs::default();
        let shared: Vec<(Entity<LiveSession>, arko::remote::Page)> = LiveSessions::all(cx)
            .into_iter()
            .filter_map(|session| {
                let page = session.read(cx).reading().page.clone()?;
                Some((session, page))
            })
            .collect();
        let served = if shared.is_empty() {
            vec![
                SettingItem::new(
                    "No session is shared",
                    SettingField::render(|_, _, _| div()),
                )
                .description(
                    "A shared session’s links and QR code show here, and in its status bar.",
                )
                .keywords(["links", "qr code", "new key"]),
            ]
        } else {
            shared
                .into_iter()
                .enumerate()
                .map(|(ix, (session, page))| {
                    let title = session.read(cx).title().clone();
                    SettingItem::render(move |_, _, cx| {
                        v_flex()
                            .id(("shared-session", ix))
                            .gap_2()
                            .child(
                                div()
                                    .text_sm()
                                    .font_medium()
                                    .text_color(cx.theme().foreground)
                                    .child(title.clone()),
                            )
                            .child(WebPagePanel::new(&page).session(&session))
                    })
                    .keywords(["links", "qr code", "new key", "stop sharing"])
                })
                .collect()
        };
        SettingPage::new(super::PAGES[5]).icon(IconName::Globe).groups([
            SettingGroup::new()
                .title("Sharing")
                .description(WHAT_IT_ALLOWS)
                .items([
                    super::switch(
                        "Share every session",
                        "Each session serves its web page as it opens, as arko --remote does. Off, a session serves it once you share it.",
                        |prefs| prefs.web_page.share_by_default,
                        |prefs, on| prefs.web_page.share_by_default = on,
                        defaults.share_by_default,
                    )
                    .keywords(["remote", "share", "phone", "browser"]),
                    self.address
                        .item("Address", "web-page-address")
                        .description(
                            "The IP address to serve on: 127.0.0.1 reaches this computer only, 0.0.0.0 every network it’s on (a VPN or Tailscale too).",
                        )
                        .keywords(["bind", "ip", "network", "remote"]),
                    self.port
                        .item("Port", "web-page-port")
                        .description(format!(
                            "Empty: {}, or any free port when that one is taken.",
                            arko::remote::DEFAULT_PORT
                        ))
                        .keywords(["bind", "remote"]),
                ]),
            SettingGroup::new().title("Shared now").items(served),
        ])
    }
}

/// An IP address as the page is served on: empty for the default (this
/// computer only).
pub fn parse_address(text: &str) -> Result<Option<String>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let ip: IpAddr = text
        .trim_start_matches('[')
        .trim_end_matches(']')
        .parse()
        .map_err(|_| "Give an IP address, such as 127.0.0.1 or 0.0.0.0.".to_string())?;
    let ip = ip.to_string();
    Ok((ip != DEFAULT_WEB_PAGE_ADDRESS).then_some(ip))
}

/// A port: empty for arko's.
pub fn parse_port(text: &str) -> Result<Option<u16>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    text.parse::<u16>()
        .ok()
        .filter(|port| *port > 0)
        .map(Some)
        .ok_or_else(|| "Give a port from 1 to 65535.".to_string())
}

#[cfg(test)]
mod tests {
    use super::{parse_address, parse_port};

    #[test]
    fn addresses_are_ip_addresses() {
        assert_eq!(parse_address(""), Ok(None));
        assert_eq!(parse_address("127.0.0.1"), Ok(None));
        assert_eq!(parse_address(" 0.0.0.0 "), Ok(Some("0.0.0.0".into())));
        assert_eq!(parse_address("[::1]"), Ok(Some("::1".into())));
        assert!(parse_address("my-laptop").is_err());
        assert!(parse_address("0.0.0.0:9000").is_err());
    }

    #[test]
    fn ports_are_from_1_to_65535() {
        assert_eq!(parse_port(""), Ok(None));
        assert_eq!(parse_port("9000"), Ok(Some(9000)));
        assert!(parse_port("0").is_err());
        assert!(parse_port("70000").is_err());
        assert!(parse_port("http").is_err());
    }
}
