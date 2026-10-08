use super::*;

use helix_core::diagnostic::Severity;

#[tokio::test(flavor = "multi_thread")]
async fn toggle_diagnostics_display() -> anyhow::Result<()> {
    use helix_core::diagnostic::{Diagnostic, DiagnosticProvider, Range};
    use helix_term::{application::Application, ui::EditorView};
    use tui::buffer::Buffer;

    fn render(app: &Application) -> Buffer {
        let editor = &app.editor;
        let view = editor.tree.get(editor.tree.focus);
        let doc = editor.document(view.doc).unwrap();
        let mut surface = Buffer::empty(editor.tree.area());
        EditorView::new(Default::default()).render_view(
            editor,
            doc,
            view,
            editor.tree.area(),
            &mut surface,
            true,
        );
        surface
    }

    fn assert_surface_eq(actual: Buffer, expected: &Buffer) {
        assert_eq!(actual.area, expected.area);
        let differences: Vec<_> = actual
            .content
            .iter()
            .zip(&expected.content)
            .enumerate()
            .filter(|(_, (a, b))| a != b)
            .collect();
        assert!(differences.is_empty(), "{differences:?}");
    }

    // Exercise end-of-line, inline, and legacy cursor diagnostic messages.
    for options in [
        "",
        "[inline-diagnostics]\ncursor-line = 'hint'\nother-lines = 'hint'",
        "end-of-line-diagnostics = 'disable'",
    ] {
        let config = Config {
            editor: toml::from_str(options)?,
            ..helpers::test_config()
        };
        let mut app = AppBuilder::new().with_config(config).build()?;
        assert!(app.editor.config().lsp.display_diagnostics);
        app.editor.refresh_config(&Default::default());
        let baseline = render(&mut app);
        let id = app.editor.tree.get(app.editor.tree.focus).doc;
        app.editor.document_mut(id).unwrap().replace_diagnostics(
            [Diagnostic {
                range: Range { start: 0, end: 1 },
                ends_at_word: false,
                starts_at_word: false,
                zero_width: false,
                line: 0,
                message: "diagnostic display regression".into(),
                severity: Some(Severity::Error),
                code: None,
                provider: DiagnosticProvider::Lsp {
                    server_id: Default::default(),
                    identifier: None,
                },
                tags: vec![],
                source: None,
                data: None,
            }],
            &[],
            None,
        );
        let shown = render(&mut app);
        assert_ne!(shown, baseline);
        test_key_sequences(
            &mut app,
            vec![
                (
                    Some(":toggle lsp.display-diagnostics<ret>"),
                    Some(&|app| {
                        assert!(!app.editor.config().lsp.display_diagnostics);
                        assert_eq!(app.editor.document(id).unwrap().diagnostics().len(), 1);
                        assert_surface_eq(render(app), &baseline);
                    }),
                ),
                (
                    Some(":toggle lsp.display-diagnostics<ret>"),
                    Some(&|app| {
                        assert!(app.editor.config().lsp.display_diagnostics);
                        assert_eq!(app.editor.document(id).unwrap().diagnostics().len(), 1);
                        let restored = render(app);
                        assert_ne!(restored, baseline);
                        let text: String = restored
                            .content
                            .iter()
                            .map(|cell| cell.symbol.as_str())
                            .collect();
                        assert!(text.contains("diagnostic display regression"));
                    }),
                ),
            ],
            false,
        )
        .await?;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn history_completion() -> anyhow::Result<()> {
    test_key_sequence(
        &mut AppBuilder::new().build()?,
        Some(":asdf<ret>:theme d<C-n><tab>"),
        Some(&|app| {
            assert!(!app.editor.is_err());
        }),
        false,
    )
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn prompt_reset_anchor() -> anyhow::Result<()> {
    test_key_sequence(
        &mut AppBuilder::new().build()?,
        Some(":string wider than the terminal window causing the anchor location to be non zero which would panic when the line is deleted<C-u>"),
        Some(&|app| {
            assert!(!app.editor.is_err());
        }),
        false,
    )
    .await?;

    Ok(())
}

async fn test_statusline(
    line: &str,
    expected_status: &str,
    expected_severity: Severity,
) -> anyhow::Result<()> {
    test_key_sequence(
        &mut AppBuilder::new().build()?,
        Some(&format!("{line}<ret>")),
        Some(&|app| {
            let (status, &severity) = app.editor.get_status().unwrap();
            assert_eq!(
                severity, expected_severity,
                "'{line}' printed {severity:?}: {status}"
            );
            assert_eq!(status.as_ref(), expected_status);
        }),
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn variable_expansion() -> anyhow::Result<()> {
    test_statusline(r#":echo %{cursor_line}"#, "1", Severity::Info).await?;
    // Double quotes can be used with expansions:
    test_statusline(
        r#":echo "line%{cursor_line}line""#,
        "line1line",
        Severity::Info,
    )
    .await?;
    // Within double quotes you can escape the percent token for an expansion by doubling it.
    test_statusline(
        r#":echo "%%{cursor_line}""#,
        "%{cursor_line}",
        Severity::Info,
    )
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn unicode_expansion() -> anyhow::Result<()> {
    test_statusline(r#":echo %u{20}"#, " ", Severity::Info).await?;
    test_statusline(r#":echo %u{0020}"#, " ", Severity::Info).await?;
    test_statusline(r#":echo %u{25CF}"#, "●", Severity::Info).await?;
    // Not a valid Unicode codepoint:
    test_statusline(
        r#":echo %u{deadbeef}"#,
        "'echo': could not interpret 'deadbeef' as a Unicode character code",
        Severity::Error,
    )
    .await?;

    Ok(())
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn shell_expansion() -> anyhow::Result<()> {
    test_statusline(
        r#":echo %sh{echo "hello world"}"#,
        "hello world",
        Severity::Info,
    )
    .await?;

    // Shell expansion is recursive.
    test_statusline(":echo %sh{echo '%{cursor_line}'}", "1", Severity::Info).await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn register_expansion() -> anyhow::Result<()> {
    test_statusline(
        r#":set-register a hello world<ret>:echo %reg{a}"#,
        "hello world",
        Severity::Info,
    )
    .await?;
    test_statusline(r#":echo %reg{a}"#, "", Severity::Info).await?;
    test_statusline(
        r#":echo %reg{abc}"#,
        "'echo': Invalid register `abc`: should only be a single character",
        Severity::Error,
    )
    .await?;

    // Register expansion evaluation is *not* recursive.
    test_statusline(
        r#":set-register a b<ret>:set-register b hello<ret>:echo %reg{%reg{a}}"#,
        "'echo': Invalid register `%reg{a}`: should only be a single character",
        Severity::Error,
    )
    .await?;
    test_statusline(
        r#":set-register a hello<ret>:set-register b %%reg{a}<ret>:echo %reg{b}"#,
        "%reg{a}",
        Severity::Info,
    )
    .await?;

    // However, you can copy the contents of one register into another with this expansion if you
    // want to.
    test_statusline(
        r#":set-register a hello<ret>:set-register b %reg{a}<ret>:echo %reg{b}"#,
        "hello",
        Severity::Info,
    )
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn percent_escaping() -> anyhow::Result<()> {
    test_statusline(
        r#":sh echo hello 10%"#,
        "'run-shell-command': '%' was not properly escaped. Please use '%%'",
        Severity::Error,
    )
    .await?;
    Ok(())
}
