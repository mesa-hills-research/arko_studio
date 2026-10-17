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
                let from = match item.from.as_deref() {
                    Some(arko::remote::MAIN_AGENT) | None => "the lead".to_string(),
                    Some(worker) => worker.to_string(),
                };
                out.push_str(&format!("\n*From {from}:*\n\n{}\n", item.text.trim()));
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
    } else {
        format!("{name}.md")
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
    use super::{Header, code, file_name, markdown};
    use arko::remote::{CallStatus, Item, Kind, Level};

    fn item(kind: Kind, text: &str) -> Item {
        Item::new(kind, text)
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

    #[test]
    fn code_spans_and_file_names_survive_odd_text() {
        assert_eq!(code("a `b` c"), "``a `b` c``");
        assert_eq!(code("`x"), "`` `x ``");
        assert_eq!(file_name("Fix: a/b \"quoted\"?"), "Fix a b quoted.md");
        assert_eq!(file_name("  ..  "), "Conversation.md");
    }
}
