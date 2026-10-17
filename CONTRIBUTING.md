# Contributing to Arko Studio

Follow the [build instructions](README.md#build-and-install) to clone Arko and arko_framework
beside this repository and install the platform dependencies.

## Tests

```sh
cargo test --locked
cargo clippy --all-targets --locked -- --deny warnings
cargo fmt --check
```

Interaction tests drive windows on GPUI's test platform by element ID, with `TestWindowExt`
from GPUI Kit. Elements a test looks up opt in with `.test_support()`, called before
`.track_focus()`. Tests use `FakeBackend` for session fixtures or Arko's engine with local mock
provider servers.

Changes in behavior should include a test.

### Screenshots

On Linux, `tests/screenshots.rs` renders windows headlessly with Mesa's software Vulkan driver
(lavapipe, from `mesa-vulkan-drivers` on Ubuntu). It compares the result with PNG goldens in
`tests/screenshots`. `tests/fonts.rs` checks font loading and rendering.

A mismatch writes the actual image and a diff to `tests/screenshots/failures`. After an
intentional visual change, inspect those images, then update the goldens.

```sh
UPDATE_GOLDENS=1 cargo test --locked --test screenshots
```

Look at the updated images in light and dark themes. The commit body should name the goldens
that changed and explain why.

## Building against a local GPUI Kit

`Cargo.toml` pins GPUI Kit to a Git commit and selects the matching GPUI fork through
`[patch.crates-io]`. To try a change in a local kit checkout, add this override to the ignored
`.cargo/config.toml`.

```toml
[patch."https://github.com/mesa-hills-research/gpui_kit"]
gpui-kit = { path = "../gpui_kit/crates/kit" }
```

The kit checkout must use the same GPUI commit as the app. When updating the published kit
revision, change both `gpui-kit` entries in `Cargo.toml`, the GPUI patches and the screenshot
harness revision to match the kit, then regenerate `Cargo.lock` without the local override.

## Source layout

| Path | What it holds |
|---|---|
| `src/main.rs`, `src/lib.rs` | Logging, startup and app globals |
| `src/backend.rs`, `src/backend/` | Session storage, credentials and defaults |
| `src/live.rs` | Running sessions and engine events |
| `src/workspace.rs` | Window panes, sheets, resizing and actions |
| `src/sidebar.rs` | Session grouping, filtering, renaming and deleting |
| `src/session_view.rs`, `src/composer.rs` | Session controls, message input and attachments |
| `src/transcript/` | Transcript rows, selection, navigation and find |
| `src/status_bar.rs`, `src/workers_panel.rs` | Session figures and worker transcripts |
| `src/settings_page.rs`, `src/settings_page/` | Settings pages and fields |
| `src/theme_picker.rs`, `src/appearance.rs` | Themes, previews, fonts, zoom and motion |
| `src/prefs.rs`, `src/drafts.rs` | Saved preferences and message drafts |
| `src/commands.rs`, `src/menus.rs` | Commands, keyboard shortcuts and menus |
| `src/paths.rs` | Data and log folders |
| `themes/` | Color themes bundled into the binary |
| `packaging/linux/` | The desktop entry and icon |
| `assets/`, `build.rs` | The app icon and Windows resources |

## Platform windows

On macOS, the content fills the window under a transparent title bar, with the traffic lights in
the app's bar. Menus are in the system menu bar. Closing the last window leaves the app running,
and clicking the Dock icon opens a window.

On Windows, the app draws the title bar, menus and window buttons. Windows handles dragging,
snapping, snap layouts on the maximize button and Alt+F4. Release builds start without a console.

On Linux, the app draws its frame, title bar, menus and window buttons, with a shadow and resize
edges when the window floats.

Buttons and menus in the title bar sit inside `title_bar_item`, which keeps clicks on them from
starting a window drag.

## Commit messages

Use an area followed by a sentence in the subject, such as `sidebar: Enter opens the first match
of the filter`. Explain what changes for the user and why in the body.
