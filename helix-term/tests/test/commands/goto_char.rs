use helix_core::hashmap;
use helix_term::keymap;
use helix_view::document::Mode;

use super::*;

fn config() -> Config {
    let mut config = helpers::test_config();
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

#[tokio::test(flavor = "multi_thread")]
async fn labels_do_not_scroll_a_wrapped_viewport() -> anyhow::Result<()> {
    let mut config = config();
    config.editor.soft_wrap.enable = Some(true);
    config.editor.soft_wrap.wrap_at_text_width = Some(true);
    config.editor.text_width = 80;
    let input = format!("{}#[.|]#\n{}", "x\n".repeat(200), "x\n".repeat(200));
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_input_text(input)
        .build()?;
    let view_id = app.editor.tree.focus;
    app.editor.ensure_cursor_in_view(view_id);
    run_event_loop_until_idle(&mut app).await;
    let (view, doc) = helix_view::current_ref!(app.editor);
    let viewport = doc.view_offset(view.id);
    let assert_viewport = |app: &Application| {
        let (view, doc) = helix_view::current_ref!(app.editor);
        assert_eq!(doc.view_offset(view.id), viewport);
    };
    test_key_sequences(
        &mut app,
        vec![
            (Some("_"), Some(&assert_viewport)),
            (Some("x"), Some(&assert_viewport)),
            (Some("a"), Some(&assert_viewport)),
            (Some("<esc>"), Some(&assert_viewport)),
        ],
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn offscreen_wrapped_characters_are_not_jump_targets() -> anyhow::Result<()> {
    let mut config = config();
    config.editor.soft_wrap.enable = Some(true);
    config.editor.soft_wrap.wrap_at_text_width = Some(true);
    config.editor.text_width = 80;
    let input = format!("#[.|]#{}z\n", "x ".repeat(8000));
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_input_text(input)
        .build()?;
    let assert_cursor = |app: &Application| {
        let (view, doc) = helix_view::current_ref!(app.editor);
        assert_eq!(
            doc.selection(view.id)
                .primary()
                .cursor(doc.text().slice(..)),
            0
        );
    };
    test_key_sequence(&mut app, Some("_z"), Some(&assert_cursor), false).await
}

#[tokio::test(flavor = "multi_thread")]
async fn choosing_or_cancelling_a_target_preserves_a_scroll_margin() -> anyhow::Result<()> {
    for mode in [Mode::Normal, Mode::Select] {
        let mut app = AppBuilder::new()
            .with_config(config())
            .with_input_text(format!(
                "{}#[.|]#\n{}",
                "x\n".repeat(200),
                "x\n".repeat(200)
            ))
            .build()?;
        app.editor.mode = mode;
        let (view, doc) = helix_view::current!(app.editor);
        let viewport = helix_view::view::ViewPosition {
            anchor: 400,
            ..Default::default()
        };
        doc.set_view_offset(view.id, viewport);
        let assert_viewport = |app: &Application| {
            let (view, doc) = helix_view::current_ref!(app.editor);
            assert_eq!(doc.view_offset(view.id), viewport);
            assert_eq!(
                doc.selection(view.id)
                    .primary()
                    .cursor(doc.text().slice(..)),
                400
            );
        };
        test_key_sequences(
            &mut app,
            vec![
                (Some("_"), Some(&assert_viewport)),
                (Some("x"), Some(&assert_viewport)),
                (Some("a"), Some(&assert_viewport)),
                (Some("<esc>"), Some(&assert_viewport)),
                (Some("_z"), Some(&assert_viewport)),
                (Some("_<esc>"), Some(&assert_viewport)),
            ],
            false,
        )
        .await?;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn two_key_jump_labels_at_line_ends_stay_on_the_target_row() -> anyhow::Result<()> {
    let mut app = AppBuilder::new()
        .with_config(config())
        .with_input_text(format!("#[.|]#\n{}", "x\n".repeat(29)))
        .build()?;
    let assert_label = |app: &Application| {
        let (view, doc) = helix_view::current_ref!(app.editor);
        let inner = view.inner_area(doc);
        let mut surface = tui::buffer::Buffer::empty(app.editor.tree.area());
        helix_term::ui::EditorView::new(Default::default()).render_view(
            &app.editor,
            doc,
            view,
            app.editor.tree.area(),
            &mut surface,
            true,
        );
        assert_eq!(surface[(inner.x + 1, inner.y)].symbol.as_str(), "a");
        assert_eq!(surface[(inner.x + 2, inner.y)].symbol.as_str(), "a");
    };
    test_key_sequence(&mut app, Some("_<ret>"), Some(&assert_label), false).await
}

#[tokio::test(flavor = "multi_thread")]
async fn horizontally_clipped_characters_are_not_jump_targets() -> anyhow::Result<()> {
    for (input, horizontal_offset, cursor) in [
        (format!("#[.|]#{}z\n", "x".repeat(200)), 0, 0),
        (format!("z{}#[.|]#\n", "x".repeat(200)), 150, 201),
        (format!("界{}#[.|]#\n", "x".repeat(200)), 1, 201),
    ] {
        let mut app = AppBuilder::new()
            .with_config(config())
            .with_input_text(input.clone())
            .build()?;
        let (view, doc) = helix_view::current!(app.editor);
        let mut viewport = doc.view_offset(view.id);
        viewport.horizontal_offset = horizontal_offset;
        doc.set_view_offset(view.id, viewport);
        let assert_cursor = |app: &Application| {
            let (view, doc) = helix_view::current_ref!(app.editor);
            assert_eq!(
                doc.selection(view.id)
                    .primary()
                    .cursor(doc.text().slice(..)),
                cursor
            );
            assert_eq!(doc.view_offset(view.id), viewport);
        };
        let keys = if input.starts_with('界') {
            "_界"
        } else {
            "_z"
        };
        test_key_sequence(&mut app, Some(keys), Some(&assert_cursor), false).await?;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn partially_visible_wide_characters_are_not_jump_targets() -> anyhow::Result<()> {
    let app = AppBuilder::new().with_config(config()).build()?;
    let (view, doc) = helix_view::current_ref!(app.editor);
    let width = view.inner_width(doc);
    drop(app);
    let mut app = AppBuilder::new()
        .with_config(config())
        .with_input_text(format!("#[.|]#{}界\n", " ".repeat(usize::from(width) - 2)))
        .build()?;
    let assert_cursor = |app: &Application| {
        let (view, doc) = helix_view::current_ref!(app.editor);
        assert_eq!(
            doc.selection(view.id)
                .primary()
                .cursor(doc.text().slice(..)),
            0
        );
    };
    test_key_sequence(&mut app, Some("_界"), Some(&assert_cursor), false).await
}

#[tokio::test(flavor = "multi_thread")]
async fn wrapped_continuations_above_the_viewport_are_not_jump_targets() -> anyhow::Result<()> {
    let mut config = config();
    config.editor.soft_wrap.enable = Some(true);
    config.editor.soft_wrap.wrap_at_text_width = Some(true);
    config.editor.text_width = 80;
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_input_text(format!("z{}#[.|]#\n", "x ".repeat(8000)))
        .build()?;
    app.editor.ensure_cursor_in_view(app.editor.tree.focus);
    let (view, doc) = helix_view::current_ref!(app.editor);
    let viewport = doc.view_offset(view.id);
    assert!(viewport.anchor > 0);
    let assert_cursor = |app: &Application| {
        let (view, doc) = helix_view::current_ref!(app.editor);
        assert_eq!(
            doc.selection(view.id)
                .primary()
                .cursor(doc.text().slice(..)),
            16001
        );
        assert_eq!(doc.view_offset(view.id), viewport);
    };
    test_key_sequence(&mut app, Some("_z"), Some(&assert_cursor), false).await
}

#[tokio::test(flavor = "multi_thread")]
async fn completing_a_jump_still_scrolls_to_satisfy_the_scroll_margin() -> anyhow::Result<()> {
    for mode in [Mode::Normal, Mode::Select] {
        let mut app = AppBuilder::new()
            .with_config(config())
            .with_input_text(format!(
                "{}#[.|]#\n{}",
                "x\n".repeat(200),
                "x\n".repeat(200)
            ))
            .build()?;
        app.editor.mode = mode;
        let (view, doc) = helix_view::current!(app.editor);
        let viewport = helix_view::view::ViewPosition {
            anchor: 400,
            ..Default::default()
        };
        doc.set_view_offset(view.id, viewport);
        let assert_jump = |app: &Application| {
            let (view, doc) = helix_view::current_ref!(app.editor);
            assert_eq!(
                doc.selection(view.id)
                    .primary()
                    .cursor(doc.text().slice(..)),
                402
            );
            assert_ne!(doc.view_offset(view.id), viewport);
            if mode == Mode::Select {
                assert_eq!(doc.selection(view.id).primary().from(), 400);
            }
        };
        test_key_sequence(&mut app, Some("_xaa"), Some(&assert_jump), false).await?;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn shared_label_renderer_preserves_word_jumps() -> anyhow::Result<()> {
    test_goto_char(("#[.|]# one two\n", "gwaa", ". #[one|]# two\n")).await?;
    test_goto_char(("#[.|]# one two\n", "vgwaa", "#[. one|]# two\n")).await
}

#[tokio::test(flavor = "multi_thread")]
async fn right_edge_labels_paint_over_a_wide_graphemes_trailing_cell() -> anyhow::Result<()> {
    fn render(app: &Application) -> tui::buffer::Buffer {
        let (view, doc) = helix_view::current_ref!(app.editor);
        let mut surface = tui::buffer::Buffer::empty(app.editor.tree.area());
        helix_term::ui::EditorView::new(Default::default()).render_view(
            &app.editor,
            doc,
            view,
            app.editor.tree.area(),
            &mut surface,
            true,
        );
        surface
    }
    let app = AppBuilder::new().with_config(config()).build()?;
    let (view, doc) = helix_view::current_ref!(app.editor);
    let width = usize::from(view.inner_width(doc));
    drop(app);
    let mut app = AppBuilder::new()
        .with_config(config())
        .with_input_text(format!(
            "#[.|]#{}界\n{}",
            " ".repeat(width - 4),
            "x\n".repeat(29)
        ))
        .build()?;
    let before = render(&app);
    let assert_label_updates = |app: &Application| {
        let (view, doc) = helix_view::current_ref!(app.editor);
        let inner = view.inner_area(doc);
        let after = render(app);
        let updates = before.diff(&after);
        for col in [inner.right() - 2, inner.right() - 1] {
            assert!(
                updates
                    .iter()
                    .any(|(x, y, cell)| *x == col && *y == inner.y && cell.symbol.as_str() == "a"),
                "terminal update missing for label at column {col}"
            );
        }
    };
    test_key_sequence(&mut app, Some("_<ret>"), Some(&assert_label_updates), false).await
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
