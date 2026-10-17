//! @-mentions in an agent session's composer: the files of the session's
//! folder, listed off the UI thread as git sees them (ignored files left
//! out), and ranked for what follows the "@".

use std::path::Path;

/// The most files a folder lists: a mention of a file deeper in a larger
/// tree is typed out.
pub const MAX_FILES: usize = 50_000;

/// The files under `root`, relative to it with `/` between folders: what
/// `.gitignore`, `.ignore` and git's excludes leave, hidden files included,
/// `.git` itself left out. Blocking: run it off the UI thread.
pub fn list(root: &Path) -> Vec<String> {
    let mut files: Vec<String> = ignore::WalkBuilder::new(root)
        .hidden(false)
        .require_git(false)
        .filter_entry(|entry| entry.file_name() != ".git")
        .build()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()))
        .filter_map(|entry| {
            let relative = entry.path().strip_prefix(root).ok()?;
            Some(relative.to_string_lossy().replace('\\', "/"))
        })
        .take(MAX_FILES)
        .collect();
    files.sort_by(|a, b| depth(a).cmp(&depth(b)).then_with(|| a.cmp(b)));
    files
}

fn depth(path: &str) -> usize {
    path.matches('/').count()
}

/// A file that matches what was typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ranked {
    pub path: String,
    /// The bytes of `path` the typed characters matched.
    pub positions: Vec<usize>,
}

/// The files of `files` that hold the characters of `query` in order
/// (ignoring case), best first, at most `limit`: matches in the file's
/// own name, at the start of a word and in a row count most. With nothing
/// typed, the first files (the shallowest).
pub fn rank(files: &[String], query: &str, limit: usize) -> Vec<Ranked> {
    let query = query.trim();
    if query.is_empty() {
        return files
            .iter()
            .take(limit)
            .map(|path| Ranked {
                path: path.clone(),
                positions: Vec::new(),
            })
            .collect();
    }
    let query: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
    let mut scored: Vec<(i64, &String, Vec<usize>)> = files
        .iter()
        .filter_map(|path| {
            let (score, positions) = score(path, &query)?;
            Some((score, path, positions))
        })
        .collect();
    // Best first, then shorter paths, then by name: the same order each time.
    let order = |a: &(i64, &String, Vec<usize>), b: &(i64, &String, Vec<usize>)| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.len().cmp(&b.1.len()))
            .then_with(|| a.1.cmp(b.1))
    };
    // Only the best `limit` are put in order.
    if scored.len() > limit && limit > 0 {
        scored.select_nth_unstable_by(limit - 1, order);
        scored.truncate(limit);
    }
    scored.sort_by(order);
    scored
        .into_iter()
        .take(limit)
        .map(|(_, path, positions)| Ranked {
            path: path.clone(),
            positions,
        })
        .collect()
}

/// How well `path` matches `query` (lowercase), and where: `None` when its
/// characters aren't all in it, in order.
fn score(path: &str, query: &[char]) -> Option<(i64, Vec<usize>)> {
    let name_start = path.rfind('/').map_or(0, |slash| slash + 1);
    // In the file's own name first, else anywhere in the path.
    let (positions, in_name) = match matches(path, query, name_start) {
        Some(positions) => (positions, true),
        None => (matches(path, query, 0)?, false),
    };
    let bytes = path.as_bytes();
    let mut score: i64 = 0;
    let mut previous: Option<usize> = None;
    let mut word_starts = 0;
    for &at in &positions {
        score += 1;
        let word_start = at == 0
            || matches!(bytes.get(at - 1), Some(b'/' | b'_' | b'-' | b'.' | b' '))
            || (bytes[at].is_ascii_uppercase() && bytes[at - 1].is_ascii_lowercase());
        if word_start {
            score += 10;
            word_starts += 1;
        }
        let next_to_previous = previous.is_some_and(|previous| {
            at == previous + path[previous..].chars().next().map_or(1, char::len_utf8)
        });
        if next_to_previous {
            score += 5;
        }
        previous = Some(at);
    }
    // Initials: every character typed starts a word ("pm" for parser/mod).
    if positions.len() > 1 && word_starts == positions.len() {
        score += 15;
    }
    if in_name {
        score += 10;
        if positions.first() == Some(&name_start) {
            score += 10;
        }
    }
    score -= (path.len() / 16) as i64;
    Some((score, positions))
}

/// The bytes where the characters of `query` are in `path` from `from` on,
/// each after the last, the earliest possible: `None` if they aren't all
/// there.
fn matches(path: &str, query: &[char], from: usize) -> Option<Vec<usize>> {
    // Most paths and most of what is typed are ASCII: bytes, compared
    // without allocating for the many paths that don't match.
    if path.is_ascii() && query.iter().all(char::is_ascii) {
        let bytes = &path.as_bytes()[from..];
        let mut wanted = query.iter().map(|character| *character as u8);
        let mut next = wanted.next();
        for byte in bytes {
            if next == Some(byte.to_ascii_lowercase()) {
                next = wanted.next();
            }
        }
        if next.is_some() {
            return None;
        }
        let mut positions = Vec::with_capacity(query.len());
        let mut wanted = query.iter().map(|character| *character as u8).peekable();
        for (at, byte) in bytes.iter().enumerate() {
            if wanted.peek() == Some(&byte.to_ascii_lowercase()) {
                positions.push(from + at);
                wanted.next();
            }
        }
        return Some(positions);
    }
    let mut positions = Vec::with_capacity(query.len());
    let mut wanted = query.iter().peekable();
    for (at, character) in path[from..].char_indices() {
        let Some(next) = wanted.peek() else {
            break;
        };
        if character.to_lowercase().eq(std::iter::once(**next)) {
            positions.push(from + at);
            wanted.next();
        }
    }
    wanted.peek().is_none().then_some(positions)
}

/// What is being mentioned in `text` with the caret at `caret`: the range
/// from the "@" to the caret, and what was typed after the "@". The "@"
/// starts the text or follows a space, and nothing typed after it is a
/// space.
pub fn typed(text: &str, caret: usize) -> Option<(std::ops::Range<usize>, &str)> {
    let before = text.get(..caret)?;
    let at = before.rfind('@')?;
    let typed = &before[at + 1..];
    if typed.chars().any(char::is_whitespace) {
        return None;
    }
    let starts_word = before[..at]
        .chars()
        .next_back()
        .is_none_or(char::is_whitespace);
    starts_word.then_some((at..caret, typed))
}

#[cfg(test)]
mod tests {
    use super::{list, rank, typed};

    fn files(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|path| path.to_string()).collect()
    }

    #[test]
    fn the_folder_lists_what_git_would_see() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path();
        std::fs::create_dir_all(root.join("src/deep")).unwrap();
        std::fs::create_dir_all(root.join("target/debug")).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        for file in [
            "Cargo.toml",
            "src/main.rs",
            "src/deep/parse.rs",
            "target/debug/app",
            ".gitignore",
            ".git/HEAD",
            "notes.log",
        ] {
            std::fs::write(root.join(file), "x").unwrap();
        }
        std::fs::write(root.join(".gitignore"), "target/\n*.log\n").unwrap();
        assert_eq!(
            list(root),
            [
                ".gitignore",
                "Cargo.toml",
                "src/main.rs",
                "src/deep/parse.rs"
            ]
        );
    }

    #[test]
    fn names_rank_above_folders_and_word_starts_above_the_rest() {
        let files = files(&[
            "src/main.rs",
            "src/parser/mod.rs",
            "src/parse.rs",
            "docs/compare.md",
            "tests/parse_tests.rs",
        ]);
        let found: Vec<String> = rank(&files, "parse", 10)
            .into_iter()
            .map(|ranked| ranked.path)
            .collect();
        assert_eq!(found[0], "src/parse.rs");
        assert_eq!(found[1], "tests/parse_tests.rs");
        assert!(found.contains(&"src/parser/mod.rs".to_string()));
        assert!(!found.contains(&"src/main.rs".to_string()));
        let ranked = rank(&files, "PM", 10);
        assert_eq!(ranked[0].path, "src/parser/mod.rs");
        assert_eq!(rank(&files, "", 2).len(), 2);
        assert_eq!(rank(&files, "src/pa", 10)[0].path, "src/parse.rs");
    }

    #[test]
    fn a_mention_is_an_at_starting_a_word() {
        assert_eq!(typed("see @src/ma", 11), Some((4..11, "src/ma")));
        assert_eq!(typed("@", 1), Some((0..1, "")));
        assert_eq!(typed("mail me@home", 12), None);
        assert_eq!(typed("@src/main.rs and", 16), None);
        assert_eq!(typed("see @src", 3), None);
    }
}
