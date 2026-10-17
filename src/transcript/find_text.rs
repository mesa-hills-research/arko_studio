//! Plain text with the current match, which the find scrolls to by the
//! line the match is laid out on.

use std::{cell::Cell, rc::Rc};

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

/// `text`, holding the current match `key` at byte `offset`, in the
/// transcript's `list`.
pub(super) struct FoundText {
    text: StyledText,
    key: Key,
    offset: usize,
    reveal: Rc<Reveal>,
    list: ListState,
}

impl FoundText {
    pub fn new(
        text: StyledText,
        key: Key,
        offset: usize,
        reveal: Rc<Reveal>,
        list: ListState,
    ) -> Self {
        Self {
            text,
            key,
            offset,
            reveal,
            list,
        }
    }

    /// The bounds of the line the match is on, as the text is laid out.
    fn line(&self) -> Option<Bounds<Pixels>> {
        let layout = self.text.layout();
        let position = layout.position_for_index(self.offset)?;
        Some(Bounds::new(position, size(px(1.), layout.line_height())))
    }
}

impl IntoElement for FoundText {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for FoundText {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
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
    ) -> (LayoutId, ()) {
        self.text.request_layout(id, inspector, window, cx)
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.text.prepaint(id, inspector, bounds, state, window, cx);
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        layout: &mut (),
        prepaint: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let line = self.line();
        #[cfg(test)]
        self.reveal.line.set(line);
        // The line's place counts what is above it in the row (tags,
        // images, padding), wrapping and zoom. The list scrolls once it has
        // finished this frame and let go of its state. Scrolling also stops
        // it following the latest row, which a reveal asked of the list in
        // layout wouldn't. Only this landing scrolls, never a later frame.
        if self.reveal.requested.get() == Some(self.key)
            && let Some(line) = line
        {
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
        self.text
            .paint(id, inspector, bounds, layout, prepaint, window, cx);
    }
}
