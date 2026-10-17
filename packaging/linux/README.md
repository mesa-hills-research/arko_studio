# Linux desktop integration

After [building Arko Studio](../../README.md#build-and-install), run these commands from the
repository root to install it for your user account.

```sh
install -Dm755 target/release/arko-studio ~/.local/bin/arko-studio
install -Dm644 packaging/linux/com.mesahillsresearch.ArkoStudio.desktop ~/.local/share/applications/com.mesahillsresearch.ArkoStudio.desktop
install -Dm644 packaging/linux/com.mesahillsresearch.ArkoStudio.png ~/.local/share/icons/hicolor/256x256/apps/com.mesahillsresearch.ArkoStudio.png
```

Make sure `~/.local/bin` is on the desktop session's `PATH`. The desktop entry starts
`arko-studio` by name and uses `com.mesahillsresearch.ArkoStudio` to match the window and icon.

The app needs a working Vulkan driver. Mesa's `mesa-vulkan-drivers` package supplies drivers for
supported hardware and the software renderer used by screenshot tests.
