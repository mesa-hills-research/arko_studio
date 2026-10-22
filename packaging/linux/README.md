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
The icon comes in sizes from 16 to 512 px, plus a scalable version. The sizes up to 32 px are
drawn for their own size, with extra weight and fitted to the pixel grid, so panel icons at 22, 24
and 32 px stay sharp. `assets/make-icons.py` makes all of them from the SVGs in `assets/`, as
described under [App icon](../../CONTRIBUTING.md#app-icon).

The app needs a working Vulkan driver. Mesa's `mesa-vulkan-drivers` package supplies drivers for
supported hardware and the software renderer used by screenshot tests.
