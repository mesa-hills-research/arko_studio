//! Text folded for find: case folded in full, so "Straße" finds "STRASSE",
//! and composed (Unicode's NFC), so an "é" typed as one character finds one
//! written as "e" and an accent. A match maps back to the bytes of the text
//! as it is, whole characters with their accents.
//!
//! Each character is folded with the marks that follow it, which is all
//! that composing and case folding look at.

use std::ops::Range;

use caseless::Caseless as _;
use unicode_normalization::char::{canonical_combining_class, is_combining_mark};
use unicode_normalization::{IsNormalized, UnicodeNormalization as _, is_nfc_quick};

/// A text folded to find words in, and how to map its bytes back.
pub(crate) struct Folded {
    folded: String,
    /// The pieces (a character and its marks) whose folded bytes don't line
    /// up with the text's, in order. Between them, folded bytes line up with
    /// the text's at the offset the last one leaves.
    moved: Vec<Moved>,
}

/// A piece of the text whose folded bytes have other lengths.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Moved {
    folded: Range<usize>,
    text: Range<usize>,
}

impl Folded {
    pub fn new(text: &str) -> Self {
        if text.is_ascii() {
            return Self {
                folded: text.to_ascii_lowercase(),
                moved: Vec::new(),
            };
        }
        let mut folded = String::with_capacity(text.len());
        let mut moved = Vec::new();
        let mut seen = Seen::default();
        let mut chars = text.char_indices().peekable();
        while let Some((start, first)) = chars.next() {
            let alone = chars.peek().is_none_or(|(_, next)| !seen.joins(*next));
            if alone {
                if first.is_ascii() {
                    folded.push(first.to_ascii_lowercase());
                    continue;
                }
                // (Folded alone, to one character as long, it lines up.)
                if let Some(one) = seen.folded(first)
                    && one.len_utf8() == first.len_utf8()
                {
                    folded.push(one);
                    continue;
                }
            }
            let mut end = start + first.len_utf8();
            while let Some(&(at, next)) = chars.peek().filter(|(_, next)| seen.joins(*next)) {
                end = at + next.len_utf8();
                chars.next();
            }
            let piece = &text[start..end];
            let from = folded.len();
            match seen.folded(first).filter(|_| alone) {
                Some(one) => folded.push(one),
                None => folded.extend(piece.nfd().default_case_fold().nfc()),
            }
            let out = &folded[from..];
            // Bytes that line up: as many, in characters as long.
            let lines_up = out.len() == piece.len()
                && out
                    .chars()
                    .map(char::len_utf8)
                    .eq(piece.chars().map(char::len_utf8));
            if !lines_up {
                moved.push(Moved {
                    folded: from..folded.len(),
                    text: start..end,
                });
            }
        }
        Self { folded, moved }
    }

    /// Every place the folded `needle` is, as byte ranges of the text, one
    /// after another. Matches inside one character that folds to more (an
    /// "s" of "ß") count once, as the whole character.
    pub fn find(&self, needle: &str) -> Vec<Range<usize>> {
        let mut found: Vec<Range<usize>> = Vec::new();
        if needle.is_empty() {
            return found;
        }
        let mut from = 0;
        while let Some(position) = self.folded[from..].find(needle) {
            let start = from + position;
            let end = start + needle.len();
            let range = self.text_start(start)..self.text_end(end);
            if found.last().is_none_or(|last| range.start >= last.end) {
                found.push(range);
            }
            from = end;
        }
        found
    }

    /// The piece moved last at or before the folded byte `at`.
    fn moved_before(&self, at: usize) -> Option<&Moved> {
        let after = self.moved.partition_point(|moved| moved.folded.start <= at);
        after.checked_sub(1).map(|ix| &self.moved[ix])
    }

    /// Where the text's character of the folded byte `at` starts.
    fn text_start(&self, at: usize) -> usize {
        match self.moved_before(at) {
            Some(moved) if at < moved.folded.end => moved.text.start,
            Some(moved) => at - moved.folded.end + moved.text.end,
            None => at,
        }
    }

    /// Where the text's character of the folded byte before `at` ends.
    fn text_end(&self, at: usize) -> usize {
        match self.moved_before(at - 1) {
            Some(moved) if at - 1 < moved.folded.end => moved.text.end,
            Some(moved) => at - moved.folded.end + moved.text.end,
            None => at,
        }
    }
}

/// `words` folded, as the texts are.
pub(crate) fn fold(words: &str) -> String {
    Folded::new(words).folded
}

/// Whether `c` goes with the character before it when folding: a mark, or
/// a Hangul vowel or final consonant that composes with the letters before.
fn joins(c: char) -> bool {
    !c.is_ascii()
        && (matches!(c, '\u{1160}'..='\u{11ff}' | '\u{d7b0}'..='\u{d7ff}')
            || canonical_combining_class(c) != 0
            || is_combining_mark(c))
}

/// The one composed character `c` folds to by itself, if it folds to one.
/// Folding needn't decompose first, but for the characters with U+0345 (a
/// Greek mark that folds to a letter).
fn folded_alone(c: char) -> Option<char> {
    if matches!(c, '\u{345}' | '\u{1f80}'..='\u{1fff}') {
        return None;
    }
    let mut folded = std::iter::once(c).default_case_fold();
    match (folded.next(), folded.next()) {
        (Some(one), None) if is_nfc_quick(std::iter::once(one)) == IsNormalized::Yes => Some(one),
        _ => None,
    }
}

/// What folding learned of the characters it saw last, by their low bits:
/// a text in one script uses few, again and again.
struct Seen {
    chars: [char; SEEN],
    /// What [`folded_alone`] says, NUL for none.
    folded: [char; SEEN],
    joins: [bool; SEEN],
}

const SEEN: usize = 128;

impl Default for Seen {
    fn default() -> Self {
        // (NUL is ASCII, which never comes here: no character matches it.)
        Self {
            chars: ['\0'; SEEN],
            folded: ['\0'; SEEN],
            joins: [false; SEEN],
        }
    }
}

impl Seen {
    fn slot(&mut self, c: char) -> usize {
        let slot = c as usize % SEEN;
        if self.chars[slot] != c {
            self.chars[slot] = c;
            self.folded[slot] = folded_alone(c).unwrap_or('\0');
            self.joins[slot] = joins(c);
        }
        slot
    }

    fn joins(&mut self, c: char) -> bool {
        !c.is_ascii() && {
            let slot = self.slot(c);
            self.joins[slot]
        }
    }

    fn folded(&mut self, c: char) -> Option<char> {
        let slot = self.slot(c);
        Some(self.folded[slot]).filter(|one| *one != '\0')
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find(text: &str, words: &str) -> Vec<Range<usize>> {
        Folded::new(text).find(&fold(words))
    }

    fn found<'a>(text: &'a str, words: &str) -> Vec<&'a str> {
        find(text, words)
            .into_iter()
            .map(|range| &text[range])
            .collect()
    }

    #[test]
    fn case_folds_in_full() {
        assert_eq!(
            found("Straße and STRASSE", "strasse"),
            ["Straße", "STRASSE"]
        );
        assert_eq!(found("Straße and STRASSE", "STRAßE"), ["Straße", "STRASSE"]);
        // A final sigma is a sigma. An accent still counts.
        let greek = "\u{39f}\u{394}\u{39f}\u{3a3}, \u{3bf}\u{3b4}\u{3bf}\u{3c2}, \u{3bf}\u{3b4}\u{3cc}\u{3c2}";
        assert_eq!(
            found(greek, "\u{39f}\u{3b4}\u{3bf}\u{3c2}"),
            [
                "\u{39f}\u{394}\u{39f}\u{3a3}",
                "\u{3bf}\u{3b4}\u{3bf}\u{3c2}"
            ]
        );
        assert_eq!(found("\u{fb01}le, FILE", "file"), ["\u{fb01}le", "FILE"]);
        assert_eq!(found("\u{130}stanbul", "i\u{307}s"), ["\u{130}s"]);
    }

    #[test]
    fn composed_and_decomposed_accents_find_each_other() {
        let composed = "Caf\u{e9} au lait";
        let decomposed = "Cafe\u{301} au lait";
        assert_eq!(found(composed, "CAFE\u{301}"), ["Caf\u{e9}"]);
        assert_eq!(found(decomposed, "caf\u{e9}"), ["Cafe\u{301}"]);
        assert_eq!(found(decomposed, "\u{c9} AU"), ["e\u{301} au"]);
        // Accents count: "cafe" isn't "café".
        assert!(find(composed, "cafe ").is_empty());
        assert!(find(decomposed, "cafe ").is_empty());
        // Hangul written as letters finds the syllables.
        assert_eq!(
            found("\u{1112}\u{1161}\u{11ab} \u{ae00}", "\u{d55c}"),
            ["\u{1112}\u{1161}\u{11ab}"]
        );
    }

    #[test]
    fn a_match_inside_a_character_that_folds_to_more_is_the_character() {
        assert_eq!(found("Straße", "s"), ["S", "ß"]);
        assert_eq!(found("Maß", "ss"), ["ß"]);
        assert_eq!(found("ßs", "ss"), ["ß"]);
        assert_eq!(found("ßßß", "ss"), ["ß", "ß", "ß"]);
    }

    #[test]
    fn matches_after_moved_bytes_map_back() {
        // "ß" folds longer, "é" written as two characters shorter.
        let text = "Maß, Cafe\u{301}, done";
        assert_eq!(found(text, "done"), ["done"]);
        assert_eq!(found(text, "café, d"), ["Cafe\u{301}, d"]);
        // The capital sharp s, the Ångström sign and the ohm sign.
        let text = "\u{1e9e} \u{212b}ngstr\u{f6}m \u{2126}";
        assert_eq!(found(text, "\u{e5}ngstr\u{f6}m"), ["\u{212b}ngstr\u{f6}m"]);
        assert_eq!(found(text, "\u{3c9}"), ["\u{2126}"]);
        assert_eq!(found(text, "ss \u{c5}"), ["\u{1e9e} \u{212b}"]);
    }

    #[test]
    fn plain_text_is_lowercased_in_place() {
        let text = "Parse the PARSER, naïvely";
        assert_eq!(find(text, "parse"), [0..5, 10..15]);
        assert_eq!(found(text, "NAÏVE"), ["naïve"]);
        assert!(Folded::new(text).moved.is_empty());
        assert!(find("text", "").is_empty());
        assert!(find("", "x").is_empty());
    }
}
