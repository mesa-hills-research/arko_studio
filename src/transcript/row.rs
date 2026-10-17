//! How each row of the transcript draws.

use std::rc::Rc;
use std::sync::Arc;

use arko::remote::{CallStatus, Delivery, Item, Kind, Level, Source, WorkerState};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::base::Easing;
use gpui_kit::base::image_cache::LruImageCache;
use gpui_kit::base::text::{TextView, TextViewMotion};
use gpui_kit::component::text::TextBlockContent;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    clipboard::Clipboard,
    diff_view::{Diff, DiffView},
    h_flex,
    shimmer::ShimmerText,
    spinner::Spinner,
    tooltip::Tooltip,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::model::{Gap, RowKey};
use super::tool::{self, Tool};
use super::{AgentFilter, CODE_LINES, TranscriptView, selectable};

/// The reading column's widest.
const COLUMN: f32 = 46.;
/// Lines of a call's output shown before "Show all N lines".
const OUTPUT_LINES: usize = 40;
/// Rows of an edit's diff shown before it scrolls inside.
const DIFF_ROWS: usize = 24;

/// The diff an edit or a write made, from the call's parameters, numbered
/// from the line the edit's result names ("Edited a.rs at lines 40-42").
pub(crate) fn edit_diff(item: &Item) -> Option<Diff> {
    let tool = item.tool.as_deref()?;
    let parameters: serde_json::Value = serde_json::from_str(item.parameters.as_deref()?).ok()?;
    let text = |name: &str| parameters.get(name).and_then(|value| value.as_str());
    let path = text("path")?.to_string();
    match tool {
        "edit_file" => {
            let diff = Diff::between(path, text("old_string")?, text("new_string")?);
            let line = item
                .output
                .as_deref()
                .filter(|output| output.starts_with("Edited "))
                .and_then(|output| output.split_once(" at lines "))
                .and_then(|(_, lines)| {
                    lines
                        .split(|c: char| !c.is_ascii_digit())
                        .next()?
                        .parse::<usize>()
                        .ok()
                });
            Some(match line {
                Some(line) if line > 1 => from_line(diff, line - 1),
                _ => diff,
            })
        }
        "write_file" => Some(Diff::between(path, "", text("content")?)),
        _ => None,
    }
}

/// `diff` with its hunks moved `by` lines down the file.
fn from_line(diff: Diff, by: usize) -> Diff {
    let shift = |range: &str| {
        let (start, rest) = range.split_once(',').unwrap_or((range, ""));
        let start = start.parse::<usize>().map_or(0, |start| start + by);
        if rest.is_empty() {
            start.to_string()
        } else {
            format!("{start},{rest}")
        }
    };
    let mut unified = String::new();
    for line in diff.to_unified().lines() {
        match line
            .strip_prefix("@@ -")
            .and_then(|rest| rest.split_once(" @@"))
            .and_then(|(ranges, tail)| {
                let (old, new) = ranges.split_once(" +")?;
                Some(format!("@@ -{} +{} @@{tail}", shift(old), shift(new)))
            }) {
            Some(header) => unified.push_str(&header),
            None => unified.push_str(line),
        }
        unified.push('\n');
    }
    Diff::parse(&unified)
}

/// Whether a call's line already says all its parameters do: a one-line
/// command, a file read or listed, an edit (whose diff shows instead).
fn parameters_shown_by_line(item: &Item) -> bool {
    match item.tool.as_deref() {
        Some("shell") => item.detail.is_none(),
        Some("read_file" | "list_files" | "edit_file" | "write_file" | "apply_diff") => true,
        _ => false,
    }
}

/// Draws the row at `ix` of `view`'s list.
pub(super) fn render(
    view: &WeakEntity<TranscriptView>,
    ix: usize,
    _: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let Some(view) = view.upgrade() else {
        return div().into_any_element();
    };
    // A block's row gets its view the first time it is drawn.
    if let Some(RowKey::Block { id, block, .. }) = view.read(cx).rows.get(ix).map(|row| row.key) {
        view.update(cx, |this, cx| this.block_view(id, block, cx));
    }
    let this = view.read(cx);
    let Some(row) = this.rows.get(ix).copied() else {
        return div().into_any_element();
    };
    let content = this.row(row.key, &view, cx);
    let gap = match row.gap {
        Gap::None => 0.,
        Gap::Tight => 0.25,
        Gap::Small => 0.5,
        Gap::Section => 1.,
        Gap::Turn => 1.75,
    };
    // The reading column: centred and at most ~46 rem wide, with the side
    // padding inside it, so narrow windows read edge to edge.
    div()
        .id(element_id(row.key))
        .test_support()
        .w_full()
        .pt(rems(gap))
        .flex()
        .justify_center()
        .child(div().w_full().max_w(rems(COLUMN)).px_4().child(content))
        .into_any_element()
}

/// The id of a row's element: stable while the row keeps its key.
pub(super) fn element_id(key: RowKey) -> ElementId {
    let (name, id): (&'static str, u64) = match key {
        RowKey::Separator(id) => ("separator", id),
        RowKey::User(id) => ("user", id),
        RowKey::Refresh(id) => ("refresh", id),
        RowKey::Block {
            id,
            generation,
            block,
            source,
        } => {
            return ElementId::Name(
                format!("block-{id}-{generation}-{block}-{}", u8::from(source)).into(),
            );
        }
        RowKey::Actions(id) => ("actions", id),
        RowKey::Thinking(id) => ("thinking", id),
        RowKey::Call(id) => ("call", id),
        RowKey::Group(id) => ("group", id),
        RowKey::Count(id) => ("count", id),
        RowKey::Notice(id) => ("notice", id),
        RowKey::Output(id) => ("output", id),
        RowKey::Turn(id) => ("turn", id),
        RowKey::Worker(id) => ("worker", id),
    };
    (name, id).into()
}

/// "12 s", "1 m 5 s", "2 h 3 m".
pub(crate) fn duration(ms: i64) -> String {
    let seconds = (ms.max(0) + 500) / 1000;
    match seconds {
        0..60 => format!("{seconds} s"),
        60..3600 => format!("{} m {} s", seconds / 60, seconds % 60),
        _ => format!("{} h {} m", seconds / 3600, seconds % 3600 / 60),
    }
}

/// The muted words on the first line of a user's message.
fn user_tags(item: &Item) -> Vec<String> {
    let mut tags = Vec::new();
    if !item.is_main() {
        tags.push(format!("To {}", item.agent));
    }
    match item.delivery {
        Some(Delivery::Note) => tags.push("Note".into()),
        Some(Delivery::Answer) => tags.push("Answer".into()),
        Some(Delivery::Queued) => tags.push("Queued".into()),
        Some(Delivery::Command) => tags.push("Command".into()),
        Some(Delivery::Turn | Delivery::Refresh) | None => {}
    }
    if item.source == Some(Source::Web) {
        tags.push("From the web page".into());
    }
    tags
}

/// The lines of a fenced or indented code block's code, from its source.
pub(crate) fn code_lines(source: &str) -> usize {
    let fence = |line: &str| {
        let line = line.trim();
        line.starts_with("```") || line.starts_with("~~~")
    };
    let mut lines = source.lines();
    let mut count = 0;
    let mut first = true;
    let mut last_is_fence = false;
    let mut fenced = false;
    for line in lines.by_ref() {
        if first {
            first = false;
            if fence(line) {
                fenced = true;
                continue;
            }
        }
        count += 1;
        last_is_fence = fence(line);
    }
    if fenced && last_is_fence {
        count -= 1;
    }
    count
}

/// Images in Markdown come only from the text itself (`data:` URLs), never
/// from the network.
fn local_images_only(uri: &SharedUri) -> ImageSource {
    if uri.starts_with("data:") {
        ImageSource::from(uri.clone())
    } else {
        ImageSource::Resource(Resource::Embedded("".into()))
    }
}

fn stream_motion(fade: bool) -> TextViewMotion {
    if fade {
        TextViewMotion::default()
            .with_stream_fade(std::time::Duration::from_millis(280))
            .with_stream_fade_stagger(std::time::Duration::from_millis(10))
            .with_stream_fade_easing(Easing::EaseOut)
    } else {
        TextViewMotion::default()
    }
}

impl TranscriptView {
    fn row(&self, key: RowKey, view: &Entity<Self>, cx: &App) -> AnyElement {
        match key {
            RowKey::Separator(id) => self.separator(id, cx),
            RowKey::User(id) => self.user(id, view, cx),
            RowKey::Refresh(_) => self.refresh(cx),
            RowKey::Block {
                id, block, source, ..
            } => self.block(id, block, source, view, cx),
            RowKey::Actions(id) => self.actions(id, view, cx),
            RowKey::Thinking(id) => self.thinking(id, view, cx),
            RowKey::Call(id) => self.call(id, view, cx),
            RowKey::Group(id) => self.group(id, view, cx),
            RowKey::Count(id) => self.count(id, view, cx),
            RowKey::Notice(id) => self.notice(id, cx),
            RowKey::Output(id) => self.output(id, cx),
            RowKey::Turn(id) => self.turn(id, cx),
            RowKey::Worker(id) => self.worker(id, view, cx),
        }
    }

    fn item(&self, id: u64) -> Option<&Item> {
        self.model.entry(id).map(|entry| &entry.item)
    }

    fn separator(&self, id: u64, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let text = self
            .item(id)
            .map(|item| item.text.clone())
            .unwrap_or_default();
        let line = || div().flex_1().h_px().bg(theme.border);
        h_flex()
            .w_full()
            .gap_3()
            .text_xs()
            .text_color(theme.muted_foreground)
            .child(line())
            .child(text)
            .child(line())
            .into_any_element()
    }

    /// The quiet line of a message that only kept the prompt cache warm.
    fn refresh(&self, cx: &App) -> AnyElement {
        let theme = cx.theme();
        div()
            .w_full()
            .px_2()
            .text_xs()
            .text_color(theme.muted_foreground)
            .child("Cache refreshed")
            .into_any_element()
    }

    fn user(&self, id: u64, view: &Entity<Self>, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let Some(item) = self.item(id) else {
            return div().into_any_element();
        };
        let tags = user_tags(item);
        let text: SharedString = item.text.clone().into();
        let group: SharedString = format!("user-{id}").into();
        let copy_text = text.clone();
        let _ = view;
        v_flex()
            .id(("user-message", id))
            .group(group.clone())
            .relative()
            .w_full()
            .gap_1()
            .px_3()
            .py_2()
            .rounded(theme.radius)
            .bg(theme.secondary)
            .text_color(theme.secondary_foreground)
            .when(!tags.is_empty(), |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(tags.join(" · ")),
                )
            })
            .children(self.thumbnails(item, view, cx))
            .child(selectable(("user-text", id), text))
            // Copy shows on hover. TODO: Edit and Branch join it.
            .child(
                h_flex()
                    .absolute()
                    .top_1()
                    .right_1()
                    .opacity(0.)
                    .group_hover(group, |this| this.opacity(1.))
                    .child(
                        Clipboard::new(("copy-user", id))
                            .tooltip("Copy")
                            .value(copy_text),
                    ),
            )
            .into_any_element()
    }

    fn block(
        &self,
        id: u64,
        block: usize,
        source: bool,
        view: &Entity<Self>,
        cx: &App,
    ) -> AnyElement {
        let theme = cx.theme();
        let (Some(item), Some(ui)) = (self.item(id), self.ui.get(&id)) else {
            return div().into_any_element();
        };
        let Some(doc) = ui.doc.as_ref() else {
            return div().into_any_element();
        };
        let Some(row) = doc.blocks.get(block) else {
            return div().into_any_element();
        };
        let thinking = item.kind == Kind::Thinking;
        let reply = item.kind == Kind::Reply;
        let label = (block == 0 && item.kind == Kind::Question).then(|| {
            h_flex()
                .gap_1()
                .pb_1()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(Icon::new(Lucide::MessageCircleQuestionMark).xsmall())
                .child("Question")
        });
        let from = (block == 0 && item.kind == Kind::Message)
            .then(|| item.from.clone())
            .flatten()
            .map(|from| {
                div()
                    .pb_1()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(if from == arko::remote::MAIN_AGENT {
                        "From the lead".to_string()
                    } else {
                        format!("From {from}")
                    })
            });
        let body = if source {
            // As text: the block's Markdown source, in the code font when
            // the reader asked for the source, as typed when Markdown is off.
            let text = row
                .range
                .clone()
                .and_then(|range| doc.text.get(range))
                .unwrap_or_default()
                .trim_end()
                .to_string();
            div()
                .when(ui.source, |this| {
                    this.font_family(theme.mono_font_family.clone())
                        .text_size(theme.mono_font_size)
                })
                .child(selectable(
                    SharedString::from(format!("source-{id}-{block}")),
                    text,
                ))
                .into_any_element()
        } else {
            let code = matches!(row.content, TextBlockContent::CodeBlock(_));
            let lines = if code {
                row.range
                    .clone()
                    .and_then(|range| doc.text.get(range))
                    .map_or(0, code_lines)
            } else {
                0
            };
            let uncapped = ui.uncapped.contains(&block);
            let capped = code && lines > CODE_LINES && !uncapped;
            let Some(row_view) = row.view.as_ref() else {
                return div().into_any_element();
            };
            let text_view = TextView::new(row_view)
                .selectable(true)
                .image_source(local_images_only)
                .motion(stream_motion(self.settings.stream_fade && !thinking))
                .when(code, |this| {
                    this.code_block_actions(|code, _, cx| {
                        let theme = cx.theme();
                        let copy = code.code();
                        h_flex()
                            .gap_1()
                            .pl_1()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .when_some(code.lang(), |this, lang| this.child(lang))
                            .child(Clipboard::new("copy").tooltip("Copy").value(copy))
                    })
                })
                .when(capped, |this| this.code_block_max_lines(CODE_LINES));
            let toggle = (code && lines > CODE_LINES).then(|| {
                let view = view.downgrade();
                Button::new(("show-all", block as u64))
                    .ghost()
                    .xsmall()
                    .label(if uncapped {
                        format!("Show the first {CODE_LINES} lines")
                    } else {
                        format!("Show all {lines} lines")
                    })
                    .on_click(move |_, _, cx| {
                        view.update(cx, |this, cx| this.toggle_code(id, block, cx))
                            .ok();
                    })
            });
            v_flex()
                .w_full()
                .child(text_view)
                .children(toggle.map(|toggle| {
                    h_flex().pt_1().child(
                        div()
                            .id(SharedString::from(format!("show-all-{id}-{block}")))
                            .test_support()
                            .child(toggle),
                    )
                }))
                .into_any_element()
        };
        let hover_view = view.downgrade();
        div()
            .id(SharedString::from(format!("block-body-{id}-{block}")))
            .w_full()
            .when(thinking, |this| {
                this.text_color(theme.muted_foreground).text_sm()
            })
            .when(reply, |this| {
                this.on_hover(move |hovered, _, cx| {
                    hover_view
                        .update(cx, |this, cx| this.set_hovered(id, *hovered, cx))
                        .ok();
                })
            })
            .children(label)
            .children(from)
            .child(body)
            .into_any_element()
    }

    /// The hover actions under a reply: they always take their space, so
    /// showing them moves nothing.
    fn actions(&self, id: u64, view: &Entity<Self>, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let shown = self.hovered == Some(id);
        let source = self.ui.get(&id).is_some_and(|ui| ui.source);
        let button = |name: &'static str, label: &'static str| {
            Button::new((name, id))
                .ghost()
                .xsmall()
                .label(label)
                .text_color(theme.muted_foreground)
        };
        let copy_view = view.downgrade();
        let markdown_view = view.downgrade();
        let source_view = view.downgrade();
        let hover_view = view.downgrade();
        h_flex()
            .id(("reply-actions", id))
            .w_full()
            .gap_1()
            .opacity(if shown { 1. } else { 0. })
            .on_hover(move |hovered, _, cx| {
                hover_view
                    .update(cx, |this, cx| this.set_hovered(id, *hovered, cx))
                    .ok();
            })
            .child(button("copy-reply", "Copy").on_click(move |_, _, cx| {
                copy_view
                    .update(cx, |this, cx| this.copy_entry(id, false, cx))
                    .ok();
            }))
            .child(
                button("copy-markdown", "Copy as Markdown").on_click(move |_, _, cx| {
                    markdown_view
                        .update(cx, |this, cx| this.copy_entry(id, true, cx))
                        .ok();
                }),
            )
            .when(self.settings.render_markdown, |this| {
                this.child(
                    button(
                        "view-source",
                        if source {
                            "View Formatted"
                        } else {
                            "View Source"
                        },
                    )
                    .on_click(move |_, _, cx| {
                        source_view
                            .update(cx, |this, cx| this.toggle_source(id, cx))
                            .ok();
                    }),
                )
            })
            // TODO: Regenerate (the last reply) and Branch.
            .into_any_element()
    }

    /// Whether thinking shows open unless the reader chose.
    pub(super) fn thinking_open_by_default(&self) -> bool {
        self.settings.expand_thinking
            && (self.settings.view == crate::prefs::TranscriptView::Full
                || matches!(self.model.filter(), AgentFilter::Worker(_)))
    }

    fn thinking(&self, id: u64, view: &Entity<Self>, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let Ok(ix) = self.model.position(id) else {
            return div().into_any_element();
        };
        let default = self.thinking_open_by_default();
        let open = self
            .ui
            .get(&id)
            .and_then(|ui| ui.expanded)
            .unwrap_or(default);
        let streaming = self.model.is_streaming(ix);
        let label: AnyElement = if streaming {
            ShimmerText::new("Thinking…")
                .id(("thinking-shimmer", id))
                .into_any_element()
        } else {
            match self.model.thought_for(ix) {
                Some(ms) => format!("Thought for {}", duration(ms)),
                None => "Thought".to_string(),
            }
            .into_any_element()
        };
        let view = view.downgrade();
        h_flex()
            .w_full()
            .child(
                div().id(("thinking-row", id)).test_support().child(
                    Button::new(("thinking-toggle", id))
                        .ghost()
                        .xsmall()
                        .text_color(theme.muted_foreground)
                        .icon(if open {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        })
                        .child(label)
                        .on_click(move |_, _, cx| {
                            view.update(cx, |this, cx| this.toggle(id, default, cx))
                                .ok();
                        }),
                ),
            )
            .into_any_element()
    }

    fn call(&self, id: u64, view: &Entity<Self>, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let Some(entry) = self.model.entry(id) else {
            return div().into_any_element();
        };
        let item = &entry.item;
        let open = self.ui.get(&id).and_then(|ui| ui.expanded).unwrap_or(false);
        let line = item.text.lines().next().unwrap_or_default().to_string();
        let tool = Tool::of(item);
        let trailing: Option<AnyElement> = match item.status {
            Some(CallStatus::Running) => Some(
                h_flex()
                    .flex_none()
                    .gap_1()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(Spinner::new().xsmall())
                    .child(duration((self.clock)() - item.at))
                    .into_any_element(),
            ),
            Some(CallStatus::Failed) => Some(
                h_flex()
                    .flex_none()
                    .min_w_0()
                    .max_w(rems(18.))
                    .gap_1()
                    .text_xs()
                    .text_color(theme.danger)
                    .child(Icon::new(Lucide::CircleX).xsmall())
                    .when_some(item.outcome.clone(), |this, outcome| {
                        this.child(div().min_w_0().truncate().child(outcome))
                    })
                    .into_any_element(),
            ),
            _ => None,
        };
        let header_view = view.downgrade();
        let header = Button::new(("call-toggle", id))
            .ghost()
            .xsmall()
            .w_full()
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .gap_1p5()
                    .text_color(theme.muted_foreground)
                    .when(!item.is_main(), |this| {
                        this.child(
                            div()
                                .flex_none()
                                .text_xs()
                                .font_family(theme.mono_font_family.clone())
                                .child(item.agent.clone()),
                        )
                    })
                    .child(Icon::new(tool.icon()).xsmall().flex_none())
                    .child(div().flex_1().min_w_0().truncate().text_left().child(line))
                    .children(trailing),
            )
            .on_click(move |_, _, cx| {
                header_view
                    .update(cx, |this, cx| this.toggle(id, false, cx))
                    .ok();
            });
        v_flex()
            .w_full()
            .child(div().id(("call-row", id)).test_support().child(header))
            .children(
                self.thumbnails(item, view, cx)
                    .map(|thumbnails| div().pl_6().pt_1().child(thumbnails)),
            )
            .when(open, |this| this.child(self.call_details(id, cx)))
            .into_any_element()
    }

    /// A call's details: its parameters in full, its output, its diff.
    fn call_details(&self, id: u64, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let Some(entry) = self.model.entry(id) else {
            return div().into_any_element();
        };
        let item = &entry.item;
        let mono = |text: String, key: &'static str| {
            div()
                .w_full()
                .px_2()
                .py_1p5()
                .rounded(theme.radius)
                .bg(theme.muted)
                .font_family(theme.mono_font_family.clone())
                .text_size(theme.mono_font_size)
                .child(selectable((key, id), text))
        };
        let caption = |text: String| {
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(text)
        };
        // The first lines of a long text, and a button that shows it all.
        let lines = |text: Arc<str>, key: &'static str, title: String| {
            let total = text.lines().count();
            let shown: String = text
                .lines()
                .take(OUTPUT_LINES)
                .collect::<Vec<_>>()
                .join("\n");
            let title: SharedString = title.into();
            v_flex()
                .w_full()
                .gap_1()
                .child(mono(shown, key))
                .when(total > OUTPUT_LINES, |this| {
                    this.child(
                        Button::new((key, id))
                            .ghost()
                            .xsmall()
                            .label(format!("Show all {total} lines"))
                            .on_click(move |_, window, cx| {
                                open_output(title.clone(), text.clone(), window, cx)
                            }),
                    )
                })
        };
        let line = item.text.trim().to_string();
        let mut details = v_flex().w_full().gap_1p5().pt_1().pl_6();
        // What the call was given: an edit's changes as a diff, else the
        // parameters (or, from before the feed kept them, the line's long
        // part: a multi-line command, a worker's task).
        let diff = self.ui.get(&id).and_then(|ui| ui.diff.clone());
        let has_diff = diff.is_some();
        let parameters: Option<Arc<str>> = if parameters_shown_by_line(item) {
            None
        } else if let Some(parameters) = item.parameters.clone() {
            Some(parameters)
        } else if line.contains('\n') || item.detail.is_some() {
            Some(item.detail.clone().unwrap_or(line.clone()).into())
        } else {
            None
        };
        if let Some(diff) = diff {
            details = details.child(DiffView::new(&diff).max_rows(DIFF_ROWS));
        } else if let Some(parameters) = parameters {
            details = details.child(caption("Parameters".into())).child(lines(
                parameters,
                "parameters",
                format!("Parameters of {line}"),
            ));
        }
        // What came back: a command's output block, or the tool's result.
        match (&entry.result, item.output.clone()) {
            (Some(result), _) => {
                details = details.child(caption(result.text.clone()));
                if let Some(body) = result
                    .whole
                    .clone()
                    .or_else(|| result.detail.clone().map(Arc::from))
                {
                    details = details.child(lines(body, "output", format!("Output of {line}")));
                }
            }
            // An edit's diff says what its result would.
            (None, Some(_)) if has_diff && item.status == Some(CallStatus::Ok) => {}
            (None, Some(output)) if !output.trim().is_empty() => {
                details = details.child(caption("Result".into())).child(lines(
                    output,
                    "output",
                    format!("Result of {line}"),
                ));
            }
            (None, _) => {
                if let Some(outcome) = &item.outcome {
                    details = details.child(caption(outcome.clone()));
                }
            }
        }
        details.into_any_element()
    }

    fn group(&self, id: u64, view: &Entity<Self>, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let members = super::model::run_of(&self.model, id, self.settings.view);
        let entries = self.model.entries();
        let calls: Vec<&Item> = members
            .iter()
            .map(|ix| &entries[*ix].item)
            .filter(|item| item.kind == Kind::Call)
            .collect();
        let running = calls
            .iter()
            .rev()
            .find(|item| item.status == Some(CallStatus::Running));
        let failed = calls
            .iter()
            .filter(|item| item.status == Some(CallStatus::Failed))
            .count();
        let open = self.group_open(id);
        let view = view.downgrade();
        Button::new(("group-toggle", id))
            .ghost()
            .xsmall()
            .w_full()
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .gap_1p5()
                    .text_color(theme.muted_foreground)
                    .child(
                        Icon::new(if open {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        })
                        .xsmall(),
                    )
                    .child(
                        div()
                            .flex_none()
                            .child(tool::summary(calls.iter().copied())),
                    )
                    .when_some(running, |this, running| {
                        this.child(
                            h_flex()
                                .flex_1()
                                .min_w_0()
                                .gap_1()
                                .child(Spinner::new().xsmall())
                                .child(div().min_w_0().truncate().child(
                                    running.text.lines().next().unwrap_or_default().to_string(),
                                )),
                        )
                    })
                    .when(running.is_none(), |this| this.child(div().flex_1()))
                    .when(failed > 0, |this| {
                        this.child(
                            h_flex()
                                .flex_none()
                                .gap_1()
                                .text_color(theme.danger)
                                .child(Icon::new(Lucide::CircleX).xsmall())
                                .child(format!("{failed} failed")),
                        )
                    }),
            )
            .on_click(move |_, _, cx| {
                view.update(cx, |this, cx| this.toggle_group(id, cx)).ok();
            })
            .into_any_element()
    }

    fn count(&self, id: u64, view: &Entity<Self>, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let members = super::model::run_of(&self.model, id, self.settings.view);
        let entries = self.model.entries();
        let calls: Vec<&Item> = members.iter().map(|ix| &entries[*ix].item).collect();
        let failed = calls
            .iter()
            .filter(|item| item.status == Some(CallStatus::Failed))
            .count();
        let running = calls
            .iter()
            .filter(|item| item.status == Some(CallStatus::Running))
            .count();
        let mut text = tool::summary(calls.iter().copied());
        if running > 0 {
            text.push_str(&format!(" · {running} running"));
        }
        let view = view.downgrade();
        Button::new(("count", id))
            .ghost()
            .xsmall()
            .text_color(theme.muted_foreground)
            .tooltip("Show in the full transcript")
            .child(text)
            .when(failed > 0, |this| {
                this.child(
                    h_flex()
                        .gap_1()
                        .text_color(theme.danger)
                        .child(Icon::new(Lucide::CircleX).xsmall())
                        .child(format!("{failed} failed")),
                )
            })
            .on_click(move |_, _, cx| {
                view.update(cx, |this, cx| this.open_run(id, cx)).ok();
            })
            .into_any_element()
    }

    /// The images an item shows (the user's attachments, what a tool
    /// returned), as tiles of a fixed size, so a row's height doesn't change
    /// when an image is decoded. Each is scaled down on a background thread
    /// through the transcript's image cache.
    fn thumbnails(&self, item: &Item, view: &Entity<Self>, cx: &App) -> Option<AnyElement> {
        if item.images.is_empty() {
            return None;
        }
        let theme = cx.theme();
        let fit = size(px(THUMBNAIL.0 * 16.), px(THUMBNAIL.1 * 16.));
        let entry = item.id;
        Some(
            h_flex()
                .flex_wrap()
                .gap_2()
                .children(item.images.iter().map(|image| {
                    let source = self
                        .image(image.id)
                        .map(|bytes| LruImageCache::source(&self.image_cache, bytes, Some(fit)));
                    let view = view.downgrade();
                    let id = image.id;
                    Thumbnail {
                        id: ElementId::Name(format!("image-{entry}-{id}").into()),
                        source,
                        caption: super::image_caption(&image.label, image.bytes).into(),
                        on_open: Rc::new(move |window, cx| {
                            view.update(cx, |this, cx| this.open_image(entry, id, window, cx))
                                .ok();
                        }),
                    }
                    .into_any_element()
                }))
                .text_color(theme.muted_foreground)
                .into_any_element(),
        )
    }

    fn notice(&self, id: u64, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let Some(item) = self.item(id) else {
            return div().into_any_element();
        };
        let (icon, color, text_color) = match item.level {
            Some(Level::Error) => (Lucide::CircleX, theme.danger, theme.foreground),
            Some(Level::Warn) => (Lucide::TriangleAlert, theme.warning, theme.foreground),
            _ => (Lucide::Info, theme.muted_foreground, theme.muted_foreground),
        };
        h_flex()
            .w_full()
            .items_start()
            .gap_1p5()
            .px_2()
            .text_sm()
            .text_color(text_color)
            .when(!item.is_main(), |this| {
                this.child(
                    div()
                        .flex_none()
                        .text_xs()
                        .font_family(theme.mono_font_family.clone())
                        .text_color(theme.muted_foreground)
                        .child(item.agent.clone()),
                )
            })
            .child(
                div()
                    .flex_none()
                    .pt_0p5()
                    .child(Icon::new(icon).xsmall().text_color(color)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(selectable(("notice-text", id), item.text.clone())),
            )
            .into_any_element()
    }

    fn output(&self, id: u64, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let Some(item) = self.item(id) else {
            return div().into_any_element();
        };
        let border = match item.level {
            Some(Level::Error) => theme.danger,
            Some(Level::Warn) => theme.warning,
            _ => theme.border,
        };
        div()
            .w_full()
            .pl_3()
            .border_l_2()
            .border_color(border)
            .font_family(theme.mono_font_family.clone())
            .text_size(theme.mono_font_size)
            .child(selectable(("output-text", id), item.text.clone()))
            .into_any_element()
    }

    fn turn(&self, id: u64, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let Some(item) = self.item(id) else {
            return div().into_any_element();
        };
        div()
            .w_full()
            .px_2()
            .text_xs()
            .text_color(theme.muted_foreground)
            .truncate()
            .child(item.text.clone())
            .into_any_element()
    }

    fn worker(&self, id: u64, view: &Entity<Self>, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let Some(item) = self.item(id) else {
            return div().into_any_element();
        };
        let worker_id = item.worker.clone().unwrap_or_default();
        let summary = self.model.worker(&worker_id);
        let title = summary
            .map(|summary| summary.title.clone())
            .filter(|title| !title.is_empty())
            .unwrap_or_else(|| item.text.clone());
        let now = (self.clock)();
        let state: AnyElement = match summary.map(|summary| summary.state) {
            Some(WorkerState::Running) => {
                let elapsed = summary
                    .and_then(|summary| summary.started_at)
                    .map(|started| duration(now - started))
                    .unwrap_or_default();
                h_flex()
                    .gap_1()
                    .child(Spinner::new().xsmall())
                    .child(elapsed)
                    .into_any_element()
            }
            Some(WorkerState::Queued) => "Queued".into_any_element(),
            Some(WorkerState::Finished) => "Finished".into_any_element(),
            Some(WorkerState::Failed) => div()
                .text_color(theme.danger)
                .child("Failed")
                .into_any_element(),
            Some(WorkerState::Stopped) => "Stopped".into_any_element(),
            None => div().into_any_element(),
        };
        let open: SharedString = worker_id.clone().into();
        let view = view.downgrade();
        let card = Button::new(("worker-card", id))
            .outline()
            .small()
            .w_full()
            .tooltip("Open in the workers panel")
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .gap_2()
                    .child(Icon::new(Lucide::Bot).small().flex_none())
                    .child(
                        div()
                            .flex_none()
                            .font_family(theme.mono_font_family.clone())
                            .text_size(theme.mono_font_size)
                            .child(worker_id),
                    )
                    .child(div().flex_1().min_w_0().truncate().text_left().child(title))
                    .child(
                        div()
                            .flex_none()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(state),
                    ),
            )
            .on_click(move |_, _, cx| {
                let open = open.clone();
                view.update(cx, |this, cx| this.open_worker(open, cx)).ok();
            });
        div()
            .id(("worker-row", id))
            .test_support()
            .child(card)
            .into_any_element()
    }
}

/// Shows a command's whole output in a dialog, as a list that lays out only
/// the lines on screen.
fn open_output(title: SharedString, text: Arc<str>, window: &mut Window, cx: &mut App) {
    let viewer = cx.new(|cx| OutputViewer::new(text, cx));
    window.open_dialog(cx, move |dialog, _, _| {
        let viewer = viewer.clone();
        dialog
            .title(title.clone())
            .w(px(760.))
            .content(move |content, _, _| content.child(viewer.clone()))
    });
}

/// A long output, one row per line.
struct OutputViewer {
    text: Arc<str>,
    lines: Vec<std::ops::Range<usize>>,
    list: ListState,
}

impl OutputViewer {
    fn new(text: Arc<str>, _: &mut Context<Self>) -> Self {
        let mut lines = Vec::new();
        let mut start = 0;
        for line in text.split_inclusive('\n') {
            let end = start + line.len();
            lines.push(start..start + line.trim_end_matches(['\n', '\r']).len());
            start = end;
        }
        let list = ListState::new(lines.len(), ListAlignment::Top, px(400.));
        Self { text, lines, list }
    }
}

impl Render for OutputViewer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let viewer = cx.entity().downgrade();
        let copy: SharedString = self.text.to_string().into();
        v_flex()
            .gap_2()
            .child(
                div()
                    .h(rems(28.))
                    .w_full()
                    .rounded(theme.radius)
                    .bg(theme.muted)
                    .font_family(theme.mono_font_family.clone())
                    .text_size(theme.mono_font_size)
                    .child(
                        list(self.list.clone(), move |ix, _, cx| {
                            let Some(viewer) = viewer.upgrade() else {
                                return div().into_any_element();
                            };
                            let viewer = viewer.read(cx);
                            let line = viewer
                                .lines
                                .get(ix)
                                .and_then(|range| viewer.text.get(range.clone()))
                                .unwrap_or_default()
                                .to_string();
                            div()
                                .px_2()
                                .child(selectable(("output-line", ix as u64), line))
                                .into_any_element()
                        })
                        .size_full(),
                    ),
            )
            .child(
                h_flex().justify_end().child(
                    Clipboard::new("copy-output")
                        .tooltip("Copy all")
                        .value(copy),
                ),
            )
    }
}

/// A thumbnail's size, in rem: up to four fit the reading column.
const THUMBNAIL: (f32, f32) = (10., 7.5);

/// What opening a thumbnail does.
type OpenImage = Rc<dyn Fn(&mut Window, &mut App)>;

/// One image of a row, as a tile that opens the image viewer (a click, or
/// Enter when it has the focus, which it gets back when the viewer closes).
#[derive(IntoElement)]
struct Thumbnail {
    id: ElementId,
    /// `None` when the session's store no longer keeps the image.
    source: Option<ImageSource>,
    caption: SharedString,
    on_open: OpenImage,
}

impl RenderOnce for Thumbnail {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let focus = window
            .use_keyed_state(self.id.clone(), cx, |_, cx| cx.focus_handle())
            .read(cx)
            .clone();
        let theme = cx.theme();
        let caption = self.caption.clone();
        let (click, key) = (self.on_open.clone(), self.on_open);
        let click_focus = focus.clone();
        div()
            .id(self.id)
            .test_support()
            .track_focus(&focus)
            .flex_none()
            .w(rems(THUMBNAIL.0))
            .h(rems(THUMBNAIL.1))
            .flex()
            .items_center()
            .justify_center()
            .overflow_hidden()
            .rounded(theme.radius)
            .border_1()
            .border_color(theme.border)
            .bg(theme.muted)
            .focus(|this| this.border_color(theme.ring))
            .tooltip(move |window, cx| Tooltip::new(caption.clone()).build(window, cx))
            .map(|this| match self.source {
                Some(source) => this.child(img(source).size_full().object_fit(ObjectFit::Contain)),
                None => this
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("Image not kept"),
            })
            .on_click(move |_, window, cx| {
                click_focus.focus(window, cx);
                click(window, cx);
            })
            .on_key_down(move |event, window, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    cx.stop_propagation();
                    key(window, cx);
                }
            })
    }
}

#[cfg(test)]
mod tests {
    // Named, not `super::*`: GPUI's glob brings a `test` macro that would
    // shadow Rust's.
    use super::{code_lines, duration, edit_diff};
    use arko::remote::{Item, Kind};

    #[test]
    fn an_edits_diff_is_numbered_from_the_line_it_changed() {
        let mut item = Item::new(Kind::Call, "edit src/a.rs");
        item.tool = Some("edit_file".into());
        item.parameters =
            Some(r#"{"new_string":"b\nc\n","old_string":"b\n","path":"src/a.rs"}"#.into());
        let diff = edit_diff(&item).unwrap();
        assert!(
            diff.to_unified().contains("@@ -1,1 +1,2 @@"),
            "{}",
            diff.to_unified()
        );
        item.output = Some("Edited src/a.rs at lines 40-41: ...".into());
        let diff = edit_diff(&item).unwrap();
        assert!(
            diff.to_unified().contains("@@ -40,1 +40,2 @@"),
            "{}",
            diff.to_unified()
        );
        assert_eq!((diff.added_lines(), diff.removed_lines()), (1, 0));
    }

    #[test]
    fn durations_read_as_people_say_them() {
        assert_eq!(duration(0), "0 s");
        assert_eq!(duration(12_400), "12 s");
        assert_eq!(duration(65_000), "1 m 5 s");
        assert_eq!(duration(7_380_000), "2 h 3 m");
    }

    #[test]
    fn code_lines_leave_out_the_fences() {
        assert_eq!(code_lines("```rust\nfn a() {}\nfn b() {}\n```\n"), 2);
        assert_eq!(code_lines("```rust\nfn a() {}\nfn b() {}"), 2);
        assert_eq!(code_lines("    indented\n    code\n"), 2);
    }
}
