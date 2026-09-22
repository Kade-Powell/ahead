# AHEAD brand and themes

<img src="../../extra/images/logo.svg" width="96" height="96" alt="AHEAD logo"/>

Use the original stacked-A mark with its floating crossbar. Do not connect
either end of the crossbar to a leg. The product name is **AHEAD**; executable
names, bundle paths, and package identifiers remain unchanged.

## Assets

| Use | Asset |
| --- | --- |
| Native UI, tinted by the active theme | `icons/ahead_logo.svg` |
| README and other flat artwork | `extra/images/logo.svg` |
| Compatibility copy of the flat mark | `extra/images/logo_color.svg` |
| App icon source | `extra/images/logo_app.svg` |
| Compatibility copy of the app icon | `extra/images/logo_app_light.svg` |
| Linux and raster consumers | `extra/images/logo.png` |
| macOS bundle and running Dock icon | `extra/macos/Ahead.app/Contents/Resources/ahead.icns` |
| Windows packaging and installer | `extra/windows/ahead.ico` |

The three filled paths are identical across the SVGs. Keep their proportions
and transparent padding. Flat artwork has no glow, filters, or embedded bitmap;
the app tile adds a dark rounded surface and a restrained green gradient.
Both app-icon SVG names intentionally use the same artwork.

Use `crate::app::ahead_icon(size, cx)` in the native UI. It renders through
gpui-kit's `Icon` and takes its color from `cx.theme().primary`. Keep accessible
labels on icon-only branding. Standard action and status icons still use Lucide.

After changing the source SVG, regenerate the 1024-pixel PNG, multi-resolution
ICNS, and ICO. Keep the three flat SVG copies and both tile sources in sync.
The ICO includes 16, 24, 32, 48, 64, 128, and 256-pixel images; inspect small sizes
as well as the full-size artwork before replacing exports.

## Palette

The greens come from [Green Serenity, ColorDrop palette 26760](https://colordrop.io/palette/26760).

| Role | Color |
| --- | --- |
| Soft green; dark-theme strings and app-icon highlight | `#A7D6A1` |
| Main green; dark-theme controls and functions | `#5DAE6D` |
| Deep green; flat logo and app-icon base | `#4CA15D` |
| Darkened green for readable light-theme controls | `#2F713F` |

Charcoal and off-white surfaces have a slight green tint. Ordinary identifiers
stay neutral; types use cyan, keywords violet, numbers warm tan, and strings
green. Comments remain legible rather than fading into the background.

| Meaning | Dark | Light |
| --- | --- | --- |
| Success and added lines | `#5DAE6D` | `#2F713F` |
| Error, deleted lines, breakpoint | `#EB8FA8` | `#AB2854` |
| Warning and modified lines | `#D6B66D` | `#856011` |
| Information | `#76C5D5` | `#176F83` |
| Agent attribution | `#C8A8DF` | `#794C99` |

These semantic hues adapt the supplied emerald color-wheel reference rather
than copying its saturated swatches. Icons, labels, and gutter positions also
communicate status; color alone is not enough.

## Theme implementation

`defaults/dark-theme.toml` and `defaults/light-theme.toml` are the color sources.
`ahead-app/src/theme.rs` resolves their base aliases into gpui-kit's native
`ThemeConfig`, including syntax highlighting, terminal ANSI colors, and UI
semantics. Settings can switch between the two configurations. Use theme tokens
in panels instead of hard-coded colors or separate dark/light branches.

Keep ordinary text and syntax at least 4.5:1 against their backgrounds and
control boundaries at least 3:1. Decorative borders and invisible-character
guides are intentionally quieter. The light theme darkens the brand green for
text and controls; do not substitute the flat logo green for body text.

Run the theme checks with `cargo test -p ahead-app --lib theme::tests`.
Also inspect both themes in a newly rebuilt app: editor selections, terminal,
completion popovers, agent attribution, controls, and the operating-system icon.
