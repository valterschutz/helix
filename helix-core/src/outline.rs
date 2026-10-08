//! The outline of a document: its chapters and summaries in document order.
//!
//! A language supports outlines when it has an `outline.scm` query. The query captures each
//! heading as `@chapter.<level>` with the text to show as `@name`, and comments as `@comment`.
//! Whether a comment is a summary is decided here from the language's comment tokens, so
//! queries don't repeat the summary syntax.

use std::ops::Range;

use ropey::RopeSlice;
use tree_house::tree_sitter::{
    query::{InvalidPredicateError, ParseError},
    Capture, Grammar, InactiveQueryCursor, Query, RopeInput,
};

use crate::line_ending::get_line_ending;
use crate::syntax::{config::LanguageConfiguration, Loader, Syntax, TREE_SITTER_MATCH_LIMIT};

/// The character that marks a comment as a summary: U+03A3 GREEK CAPITAL LETTER SIGMA.
pub const SUMMARY_MARKER: char = 'Σ';

#[derive(Debug)]
pub struct OutlineQuery {
    query: Query,
    chapter_captures: Vec<(Capture, u8)>,
    name_capture: Option<Capture>,
    comment_capture: Option<Capture>,
}

impl OutlineQuery {
    pub(crate) fn new(grammar: Grammar, source: &str) -> Result<Self, ParseError> {
        let query = Query::new(grammar, source, |_pattern, predicate| {
            Err(InvalidPredicateError::unknown(predicate))
        })?;
        let chapter_captures = query
            .captures()
            .filter_map(|(capture, name)| {
                let level = name.strip_prefix("chapter.")?.parse().ok()?;
                Some((capture, level))
            })
            .collect();
        Ok(Self {
            chapter_captures,
            name_capture: query.get_capture("name"),
            comment_capture: query.get_capture("comment"),
            query,
        })
    }

    fn chapter_level(&self, capture: Capture) -> Option<u8> {
        self.chapter_captures
            .iter()
            .find_map(|&(chapter_capture, level)| (chapter_capture == capture).then_some(level))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutlineEntryKind {
    Chapter { level: u8 },
    Summary,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutlineEntry {
    pub kind: OutlineEntryKind,
    /// The heading text, or the summary text without comment syntax and `Σ`.
    pub text: String,
    /// The lines the heading or summary spans.
    pub lines: Range<usize>,
    /// Indentation steps in the outline view. Chapters count from the shallowest heading level
    /// in the document, and summaries sit one step under the chapter they follow.
    pub depth: usize,
}

/// A summary plus everything after it up to the next summary or chapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Passage<'a> {
    pub summary: &'a OutlineEntry,
    pub lines: Range<usize>,
}

#[derive(Debug)]
pub struct Outline {
    entries: Vec<OutlineEntry>,
    /// The number of lines in the document, not counting the empty line after a final line
    /// ending.
    line_count: usize,
}

impl Outline {
    /// Builds the outline from the root language's outline query, or returns `None` when that
    /// language has none. Injected languages are not searched, so code blocks never contribute.
    pub fn new(text: RopeSlice, syntax: &Syntax, loader: &Loader) -> Option<Self> {
        let language = syntax.root_language();
        let query = loader.outline_query(language)?;
        let config = loader.language(language).config();
        let root = syntax.tree().root_node();
        let mut cursor = InactiveQueryCursor::new(0..u32::MAX, TREE_SITTER_MATCH_LIMIT)
            .execute_query(&query.query, &root, RopeInput::new(text));

        let mut entries = Vec::new();
        while let Some(mat) = cursor.next_match() {
            let mut chapter = None;
            let mut name = None;
            let mut comment = None;
            for matched in mat.matched_nodes() {
                let byte_range = matched.node.byte_range();
                let byte_range = byte_range.start as usize..byte_range.end as usize;
                if let Some(level) = query.chapter_level(matched.capture) {
                    chapter = Some((level, byte_range));
                } else if query.name_capture == Some(matched.capture) {
                    name = Some(byte_range);
                } else if query.comment_capture == Some(matched.capture) {
                    comment = Some(byte_range);
                }
            }
            if let Some((level, byte_range)) = chapter {
                let name = name.map_or_else(String::new, |name| {
                    let name = text.byte_slice(name).to_string();
                    name.split_whitespace().collect::<Vec<_>>().join(" ")
                });
                entries.push(OutlineEntry {
                    kind: OutlineEntryKind::Chapter { level },
                    text: name,
                    lines: line_range(text, byte_range),
                    depth: 0,
                });
            } else if let Some(byte_range) = comment {
                let lines = line_range(text, byte_range.clone());
                if lines.len() != 1 {
                    continue;
                }
                let comment = text.byte_slice(byte_range).to_string();
                if let Some(summary) = summary_text(comment.trim(), config) {
                    entries.push(OutlineEntry {
                        kind: OutlineEntryKind::Summary,
                        text: summary.to_string(),
                        lines,
                        depth: 0,
                    });
                }
            }
        }
        entries.sort_by_key(|entry| entry.lines.start);

        let shallowest_level = entries
            .iter()
            .filter_map(|entry| match entry.kind {
                OutlineEntryKind::Chapter { level } => Some(level),
                OutlineEntryKind::Summary => None,
            })
            .min()
            .unwrap_or_default();
        let mut summary_depth = 0;
        for entry in &mut entries {
            match entry.kind {
                OutlineEntryKind::Chapter { level } => {
                    entry.depth = usize::from(level - shallowest_level);
                    summary_depth = entry.depth + 1;
                }
                OutlineEntryKind::Summary => entry.depth = summary_depth,
            }
        }

        let line_count = text.len_lines() - usize::from(get_line_ending(&text).is_some());
        Some(Self {
            entries,
            line_count,
        })
    }

    pub fn entries(&self) -> &[OutlineEntry] {
        &self.entries
    }

    pub fn passages(&self) -> impl Iterator<Item = Passage<'_>> {
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.kind == OutlineEntryKind::Summary)
            .map(|(idx, summary)| {
                let end = self
                    .entries
                    .get(idx + 1)
                    .map_or(self.line_count, |next| next.lines.start);
                Passage {
                    summary,
                    lines: summary.lines.start..end,
                }
            })
    }

    /// The chapter or summary that spans `line`.
    pub fn entry_at_line(&self, line: usize) -> Option<&OutlineEntry> {
        self.entries
            .iter()
            .find(|entry| entry.lines.contains(&line))
    }

    /// The lines of the paragraph containing `line`: the contiguous non-blank lines around it,
    /// excluding chapter and summary lines. Returns `None` when `line` is blank or belongs to a
    /// chapter or summary.
    pub fn paragraph_at(&self, text: RopeSlice, line: usize) -> Option<Range<usize>> {
        let in_paragraph = |line: usize| {
            line < self.line_count
                && !text.line(line).chars().all(char::is_whitespace)
                && self.entry_at_line(line).is_none()
        };
        if !in_paragraph(line) {
            return None;
        }
        let start = (0..line)
            .rev()
            .take_while(|&line| in_paragraph(line))
            .last()
            .unwrap_or(line);
        let end = (line + 1..self.line_count)
            .take_while(|&line| in_paragraph(line))
            .last()
            .unwrap_or(line);
        Some(start..end + 1)
    }

    /// The summary on the line directly above `paragraph`.
    pub fn summary_above(&self, paragraph: &Range<usize>) -> Option<&OutlineEntry> {
        let line_above = paragraph.start.checked_sub(1)?;
        self.entry_at_line(line_above)
            .filter(|entry| entry.kind == OutlineEntryKind::Summary)
    }
}

/// Returns the summary text of a single-line `comment` whose text, after the language's comment
/// opener, starts with [`SUMMARY_MARKER`].
fn summary_text<'a>(comment: &'a str, config: &LanguageConfiguration) -> Option<&'a str> {
    let line_comment_texts = config
        .comment_tokens
        .iter()
        .flatten()
        .filter_map(|token| comment.strip_prefix(token.as_str()));
    let block_comment_texts = config
        .block_comment_tokens
        .iter()
        .flatten()
        .filter_map(|token| {
            comment
                .strip_prefix(token.start.as_str())?
                .strip_suffix(token.end.as_str())
        });
    line_comment_texts
        .chain(block_comment_texts)
        .find_map(|text| Some(text.trim_start().strip_prefix(SUMMARY_MARKER)?.trim()))
}

/// The lines spanned by a node, ignoring trailing whitespace such as the line ending or a block
/// continuation that some grammars include in the node.
fn line_range(text: RopeSlice, byte_range: Range<usize>) -> Range<usize> {
    let start = byte_range.start;
    let node_text = text.byte_slice(byte_range).to_string();
    let end = start + node_text.trim_end().len();
    let start_line = text.byte_to_line(start);
    let end_line = text.byte_to_line(end.saturating_sub(1).max(start));
    start_line..end_line + 1
}

#[cfg(test)]
mod test {
    use std::fmt::Write as _;

    use indoc::indoc;
    use once_cell::sync::Lazy;

    use super::*;
    use crate::{syntax::Loader, Rope, Syntax};

    static LOADER: Lazy<Loader> = Lazy::new(crate::config::default_lang_loader);

    fn markdown_outline(text: &str) -> (Rope, Outline) {
        let text = Rope::from_str(text);
        let language = LOADER.language_for_name("markdown").unwrap();
        let syntax = Syntax::new(text.slice(..), language, &LOADER).unwrap();
        let outline = Outline::new(text.slice(..), &syntax, &LOADER).unwrap();
        (text, outline)
    }

    /// Renders one entry per line: indentation by depth, `h<level>` or `Σ`, the text and the
    /// entry's line range.
    fn render(outline: &Outline) -> String {
        let mut out = String::new();
        for entry in outline.entries() {
            let marker = match entry.kind {
                OutlineEntryKind::Chapter { level } => format!("h{level}"),
                OutlineEntryKind::Summary => "Σ".to_string(),
            };
            let indent = "  ".repeat(entry.depth);
            writeln!(out, "{indent}{marker} {} {:?}", entry.text, entry.lines).unwrap();
        }
        out
    }

    #[test]
    fn chapters_at_every_level_are_indented_from_the_shallowest_level() {
        let (_, outline) = markdown_outline(indoc! {"
            ## Two

            ### Three
            #### Four
            ##### Five

            ###### Six
            ## Two again
        "});
        assert_eq!(
            render(&outline),
            indoc! {"
                h2 Two 0..1
                  h3 Three 2..3
                    h4 Four 3..4
                      h5 Five 4..5
                        h6 Six 6..7
                h2 Two again 7..8
            "}
        );

        let (_, outline) = markdown_outline("# One\n## Two\n");
        assert_eq!(render(&outline), "h1 One 0..1\n  h2 Two 1..2\n");
    }

    #[test]
    fn setext_headings_are_chapters() {
        let (_, outline) = markdown_outline(indoc! {"
            Title
            =====

            A heading that
            wraps
            ---
        "});
        assert_eq!(
            render(&outline),
            indoc! {"
                h1 Title 0..2
                  h2 A heading that wraps 3..6
            "}
        );
    }

    #[test]
    fn summaries_sit_one_step_under_their_chapter() {
        let (_, outline) = markdown_outline(indoc! {"
            <!-- Σ Why this document exists -->
            Intro text.

            ## Background
            <!--Σ The problem -->
            Some text.
            <!-- Σ   A second passage   -->
            More text.

            ### Detail

            <!-- Σ -->
            Unsummarised text.
            <!--Σ-->
        "});
        assert_eq!(
            render(&outline),
            indoc! {"
                Σ Why this document exists 0..1
                h2 Background 3..4
                  Σ The problem 4..5
                  Σ A second passage 6..7
                  h3 Detail 9..10
                    Σ  11..12
                    Σ  13..14
            "}
        );
    }

    #[test]
    fn only_single_line_comments_starting_with_the_marker_are_summaries() {
        let (_, outline) = markdown_outline(indoc! {"
            # Notes
            <!-- Pass 1: read title and abstract -->
            <!-- A question about Σ -->
            <!-- ∑ n-ary summation is not the marker -->
            <!-- Σ a summary
            that spans two lines -->
            <!-- Σ text after the closer --> more
            Text with an inline <!-- Σ comment -->.

            ```html
            <!-- Σ sample code -->
            ```

                <!-- Σ indented code -->

            <!-- Σ The only summary -->
        "});
        assert_eq!(
            render(&outline),
            indoc! {"
                h1 Notes 0..1
                  Σ The only summary 15..16
            "}
        );
    }

    #[test]
    fn languages_without_an_outline_query_have_no_outline() {
        let text = Rope::from_str("// Σ a comment\nfn main() {}\n");
        let language = LOADER.language_for_name("rust").unwrap();
        let syntax = Syntax::new(text.slice(..), language, &LOADER).unwrap();
        assert!(Outline::new(text.slice(..), &syntax, &LOADER).is_none());
    }

    #[test]
    fn passages_run_from_a_summary_to_the_next_summary_or_chapter() {
        let (_, outline) = markdown_outline(indoc! {"
            <!-- Σ intro -->
            Intro.

            # One
            <!-- Σ first -->
            First.
            <!-- Σ second -->
            Second.

            Still second.
            ## Two
            Unsummarised.
            <!-- Σ last -->
            Last.
        "});
        let passages: Vec<_> = outline
            .passages()
            .map(|passage| (passage.summary.text.as_str(), passage.lines))
            .collect();
        assert_eq!(
            passages,
            [
                ("intro", 0..3),
                ("first", 4..6),
                ("second", 6..10),
                ("last", 12..14),
            ]
        );
    }

    #[test]
    fn paragraphs_are_bounded_by_blank_lines_chapters_and_summaries() {
        let (text, outline) = markdown_outline(indoc! {"
            # Title
            <!-- Σ first -->
            One a.
            One b.
            One c.

            Two a.
            Two b.
            <!-- Σ third -->
            Three.
        "});
        let text = text.slice(..);
        let summary_above = |line| {
            let paragraph = outline.paragraph_at(text, line)?;
            Some(outline.summary_above(&paragraph)?.text.as_str())
        };

        for line in 2..5 {
            assert_eq!(outline.paragraph_at(text, line), Some(2..5));
            assert_eq!(summary_above(line), Some("first"));
        }
        for line in 6..8 {
            assert_eq!(outline.paragraph_at(text, line), Some(6..8));
            assert_eq!(summary_above(line), None);
        }
        assert_eq!(outline.paragraph_at(text, 9), Some(9..10));
        assert_eq!(summary_above(9), Some("third"));

        // Blank lines, chapters and summaries are not part of any paragraph.
        for line in [0, 1, 5, 8] {
            assert_eq!(outline.paragraph_at(text, line), None);
        }
        let entry_text = |line| Some(outline.entry_at_line(line)?.text.as_str());
        assert_eq!(entry_text(0), Some("Title"));
        assert_eq!(entry_text(1), Some("first"));
        assert_eq!(entry_text(2), None);
    }
}
