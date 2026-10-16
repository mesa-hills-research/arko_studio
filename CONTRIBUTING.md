# Contributing to Arko Studio

## Building against a local GPUI Kit

`Cargo.toml` takes GPUI Kit from GitHub at a fixed commit, and the GPUI fork through
`[patch.crates-io]`. To try a change to the kit before it's pushed, point Cargo at your checkout
in `.cargo/config.toml`, which git ignores:

```toml
[patch."https://github.com/mesa-hills-research/gpui_kit"]
gpui-kit = { path = "../gpui_kit/crates/kit" }
```

The checkout has to build against the same GPUI commit as `Cargo.toml`. Once the kit change is
pushed, move the `rev` in `Cargo.toml` to it (both entries, the dependency and the
dev-dependency) and delete the override.

## Tests

```sh
cargo test
cargo clippy --all-targets -- --deny warnings
cargo fmt --check
```

The interaction tests in `src/tests.rs` open windows on GPUI's test platform and drive them by
element id, with `TestWindowExt` from GPUI Kit. An element a test looks up opts in with
`.test_support()`, called before `.track_focus()`. The tests run on a fake backend, `FakeBackend`
in `src/backend.rs`, with a fixed set of sessions.

Every change in behaviour comes with a test.

## How the source is laid out

| Path | What it holds |
|---|---|
| `src/main.rs` | Logging and startup |
| `src/lib.rs` | What startup loads, and `init`, which sets up the app's globals |
| `src/backend.rs` | `Backend`, the app's boundary with Arko's sessions, and `FakeBackend` |
| `src/workspace.rs` | A window: its panes for each width, sheets, resizing, and the actions that act on the window |
| `src/sidebar.rs` | The sessions list: grouping, filtering, keyboard use, renaming, deleting |
| `src/session_view.rs` | The main column: the transcript and the composer |
| `src/settings_page.rs` | Settings, shown in the main column |
| `src/theme_picker.rs` | The theme lists in Settings, with previews |
| `src/appearance.rs` | Themes, fonts, zoom and motion, applied from the settings |
| `src/prefs.rs` | `studio.json`: the settings and their saving |
| `src/fonts.rs` | Bundled fonts and the font files a user adds |
| `src/menus.rs` | Actions, key bindings and the menus |
| `src/chrome.rs`, `src/title_bar.rs` | Window options, closing, and the title bar |
| `src/paths.rs` | Arko's folder, the logs folder |
| `themes/` | The bundled color themes, built into the binary |
| `packaging/linux/` | The desktop entry and icon for Linux |
| `assets/`, `build.rs` | The app icon, embedded in Windows builds |

## The window on each platform

- **macOS:** the content fills the window under a transparent title bar, with the traffic lights
  in the app's bar. The menus are in the system menu bar. Closing the last window leaves the app
  running, and clicking the Dock icon opens a new window.
- **Windows:** the app draws the title bar with its menus and the window buttons. Windows still
  handles dragging, snapping, snap layouts on the maximize button and Alt+F4. Release builds
  start without a console.
- **Linux:** the app draws its own frame, title bar, menus and window buttons, with a shadow and
  resize edges when the window floats.

Buttons and menus in the title bar sit inside `title_bar_item`, which keeps a click on them from
starting a window drag.

## Commit messages

A subject names the area and says what changes, as a sentence: `sidebar: Enter opens the first
match of the filter`. The body explains what changes for the user and why.
