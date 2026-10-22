//! What a tool call is, for its icon and for a line that sums up a run of
//! calls ("Read 5 files, ran 2 commands").

use arko::remote::Item;
use gpui_kit::assets::IconName;

/// The kind of tool a call used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tool {
    Shell,
    Read,
    Write,
    Edit,
    List,
    Search,
    Workers,
    Monitor,
    Web,
    Mcp,
    Skill,
    Other,
}

impl Tool {
    /// The tool of the call `item`: by the tool's name, or, for an item
    /// without one (the feed keeps names for the latest calls only), by its
    /// line ("read src/main.rs:1-40", "cargo test").
    pub fn of(item: &Item) -> Self {
        match item.tool.as_deref() {
            Some(name) => Self::named(name),
            None => Self::by_line(&item.text),
        }
    }

    fn named(name: &str) -> Self {
        match name {
            "shell" | "terminal_start" | "terminal_status" | "terminal_input"
            | "terminal_control" => Tool::Shell,
            "read_file" => Tool::Read,
            "write_file" => Tool::Write,
            "edit_file" | "apply_diff" => Tool::Edit,
            "list_files" => Tool::List,
            "index_search" | "index_status" | "index_create" | "index_manage" => Tool::Search,
            "start_agent" | "message_agent" | "wait_for_agents" | "list_agents" | "stop_agent"
            | "run_workflow_agent" => Tool::Workers,
            "monitor_start" | "monitor_control" => Tool::Monitor,
            "load_skill" => Tool::Skill,
            name => match name
                .strip_prefix("mcp__")
                .and_then(|rest| rest.split_once("__"))
            {
                Some((server, tool)) => {
                    if is_web(server) || is_web(tool) {
                        Tool::Web
                    } else {
                        Tool::Mcp
                    }
                }
                None if is_web(name) => Tool::Web,
                None => Tool::Other,
            },
        }
    }

    fn by_line(line: &str) -> Self {
        let line = line.trim_start();
        if line.starts_with("(in ") {
            return Tool::Shell;
        }
        if let Some((name, rest)) = line.split_once(" (")
            && !name.contains(char::is_whitespace)
            && let Some((server, arguments)) = rest.split_once(')')
            && !server.is_empty()
            && (arguments.trim().is_empty() || arguments.trim_start().starts_with('{'))
        {
            return if is_web(server) || is_web(name) {
                Tool::Web
            } else {
                Tool::Mcp
            };
        }
        let mut words = line.split_whitespace();
        let first = words.next().unwrap_or_default();
        let second = words.next().unwrap_or_default();
        match first {
            "read" => Tool::Read,
            "write" => Tool::Write,
            "edit" => Tool::Edit,
            "list" if second == "workers" => Tool::Workers,
            "list" => Tool::List,
            "start" if second == "worker" || second.starts_with("worker") => Tool::Workers,
            "worker:" | "worker" => Tool::Workers,
            "message" | "waiting" => Tool::Workers,
            "stop"
                if second.starts_with(['r', 'w']) && second[1..].starts_with(char::is_numeric) =>
            {
                Tool::Workers
            }
            "skill" => Tool::Skill,
            "monitor" if second.starts_with('$') || ["list", "rearm", "stop"].contains(&second) => {
                Tool::Monitor
            }
            "index" => Tool::Search,
            "handover" => Tool::Other,
            "background" | "terminal" => Tool::Shell,
            server if server.len() > 1 && server.ends_with(':') => {
                if is_web(server) || is_web(second) {
                    Tool::Web
                } else {
                    Tool::Mcp
                }
            }
            _ => Tool::Shell,
        }
    }

    /// The Lucide icon of the tool.
    pub fn icon(self) -> IconName {
        match self {
            Tool::Shell => IconName::SquareTerminal,
            Tool::Read => IconName::FileText,
            Tool::Write | Tool::Edit => IconName::FilePen,
            Tool::List => IconName::FolderTree,
            Tool::Search => IconName::Search,
            Tool::Workers => IconName::Bot,
            Tool::Monitor => IconName::Activity,
            Tool::Web => IconName::Globe,
            Tool::Mcp => IconName::Plug,
            Tool::Skill => IconName::BookOpen,
            Tool::Other => IconName::Wrench,
        }
    }

    /// The tool's part of a summary: "read 5 files".
    fn phrase(self, count: usize) -> String {
        let (verb, one, many) = match self {
            Tool::Shell => ("ran", "command", "commands"),
            Tool::Read => ("read", "file", "files"),
            Tool::Write => ("wrote", "file", "files"),
            Tool::Edit => ("made", "edit", "edits"),
            Tool::List => ("listed", "folder", "folders"),
            Tool::Search => ("searched", "time", "times"),
            Tool::Workers => ("used workers", "time", "times"),
            Tool::Monitor => ("used monitors", "time", "times"),
            Tool::Web => ("made", "web request", "web requests"),
            Tool::Mcp => ("made", "MCP call", "MCP calls"),
            Tool::Skill => ("loaded", "skill", "skills"),
            Tool::Other => ("made", "other call", "other calls"),
        };
        format!("{verb} {count} {}", if count == 1 { one } else { many })
    }
}

/// Whether a tool's or a server's name says it reaches the web.
fn is_web(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    ["web", "fetch", "browser", "http", "url"]
        .iter()
        .any(|word| name.contains(word))
}

/// "Read 5 files, ran 2 commands": the calls `items`, by tool, in the order
/// each tool first came.
pub(crate) fn summary<'a>(items: impl IntoIterator<Item = &'a Item>) -> String {
    let mut counts: Vec<(Tool, usize)> = Vec::new();
    for item in items {
        let tool = Tool::of(item);
        match counts.iter_mut().find(|(seen, _)| *seen == tool) {
            Some((_, count)) => *count += 1,
            None => counts.push((tool, 1)),
        }
    }
    let text = counts
        .into_iter()
        .map(|(tool, count)| tool.phrase(count))
        .collect::<Vec<_>>()
        .join(", ");
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arko::remote::Kind;

    fn call(line: &str) -> Item {
        Item::new(Kind::Call, line)
    }

    #[test]
    fn calls_are_told_apart_by_their_line() {
        let cases = [
            ("read src/main.rs:10-40", Tool::Read),
            ("write a.rs (2 lines)", Tool::Write),
            ("edit a.rs (all)", Tool::Edit),
            ("list .", Tool::List),
            ("list workers", Tool::Workers),
            ("start worker (worktree): Fix the parser.", Tool::Workers),
            ("message r1: use plan B", Tool::Workers),
            ("waiting for the workers", Tool::Workers),
            ("stop r2", Tool::Workers),
            ("skill rust-style", Tool::Skill),
            ("index search: where are retries", Tool::Search),
            ("github: create_issue {\"title\":\"x\"}", Tool::Mcp),
            ("create_issue (github) {\"title\":\"x\"}", Tool::Mcp),
            ("read (github) {}", Tool::Mcp),
            ("create_issue (github)", Tool::Mcp),
            ("fetch (web) {\"url\":\"https://a.b\"}", Tool::Web),
            ("start worker “Fix the parser”", Tool::Workers),
            ("start worker (worktree) “Fix the parser”", Tool::Workers),
            ("message r1 “use plan B”", Tool::Workers),
            ("index search “where are retries”", Tool::Search),
            ("monitor $ npm run dev", Tool::Monitor),
            ("monitor rearm", Tool::Monitor),
            ("monitor-disk.sh", Tool::Shell),
            ("fetch: fetch {\"url\":\"https://a.b\"}", Tool::Web),
            ("cargo test -p arko", Tool::Shell),
            ("(in crates/kit) cargo build", Tool::Shell),
            ("stop-the-world.sh", Tool::Shell),
        ];
        for (line, tool) in cases {
            assert_eq!(Tool::of(&call(line)), tool, "{line}");
        }
    }

    #[test]
    fn a_calls_tool_name_comes_first() {
        let named = |line: &str, name: &str| {
            let mut item = call(line);
            item.tool = Some(name.into());
            Tool::of(&item)
        };
        assert_eq!(named("read me.sh", "shell"), Tool::Shell);
        assert_eq!(named("edit a.rs", "edit_file"), Tool::Edit);
        assert_eq!(named("x", "mcp__github__create_issue"), Tool::Mcp);
        assert_eq!(named("x", "mcp__browser__navigate"), Tool::Web);
        assert_eq!(
            named("monitor $ npm run dev", "monitor_start"),
            Tool::Monitor
        );
        assert_eq!(named("x", "something_new"), Tool::Other);
    }

    #[test]
    fn a_run_is_summed_up_by_tool_in_order() {
        let calls = [
            call("read a.rs"),
            call("cargo test"),
            call("read b.rs"),
            call("edit a.rs"),
        ];
        assert_eq!(summary(&calls), "Read 2 files, ran 1 command, made 1 edit");
        assert_eq!(summary(&[]), "");
    }
}
