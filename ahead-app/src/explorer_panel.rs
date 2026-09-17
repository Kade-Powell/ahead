//! AHEAD file explorer panel - left sidebar file tree.

use gpui_kit::*;
use gpui_kit::component::button::Button;
use gpui_kit::component::dock::{BasePanel, Panel, PanelControl, PanelEvent};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{h_flex, v_flex, ActiveTheme};
use gpui_kit_assets::IconName;

pub struct ExplorerPanel {
    pub focus: FocusHandle,
    pub search: Entity<InputState>,
    pub root: String,
    pub entries: Vec<ExplorerEntry>,
    pub status: SharedString,
}

#[derive(Clone, Debug)]
pub struct ExplorerEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub depth: usize,
}

impl ExplorerPanel {
    pub fn new(root: &str, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx));
        let mut panel = Self {
            focus: cx.focus_handle(),
            search,
            root: root.to_string(),
            entries: Vec::new(),
            status: "Explorer ready".into(),
        };
        panel.refresh(cx);
        panel
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let query = self.search.read(cx).value().to_lowercase();
        let mut entries = Vec::new();
        Self::collect(&std::path::PathBuf::from(&self.root), &self.root, 0, &query, &mut entries);
        entries.sort_by(|a: &ExplorerEntry, b: &ExplorerEntry| {
            b.is_dir.cmp(&a.is_dir).then_with(|| a.name.cmp(&b.name))
        });
        self.entries = entries;
        self.status = format!("{} entries", self.entries.len()).into();
        cx.notify();
    }

    fn collect(dir: &std::path::Path, root: &str, depth: usize, query: &str, out: &mut Vec<ExplorerEntry>) {
        let Ok(read) = std::fs::read_dir(dir) else { return };
        let mut names: Vec<std::path::PathBuf> = read.filter_map(|e| e.ok().map(|e| e.path())).collect();
        names.sort();
        for path in names.into_iter().take(400) {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("?").to_string();
            if name.starts_with('.') || name == "target" || name == "node_modules" {
                continue;
            }
            let is_dir = path.is_dir();
            if !query.is_empty() && !name.to_lowercase().contains(query) && !is_dir {
                continue;
            }
            out.push(ExplorerEntry {
                name,
                path: path.to_string_lossy().to_string(),
                is_dir,
                depth,
            });
            if is_dir && depth < 2 {
                Self::collect(&path, root, depth + 1, query, out);
            }
        }
    }
}

impl BasePanel for ExplorerPanel {
    fn panel_name(&self) -> &'static str {
        "ahead_explorer"
    }
}

impl Panel for ExplorerPanel {
    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        "Explorer"
    }
    fn zoom_control(&self, _: &App) -> Option<PanelControl> {
        Some(PanelControl::Toolbar)
    }
}

impl EventEmitter<PanelEvent> for ExplorerPanel {}

impl Focusable for ExplorerPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ExplorerPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let border = cx.theme().border;
        let text = cx.theme().sidebar_foreground;
        let group = cx.theme().group_box;
        v_flex()
            .size_full()
            .p_2()
            .gap_1()
            .track_focus(&self.focus)
            .child(
                v_flex()
                    .gap_1()
                    .p_2()
                    .rounded_lg()
                    .bg(group)
                    .border_1()
                    .border_color(border)
                    .child(Input::new(&self.search).aria_label("Filter files"))
                    .child(
                        Button::new("explorer_refresh")
                            .icon(IconName::RefreshCw)
                            .label("Refresh")
                            .on_click(cx.listener(|this: &mut Self, _, _, cx| this.refresh(cx))),
                    ),
            )
            .child(
                div()
                    .px_2()
                    .pt_1()
                    .font_weight(gpui_kit::FontWeight::BOLD)
                    .text_size(px(11.))
                    .text_color(text)
                    .child(self.root.clone()),
            )
            .child(
                v_flex()
                    .flex_1()
                    .gap_0()
                    .children(self.entries.iter().map(|entry| {
                        h_flex()
                            .gap_1()
                            .items_center()
                            .px_2()
                            .py_0()
                            .child(if entry.is_dir { IconName::Folder } else { IconName::File })
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(text)
                                    .child(format!("{}{}", "  ".repeat(entry.depth), entry.name.clone())),
                            )
                    })),
            )
            .child(
                div()
                    .pt_1()
                    .text_size(px(10.))
                    .text_color(text)
                    .child(self.status.clone()),
            )
    }
}
