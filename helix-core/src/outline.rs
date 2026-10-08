//! The outline of a document: its chapters and summaries in document order.
//!
//! A language supports outlines when it has an `outline.scm` query. The query captures each
//! heading as `@chapter.<level>` with the text to show as `@name`, and comments as `@comment`.
//! `@name` may capture several nodes, such as the text and markup in a Typst heading, and then
//! spans from the first to the last of them.
//! `@chapter.<level>` must capture the heading or title node, not a node that spans the whole
//! chapter body such as LaTeX's `section`: the captured node's lines are the chapter entry's lines,
//! which the picker jumps to and which are not part of any paragraph.
//!
//! Whether a captured comment is a summary is decided here from the language's comment tokens,
//! so outline queries don't repeat the summary syntax. Highlight and injection queries can't call
//! into Rust, so Markdown's `highlights.scm` and `injections.scm`, and the `highlights.scm` of
//! LaTeX and Typst, repeat the summary pattern as a regex that must be kept in step with this
//! module.

use std::ops::Range;

use ropey::RopeSlice;
use tree_house::tree_sitter::{
    query::{InvalidPredicateError, ParseError},
    Capture, Grammar, InactiveQueryCursor, Query, RopeInput,
};

use crate::line_ending::{get_line_ending, line_end_char_index};
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
    Chapter {
        level: u8,
    },
    Summary {
        /// The char index in the document where editing the summary resumes: just after the
        /// summary text, or where the text of an empty summary goes.
        text_end: usize,
        /// Spaces to insert at `text_end` that lay out an empty summary like a new one from
        /// [`empty_summary`], such as the space before `-->` in `<!-- Σ -->`.
        padding: SummaryPadding,
    },
}

/// Spaces to insert before and after the cursor when editing a summary resumes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SummaryPadding {
    pub before: &'static str,
    pub after: &'static str,
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

impl OutlineEntry {
    pub fn is_summary(&self) -> bool {
        matches!(self.kind, OutlineEntryKind::Summary { .. })
    }

    pub fn chapter_level(&self) -> Option<u8> {
        match self.kind {
            OutlineEntryKind::Chapter { level } => Some(level),
            OutlineEntryKind::Summary { .. } => None,
        }
    }
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
                let byte_range = without_continuation_prefix(
                    text,
                    byte_range.start as usize..byte_range.end as usize,
                );
                if let Some(level) = query.chapter_level(matched.capture) {
                    chapter = Some((level, byte_range));
                } else if query.name_capture == Some(matched.capture) {
                    name = Some(match name {
                        Some(Range { start, .. }) => start..byte_range.end,
                        None => byte_range,
                    });
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
                // Trailing whitespace is kept, as an empty line comment summary's text goes after it.
                let line_end = text.char_to_byte(line_end_char_index(&text, lines.start));
                let comment = text
                    .byte_slice(byte_range.start..byte_range.end.min(line_end))
                    .to_string();
                let comment_start = byte_range.start + comment.len() - comment.trim_start().len();
                let comment = comment.trim_start();
                if let Some((summary, padding)) = summary_text_range(comment, config) {
                    let text_end = text.byte_to_char(comment_start + summary.end);
                    entries.push(OutlineEntry {
                        kind: OutlineEntryKind::Summary { text_end, padding },
                        text: comment[summary].to_string(),
                        lines,
                        depth: 0,
                    });
                }
            }
        }
        entries.sort_by_key(|entry| entry.lines.start);

        let shallowest_level = entries
            .iter()
            .filter_map(OutlineEntry::chapter_level)
            .min()
            .unwrap_or_default();
        let mut summary_depth = 0;
        for entry in &mut entries {
            if let Some(level) = entry.chapter_level() {
                entry.depth = usize::from(level - shallowest_level);
                summary_depth = entry.depth + 1;
            } else {
                entry.depth = summary_depth;
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
            .filter(|(_, entry)| entry.is_summary())
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
            .filter(|entry| entry.is_summary())
    }
}

/// An empty summary in the language's comment syntax.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmptySummary {
    /// The summary line without indentation and line ending.
    pub line: String,
    /// The char offset in `line` where the summary text goes.
    pub text_offset: usize,
}

/// An empty summary in the language's comment syntax, preferring line comments. Returns `None`
/// when the language has no comment tokens.
pub fn empty_summary(config: &LanguageConfiguration) -> Option<EmptySummary> {
    let line_comment = config.comment_tokens.iter().flatten().next();
    let block_comment = config.block_comment_tokens.iter().flatten().next();
    let summary = match (line_comment, block_comment) {
        (Some(token), _) => {
            let line = format!("{token} {SUMMARY_MARKER} ");
            let text_offset = line.chars().count();
            EmptySummary { line, text_offset }
        }
        (None, Some(token)) => {
            let opener = format!("{} {SUMMARY_MARKER} ", token.start);
            let text_offset = opener.chars().count();
            let line = format!("{opener} {}", token.end);
            EmptySummary { line, text_offset }
        }
        (None, None) => return None,
    };
    Some(summary)
}

/// Returns the byte range of the trimmed summary text in a single-line `comment` whose text, after
/// the language's comment opener and any spaces or tabs, starts with [`SUMMARY_MARKER`], and the
/// padding that lays out an empty summary like a new one.
fn summary_text_range(
    comment: &str,
    config: &LanguageConfiguration,
) -> Option<(Range<usize>, SummaryPadding)> {
    let line_comment_texts = config.comment_tokens.iter().flatten().filter_map(|token| {
        let text = comment.strip_prefix(token.as_str())?;
        Some((token.len(), text, false))
    });
    let block_comment_texts = config
        .block_comment_tokens
        .iter()
        .flatten()
        .filter_map(|token| {
            let text = comment
                .trim_end()
                .strip_prefix(token.start.as_str())?
                .strip_suffix(token.end.as_str())?;
            Some((token.start.len(), text, true))
        });
    line_comment_texts
        .chain(block_comment_texts)
        .find_map(|(text_start, text, has_closer)| {
            let after_marker = text
                .trim_start_matches([' ', '\t'])
                .strip_prefix(SUMMARY_MARKER)?;
            let marker_end = text_start + text.len() - after_marker.len();
            let summary = after_marker.trim();
            if !summary.is_empty() {
                let summary_start =
                    marker_end + after_marker.len() - after_marker.trim_start().len();
                return Some((
                    summary_start..summary_start + summary.len(),
                    SummaryPadding::default(),
                ));
            }
            // An empty summary's text goes after the space following the marker and, in a block
            // comment, before a space ahead of the closer, as in a new summary.
            let summary_start = marker_end + after_marker.chars().next().map_or(0, char::len_utf8);
            let padding = SummaryPadding {
                before: if after_marker.is_empty() { " " } else { "" },
                after: if has_closer && after_marker.chars().count() < 2 {
                    " "
                } else {
                    ""
                },
            };
            Some((summary_start..summary_start, padding))
        })
}

/// The byte range of a captured node without the next line's container prefix that some grammars
/// include at its end, such as `> ` in a Markdown block quote or a list item's indentation. That
/// prefix is the node's text after its last line ending when it is no wider than the text before
/// the node on its first line, and only holds whitespace and characters of that text.
fn without_continuation_prefix(text: RopeSlice, byte_range: Range<usize>) -> Range<usize> {
    let start_line = text.byte_to_line(byte_range.start);
    let last_line = text.byte_to_line(byte_range.end);
    if last_line == start_line {
        return byte_range;
    }
    let prefix = text.byte_slice(text.line_to_byte(start_line)..byte_range.start);
    let last_line_start = text.line_to_byte(last_line);
    let tail = text.byte_slice(last_line_start..byte_range.end);
    let is_prefix = tail.len_chars() <= prefix.len_chars()
        && tail
            .chars()
            .all(|ch| ch.is_whitespace() || prefix.chars().any(|prefix_ch| prefix_ch == ch));
    if is_prefix {
        byte_range.start..last_line_start
    } else {
        byte_range
    }
}

/// The lines spanned by a node, ignoring trailing whitespace such as the line ending that some
/// grammars include in the node.
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
        language_outline("markdown", text)
    }

    fn typst_outline(text: &str) -> (Rope, Outline) {
        language_outline("typst", text)
    }

    fn latex_outline(text: &str) -> (Rope, Outline) {
        language_outline("latex", text)
    }

    fn language_outline(language: &str, text: &str) -> (Rope, Outline) {
        let text = Rope::from_str(text);
        let language = LOADER.language_for_name(language).unwrap();
        let syntax = Syntax::new(text.slice(..), language, &LOADER).unwrap();
        let outline = Outline::new(text.slice(..), &syntax, &LOADER).unwrap();
        (text, outline)
    }

    /// Renders one entry per line: indentation by depth, `h<level>` or `Σ`, the text and the
    /// entry's line range.
    fn render(outline: &Outline) -> String {
        let mut out = String::new();
        for entry in outline.entries() {
            let marker = match entry.chapter_level() {
                Some(level) => format!("h{level}"),
                None => "Σ".to_string(),
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
    fn only_spaces_and_tabs_may_separate_the_comment_opener_from_the_marker() {
        // The highlight queries allow only `[ \t]` there, and the outline agrees with them.
        let (_, outline) = markdown_outline("<!--\tΣ Tab -->\n\n<!--\u{a0}Σ No-break space -->\n");
        assert_eq!(render(&outline), "Σ Tab 0..1\n");
        let (_, outline) = latex_outline("%\tΣ Tab\n%\u{a0}Σ No-break space\n");
        assert_eq!(render(&outline), "Σ Tab 0..1\n");
        let (_, outline) =
            typst_outline("//\tΣ Tab\n//\u{a0}Σ No-break space\n/*\u{a0}Σ No-break space */\n");
        assert_eq!(render(&outline), "Σ Tab 0..1\n");
    }

    #[test]
    fn summaries_in_block_quotes_and_list_items_are_summaries() {
        let (text, outline) = markdown_outline(indoc! {"
            > <!-- Σ Quoted -->
            > More.

            > > <!-- Σ Nested -->
            > > More.

            >   <!-- Σ Indented -->
            >   More.

            - <!-- Σ Listed -->
              More.

            > - <!-- Σ Quoted and listed -->
            >   More.

            > <!-- Σ -->
            > More.

            > <!-- Σ a summary
            > that spans two lines -->
            > More.

            > # Quoted chapter
            > More.

            > Quoted setext chapter
            > ---
            > More.

            - # Listed chapter
              More.
        "});
        assert_eq!(
            render(&outline),
            indoc! {"
                Σ Quoted 0..1
                Σ Nested 3..4
                Σ Indented 6..7
                Σ Listed 9..10
                Σ Quoted and listed 12..13
                Σ  15..16
                h1 Quoted chapter 22..23
                  h2 Quoted setext chapter 25..27
                h1 Listed chapter 29..30
            "}
        );
        assert_eq!(
            entered_summaries(&text, &outline)
                .last()
                .map(String::as_str),
            Some("> <!-- Σ | -->")
        );
        let text = text.slice(..);
        for line in [1, 23, 27, 30] {
            assert_eq!(outline.paragraph_at(text, line), Some(line..line + 1));
        }
    }

    #[test]
    fn empty_summaries_use_the_language_comment_syntax_preferring_line_comments() {
        let empty_summary = |language| {
            let language = LOADER.language_for_name(language).unwrap();
            let EmptySummary { line, text_offset } =
                super::empty_summary(LOADER.language(language).config())?;
            let text_start = line
                .char_indices()
                .map(|(idx, _)| idx)
                .chain([line.len()])
                .nth(text_offset)?;
            let (before, after) = line.split_at(text_start);
            Some(format!("{before}|{after}"))
        };
        assert_eq!(empty_summary("markdown").as_deref(), Some("<!-- Σ | -->"));
        assert_eq!(empty_summary("latex").as_deref(), Some("% Σ |"));
        assert_eq!(empty_summary("typst").as_deref(), Some("// Σ |"));
        assert_eq!(empty_summary("json"), None);
    }

    /// The line of each summary as editing it resumes, with `|` at the cursor and the summary's
    /// padding inserted.
    fn entered_summaries(text: &Rope, outline: &Outline) -> Vec<String> {
        let text = text.slice(..);
        outline
            .entries()
            .iter()
            .filter_map(|entry| {
                let OutlineEntryKind::Summary { text_end, padding } = entry.kind else {
                    return None;
                };
                let line = entry.lines.start;
                let line_end = text.line_to_char(line) + text.line(line).len_chars();
                let line_end = line_end - get_line_ending(&text.line(line)).map_or(0, |_| 1);
                Some(format!(
                    "{}{}|{}{}",
                    text.slice(text.line_to_char(line)..text_end),
                    padding.before,
                    padding.after,
                    text.slice(text_end..line_end),
                ))
            })
            .collect()
    }

    #[test]
    fn summaries_are_entered_at_the_end_of_their_text() {
        let (text, outline) = markdown_outline(indoc! {"
            <!-- Σ Some text   -->
            <!--ΣTight-->
            <!-- Σ  -->
            <!-- Σ -->
            <!--Σ-->
            <!-- Σ     -->
        "});
        assert_eq!(
            entered_summaries(&text, &outline),
            [
                "<!-- Σ Some text|   -->",
                "<!--ΣTight|-->",
                "<!-- Σ | -->",
                "<!-- Σ | -->",
                "<!--Σ | -->",
                "<!-- Σ |    -->",
            ]
        );
    }

    #[test]
    fn latex_summaries_are_entered_at_the_end_of_their_text() {
        let (text, outline) =
            latex_outline("% Σ Some text  \n%ΣTight\n% Σ \n% Σ\n%Σ\n% Σ   \nText.\n");
        assert_eq!(
            entered_summaries(&text, &outline),
            [
                "% Σ Some text|  ",
                "%ΣTight|",
                "% Σ |",
                "% Σ |",
                "%Σ |",
                "% Σ |  ",
            ]
        );
    }

    #[test]
    fn latex_headings_are_chapters_at_successive_levels() {
        let (_, outline) = latex_outline(indoc! {r"
            \documentclass{book}
            \begin{document}
            \part{One}
            \chapter{Two}
            \section{Three}
            Text.
            \subsection*{Four}
            \subsubsection{Five}
            \paragraph{Six}
            \subparagraph*{Seven}
            \chapter*{Two again}
            \end{document}
        "});
        assert_eq!(
            render(&outline),
            indoc! {"
                h1 One 2..3
                  h2 Two 3..4
                    h3 Three 4..5
                      h4 Four 6..7
                        h5 Five 7..8
                          h6 Six 8..9
                            h7 Seven 9..10
                  h2 Two again 10..11
            "}
        );

        let (_, outline) = latex_outline("\\section{One}\n\\subsection{Two}\n");
        assert_eq!(render(&outline), "h3 One 0..1\n  h4 Two 1..2\n");
    }

    #[test]
    fn latex_summaries_sit_one_step_under_their_chapter() {
        let (_, outline) = latex_outline(indoc! {r"
            % Σ Why this document exists
            Intro text.

            \section{Background}
            %Σ The problem
            Some text.
              % Σ   An indented passage
              More text.

            \subsection{Detail}

            % Σ
            Unsummarised text.
        "});
        assert_eq!(
            render(&outline),
            indoc! {"
                Σ Why this document exists 0..1
                h3 Background 3..4
                  Σ The problem 4..5
                  Σ An indented passage 6..7
                  h4 Detail 9..10
                    Σ  11..12
            "}
        );
    }

    #[test]
    fn latex_only_line_comments_starting_with_the_marker_are_summaries() {
        let (_, outline) = latex_outline(indoc! {r"
            \section{Notes}
            % Pass 1: read title and abstract
            % A question about Σ
            % ∑ n-ary summation is not the marker
            \begin{verbatim}
            % Σ sample code
            \end{verbatim}
            \begin{comment}
            % Σ commented out
            \end{comment}
            \iffalse
            % Σ skipped
            \fi
            % Σ The only summary
        "});
        assert_eq!(
            render(&outline),
            indoc! {"
                h3 Notes 0..1
                  Σ The only summary 13..14
            "}
        );
    }

    #[test]
    fn latex_summaries_after_text_on_their_line_are_summaries() {
        // The comment node covers only the comment, not its line, so the outline agrees with
        // the highlight query, which can't see the text before the comment either.
        let (_, outline) = latex_outline("Text. % Σ Trailing\n");
        assert_eq!(render(&outline), "Σ Trailing 0..1\n");
    }

    #[test]
    fn latex_chapters_span_their_title_lines() {
        let (text, outline) = latex_outline(indoc! {r"
            \section[Short]{A title
              that \emph{wraps}}\label{sec:wraps}
            Text.
            \subsection{}
            Text.
        "});
        assert_eq!(
            render(&outline),
            indoc! {r"
                h3 A title that \emph{wraps} 0..2
                  h4  3..4
            "}
        );
        let text = text.slice(..);
        assert_eq!(outline.paragraph_at(text, 1), None);
        assert_eq!(outline.paragraph_at(text, 2), Some(2..3));
    }

    #[test]
    fn latex_chapter_titles_with_math_are_shown_whole() {
        let (_, outline) = latex_outline("\\section{The $x$ case}\n");
        assert_eq!(render(&outline), "h3 The $x$ case 0..1\n");
    }

    #[test]
    fn latex_passages_and_paragraphs_are_bounded_by_summaries_and_chapters() {
        let (text, outline) = latex_outline(indoc! {r"
            % Σ intro
            Intro.
            \section{One}
            % Σ first
            First a.
            First b.

            Unsummarised.
            \section{Two}
        "});
        let passages: Vec<_> = outline
            .passages()
            .map(|passage| (passage.summary.text.as_str(), passage.lines))
            .collect();
        assert_eq!(passages, [("intro", 0..2), ("first", 3..8)]);

        let text = text.slice(..);
        let summary_above = |line| {
            let paragraph = outline.paragraph_at(text, line)?;
            Some(outline.summary_above(&paragraph)?.text.as_str())
        };
        assert_eq!(outline.paragraph_at(text, 5), Some(4..6));
        assert_eq!(summary_above(5), Some("first"));
        assert_eq!(outline.paragraph_at(text, 7), Some(7..8));
        assert_eq!(summary_above(7), None);
        for line in [0, 2, 3, 6, 8] {
            assert_eq!(outline.paragraph_at(text, line), None);
        }
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

    #[test]
    fn typst_headings_are_chapters_at_their_number_of_equals_signs() {
        let (_, outline) = typst_outline(indoc! {"
            == Two

            === Three
            ==== Four
            ===== Five

            ====== Six
            == Two again
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

        let (_, outline) = typst_outline("= One\n== Two\n");
        assert_eq!(render(&outline), "h1 One 0..1\n  h2 Two 1..2\n");
    }

    #[test]
    fn typst_chapter_names_span_the_heading_markup_without_its_label() {
        // A Typst heading has no single node for its text, so `@name` captures several nodes.
        let (_, outline) = typst_outline(indoc! {"
            = The *main*   result <intro>
            == `code` and $x$ math
            === Plain <plain>
            ====
        "});
        assert_eq!(
            render(&outline),
            indoc! {"
                h1 The *main* result 0..1
                  h2 `code` and $x$ math 1..2
                    h3 Plain 2..3
                      h4  3..4
            "}
        );
    }

    #[test]
    fn typst_line_and_single_line_block_comment_summaries_sit_under_their_chapter() {
        let (_, outline) = typst_outline(indoc! {"
            // Σ Why this document exists
            Intro text.

            = Background
            /* Σ The problem */
            Some text.
            //Σ   A second passage
            More text.

            == Detail

            // Σ
            Unsummarised text.
            /*Σ*/
        "});
        assert_eq!(
            render(&outline),
            indoc! {"
                Σ Why this document exists 0..1
                h1 Background 3..4
                  Σ The problem 4..5
                  Σ A second passage 6..7
                  h2 Detail 9..10
                    Σ  11..12
                    Σ  13..14
            "}
        );
    }

    #[test]
    fn typst_comments_without_the_marker_first_on_one_line_are_not_summaries() {
        let (_, outline) = typst_outline(indoc! {"
            = Notes
            // Pass 1: read title and abstract
            /* A question about Σ */
            // ∑ n-ary summation is not the marker
            /* Σ a summary
            that spans two lines */

            ```typ
            // Σ sample code
            ```

            // Σ The only summary
        "});
        assert_eq!(
            render(&outline),
            indoc! {"
                h1 Notes 0..1
                  Σ The only summary 11..12
            "}
        );
    }

    #[test]
    fn typst_comments_sharing_a_line_with_other_text_are_summaries() {
        // Typst comment nodes don't include the rest of their line, so neither this module nor
        // the highlight query can tell them apart from summaries on their own line. Summaries go
        // on their own line by convention.
        let (_, outline) = typst_outline(indoc! {"
            Text with a trailing // Σ comment
            /* Σ text before more */ more
            #let x = 1 // Σ code
        "});
        assert_eq!(
            render(&outline),
            indoc! {"
                Σ comment 0..1
                Σ text before more 1..2
                Σ code 2..3
            "}
        );
    }

    #[test]
    fn typst_passages_and_paragraphs_are_bounded_by_summaries_and_chapters() {
        let (text, outline) = typst_outline(indoc! {"
            // Σ intro
            Intro.

            = One
            /* Σ first */
            First a.
            First b.
            // Σ second
            Second.

            Still second.
            == Two
            Unsummarised.
        "});
        let passages: Vec<_> = outline
            .passages()
            .map(|passage| (passage.summary.text.as_str(), passage.lines))
            .collect();
        assert_eq!(
            passages,
            [("intro", 0..3), ("first", 4..7), ("second", 7..11)]
        );

        let text = text.slice(..);
        let summary_above = |line| {
            let paragraph = outline.paragraph_at(text, line)?;
            Some(outline.summary_above(&paragraph)?.text.as_str())
        };
        assert_eq!(outline.paragraph_at(text, 6), Some(5..7));
        assert_eq!(summary_above(6), Some("first"));
        assert_eq!(outline.paragraph_at(text, 10), Some(10..11));
        assert_eq!(summary_above(10), None);
        for line in [3, 4, 7, 11] {
            assert_eq!(outline.paragraph_at(text, line), None);
        }
    }

    #[test]
    fn typst_summaries_are_entered_at_the_end_of_their_text() {
        let (text, outline) =
            typst_outline("// Σ Some text  \n//ΣTight\n// Σ\n/* Σ Block text */\n/* Σ */\n/*Σ*/\n");
        assert_eq!(
            entered_summaries(&text, &outline),
            [
                "// Σ Some text|  ",
                "//ΣTight|",
                "// Σ |",
                "/* Σ Block text| */",
                "/* Σ | */",
                "/*Σ | */",
            ]
        );
    }
}
