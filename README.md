# Arko Studio

Arko Studio is a desktop app for [Arko](https://github.com/mesa-hills-research/arko), the
general-purpose agent, on macOS, Windows and Linux. It works with the same saved sessions as Arko in
the terminal, in a native window built in Rust on [GPUI](https://www.gpui.rs) and
[GPUI Kit](https://github.com/mesa-hills-research/gpui_kit).

- A sidebar of sessions, chats first and then by folder, with a filter, renaming and deleting, all
  of it from the keyboard too
- Layouts for wide, half-screen and narrow windows, where the sidebar and the workers panel slide
  in over the session
- 41 color themes in a light and a dark choice that follow the system, with previews as you move
  through the list
- Your own interface and code fonts, font files you add, zoom, and motion that follows the system
  or your choice
- Your own themes in GPUI Kit's format, shared with Arko

## Status

Arko Studio is early in its development. The window, the sidebar and Settings work, on a set of
sample sessions. Opening real sessions, the transcript and the composer come next, once Arko's
engine is a library. Until then, menu items that need the engine say so when you choose them.

## Building

Arko Studio needs Rust 1.89 or later and the tools GPUI builds with: Xcode on macOS, the Visual
Studio C++ build tools on Windows, and on Linux a C compiler with the fontconfig, Wayland,
xkbcommon and X11 development packages (`libfontconfig-dev libwayland-dev libxkbcommon-x11-dev
libx11-xcb-dev` on Ubuntu).

```sh
git clone https://github.com/mesa-hills-research/arko_studio.git
cd arko_studio
cargo run --release
```

The binary is `arko-studio`. On Linux, install it with its desktop entry and icon so launchers and
window lists show its name and icon:

```sh
install -Dm755 target/release/arko-studio ~/.local/bin/arko-studio
install -Dm644 packaging/linux/com.mesahillsresearch.ArkoStudio.desktop ~/.local/share/applications/com.mesahillsresearch.ArkoStudio.desktop
install -Dm644 packaging/linux/com.mesahillsresearch.ArkoStudio.png ~/.local/share/icons/hicolor/256x256/apps/com.mesahillsresearch.ArkoStudio.png
```

## Where it keeps things

Arko Studio shares Arko's folder: `ARKO_HOME` when it's set, otherwise `arko` in the platform's
data folder (`~/.local/share/arko` on Linux, `~/Library/Application Support/arko` on macOS and
`%LOCALAPPDATA%\arko` on Windows).

| Path | What it holds |
|---|---|
| `studio.json` | Arko Studio's settings, and the window's and sidebar's sizes |
| `themes/` | Your own color themes, which Arko's terminal view lists too |
| `fonts/` | Font files added in Settings |

Release builds write a log, `arko-studio.log`, to `com.mesahillsresearch.ArkoStudio` in the
platform's log folder: `~/.local/state` on Linux, `~/Library/Logs` on macOS and `%LOCALAPPDATA%` on
Windows. Help > Open Logs Folder opens it.

## Contributing

[CONTRIBUTING.md](CONTRIBUTING.md) covers building against a local GPUI Kit, the tests and how the
source is laid out.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT), at your option.

The bundled color themes and icons keep their own licenses, listed in [NOTICE](NOTICE).
