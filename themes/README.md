# Color themes

These GPUI Kit JSON themes are bundled into Arko Studio. Each file names its color scheme's
author and source. See [NOTICE](../NOTICE) and [LICENSE-APACHE](LICENSE-APACHE) for attribution
and licensing.

To add a theme, copy a JSON file such as [macos-classic.json](macos-classic.json) into the
`themes` subfolder of your [Arko data folder](../docs/configuration.md#shared-arko-folder).
Give the theme and each variant a name, then edit its colors. The file's `$schema` field links
to the GPUI Kit format.

The app watches this folder for changes. Choose the light and dark variants in
**Settings › Appearance**. User themes with the same name as a bundled theme replace it.
Arko in the terminal reads the same folder.
