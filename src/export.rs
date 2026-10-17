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

/// The whole saved conversation as Markdown, with the reports of its
/// handovers after it, and whether it starts partway (after a handover).
/// The session gives it once nothing can still add to it. Render the
/// Markdown off the UI thread.
pub fn with_saved(mut header: Header, saved: arko::sessions::SavedConversation) -> (String, bool) {
    header.starts_partway = !saved.reports.is_empty();
    let mut text = markdown(&header, &saved.items);
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
                    out.push_str(&format!("\n*[Image · {}]*\n", image_label(&image.label)));
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
                let text = protect_details(&closed(trimmed(&item.text)));
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
                    entry.push_str(&format!(" (failed, {})", one_line(why)));
                }
                for image in &item.images {
                    entry.push_str(&format!(" *[Image · {}]*", image_label(&image.label)));
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
                out.push_str(&format!(
                    "\n> **{label}**\n>\n> {}\n",
                    quoted(item.text.trim())
                ));
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
                out.push_str(&format!(
                    "\n*{worker} started — {}*\n",
                    one_line(&item.text)
                ));
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
    use markdown::mdast::Node;

    // Container boundaries already end fenced blocks in quotes and lists.
    // Only a top-level fence can swallow the next speaker heading.
    let tree = markdown::to_mdast(text, &markdown::ParseOptions::default())
        .expect("CommonMark parsing is infallible");
    let code = tree.children().and_then(|children| children.last());
    if let Some(Node::Code(code)) = code
        && let Some(position) = &code.position
        && position.end.offset == text.len()
    {
        let start = text[..position.start.offset]
            .rfind('\n')
            .map_or(0, |offset| offset + 1);
        let source = &text[start..];
        let first = source.lines().next().unwrap_or_default();
        let line = first.trim_start_matches(' ');
        let indent = first.len() - line.len();
        if indent <= 3
            && let Some(mark @ ('`' | '~')) = line.chars().next()
        {
            let length = line.chars().take_while(|c| *c == mark).count();
            if length >= 3 {
                let last = source.lines().last().unwrap_or_default();
                let closing = last.trim_start_matches(' ');
                let count = closing.chars().take_while(|c| *c == mark).count();
                let closed = source.contains('\n')
                    && last.len() - closing.len() <= 3
                    && count >= length
                    && closing[count..].trim().is_empty();
                if !closed {
                    return format!("{text}\n{}", mark.to_string().repeat(length));
                }
            }
        }
    }
    text.to_string()
}

fn protect_details(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (offset, character) in text.char_indices() {
        if text[offset..]
            .get(..9)
            .is_some_and(|tag| tag.eq_ignore_ascii_case("</details"))
        {
            out.push_str("&lt;");
        } else {
            out.push(character);
        }
    }
    out
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
        let ordered = digits > 0 && matches!(rest[digits..].chars().next(), Some('.' | ')'));
        let block = matches!(first, Some('#' | '-' | '+' | '=' | '|' | '~'));
        for (offset, character) in rest.char_indices() {
            if matches!(
                character,
                '\\' | '*' | '_' | '`' | '[' | ']' | '<' | '>' | '&'
            ) || (ordered && offset == digits)
                || (block && offset == 0)
            {
                out.push('\\');
            }
            out.push(character);
        }
        // A line break as typed, before another line of the same paragraph.
        if lines
            .get(index + 1)
            .is_some_and(|next| !next.trim().is_empty())
        {
            out.push('\\');
        }
    }
    out
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
    use std::io::Write;

    let folder = path.parent().filter(|path| !path.as_os_str().is_empty());
    let mut temporary = tempfile::NamedTempFile::new_in(folder.unwrap_or(Path::new(".")))?;
    temporary.write_all(text.as_bytes())?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
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
    use super::{
        Header, STARTS_PARTWAY, code, file_name, free_path, markdown, why, with_saved, write,
    };
    use arko::remote::{CallStatus, Item, Kind, Level};

    fn item(kind: Kind, text: &str) -> Item {
        Item::new(kind, text)
    }

    #[test]
    fn the_saved_conversation_is_exported_whole_with_its_handover_reports() {
        let mut first = item(Kind::User, "The start.");
        first.message_index = Some(0);
        let saved = arko::sessions::SavedConversation {
            number: 1,
            title: "The start.".into(),
            items: vec![first, item(Kind::Reply, "The first reply.")],
            images: Default::default(),
            reports: Vec::new(),
        };
        let header = Header {
            starts_partway: true,
            ..Header::default()
        };
        let (text, partial) = with_saved(header.clone(), saved.clone());
        assert!(!partial);
        assert!(text.contains("The start."));
        assert!(text.contains("The first reply."));
        assert!(!text.contains(STARTS_PARTWAY));
        let mut handed_over = saved;
        handed_over
            .reports
            .push("The earlier work was summarized.".into());
        let (text, partial) = with_saved(header, handed_over);
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
- `$ cargo test -p parse` (failed, exit 101)

> **Warning**
>
> rate limited, retrying

Done: **streaming** now.

*48 s · $0.21*

## You

Thanks.
"
        );
    }

    #[test]
    fn stopped_and_false_fences_keep_later_speakers_outside_code() {
        for reply in [
            "> ```rust\n> fn stopped() {",
            "1. ```rust\n   fn stopped() {",
            "```an `inline` fragment",
            "```text\n> ```\nstill code\n```",
            "```rust\nfn stopped() {",
            "~~~rust\nfn stopped() {",
            "    ```rust\n    indented code",
        ] {
            let text = markdown(
                &Header {
                    title: "Export".into(),
                    ..Header::default()
                },
                &[
                    item(Kind::Reply, reply),
                    item(Kind::User, "next message"),
                    item(Kind::Reply, "next reply"),
                ],
            );
            let tree = markdown::to_mdast(&text, &Default::default()).unwrap();
            let headings = tree
                .children()
                .unwrap()
                .iter()
                .filter(|node| matches!(node, markdown::mdast::Node::Heading(_)))
                .count();
            assert_eq!(headings, 4, "{reply:?}\n{text}");
        }
    }

    #[test]
    fn user_inline_syntax_renders_literally() {
        let original = "Use **literal stars**, _underscores_, `backticks`, &copy; and \\*escaped stars\\*. [a](b) <i>";
        let text = markdown(&Header::default(), &[item(Kind::User, original)]);
        let body = text.split("## You\n").nth(1).unwrap();
        let tree = markdown::to_mdast(body, &Default::default()).unwrap();
        assert_eq!(tree.to_string(), original);
    }

    #[test]
    fn thinking_cannot_close_its_container_with_mixed_case_tags() {
        let text = markdown(
            &Header::default(),
            &[item(Kind::Thinking, "text </Details> more </dEtAiLs >")],
        );
        assert!(!text.contains("</Details>"));
        assert!(!text.contains("</dEtAiLs >"));
        assert_eq!(text.matches("</details>").count(), 1);
    }

    #[test]
    fn attachments_workers_and_notices_keep_their_labels() {
        let image = arko::images::ImageRef {
            id: 1,
            mime: "image/png".into(),
            label: "diagram.png".into(),
            bytes: 42,
        };
        let mut user = item(Kind::User, "Check this.");
        user.images = vec![image.clone()];
        let mut call = item(Kind::Call, "read diagram.png");
        call.images = vec![image];
        let mut worker = item(Kind::WorkerLine, "Check the parser");
        worker.worker = Some("r1".into());
        let text = markdown(
            &Header::default(),
            &[
                user,
                call,
                worker,
                item(Kind::Notice, "The folder is missing.").level(Level::Error),
                item(Kind::Notice, "Try again."),
            ],
        );
        assert_eq!(text.matches("*[Image · diagram.png]*").count(), 2);
        assert!(text.contains("*r1 started — Check the parser*"));
        assert!(text.contains("> **Error**\n>\n> The folder is missing."));
        assert!(text.contains("> **Note**\n>\n> Try again."));
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
see \\<b\\>this\\</b\\>

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
    fn exporting_preserves_unrelated_temporary_names_even_on_failure() {
        let temp = tempfile::tempdir().unwrap();
        let unrelated = temp.path().join(".notes.md.part");
        std::fs::write(&unrelated, "valuable data").unwrap();
        let destination = temp.path().join("notes.md");
        write(&destination, "export").unwrap();
        assert_eq!(
            std::fs::read_to_string(&unrelated).unwrap(),
            "valuable data"
        );
        std::fs::remove_file(&destination).unwrap();
        std::fs::create_dir(&destination).unwrap();
        assert!(write(&destination, "failed export").is_err());
        assert_eq!(
            std::fs::read_to_string(&unrelated).unwrap(),
            "valuable data"
        );
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn exporting_never_follows_an_unrelated_temporary_symlink() {
        let temp = tempfile::tempdir().unwrap();
        let unrelated = temp.path().join("other-document.txt");
        std::fs::write(&unrelated, "valuable data").unwrap();
        let link = temp.path().join(".notes.md.part");
        std::os::unix::fs::symlink(&unrelated, &link).unwrap();
        write(&temp.path().join("notes.md"), "export").unwrap();
        assert_eq!(
            std::fs::read_to_string(&unrelated).unwrap(),
            "valuable data"
        );
        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 3);
    }

    #[test]
    fn concurrent_exports_each_replace_the_destination_whole() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("notes.md");
        let barrier = std::sync::Barrier::new(8);
        std::thread::scope(|scope| {
            for number in 0..8 {
                let destination = &destination;
                let barrier = &barrier;
                scope.spawn(move || {
                    barrier.wait();
                    write(destination, &number.to_string().repeat(16_384)).unwrap();
                });
            }
        });
        let text = std::fs::read_to_string(destination).unwrap();
        assert_eq!(text.len(), 16_384);
        assert!(text.bytes().all(|byte| byte == text.as_bytes()[0]));
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
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
