use helix_core::hashmap;
use helix_term::keymap;
use helix_view::{document::Mode, editor::Severity};

use super::*;

fn config_with_outline_picker() -> Config {
    let mut config = Config::default();
    config.keys.insert(
        Mode::Normal,
        keymap!({"Normal Mode"
            "F2" => outline_picker,
        }),
    );
    config
}

#[tokio::test(flavor = "multi_thread")]
async fn outline_picker_jumps_to_the_chosen_summary() -> anyhow::Result<()> {
    test_with_config(
        AppBuilder::new().with_config(config_with_outline_picker()),
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
async fn outline_picker_reports_languages_without_outline_support() -> anyhow::Result<()> {
    test_key_sequence(
        &mut AppBuilder::new()
            .with_config(config_with_outline_picker())
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
