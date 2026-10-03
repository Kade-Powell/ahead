use std::rc::Rc;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::dock::{
    BasePanel, Panel, PanelControl, PanelEvent, PanelId, TabGroup,
};
use gpui_kit::component::text::TextView;
use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use gpui_kit_assets::IconName;

const TOPICS: &[(&str, &str)] = &[
    ("Start here", include_str!("../../docs/guide/start-here.md")),
    (
        "Files and editing",
        include_str!("../../docs/guide/files-and-editing.md"),
    ),
    (
        "Search and Git",
        include_str!("../../docs/guide/search-and-git.md"),
    ),
    (
        "Terminal and debugging",
        include_str!("../../docs/guide/terminal-and-debugging.md"),
    ),
    (
        "Agent sessions",
        include_str!("../../docs/guide/agent-sessions.md"),
    ),
    (
        "Connections and settings",
        include_str!("../../docs/guide/connections-and-settings.md"),
    ),
    (
        "Voice and sharing",
        include_str!("../../docs/guide/voice-and-sharing.md"),
    ),
    ("Shortcuts", include_str!("../../docs/guide/shortcuts.md")),
];

pub struct HelpPanel {
    focus: FocusHandle,
    selected: usize,
    panel_id: PanelId,
    tab_group: Option<WeakEntity<TabGroup>>,
    close_handler: Option<Rc<dyn Fn(PanelId, &mut Window, &mut App)>>,
}

impl HelpPanel {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
            selected: 0,
            panel_id: PanelId::from(cx.entity_id()),
            tab_group: None,
            close_handler: None,
        }
    }

    pub fn set_close_handler<F>(&mut self, handler: F)
    where
        F: Fn(PanelId, &mut Window, &mut App) + 'static,
    {
        self.close_handler = Some(Rc::new(handler));
    }
}

impl BasePanel for HelpPanel {
    fn panel_name(&self) -> &'static str {
        "ahead_help"
    }

    fn on_added_to(
        &mut self,
        group: WeakEntity<TabGroup>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) {
        self.tab_group = Some(group);
    }
}

impl Panel for HelpPanel {
    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let group = self.tab_group.clone();
        let panel_id = self.panel_id;
        let close_handler = self.close_handler.clone();
        h_flex().items_center().gap_1().child("Help").child(
            Button::new("close_help")
                .icon(IconName::X)
                .ghost()
                .tooltip("Close Help")
                .on_click(move |_, window, cx| {
                    if let Some(handler) = close_handler.as_ref() {
                        handler(panel_id, window, cx);
                    } else if let Some(group) = group.as_ref() {
                        _ = group
                            .update(cx, |group, cx| group.close_panel(panel_id, cx));
                    }
                }),
        )
    }

    fn zoom_control(&self, _: &App) -> Option<PanelControl> {
        Some(PanelControl::Toolbar)
    }
}

impl EventEmitter<PanelEvent> for HelpPanel {}

impl Focusable for HelpPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for HelpPanel {
    fn render(
        &mut self,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let selected = self.selected;
        h_flex()
            .size_full()
            .min_h_0()
            .track_focus(&self.focus)
            .child(
                v_flex()
                    .w(px(210.))
                    .h_full()
                    .p_3()
                    .gap_1()
                    .border_r_1()
                    .border_color(cx.theme().border)
                    .children(TOPICS.iter().enumerate().map(
                        |(index, (title, _))| {
                            let button = Button::new(("help_topic", index))
                                .label(*title)
                                .ghost()
                                .tooltip(format!("Read {title}"));
                            let button = if index == selected {
                                button.text_color(cx.theme().success)
                            } else {
                                button
                            };
                            button.on_click(cx.listener(
                                move |this: &mut Self, _, _, cx| {
                                    this.selected = index;
                                    cx.notify();
                                },
                            ))
                        },
                    )),
            )
            .child(
                div().flex_1().h_full().min_w_0().p_4().child(
                    TextView::markdown(
                        SharedString::from(format!("ahead-help-{selected}")),
                        SharedString::from(TOPICS[selected].1),
                    )
                    .scrollable(true),
                ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::TOPICS;

    #[test]
    fn guide_topics_have_distinct_titles_and_content() {
        let mut titles = std::collections::HashSet::new();
        for (title, content) in TOPICS {
            assert!(titles.insert(title));
            assert!(content.starts_with("# "), "{title} needs a heading");
            assert!(content.len() > 300, "{title} needs useful instructions");
        }
    }
}
