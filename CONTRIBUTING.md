# Contributing to Arko Studio

Follow the [build instructions](README.md#build-and-install) to install the platform dependencies
and build the app.

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

`Cargo.toml` pins GPUI Kit to a Git commit. The kit takes GPUI from the published `mhr-gpui`
crates, so the app has no GPUI dependency of its own besides the screenshot harness. To try a
change in a local kit checkout, add this override to the ignored `.cargo/config.toml`.

```toml
[patch."https://github.com/mesa-hills-research/gpui_kit"]
gpui-kit = { path = "../gpui_kit/crates/kit" }
```

The kit checkout must use the same `mhr-gpui` release as the app. When updating the kit revision,
change both `gpui-kit` entries in `Cargo.toml`. If the kit moves to another `mhr-gpui` release,
also set the screenshot harness to the gpui commit that release was cut from, and set the `=`
versions of `mhr-gpui-wgpu` and of the two entries in
`[patch."https://github.com/mesa-hills-research/gpui"]` to the kit's. Then regenerate `Cargo.lock`
without the local override.

## Building against local Arko and framework checkouts

`Cargo.toml` takes Arko (the `arko-tui` package) and its framework crates from crates.io. To work
on them together with the app, clone [Arko](https://github.com/mesa-hills-research/arko) and
[arko_framework](https://github.com/mesa-hills-research/arko_framework) beside this repository
and add this override to the ignored `.cargo/config.toml`.

```toml
[patch.crates-io]
arko-tui = { path = "../arko" }
arko_ai = { path = "../arko_framework/arko_ai" }
arko_agents = { path = "../arko_framework/arko_agents" }
arko_workflows = { path = "../arko_framework/arko_workflows" }
arko_index = { path = "../arko_framework/arko_index" }
```

Override all five, so the app and Arko use the same framework crates. The checkouts' versions
must match the ones `Cargo.toml` asks for. Cargo records the override in `Cargo.lock`, so
regenerate it without the override before committing.

## App icon

The artwork is in `assets/`. The mark has three levels of detail. The arch, the stem and the arrow
come first, then the nodes, then the thin links and the gap in the top of the arch. The full mark
sits in a wide margin, so scaled down as it is, it fills only part of a small icon. Sizes from 48 px
up use `icon.svg` as it is. The sizes below 48 px use the same mark scaled to fill the canvas.

`CHOICE` in `assets/make-icons.py` gives the drawing for 16, 20, 22, 24, 32 and 40 px as a letter.
It is `d` at every size. To switch a size, change its letter and run the script again.

| Letter | Drawing |
|---|---|
| `d` | The full mark scaled to fill the canvas. This is the default |
| `n` | The full mark scaled to the size with its margin, as it was before `d` |
| `e` | `d` with weight added around every shape |
| `f` | `e` with its size, position and weight fitted to the pixel grid, then drawn at 8 times the size and averaged down |
| `g` | Like `f`, but only the thin parts get heavier, the arrow shaft is a whole number of pixels thick and the nodes grow a little |
| `a`, `b`, `c` | Hand drawn candidates in `assets/icon-candidates/`, named `icon-<size>-<letter>.svg`. These are the arch, stem and arrow only (`a`), with the nodes and the line through them (`b`), and the mark of `b` on a rounded tile (`c`) |

The drawings `d` to `g` come from `icon.svg` by script, so a change to the mark reaches every size.
`LOOK` in the script sets the weight for each size, and the numbers that fitting aligns to the
pixel grid are next to it. `icon-tile.svg` puts the full mark on a rounded tile.

After changing an SVG, run `python3 assets/make-icons.py`, which needs `rsvg-convert` or `resvg`,
and commit the files it writes. These are `assets/icon.ico` (16, 20, 24, 32, 40, 48, 64, 128 and
256 px), `assets/icon-1024.png` and the icons in `packaging/linux/icons`. Judge a small drawing at
its own size. A viewer that scales it hides the edges it is made to keep, so enlarge it without
smoothing to look closer. The screenshot tests use the 256 px icon as a sample image, so changing
the artwork changes the transcript goldens too.

## Source layout

| Path | What it holds |
|---|---|
| `src/main.rs`, `src/lib.rs` | Logging, startup and app globals |
| `src/backend.rs`, `src/backend/` | Session storage, credentials, defaults and worker profiles |
| `src/live.rs` | Running sessions and engine events |
| `src/workspace.rs` | Window panes, sheets, resizing and actions |
| `src/sidebar.rs` | Session grouping, filtering, renaming and deleting |
| `src/session_view.rs`, `src/composer.rs` | Session controls, message input and attachments |
| `src/transcript/` | Transcript rows, selection, navigation and find |
| `src/status_bar.rs`, `src/workers_panel.rs`, `src/workers.rs` | Session figures, the workers panel and how it lists workers |
| `src/settings_page.rs`, `src/settings_page/` | Settings pages and fields |
| `src/theme_picker.rs`, `src/appearance.rs` | Themes, previews, fonts, zoom and motion |
| `src/prefs.rs`, `src/drafts.rs` | Saved preferences and message drafts |
| `src/commands.rs`, `src/menus.rs` | Commands, keyboard shortcuts and menus |
| `src/paths.rs` | Data and log folders |
| `themes/` | Color themes bundled into the binary |
| `packaging/linux/` | The desktop entry and the icon sizes for Linux |
| `assets/`, `build.rs` | The icon artwork, its generated files and the Windows resources |

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

## Words in the app

Everything a user reads uses the fewest words that keep the meaning. A label doesn't repeat what
surrounds it, a state the user can already see isn't named, and one word replaces a sentence where
it says the same ("Pending" for a note the agent hasn't read). Keep every fact the reader needs,
write plain sentences, and join clauses with periods, commas or "so", not colons or semicolons.

## Commit messages

Use an area followed by a sentence in the subject, such as `sidebar: Enter opens the first match
of the filter`. Explain what changes for the user and why in the body.
