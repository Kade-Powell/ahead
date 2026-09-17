//! AHEAD Application Shell - GPUI Application Lifecycle
//!
//! Zed-style layout: explorer on the left dock, code in the center,
//! agent + threads as right-dock tabs, terminal on the bottom dock,
//! with TitleBar chrome on top and a StatusBar along the bottom.

use gpui_kit::*;
use gpui_kit::component::dock::{DockArea, DockLayout, DockPlacement, DockSkin, PanelStyle, panel_handle};
use gpui_kit::component::status_bar::StatusBar;
use gpui_kit::component::TitleBar;
use gpui_kit::component::{h_flex, v_flex, ActiveTheme};
use gpui_kit_assets::IconName;

pub struct Shell {
    pub area: Entity<DockArea>,
}

impl Render for Shell {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let border = cx.theme().border;
        let text = cx.theme().sidebar_foreground;
        v_flex()
            .size_full()
            .child(TitleBar::new().child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(IconName::Zap)
                    .child(
                        div()
                            .font_weight(gpui_kit::FontWeight::BOLD)
                            .text_size(px(12.))
                            .text_color(text)
                            .child("AHEAD"),
                    ),
            ))
            .child(div().flex_1().child(self.area.clone()))
            .child(
                StatusBar::new()
                    .left(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(IconName::GitBranch)
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(text)
                                    .child("main · AHEAD session active"),
                            ),
                    )
                    .right(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(IconName::CircleCheck)
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(text)
                                    .child("proxy connected · 0 problems"),
                            ),
                    )
                    .border_t_1()
                    .border_color(border),
            )
    }
}

pub fn launch() {
    let args: Vec<String> = std::env::args().collect();
    let mut file_path = String::new();
    let mut root_arg: Option<String> = None;

    for arg in args.into_iter().skip(1) {
        if arg.starts_with('-') {
            continue;
        }
        if file_path.is_empty() {
            file_path = arg;
        } else if root_arg.is_none() {
            root_arg = Some(arg);
        }
    }

    if file_path.is_empty() {
        file_path = "/tmp/ahead-sample.rs".to_string();
    }

    let explorer_root = root_arg
        .or_else(|| {
            std::path::Path::new(&file_path)
                .parent()
                .and_then(|p| p.to_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| "/Users/kpowel859@cable.comcast.com/dev/ahead".to_string());

    if !std::path::Path::new(&file_path).exists() {
        let _ = std::fs::write(
            &file_path,
            "//! AHEAD Engineering Session\n\npub struct ServiceConfig {\n    pub max_retries: u32,\n    pub backoff_ms: u64,\n}\n\nimpl ServiceConfig {\n    pub fn default() -> Self {\n        Self {\n            max_retries: 3,\n            backoff_ms: 200,\n        }\n    }\n}\n",
        );
    }

    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);
        gpui_kit::component::Theme::change(gpui_kit::component::ThemeMode::Dark, None, cx);
        let path = file_path.clone();
        let explorer_root = explorer_root.clone();
        cx.spawn(async move |cx| {
            cx.open_window(
                TitleBar::window_options(),
                |window, cx| {
                    let (area, skin) = DockSkin::dock_area("ahead-shell", None, window, cx);
                    skin.set_panel_style(PanelStyle::TabBar, cx);

                    let code = cx.new(|cx| crate::code_panel::CodePanel::new(&path, window, cx));
                    let session = cx.new(|cx| crate::session_panel::SessionPanel::new(window, cx));
                    let threads = cx.new(|cx| crate::threads_panel::ThreadsPanel::new(window, cx));
                    let explorer =
                        cx.new(|cx| crate::explorer_panel::ExplorerPanel::new(&explorer_root, window, cx));
                    let terminal = cx.new(|cx| crate::terminal_panel::TerminalPanel::new(window, cx));
                    let settings = cx.new(|cx| crate::settings_panel::SettingsPanel::new(window, cx));

                    let center = DockLayout::tabs().panel_view(panel_handle(code), cx);
                    let right = DockLayout::tabs()
                        .panel_view(panel_handle(session), cx)
                        .panel_view(panel_handle(threads), cx)
                        .panel_view(panel_handle(settings), cx);
                    let left = DockLayout::tabs().panel_view(panel_handle(explorer), cx);
                    let bottom = DockLayout::tabs().panel_view(panel_handle(terminal), cx);

                    area.update(cx, |area, cx| {
                        area.set_center(center, window, cx);
                        area.set_dock(DockPlacement::Left, left, window, cx);
                        area.set_dock(DockPlacement::Right, right, window, cx);
                        area.set_dock(DockPlacement::Bottom, bottom, window, cx);
                    });
                    let shell = cx.new(|_| Shell { area });
                    cx.new(|cx| gpui_kit::component::Root::new(shell, window, cx))
                },
            )
            .expect("Failed to open AHEAD window");
        })
        .detach();
    });
}
