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
| `studio-notifications.json` | The notification history and the sessions with a dot |
| `themes/` | User color themes, also listed in Arko's terminal view |
| `fonts/` | Font files added in Settings |
| `studio-writing/` | Learned vocabulary and words added to the dictionary |

## Settings

**Settings** controls appearance, input and defaults for new sessions. Click an open session's
title to change its live settings in **Session info**, such as whether the agent that started a
worker gets a copy of your messages to it. It also lists the notes the main agent keeps for the
session, and you can remove one.

**Settings › Models & accounts** shows discovered credentials and lets you add or replace keys.
Keys saved there go in the Arko folder's `.env` file. ChatGPT and Claude Code use their existing
logins. You can add several ChatGPT logins (each a Codex `auth.json`). Arko uses them in order, and
the next one takes over when the one before it reaches its limit. Make first and Remove change the
order and the list. See [Arko's documentation](https://github.com/mesa-hills-research/arko) for
providers and credential discovery.

**Settings › Worker profiles** lists the profiles the main agent starts workers with. A profile is
a Markdown file in the `profiles` folder of the Arko folder, with a model, an effort and
instructions, so Arko in the terminal reads the same ones. Add, edit, duplicate and delete your
profiles there. The page also lists each recent project's and repository's `.arko/profiles`, read
only, and the older `.agents/profiles` where a project still has one. A profile read from the older
folder says so. Two switches choose where profiles are kept. **Keep agents' project files out of
git** keeps the skills, MCP servers and profiles agents save in a project's `.arko` folder out of
git, and the other makes a project in a subfolder of a git repository read the repository root's
profiles too. A change to the first goes to the sessions open in Studio, as `/extensions` does in
the terminal, so their web pages show it too. The main agent can save profiles too, for the project
unless it says otherwise. It asks you first when git would track the folder, when the project isn't
in a git repository, or when the profile is for every project. See
[Arko's documentation](https://github.com/mesa-hills-research/arko) for the file format.

**Settings › Sessions** sets who chooses the workers' models, whether workers may run on models
billed per token (**Worker billing**, which asks you first by default), whether new workers on API
models stop starting once a day's API spend reaches $10 (**Daily API budget**, off by default), how
many levels of workers a new session allows, whether a worker fails after 10 failed model requests
in a row (**Worker retry cap**, off by default), and whether the agent that started a worker gets a
copy of your messages to it (**Message copies**, off by default), along with the other defaults for
new sessions. Until you set worker billing or the budget there, Arko's `ARKO_WORKER_BILLING` and
`ARKO_API_BUDGET` settings decide.

Under **Names**, **Session names** chooses how a new session is named. **First words** uses the start
of its first message, cut to 30 display columns. **Named by a model** asks a model in the background,
and the first message goes to the provider that runs it. The ways are Haiku 5.5 on your Claude plan,
GPT-6 Luna on your ChatGPT plan, and the same two models with your Anthropic and OpenAI API keys, which
bill per token and start off. A checkbox turns each on, the arrows set the order, and a way that can't
run says what it lacks. A way that fails, gives no usable name or sends nothing for two minutes hands
over to the next, and one that is still working is never cut off. The first words stay the name when
none works. A name you give a session is never replaced, and a name that arrives after you close the
session is kept for the next time you open it. Arko in the terminal reads the same setting
(`session-names.json`) and changes it with `/session-names`.

**Settings › Extensions** lists the MCP servers and skills sessions get. Its **Approvals** group
says what agents may add without asking you. **Ask before adding MCP servers** is on until you turn
it off, which asks first, and **Review project skills** makes agents ask before they save a skill in
the project too. A change goes to the sessions open in Studio, as on the Worker profiles page. A
skill or a server an agent added says when and why, and **Remove** takes it away after asking. Each
recent project's `.arko/mcp.json`, where agents add the project's servers, is listed with its
`.mcp.json`, each with the answer Arko remembers and **Forget answer**. Arko's built-in skills have
a switch for all of them and one for each. A session opened or resumed after a change gets it, and a
session already open keeps its skills. A switch you haven't used leaves its part to Arko's
`ARKO_BUILTIN_SKILLS` and `ARKO_BUILTIN_SKILLS_OFF` settings.

Each open Agent session runs its own copy of every MCP server, so close the sessions you aren't
using. Chat sessions run none. A launcher such as `npx -y` adds a wrapper process to each copy, so
`node path/to/server.js` is lighter.

**Settings › Notifications** chooses which events notify and whether a sound plays. Each one names
its session, and clicking it shows that session. Every notification that is on is also kept in a
history, even when the window was in front and nothing popped up. Open the history from the unread
count in the status bar, **View › Notifications** or Ctrl+Shift+M (Cmd+Shift+M on macOS). The
latest 200 are kept across restarts. A notification is unread until you open its session.

A session that finished a turn, asked a question, or had a worker report or fail while you looked at
another one has a dot in the sidebar until you open it, also when those notifications are off. A
closed folder shows the dot too while a session inside has one.

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
