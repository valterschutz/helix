use helix_core::hashmap;
use helix_term::keymap;
use helix_view::{doc, document::Mode, editor::Severity};
use tempfile::NamedTempFile;

use super::*;

fn config() -> Config {
    let mut config = Config::default();
    config.keys.insert(
        Mode::Normal,
        keymap!({"Normal Mode"
            "g" => { "Goto"
                "1" => goto_buffer_1,
                "2" => goto_buffer_2,
                "5" => goto_buffer_5,
                "0" => goto_last_buffer,
            },
        }),
    );
    config
}

fn three_files() -> anyhow::Result<[NamedTempFile; 3]> {
    Ok([
        NamedTempFile::new()?,
        NamedTempFile::new()?,
        NamedTempFile::new()?,
    ])
}

fn app_with(files: &[NamedTempFile; 3]) -> anyhow::Result<Application> {
    files
        .iter()
        .fold(AppBuilder::new().with_config(config()), |builder, file| {
            builder.with_file(file.path(), None)
        })
        .build()
}

fn current_path(app: &Application) -> std::path::PathBuf {
    doc!(app.editor).path().unwrap().to_path_buf()
}

#[tokio::test(flavor = "multi_thread")]
async fn goto_buffer_n_switches_to_the_nth_buffer_in_bufferline_order() -> anyhow::Result<()> {
    let files = three_files()?;
    let first = files[0].path().to_path_buf();
    let second = files[1].path().to_path_buf();
    test_key_sequences(
        &mut app_with(&files)?,
        vec![
            (
                Some("g2"),
                Some(&|app| assert_eq!(current_path(app), second)),
            ),
            (
                Some("g1"),
                Some(&|app| assert_eq!(current_path(app), first)),
            ),
        ],
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn goto_last_buffer_switches_to_the_last_buffer() -> anyhow::Result<()> {
    let files = three_files()?;
    let last = files[2].path().to_path_buf();
    test_key_sequences(
        &mut app_with(&files)?,
        vec![
            (Some("g1"), None),
            (Some("g0"), Some(&|app| assert_eq!(current_path(app), last))),
        ],
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn goto_buffer_n_past_the_last_buffer_stays_put_and_reports_it() -> anyhow::Result<()> {
    let files = three_files()?;
    let first = files[0].path().to_path_buf();
    test_key_sequences(
        &mut app_with(&files)?,
        vec![
            (Some("g1"), None),
            (
                Some("g5"),
                Some(&|app| {
                    assert_eq!(current_path(app), first);
                    let (message, severity) = app.editor.get_status().unwrap();
                    assert_eq!(message, "no buffer 5: only 3 open");
                    assert_eq!(severity, &Severity::Error);
                }),
            ),
        ],
        false,
    )
    .await
}
