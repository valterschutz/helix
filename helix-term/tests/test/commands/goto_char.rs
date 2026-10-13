use helix_core::hashmap;
use helix_term::keymap;
use helix_view::document::Mode;

use super::*;

fn config() -> Config {
    let mut config = Config::default();
    config.keys.insert(
        Mode::Normal,
        keymap!({"Normal Mode"
            "_" => goto_char,
        }),
    );
    config.keys.insert(
        Mode::Select,
        keymap!({"Select Mode"
            "_" => extend_to_char,
        }),
    );
    config
}

async fn test_goto_char<T: Into<TestCase>>(test_case: T) -> anyhow::Result<()> {
    test_with_config(AppBuilder::new().with_config(config()), test_case).await
}

#[tokio::test(flavor = "multi_thread")]
async fn targets_after_the_cursor_get_single_letter_labels_in_document_order() -> anyhow::Result<()>
{
    test_goto_char(("#[x|]# one two one\n", "_ob", "x one tw#[o|]# one\n")).await
}

#[tokio::test(flavor = "multi_thread")]
async fn targets_before_the_cursor_get_labels_nearest_first() -> anyhow::Result<()> {
    test_goto_char(("one two #[.|]#\n", "_ob", "#[o|]#ne two .\n")).await
}

#[tokio::test(flavor = "multi_thread")]
async fn labels_alternate_between_targets_after_and_before_the_cursor() -> anyhow::Result<()> {
    // Distance order: "o" of "two" (a), "o" of the first "one" (b),
    // "o" of the last "one" (c).
    test_goto_char(("one #[.|]# two one\n", "_oc", "one . two #[o|]#ne\n")).await
}

#[tokio::test(flavor = "multi_thread")]
async fn more_targets_than_the_alphabet_use_two_letter_labels() -> anyhow::Result<()> {
    let targets = 30;
    let input = format!(".#[ |]#{}\n", "x ".repeat(targets));
    // Label "ac" is the third target.
    let mut output = format!(". {}", "x ".repeat(targets));
    output.replace_range(6..7, "#[x|]#");
    output.push('\n');
    test_goto_char((input, "_xac", output)).await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_lowercase_search_character_matches_both_cases() -> anyhow::Result<()> {
    test_goto_char(("#[.|]# a A a\n", "_ab", ". a #[A|]# a\n")).await
}

#[tokio::test(flavor = "multi_thread")]
async fn an_uppercase_search_character_matches_only_uppercase() -> anyhow::Result<()> {
    // The single target is jumped to without a label.
    test_goto_char(("#[.|]# a A a\n", "_A", ". a #[A|]# a\n")).await
}

#[tokio::test(flavor = "multi_thread")]
async fn enter_targets_line_ends() -> anyhow::Result<()> {
    test_goto_char(("#[.|]# ab\ncd\nef\n", "_<ret>b", ". ab\ncd#[\n|]#ef\n")).await
}

#[tokio::test(flavor = "multi_thread")]
async fn space_targets_whitespace_but_not_line_ends() -> anyhow::Result<()> {
    test_goto_char(("#[.|]# a b\nc\td\n", "_<space>c", ". a b\nc#[\t|]#d\n")).await
}

#[tokio::test(flavor = "multi_thread")]
async fn no_target_leaves_the_selection_alone() -> anyhow::Result<()> {
    test_goto_char(("#[.|]# abc\n", "_z", "#[.|]# abc\n")).await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_key_outside_the_alphabet_cancels_the_jump() -> anyhow::Result<()> {
    test_goto_char(("#[.|]# a a\n", "_a<esc>", "#[.|]# a a\n")).await
}

#[tokio::test(flavor = "multi_thread")]
async fn extend_forward_keeps_the_anchor() -> anyhow::Result<()> {
    test_goto_char(("#[o|]#ne two one\n", "v_oa", "#[one two|]# one\n")).await
}

#[tokio::test(flavor = "multi_thread")]
async fn extend_backward_extends_from_the_selection_end() -> anyhow::Result<()> {
    // Distance order: "o" of the last "one" (a), "o" of "two" (b).
    test_goto_char(("one two on#[e|]#\n", "v_ob", "one tw#[|o one]#\n")).await
}
