//! Settings › Web page: whether sessions serve their web page as they
//! open, where they serve it, what it allows, and the pages served now,
//! each with its links, QR code, New key and Stop sharing.

use std::net::IpAddr;

use gpui_kit::component::setting::{SettingField, SettingGroup, SettingItem, SettingPage};
use gpui_kit::component::{ActiveTheme as _, IconName, StyledExt as _, v_flex};
use gpui_kit::*;

use super::fields::{self, TextKind, TextSetting};
use crate::live::{LiveSession, LiveSessions};
use crate::prefs::{self, DEFAULT_WEB_PAGE_ADDRESS, THIS_COMPUTER_ONLY};
use crate::web_page::WebPagePanel;

pub use crate::web_page::WHAT_IT_ALLOWS;

/// A session sharing its web page, as the page lists it: the session, its
/// title and its page.
type Shared = (Entity<LiveSession>, SharedString, arko::remote::Page);

/// The page's state: the address and port typed in, and the open sessions
/// it hears from (their pages start and stop).
pub struct WebPageSettings {
    address: TextSetting<String>,
    port: TextSetting<u16>,
    /// The preferences as the page last showed them.
    seen: prefs::Prefs,
    /// The sessions sharing their page, as last shown, and how many times
    /// they changed.
    shared: Vec<Shared>,
    shared_revision: u64,
    _subscriptions: Vec<Subscription>,
    _sessions: Vec<Subscription>,
}

/// The open sessions sharing their web page now.
fn shared_now(cx: &App) -> Vec<Shared> {
    LiveSessions::all(cx)
        .into_iter()
        .filter_map(|session| {
            let live = session.read(cx);
            let page = live.reading().page.clone()?;
            let title = live.title().clone();
            Some((session, title, page))
        })
        .collect()
}

impl WebPageSettings {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let address = TextSetting::new(
            TextKind {
                get: |prefs| {
                    let address = prefs.web_page.address();
                    (address != DEFAULT_WEB_PAGE_ADDRESS).then(|| address.to_string())
                },
                set: |prefs, address| prefs.web_page.address = address,
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
            fields::subscribe(|page: &mut Self| &mut page.address, address.input(), cx),
            fields::subscribe(|page: &mut Self| &mut page.port, port.input(), cx),
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
                this.sync_shared(cx);
            }),
        ];
        // Settings closed while typing: what's typed is saved.
        cx.on_release(|this, cx| {
            this.address.commit(cx);
            this.port.commit(cx);
        })
        .detach();
        let mut this = Self {
            address,
            port,
            seen: super::shown_prefs(prefs::get(cx)),
            shared: shared_now(cx),
            shared_revision: 0,
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
            .map(|session| cx.observe(session, |this, _, cx| this.sync_shared(cx)))
            .collect();
    }

    /// Draws again when a session's page started or stopped, or changed:
    /// not on the sessions' other changes (a reply streaming in).
    fn sync_shared(&mut self, cx: &mut Context<Self>) {
        let shared = shared_now(cx);
        if shared != self.shared {
            self.shared = shared;
            self.shared_revision += 1;
            cx.notify();
        }
    }

    /// Why the address and the port typed aren't saved, if they aren't.
    pub fn errors(&self) -> [Option<SharedString>; 2] {
        [self.address.error(), self.port.error()]
    }

    pub fn page(&self, cx: &App) -> SettingPage {
        let defaults = prefs::WebPagePrefs::default();
        let shown = &prefs::get(cx).web_page;
        let errors = self.errors();
        let sharing = (
            shown.share_by_default,
            &shown.address,
            shown.port,
            errors.each_ref().map(|error| error.as_deref()),
        );
        let served = if self.shared.is_empty() {
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
            self.shared
                .iter()
                .cloned()
                .enumerate()
                .map(|(ix, (session, title, page))| {
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
        SettingPage::new(super::PAGES[super::WEB_PAGE_PAGE]).icon(IconName::Globe).groups([
            SettingGroup::new()
                .key(super::group_key("Sharing", &sharing))
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
                        .description(format!(
                            "Left empty, the page is served on every network this computer is on ({DEFAULT_WEB_PAGE_ADDRESS}), so a phone or another computer on the same network, a VPN or Tailscale too, can open a session’s page from its link. {THIS_COMPUTER_ONLY} keeps it to this computer.",
                        ))
                        .keywords(["bind", "ip", "network", "remote"]),
                    self.port
                        .item("Port", "web-page-port")
                        .description(format!(
                            "Left empty, it’s {}, or any free port when that one is taken. A port set here goes to the first session that shares its page, and the others take any free port.",
                            arko::remote::DEFAULT_PORT
                        ))
                        .keywords(["bind", "remote"]),
                ]),
            SettingGroup::new()
                .key(super::group_key("Shared now", &self.shared_revision))
                .title("Shared now")
                .items(served),
        ])
    }
}

/// An IP address as the page is served on: empty, or the default's, for
/// the default (every network this computer is on).
pub fn parse_address(text: &str) -> Result<Option<String>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let ip: IpAddr = text
        .trim_start_matches('[')
        .trim_end_matches(']')
        .parse()
        .map_err(|_| "Give an IP address, such as 0.0.0.0 or 127.0.0.1.".to_string())?;
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
        assert_eq!(parse_address(" 0.0.0.0 "), Ok(None), "the default");
        assert_eq!(parse_address("127.0.0.1"), Ok(Some("127.0.0.1".into())));
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
