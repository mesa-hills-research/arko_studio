//! The approval tray above the message box: the requests that wait for the
//! user's approval (a worker profile or a skill to save, an MCP server to
//! add, a worker to start on a model billed per token), a row each, which
//! opens in place to its details. With several, each row has a box, all
//! checked at first. Approve approves the rows checked and rejects the
//! others in the same click, Reject all rejects them all, and what the
//! message box holds goes to the agent with the answer
//! ([`arko::Input::AnswerApprovals`]). The session view sends it (see
//! `SessionView::answer_approvals`).
//!
//! Mod+Shift+A and Mod+Shift+R answer from anywhere in the window, the
//! message box too. Alt+A gives the tray the keyboard, where ↑ and ↓ move
//! between the rows, Space checks one, A checks all or none, → opens a
//! row's details, Enter approves, and Esc closes the details, then gives the
//! keyboard back to the message box.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use arko::approvals::{
    Approval, ApprovalAnswers, ApprovalRequest, Look, approvals_heading, approve_label,
    reject_label,
};
use gpui_kit::base::SelectableText;
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, RoleOverride, Selectable as _, Sizable as _, StyledExt as _,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex,
    scroll::ScrollableElement as _,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

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
        /// Closes the details, or gives the keyboard back to the message
        /// box when none are open (Esc).
        CloseDetails,
    ]
);

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
        }
    }

    /// Shows `requests`, in the order they came. What was set for those
    /// gone goes, and those new join checked.
    pub fn set_requests(&mut self, requests: Vec<ApprovalRequest>, cx: &mut Context<Self>) {
        if requests == self.requests {
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

    /// The answer to every request it shows, with `words` from the message
    /// box: when `approve`, those checked are approved (with their options)
    /// and the others rejected, else all are rejected.
    pub fn answers(&self, approve: bool, words: &str) -> ApprovalAnswers {
        ApprovalAnswers::of(
            &self.requests,
            |id| approve && !self.unselected.contains(id),
            |id| self.options.contains(id),
            words,
        )
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

    fn render_row(
        &self,
        request: &ApprovalRequest,
        focused: bool,
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
                    .text_color(muted)
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
        v_flex()
            .w_full()
            .child(h_flex().w_full().gap_1().children(check).child(line))
            .when(open, |this| {
                this.child(self.render_details(request, several, cx))
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
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let id = request.id.clone();
        let approval = &request.approval;
        let files = match approval {
            Approval::Skill(skill) if !skill.files.is_empty() => Some(skill.files.clone()),
            _ => None,
        };
        let scroll_box = |name: &str, text: String| {
            div()
                .id(part(name, &id))
                .test_support()
                .child(
                    div()
                        .max_h(rems(10.))
                        .overflow_y_scrollbar()
                        // Each box keeps its own place.
                        .id(part(&format!("{name}-scroll"), &id))
                        .p_2()
                        .rounded(theme.radius)
                        .bg(theme.muted)
                        .font_family(theme.mono_font_family.clone())
                        .text_xs()
                        .child(SelectableText::new(
                            part(&format!("{name}-text"), &id),
                            text,
                        )),
                )
                .into_any_element()
        };
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
                Look::Heading => div()
                    .pt_1()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(detail.text)
                    .into_any_element(),
                Look::Warning => div()
                    .text_color(theme.warning)
                    .child(SelectableText::new(line_id, detail.text))
                    .into_any_element(),
                // A skill's files show below, with the one chosen in the box.
                Look::Box | Look::File if files.is_some() => continue,
                Look::Box => scroll_box("approval-box", detail.text),
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
            v_flex()
                .gap_1()
                .children(chips)
                .child(scroll_box("approval-box", text))
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

impl Render for ApprovalTray {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus.is_focused(window);
        let rows: Vec<AnyElement> = self
            .requests
            .clone()
            .iter()
            .map(|request| self.render_row(request, focused, cx))
            .collect();
        let count = self.requests.len();
        let heading = approvals_heading(count);
        let approve = approve_label(self.selected(), count);
        let reject = reject_label(count);
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
            .child(
                h_flex()
                    .flex_none()
                    .gap_2()
                    .justify_end()
                    .child(
                        Button::new("approval-reject")
                            .outline()
                            .small()
                            .accessibility_label(reject)
                            .child(h_flex().gap_1p5().child(reject).children(reject_key))
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.emit(TrayEvent::Answer { approve: false })
                            })),
                    )
                    .child(
                        Button::new("approval-approve")
                            .primary()
                            .small()
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
    use super::code_spans;

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
}
