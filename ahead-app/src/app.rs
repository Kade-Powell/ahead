//! AHEAD Application Shell - GPUI Application Lifecycle
//!
//! 4-column Zed-style layout:
//! - Left dock: File explorer (toggleable via bottom icon row)
//! - Center: Code editor on top, interactive PTY terminal on bottom (toggleable via bottom icon row)
//! - Right dock: AHEAD Agent chat & Work Items checklist tabs, Threads sidebar on far-right (toggleable via bottom icon row)
//! - Top: TitleBar chrome with brand and native traffic lights
//! - Bottom: Rich StatusBar with dock toggles and panel switchers

use gpui_kit::*;
use gpui_kit::component::button::Button;
use gpui_kit::component::dock::{
    DockArea, DockLayout, DockPlacement, DockSkin, PanelStyle, panel_handle,
};
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
        let area_left = self.area.clone();
        let area_bottom = self.area.clone();
        let area_right = self.area.clone();

        v_flex()
            .size_full()
            .child(
                TitleBar::new().child(
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
                ),
            )
            .child(div().flex_1().child(self.area.clone()))
            .child(
                StatusBar::new()
                    // Left Region: Dock toggles and panel quick-switchers
                    .left(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .child(
                                Button::new("toggle_left_dock")
                                    .icon(IconName::PanelLeft)
                                    .on_click(cx.listener(move |_, _, window, cx| {
                                        area_left.update(cx, |area, cx| area.toggle_dock(DockPlacement::Left, window, cx));
                                    }))
                            )
                            .child(
                                Button::new("files_btn")
                                    .icon(IconName::Folder)
                                    .label("Files")
                                    .on_click(cx.listener({
                                        let area = self.area.clone();
                                        move |_, _, window, cx| {
                                            area.update(cx, |area, cx| {
                                                if !area.is_dock_open(DockPlacement::Left) {
                                                    area.toggle_dock(DockPlacement::Left, window, cx);
                                                }
                                            });
                                        }
                                    }))
                            )
                            .child(
                                Button::new("git_btn")
                                    .icon(IconName::GitBranch)
                                    .label("main")
                            )
                            .child(
                                Button::new("toggle_bottom_dock")
                                    .icon(IconName::PanelBottom)
                                    .on_click(cx.listener(move |_, _, window, cx| {
                                        area_bottom.update(cx, |area, cx| area.toggle_dock(DockPlacement::Bottom, window, cx));
                                    }))
                            )
                            .child(
                                Button::new("terminal_btn")
                                    .icon(IconName::Terminal)
                                    .label("Terminal")
                                    .on_click(cx.listener({
                                        let area = self.area.clone();
                                        move |_, _, window, cx| {
                                            area.update(cx, |area, cx| {
                                                if !area.is_dock_open(DockPlacement::Bottom) {
                                                    area.toggle_dock(DockPlacement::Bottom, window, cx);
                                                }
                                            });
                                        }
                                    }))
                            )
                            .child(
                                Button::new("problems_btn")
                                    .icon(IconName::ShieldCheck)
                                    .label("0 Problems")
                            )
                    )
                    // Center Region: Active work session chip
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(IconName::Zap)
                            .child(
                                div()
                                    .font_weight(gpui_kit::FontWeight::BOLD)
                                    .text_size(px(11.))
                                    .text_color(text)
                                    .child("AHEAD: Implement resilient retry logic [Assist · Plan]")
                            )
                    )
                    // Right Region: Editor stats and Right dock panel switchers
                    .right(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(text)
                                    .child("Ln 1, Col 1 · Rust · UTF-8")
                            )
                            .child(
                                Button::new("agent_btn")
                                    .icon(IconName::MessageSquare)
                                    .label("Agent")
                                    .on_click(cx.listener({
                                        let area = self.area.clone();
                                        move |_, _, window, cx| {
                                            area.update(cx, |area, cx| {
                                                if !area.is_dock_open(DockPlacement::Right) {
                                                    area.toggle_dock(DockPlacement::Right, window, cx);
                                                }
                                            });
                                        }
                                    }))
                            )
                            .child(
                                Button::new("work_items_btn")
                                    .icon(IconName::ListTodo)
                                    .label("Work Items")
                                    .on_click(cx.listener({
                                        let area = self.area.clone();
                                        move |_, _, window, cx| {
                                            area.update(cx, |area, cx| {
                                                if !area.is_dock_open(DockPlacement::Right) {
                                                    area.toggle_dock(DockPlacement::Right, window, cx);
                                                }
                                            });
                                        }
                                    }))
                            )
                            .child(
                                Button::new("threads_btn")
                                    .icon(IconName::Layers)
                                    .label("Threads")
                                    .on_click(cx.listener({
                                        let area = self.area.clone();
                                        move |_, _, window, cx| {
                                            area.update(cx, |area, cx| {
                                                if !area.is_dock_open(DockPlacement::Right) {
                                                    area.toggle_dock(DockPlacement::Right, window, cx);
                                                }
                                            });
                                        }
                                    }))
                            )
                            .child(
                                Button::new("toggle_right_dock")
                                    .icon(IconName::PanelRight)
                                    .on_click(cx.listener(move |_, _, window, cx| {
                                        area_right.update(cx, |area, cx| area.toggle_dock(DockPlacement::Right, window, cx));
                                    }))
                            )
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

    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            gpui_kit::component::Theme::change(gpui_kit::component::ThemeMode::Dark, None, cx);
            let path = file_path.clone();
            let explorer_root = explorer_root.clone();
            cx.spawn(async move |cx| {
                cx.open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(Bounds {
                            origin: Point::new(px(40.), px(40.)),
                            size: Size::new(px(1600.), px(1000.)),
                        })),
                        ..TitleBar::window_options()
                    },
                    |window, cx| {
                        let (area, skin) = DockSkin::dock_area("ahead-shell", None, window, cx);
                        skin.set_panel_style(PanelStyle::TabBar, cx);

                        let code = cx.new(|cx| crate::code_panel::CodePanel::new(&path, window, cx));
                        let session = cx.new(|cx| crate::session_panel::SessionPanel::new(window, cx));
                        let work_items = cx.new(|cx| crate::work_items_panel::WorkItemsPanel::new(window, cx));
                        let threads = cx.new(|cx| crate::threads_panel::ThreadsPanel::new(window, cx));
                        let explorer = cx.new(|cx| crate::explorer_panel::ExplorerPanel::new(&explorer_root, window, cx));
                        let terminal = cx.new(|cx| crate::terminal_panel::TerminalPanel::new(window, cx));

                        // Center: Code on top, Terminal on bottom (vertical split)
                        let center = DockLayout::v_split()
                            .child(DockLayout::tabs().panel_view(panel_handle(code), cx), None)
                            .child(DockLayout::tabs().panel_view(panel_handle(terminal), cx), Some(px(220.)));

                        // Right dock: Agent chat and Work Items checklist as tabs, Threads sidebar on far right
                        let right = DockLayout::h_split()
                            .child(
                                DockLayout::tabs()
                                    .panel_view(panel_handle(session), cx)
                                    .panel_view(panel_handle(work_items), cx),
                                None,
                            )
                            .child(
                                DockLayout::tabs().panel_view(panel_handle(threads), cx),
                                Some(px(240.)),
                            );

                        // Left dock: File Explorer
                        let left = DockLayout::tabs().panel_view(panel_handle(explorer), cx);

                        area.update(cx, |area, cx| {
                            area.set_center(center, window, cx);
                            area.set_dock(DockPlacement::Left, left, window, cx);
                            area.set_dock(DockPlacement::Right, right, window, cx);
                            area.set_dock_size(DockPlacement::Right, px(720.), window, cx);
                            area.set_dock_size(DockPlacement::Left, px(240.), window, cx);
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
