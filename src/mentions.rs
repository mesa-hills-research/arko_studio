//! @-mentions in an agent session's composer: the files of the session's
//! folder, listed off the UI thread as git sees them (ignored files left
//! out), and ranked for what follows the "@".

use std::path::Path;

/// The most files a folder lists: a mention of a file deeper in a larger
/// tree is typed out.
pub const MAX_FILES: usize = 50_000;

/// The most files and folders listing a folder looks at, to keep the
/// shallowest [`MAX_FILES`] of a larger tree.
const VISITS: usize = 200_000;

/// The files under `root`, relative to it with `/` between folders: what
/// `.gitignore`, `.ignore` and git's excludes leave, hidden files included,
/// `.git` itself left out. In a larger tree, the shallowest. Blocking: run
/// it off the UI thread.
pub fn list(root: &Path) -> Vec<String> {
    list_within(root, MAX_FILES, VISITS)
}

/// The shallowest `most` files under `root`, of the first `visits` files
/// and folders seen. Hidden folders (`.cache`, `.venv`) are looked in
/// after the rest, and their files come last.
fn list_within(root: &Path, most: usize, visits: usize) -> Vec<String> {
    use std::collections::BinaryHeap;
    use std::sync::{Arc, Mutex};
    // The shallowest seen, the one to give way first on top.
    let mut kept: BinaryHeap<(bool, usize, String)> = BinaryHeap::new();
    let mut visited = 0;
    let hidden_folders: Arc<Mutex<Vec<std::path::PathBuf>>> = Arc::default();
    let mut walk = |start: &Path, hidden: bool, kept: &mut BinaryHeap<(bool, usize, String)>| {
        let later = hidden_folders.clone();
        let walker = ignore::WalkBuilder::new(start)
            .hidden(false)
            .require_git(false)
            .filter_entry(move |entry| {
                if entry.file_name() == ".git" {
                    return false;
                }
                let folder = entry.file_type().is_some_and(|kind| kind.is_dir());
                let dot = entry.file_name().to_string_lossy().starts_with('.');
                if !hidden && folder && dot && entry.depth() > 0 {
                    if let Ok(mut later) = later.lock() {
                        later.push(entry.path().to_path_buf());
                    }
                    return false;
                }
                true
            })
            .build();
        for entry in walker.filter_map(Result::ok) {
            visited += 1;
            if visited > visits {
                break;
            }
            if !entry.file_type().is_some_and(|kind| kind.is_file()) {
                continue;
            }
            let Ok(relative) = entry.path().strip_prefix(root) else {
                continue;
            };
            let path = relative.to_string_lossy().replace('\\', "/");
            let key = (hidden, depth(&path), path);
            if kept.len() < most {
                kept.push(key);
            } else if kept.peek().is_some_and(|deepest| key < *deepest) {
                kept.pop();
                kept.push(key);
            }
        }
    };
    walk(root, false, &mut kept);
    let later = std::mem::take(
        &mut *hidden_folders
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    );
    for folder in later {
        walk(&folder, true, &mut kept);
    }
    kept.into_sorted_vec()
        .into_iter()
        .map(|(_, _, path)| path)
        .collect()
}

fn depth(path: &str) -> usize {
    path.matches('/').count()
}

/// A folder's files, made ready to rank once when they're listed: each
/// path's lowercase form and where its name starts, so a keystroke never
/// works them out again.
#[derive(Debug, Default)]
pub struct Files {
    listed: Vec<Listed>,
}

#[derive(Debug)]
struct Listed {
    path: String,
    /// The path lowercased, for a path all ASCII (its bytes stay where
    /// they were).
    lower: Option<Box<str>>,
    /// Where the file's own name starts.
    name_start: usize,
}

impl Files {
    pub fn new(paths: Vec<String>) -> Self {
        let listed = paths
            .into_iter()
            .map(|path| Listed {
                lower: path.is_ascii().then(|| path.to_ascii_lowercase().into()),
                name_start: path.rfind('/').map_or(0, |slash| slash + 1),
                path,
            })
            .collect();
        Self { listed }
    }

    pub fn len(&self) -> usize {
        self.listed.len()
    }

    pub fn is_empty(&self) -> bool {
        self.listed.is_empty()
    }

    /// How many files are named `name`.
    pub fn named(&self, name: &str) -> usize {
        self.listed
            .iter()
            .filter(|listed| &listed.path[listed.name_start..] == name)
            .count()
    }
}

/// What ranking found: the best files, and every file that matched (by
/// its place in the list), among which a longer query is ranked next.
#[derive(Debug, Default)]
pub struct Ranking {
    pub ranked: Vec<Ranked>,
    pub matched: Vec<u32>,
}

/// `words` lowercased, as ranking compares them.
pub fn lowercase(words: &str) -> String {
    words.trim().chars().flat_map(char::to_lowercase).collect()
}

/// A file that matches what was typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ranked {
    pub path: String,
    /// The bytes of `path` the typed characters matched.
    pub positions: Vec<usize>,
    /// It matches strongly: the characters in a row, or each starting a
    /// word.
    pub strong: bool,
}

/// The files of `files` that hold the characters of `query` in order
/// (ignoring case), best first, at most `limit`: matches in the file's
/// own name, at the start of a word and in a row count most. With nothing
/// typed, the first files (the shallowest). Only `among` are looked at
/// when given: those an earlier query matched, which a longer one extends.
pub fn rank(files: &Files, among: Option<&[u32]>, query: &str, limit: usize) -> Ranking {
    let query = lowercase(query);
    if query.is_empty() {
        return Ranking {
            ranked: files
                .listed
                .iter()
                .take(limit)
                .map(|listed| Ranked {
                    path: listed.path.clone(),
                    positions: Vec::new(),
                    strong: false,
                })
                .collect(),
            matched: Vec::new(),
        };
    }
    let query: Vec<char> = query.chars().collect();
    let all = 0..files.listed.len() as u32;
    let indices: Box<dyn Iterator<Item = u32>> = match among {
        Some(among) => Box::new(among.iter().copied()),
        None => Box::new(all),
    };
    // Scored without noting where the characters are: only the files
    // shown need that.
    let mut matched = Vec::new();
    let mut scored: Vec<(i64, u32)> = indices
        .filter_map(|index| {
            let listed = files.listed.get(index as usize)?;
            let (score, _) = score(listed, &query, None)?;
            matched.push(index);
            Some((score, index))
        })
        .collect();
    // Best first, then shorter paths, then by name: the same order each time.
    let path = |index: u32| files.listed[index as usize].path.as_str();
    let order = |a: &(i64, u32), b: &(i64, u32)| {
        b.0.cmp(&a.0)
            .then_with(|| path(a.1).len().cmp(&path(b.1).len()))
            .then_with(|| path(a.1).cmp(path(b.1)))
    };
    // Only the best `limit` are put in order.
    if scored.len() > limit && limit > 0 {
        scored.select_nth_unstable_by(limit - 1, order);
        scored.truncate(limit);
    }
    scored.sort_by(order);
    let ranked = scored
        .into_iter()
        .take(limit)
        .map(|(_, index)| {
            let listed = &files.listed[index as usize];
            let mut positions = Vec::with_capacity(query.len());
            let strong =
                score(listed, &query, Some(&mut positions)).is_some_and(|(_, strong)| strong);
            Ranked {
                path: listed.path.clone(),
                positions,
                strong,
            }
        })
        .collect();
    Ranking { ranked, matched }
}

/// How a file's characters matched a query.
#[derive(Default)]
struct Scan {
    count: i64,
    word_starts: i64,
    in_a_row: i64,
    first: Option<usize>,
}

/// How well a file matches `query` (lowercase), and whether strongly (its
/// letters in a row, or each starting a word): `None` when its characters
/// aren't all in it, in order. Where they matched goes to `positions`
/// when given.
fn score(
    listed: &Listed,
    query: &[char],
    mut positions: Option<&mut Vec<usize>>,
) -> Option<(i64, bool)> {
    let name_start = listed.name_start;
    // In the file's own name first, else anywhere in the path.
    let (scan, in_name) = match scan(listed, query, name_start, positions.as_deref_mut()) {
        Some(scan) => (scan, true),
        None => {
            if let Some(positions) = positions.as_deref_mut() {
                positions.clear();
            }
            (scan(listed, query, 0, positions)?, false)
        }
    };
    let mut score = scan.count + 10 * scan.word_starts + 5 * scan.in_a_row;
    // Initials: every character typed starts a word ("pm" for parser/mod).
    if scan.count > 1 && scan.word_starts == scan.count {
        score += 15;
    }
    if in_name {
        score += 10;
        if scan.first == Some(name_start) {
            score += 10;
        }
    }
    score -= (listed.path.len() / 16) as i64;
    let strong = scan.in_a_row + 1 == scan.count || scan.word_starts == scan.count;
    Some((score, strong))
}

/// The earliest match of `query` in the file from byte `from` on: each
/// character after the last. `None` if they aren't all there.
fn scan(
    listed: &Listed,
    query: &[char],
    from: usize,
    mut positions: Option<&mut Vec<usize>>,
) -> Option<Scan> {
    let path = listed.path.as_str();
    let bytes = path.as_bytes();
    let mut scan = Scan::default();
    let mut previous: Option<usize> = None;
    let mut note = |at: usize, scan: &mut Scan| {
        scan.count += 1;
        scan.first.get_or_insert(at);
        let word_start = at == 0
            || matches!(bytes.get(at - 1), Some(b'/' | b'_' | b'-' | b'.' | b' '))
            || (bytes[at].is_ascii_uppercase() && bytes[at - 1].is_ascii_lowercase());
        if word_start {
            scan.word_starts += 1;
        }
        let next_to_previous = previous.is_some_and(|previous| {
            at == previous + path[previous..].chars().next().map_or(1, char::len_utf8)
        });
        if next_to_previous {
            scan.in_a_row += 1;
        }
        previous = Some(at);
        if let Some(positions) = positions.as_deref_mut() {
            positions.push(at);
        }
    };
    // Most paths and most of what is typed are ASCII: the lowercase bytes,
    // compared without allocating.
    if let Some(lower) = &listed.lower
        && query.iter().all(char::is_ascii)
    {
        let mut wanted = query.iter().map(|character| *character as u8).peekable();
        for (at, byte) in lower.as_bytes()[from..].iter().enumerate() {
            match wanted.peek() {
                Some(next) if next == byte => {
                    note(from + at, &mut scan);
                    wanted.next();
                }
                Some(_) => {}
                None => break,
            }
        }
        return wanted.peek().is_none().then_some(scan);
    }
    let mut wanted = query.iter().peekable();
    for (at, character) in path[from..].char_indices() {
        let Some(next) = wanted.peek() else {
            break;
        };
        if character.to_lowercase().eq(std::iter::once(**next)) {
            note(from + at, &mut scan);
            wanted.next();
        }
    }
    wanted.peek().is_none().then_some(scan)
}

/// What is being mentioned in `text` with the caret at `caret`: the range
/// from the "@" to the caret, and what was typed after the "@". The "@"
/// starts the text or a word (after a space, or an opening bracket or
/// quote), and nothing typed after it is a space.
pub fn typed(text: &str, caret: usize) -> Option<(std::ops::Range<usize>, &str)> {
    let before = text.get(..caret)?;
    let at = before.rfind('@')?;
    let typed = &before[at + 1..];
    if typed.chars().any(char::is_whitespace) {
        return None;
    }
    let starts_word = before[..at].chars().next_back().is_none_or(|previous| {
        previous.is_whitespace()
            || matches!(previous, '(' | '[' | '{' | '"' | '\'' | '`' | '“' | '‘')
    });
    starts_word.then_some((at..caret, typed))
}

#[cfg(test)]
mod tests {
    use super::{Files, list, rank, typed};

    fn files(paths: &[&str]) -> Files {
        Files::new(paths.iter().map(|path| path.to_string()).collect())
    }

    fn ranked(files: &Files, query: &str, limit: usize) -> Vec<super::Ranked> {
        rank(files, None, query, limit).ranked
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

    /// A folder with more files than the list keeps: the shallowest stay,
    /// and hidden folders' files come last.
    #[test]
    fn a_large_folder_keeps_its_shallowest_files() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path();
        for data in ["aaa_data", "bbb_data", "ccc_data"] {
            let deep = root.join(data).join("x/y");
            std::fs::create_dir_all(&deep).unwrap();
            for n in 0..30 {
                std::fs::write(deep.join(format!("{n}.csv")), "x").unwrap();
            }
        }
        std::fs::create_dir_all(root.join(".cache/pip")).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        for file in [
            "README.md",
            "Cargo.toml",
            "src/main.rs",
            ".cache/pip/a",
            ".env",
        ] {
            std::fs::write(root.join(file), "x").unwrap();
        }
        let listed = super::list_within(root, 6, 10_000);
        assert_eq!(listed.len(), 6);
        for shallow in [".env", "Cargo.toml", "README.md", "src/main.rs"] {
            assert!(
                listed.contains(&shallow.to_string()),
                "{shallow} in {listed:?}"
            );
        }
        assert!(!listed.contains(&".cache/pip/a".to_string()));
        // With room, a hidden folder's files come last.
        let all = super::list_within(root, 1000, 10_000);
        assert_eq!(all.last().map(String::as_str), Some(".cache/pip/a"));
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
        let found: Vec<String> = ranked(&files, "parse", 10)
            .into_iter()
            .map(|ranked| ranked.path)
            .collect();
        assert_eq!(found[0], "src/parse.rs");
        assert_eq!(found[1], "tests/parse_tests.rs");
        assert!(found.contains(&"src/parser/mod.rs".to_string()));
        assert!(!found.contains(&"src/main.rs".to_string()));
        let initials = ranked(&files, "PM", 10);
        assert_eq!(initials[0].path, "src/parser/mod.rs");
        assert_eq!(ranked(&files, "", 2).len(), 2);
        assert_eq!(ranked(&files, "src/pa", 10)[0].path, "src/parse.rs");
    }

    /// Typing on ranks only the files the shorter words matched, and
    /// finds what a full ranking would.
    #[test]
    fn longer_words_rank_among_what_the_shorter_ones_matched() {
        let paths: Vec<String> = (0..2000)
            .map(|n| format!("crates/part_{n}/src/module_{}.rs", n % 37))
            .chain(["src/parser/mod.rs".to_string(), "README.md".to_string()])
            .collect();
        let files = Files::new(paths);
        let first = rank(&files, None, "pa", 50);
        assert!(first.matched.len() < files.len());
        for longer in ["par", "pars", "parse", "parser/m"] {
            let narrowed = rank(&files, Some(&first.matched), longer, 50);
            let full = rank(&files, None, longer, 50);
            assert_eq!(narrowed.ranked, full.ranked, "{longer}");
            assert_eq!(narrowed.matched, full.matched, "{longer}");
        }
    }

    #[test]
    fn a_mention_is_an_at_starting_a_word() {
        assert_eq!(typed("see @src/ma", 11), Some((4..11, "src/ma")));
        assert_eq!(typed("@", 1), Some((0..1, "")));
        assert_eq!(typed("mail me@home", 12), None);
        assert_eq!(typed("@src/main.rs and", 16), None);
        assert_eq!(typed("see @src", 3), None);
        assert_eq!(typed("(@src", 5), Some((1..5, "src")));
        assert_eq!(typed("“@doc", 7), Some((3..7, "doc")));
    }
}
