# Arko Studio

Arko Studio is a desktop app for [Arko](https://github.com/mesa-hills-research/arko), an AI agent
harness. It shares sessions with Arko in the terminal and runs on macOS, Windows and Linux.
The native interface is written in Rust with [GPUI](https://www.gpui.rs) and
[GPUI Kit](https://github.com/mesa-hills-research/gpui_kit).

![Arko Studio showing a conversation and tool calls in a dark theme](tests/screenshots/transcript-wide-dark.png)

- Chat with a model, or work in a folder with tools and background workers.
- Read streamed replies, Markdown, highlighted code, diffs and images in one transcript.
- Find, edit, regenerate, branch and export conversations, with keyboard shortcuts and a command
  palette.
- See context use, costs and cache expiry, and share a session through its web page.
- Choose light and dark themes, fonts, input keys, spell checking and word suggestions.

## Build and install

Install a current stable [Rust toolchain](https://rustup.rs). The app's minimum Rust version is
1.95. Platform build tools are also needed.

- On macOS, install Xcode and its command-line tools. With Xcode 26, install the Metal toolchain
  using `xcodebuild -downloadComponent MetalToolchain` if it is missing.
- On Windows, install the Visual Studio C++ build tools and use Rust's MSVC toolchain.
- On Debian or Ubuntu, install the compiler, development libraries and Vulkan driver below.
  Other Linux distributions have equivalent packages.

```sh
sudo apt install build-essential pkg-config cmake libfontconfig-dev libwayland-dev \
  libxkbcommon-x11-dev libx11-xcb-dev libvulkan1 mesa-vulkan-drivers
```

Clone the repository and build it. Cargo fetches Arko, its framework and GPUI (the `mhr-gpui`
crates) from crates.io, and the pinned GPUI Kit commit from GitHub.

```sh
git clone https://github.com/mesa-hills-research/arko_studio.git
cd arko_studio
cargo build --release --locked
```

The executable is `target/release/arko-studio` (`arko-studio.exe` on Windows). On Linux,
[install the desktop entry and icon](packaging/linux/README.md) to add it to your launcher.

## Start a session

Open the app and choose **New session**. The chip below the message field selects Chat or Agent
and the model. Agent mode asks for a folder to work in. Add credentials in **Settings › Models &
accounts**, or use an existing ChatGPT or Claude Code login. Type a message and press Enter to
send it. **Settings › Input** changes the send key.

Open a session from the sidebar, then click its title at the top of the window to open
**Session info**. Use **Help › Keyboard Shortcuts** to find an action's keys.

## More

- [Configuration and data](docs/configuration.md) covers shared sessions, preferences and logs.
- [Themes](themes/README.md) explains how to add a color scheme.
- [Contributing](CONTRIBUTING.md) covers tests and the source layout.
- [Arko](https://github.com/mesa-hills-research/arko) documents the engine, providers and commands.

## License

Licensed under either the [Apache License, Version 2.0](LICENSE-APACHE) or the
[MIT License](LICENSE-MIT), at your option.

Bundled fonts, color themes and icons keep their own licenses, listed in [NOTICE](NOTICE).
