//! What a row's height will be before the list draws it.
//!
//! The list measures a row when it draws it. For the rows it hasn't drawn,
//! its scrollbar counts the heights estimated here, so the closer they are,
//! the more evenly the thumb moves and the less its size changes as the
//! reader scrolls onto new rows. An estimate is made from what is known of
//! a row without laying it out:
//!
//! - rows of fixed layout (a separator, a call's line, a footer, a worker's
//!   card, a thumbnail) exactly, from the theme's sizes
//! - code and a call's output as their lines, as many as the row shows,
//!   times the code's line height
//! - prose (a message, a paragraph, a heading, a list, a thinking's text)
//!   as the lines its characters wrap to at the row's width, times the line
//!   height, from the font's average character width.
//!
//! Each font's average character width is measured once with the text
//! system, from a sample of prose, and again when the fonts, their sizes or
//! the zoom change. The characters of a row's text are counted once, when
//! the text comes or changes, so an estimate costs a few multiplications.

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::*;

/// How tall a line of text is, as a share of its font's size (GPUI's
/// default line height, which the transcript's text keeps).
const LINE_HEIGHT: f32 = 1.618_034;

/// The prose the fonts' average character width is measured on, sentences
/// as an agent writes them with names in code among them.
const SAMPLE: &str = "The parser keeps the block open while text streams in, and \
    `Parser::feed` returns the blocks it completed. A finished block is never \
    parsed again, so a long answer costs what its last block costs. Run the \
    tests with cargo test -p parse, then check that every one of them passes \
    before you commit the change and push it to the branch for review.";

/// A font's average character width, and how many characters a wrapped
/// line leaves unused at its end, on average (the line breaks between
/// words).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Font {
    /// The size it was measured at.
    size: f32,
    advance: f32,
    waste: f32,
}

impl Font {
    /// The lines `chars` characters wrap to in `width` pixels.
    pub fn lines(&self, chars: u32, width: f32) -> f32 {
        if chars == 0 {
            return 1.;
        }
        let per_line = (width / self.advance - self.waste).max(1.);
        (chars as f32 / per_line).ceil()
    }

    /// The lines of a text whose hard lines have `lines` characters each.
    pub fn text_lines(&self, lines: &[u32], width: f32) -> f32 {
        if lines.is_empty() {
            return 1.;
        }
        lines.iter().map(|chars| self.lines(*chars, width)).sum()
    }

    /// The same font at `scale` times the size.
    fn scaled(self, scale: f32) -> Self {
        Self {
            size: self.size * scale,
            advance: self.advance * scale,
            waste: self.waste,
        }
    }

    /// The average character width at `size` pixels.
    pub fn advance_at(&self, size: f32) -> f32 {
        self.advance * size / self.size
    }

    /// Measures the font `font` at `size` on [`SAMPLE`].
    fn measure(font: gpui_kit::Font, size: Pixels, text_system: &WindowTextSystem) -> Self {
        let run = TextRun {
            len: SAMPLE.len(),
            font: font.clone(),
            color: Hsla::default(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let chars = SAMPLE.chars().count() as f32;
        let width = f32::from(text_system.layout_line(SAMPLE, size, &[run], None).width);
        let advance = (width / chars).max(0.1);
        // Wrapped at 60 characters' width, the full lines hold fewer, and
        // the difference is what a line leaves at its end.
        let wrap = 60.;
        let mut wrapper = text_system.line_wrapper(font, size);
        let fragments = [LineFragment::text(SAMPLE)];
        let ends: Vec<usize> = wrapper
            .wrap_line(&fragments, px(advance * wrap), IndentAdjustment::NoIndent)
            .map(|boundary| boundary.ix)
            .collect();
        let waste = match ends.last() {
            Some(end) => {
                let full = SAMPLE[..*end].chars().count() as f32 / ends.len() as f32;
                (wrap - full).clamp(0., wrap / 2.)
            }
            None => 0.,
        };
        Self {
            size: f32::from(size),
            advance,
            waste,
        }
    }
}

/// What the sizes in the estimates depend on.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Key {
    rem: Pixels,
    font_size: Pixels,
    mono_font_size: Pixels,
    font_family: SharedString,
    mono_font_family: SharedString,
    scale: f32,
}

impl Key {
    pub fn current(window: &Window, cx: &App) -> Self {
        let theme = cx.theme();
        Self {
            rem: window.rem_size(),
            font_size: theme.font_size,
            mono_font_size: theme.mono_font_size,
            font_family: theme.font_family.clone(),
            mono_font_family: theme.mono_font_family.clone(),
            scale: window.scale_factor(),
        }
    }
}

/// The sizes the rows are laid out with, in pixels (lines, fonts and the
/// theme's rem).
#[derive(Debug, Clone)]
pub(super) struct Metrics {
    key: Key,
    pub rem: f32,
    /// Line heights of the interface's text at its sizes: extra small
    /// (0.75 rem), small (0.875 rem) and normal (1 rem).
    pub line_xs: f32,
    pub line_sm: f32,
    pub line: f32,
    /// The line height of code and of a call's output.
    pub line_mono: f32,
    /// The interface's font at 1 rem, and the code font.
    pub body: Font,
    pub mono: Font,
    /// Each heading level's font and line height.
    pub headings: [(Font, f32); 6],
}

/// A heading's text size, in pixels, and whether it's bold rather than
/// semibold, by level (as the text view lays headings out).
const HEADINGS: [(f32, FontWeight); 6] = [
    (28., FontWeight::BOLD),
    (21., FontWeight::SEMIBOLD),
    (17.5, FontWeight::SEMIBOLD),
    (15.75, FontWeight::SEMIBOLD),
    (14., FontWeight::SEMIBOLD),
    (14., FontWeight::MEDIUM),
];

impl Metrics {
    pub fn matches(&self, key: &Key) -> bool {
        self.key == *key
    }

    /// Resolving a new font weight can take a frame, so measure off the UI thread.
    pub fn measure(key: Key, window: &Window, cx: &App) -> Task<Self> {
        let rem = f32::from(key.rem);
        let snap = |size: f32| f32::from(window.pixel_snap(px(size * LINE_HEIGHT)));
        let line_xs = snap(rem * 0.75);
        let line_sm = snap(rem * 0.875);
        let line = snap(rem);
        let line_mono = snap(f32::from(key.mono_font_size));
        let heading_lines = HEADINGS.map(|(size, _)| snap(size));
        let text_system = cx.text_system().clone();
        cx.background_spawn(async move {
            let text_system = WindowTextSystem::new(text_system);
            let body = Font::measure(font(key.font_family.clone()), key.rem, &text_system);
            let mono = Font::measure(
                font(key.mono_font_family.clone()),
                key.mono_font_size,
                &text_system,
            );
            let headings = std::array::from_fn(|ix| {
                let (size, weight) = HEADINGS[ix];
                let font = gpui_kit::Font {
                    weight,
                    ..font(key.font_family.clone())
                };
                (
                    Font::measure(font, px(size), &text_system),
                    heading_lines[ix],
                )
            });
            Self {
                rem,
                line_xs,
                line_sm,
                line,
                line_mono,
                body,
                mono,
                headings,
                key,
            }
        })
    }

    /// The interface's font at `scale` rem.
    pub fn body_at(&self, scale: f32) -> Font {
        self.body.scaled(scale)
    }
}

/// The characters of each hard line of `text` (its lines between
/// newlines).
pub(super) fn hard_lines(text: &str) -> Vec<u32> {
    text.split('\n')
        .map(|line| line.trim_end_matches('\r').chars().count() as u32)
        .collect()
}

/// What a block of Markdown takes to estimate its height, counted from its
/// source when it comes or changes.
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) enum BlockFacts {
    /// The characters a paragraph shows (its soft line breaks are
    /// spaces).
    Prose(u32),
    /// A quote's characters.
    Quote(u32),
    Heading {
        level: u8,
        chars: u32,
    },
    /// A list's items, each one's depth (0 at the top) and characters.
    List(Vec<(u8, u32)>),
    /// The characters of each line of a code block's code.
    Code(Vec<u32>),
    /// A table's rows.
    Table(usize),
    /// A thematic break.
    Rule,
    #[default]
    Other,
}

impl BlockFacts {
    pub fn of(content: &text::TextBlockContent, source: &str) -> Self {
        use text::TextBlockContent as Content;
        match content {
            Content::Paragraph | Content::Other | Content::Custom(_) => {
                Self::Prose(prose_chars(source))
            }
            Content::Quote => Self::Quote(prose_chars(source)),
            Content::Heading(level) => Self::Heading {
                level: *level,
                chars: prose_chars(source.trim_start().trim_start_matches('#')),
            },
            Content::List => Self::List(list_items(source)),
            Content::CodeBlock(_) => Self::Code(code_line_chars(source)),
            Content::Table => Self::Table(
                source
                    .lines()
                    .filter(|line| {
                        let line = line.trim();
                        line.starts_with('|') && !line.trim_matches(['|', '-', ':', ' ']).is_empty()
                    })
                    .count(),
            ),
            Content::Rule => Self::Rule,
            _ => Self::Other,
        }
    }
}

use gpui_kit::component::text;

/// The characters Markdown `source` shows: without emphasis and code marks
/// and without the destinations of links, its line breaks as spaces.
pub(super) fn prose_chars(source: &str) -> u32 {
    let mut chars = 0;
    let mut in_link_target = false;
    let mut previous = '\0';
    let mut start_of_line = true;
    for c in source.trim().chars() {
        if in_link_target {
            if c == ')' {
                in_link_target = false;
            }
            previous = c;
            continue;
        }
        match c {
            '(' if previous == ']' => in_link_target = true,
            '*' | '`' | '[' | ']' => {}
            '>' if start_of_line => {}
            _ => chars += 1,
        }
        start_of_line = c == '\n' || (start_of_line && c.is_whitespace());
        previous = c;
    }
    chars
}

/// The characters of each line of a code block's code, from its source,
/// without its fences.
fn code_line_chars(source: &str) -> Vec<u32> {
    let fence = |line: &str| {
        let line = line.trim();
        line.starts_with("```") || line.starts_with("~~~")
    };
    let mut lines: Vec<&str> = source.lines().collect();
    if lines.first().is_some_and(|line| fence(line)) {
        lines.remove(0);
        if lines.last().is_some_and(|line| fence(line)) {
            lines.pop();
        }
    }
    lines
        .iter()
        .map(|line| line.chars().count() as u32)
        .collect()
}

/// A list's items from its source, each one's depth and the characters
/// it shows.
fn list_items(source: &str) -> Vec<(u8, u32)> {
    let mut items: Vec<(u8, u32)> = Vec::new();
    for line in source.lines() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            continue;
        }
        let indent = line.len() - trimmed.len();
        let marker = list_marker(trimmed);
        match marker {
            Some(marker) => {
                let depth = (indent / 2).min(8) as u8;
                items.push((depth, prose_chars(&trimmed[marker..])));
            }
            None => match items.last_mut() {
                // A line that goes on with the item before.
                Some((_, chars)) => *chars += 1 + prose_chars(trimmed),
                None => items.push((0, prose_chars(trimmed))),
            },
        }
    }
    items
}

/// The length of the list marker `line` starts with ("- ", "1. "), if it
/// starts with one.
fn list_marker(line: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    match bytes.first()? {
        b'-' | b'*' | b'+' if bytes.get(1).is_none_or(|b| *b == b' ') => Some(bytes.len().min(2)),
        b'0'..=b'9' => {
            let digits = bytes.iter().take_while(|b| b.is_ascii_digit()).count();
            match (bytes.get(digits), bytes.get(digits + 1)) {
                (Some(b'.' | b')'), None | Some(b' ')) => Some((digits + 2).min(bytes.len())),
                _ => None,
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{code_line_chars, list_items, prose_chars};

    #[test]
    fn prose_counts_the_characters_it_shows() {
        assert_eq!(prose_chars("Some **bold** and `code`."), 19);
        assert_eq!(prose_chars("A [link](https://example.com/a/b) here."), 12);
        assert_eq!(prose_chars("> quoted\n> text"), 13);
    }

    #[test]
    fn code_counts_the_characters_of_its_lines_inside_its_fences() {
        assert_eq!(code_line_chars("```rust\nfn a() {}\n\n```"), vec![9, 0]);
        assert_eq!(code_line_chars("```\nstill streaming"), vec![15]);
        assert_eq!(code_line_chars("    indented"), vec![12]);
    }

    #[test]
    fn a_lists_items_count_with_their_depth() {
        let list = "- one\n- two\n  goes on\n  - nested\n1. first";
        assert_eq!(list_items(list), vec![(0, 3), (0, 11), (1, 6), (0, 5)]);
    }
}
