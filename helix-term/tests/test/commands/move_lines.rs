use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn move_lines_down_moves_the_selected_line_and_keeps_the_selection() -> anyhow::Result<()> {
    test((
        indoc! {"\
            one
            t#[w|]#o
            three
            "},
        "J",
        indoc! {"\
            one
            three
            t#[w|]#o
            "},
    ))
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn move_lines_up_moves_every_line_a_selection_touches() -> anyhow::Result<()> {
    test((
        indoc! {"\
            one
            tw#[o
            th|]#ree
            four
            "},
        "K",
        indoc! {"\
            tw#[o
            th|]#ree
            one
            four
            "},
    ))
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn move_lines_keeps_a_whole_line_selection_on_its_line() -> anyhow::Result<()> {
    test((
        indoc! {"\
            one
            #[two
            |]#three
            "},
        "J",
        indoc! {"\
            one
            three
            #[two
            |]#"},
    ))
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn move_lines_moves_separate_blocks_independently() -> anyhow::Result<()> {
    test((
        indoc! {"\
            1
            #(2|)#
            3
            #[4|]#
            5
            6
            "},
        "2J",
        indoc! {"\
            1
            3
            5
            #(2|)#
            6
            #[4|]#
            "},
    ))
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn move_lines_treats_selections_on_adjacent_lines_as_one_block() -> anyhow::Result<()> {
    test((
        indoc! {"\
            1
            #(2|)#
            #[3|]#
            4
            "},
        "J",
        indoc! {"\
            1
            4
            #(2|)#
            #[3|]#
            "},
    ))
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn move_lines_does_nothing_when_a_block_is_at_the_edge() -> anyhow::Result<()> {
    test((
        indoc! {"\
            #(1|)#
            2
            #[3|]#
            4
            "},
        "K",
        indoc! {"\
            #(1|)#
            2
            #[3|]#
            4
            "},
    ))
    .await?;

    test((
        indoc! {"\
            1
            #[2|]#
            "},
        "J",
        indoc! {"\
            1
            #[2|]#
            "},
    ))
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn move_lines_count_is_limited_by_the_room_left() -> anyhow::Result<()> {
    test((
        indoc! {"\
            1
            2
            #[3|]#
            4
            "},
        "5K",
        indoc! {"\
            #[3|]#
            1
            2
            4
            "},
    ))
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn move_lines_preserves_a_missing_final_line_ending() -> anyhow::Result<()> {
    test((
        indoc! {"\
            1
            #[2|]#
            3"},
        "J",
        indoc! {"\
            1
            3
            #[2|]#"},
    ))
    .await?;

    test((
        indoc! {"\
            1
            2
            #[3|]#"},
        "K",
        indoc! {"\
            1
            #[3|]#
            2"},
    ))
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn ctrl_j_joins_and_ctrl_k_keeps_selections() -> anyhow::Result<()> {
    test((
        indoc! {"\
            #[one
            two|]#
            "},
        "<C-j>",
        indoc! {"\
            #[one two|]#
            "},
    ))
    .await?;

    test((
        indoc! {"\
            #(one|)#
            #[two|]#
            "},
        "<C-k>two<ret>",
        indoc! {"\
            one
            #[two|]#
            "},
    ))
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn move_lines_is_undone_one_press_at_a_time() -> anyhow::Result<()> {
    test((
        indoc! {"\
            #[1|]#
            2
            3
            "},
        "JJu",
        indoc! {"\
            2
            #[1|]#
            3
            "},
    ))
    .await?;

    Ok(())
}
