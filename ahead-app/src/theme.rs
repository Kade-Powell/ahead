use std::collections::HashMap;

use anyhow::{Context as _, Result, bail};
use gpui_kit::component::ThemeConfig;
use serde::Deserialize;
use serde_json::{Map, Value, json};

#[derive(Deserialize)]
struct ThemeFile {
    #[serde(rename = "color-theme")]
    palette: Palette,
    #[serde(default)]
    ui: HashMap<String, toml::Value>,
}

#[derive(Deserialize)]
struct Palette {
    name: String,
    base: HashMap<String, String>,
    ui: HashMap<String, String>,
    syntax: HashMap<String, String>,
}

impl Palette {
    fn resolve(&self, value: &str) -> Result<String> {
        let mut value = value;
        let mut visited = Vec::new();
        while let Some(name) = value.strip_prefix('$') {
            if visited.contains(&name) {
                bail!("Cyclic theme color: {name}");
            }
            visited.push(name);
            value = self
                .base
                .get(name)
                .with_context(|| format!("Missing theme color: {name}"))?;
        }
        if !value.starts_with('#')
            || !matches!(value.len(), 7 | 9)
            || !value[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            bail!("Invalid theme color: {value}");
        }
        Ok(value.to_owned())
    }

    fn color(&self, source: &str) -> Result<String> {
        self.resolve(if source.starts_with('$') {
            source
        } else {
            self.ui
                .get(source)
                .with_context(|| format!("Missing UI color: {source}"))?
        })
    }
}

fn theme_value(source: &str, mode: &str) -> Result<Value> {
    let ThemeFile { palette, ui } = toml::from_str::<ThemeFile>(source)?;
    let font_size = ui
        .get("font-size")
        .and_then(toml::Value::as_float)
        .or_else(|| {
            ui.get("font-size")
                .and_then(toml::Value::as_integer)
                .map(|value| value as f64)
        })
        .with_context(|| "Missing or invalid UI value: font-size")?;
    let mut colors = Map::new();
    for (target, source) in [
        ("background", "editor.background"),
        ("foreground", "$text"),
        ("border", "ahead.border"),
        ("accent.background", "panel.hovered.background"),
        ("accent.foreground", "$text"),
        ("input.border", "$input-border"),
        ("ring", "editor.focus"),
        ("accordion.background", "$secondary-background"),
        ("button.background", "$secondary-background"),
        ("button.foreground", "$text"),
        ("button.hover.background", "panel.hovered.background"),
        ("button.active.background", "$current-background"),
        (
            "button.primary.background",
            "ahead.button.primary.background",
        ),
        (
            "button.primary.foreground",
            "ahead.button.primary.foreground",
        ),
        (
            "button.primary.hover.background",
            "ahead.button.primary.hover",
        ),
        (
            "button.primary.active.background",
            "ahead.button.primary.active",
        ),
        ("button.secondary.background", "$secondary-background"),
        ("button.secondary.foreground", "$text"),
        (
            "button.secondary.hover.background",
            "panel.hovered.background",
        ),
        (
            "button.secondary.active.background",
            "panel.hovered.active.background",
        ),
        ("button.success.background", "$green"),
        (
            "button.success.foreground",
            "ahead.button.primary.foreground",
        ),
        (
            "button.success.hover.background",
            "ahead.button.primary.hover",
        ),
        (
            "button.success.active.background",
            "ahead.button.primary.active",
        ),
        ("button.info.background", "$cyan"),
        ("button.info.foreground", "ahead.button.primary.foreground"),
        ("button.info.hover.background", "ahead.button.primary.hover"),
        (
            "button.info.active.background",
            "ahead.button.primary.active",
        ),
        ("button.warning.background", "$yellow"),
        (
            "button.warning.foreground",
            "ahead.button.primary.foreground",
        ),
        (
            "button.warning.hover.background",
            "ahead.button.primary.hover",
        ),
        (
            "button.warning.active.background",
            "ahead.button.primary.active",
        ),
        ("button.danger.background", "$red"),
        (
            "button.danger.foreground",
            "ahead.button.primary.foreground",
        ),
        (
            "button.danger.hover.background",
            "ahead.button.primary.hover",
        ),
        (
            "button.danger.active.background",
            "ahead.button.primary.active",
        ),
        ("group_box.background", "$popup-background"),
        ("group_box.foreground", "$text"),
        ("group_box.title.foreground", "$text"),
        ("caret", "editor.caret"),
        ("chart.1", "$blue"),
        ("chart.2", "$cyan"),
        ("chart.3", "$green"),
        ("chart.4", "$yellow"),
        ("chart.5", "$red"),
        ("chart.bullish", "$green"),
        ("chart.bearish", "$red"),
        ("danger.background", "$red"),
        ("danger.active.background", "ahead.button.primary.active"),
        ("danger.foreground", "ahead.button.primary.foreground"),
        ("danger.hover.background", "ahead.button.primary.hover"),
        ("description_list.label.background", "$secondary-background"),
        ("description_list.label.foreground", "$dim-text"),
        ("drag.border", "$green"),
        ("drop_target.background", "editor.drag_drop_background"),
        ("info.background", "$cyan"),
        ("info.active.background", "ahead.button.primary.active"),
        ("info.foreground", "ahead.button.primary.foreground"),
        ("info.hover.background", "ahead.button.primary.hover"),
        ("link", "editor.link"),
        ("link.active", "editor.link"),
        ("link.hover", "editor.link"),
        ("list.background", "panel.background"),
        ("list.active.border", "$green"),
        ("list.active.background", "editor.selection"),
        ("list.even.background", "panel.background"),
        ("list.head.background", "$secondary-background"),
        ("list.hover.background", "panel.hovered.background"),
        ("muted.background", "$current-background"),
        ("muted.foreground", "$dim-text"),
        ("popover.background", "$popup-background"),
        ("popover.foreground", "$text"),
        ("primary.background", "$green"),
        ("primary.active.background", "ahead.button.primary.active"),
        ("primary.foreground", "ahead.button.primary.foreground"),
        ("primary.hover.background", "ahead.button.primary.hover"),
        ("progress.bar.background", "$green"),
        ("progress.ring", "editor.focus"),
        ("scrollbar.background", "$secondary-background"),
        ("scrollbar.thumb.background", "$input-border"),
        ("scrollbar.thumb.hover.background", "$dim-text"),
        ("secondary.background", "$current-background"),
        (
            "secondary.active.background",
            "panel.hovered.active.background",
        ),
        ("secondary.foreground", "$text"),
        ("secondary.hover.background", "panel.hovered.background"),
        ("selection.background", "editor.selection"),
        ("sidebar.background", "panel.background"),
        ("sidebar.foreground", "$text"),
        ("sidebar.border", "ahead.border"),
        ("sidebar.accent.background", "$current-background"),
        ("sidebar.accent.foreground", "$text"),
        ("sidebar.primary.background", "$green"),
        (
            "sidebar.primary.foreground",
            "ahead.button.primary.foreground",
        ),
        ("skeleton.background", "$secondary-background"),
        ("slider.background", "$green"),
        ("slider.thumb.background", "ahead.button.primary.foreground"),
        ("success.background", "$green"),
        ("success.active.background", "ahead.button.primary.active"),
        ("success.foreground", "ahead.button.primary.foreground"),
        ("success.hover.background", "ahead.button.primary.hover"),
        ("switch.background", "$input-border"),
        ("switch.thumb.background", "ahead.button.primary.foreground"),
        ("tab.background", "ahead.tab.inactive.background"),
        ("tab.foreground", "$dim-text"),
        ("tab.active.background", "ahead.tab.active.background"),
        ("tab.active.foreground", "$text"),
        ("tab_bar.background", "$secondary-background"),
        ("tab_bar.segmented.background", "$secondary-background"),
        ("table.background", "editor.background"),
        ("table.active.background", "editor.selection"),
        ("table.active.border", "$green"),
        ("table.even.background", "$secondary-background"),
        ("table.head.background", "$secondary-background"),
        ("table.head.foreground", "$text"),
        ("table.foot.background", "$secondary-background"),
        ("table.foot.foreground", "$text"),
        ("table.hover.background", "panel.hovered.background"),
        ("table.row.border", "ahead.border"),
        ("title_bar.background", "$secondary-background"),
        ("title_bar.border", "ahead.border"),
        ("status_bar.background", "status.background"),
        ("status_bar.border", "ahead.border"),
        ("warning.background", "$yellow"),
        ("warning.active.background", "ahead.button.primary.active"),
        ("warning.foreground", "ahead.button.primary.foreground"),
        ("warning.hover.background", "ahead.button.primary.hover"),
        ("window.border", "ahead.border"),
    ] {
        colors.insert(target.into(), json!(palette.color(source)?));
    }
    for name in ["blue", "cyan", "green", "magenta", "red", "yellow"] {
        colors.insert(
            format!("base.{name}"),
            json!(palette.color(&format!("terminal.{name}"))?),
        );
        colors.insert(
            format!("base.{name}.light"),
            json!(palette.color(&format!("terminal.bright_{name}"))?),
        );
    }
    colors.insert("scrollbar.background".into(), json!("#00000000"));
    colors.insert("overlay".into(), json!("#00000033"));

    let mut highlight = Map::new();
    for (target, source) in [
        ("editor.background", "editor.background"),
        ("editor.foreground", "editor.foreground"),
        ("editor.active_line.background", "editor.current_line"),
        ("editor.line_number", "$dim-text"),
        ("editor.active_line_number", "$green"),
        ("editor.invisible", "editor.visible_whitespace"),
        ("editor.gutter.background", "editor.background"),
        ("error", "$red"),
        ("error.background", "error_lens.error.background"),
        ("error.border", "$red"),
        ("warning", "$yellow"),
        ("warning.background", "error_lens.warning.background"),
        ("warning.border", "$yellow"),
        ("info", "$cyan"),
        ("info.background", "editor.selection"),
        ("info.border", "$cyan"),
        ("success", "$green"),
        ("success.background", "editor.selection"),
        ("success.border", "$green"),
        ("hint", "$dim-text"),
        ("hint.background", "editor.current_line"),
        ("hint.border", "$dim-text"),
    ] {
        highlight.insert(target.into(), json!(palette.color(source)?));
    }
    let mut syntax = Map::new();
    for (target, source) in [
        ("attribute", "attribute"),
        ("boolean", "boolean"),
        ("comment", "comment"),
        ("comment_doc", "comment"),
        ("constant", "constant"),
        ("constructor", "constructor"),
        ("embedded", "embedded"),
        ("emphasis", "markup.italic"),
        ("emphasis.strong", "markup.bold"),
        ("enum", "enum"),
        ("function", "function"),
        ("hint", "comment"),
        ("keyword", "keyword"),
        ("label", "text.reference"),
        ("link_text", "markup.link.text"),
        ("link_uri", "markup.link.url"),
        ("number", "number"),
        ("operator", "operator"),
        ("predictive", "comment"),
        ("preproc", "keyword"),
        ("primary", "variable"),
        ("property", "property"),
        ("punctuation", "punctuation"),
        ("punctuation.bracket", "punctuation"),
        ("punctuation.delimiter", "punctuation.delimiter"),
        ("punctuation.list_marker", "markup.list"),
        ("punctuation.special", "embedded"),
        ("string", "string"),
        ("string.escape", "string.escape"),
        ("string.regex", "string"),
        ("string.special", "string"),
        ("string.special.symbol", "constant"),
        ("tag", "tag"),
        ("tag.doctype", "keyword"),
        ("text.code.span", "string"),
        ("text.literal", "string"),
        ("title", "text.title"),
        ("type", "type"),
        ("variable", "variable"),
        ("variable.special", "selfKeyword"),
        ("variant", "enumMember"),
    ] {
        let color = palette
            .syntax
            .get(source)
            .with_context(|| format!("Missing syntax color: {source}"))?;
        let mut style = Map::new();
        style.insert("color".into(), json!(palette.resolve(color)?));
        match target {
            "emphasis" => {
                style.insert("font_style".into(), json!("italic"));
            }
            "emphasis.strong" => {
                style.insert("font_weight".into(), json!(700));
            }
            "link_uri" => {
                style.insert("font_style".into(), json!("italic"));
            }
            _ => {}
        }
        syntax.insert(target.into(), Value::Object(style));
    }
    highlight.insert("syntax".into(), json!(syntax));
    let mut theme = json!({
        "name": palette.name,
        "mode": mode,
        "font.size": font_size,
        "mono_font.size": font_size,
        "radius": 1,
        "radius.lg": 1,
        "shadow": false,
        "colors": colors,
        "highlight": highlight,
    });
    if let Some(font_family) = ui
        .get("font-family")
        .and_then(toml::Value::as_str)
        .filter(|value| !value.is_empty())
    {
        theme
            .as_object_mut()
            .context("Theme value is not an object")?
            .insert("font.family".into(), json!(font_family));
    }
    Ok(theme)
}

pub(crate) fn default_themes() -> Result<(ThemeConfig, ThemeConfig)> {
    Ok((
        serde_json::from_value(theme_value(
            include_str!("../../defaults/dark-theme.toml"),
            "dark",
        )?)?,
        serde_json::from_value(theme_value(
            include_str!("../../defaults/light-theme.toml"),
            "light",
        )?)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb(color: &str) -> Result<[f64; 3]> {
        let value =
            u32::from_str_radix(color.strip_prefix('#').context("Missing #")?, 16)?;
        let value = if color.len() == 9 { value >> 8 } else { value };
        Ok([16, 8, 0].map(|shift| f64::from((value >> shift) & 255) / 255.0))
    }

    fn contrast(foreground: [f64; 3], background: [f64; 3]) -> f64 {
        let luminance = |color: [f64; 3]| {
            color
                .into_iter()
                .zip([0.2126, 0.7152, 0.0722])
                .map(|(channel, weight)| {
                    weight
                        * if channel <= 0.04045 {
                            channel / 12.92
                        } else {
                            ((channel + 0.055) / 1.055).powf(2.4)
                        }
                })
                .sum::<f64>()
        };
        let a = luminance(foreground);
        let b = luminance(background);
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn text_and_syntax_keep_contrast_in_both_modes() -> Result<()> {
        for source in [
            include_str!("../../defaults/dark-theme.toml"),
            include_str!("../../defaults/light-theme.toml"),
        ] {
            let palette = toml::from_str::<ThemeFile>(source)?.palette;
            let editor = rgb(&palette.color("editor.background")?)?;
            let current_line = rgb(&palette.color("editor.current_line")?)?;
            let selection = palette.color("editor.selection")?;
            let alpha = (f64::from(
                u32::from_str_radix(selection.trim_start_matches('#'), 16)? & 255,
            ) / 255.0)
                .min(0.3);
            let mut selected = editor;
            for ((channel, background), overlay) in
                selected.iter_mut().zip(editor).zip(rgb(&selection)?)
            {
                *channel = background * (1.0 - alpha) + overlay * alpha;
            }
            for (name, color) in &palette.syntax {
                for background in [editor, current_line, selected] {
                    let ratio = contrast(rgb(&palette.resolve(color)?)?, background);
                    assert!(ratio >= 4.5, "{} {name}: {ratio:.2}:1", palette.name);
                }
            }
            for surface in [
                "$primary-background",
                "$secondary-background",
                "$popup-background",
                "$current-background",
            ] {
                for foreground in [
                    "$text",
                    "$dim-text",
                    "$green",
                    "$red",
                    "$yellow",
                    "$cyan",
                    "$magenta",
                ] {
                    let ratio = contrast(
                        rgb(&palette.color(foreground)?)?,
                        rgb(&palette.color(surface)?)?,
                    );
                    assert!(
                        ratio >= 4.5,
                        "{} {foreground} on {surface}: {ratio:.2}:1",
                        palette.name
                    );
                }
            }
            for state in ["background", "hover", "active"] {
                assert!(
                    contrast(
                        rgb(&palette.color("ahead.button.primary.foreground")?)?,
                        rgb(&palette
                            .color(&format!("ahead.button.primary.{state}"))?)?
                    ) >= 4.5
                );
            }
            assert!(contrast(rgb(&palette.color("$input-border")?)?, editor) >= 3.0);
        }
        Ok(())
    }

    #[test]
    fn defaults_load_with_syntax_and_semantics() -> Result<()> {
        let (dark, light) = default_themes()?;
        assert!(dark.mode.is_dark());
        assert!(!light.mode.is_dark());
        for theme in [dark, light] {
            assert_eq!(theme.radius, Some(1));
            assert_eq!(theme.radius_lg, Some(1));
            assert!(theme.highlight.is_some());
            assert_eq!(theme.colors.primary, theme.colors.success);
            assert_ne!(theme.colors.primary, theme.colors.danger);
        }
        let mut palette = toml::from_str::<ThemeFile>(include_str!(
            "../../defaults/dark-theme.toml"
        ))?
        .palette;
        assert!(palette.resolve("$missing").is_err());
        palette.base.insert("cycle".into(), "$cycle".into());
        assert!(palette.resolve("$cycle").is_err());
        Ok(())
    }

    #[test]
    fn emitted_themes_cover_gpui_kit_tokens() -> Result<()> {
        for (source, mode) in [
            (include_str!("../../defaults/dark-theme.toml"), "dark"),
            (include_str!("../../defaults/light-theme.toml"), "light"),
        ] {
            let value = theme_value(source, mode)?;
            let colors = value
                .get("colors")
                .and_then(Value::as_object)
                .context("Theme colors are not an object")?;
            let expected_colors = serde_json::to_value(
                gpui_kit::component::theme::ThemeConfigColors::default(),
            )?;
            for token in expected_colors
                .as_object()
                .context("GPUI colors are not an object")?
                .keys()
            {
                assert!(
                    colors.contains_key(token),
                    "{mode} theme is missing GPUI color token {token}"
                );
            }

            let highlight = value
                .get("highlight")
                .and_then(Value::as_object)
                .context("Theme highlight is not an object")?;
            let expected_highlight = serde_json::to_value(
                gpui_kit::component::highlighter::HighlightThemeStyle::default(),
            )?;
            for token in expected_highlight
                .as_object()
                .context("GPUI highlight is not an object")?
                .keys()
                .filter(|token| *token != "syntax")
            {
                assert!(
                    highlight.contains_key(token),
                    "{mode} theme is missing GPUI highlight token {token}"
                );
            }

            let syntax = highlight
                .get("syntax")
                .and_then(Value::as_object)
                .context("Theme syntax is not an object")?;
            let expected_syntax = serde_json::to_value(
                gpui_kit::component::highlighter::SyntaxColors::default(),
            )?;
            for token in expected_syntax
                .as_object()
                .context("GPUI syntax is not an object")?
                .keys()
            {
                assert!(
                    syntax.contains_key(token),
                    "{mode} theme is missing GPUI syntax token {token}"
                );
            }
            assert_eq!(syntax["emphasis"]["font_style"], "italic");
            assert_eq!(syntax["emphasis.strong"]["font_weight"], 700);
            assert_eq!(syntax["link_uri"]["font_style"], "italic");

            serde_json::from_value::<ThemeConfig>(value)?;
        }
        Ok(())
    }
}
