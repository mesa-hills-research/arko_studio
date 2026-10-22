# Linux desktop integration

After [building Arko Studio](../../README.md#build-and-install), run these commands from the
repository root to install it for your user account.

```sh
install -Dm755 target/release/arko-studio ~/.local/bin/arko-studio
install -Dm644 packaging/linux/com.mesahillsresearch.ArkoStudio.desktop ~/.local/share/applications/com.mesahillsresearch.ArkoStudio.desktop
mkdir -p ~/.local/share/icons && cp -r packaging/linux/icons/hicolor ~/.local/share/icons/
```

Make sure `~/.local/bin` is on the desktop session's `PATH`. The desktop entry starts
`arko-studio` by name and uses `com.mesahillsresearch.ArkoStudio` to match the window and icon.
On X11 the window also sets its own icon, so a binary run from the build folder has one. Wayland
takes the icon from the desktop entry.
The icon comes in sizes from 16 to 512 px, plus a scalable version. The sizes up to 32 px show the
full mark scaled to fill the icon, so it is as large as it can be in panels at 22, 24 and 32 px.
`assets/make-icons.py` makes all of them from the SVGs in `assets/`, as described under
[App icon](../../CONTRIBUTING.md#app-icon).

The app needs a working Vulkan driver. Mesa's `mesa-vulkan-drivers` package supplies drivers for
supported hardware and the software renderer used by screenshot tests.
