# Configuration and data

## Shared Arko folder

Arko Studio shares sessions, keys, message history and user themes with Arko in the terminal.
Set `ARKO_HOME` to choose the data folder. Otherwise, it uses the platform's data directory.

| Platform | Default Arko folder |
|---|---|
| Linux | `$XDG_DATA_HOME/arko`, or `~/.local/share/arko` |
| macOS | `~/Library/Application Support/arko` |
| Windows | `%LOCALAPPDATA%\arko` |

A session can be open in one app at a time. Close it in the terminal before opening it in
Arko Studio, or the other way around.

Arko Studio adds these files inside the shared folder.

| Path | What it holds |
|---|---|
| `studio.json` | App preferences and window layout |
| `studio-drafts.json` | Unsent message drafts |
| `themes/` | User color themes, also listed in Arko's terminal view |
| `fonts/` | Font files added in Settings |
| `studio-writing/` | Learned vocabulary and words added to the dictionary |

## Settings

**Settings** controls appearance, input and defaults for new sessions. Click an open session's
title to change its live settings in **Session info**.

**Settings › Models & accounts** shows discovered credentials and lets you add or replace keys.
Keys saved there go in the Arko folder's `.env` file. ChatGPT and Claude Code use their existing
logins. See [Arko's documentation](https://github.com/mesa-hills-research/arko) for providers and
credential discovery.

**Settings › Web page** controls sharing. A session's link lets anyone who has it read the
conversation, send messages and commands, answer questions and stop turns. The page is not
encrypted, so share it only on networks you trust. **New key** ends the old links.

## Logs

**Help › Open Logs Folder** opens the app's log folder.

| Platform | Log folder |
|---|---|
| Linux | `$XDG_STATE_HOME/com.mesahillsresearch.ArkoStudio`, or `~/.local/state/com.mesahillsresearch.ArkoStudio` |
| macOS | `~/Library/Logs/com.mesahillsresearch.ArkoStudio` |
| Windows | `%LOCALAPPDATA%\com.mesahillsresearch.ArkoStudio` |

Release builds write `arko-studio.log`, keeping the previous run as `arko-studio.old.log`.
Debug builds write app logs to the terminal. The engine writes `engine.log` in the log folder.
Set `RUST_LOG` to change the app's log level, which defaults to `info`.
