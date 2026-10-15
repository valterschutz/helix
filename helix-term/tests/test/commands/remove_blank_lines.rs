use super::*;
use helix_term::keymap::merge_keys;

#[tokio::test(flavor = "multi_thread")]
async fn remove_blank_lines_shifted_bindings() -> anyhow::Result<()> {
    for keys in ["{<space>", "v{<space>", "}<space>", "v}<space>"] {
        let raw: helix_term::config::ConfigRaw = toml::from_str(
            r#"
            [keys.normal."{"]
            space = "remove_blank_line_above"
            [keys.normal."}"]
            space = "remove_blank_line_below"
            [keys.select."{"]
            space = "remove_blank_line_above"
            [keys.select."}"]
            space = "remove_blank_line_below"
            "#,
        )?;
        let mut config = Config::default();
        merge_keys(&mut config.keys, raw.keys.unwrap());
        let output = if keys.contains('{') {
            "#[text|]#\n\n"
        } else {
            "\n#[text|]#\n"
        };
        test_with_config(
            AppBuilder::new().with_config(config),
            ("\n#[text|]#\n\n", keys, output),
        )
        .await?;
    }
    Ok(())
}

const ABOVE: &str = "<space>?remove_blank_line_above<ret>";
const BELOW: &str = "<space>?remove_blank_line_below<ret>";

#[tokio::test(flavor = "multi_thread")]
async fn remove_blank_lines_adjacent_only() -> anyhow::Result<()> {
    for (input, command, output) in [
        ("before\n\n\n#[text|]#\n", ABOVE, "before\n\n#[text|]#\n"),
        ("#[text|]#\n\n\nafter\n", BELOW, "#[text|]#\n\nafter\n"),
        ("before\n \t\n#[text|]#\n", ABOVE, "before\n#[text|]#\n"),
        ("#[text|]#\n\t \nafter\n", BELOW, "#[text|]#\nafter\n"),
        ("before\n#[text|]#\n", ABOVE, "before\n#[text|]#\n"),
        ("#[text|]#\nafter\n", BELOW, "#[text|]#\nafter\n"),
        ("#[text|]#\n", ABOVE, "#[text|]#\n"),
        ("#[text|]#\n", BELOW, "#[text|]#\n"),
        ("#[|]#", ABOVE, "#[|]#"),
        ("#[|]#", BELOW, "#[|]#"),
    ] {
        test((input, command, output, LineFeedHandling::AsIs)).await?;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn remove_blank_lines_count_and_undo() -> anyhow::Result<()> {
    test((
        "before\n\n\n#[text|]#\n",
        format!("9{ABOVE}"),
        "before\n#[text|]#\n",
    ))
    .await?;
    test((
        "#[text|]#\n\n\nafter\n",
        format!("9{BELOW}"),
        "#[text|]#\nafter\n",
    ))
    .await?;
    test(("\n\n#[text|]#\n", format!("9{ABOVE}"), "#[text|]#\n")).await?;
    test(("#[text|]#\n\n\n", format!("9{BELOW}"), "#[text|]#\n")).await?;
    test(("\n\n#[text|]#\n", format!("2{ABOVE}u"), "\n\n#[text|]#\n")).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn remove_blank_lines_preserves_selections() -> anyhow::Result<()> {
    test(("\n#[|one\ntwo]#\n\n", ABOVE, "#[|one\ntwo]#\n\n")).await?;
    test((
        "\n#[one\ntwo|]#\n\n",
        format!("v{BELOW}"),
        "\n#[one\ntwo|]#\n",
    ))
    .await?;
    test(("\n#(one|)#\n\n#[two|]#\n", ABOVE, "#(one|)#\n#[two|]#\n")).await?;
    test(("#(one|)#\n\n#[two|]#\n\n", BELOW, "#(one|)#\n#[two|]#\n")).await?;
    // Two selections on the same line target a blank line only once.
    test(("\n#(a|)#b#[c|]#\n", ABOVE, "#(a|)#b#[c|]#\n")).await?;
    // A selected blank line is protected from another selection's count.
    test((
        "#[one|]#\n\n#(\n|)#\n",
        format!("9{BELOW}"),
        "#[one|]#\n#(\n|)#",
    ))
    .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn remove_blank_lines_line_endings() -> anyhow::Result<()> {
    for (input, command, output) in [
        (
            "before\r\n \t\r\n#[text|]#\r\n",
            ABOVE,
            "before\r\n#[text|]#\r\n",
        ),
        (
            "#[text|]#\r\n\r\nafter\r\n",
            BELOW,
            "#[text|]#\r\nafter\r\n",
        ),
        ("#[text|]#\n \t", BELOW, "#[text|]#\n"),
        ("\n#[text|]#", ABOVE, "#[text|]#"),
        ("#[text|]#", BELOW, "#[text|]#"),
    ] {
        test((input, command, output, LineFeedHandling::AsIs)).await?;
    }
    Ok(())
}
