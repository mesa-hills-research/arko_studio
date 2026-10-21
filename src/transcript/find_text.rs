//! Text holding the current match, which the find scrolls to by the line
//! the match is laid out on.

use std::{cell::Cell, rc::Rc};

use gpui_kit::base::SelectableText;
use gpui_kit::*;

use super::Key;

/// The match the find lands on next, until the text holding it is laid out
/// and scrolls the transcript to its line.
#[derive(Default)]
pub(super) struct Reveal {
    requested: Cell<Option<Key>>,
    /// Where the current match's line was last painted.
    #[cfg(test)]
    pub line: Cell<Option<Bounds<Pixels>>>,
}

impl Reveal {
    pub fn set(&self, key: Option<Key>) {
        self.requested.set(key);
    }
}

/// A text element that says where it laid out the line holding a byte.
pub(super) trait Lines: Element {
    /// The bounds of the line holding byte `offset` of the text, as wide as
    /// a caret, once the text is laid out.
    fn line(&self, offset: usize) -> Option<Bounds<Pixels>>;
}

/// The bounds of the line of `layout` holding byte `offset`.
fn line_in(layout: &TextLayout, offset: usize) -> Option<Bounds<Pixels>> {
    let position = layout.position_for_index(offset)?;
    Some(Bounds::new(position, size(px(1.), layout.line_height())))
}

impl Lines for StyledText {
    fn line(&self, offset: usize) -> Option<Bounds<Pixels>> {
        line_in(self.layout(), offset)
    }
}

impl Lines for SelectableText {
    fn line(&self, offset: usize) -> Option<Bounds<Pixels>> {
        line_in(self.layout(), offset)
    }
}

/// `text`, holding the current match `key` at byte `offset`, in the
/// transcript's `list`.
pub(super) struct FoundText<T> {
    text: T,
    key: Key,
    offset: usize,
    reveal: Rc<Reveal>,
    list: ListState,
}

impl<T: Lines> FoundText<T> {
    pub fn new(text: T, key: Key, offset: usize, reveal: Rc<Reveal>, list: ListState) -> Self {
        Self {
            text,
            key,
            offset,
            reveal,
            list,
        }
    }
}

impl<T: Lines> IntoElement for FoundText<T> {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl<T: Lines> Element for FoundText<T> {
    type RequestLayoutState = T::RequestLayoutState;
    type PrepaintState = T::PrepaintState;

    fn id(&self) -> Option<ElementId> {
        self.text.id()
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        self.text.request_layout(id, inspector, window, cx)
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.text.prepaint(id, inspector, bounds, state, window, cx)
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.text
            .paint(id, inspector, bounds, state, prepaint, window, cx);
        let landing = self.reveal.requested.get() == Some(self.key);
        if !landing && !cfg!(test) {
            return;
        }
        let line = self.text.line(self.offset);
        #[cfg(test)]
        self.reveal.line.set(line);
        // The line's place counts what is above it in the row (tags,
        // images, padding), wrapping and zoom. The list scrolls once it has
        // finished this frame and let go of its state. Scrolling also stops
        // it following the latest row, which a reveal asked of the list in
        // layout wouldn't. Only this landing scrolls, never a later frame.
        if landing && let Some(line) = line {
            let (reveal, key, list) = (self.reveal.clone(), self.key, self.list.clone());
            window.defer(cx, move |window, _| {
                if reveal.requested.get() != Some(key) {
                    return;
                }
                reveal.set(None);
                let viewport = list.viewport_bounds();
                let distance = if line.top() < viewport.top() {
                    line.top() - viewport.top()
                } else if line.bottom() > viewport.bottom() {
                    line.bottom() - viewport.bottom()
                } else {
                    px(0.)
                };
                if distance != px(0.) {
                    list.scroll_by(distance);
                    window.refresh();
                }
            });
        }
    }
}
