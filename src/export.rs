//! Exporting a conversation as Markdown (Session › Export as Markdown…):
//! the user's messages and the agent's replies as they were written, its
//! thinking folded, its tool calls as one line each, and what Arko said
//! along the way.

use std::path::{Path, PathBuf};

use arko::remote::{CallStatus, Item, Kind, Level};

/// What the export's header says of the session.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Header {
    pub title: String,
    /// "Agent" or "Chat".
    pub mode: String,
    /// The main agent's model, as people say it.
    pub model: String,
    /// The folder an agent worked in, as shown (`~/src/arko`).
    pub folder: Option<String>,
    /// When it was exported, as shown.
    pub when: String,
    /// The conversation's first messages aren't in the items: the
    /// transcript keeps the latest part of a long conversation.
    pub starts_partway: bool,
}

/// Uses the whole saved conversation, adding a newer visible turn that
/// has not been saved yet. Render the Markdown off the UI thread.
pub fn with_saved(
    mut header: Header,
    visible: Vec<Item>,
    saved: Option<arko::sessions::SavedConversation>,
) -> (String, bool) {
    let Some(saved) = saved else {
        return (markdown(&header, &visible), header.starts_partway);
    };
    header.starts_partway = !saved.reports.is_empty();
    let mut items = saved.items;
    let last_saved = items.iter().filter_map(|item| item.message_index).max();
    if let Some(start) = visible.iter().position(|item| {
        item.kind == Kind::User && item.is_main() && item.message_index > last_saved
    }) {
        items.extend_from_slice(&visible[start..]);
    }
    let mut text = markdown(&header, &items);
    if !saved.reports.is_empty() {
        text.push_str("\n## Earlier work\n");
        for report in saved.reports {
            text.push_str(&format!("\n{}\n", closed(&report)));
        }
    }
    (text, header.starts_partway)
}

/// The conversation `items` (one agent's, in order) as a Markdown
/// document.
pub fn markdown(header: &Header, items: &[Item]) -> String {
    let mut out = String::new();
    out.push_str(&format!("# {}\n\n", one_line(&header.title)));
    let mut facts = vec![header.mode.clone()];
    facts.extend(Some(header.model.clone()).filter(|model| !model.is_empty()));
    facts.extend(header.folder.clone());
    facts.push(format!("exported {}", header.when));
    out.push_str(&format!("*{}*\n", facts.join(" · ")));
    if header.starts_partway {
        out.push_str(&format!("\n*{STARTS_PARTWAY}*\n"));
    }
    // Who said the last section: a new heading only when it changes.
    let mut speaker: Option<&'static str> = None;
    let mut calls = false;
    for item in items {
        let mut section = |out: &mut String, name: &'static str| {
            if speaker != Some(name) {
                out.push_str(&format!("\n## {name}\n"));
                speaker = Some(name);
            }
        };
        let is_call = item.kind == Kind::Call;
        if calls && !is_call {
            calls = false;
        }
        match item.kind {
            Kind::User => {
                section(&mut out, "You");
                // As typed: the app shows it with Markdown off.
                let mut text = as_typed(trimmed(&item.text));
                if !item.is_main() {
                    text = format!("*(to {})* {text}", item.agent);
                }
                out.push_str(&format!("\n{text}\n"));
                for image in &item.images {
                    out.push_str(&format!("\n*[image: {}]*\n", image_label(&image.label)));
                }
            }
            Kind::Text | Kind::Reply | Kind::Question => {
                if item.text.trim().is_empty() {
                    continue;
                }
                section(&mut out, "Arko");
                out.push_str(&format!("\n{}\n", closed(trimmed(&item.text))));
            }
            Kind::Message => {
                section(&mut out, "Arko");
                // Arko's own words to the user carry no sender. A worker's
                // say whose they are.
                if let Some(from) = item.from.as_deref() {
                    let from = match from {
                        arko::remote::MAIN_AGENT => "the lead",
                        worker => worker,
                    };
                    out.push_str(&format!("\n*From {from}*\n"));
                }
                out.push_str(&format!("\n{}\n", closed(trimmed(&item.text))));
            }
            Kind::Thinking => {
                if item.text.trim().is_empty() {
                    continue;
                }
                section(&mut out, "Arko");
                let text = closed(trimmed(&item.text))
                    .replace("</details", "&lt;/details")
                    .replace("</DETAILS", "&lt;/DETAILS");
                out.push_str(&format!(
                    "\n<details><summary>Thinking</summary>\n\n{text}\n\n</details>\n"
                ));
            }
            Kind::Call => {
                section(&mut out, "Arko");
                if !calls {
                    out.push('\n');
                    calls = true;
                }
                let line = item.text.lines().next().unwrap_or_default().trim();
                let mut entry = format!("- {}", code(line));
                if !item.is_main() {
                    entry = format!("- {} {}", item.agent, code(line));
                }
                if item.status == Some(CallStatus::Failed) {
                    let why = item.outcome.as_deref().unwrap_or("failed");
                    entry.push_str(&format!(" (failed: {})", one_line(why)));
                }
                for image in &item.images {
                    entry.push_str(&format!(" *[image: {}]*", image_label(&image.label)));
                }
                out.push_str(&entry);
                out.push('\n');
            }
            Kind::Notice => {
                let label = match item.level {
                    Some(Level::Error) => "Error",
                    Some(Level::Warn) => "Warning",
                    _ => "Note",
                };
                out.push_str(&format!("\n> **{label}:** {}\n", quoted(item.text.trim())));
            }
            Kind::Output => {
                let text = item.text.trim_end();
                let fence = fence_for(text);
                out.push_str(&format!("\n{fence}text\n{text}\n{fence}\n"));
            }
            Kind::Turn => {
                if !item.text.trim().is_empty() {
                    out.push_str(&format!("\n*{}*\n", one_line(&item.text)));
                }
            }
            Kind::WorkerLine => {
                section(&mut out, "Arko");
                let worker = item.worker.as_deref().unwrap_or("A worker");
                out.push_str(&format!("\n*{worker} started: {}*\n", one_line(&item.text)));
            }
            Kind::Conversation => {
                out.push_str(&format!("\n---\n\n*{}*\n", one_line(&item.text)));
                speaker = None;
            }
            Kind::Result => {}
        }
    }
    out
}

/// `text` as inline code, with a fence longer than any run of backticks
/// in it.
fn code(text: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for character in text.chars() {
        if character == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    let fence = "`".repeat(longest + 1);
    let pad = if text.starts_with('`') || text.ends_with('`') {
        " "
    } else {
        ""
    };
    format!("{fence}{pad}{text}{pad}{fence}")
}

/// What the export says under its header when it starts partway.
const STARTS_PARTWAY: &str =
    "This export starts partway through the conversation. Earlier messages aren’t included.";

/// The fence a block of `text` needs: a backtick more than its longest run
/// of them, and three at least.
fn fence_for(text: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for character in text.chars() {
        if character == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    "`".repeat((longest + 1).max(3))
}

/// `text` without the blank lines around it. Its first line keeps its
/// indentation (an indented code block stays one).
fn trimmed(text: &str) -> &str {
    let text = text.trim_end();
    let blank: usize = text
        .split_inclusive('\n')
        .take_while(|line| line.trim().is_empty())
        .map(str::len)
        .sum();
    &text[blank..]
}

/// Markdown `text` with a code fence it leaves open closed (a reply
/// stopped halfway through a code block), so that what follows it in the
/// export isn't code too.
fn closed(text: &str) -> String {
    let mut open: Option<(char, usize)> = None;
    for line in text.lines() {
        // Inside a list item or a quote, the fence comes after its marks.
        let line = line.trim_start_matches([' ', '\t', '>']);
        let fence = ['`', '~'].into_iter().find_map(|mark| {
            let length = line.chars().take_while(|c| *c == mark).count();
            (length >= 3).then_some((mark, length))
        });
        match (open, fence) {
            (None, Some(fence)) => open = Some(fence),
            (Some((mark, length)), Some((closing, closing_length)))
                if closing == mark
                    && closing_length >= length
                    && line[closing_length..].trim().is_empty() =>
            {
                open = None
            }
            _ => {}
        }
    }
    match open {
        Some((mark, length)) => format!("{text}\n{}", mark.to_string().repeat(length)),
        None => text.to_string(),
    }
}

/// The user's `text` as they typed it, read as Markdown: what would start
/// a heading, a quote, a list, a code block or HTML at a line's start is
/// escaped, and its line breaks are kept.
fn as_typed(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = String::with_capacity(text.len() + 16);
    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        if line.trim().is_empty() {
            continue;
        }
        let indent = line.len() - line.trim_start_matches(' ').len();
        let rest = &line[indent..];
        if indent >= 4 {
            // Four spaces start a code block: the first one as an entity.
            out.push_str("&#32;");
            out.push_str(&line[1..indent]);
        } else {
            out.push_str(&line[..indent]);
        }
        let digits = rest.chars().take_while(char::is_ascii_digit).count();
        let first = rest.chars().next();
        if digits > 0 && matches!(rest[digits..].chars().next(), Some('.' | ')')) {
            // "1. " starts a list: its mark escaped.
            out.push_str(&rest[..digits]);
            out.push('\\');
            out.push_str(&rest[digits..]);
        } else if matches!(
            first,
            Some('#' | '>' | '-' | '+' | '*' | '=' | '|' | '`' | '~' | '_')
        ) {
            out.push('\\');
            out.push_str(rest);
        } else {
            out.push_str(rest);
        }
        // A line break as typed, before another line of the same paragraph.
        if lines
            .get(index + 1)
            .is_some_and(|next| !next.trim().is_empty())
        {
            out.push('\\');
        }
    }
    // No HTML: what looks like a tag shows as typed.
    out.replace('<', "\\<")
}

/// `text` on one line.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `text` in a quote, line by line.
fn quoted(text: &str) -> String {
    text.lines().collect::<Vec<_>>().join("\n> ")
}

fn image_label(label: &str) -> &str {
    if label.is_empty() { "image" } else { label }
}

/// A file name for the export of a session titled `title`.
pub fn file_name(title: &str) -> String {
    let name: String = title
        .chars()
        .filter(|character| !is_format(*character))
        .map(|character| match character {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => ' ',
            character if character.is_control() => ' ',
            character => character,
        })
        .collect();
    let name = one_line(&name);
    let name: String = name.chars().take(80).collect();
    let name = name.trim_matches(['.', ' ']);
    if name.is_empty() {
        "Conversation.md".to_string()
    } else if is_reserved(name) {
        // Windows keeps these names for devices, with any extension.
        format!("Conversation {name}.md")
    } else {
        format!("{name}.md")
    }
}

/// Whether `name` is one Windows keeps for a device (`CON`, `aux`,
/// `COM1`), whatever follows a dot.
fn is_reserved(name: &str) -> bool {
    let stem = name
        .split('.')
        .next()
        .unwrap_or(name)
        .trim_end()
        .to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ["COM", "LPT"].iter().any(|device| {
            stem.strip_prefix(device).is_some_and(|number| {
                matches!(
                    number,
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            })
        })
}

/// Whether `character` is an invisible formatting character: one that
/// turns text right to left, joins or hides, which a file name shouldn't
/// carry.
fn is_format(character: char) -> bool {
    matches!(
        character,
        '\u{00AD}'
            | '\u{061C}'
            | '\u{180E}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206F}'
            | '\u{FEFF}'
            | '\u{FFF9}'..='\u{FFFB}'
    )
}

/// A file in `folder` named `name`, or `name` with a number when that one
/// is taken (`Notes 2.md`).
pub fn free_path(folder: &Path, name: &str) -> PathBuf {
    let path = folder.join(name);
    if !path.exists() {
        return path;
    }
    let (stem, extension) = name.rsplit_once('.').unwrap_or((name, "md"));
    (2..)
        .map(|number| folder.join(format!("{stem} {number}.{extension}")))
        .find(|path| !path.exists())
        .expect("a free name")
}

/// Writes `text` to `path` whole: to a file beside it first, which then
/// takes its name. A failed write leaves the file that was there as it was.
pub fn write(path: &Path, text: &str) -> std::io::Result<()> {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let beside = path.with_file_name(format!(".{name}.part"));
    let written = std::fs::write(&beside, text).and_then(|()| std::fs::rename(&beside, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&beside);
    }
    written
}

/// Why a write failed, in plain words.
pub fn why(error: &std::io::Error) -> String {
    use std::io::ErrorKind;
    match error.kind() {
        ErrorKind::PermissionDenied => "Arko isn’t allowed to write there.".to_string(),
        ErrorKind::NotFound => "The folder isn’t there anymore.".to_string(),
        ErrorKind::StorageFull => "The disk is full.".to_string(),
        ErrorKind::ReadOnlyFilesystem => "The disk is read-only.".to_string(),
        _ => format!("The system said “{error}”."),
    }
}

/// Where the save dialog opens: the folder the session worked in, else the
/// home folder.
pub fn folder(workspace: Option<&Path>, home: Option<&Path>) -> PathBuf {
    workspace
        .or(home)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
#[path = "export_tests.rs"]
mod engine_tests;

#[cfg(test)]
mod tests {
    use super::{Header, STARTS_PARTWAY, code, file_name, free_path, markdown, why, with_saved, write};
    use arko::remote::{CallStatus, Item, Kind, Level};

    fn item(kind: Kind, text: &str) -> Item {
        Item::new(kind, text)
    }

    #[test]
    fn saved_messages_fill_a_trimmed_transcript_without_losing_a_new_turn() {
        let mut first = item(Kind::User, "The start.");
        first.message_index = Some(0);
        let mut next = item(Kind::User, "A new turn.");
        next.message_index = Some(2);
        let saved = arko::sessions::SavedConversation {
            number: 1,
            title: "The start.".into(),
            items: vec![first, item(Kind::Reply, "The first reply.")],
            images: Default::default(),
            reports: Vec::new(),
        };
        let header = Header { starts_partway: true, ..Header::default() };
        let (text, partial) = with_saved(header.clone(), vec![next], Some(saved.clone()));
        assert!(!partial);
        assert!(text.contains("The start."));
        assert!(text.contains("A new turn."));
        assert!(!text.contains(STARTS_PARTWAY));
        let mut handed_over = saved;
        handed_over.reports.push("The earlier work was summarized.".into());
        let (text, partial) = with_saved(header, Vec::new(), Some(handed_over));
        assert!(partial);
        assert!(text.contains(STARTS_PARTWAY));
        assert!(text.contains("The earlier work was summarized."));
    }

    #[test]
    fn a_conversation_reads_as_markdown() {
        let mut failed = item(Kind::Call, "$ cargo test -p parse");
        failed.status = Some(CallStatus::Failed);
        failed.outcome = Some("exit 101".into());
        let header = Header {
            title: "Streaming parser".into(),
            mode: "Agent".into(),
            model: "Opus 5.5".into(),
            folder: Some("~/src/parse".into()),
            when: "2026-10-17 09:30".into(),
            starts_partway: false,
        };
        let text = markdown(
            &header,
            &[
                item(Kind::User, "Make the parser streaming."),
                item(Kind::Thinking, "It takes a whole string."),
                item(Kind::Call, "read src/parse.rs:1-120"),
                failed,
                item(Kind::Notice, "rate limited, retrying").level(Level::Warn),
                item(Kind::Reply, "Done: **streaming** now."),
                item(Kind::Turn, "48 s · $0.21"),
                item(Kind::User, "Thanks."),
            ],
        );
        assert_eq!(
            text,
            "# Streaming parser

*Agent · Opus 5.5 · ~/src/parse · exported 2026-10-17 09:30*

## You

Make the parser streaming.

## Arko

<details><summary>Thinking</summary>

It takes a whole string.

</details>

- `read src/parse.rs:1-120`
- `$ cargo test -p parse` (failed: exit 101)

> **Warning:** rate limited, retrying

Done: **streaming** now.

*48 s · $0.21*

## You

Thanks.
"
        );
    }

    /// What a reply, an output, a thinking or the user's text holds keeps
    /// to its place in the export.
    #[test]
    fn odd_text_keeps_its_place() {
        let header = Header {
            title: "Odd".into(),
            mode: "Agent".into(),
            when: "now".into(),
            ..Header::default()
        };
        let text = markdown(
            &header,
            &[
                item(
                    Kind::User,
                    "## Arko\n1. first\n    four spaces\nsee <b>this</b>\n\nnext",
                ),
                item(Kind::Reply, "Stopped in code:\n\n```rust\nfn main() {\n"),
                item(Kind::Output, "a\n```\nb"),
                item(Kind::Thinking, "Ends with </details> early."),
                item(Kind::Reply, "    let indented = 1;\n"),
                item(Kind::User, "Thanks."),
            ],
        );
        assert_eq!(
            text,
            "# Odd

*Agent · exported now*

## You

\\## Arko\\
1\\. first\\
&#32;   four spaces\\
see \\<b>this\\</b>

next

## Arko

Stopped in code:

```rust
fn main() {
```

````text
a
```
b
````

<details><summary>Thinking</summary>

Ends with &lt;/details> early.

</details>

    let indented = 1;

## You

Thanks.
"
        );
    }

    /// Arko's own messages to the user, as it works, carry no sender. A
    /// worker's say whose they are.
    #[test]
    fn arkos_own_messages_carry_no_sender() {
        let header = Header {
            title: "Messages".into(),
            mode: "Agent".into(),
            when: "now".into(),
            ..Header::default()
        };
        let mut from_worker = item(Kind::Message, "Done with the parser.");
        from_worker.from = Some("r1".into());
        let text = markdown(&header, &[item(Kind::Message, "I'm on it."), from_worker]);
        assert!(
            text.ends_with("## Arko\n\nI'm on it.\n\n*From r1*\n\nDone with the parser.\n"),
            "{text}"
        );
    }

    #[test]
    fn an_export_that_starts_partway_says_so() {
        let header = Header {
            title: "Long".into(),
            mode: "Chat".into(),
            when: "now".into(),
            starts_partway: true,
            ..Header::default()
        };
        let text = markdown(&header, &[item(Kind::User, "Go on.")]);
        assert!(
            text.starts_with(
                "# Long\n\n*Chat · exported now*\n\n*This export starts partway through the conversation. Earlier messages aren’t included.*\n"
            ),
            "{text}"
        );
    }

    #[test]
    fn an_export_is_written_whole_beside_a_taken_name() {
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path();
        assert_eq!(free_path(folder, "Notes.md"), folder.join("Notes.md"));
        std::fs::write(folder.join("Notes.md"), "old").unwrap();
        assert_eq!(free_path(folder, "Notes.md"), folder.join("Notes 2.md"));
        write(&folder.join("Notes.md"), "new").unwrap();
        assert_eq!(
            std::fs::read_to_string(folder.join("Notes.md")).unwrap(),
            "new"
        );
        // Nothing is left beside it.
        assert_eq!(std::fs::read_dir(folder).unwrap().count(), 1);
        // A folder that isn't there says so plainly.
        let error = write(&folder.join("gone/Notes.md"), "x").unwrap_err();
        assert_eq!(why(&error), "The folder isn’t there anymore.");
    }

    #[test]
    fn code_spans_and_file_names_survive_odd_text() {
        assert_eq!(code("a `b` c"), "``a `b` c``");
        assert_eq!(code("`x"), "`` `x ``");
        assert_eq!(file_name("Fix: a/b \"quoted\"?"), "Fix a b quoted.md");
        assert_eq!(file_name("  ..  "), "Conversation.md");
        // Names Windows keeps for devices, and invisible characters that
        // turn the name around.
        assert_eq!(file_name("CON"), "Conversation CON.md");
        assert_eq!(file_name("aux"), "Conversation aux.md");
        assert_eq!(file_name("com1"), "Conversation com1.md");
        assert_eq!(file_name("Console"), "Console.md");
        assert_eq!(file_name("report\u{202E}dm.exe"), "reportdm.exe.md");
    }
}
