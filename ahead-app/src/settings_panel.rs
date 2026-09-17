//! AHEAD settings panel - theme and appearance chrome.

use gpui_kit::*;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::dock::{BasePanel, Panel, PanelControl, PanelEvent};
use gpui_kit::component::{h_flex, v_flex, ActiveTheme};
use gpui_kit_assets::IconName;

pub struct SettingsPanel {
    pub focus: FocusHandle,
    pub status: SharedString,
}

impl SettingsPanel {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
            status: "Appearance settings".into(),
        }
    }
}

impl BasePanel for SettingsPanel {
    fn panel_name(&self) -> &'static str {
        "ahead_settings"
    }
}

impl Panel for SettingsPanel {
    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        "Settings"
    }
    fn zoom_control(&self, _: &App) -> Option<PanelControl> {
        Some(PanelControl::Toolbar)
    }
}

impl EventEmitter<PanelEvent> for SettingsPanel {}

impl Focusable for SettingsPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for SettingsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let border = cx.theme().border;
        let text = cx.theme().sidebar_foreground;
        let group = cx.theme().group_box;
        let is_dark = cx.theme().mode.is_dark();
        v_flex()
            .size_full()
            .p_3()
            .gap_2()
            .track_focus(&self.focus)
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(IconName::Settings)
                    .child(
                        div()
                            .font_weight(gpui_kit::FontWeight::BOLD)
                            .text_size(px(12.))
                            .text_color(text)
                            .child("SETTINGS"),
                    ),
            )
            .child(
                v_flex()
                    .gap_1()
                    .p_3()
                    .rounded_lg()
                    .bg(group)
                    .border_1()
                    .border_color(border)
                    .child(
                        div()
                            .font_weight(gpui_kit::FontWeight::BOLD)
                            .text_size(px(11.))
                            .text_color(text)
                            .child("APPEARANCE"),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(text)
                            .child(format!(
                                "Theme: {} (follows gpui-kit Theme::change)",
                                if is_dark { "Dark" } else { "Light" }
                            )),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                Button::new("settings_dark")
                                    .primary()
                                    .icon(IconName::Moon)
                                    .label("Dark")
                                    .on_click(cx.listener(|this: &mut Self, _, window, cx| {
                                        gpui_kit::component::Theme::change(
                                            gpui_kit::component::ThemeMode::Dark,
                                            Some(window),
                                            cx,
                                        );
                                        this.status = "Theme: Dark".into();
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("settings_light")
                                    .icon(IconName::Sun)
                                    .label("Light")
                                    .on_click(cx.listener(|this: &mut Self, _, window, cx| {
                                        gpui_kit::component::Theme::change(
                                            gpui_kit::component::ThemeMode::Light,
                                            Some(window),
                                            cx,
                                        );
                                        this.status = "Theme: Light".into();
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .child(
                div()
                    .text_size(px(10.))
                    .text_color(text)
                    .child(self.status.clone()),
            )
    }
}
