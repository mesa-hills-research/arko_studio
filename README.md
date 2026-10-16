# Arko Studio

Arko Studio is a desktop app for macOS, Windows and Linux, built with
[GPUI](https://www.gpui.rs) and [GPUI Kit](https://github.com/mesa-hills-research/gpui_kit).

## Running it

```sh
cargo run
```

Debug builds log to the terminal. Release builds (`cargo run --release`) log to a file in the
platform's log folder, under `com.mesahillsresearch.ArkoStudio`: `~/Library/Logs` on macOS, `%LOCALAPPDATA%` on
Windows and `~/.local/state` on Linux. `RUST_LOG` sets the level.

Building needs the platform's tools: Xcode on macOS, the Visual Studio C++ build tools on Windows,
and on Linux a C compiler with the fontconfig, Wayland, xkbcommon and X11 development packages
(`libfontconfig-dev libwayland-dev libxkbcommon-x11-dev libx11-xcb-dev` on Ubuntu).

## Where things live

| Path | What it holds |
|---|---|
| `src/main.rs` | The app's name and id, logging, startup |
| `src/chrome.rs` | Window options and `can_close`, the check every way of closing a window goes through |
| `src/menus.rs` | Actions, key bindings and the menus |
| `src/title_bar.rs` | The title bar, its menus and buttons |
| `src/workspace.rs` | The window's root view, where the app's content goes |
| `build.rs` | Embeds `assets/icon.ico` in Windows builds |
| `assets/` | The app icon, as SVG and Windows `.ico` |
| `packaging/linux/` | The desktop entry and icon for Linux |

## The window on each platform

- **macOS:** the content fills the window under a transparent title bar, with the traffic lights
  in the app's bar. The menus are in the system menu bar, with About, Settings, Services, Hide and
  Quit in the app menu. Closing the last window leaves the app running, and clicking the Dock icon
  opens a new window.
- **Windows:** the app draws the title bar with its menus and the minimize, maximize and close
  buttons. Windows still handles dragging, snapping, snap layouts on the maximize button, the
  window menu and Alt+F4. Release builds start without a console.
- **Linux:** the app draws its own frame, title bar, menus and window buttons on every desktop,
  with a shadow and resize edges when the window floats.

Buttons and menus in the title bar sit inside `title_bar_item`, which keeps a click on them from
starting a window drag.

## Linux desktop entry

The desktop entry gives the app its name and icon in launchers and window lists. Install it, the
icon and the binary for your user:

```sh
cargo build --release
install -Dm755 target/release/arko_studio ~/.local/bin/arko_studio
install -Dm644 packaging/linux/com.mesahillsresearch.ArkoStudio.desktop ~/.local/share/applications/com.mesahillsresearch.ArkoStudio.desktop
install -Dm644 packaging/linux/com.mesahillsresearch.ArkoStudio.png ~/.local/share/icons/hicolor/256x256/apps/com.mesahillsresearch.ArkoStudio.png
```

## Icons

`assets/icon.svg` is a placeholder. Replace it, then regenerate `assets/icon.ico` (16 to 256 px)
and `packaging/linux/com.mesahillsresearch.ArkoStudio.png` (256 px) from it.
