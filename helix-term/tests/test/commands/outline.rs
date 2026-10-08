use helix_term::{
    commands::MappableCommand,
    keymap::{KeyTrie, KeyTrieNode},
};
use helix_view::{document::Mode, editor::Severity};

use super::*;

/// The default config with `key` bound to `command` in normal mode.
fn config_with_key(key: &str, command: MappableCommand) -> Config {
    let mut config = Config::default();
    let keys = [(key.parse().unwrap(), KeyTrie::MappableCommand(command))];
    config.keys.insert(
        Mode::Normal,
        KeyTrie::Node(KeyTrieNode::new("Normal Mode", keys.into_iter().collect())),
    );
    config
}

#[tokio::test(flavor = "multi_thread")]
async fn outline_picker_jumps_to_the_chosen_summary() -> anyhow::Result<()> {
    test_with_config(
        AppBuilder::new().with_config(config_with_key("F2", MappableCommand::outline_picker)),
        (
            indoc! {"\
                #[#|]# Title
                <!-- Σ The first passage -->
                First.

                ## Detail
                <!-- Σ The second passage -->
                Second.
            "},
            ":lang markdown<ret><F2>second<ret>",
            indoc! {"\
                # Title
                <!-- Σ The first passage -->
                First.

                ## Detail
                #[<|]#!-- Σ The second passage -->
                Second.
            "},
        ),
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn outline_picker_lists_an_empty_summary_as_a_placeholder() -> anyhow::Result<()> {
    test_with_config(
        AppBuilder::new().with_config(config_with_key("F2", MappableCommand::outline_picker)),
        (
            indoc! {"\
                #[#|]# Title
                <!-- Σ The first passage -->
                First.

                <!-- Σ -->
                Second.
            "},
            ":lang markdown<ret><F2>(empty summary)<ret>",
            indoc! {"\
                # Title
                <!-- Σ The first passage -->
                First.

                #[<|]#!-- Σ -->
                Second.
            "},
        ),
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn outline_picker_reports_languages_without_outline_support() -> anyhow::Result<()> {
    test_key_sequence(
        &mut AppBuilder::new()
            .with_config(config_with_key("F2", MappableCommand::outline_picker))
            .build()?,
        Some(":lang rust<ret><F2>"),
        Some(&|app| {
            let (status, &severity) = app.editor.get_status().unwrap();
            assert_eq!(severity, Severity::Error);
            assert_eq!(
                status.as_ref(),
                "No outline available for this buffer's language"
            );
        }),
        false,
    )
    .await
}

async fn test_add_summary<T: Into<TestCase>>(test_case: T) -> anyhow::Result<()> {
    test_with_config(
        AppBuilder::new().with_config(config_with_key("F3", MappableCommand::add_summary)),
        test_case,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn add_summary_inserts_a_summary_above_the_paragraph_from_any_of_its_lines(
) -> anyhow::Result<()> {
    let expected = indoc! {"\
        # Title

        <!-- Σ #[ |]#-->
        First line.
        Middle line.
        Last line.
    "};
    for input in [
        indoc! {"\
            # Title

            #[F|]#irst line.
            Middle line.
            Last line.
        "},
        indoc! {"\
            # Title

            First line.
            Middle #[l|]#ine.
            Last line.
        "},
        indoc! {"\
            # Title

            First line.
            Middle line.
            Last line.#[\n|]#
        "},
    ] {
        test_add_summary((input, ":lang markdown<ret><F3>", expected)).await?;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn add_summary_indents_the_summary_like_the_paragraph() -> anyhow::Result<()> {
    test_add_summary((
        indoc! {"\
            - A list item

              A second paragraph
              in the #[l|]#ist item.
        "},
        ":lang markdown<ret><F3>",
        indoc! {"\
            - A list item

              <!-- Σ #[ |]#-->
              A second paragraph
              in the list item.
        "},
    ))
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn add_summary_moves_into_the_existing_summary_above_the_paragraph() -> anyhow::Result<()> {
    test_add_summary((
        indoc! {"\
            # Title
            <!-- Σ The first passage -->
            First line.
            Last #[l|]#ine.
        "},
        ":lang markdown<ret><F3>",
        indoc! {"\
            # Title
            <!-- Σ The first passage#[ |]#-->
            First line.
            Last line.
        "},
    ))
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn add_summary_moves_into_the_summary_above_a_paragraph_in_a_block_quote(
) -> anyhow::Result<()> {
    test_add_summary((
        indoc! {"\
            > <!-- Σ Quoted -->
            > Quoted #[t|]#ext.
        "},
        ":lang markdown<ret><F3>",
        indoc! {"\
            > <!-- Σ Quoted#[ |]#-->
            > Quoted text.
        "},
    ))
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn add_summary_on_a_summary_line_moves_into_that_summary() -> anyhow::Result<()> {
    test_add_summary((
        indoc! {"\
            <!-- #[Σ|]# Intro -->
            Intro.
        "},
        ":lang markdown<ret><F3>",
        indoc! {"\
            <!-- Σ Intro#[ |]#-->
            Intro.
        "},
    ))
    .await?;

    // An empty summary is entered where add-summary leaves the cursor in a new summary.
    test_add_summary((
        indoc! {"\
            #[<|]#!-- Σ  -->
            Intro.
        "},
        ":lang markdown<ret><F3>",
        indoc! {"\
            <!-- Σ #[ |]#-->
            Intro.
        "},
    ))
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn add_summary_spaces_out_an_empty_summary_like_a_new_one() -> anyhow::Result<()> {
    for (input, expected) in [
        (
            indoc! {"\
                #[<|]#!-- Σ -->
                Intro.
            "},
            indoc! {"\
                <!-- Σ #[ |]#-->
                Intro.
            "},
        ),
        (
            indoc! {"\
                #[<|]#!--Σ-->
                Intro.
            "},
            indoc! {"\
                <!--Σ #[ |]#-->
                Intro.
            "},
        ),
    ] {
        test_add_summary((input, ":lang markdown<ret><F3>", expected)).await?;
    }

    // Typed text is spaced out from the comment syntax.
    test_add_summary((
        indoc! {"\
            #[<|]#!-- Σ -->
            Intro.
        "},
        ":lang markdown<ret><F3>Why",
        indoc! {"\
            <!-- Σ Why#[ |]#-->
            Intro.
        "},
    ))
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn add_summary_splits_a_passage_at_a_later_paragraph() -> anyhow::Result<()> {
    test_add_summary((
        indoc! {"\
            <!-- Σ The passage -->
            First paragraph.

            Second #[p|]#aragraph.
        "},
        ":lang markdown<ret><F3>",
        indoc! {"\
            <!-- Σ The passage -->
            First paragraph.

            <!-- Σ #[ |]#-->
            Second paragraph.
        "},
    ))
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn add_summary_reports_an_error_outside_a_paragraph() -> anyhow::Result<()> {
    for input in [
        indoc! {"\
            # Title
            #[\n|]#
            Text.
        "},
        indoc! {"\
            # T#[i|]#tle
            Text.
        "},
    ] {
        test_key_sequence(
            &mut AppBuilder::new()
                .with_config(config_with_key("F3", MappableCommand::add_summary))
                .with_input_text(input)
                .build()?,
            Some(":lang markdown<ret><F3>"),
            Some(&|app| {
                let (status, &severity) = app.editor.get_status().unwrap();
                assert_eq!(severity, Severity::Error);
                assert_eq!(status.as_ref(), "No paragraph under the cursor");

                let (view, doc) = helix_view::current_ref!(app.editor);
                let (text, selection) = helix_core::test::print(input);
                assert_eq!(doc.text().to_string(), text);
                assert_eq!(doc.selection(view.id), &selection);
                assert_eq!(app.editor.mode, Mode::Normal);
            }),
            false,
        )
        .await?;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn add_summary_is_undone_in_one_step() -> anyhow::Result<()> {
    test_add_summary((
        indoc! {"\
            First line.
            Last #[l|]#ine.
        "},
        ":lang markdown<ret><F3><esc>u",
        indoc! {"\
            First line.
            Last #[l|]#ine.
        "},
    ))
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn outline_picker_jumps_to_a_typst_summary() -> anyhow::Result<()> {
    test_with_config(
        AppBuilder::new().with_config(config_with_key("F2", MappableCommand::outline_picker)),
        (
            indoc! {"\
                #[=|]# Title
                // Σ The first passage
                First.

                == Detail
                /* Σ The second passage */
                Second.
            "},
            ":lang typst<ret><F2>second<ret>",
            indoc! {"\
                = Title
                // Σ The first passage
                First.

                == Detail
                #[/|]#* Σ The second passage */
                Second.
            "},
        ),
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn add_summary_inserts_a_typst_line_comment_summary_indented_like_the_paragraph(
) -> anyhow::Result<()> {
    let input = indoc! {"\
        = Title

        - A list item

          A second paragraph
          in the #[l|]#ist item.
    "};
    test_add_summary((
        input,
        ":lang typst<ret><F3>",
        indoc! {"\
            = Title

            - A list item

              // Σ #[\n|]#
              A second paragraph
              in the list item.
        "},
    ))
    .await?;

    test_add_summary((
        input,
        ":lang typst<ret><F3>Why",
        indoc! {"\
            = Title

            - A list item

              // Σ Why#[\n|]#
              A second paragraph
              in the list item.
        "},
    ))
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn add_summary_moves_into_an_existing_typst_summary() -> anyhow::Result<()> {
    for (input, expected) in [
        (
            indoc! {"\
                /* Σ Intro */
                Intro #[t|]#ext.
            "},
            indoc! {"\
                /* Σ Intro#[ |]#*/
                Intro text.
            "},
        ),
        (
            indoc! {"\
                // Σ Intro
                Intro #[t|]#ext.
            "},
            indoc! {"\
                // Σ Intro#[\n|]#
                Intro text.
            "},
        ),
        (
            indoc! {"\
                #[/|]#/ Σ
                Intro text.
            "},
            indoc! {"\
                // Σ #[\n|]#
                Intro text.
            "},
        ),
    ] {
        test_add_summary((input, ":lang typst<ret><F3>", expected)).await?;
    }
    Ok(())
}
