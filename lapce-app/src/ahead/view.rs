//! AHEAD Floem UI Components
//!
//! Grounded in Section 4.4 and Section 4.5 of `ahead-editor-mvp.md`.
//! Includes:
//! - Status bar indicator for Work Kind, Workflow Phase, and Assistance Mode
//! - Start Work 5-step interactive wizard modal
//! - Non-caret-stealing Presentation Cue card and Teaching Highlight overlay

use std::sync::Arc;
use floem::{
    peniko::Color,
    reactive::{ReadSignal, SignalGet, SignalUpdate},
    style::{CursorStyle, Display, JustifyContent, Position},
    views::{container, label, stack, text_input, Decorators},
    View,
};
use lapce_rpc::ahead::{AssistanceMode, WorkKind};

use crate::{
    ahead::state::AheadState,
    config::{color::LapceColor, LapceConfig},
};

/// Status bar indicator for active AHEAD session
pub fn ahead_status_item(
    ahead_state: AheadState,
    _config: ReadSignal<Arc<LapceConfig>>,
) -> impl View {
    stack((
        label(move || {
            let session = ahead_state.active_session.get();
            match session {
                Some(view) => {
                    let mode_str = match view.session.mode {
                        AssistanceMode::Learn => "Learn",
                        AssistanceMode::Assist => "Assist",
                    };
                    format!(
                        "⚡ AHEAD: {} · {} · [{}]",
                        view.session.work_kind.display_name(),
                        view.workflow.phase.title,
                        mode_str
                    )
                }
                None => "⚡ AHEAD: Start Work".to_string(),
            }
        })
        .on_click_stop(move |_| {
            if ahead_state.active_session.get().is_some() {
                ahead_state.toggle_mode();
            } else {
                ahead_state.show_start_work_modal.update(|v| *v = !*v);
            }
        })
        .style(move |s| {
            let session = ahead_state.active_session.get();
            let (bg, fg) = match session {
                Some(view) => match view.session.mode {
                    AssistanceMode::Learn => (Color::from_rgb8(34, 139, 34), Color::from_rgb8(255, 255, 255)), // Forest Green
                    AssistanceMode::Assist => (Color::from_rgb8(65, 105, 225), Color::from_rgb8(255, 255, 255)), // Royal Blue
                },
                None => (Color::from_rgb8(75, 85, 99), Color::from_rgb8(255, 255, 255)), // Charcoal Slate
            };

            s.background(bg)
                .color(fg)
                .padding_horiz(10.0)
                .padding_vert(3.0)
                .border_radius(4.0)
                .margin_horiz(4.0)
                .cursor(CursorStyle::Pointer)
                .font_bold()
        }),
    ))
    .style(|s| s.items_center().height_pct(100.0))
}

/// Interactive Start Work Modal
pub fn start_work_modal(
    ahead_state: AheadState,
    config: ReadSignal<Arc<LapceConfig>>,
) -> impl View {
    let title_input = floem::reactive::create_rw_signal("Implement Feature".to_string());
    let selected_kind = floem::reactive::create_rw_signal(WorkKind::ProductChange);
    let selected_mode = floem::reactive::create_rw_signal(AssistanceMode::Assist);

    container(
        stack((
            // Header
            label(|| "AHEAD: Start Work Session".to_string()).style(move |s| {
                s.font_size(18.0)
                    .font_bold()
                    .margin_bottom(12.0)
                    .color(config.get().color(LapceColor::EDITOR_FOREGROUND))
            }),
            // Step 1: Work Kind Selection
            label(|| "1. Name the Outcome".to_string()).style(move |s| {
                s.font_bold()
                    .margin_bottom(6.0)
                    .color(config.get().color(LapceColor::EDITOR_FOREGROUND))
            }),
            stack((
                work_kind_btn(selected_kind, WorkKind::ProductChange, "Product Change"),
                work_kind_btn(selected_kind, WorkKind::CorrectiveDebugging, "Debug / Fix"),
                work_kind_btn(selected_kind, WorkKind::InternalImprovement, "Improvement"),
                work_kind_btn(selected_kind, WorkKind::Investigation, "Investigation"),
                work_kind_btn(selected_kind, WorkKind::Decision, "Decision"),
                work_kind_btn(selected_kind, WorkKind::OperationalStabilization, "Stabilize"),
            ))
            .style(|s| s.margin_bottom(12.0)),

            // Step 2: Mode Selection
            label(|| "2. Assistance Mode".to_string()).style(move |s| {
                s.font_bold()
                    .margin_bottom(6.0)
                    .color(config.get().color(LapceColor::EDITOR_FOREGROUND))
            }),
            stack((
                mode_btn(selected_mode, AssistanceMode::Assist, "Assist (Guided / Mechanical)"),
                mode_btn(selected_mode, AssistanceMode::Learn, "Learn (Read-Only / Socratic)"),
            ))
            .style(|s| s.margin_bottom(12.0)),

            // Step 3: Title
            label(|| "3. Work Title / Objective".to_string()).style(move |s| {
                s.font_bold()
                    .margin_bottom(6.0)
                    .color(config.get().color(LapceColor::EDITOR_FOREGROUND))
            }),
            text_input(title_input)
                .style(move |s| {
                    s.width(400.0)
                        .padding(6.0)
                        .border(1.0)
                        .border_color(Color::from_rgb8(120, 120, 120))
                        .border_radius(4.0)
                        .margin_bottom(16.0)
                        .background(config.get().color(LapceColor::EDITOR_BACKGROUND))
                        .color(config.get().color(LapceColor::EDITOR_FOREGROUND))
                }),

            // Action Buttons
            stack((
                label(|| "Begin Session".to_string())
                    .on_click_stop(move |_| {
                        ahead_state.start_local_session(
                            selected_kind.get(),
                            selected_mode.get(),
                            title_input.get(),
                        );
                    })
                    .style(move |s| {
                        s.background(Color::from_rgb8(37, 99, 235))
                            .color(Color::from_rgb8(255, 255, 255))
                            .padding_horiz(16.0)
                            .padding_vert(8.0)
                            .border_radius(4.0)
                            .cursor(CursorStyle::Pointer)
                            .font_bold()
                            .margin_right(8.0)
                    }),
                label(|| "Cancel".to_string())
                    .on_click_stop(move |_| {
                        ahead_state.show_start_work_modal.set(false);
                    })
                    .style(move |s| {
                        s.background(Color::from_rgb8(75, 85, 99))
                            .color(Color::from_rgb8(255, 255, 255))
                            .padding_horiz(16.0)
                            .padding_vert(8.0)
                            .border_radius(4.0)
                            .cursor(CursorStyle::Pointer)
                    }),
            )),
        ))
        .style(move |s| {
            s.flex_col()
                .padding(24.0)
                .background(config.get().color(LapceColor::PANEL_BACKGROUND))
                .border(1.0)
                .border_color(Color::from_rgb8(80, 80, 80))
                .border_radius(8.0)
        }),
    )
    .style(move |s| {
        let is_open = ahead_state.show_start_work_modal.get();
        s.position(Position::Absolute)
            .size_pct(100.0, 100.0)
            .justify_content(Some(JustifyContent::Center))
            .items_center()
            .background(Color::from_rgba8(0, 0, 0, 180))
            .display(if is_open { Display::Flex } else { Display::None })
            .z_index(100)
    })
}

fn work_kind_btn(
    selected: floem::reactive::RwSignal<WorkKind>,
    target: WorkKind,
    text: &'static str,
) -> impl View {
    label(move || text.to_string())
        .on_click_stop(move |_| selected.set(target))
        .style(move |s| {
            let is_sel = selected.get() == target;
            let bg = if is_sel { Color::from_rgb8(37, 99, 235) } else { Color::from_rgb8(55, 65, 81) };
            s.background(bg)
                .color(Color::from_rgb8(255, 255, 255))
                .padding_horiz(8.0)
                .padding_vert(4.0)
                .border_radius(4.0)
                .margin_right(4.0)
                .cursor(CursorStyle::Pointer)
                .font_size(12.0)
        })
}

fn mode_btn(
    selected: floem::reactive::RwSignal<AssistanceMode>,
    target: AssistanceMode,
    text: &'static str,
) -> impl View {
    label(move || text.to_string())
        .on_click_stop(move |_| selected.set(target))
        .style(move |s| {
            let is_sel = selected.get() == target;
            let bg = if is_sel {
                match target {
                    AssistanceMode::Learn => Color::from_rgb8(34, 139, 34),
                    AssistanceMode::Assist => Color::from_rgb8(37, 99, 235),
                }
            } else {
                Color::from_rgb8(55, 65, 81)
            };
            s.background(bg)
                .color(Color::from_rgb8(255, 255, 255))
                .padding_horiz(10.0)
                .padding_vert(5.0)
                .border_radius(4.0)
                .margin_right(8.0)
                .cursor(CursorStyle::Pointer)
                .font_size(12.0)
        })
}

/// Presentation Cue overlay (non-caret-stealing pointer & highlight card)
pub fn presentation_cue_card(
    ahead_state: AheadState,
    _config: ReadSignal<Arc<LapceConfig>>,
) -> impl View {
    let has_cue = move || ahead_state.presentation_cue.get().is_some();

    container(
        stack((
            label(move || {
                ahead_state.presentation_cue.get()
                    .map(|c| format!("🎯 {} (pointing to {}:{})", c.label, c.anchor.path, c.anchor.range.start.line + 1))
                    .unwrap_or_default()
            })
            .style(move |s| {
                s.font_size(13.0)
                    .color(Color::from_rgb8(255, 255, 255))
                    .margin_right(8.0)
            }),
            label(|| "Dismiss".to_string())
                .on_click_stop(move |_| {
                    ahead_state.presentation_cue.set(None);
                })
                .style(move |s| {
                    s.font_size(11.0)
                        .color(Color::from_rgb8(200, 200, 200))
                        .cursor(CursorStyle::Pointer)
                        .padding_horiz(4.0)
                }),
        ))
        .style(move |s| {
            s.padding_horiz(12.0)
                .padding_vert(6.0)
                .background(Color::from_rgba8(30, 41, 59, 230))
                .border(1.0)
                .border_color(Color::from_rgb8(59, 130, 246))
                .border_radius(6.0)
                .items_center()
        }),
    )
    .style(move |s| {
        s.position(Position::Absolute)
            .size_pct(100.0, 100.0)
            .items_end()
            .padding_top(10.0)
            .padding_right(20.0)
            .display(if has_cue() { Display::Flex } else { Display::None })
            .pointer_events_none()
            .z_index(50)
    })
}
