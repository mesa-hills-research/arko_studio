//! The status bar under the window. It shows only what the user may act on:
//! a turn's state while one runs, workers, unusual states such as outside
//! access, and on the right the cache timer, cost and context. An idle
//! window with nothing unusual has an empty bar.

use gpui_kit::component::status_bar::StatusBar;
use gpui_kit::*;

/// The bar's contents. Each part comes with the engine; until then the bar
/// keeps its place, empty.
pub fn status_bar() -> StatusBar {
    StatusBar::new().h_7().flex_none()
}
