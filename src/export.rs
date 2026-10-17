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
                let mut text = item.text.trim().to_string();
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
                out.push_str(&format!("\n{}\n", item.text.trim()));
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
                out.push_str(&format!(
                    "\n<details><summary>Thinking</summary>\n\n{}\n\n</details>\n",
                    item.text.trim()
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
                out.push_str(&format!("\n```text\n{}\n```\n", item.text.trim_end()));
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

    #[test]
    fn code_spans_and_file_names_survive_odd_text() {
        assert_eq!(code("a `b` c"), "``a `b` c``");
        assert_eq!(code("`x"), "`` `x ``");
        assert_eq!(file_name("Fix: a/b \"quoted\"?"), "Fix a b quoted.md");
        assert_eq!(file_name("  ..  "), "Conversation.md");
    }
}
