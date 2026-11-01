//! The approval tray above the message box: the requests that wait for the
//! user's approval (a worker profile or a skill to save, an MCP server to
//! add, a worker to start on a model billed per token), a row each, which
//! opens in place to its details. With several, each row has a box, all
//! checked at first. Approve approves the rows checked and rejects the
//! others in the same click, and Reject all rejects them all. While the
//! message box has text, Include message says whether it goes to the agent
//! with the answer ([`arko::Input::AnswerApprovals`]). It starts off for a
//! draft that was there when the newest request came, and on for one
//! written after ([`MessageSwitch`]). Approve and Reject answer only the
//! requests shown for half a second, and their labels count those
//! ([`ShownApprovals`]). The session view sends the answer (see
//! `SessionView::answer_approvals`).
//!
//! Mod+Shift+A and Mod+Shift+R answer from anywhere in the window, the
//! message box too. Alt+A gives the tray the keyboard, where ↑ and ↓ move
//! between the rows, Space checks one, A checks all or none, → opens a
//! row's details, M turns Include message on or off, Enter approves, and Esc
//! closes the details, then gives the
//! keyboard back to the message box. Tab reaches the boxes of text in the
//! details, where ↑ and ↓ scroll a line, Page Up and Page Down a box's
//! height, and Home and End to its start and end.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::rc::Rc;
use std::time::Instant;

use arko::approvals::{
    Approval, ApprovalAnswers, ApprovalRequest, Look, answer_labels, approvals_heading,
};
use gpui_kit::base::SelectableText;
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, RoleOverride, Selectable as _,
    Sizable as _, StyledExt as _, Theme,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex,
    scroll::ScrollableElement as _,
    switch::Switch,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

mod message_switch;
mod shown_approvals;

pub use message_switch::MessageSwitch;
#[cfg(test)]
pub(crate) use shown_approvals::READ_MS;
use shown_approvals::ShownApprovals;

/// The key context of the tray.
pub const CONTEXT: &str = "ApprovalTray";

/// The tallest the tray grows, as a share of the window's height, before
/// its rows scroll.
pub const MAX_HEIGHT: f32 = 0.6;

/// The tallest it grows while the answer panel floats above it, which then
/// has room too.
pub const MAX_HEIGHT_UNDER_PANEL: f32 = 0.3;

gpui_kit::actions!(
    approvals,
    [
        /// Approves the requests checked and rejects the others
        /// (Mod+Shift+A, anywhere in the window).
        ApproveSelected,
        /// Rejects every request (Mod+Shift+R, anywhere in the window).
        RejectAll,
        /// Gives the tray the keyboard, or gives it back (Alt+A).
        FocusApprovals,
        /// Enter in the tray: approves the requests checked.
        Confirm,
        /// Moves to the request above.
        SelectPrevious,
        /// Moves to the request below.
        SelectNext,
        /// Checks the request the keyboard is on, or clears it (Space).
        ToggleSelected,
        /// Checks every request, or none when all are (A).
        SelectAll,
        /// Opens the details of the request the keyboard is on (→).
        OpenDetails,
        /// Turns Include message on or off (M).
        ToggleMessage,
        /// Closes the details, or gives the keyboard back to the message
        /// box when none are open (Esc).
        CloseDetails,
        /// Scrolls a box of text in the details up a line (↑).
        ScrollUp,
        /// Scrolls a box of text down a line (↓).
        ScrollDown,
        /// Scrolls a box of text up by its height (Page Up).
        ScrollPageUp,
        /// Scrolls a box of text down by its height (Page Down).
        ScrollPageDown,
        /// Scrolls a box of text to its start (Home).
        ScrollToStart,
        /// Scrolls a box of text to its end (End).
        ScrollToEnd,
    ]
);

/// The key context of a box of text in a request's details.
pub const BOX_CONTEXT: &str = "ApprovalBox";

/// What the tray asks of the session view.
#[derive(Debug, Clone, PartialEq)]
pub enum TrayEvent {
    /// Answer every request: those checked approved and the others rejected
    /// when `approve`, else all rejected.
    Answer { approve: bool },
    /// Give the keyboard back to the message box.
    Leave,
}

impl EventEmitter<TrayEvent> for ApprovalTray {}

/// The id of a part of the request `id` in the tray. It follows the
/// request, not its place.
pub(crate) fn part(name: &str, id: &str) -> ElementId {
    SharedString::from(format!("{name}-{id}")).into()
}

/// The contrast the tray's warnings need on its fill, WCAG's for body text.
pub(crate) const WARNING_CONTRAST: f32 = 4.5;

/// The color of the tray's warnings, and of a scope of every project. It's
/// the theme's warning color while that reads at [`WARNING_CONTRAST`] on
/// the tray's fill and on a row's hovered fill, else that color darkened
/// (or, on a dark fill, lightened) until it does. Most light themes' amber
/// is too light for a hovered row.
pub(crate) fn warning_color(theme: &Theme) -> Hsla {
    use gpui_kit::component::contrast_ratio;
    let fill = theme.background.blend(theme.popover);
    let surfaces = [fill, fill.blend(theme.secondary_hover)];
    let least = |color: Hsla| {
        surfaces
            .iter()
            .map(|surface| contrast_ratio(surface.blend(color), *surface))
            .fold(f32::INFINITY, f32::min)
    };
    // Black or white, whichever stands out more.
    let (black, white) = (gpui_kit::black(), gpui_kit::white());
    let toward = if least(black) >= least(white) {
        black
    } else {
        white
    };
    (0..=20)
        .map(|step| theme.warning.blend(toward.opacity(step as f32 / 20.)))
        .find(|shade| least(*shade) >= WARNING_CONTRAST)
        .unwrap_or(toward)
}

/// Splits `text` at its backticks into the words and the byte ranges of
/// its code, without the backticks.
pub fn code_spans(text: &str) -> (String, Vec<Range<usize>>) {
    let mut words = String::new();
    let mut code = Vec::new();
    for (at, part) in text.split('`').enumerate() {
        let start = words.len();
        words.push_str(part);
        if at % 2 == 1 && !part.is_empty() {
            code.push(start..words.len());
        }
    }
    (words, code)
}

/// Which of the tray's requests the user checked and which took their
/// option.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TraySelection {
    /// The requests it is of.
    requests: Vec<String>,
    unselected: HashSet<String>,
    options: HashSet<String>,
}

/// Puts `id` in `set` (`on`), or takes it out.
fn set(set: &mut HashSet<String>, id: &str, on: bool) {
    if on {
        set.insert(id.to_string());
    } else {
        set.remove(id);
    }
}

/// Where each row's line was drawn, by request, as the rows draw.
type DrawnLines = Rc<RefCell<Vec<(String, Bounds<Pixels>)>>>;

/// The requests in the tray, with what the user set for each.
pub struct ApprovalTray {
    requests: Vec<ApprovalRequest>,
    /// Those not checked. Each starts checked.
    unselected: HashSet<String>,
    /// Those whose option is on.
    options: HashSet<String>,
    /// The one whose details show.
    open: Option<String>,
    /// The one the keyboard is on.
    cursor: Option<String>,
    /// The keyboard came by a key (Alt+A or the tray's own), so the row it
    /// is on is ringed. A click on a row leaves it plain.
    keyed: bool,
    /// The file a skill's box shows, by its place among the skill's files.
    files: HashMap<String, usize>,
    focus: FocusHandle,
    /// The rows, which scroll to the one the keyboard is on.
    scroll: ScrollHandle,
    /// Whether the message box's text goes with the answer.
    message: MessageSwitch,
    /// When each request came into sight, which answers take only once it
    /// could be read.
    shown: ShownApprovals,
    /// What the tray's clock counts from.
    epoch: Instant,
    /// Where each row's line was drawn in the last frame, by request.
    lines: DrawnLines,
    /// Draws the tray again when its next request can be answered.
    ready_timer: Option<(u64, Task<()>)>,
}

impl ApprovalTray {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            requests: Vec::new(),
            unselected: HashSet::new(),
            options: HashSet::new(),
            open: None,
            cursor: None,
            keyed: false,
            files: HashMap::new(),
            focus: cx.focus_handle(),
            scroll: ScrollHandle::default(),
            message: MessageSwitch::default(),
            shown: ShownApprovals::default(),
            epoch: cx.background_executor().now(),
            lines: Rc::default(),
            ready_timer: None,
        }
    }

    /// Shows `requests`, in the order they came, while the message box has
    /// text or not (`has_text`). `waiting` is every request the session's
    /// status lists, those whose answer is on its way included, which
    /// Include message and the reading guard follow, so an answer that comes
    /// back brings no new request. What was set for those gone goes, and
    /// those new join checked.
    pub fn set_requests(
        &mut self,
        requests: Vec<ApprovalRequest>,
        waiting: HashSet<String>,
        has_text: bool,
        cx: &mut Context<Self>,
    ) {
        let before = self.message.clone();
        self.shown.retain(&waiting);
        self.message.requests(waiting, has_text);
        if requests == self.requests {
            if self.message != before {
                cx.notify();
            }
            return;
        }
        let waits = |id: &String| requests.iter().any(|request| request.id == *id);
        self.unselected.retain(waits);
        self.options.retain(waits);
        self.files.retain(|id, _| waits(id));
        if self.open.as_ref().is_some_and(|id| !waits(id)) {
            self.open = None;
        }
        if self.cursor.as_ref().is_none_or(|id| !waits(id)) {
            self.cursor = requests.first().map(|request| request.id.clone());
        }
        self.requests = requests;
        cx.notify();
    }

    /// The requests it shows.
    pub fn requests(&self) -> &[ApprovalRequest] {
        &self.requests
    }

    /// How many requests are checked.
    pub fn selected(&self) -> usize {
        self.requests
            .iter()
            .filter(|request| !self.unselected.contains(&request.id))
            .count()
    }

    /// Which of its requests are checked and which take their option, to
    /// set them back with [`select`](Self::select).
    pub fn selection(&self) -> TraySelection {
        TraySelection {
            requests: self
                .requests
                .iter()
                .map(|request| request.id.clone())
                .collect(),
            unselected: self.unselected.clone(),
            options: self.options.clone(),
        }
    }

    /// Checks the requests `selection` names, and their options, as it had
    /// them. The others stay as they are.
    pub fn select(&mut self, selection: &TraySelection, cx: &mut Context<Self>) {
        for request in &self.requests {
            let id = &request.id;
            if !selection.requests.contains(id) {
                continue;
            }
            set(&mut self.unselected, id, selection.unselected.contains(id));
            set(&mut self.options, id, selection.options.contains(id));
        }
        cx.notify();
    }

    /// The message box's text changed, and has text or not.
    pub fn edited(&mut self, has_text: bool, cx: &mut Context<Self>) {
        let before = self.message.clone();
        self.message.edited(has_text);
        if self.message != before {
            cx.notify();
        }
    }

    /// Whether the message box's text goes with the answer, and what the
    /// switch was as an answer went, to give back should it come back.
    pub fn message(&self) -> &MessageSwitch {
        &self.message
    }

    /// Words a send took came back to the message box, which has text or
    /// not (`has_text`). Include message is as it was for them, `previous`
    /// as the send took them, unless a request came since.
    pub fn words_restored(
        &mut self,
        previous: &MessageSwitch,
        has_text: bool,
        cx: &mut Context<Self>,
    ) {
        self.message.restored(previous, has_text);
        cx.notify();
    }

    /// The time on the tray's clock, in milliseconds.
    fn now_ms(&self, cx: &App) -> u64 {
        let now = cx.background_executor().now();
        now.saturating_duration_since(self.epoch).as_millis() as u64
    }

    /// The requests an answer takes now, those shown long enough to read.
    pub fn ready(&self, cx: &App) -> Vec<&ApprovalRequest> {
        let now = self.now_ms(cx);
        self.requests
            .iter()
            .filter(|request| self.shown.can_answer(&request.id, now))
            .collect()
    }

    /// The answer to the requests shown long enough to read, with `words`
    /// from the message box if Include message is on. When `approve`, those
    /// checked are approved (with their options) and the others rejected,
    /// else all are rejected. The others keep waiting.
    pub fn answers(&self, approve: bool, words: &str, cx: &App) -> ApprovalAnswers {
        let ready: Vec<ApprovalRequest> = self.ready(cx).into_iter().cloned().collect();
        let words = if !ready.is_empty() && self.message.includes() {
            words
        } else {
            ""
        };
        ApprovalAnswers::of(
            &ready,
            |id| approve && !self.unselected.contains(id),
            |id| self.options.contains(id),
            words,
        )
    }

    /// The tray goes out of sight (the window shows another session). A
    /// request not read yet waits again when it comes back.
    pub fn hidden(&mut self, cx: &mut Context<Self>) {
        let now = self.now_ms(cx);
        self.shown.visible(&HashSet::new(), now);
        self.ready_timer = None;
    }

    /// The rows whose line the last frame drew in sight of the tray's list.
    /// A request out of sight before it could be read waits again.
    fn rows_drawn(&mut self, cx: &mut Context<Self>) {
        let lines = std::mem::take(&mut *self.lines.borrow_mut());
        let list = self.scroll.bounds();
        let in_sight: HashSet<String> = lines
            .into_iter()
            .filter(|(_, line)| {
                line.size.height > px(0.)
                    && line.top() >= list.top() - px(1.)
                    && line.bottom() <= list.bottom() + px(1.)
            })
            .map(|(id, _)| id)
            .collect();
        let now = self.now_ms(cx);
        self.shown.visible(&in_sight, now);
        let Some(ready_at) = self.shown.ready_at_ms(now) else {
            self.ready_timer = None;
            return;
        };
        if self.ready_timer.as_ref().map(|(at, _)| *at) != Some(ready_at) {
            let wait = std::time::Duration::from_millis(ready_at - now);
            let timer = cx.spawn(async move |this, cx| {
                cx.background_executor().timer(wait).await;
                this.update(cx, |_, cx| cx.notify()).ok();
            });
            self.ready_timer = Some((ready_at, timer));
        }
    }

    /// Gives the tray the keyboard, on the row it was last on (Alt+A).
    pub fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.cursor.is_none() {
            self.cursor = self.requests.first().map(|request| request.id.clone());
        }
        self.keyed = true;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// Whether the keyboard is in the tray.
    pub fn contains_focus(&self, window: &Window, cx: &App) -> bool {
        self.focus.contains_focused(window, cx)
    }

    /// The place of the row the keyboard is on.
    fn cursor_index(&self) -> usize {
        self.cursor
            .as_ref()
            .and_then(|id| self.requests.iter().position(|request| request.id == *id))
            .unwrap_or(0)
    }

    fn move_cursor(&mut self, down: bool, window: &mut Window, cx: &mut Context<Self>) {
        let at = self.cursor_index();
        let to = if down {
            (at + 1).min(self.requests.len().saturating_sub(1))
        } else {
            at.saturating_sub(1)
        };
        self.cursor = self.requests.get(to).map(|request| request.id.clone());
        self.keyed = true;
        self.scroll.scroll_to_item(to);
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn toggle_selected(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.unselected.remove(id) {
            self.unselected.insert(id.to_string());
        }
        cx.notify();
    }

    /// Checks every request, or none when all are checked.
    fn select_all(&mut self, cx: &mut Context<Self>) {
        if self.unselected.is_empty() {
            self.unselected = self
                .requests
                .iter()
                .map(|request| request.id.clone())
                .collect();
        } else {
            self.unselected.clear();
        }
        cx.notify();
    }

    fn toggle_open(&mut self, id: &str, cx: &mut Context<Self>) {
        self.open = if self.open.as_deref() == Some(id) {
            None
        } else {
            Some(id.to_string())
        };
        cx.notify();
    }

    fn toggle_option(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.options.remove(id) {
            self.options.insert(id.to_string());
        }
        cx.notify();
    }

    /// Enter and Space belong to the button or the box the keyboard is on
    /// in the tray, and to the tray itself only when it has the keyboard.
    fn tray_has_keyboard(&self, window: &Window, cx: &mut Context<Self>) -> bool {
        let own = self.focus.is_focused(window);
        if !own {
            cx.propagate();
        }
        own
    }

    fn on_confirm(&mut self, _: &Confirm, window: &mut Window, cx: &mut Context<Self>) {
        if self.tray_has_keyboard(window, cx) {
            cx.emit(TrayEvent::Answer { approve: true });
        }
    }

    fn on_toggle(&mut self, _: &ToggleSelected, window: &mut Window, cx: &mut Context<Self>) {
        if self.tray_has_keyboard(window, cx)
            && self.requests.len() > 1
            && let Some(id) = self.cursor.clone()
        {
            self.toggle_selected(&id, cx);
        }
    }

    fn on_select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        if self.requests.len() > 1 {
            self.select_all(cx);
        }
    }

    fn on_toggle_message(&mut self, _: &ToggleMessage, _: &mut Window, cx: &mut Context<Self>) {
        self.message.flip();
        cx.notify();
    }

    fn on_open(&mut self, _: &OpenDetails, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.cursor.clone() {
            self.open = Some(id);
            self.keyed = true;
            window.focus(&self.focus, cx);
            cx.notify();
        }
    }

    fn on_close(&mut self, _: &CloseDetails, window: &mut Window, cx: &mut Context<Self>) {
        if self.open.take().is_some() {
            window.focus(&self.focus, cx);
            cx.notify();
        } else {
            cx.emit(TrayEvent::Leave);
        }
    }

    /// A request's row, with its details when it's open. `warning` is
    /// [`warning_color`].
    fn render_row(
        &self,
        request: &ApprovalRequest,
        focused: bool,
        warning: Hsla,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let approval = &request.approval;
        let id = request.id.clone();
        let several = self.requests.len() > 1;
        let checked = !self.unselected.contains(&id);
        let open = self.open.as_ref() == Some(&id);
        let here = self.cursor.as_ref() == Some(&id);
        let summary = approval.summary();
        let mut label = format!(
            "{}, {}, {}",
            approval.kind_word(),
            approval.name(),
            approval.scope()
        );
        if !summary.is_empty() {
            label.push_str(&format!(", {summary}"));
        }
        let muted = theme.muted_foreground;
        let ring = if here && focused && self.keyed {
            theme.ring
        } else {
            gpui_kit::transparent_black()
        };
        let check = several.then(|| {
            let id = id.clone();
            Checkbox::new(part("approval-check", &id))
                .small()
                .checked(checked)
                // The row says whether it is checked, and Space checks it.
                .role(RoleOverride::Presentational)
                .tab_stop(false)
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_selected(&id, cx)))
        });
        let line = h_flex()
            .id(part("approval-row", &id))
            .test_support()
            .role(Role::ListItem)
            .aria_label(label)
            .aria_expanded(open)
            .when(several, |this| this.aria_selected(checked))
            .when(here, |this| this.aria_active_descendant())
            .flex_1()
            .min_w_0()
            .gap_2()
            .px_1p5()
            .py_1()
            .rounded(theme.radius)
            .border_1()
            .border_color(ring)
            .cursor_pointer()
            .hover(|style| style.bg(theme.secondary_hover))
            .child(
                div()
                    .flex_none()
                    .min_w(rems(3.5))
                    .text_color(muted)
                    .child(approval.kind_word()),
            )
            .child(
                div()
                    .flex_none()
                    .max_w(rems(14.))
                    .truncate()
                    .font_medium()
                    .child(approval.name().to_string()),
            )
            .child(
                div()
                    .flex_none()
                    // Every project stands out.
                    .text_color(if approval.every_project() {
                        warning
                    } else {
                        muted
                    })
                    .child(approval.scope().to_string()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(muted)
                    .child(summary),
            )
            .child(
                Icon::new(if open {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                })
                .xsmall()
                .text_color(muted),
            )
            .on_click(cx.listener({
                let id = id.clone();
                move |this, click: &ClickEvent, window, cx| {
                    this.cursor = Some(id.clone());
                    this.keyed = click.is_keyboard();
                    this.toggle_open(&id, cx);
                    window.focus(&this.focus, cx);
                }
            }));
        let lines = self.lines.clone();
        v_flex()
            .w_full()
            // Where its line is drawn, which says whether it's in sight.
            .on_children_prepainted(move |bounds, _, _| {
                if let Some(line) = bounds.first() {
                    lines.borrow_mut().push((id.clone(), *line));
                }
            })
            .child(h_flex().w_full().gap_1().children(check).child(line))
            .when(open, |this| {
                this.child(self.render_details(request, several, warning, cx))
            })
            .into_any_element()
    }

    /// A request's details, as its row opens to them: what it asks, its
    /// lines, a skill's files with the one chosen in the box, and its
    /// option.
    fn render_details(
        &self,
        request: &ApprovalRequest,
        several: bool,
        warning: Hsla,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let id = request.id.clone();
        let approval = &request.approval;
        let files = match approval {
            Approval::Skill(skill) if !skill.files.is_empty() => Some(skill.files.clone()),
            _ => None,
        };
        // The box of a skill's SKILL.md, which the files below show. An
        // update's changed lines have a box of their own.
        let main_text = files
            .as_ref()
            .and_then(|files| files[0].text.as_deref())
            .map(str::trim_end);
        let text_box = |name: &str, label: &str, text: String| TextBox {
            name: name.to_string(),
            request: id.clone(),
            label: label.to_string().into(),
            text,
        };
        // A box is named by the heading above it.
        let mut heading = String::new();
        let mut lines: Vec<AnyElement> = Vec::new();
        for (n, detail) in approval.details().into_iter().enumerate() {
            let line_id = (SharedString::from(format!("approval-line-{id}")), n);
            lines.push(match detail.look {
                Look::Text => {
                    let (words, code) = code_spans(&detail.text);
                    let style = HighlightStyle {
                        background_color: Some(theme.muted),
                        ..HighlightStyle::default()
                    };
                    SelectableText::new(line_id, words)
                        .highlights(code.into_iter().map(|range| (range, style)))
                        .into_any_element()
                }
                Look::Heading => {
                    heading.clone_from(&detail.text);
                    div()
                        .pt_1()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(detail.text)
                        .into_any_element()
                }
                Look::Warning => div()
                    .text_color(warning)
                    .child(SelectableText::new(line_id, detail.text))
                    .into_any_element(),
                // A skill's files show below, with the one chosen in the box.
                Look::Box if main_text == Some(detail.text.as_str()) => continue,
                Look::File if files.is_some() => continue,
                Look::Box if files.is_some() => {
                    text_box("approval-diff", "Changed lines", detail.text).into_any_element()
                }
                Look::Box => text_box("approval-box", &heading, detail.text).into_any_element(),
                Look::File => SelectableText::new(line_id, detail.text).into_any_element(),
            });
        }
        let files = files.map(|files| {
            let shown = self
                .files
                .get(&id)
                .copied()
                .unwrap_or(0)
                .min(files.len() - 1);
            let text = match &files[shown].text {
                Some(text) => text.trim_end().to_string(),
                None => format!("{} has no text to show.", files[shown].path),
            };
            let chosen_outline = theme.foreground;
            let chips = (files.len() > 1).then(|| {
                h_flex()
                    .flex_wrap()
                    .gap_1()
                    .children(files.iter().enumerate().map(|(n, file)| {
                        let chosen = n == shown;
                        let id = id.clone();
                        Button::new((SharedString::from(format!("approval-file-{id}")), n))
                            .xsmall()
                            .outline()
                            .label(file.path.clone())
                            .selected(chosen)
                            .toggled(chosen)
                            .when(chosen, |this| this.border_color(chosen_outline))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.files.insert(id.clone(), n);
                                cx.notify();
                            }))
                    }))
            });
            let file_box = text_box("approval-box", &files[shown].path, text);
            v_flex().gap_1().children(chips).child(file_box)
        });
        let option = approval.option().map(|option| {
            let id = id.clone();
            Checkbox::new(part("approval-option", &id))
                .small()
                .checked(self.options.contains(&id))
                .label(option)
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_option(&id, cx)))
        });
        v_flex()
            .id(part("approval-details", &request.id))
            .test_support()
            // Under the row's words, past its box.
            .ml(rems(if several { 2. } else { 0.5 }))
            .pt_1()
            .pb_2()
            .gap_1()
            .child(
                div()
                    .id(part("approval-question", &request.id))
                    .test_support()
                    .font_medium()
                    .child(SelectableText::new(
                        part("approval-question-text", &request.id),
                        approval.title(),
                    )),
            )
            .children(lines)
            .children(files)
            .children(option)
            .into_any_element()
    }
}

/// A box of text in a request's details, such as SKILL.md or an update's
/// changed lines, at most 10 rem tall. It scrolls by the wheel and its
/// scrollbar, and once Tab reaches it, by ↑ and ↓ a line at a time, Page Up
/// and Page Down, Home and End.
#[derive(IntoElement)]
struct TextBox {
    /// What its parts are named after, with the request's id.
    name: String,
    request: String,
    /// What a screen reader calls it.
    label: SharedString,
    text: String,
}

impl RenderOnce for TextBox {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let id = part(&self.name, &self.request);
        let name = |of: &str| part(&format!("{}-{of}", self.name), &self.request);
        let focus = window
            .use_keyed_state(id.clone(), cx, |_, cx| cx.focus_handle().tab_stop(true))
            .read(cx)
            .clone();
        // Each box keeps its own place.
        let scroll = window
            .use_keyed_state(name("scroll"), cx, |_, _| ScrollHandle::default())
            .read(cx)
            .clone();
        let wheel = window.use_keyed_state(name("wheel"), cx, |_, _| Wheel::default());
        let rows = self.text.lines().count().max(1);
        let keys = |step: Step| {
            let scroll = scroll.clone();
            move |window: &mut Window| scroll_text(&scroll, step, rows, window)
        };
        let theme = cx.theme();
        let ring = theme.ring;
        div()
            .id(id)
            .test_support()
            .role(Role::ScrollView)
            .aria_label(self.label)
            .key_context(BOX_CONTEXT)
            .track_focus(&focus)
            .on_action({
                let to = keys(Step::Rows(-1.));
                move |_: &ScrollUp, window, _| to(window)
            })
            .on_action({
                let to = keys(Step::Rows(1.));
                move |_: &ScrollDown, window, _| to(window)
            })
            .on_action({
                let to = keys(Step::Pages(-1.));
                move |_: &ScrollPageUp, window, _| to(window)
            })
            .on_action({
                let to = keys(Step::Pages(1.));
                move |_: &ScrollPageDown, window, _| to(window)
            })
            .on_action({
                let to = keys(Step::Start);
                move |_: &ScrollToStart, window, _| to(window)
            })
            .on_action({
                let to = keys(Step::End);
                move |_: &ScrollToEnd, window, _| to(window)
            })
            // The ring sits just outside the fill, so the focus moves
            // nothing, and shows only while the keyboard is in use.
            .m(px(-1.))
            .border_1()
            .border_color(transparent_black())
            .rounded(theme.radius + px(1.))
            .focus_visible(move |this| this.border_color(ring))
            .relative()
            // Before the text, so it sees each wheel event on both sides of
            // the text's own handling.
            .child(wheel_stays(wheel, &scroll))
            .child(
                div()
                    .id(name("area"))
                    .max_h(rems(10.))
                    .overflow_y_scroll()
                    .track_scroll(&scroll)
                    .child(
                        div()
                            .id(name("lines"))
                            .test_support()
                            .p_2()
                            .rounded(theme.radius)
                            .bg(theme.muted)
                            .font_family(theme.mono_font_family.clone())
                            .text_xs()
                            .child(SelectableText::new(name("text"), self.text)),
                    ),
            )
            .vertical_scrollbar(&scroll)
    }
}

/// What [`wheel_stays`] keeps of the wheel events over a box.
#[derive(Default)]
struct Wheel {
    /// The box's place before it handled the event being dispatched.
    before: Option<Point<Pixels>>,
    /// When the last event over the box came.
    last: Option<std::time::Instant>,
    /// The box moved during the current gesture.
    moved: bool,
}

/// A pause between wheel events this long starts a new gesture.
const GESTURE_GAP: std::time::Duration = std::time::Duration::from_millis(500);

/// Keeps the wheel over a box of text from scrolling the tray's rows too.
/// GPUI gives each wheel event to every scroll area under the pointer, so
/// this reads the box's place before the box handles an event, and once the
/// box has moved, the event goes no further. At the box's end the event
/// goes on to the rows, unless the box moved earlier in the same gesture.
///
/// This mirrors the nested scrolling GPUI Kit keeps private to its own
/// scroll areas, a shortcut taken because those keep their scroll handle
/// to themselves and the keys need it. Use the kit's once it gives a
/// handle.
fn wheel_stays(wheel: Entity<Wheel>, scroll: &ScrollHandle) -> impl IntoElement {
    let scroll = scroll.clone();
    canvas(
        |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
        move |_, hitbox, window, _| {
            window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, cx| {
                let now = cx.background_executor().now();
                wheel.update(cx, |wheel, cx| {
                    if !hitbox.should_handle_scroll(window) {
                        // The pointer is elsewhere, so the box's gesture is over.
                        wheel.last = None;
                        wheel.moved = false;
                        return;
                    }
                    match phase {
                        DispatchPhase::Capture => wheel.before = Some(clamped_offset(&scroll)),
                        DispatchPhase::Bubble => {
                            let Some(before) = wheel.before.take() else {
                                return;
                            };
                            let new_gesture = event.touch_phase == TouchPhase::Started
                                || wheel.last.is_none_or(|at| {
                                    now.saturating_duration_since(at) >= GESTURE_GAP
                                });
                            if new_gesture {
                                wheel.moved = false;
                            }
                            wheel.last = Some(now);
                            if clamped_offset(&scroll) != before {
                                wheel.moved = true;
                            }
                            if wheel.moved {
                                cx.stop_propagation();
                            }
                        }
                    }
                });
            });
        },
    )
    .absolute()
    .inset_0()
}

/// The box's place within its range. A div adds a wheel's move to its
/// place at once and keeps it in range when it's next laid out, so a move
/// at the end would otherwise look like one.
fn clamped_offset(scroll: &ScrollHandle) -> Point<Pixels> {
    let most = scroll.max_offset();
    let offset = scroll.offset();
    point(
        offset.x.clamp(-most.x, px(0.)),
        offset.y.clamp(-most.y, px(0.)),
    )
}

/// How far a key scrolls a [`TextBox`].
#[derive(Debug, Clone, Copy)]
enum Step {
    /// Rows of its text, up when negative.
    Rows(f32),
    /// Its height less a row, up when negative.
    Pages(f32),
    Start,
    End,
}

/// Scrolls the text of a [`TextBox`] that `scroll` moves by `step`. A row
/// is its text's height over its `rows` lines.
fn scroll_text(scroll: &ScrollHandle, step: Step, rows: usize, window: &mut Window) {
    let most = scroll.max_offset().y;
    let viewport = scroll.bounds().size.height;
    // The text, without the box's padding.
    let text = most + viewport - window.rem_size();
    let row = (text / rows as f32).max(px(1.));
    let down = -scroll.offset().y;
    let to = match step {
        Step::Rows(rows) => down + row * rows,
        Step::Pages(pages) => down + (viewport - row).max(row) * pages,
        Step::Start => px(0.),
        Step::End => most,
    };
    let offset = scroll.offset();
    scroll.set_offset(point(offset.x, -to.clamp(px(0.), most)));
    window.refresh();
}

impl Render for ApprovalTray {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus.is_focused(window);
        let warning = warning_color(cx.theme());
        let rows: Vec<AnyElement> = self
            .requests
            .clone()
            .iter()
            .map(|request| self.render_row(request, focused, warning, cx))
            .collect();
        let count = self.requests.len();
        let heading = approvals_heading(count);
        let ready = self.ready(cx);
        let checked = ready
            .iter()
            .filter(|request| !self.unselected.contains(&request.id))
            .count();
        let (approve, reject) = answer_labels(checked, ready.len(), count);
        let none_ready = ready.is_empty();
        let includes = self.message.includes();
        let has_text = self.message.has_text();
        let tray = cx.entity().downgrade();
        let theme = cx.theme();
        // A grey cap would sit like a sticker on Approve's fill, so its cap
        // takes the button's text color.
        let on_fill = theme.button_primary_foreground;
        let approve_key = Kbd::global_binding_for_action(&ApproveSelected, window).map(|kbd| {
            kbd.bg(on_fill.opacity(0.15))
                .border_color(on_fill.opacity(0.35))
                .text_color(on_fill.opacity(0.85))
        });
        let reject_key = Kbd::global_binding_for_action(&RejectAll, window);
        v_flex()
            .id("approval-tray")
            .test_support()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .role(Role::Group)
            .aria_label(heading.clone())
            .on_action(cx.listener(Self::on_confirm))
            .on_action(cx.listener(Self::on_toggle))
            .on_action(cx.listener(Self::on_select_all))
            .on_action(cx.listener(Self::on_open))
            .on_action(cx.listener(Self::on_toggle_message))
            .on_action(cx.listener(Self::on_close))
            .on_action(cx.listener(|this, _: &SelectPrevious, window, cx| {
                this.move_cursor(false, window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &SelectNext, window, cx| this.move_cursor(true, window, cx)),
            )
            // The session view sets how tall it grows.
            .flex_shrink(1.)
            .min_h_0()
            .w_full()
            .p_3()
            .gap_2()
            .bg(theme.popover)
            .text_color(theme.popover_foreground)
            .border_1()
            .border_color(theme.border)
            .rounded(theme.radius_lg)
            .shadow_md()
            .child(
                div()
                    .id("approval-heading")
                    .test_support()
                    .role(Role::Heading)
                    .aria_label(heading.clone())
                    .flex_none()
                    .text_sm()
                    .font_medium()
                    .child(heading),
            )
            .child(
                v_flex()
                    .id("approval-rows")
                    .role(Role::List)
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .gap_0p5()
                    .text_sm()
                    .children(rows),
            )
            // Once the rows are drawn, which of them are in sight.
            .child(
                canvas(
                    move |_, _, cx| {
                        tray.update(cx, |tray, cx| tray.rows_drawn(cx)).ok();
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_0(),
            )
            .child(
                h_flex()
                    .flex_none()
                    .gap_2()
                    .justify_end()
                    .when(has_text, |this| {
                        this.child(
                            Switch::new("approval-message")
                                .small()
                                .label("Include message")
                                .checked(includes)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.message.flip();
                                    cx.notify();
                                })),
                        )
                    })
                    .child(
                        Button::new("approval-reject")
                            .outline()
                            .small()
                            .disabled(none_ready)
                            .accessibility_label(reject.clone())
                            .child(h_flex().gap_1p5().child(reject).children(reject_key))
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.emit(TrayEvent::Answer { approve: false })
                            })),
                    )
                    .child(
                        Button::new("approval-approve")
                            .primary()
                            .small()
                            .disabled(none_ready)
                            .accessibility_label(approve.clone())
                            .child(h_flex().gap_1p5().child(approve).children(approve_key))
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.emit(TrayEvent::Answer { approve: true })
                            })),
                    ),
            )
    }
}

/// A request from its JSON, as arko's status gives it (tests).
#[cfg(test)]
pub(crate) fn request(json: serde_json::Value) -> ApprovalRequest {
    serde_json::from_value(json).unwrap()
}

#[cfg(test)]
mod tests {
    use super::{WARNING_CONTRAST, code_spans, warning_color};
    use gpui_kit::component::contrast_ratio;
    use gpui_kit::component::{ActiveTheme as _, Theme};
    use gpui_kit::{Hsla, TestAppContext};

    #[test]
    fn backticks_mark_code() {
        let (words, code) = code_spans("Runs `npx -y server`.");
        assert_eq!(words, "Runs npx -y server.");
        assert_eq!(code.len(), 1);
        assert_eq!(&words[code[0].clone()], "npx -y server");
        assert_eq!(code_spans("Plain."), ("Plain.".to_string(), vec![]));
        assert_eq!(
            code_spans("`a` and `b`"),
            ("a and b".to_string(), vec![0..1, 6..7])
        );
    }

    /// The tray's warnings, and a scope of every project, read on its fill
    /// and on a hovered row in every bundled theme, light and dark, in a
    /// shade of the theme's warning color. One that reads there already
    /// stays as it is.
    #[gpui_kit::test]
    fn warnings_read_on_the_tray_in_every_bundled_theme(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            let mut shaded = [0, 0];
            for config in crate::appearance::parse_bundled() {
                let name = config.name.clone();
                Theme::update(cx, |theme| theme.apply_config(&std::rc::Rc::new(config)));
                let theme = cx.theme().clone();
                let fill = theme.background.blend(theme.popover);
                let least = |color: Hsla| {
                    [fill, fill.blend(theme.secondary_hover)]
                        .iter()
                        .map(|surface| contrast_ratio(surface.blend(color), *surface))
                        .fold(f32::INFINITY, f32::min)
                };
                let warning = warning_color(&theme);
                assert!(
                    least(warning) >= WARNING_CONTRAST,
                    "{name}: {:.2}",
                    least(warning)
                );
                assert!(
                    warning != gpui_kit::black() && warning != gpui_kit::white(),
                    "{name}"
                );
                if least(theme.warning) >= WARNING_CONTRAST {
                    assert_eq!(warning, theme.warning, "{name}");
                } else {
                    shaded[theme.is_dark() as usize] += 1;
                }
            }
            // Light themes and dark ones both needed a shade.
            assert!(shaded.iter().all(|count| *count > 0), "{shaded:?}");
        });
    }
}
