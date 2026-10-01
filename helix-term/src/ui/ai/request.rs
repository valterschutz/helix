//! Request text handling for an AI edit conversation: finding the reference or
//! command token under the cursor and normalising a request before pi sees it.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    Reference,
    Command,
}

/// The `@reference` or `/command` token that ends exactly at the cursor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionToken {
    pub kind: TokenKind,
    /// Byte offset of the sigil within the text.
    pub start: usize,
    /// The text typed after the sigil.
    pub query: String,
}

/// Finds the reference or command token under the cursor. A token starts with
/// `@` or `/` at the beginning of the text or after whitespace and contains no
/// whitespace.
pub fn completion_token(before_cursor: &str) -> Option<CompletionToken> {
    let start = before_cursor
        .char_indices()
        .rev()
        .find(|(_, character)| character.is_whitespace())
        .map_or(0, |(index, character)| index + character.len_utf8());
    let token = &before_cursor[start..];
    let kind = match token.chars().next()? {
        '@' => TokenKind::Reference,
        '/' => TokenKind::Command,
        _ => return None,
    };
    Some(CompletionToken {
        kind,
        start,
        query: token[1..].to_owned(),
    })
}

/// A command pi reported through `get_commands`: a skill or a prompt template.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandInfo {
    pub name: String,
    pub description: Option<String>,
}

/// Workspace files and open buffers matching the query: the current file
/// first, then other open buffers, then the rest. With an empty query the
/// files keep their given order within each group.
pub fn reference_candidates<'a>(
    query: &str,
    files: &'a [String],
    open_buffers: &'a [String],
    current_file: Option<&str>,
) -> Vec<&'a str> {
    let is_open = |file: &str| open_buffers.iter().any(|open| open == file);
    let candidates = files
        .iter()
        .chain(open_buffers.iter().filter(|open| !files.contains(open)));
    let mut matched: Vec<(&str, u16)> = if query.is_empty() {
        candidates.map(|file| (file.as_str(), 0)).collect()
    } else {
        helix_core::fuzzy::fuzzy_match(query, candidates.map(String::as_str), true)
    };
    matched.sort_by_key(|(file, score)| {
        (
            Some(*file) != current_file,
            !is_open(file),
            std::cmp::Reverse(*score),
        )
    });
    matched.into_iter().map(|(file, _)| file).collect()
}

/// Commands whose name contains the query. Names whose bare name (without the
/// `skill:` prefix) starts with the query come first, then alphabetical.
pub fn command_candidates<'a>(query: &str, commands: &'a [CommandInfo]) -> Vec<&'a CommandInfo> {
    let query = query.to_lowercase();
    let mut matched: Vec<&CommandInfo> = commands
        .iter()
        .filter(|command| command.name.to_lowercase().contains(&query))
        .collect();
    matched.sort_by_key(|command| {
        let name = command.name.to_lowercase();
        let bare = name.strip_prefix("skill:").unwrap_or(&name).to_owned();
        (
            !(bare.starts_with(&query) || name.starts_with(&query)),
            bare,
        )
    });
    matched
}

/// Replaces the token starting at `token_start` and ending at `cursor` with
/// `value` followed by a space. Returns the new text and cursor.
pub fn apply_completion(
    text: &str,
    cursor: usize,
    token_start: usize,
    value: &str,
) -> (String, usize) {
    assert!(token_start <= cursor && cursor <= text.len());
    let mut replaced = String::with_capacity(text.len() + value.len() + 1);
    replaced.push_str(&text[..token_start]);
    replaced.push_str(value);
    replaced.push(' ');
    let new_cursor = replaced.len();
    replaced.push_str(&text[cursor..]);
    (replaced, new_cursor)
}

/// Normalises a request so that pi, which only expands a command at the very
/// start of a message, sees a trailing known command first. Any other request
/// is returned unchanged.
pub fn normalize_request(request: &str, commands: &[String]) -> String {
    let request = request.trim();
    let Some(last) = request.rsplit(char::is_whitespace).next() else {
        return request.to_owned();
    };
    let is_known_command = last
        .strip_prefix('/')
        .is_some_and(|name| commands.iter().any(|command| command == name));
    let prefix = request[..request.len() - last.len()].trim_end();
    if !is_known_command || prefix.is_empty() {
        return request.to_owned();
    }
    format!("{last} {prefix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_token_is_found_at_the_cursor() {
        assert_eq!(
            completion_token("Implement this like @src/ma"),
            Some(CompletionToken {
                kind: TokenKind::Reference,
                start: 20,
                query: "src/ma".into(),
            })
        );
    }

    #[test]
    fn command_token_is_found_after_a_newline() {
        assert_eq!(
            completion_token("Tidy this\n/skill:con"),
            Some(CompletionToken {
                kind: TokenKind::Command,
                start: 10,
                query: "skill:con".into(),
            })
        );
    }

    #[test]
    fn sigils_inside_words_are_plain_text() {
        assert_eq!(completion_token("email me@example"), None);
        assert_eq!(completion_token("use a/b"), None);
        assert_eq!(completion_token("@src/main.rs "), None);
        assert_eq!(completion_token(""), None);
    }

    #[test]
    fn tokens_may_follow_multibyte_whitespace() {
        assert_eq!(
            completion_token("see\u{a0}@x"),
            Some(CompletionToken {
                kind: TokenKind::Reference,
                start: 5,
                query: "x".into(),
            })
        );
    }

    #[test]
    fn trailing_command_moves_to_the_front() {
        let commands = vec!["skill:conventions-python".to_owned()];

        assert_eq!(
            normalize_request(
                "Implement this like @x.py /skill:conventions-python",
                &commands
            ),
            "/skill:conventions-python Implement this like @x.py"
        );
    }

    #[test]
    fn leading_unknown_and_lone_commands_are_left_alone() {
        let commands = vec!["skill:tdd".to_owned(), "fix-tests".to_owned()];

        assert_eq!(
            normalize_request("/skill:tdd add a test", &commands),
            "/skill:tdd add a test"
        );
        assert_eq!(
            normalize_request("split this /nonsense", &commands),
            "split this /nonsense"
        );
        assert_eq!(normalize_request("  /fix-tests  ", &commands), "/fix-tests");
    }

    #[test]
    fn multiline_requests_keep_their_lines_when_rotated() {
        let commands = vec!["fix-tests".to_owned()];

        assert_eq!(
            normalize_request("first line\nsecond line\n/fix-tests", &commands),
            "/fix-tests first line\nsecond line"
        );
    }

    #[test]
    fn references_rank_current_file_then_open_buffers_and_match_fuzzily() {
        let files = vec![
            "docs/readme.md".to_owned(),
            "src/main.rs".to_owned(),
            "src/ui/menu.rs".to_owned(),
        ];
        let open = vec!["src/ui/menu.rs".to_owned(), "src/main.rs".to_owned()];

        assert_eq!(
            reference_candidates("", &files, &open, Some("src/main.rs")),
            vec!["src/main.rs", "src/ui/menu.rs", "docs/readme.md"]
        );
        assert_eq!(
            reference_candidates("mn", &files, &open, None),
            vec!["src/ui/menu.rs", "src/main.rs"]
        );
        assert_eq!(
            reference_candidates("zzz", &files, &open, None),
            Vec::<&str>::new()
        );
    }

    #[test]
    fn open_buffers_outside_the_walk_are_still_offered() {
        let files = vec!["src/main.rs".to_owned()];
        let open = vec!["build/generated.rs".to_owned()];

        assert_eq!(
            reference_candidates("gen", &files, &open, None),
            vec!["build/generated.rs"]
        );
    }

    #[test]
    fn commands_match_by_name_with_prefixes_first() {
        let commands = vec![
            command("skill:conventions-python", "Python style"),
            command("fix-tests", "Fix failing tests"),
            command("skill:tdd", "Test first"),
        ];

        let names: Vec<&str> = command_candidates("t", &commands)
            .iter()
            .map(|command| command.name.as_str())
            .collect();

        assert_eq!(
            names,
            vec!["skill:tdd", "skill:conventions-python", "fix-tests"]
        );
        assert!(command_candidates("zzz", &commands).is_empty());
    }

    #[test]
    fn accepting_replaces_the_token_and_adds_a_space() {
        assert_eq!(
            apply_completion("like @sty and more", 9, 5, "@src/style.py"),
            ("like @src/style.py  and more".to_owned(), 19)
        );
        assert_eq!(
            apply_completion("/co", 3, 0, "/skill:code-review"),
            ("/skill:code-review ".to_owned(), 19)
        );
    }

    fn command(name: &str, description: &str) -> CommandInfo {
        CommandInfo {
            name: name.to_owned(),
            description: Some(description.to_owned()),
        }
    }
}
