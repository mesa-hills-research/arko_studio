//! The context menu of a reply's text. A right-click on it, or the Menu key
//! or Shift+F10 in the text, shows the menu every selectable text has, with
//! Copy and Select All, and View source under them.
//!
//! The kit draws the one menu all selectable text in the app shares, and it
//! doesn't say which text it opened on. So the menu finds the reply by where
//! it opened, among the transcripts of the window.

use gpui_kit::component::native_menu::NativeMenu;
use gpui_kit::component::text::{TextSelectionMenu, TextSelectionMenuTarget};
use gpui_kit::*;

use arko::remote::Kind;

use super::TranscriptView;
use super::model::RowKey;
use super::row::source_label;

/// The transcripts that exist, for the menu to look through.
#[derive(Default)]
struct Transcripts(Vec<WeakEntity<TranscriptView>>);

impl Global for Transcripts {}

/// Notes a new transcript.
pub(super) fn register(view: WeakEntity<TranscriptView>, cx: &mut App) {
    let transcripts = cx.default_global::<Transcripts>();
    transcripts.0.retain(|other| other.upgrade().is_some());
    transcripts.0.push(view);
}

/// Switches a reply between its formatted text and its Markdown source. The
/// menu of the reply's text dispatches it, with the transcript and the reply
/// it opened on, so it works wherever the keyboard focus is.
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = transcript, no_json)]
pub struct ToggleReplySource {
    view: u64,
    reply: u64,
}

/// Sets the menu of selectable text up, once for the app.
pub(super) fn init(cx: &mut App) {
    cx.on_action(|action: &ToggleReplySource, cx| {
        let views = cx
            .try_global::<Transcripts>()
            .map(|transcripts| transcripts.0.clone())
            .unwrap_or_default();
        let view = views
            .iter()
            .filter_map(WeakEntity::upgrade)
            .find(|view| view.entity_id().as_u64() == action.view);
        if let Some(view) = view {
            view.update(cx, |this, cx| this.toggle_source(action.reply, cx));
        }
    });
    TextSelectionMenu::set_builder(build, cx);
}

/// Copy and Select All, and over a reply's text, View source.
fn build(
    menu: NativeMenu,
    target: &TextSelectionMenuTarget,
    window: &mut Window,
    cx: &mut App,
) -> NativeMenu {
    let menu = target.standard_items(menu);
    match reply_at(target.position(), window, cx) {
        Some((view, reply, source)) => menu.separator().menu(
            source_label(source),
            Box::new(ToggleReplySource { view, reply }),
        ),
        None => menu,
    }
}

/// The transcript and the reply under `position` in the window, and whether
/// the reply shows its source.
fn reply_at(position: Point<Pixels>, window: &Window, cx: &App) -> Option<(u64, u64, bool)> {
    let (rem, window) = (window.rem_size(), window.window_handle().window_id());
    let transcripts = cx.try_global::<Transcripts>()?;
    transcripts
        .0
        .iter()
        .filter_map(WeakEntity::upgrade)
        .find_map(|view| {
            let this = view.read(cx);
            if this.window != window {
                return None;
            }
            let (reply, source) = this.reply_at(position, rem)?;
            Some((view.entity_id().as_u64(), reply, source))
        })
}

impl TranscriptView {
    /// The reply whose text is at `position` in the window, if it has a
    /// source view, and whether it shows it.
    fn reply_at(&self, position: Point<Pixels>, rem: Pixels) -> Option<(u64, bool)> {
        let row = self.list.row_at(position, rem)?;
        let RowKey::Block { id, .. } = self.rows.get(row)?.key else {
            return None;
        };
        let reply = self.model.entry(id)?.item.kind == Kind::Reply;
        (reply && self.settings.render_markdown).then(|| (id, self.shows_source(id)))
    }
}
