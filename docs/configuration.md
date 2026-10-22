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
| `studio-notifications.json` | The notification history |
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

**Settings › Worker profiles** lists the profiles the main agent starts workers with. A profile is
a Markdown file in the `profiles` folder of the Arko folder, with a model, an effort and
instructions, so Arko in the terminal reads the same ones. Add, edit, duplicate and delete your
profiles there. The page also lists each recent project's `.agents/profiles`, read only. The main
agent can save profiles too, and asks you first when it saves one for a project or for every
project. See [Arko's documentation](https://github.com/mesa-hills-research/arko) for the file
format.

**Settings › Sessions** sets who chooses the workers' models, and how many levels of workers a new
session allows, along with the other defaults for new sessions.

**Settings › Notifications** chooses which events notify and whether a sound plays. Each one names
its session, and clicking it shows that session. Every notification that is on is also kept in a
history, even when the window was in front and nothing popped up. Open the history from the unread
count in the status bar, **View › Notifications** or Ctrl+Shift+M (Cmd+Shift+M on macOS). The
latest 200 are kept across restarts. A notification is unread until you open its session.

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
