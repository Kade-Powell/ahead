//! AHEAD Floem UI Components
//!
//! Grounded in Section 4.4 and Section 4.5 of `ahead-editor-mvp.md`.
//! Includes:
//! - Status bar indicator for Work Kind, Workflow Phase, and Assistance Mode
//! - Start Work 5-step interactive wizard modal
//! - Non-caret-stealing Presentation Cue card and Teaching Highlight overlay

use std::{rc::Rc, sync::Arc};
use floem::{
    ext_event::create_ext_action,
    peniko::Color,
    reactive::{ReadSignal, SignalGet, SignalUpdate},
    style::{CursorStyle, Display, JustifyContent, Position},
    views::{container, dyn_stack, label, scroll, stack, text_input, Decorators},
    View,
};
use lapce_rpc::ahead::{
    AssistanceMode, SessionParticipantRecord, SessionRole, SessionView, WorkKind, WorkflowState,
};
use crate::editor::location::{EditorLocation, EditorPosition};

use crate::{
    ahead::state::AheadState,
    config::{color::LapceColor, LapceConfig},
    panel::{kind::PanelKind, position::PanelPosition},
    window_tab::WindowTabData,
};

/// Status bar indicator for the active AHEAD session.
///
/// The session chip opens the Start Work wizard. It never mutates policy:
/// Learn/Assist switching is an explicit host-backed control in the agent panel.
pub fn ahead_status_item(
    ahead_state: AheadState,
    _config: ReadSignal<Arc<LapceConfig>>,
    window_tab_data: Rc<WindowTabData>,
) -> impl View {
    let window_tab_data_ai = window_tab_data.clone();
    let window_tab_data_voice = window_tab_data.clone();
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
            ahead_state.show_start_work_modal.update(|v| *v = !*v);
        })
        .style(move |s| {
            let session = ahead_state.active_session.get();
            let (bg, fg) = match session {
                Some(view) => match view.session.mode {
                    AssistanceMode::Learn => (Color::from_rgb8(34, 139, 34), Color::from_rgb8(255, 255, 255)), // Forest Green
                    AssistanceMode::Assist => (Color::from_rgb8(16, 185, 129), Color::from_rgb8(6, 40, 30)), // Emerald Green
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
        // Dedicated ✨ AI Agent Panel Toggle
        label(|| "✨ AI Agent".to_string())
            .on_click_stop(move |_| {
                window_tab_data_ai.toggle_panel_visual(PanelKind::AheadAgent);
            })
            .style(move |s| {
                s.background(Color::from_rgb8(30, 41, 59))
                    .color(Color::from_rgb8(245, 158, 11)) // Amber / Sparkle
                    .padding_horiz(8.0)
                    .padding_vert(3.0)
                    .border_radius(4.0)
                    .margin_right(4.0)
                    .cursor(CursorStyle::Pointer)
                    .font_bold()
                    .font_size(12.0)
            }),
        // Dedicated 🎙️ Voice Chat Indicator & Toggle
        label(move || {
            if ahead_state.voice_active.get() {
                "🎙️ Voice: Live".to_string()
            } else {
                "🎙️ Voice: Off".to_string()
            }
        })
        .on_click_stop(move |_| {
            ahead_state.toggle_voice();
            if ahead_state.voice_active.get() {
                window_tab_data_voice.show_panel(PanelKind::AheadAgent);
            }
        })
        .style(move |s| {
            let is_voice = ahead_state.voice_active.get();
            let bg = if is_voice {
                Color::from_rgb8(16, 185, 129) // Emerald Green
            } else {
                Color::from_rgb8(51, 65, 85) // Slate
            };
            s.background(bg)
                .color(Color::from_rgb8(255, 255, 255))
                .padding_horiz(8.0)
                .padding_vert(3.0)
                .border_radius(4.0)
                .margin_right(4.0)
                .cursor(CursorStyle::Pointer)
                .font_size(12.0)
        }),
        // GitHub Account badge / sign-in affordance
        label(move || {
            if let Some(u) = ahead_state.authenticated_user.get() {
                format!("👤 @{}", u.login)
            } else {
                "👤 Sign In".to_string()
            }
        })
        .on_click_stop(move |_| {
            ahead_state.show_auth_modal.update(|v| *v = !*v);
        })
        .style(move |s| {
            let is_auth = ahead_state.authenticated_user.get().is_some();
            let bg = if is_auth { Color::from_rgb8(31, 41, 55) } else { Color::from_rgb8(55, 65, 81) };
            s.background(bg)
                .color(Color::from_rgb8(229, 231, 235))
                .padding_horiz(8.0)
                .padding_vert(3.0)
                .border_radius(4.0)
                .margin_right(4.0)
                .cursor(CursorStyle::Pointer)
                .font_size(12.0)
        }),
        // Collaboration Modal Trigger
        label(move || {
            let count = ahead_state.workspace_participants.get().len();
            if count > 0 {
                format!("👥 Team ({})", count)
            } else {
                "👥 Add People".to_string()
            }
        })
        .on_click_stop(move |_| {
            ahead_state.show_collab_modal.update(|v| *v = !*v);
        })
        .style(move |s| {
            s.background(Color::from_rgb8(15, 23, 42))
                .color(Color::from_rgb8(52, 211, 153))
                .border(1.0)
                .border_color(Color::from_rgb8(16, 185, 129))
                .padding_horiz(8.0)
                .padding_vert(3.0)
                .border_radius(4.0)
                .margin_right(4.0)
                .cursor(CursorStyle::Pointer)
                .font_size(12.0)
        }),
    ))
    .style(|s| s.items_center().height_pct(100.0))
}

/// Start Work wizard: five short, resumable steps backed by the session host.
///
/// Steps: 1) choose work (resume saved or start new), 2) name the outcome,
/// 3) give a starting point, 4) set collaboration (mode + private/shared),
/// 5) begin a durable session via StartWork RPC.
pub fn start_work_modal(
    window_tab_data: Rc<WindowTabData>,
    config: ReadSignal<Arc<LapceConfig>>,
) -> impl View {
    let ahead_state = window_tab_data.ahead;
    let scope = window_tab_data.scope;
    let proxy = window_tab_data.common.proxy.clone();
    let window_tab_data_begin = window_tab_data.clone();
    let selected_kind = floem::reactive::create_rw_signal(WorkKind::ProductChange);
    let selected_mode = floem::reactive::create_rw_signal(AssistanceMode::Assist);
    let selected_resume = floem::reactive::create_rw_signal(Option::<String>::None);

    container(
        stack((
            stack((
                // Header
                label(|| "AHEAD: Start Work Session".to_string()).style(move |s| {
                    s.font_size(18.0)
                        .font_bold()
                        .margin_bottom(4.0)
                        .color(config.get().color(LapceColor::EDITOR_FOREGROUND))
                }),
                label(move || {
                    format!(
                        "Step {} of 5 · durable session in the workspace store",
                        (ahead_state.wizard_step.get() + 1).min(5)
                    )
                })
                .style(move |s| {
                    s.font_size(12.0)
                        .margin_bottom(12.0)
                        .color(config.get().color(LapceColor::EDITOR_DIM))
                }),
                // Step 1: Choose work (resume or new)
                label(|| "1. Choose work: resume saved or start new".to_string()).style(move |s| {
                    s.font_bold()
                        .margin_bottom(6.0)
                        .color(config.get().color(LapceColor::EDITOR_FOREGROUND))
                }),
                dyn_stack(
                    move || ahead_state.saved_sessions.get(),
                    |v: &SessionView| v.session.id.clone(),
                    move |v: SessionView| {
                        let id = v.session.id.clone();
                        let title = v.session.title.clone();
                        let phase = v.workflow.phase.title.clone();
                        let is_sel = selected_resume.get().as_deref() == Some(id.as_str());
                        label(move || format!("Resume: {} · {}", title.clone(), phase.clone()))
                            .on_click_stop(move |_| {
                                selected_resume.set(Some(id.clone()));
                            })
                            .style(move |s| {
                                let bg = if is_sel { Color::from_rgb8(16, 185, 129) } else { Color::from_rgb8(55, 65, 81) };
                                let fg = if is_sel { Color::from_rgb8(6, 40, 30) } else { Color::from_rgb8(255, 255, 255) };
                                s.background(bg)
                                    .color(fg)
                                    .padding_horiz(8.0)
                                    .padding_vert(4.0)
                                    .border_radius(4.0)
                                    .margin_right(4.0)
                                    .margin_bottom(4.0)
                                    .cursor(CursorStyle::Pointer)
                                    .font_size(12.0)
                            })
                    },
                )
                .style(|s| s.flex_col().margin_bottom(8.0)),
                label(|| "Or start new below — resuming reopens the same durable session.".to_string())
                    .style(move |s| {
                        s.font_size(11.0)
                            .margin_bottom(12.0)
                            .color(config.get().color(LapceColor::EDITOR_DIM))
                    }),
            ))
            .style(|s| s.flex_col()),
            stack((
                // Step 2: Name the outcome
                label(|| "2. Name the outcome (work kind + title)".to_string()).style(move |s| {
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
                text_input(ahead_state.wizard_title)
                    .placeholder("Work title, e.g. Implement retry backoff")
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
            ))
            .style(|s| s.flex_col()),
            stack((
                // Step 3: Starting point
                label(|| "3. Give your starting point (typed; voice transcript lands here)".to_string()).style(move |s| {
                    s.font_bold()
                        .margin_bottom(6.0)
                        .color(config.get().color(LapceColor::EDITOR_FOREGROUND))
                }),
                text_input(ahead_state.wizard_starting_point)
                    .placeholder("Observed vs expected, invariant, hypothesis, or open question")
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
                // Step 4: Collaboration (mode + privacy)
                label(|| "4. Set the collaboration (mode + sharing)".to_string()).style(move |s| {
                    s.font_bold()
                        .margin_bottom(6.0)
                        .color(config.get().color(LapceColor::EDITOR_FOREGROUND))
                }),
                stack((
                    mode_btn(selected_mode, AssistanceMode::Assist, "Assist (Guided / Mechanical)"),
                    mode_btn(selected_mode, AssistanceMode::Learn, "Learn (Read-Only / Socratic)"),
                ))
                .style(|s| s.margin_bottom(8.0)),
                label(move || {
                    if ahead_state.wizard_private.get() {
                        "Private session: stays on this machine until explicitly shared.".to_string()
                    } else {
                        "Shared session: teammates added via Team see session content.".to_string()
                    }
                })
                .style(move |s| {
                    s.font_size(11.0)
                        .margin_bottom(6.0)
                        .color(config.get().color(LapceColor::EDITOR_DIM))
                }),
                label(move || {
                    if ahead_state.wizard_private.get() { "🔒 Private (switch to Shared)".to_string() } else { "👥 Shared (switch to Private)".to_string() }
                })
                .on_click_stop(move |_| {
                    ahead_state.wizard_private.update(|v| *v = !*v);
                })
                .style(move |s| {
                    s.background(Color::from_rgb8(55, 65, 81))
                        .color(Color::from_rgb8(255, 255, 255))
                        .padding_horiz(10.0)
                        .padding_vert(5.0)
                        .border_radius(4.0)
                        .margin_bottom(16.0)
                        .cursor(CursorStyle::Pointer)
                        .font_size(12.0)
                }),
            ))
            .style(|s| s.flex_col()),
            stack((
                // Error surface
                label(move || ahead_state.session_error.get().unwrap_or_default())
                    .style(move |s| {
                        let has_err = ahead_state.session_error.get().is_some();
                        s.font_size(12.0)
                            .margin_bottom(if has_err { 8.0 } else { 0.0 })
                            .color(Color::from_rgb8(248, 113, 113))
                            .display(if has_err { Display::Flex } else { Display::None })
                    }),
                // Step 5: Begin (durable host session or resume)
                label(|| "5. Begin: opens the session workspace in the right panel".to_string()).style(move |s| {
                    s.font_bold()
                        .margin_bottom(6.0)
                        .color(config.get().color(LapceColor::EDITOR_FOREGROUND))
                }),
                stack((
                    label(move || {
                        if ahead_state.session_busy.get() { "Working…".to_string() } else { "Begin Session".to_string() }
                    })
                        .on_click_stop(move |_| {
                            begin_session_from_wizard(ahead_state, scope, proxy.clone(), window_tab_data_begin.clone(), selected_kind.get_untracked(), selected_mode.get_untracked(), selected_resume.get_untracked());
                        })
                        .style(move |s| {
                            s.background(Color::from_rgb8(16, 185, 129))
                                .color(Color::from_rgb8(6, 40, 30))
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
            .style(|s| s.flex_col()),
        ))
        .style(move |s| {
            s.flex_col()
                .padding(24.0)
                .background(config.get().color(LapceColor::PANEL_BACKGROUND))
                .border(1.0)
                .border_color(Color::from_rgb8(80, 80, 80))
                .border_radius(8.0)
                .width(460.0)
        }),
    )
    .style(move |s| {
        s.position(Position::Absolute)
            .size_pct(100.0, 100.0)
            .items_center()
            .justify_center()
            .background(Color::from_rgba8(0, 0, 0, 180))
            .display(if ahead_state.show_start_work_modal.get() { Display::Flex } else { Display::None })
            .z_index(100)
    })
}

fn begin_session_from_wizard(
    ahead_state: AheadState,
    scope: floem::reactive::Scope,
    proxy: lapce_rpc::proxy::ProxyRpcHandler,
    window_tab_data: Rc<WindowTabData>,
    kind: WorkKind,
    mode: AssistanceMode,
    resume_id: Option<String>,
) {
    if ahead_state.session_busy.get_untracked() {
        return;
    }
    if let Some(resume_id) = resume_id {
        ahead_state.session_busy.set(true);
        let send = create_ext_action(scope, move |res: Result<serde_json::Value, lapce_rpc::RpcError>| {
            match res {
                Ok(val) => match serde_json::from_value::<Option<SessionView>>(val) {
                    Ok(Some(view)) => {
                        ahead_state.adopt_durable_session(view);
                        ahead_state.push_message("AHEAD Agent", "Reopened the saved session. Next useful step is in the plan phase.".to_string(), false);
                        window_tab_data.show_panel(PanelKind::AheadAgent);
                    }
                    _ => ahead_state.session_error.set(Some("Saved session no longer exists in the store.".to_string())),
                },
                Err(e) => ahead_state.session_error.set(Some(format!("Resume failed: {}", e.message))),
            }
            ahead_state.session_busy.set(false);
        });
        proxy.ahead_request(
            lapce_rpc::ahead::AheadRequest::GetSession { session_id: resume_id },
            move |res| send(res),
        );
        return;
    }
    let title = ahead_state.wizard_title.get_untracked().trim().to_string();
    if title.is_empty() {
        ahead_state.session_error.set(Some("Give the work a title before beginning.".to_string()));
        return;
    }
    let starting_point = ahead_state.wizard_starting_point.get_untracked();
    ahead_state.session_busy.set(true);
    ahead_state.session_error.set(None);
    let send = create_ext_action(scope, move |res: Result<serde_json::Value, lapce_rpc::RpcError>| {
        match res {
            Ok(val) => match serde_json::from_value::<SessionView>(val) {
                Ok(view) => {
                    let phase_title = view.workflow.phase.title.clone();
                    ahead_state.adopt_durable_session(view.clone());
                    ahead_state.saved_sessions.update(|list| {
                        if !list.iter().any(|v| v.session.id == view.session.id) {
                            list.insert(0, view.clone());
                        }
                    });
                    ahead_state.push_message("AHEAD Agent", format!("Session started. Current phase: {}. State your next reasoning step.", phase_title), false);
                    window_tab_data.show_panel(PanelKind::AheadAgent);
                }
                Err(_) => ahead_state.session_error.set(Some("Session host returned an unreadable session.".to_string())),
            },
            Err(e) => ahead_state.session_error.set(Some(format!("Begin failed: {}", e.message))),
        }
        ahead_state.session_busy.set(false);
    });
    proxy.ahead_request(
        lapce_rpc::ahead::AheadRequest::StartWork {
            work_kind: kind,
            mode,
            title,
            starting_point,
            work_item: None,
        },
        move |res| send(res),
    );
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
            let bg = if is_sel { Color::from_rgb8(16, 185, 129) } else { Color::from_rgb8(55, 65, 81) };
            let fg = if is_sel { Color::from_rgb8(6, 40, 30) } else { Color::from_rgb8(255, 255, 255) };
            s.background(bg)
                .color(fg)
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
            let (bg, fg) = if is_sel {
                match target {
                    AssistanceMode::Learn => (Color::from_rgb8(34, 139, 34), Color::from_rgb8(255, 255, 255)),
                    AssistanceMode::Assist => (Color::from_rgb8(16, 185, 129), Color::from_rgb8(6, 40, 30)),
                }
            } else {
                (Color::from_rgb8(55, 65, 81), Color::from_rgb8(255, 255, 255))
            };
            s.background(bg)
                .color(fg)
                .padding_horiz(10.0)
                .padding_vert(5.0)
                .border_radius(4.0)
                .margin_right(8.0)
                .cursor(CursorStyle::Pointer)
                .font_size(12.0)
        })
}

/// Presentation Cue card: shows what the agent is pointing at and lets the
/// human jump there. The outer overlay ignores pointer events; the inner
/// card re-enables them so Dismiss / Show in editor stay clickable.
pub fn presentation_cue_card(
    window_tab_data: Rc<WindowTabData>,
    _config: ReadSignal<Arc<LapceConfig>>,
) -> impl View {
    let ahead_state = window_tab_data.ahead;
    let has_cue = move || ahead_state.presentation_cue.get().is_some();
    let ahead_state_jump = ahead_state;
    let window_tab_data_jump = window_tab_data.clone();

    container(
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
                label(|| "Show in editor".to_string())
                    .on_click_stop(move |_| {
                        if let Some(cue) = ahead_state_jump.presentation_cue.get_untracked() {
                            let workspace = window_tab_data_jump.workspace.clone();
                            if let Some(root) = workspace.path.clone() {
                                let path = root.join(&cue.anchor.path);
                                if path.exists() {
                                    window_tab_data_jump.main_split.jump_to_location(
                                        EditorLocation {
                                            path,
                                            position: Some(EditorPosition::Position(lsp_types::Position {
                                                line: cue.anchor.range.start.line,
                                                character: cue.anchor.range.start.col,
                                            })),
                                            scroll_offset: None,
                                            ignore_unconfirmed: false,
                                            same_editor_tab: false,
                                        },
                                        None,
                                    );
                                } else {
                                    ahead_state_jump.session_error.set(Some(format!("Cue target not in this worktree: {}", cue.anchor.path)));
                                }
                            }
                        }
                    })
                    .style(move |s| {
                        s.font_size(11.0)
                            .color(Color::from_rgb8(52, 211, 153))
                            .cursor(CursorStyle::Pointer)
                            .padding_horiz(6.0)
                            .font_bold()
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
                    .background(Color::from_rgba8(19, 62, 47, 230))
                    .border(1.0)
                    .border_color(Color::from_rgb8(16, 185, 129))
                    .border_radius(6.0)
                    .items_center()
            }),
        )
        .style(|s| s.pointer_events_auto()),
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

/// GitHub Account modal: explicit sign-in, no silent auto-import.
///
/// Shows loading / error / confirmation states, masks the token as a secret
/// (cleared after submit), and explains the token stays in ~/.ahead/auth.json.
pub fn github_auth_modal(
    window_tab_data: Rc<WindowTabData>,
    config: ReadSignal<Arc<LapceConfig>>,
) -> impl View {
    let ahead_state = window_tab_data.ahead;
    let scope = window_tab_data.scope;
    let proxy = window_tab_data.common.proxy.clone();

    container(
        stack((
            stack((
                label(|| "GitHub Authentication".to_string()).style(move |s| {
                    s.font_size(18.0)
                        .font_bold()
                        .margin_bottom(12.0)
                        .color(config.get().color(LapceColor::EDITOR_FOREGROUND))
                }),
                // User status info (never exposes raw email bulk; login + status only)
                label(move || {
                    if let Some(user) = ahead_state.authenticated_user.get() {
                        let auth_str = if user.is_authenticated {
                            "Authenticated — token stored locally in ~/.ahead/auth.json"
                        } else {
                            "Local Git identity (not authenticated to GitHub)"
                        };
                        format!(
                            "Signed in as @{}\nStatus: {}",
                            user.login,
                            auth_str
                        )
                    } else if let Some(code) = ahead_state.pending_device_code.get() {
                        format!("Enter code at {}:\n\n{}", code.verification_uri, code.user_code)
                    } else {
                        "Not connected to GitHub. Import the gh CLI session or paste a fine-grained token (repo + read:org scopes).".to_string()
                    }
                })
                .style(move |s| {
                    s.font_size(13.0)
                        .margin_bottom(8.0)
                        .color(config.get().color(LapceColor::EDITOR_FOREGROUND))
                }),
                label(|| "Tokens never leave this machine except to api.github.com for verification.".to_string())
                    .style(move |s| {
                        s.font_size(11.0)
                            .margin_bottom(16.0)
                            .color(config.get().color(LapceColor::EDITOR_DIM))
                    }),
                label(move || ahead_state.auth_error.get().unwrap_or_default())
                    .style(move |s| {
                        let has_err = ahead_state.auth_error.get().is_some();
                        s.font_size(12.0)
                            .margin_bottom(if has_err { 8.0 } else { 0.0 })
                            .color(Color::from_rgb8(248, 113, 113))
                            .display(if has_err { Display::Flex } else { Display::None })
                    }),
                // Token input if toggled (treated as secret: cleared on submit/close)
                stack((
                    text_input(ahead_state.token_input)
                        .placeholder("Secret token — paste, submit, it is cleared (ghp_... / gho_...)")
                        .style(move |s| {
                            let is_visible = ahead_state.show_token_input.get();
                            s.width(360.0)
                                .padding(6.0)
                                .border(1.0)
                                .border_color(Color::from_rgb8(120, 120, 120))
                                .border_radius(4.0)
                                .margin_bottom(10.0)
                                .background(config.get().color(LapceColor::EDITOR_BACKGROUND))
                                .color(config.get().color(LapceColor::EDITOR_FOREGROUND))
                                .display(if is_visible { Display::Flex } else { Display::None })
                        }),
                )),
            ))
            .style(|s| s.flex_col()),
            stack((
                // When authenticated: Sign Out
                label(|| "Sign Out".to_string())
                    .on_click_stop({
                        let proxy = proxy.clone();
                        move |_| {
                            ahead_state.auth_busy.set(true);
                            ahead_state.auth_error.set(None);
                            let send = create_ext_action(scope, move |res: Result<serde_json::Value, lapce_rpc::RpcError>| {
                                match res {
                                    Ok(val) => {
                                        if let Ok(user) = serde_json::from_value::<lapce_rpc::ahead::GitHubUser>(val) {
                                            ahead_state.authenticated_user.set(Some(user));
                                        }
                                        ahead_state.token_input.set(String::new());
                                    }
                                    Err(e) => ahead_state.auth_error.set(Some(format!("Sign out failed: {}", e.message))),
                                }
                                ahead_state.auth_busy.set(false);
                            });
                            proxy.ahead_request(
                                lapce_rpc::ahead::AheadRequest::GitHubAuthSignOut,
                                move |res| send(res),
                            );
                            ahead_state.pending_device_code.set(None);
                            ahead_state.show_token_input.set(false);
                        }
                    })
                    .style(move |s| {
                        let is_auth = ahead_state.authenticated_user.get().map(|u| u.is_authenticated).unwrap_or(false);
                        s.background(Color::from_rgb8(220, 38, 38))
                            .color(Color::from_rgb8(255, 255, 255))
                            .padding_horiz(14.0)
                            .padding_vert(6.0)
                            .border_radius(4.0)
                            .cursor(CursorStyle::Pointer)
                            .margin_right(8.0)
                            .display(if is_auth { Display::Flex } else { Display::None })
                    }),
                // When NOT authenticated: Connect via GitHub CLI
                label(move || {
                    if ahead_state.auth_busy.get() { "Working…".to_string() } else { "Import gh CLI session".to_string() }
                })
                    .on_click_stop({
                        let proxy = proxy.clone();
                        move |_| {
                            if ahead_state.auth_busy.get_untracked() {
                                return;
                            }
                            ahead_state.auth_busy.set(true);
                            ahead_state.auth_error.set(None);
                            let send = create_ext_action(scope, move |res: Result<serde_json::Value, lapce_rpc::RpcError>| {
                                match res {
                                    Ok(val) => match serde_json::from_value::<lapce_rpc::ahead::GitHubUser>(val) {
                                        Ok(user) => {
                                            ahead_state.authenticated_user.set(Some(user));
                                            ahead_state.show_auth_modal.set(false);
                                        }
                                        Err(_) => ahead_state.auth_error.set(Some("gh CLI returned an unreadable user.".to_string())),
                                    },
                                    Err(e) => ahead_state.auth_error.set(Some(format!("gh import failed (is `gh auth login` done?): {}", e.message))),
                                }
                                ahead_state.auth_busy.set(false);
                            });
                            proxy.ahead_request(
                                lapce_rpc::ahead::AheadRequest::GitHubAuthDetectCli,
                                move |res| send(res),
                            );
                        }
                    })
                    .style(move |s| {
                        let is_auth = ahead_state.authenticated_user.get().map(|u| u.is_authenticated).unwrap_or(false);
                        s.background(Color::from_rgb8(16, 185, 129))
                            .color(Color::from_rgb8(6, 40, 30))
                            .padding_horiz(14.0)
                            .padding_vert(6.0)
                            .border_radius(4.0)
                            .cursor(CursorStyle::Pointer)
                            .margin_right(8.0)
                            .display(if !is_auth && !ahead_state.show_token_input.get() { Display::Flex } else { Display::None })
                    }),
                // When NOT authenticated: Enter Token toggle / Submit
                label(move || {
                    if ahead_state.auth_busy.get() {
                        "Working…".to_string()
                    } else if ahead_state.show_token_input.get() {
                        "Submit Token".to_string()
                    } else {
                        "Sign in with Token".to_string()
                    }
                })
                .on_click_stop({
                    let proxy = proxy.clone();
                    move |_| {
                        if ahead_state.auth_busy.get_untracked() {
                            return;
                        }
                        if ahead_state.show_token_input.get() {
                            let token = ahead_state.token_input.get_untracked().trim().to_string();
                            if token.is_empty() {
                                ahead_state.auth_error.set(Some("Paste a token before submitting.".to_string()));
                                return;
                            }
                            ahead_state.auth_busy.set(true);
                            ahead_state.auth_error.set(None);
                            let send = create_ext_action(scope, move |res: Result<serde_json::Value, lapce_rpc::RpcError>| {
                                match res {
                                    Ok(val) => match serde_json::from_value::<lapce_rpc::ahead::GitHubUser>(val) {
                                        Ok(user) => {
                                            ahead_state.authenticated_user.set(Some(user));
                                            ahead_state.show_token_input.set(false);
                                            ahead_state.show_auth_modal.set(false);
                                        }
                                        Err(_) => ahead_state.auth_error.set(Some("GitHub rejected the token or returned an unreadable user.".to_string())),
                                    },
                                    Err(e) => ahead_state.auth_error.set(Some(format!("Sign in failed: {}", e.message))),
                                }
                                ahead_state.token_input.set(String::new());
                                ahead_state.auth_busy.set(false);
                            });
                            proxy.ahead_request(
                                lapce_rpc::ahead::AheadRequest::GitHubAuthSignInWithToken { token },
                                move |res| send(res),
                            );
                            ahead_state.token_input.set(String::new());
                        } else {
                            ahead_state.show_token_input.set(true);
                        }
                    }
                })
                .style(move |s| {
                    let is_auth = ahead_state.authenticated_user.get().map(|u| u.is_authenticated).unwrap_or(false);
                    s.background(Color::from_rgb8(16, 185, 129))
                        .color(Color::from_rgb8(255, 255, 255))
                        .padding_horiz(14.0)
                        .padding_vert(6.0)
                        .border_radius(4.0)
                        .cursor(CursorStyle::Pointer)
                        .margin_right(8.0)
                        .display(if !is_auth { Display::Flex } else { Display::None })
                }),
                // Close button
                label(|| "Close".to_string())
                    .on_click_stop(move |_| {
                        ahead_state.show_auth_modal.set(false);
                        ahead_state.show_token_input.set(false);
                        ahead_state.token_input.set(String::new());
                        ahead_state.auth_error.set(None);
                    })
                    .style(move |s| {
                        s.background(Color::from_rgb8(75, 85, 99))
                            .color(Color::from_rgb8(255, 255, 255))
                            .padding_horiz(14.0)
                            .padding_vert(6.0)
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
                .width(440.0)
        }),
    )
    .style(move |s| {
        let is_open = ahead_state.show_auth_modal.get();
        s.position(Position::Absolute)
            .size_pct(100.0, 100.0)
            .justify_content(Some(JustifyContent::Center))
            .items_center()
            .background(Color::from_rgba8(0, 0, 0, 180))
            .display(if is_open { Display::Flex } else { Display::None })
            .z_index(100)
    })
}

/// Workspace Team modal: workspace-level membership only.
///
/// Explains that adding a handle grants workspace file access (not just
/// session content), validates GitHub usernames, and surfaces add/revoke
/// errors inline instead of failing silently.
pub fn workspace_collab_modal(
    window_tab_data: Rc<WindowTabData>,
    config: ReadSignal<Arc<LapceConfig>>,
) -> impl View {
    let ahead_state = window_tab_data.ahead;
    let scope = window_tab_data.scope;
    let proxy = window_tab_data.common.proxy.clone();

    container(
        stack((
            stack((
                // Header
                label(|| "Workspace Team & Collaborators".to_string()).style(move |s| {
                    s.font_size(18.0)
                        .font_bold()
                        .margin_bottom(6.0)
                        .color(config.get().color(LapceColor::EDITOR_FOREGROUND))
                }),
                label(|| "Workspace membership grants file + session access. Session-only sharing is not supported yet — use Private in Start Work to keep work local.".to_string())
                    .style(move |s| {
                        s.font_size(12.0)
                            .margin_bottom(16.0)
                            .color(config.get().color(LapceColor::EDITOR_DIM))
                    }),
                // Section 1: Current Members
                label(|| "Current Workspace Members".to_string()).style(move |s| {
                    s.font_bold()
                        .margin_bottom(8.0)
                        .color(config.get().color(LapceColor::EDITOR_FOREGROUND))
                }),
                dyn_stack(
                    move || ahead_state.workspace_participants.get(),
                    |p: &SessionParticipantRecord| p.participant.id().to_string(),
                    {
                        let proxy = proxy.clone();
                        move |p: SessionParticipantRecord| {
                            let role_name = match p.role {
                                SessionRole::Owner => "Owner",
                                SessionRole::Editor => "Editor",
                                SessionRole::Reviewer => "Reviewer",
                                SessionRole::Viewer => "Viewer",
                            };
                            let (role_bg, role_fg) = match p.role {
                                SessionRole::Owner => (Color::from_rgb8(124, 58, 237), Color::from_rgb8(255, 255, 255)),
                                SessionRole::Editor => (Color::from_rgb8(37, 99, 235), Color::from_rgb8(255, 255, 255)),
                                SessionRole::Reviewer => (Color::from_rgb8(217, 119, 6), Color::from_rgb8(255, 255, 255)),
                                SessionRole::Viewer => (Color::from_rgb8(100, 116, 139), Color::from_rgb8(255, 255, 255)),
                            };
                            let is_owner = p.role == SessionRole::Owner;
                            let handle = p.participant.id().to_string();

                            stack((
                                label(move || format!("👤 @{} ({})", p.participant.id(), p.participant.display_name()))
                                    .style(move |s| {
                                        s.color(config.get().color(LapceColor::EDITOR_FOREGROUND))
                                            .margin_right(12.0)
                                            .flex_grow(1.0)
                                    }),
                                label(move || role_name.to_string()).style(move |s| {
                                    s.background(role_bg)
                                        .color(role_fg)
                                        .padding_horiz(8.0)
                                        .padding_vert(2.0)
                                        .border_radius(4.0)
                                        .font_size(11.0)
                                        .margin_right(10.0)
                                }),
                                label(|| "Revoke".to_string())
                                    .on_click_stop({
                                        let proxy = proxy.clone();
                                        let handle = handle.clone();
                                        move |_| {
                                            ahead_state.collab_error.set(None);
                                            let send = create_ext_action(scope, move |res: Result<serde_json::Value, lapce_rpc::RpcError>| {
                                                match res {
                                                    Ok(val) => match serde_json::from_value::<Vec<SessionParticipantRecord>>(val) {
                                                        Ok(parts) => ahead_state.workspace_participants.set(parts),
                                                        Err(_) => ahead_state.collab_error.set(Some("Revoke returned an unreadable member list.".to_string())),
                                                    },
                                                    Err(e) => ahead_state.collab_error.set(Some(format!("Revoke failed: {}", e.message))),
                                                }
                                            });
                                            proxy.ahead_request(
                                                lapce_rpc::ahead::AheadRequest::RevokeWorkspaceParticipant {
                                                    user_handle: handle.clone(),
                                                },
                                                move |res| send(res),
                                            );
                                        }
                                    })
                                    .style(move |s| {
                                        s.color(Color::from_rgb8(239, 68, 68))
                                            .border(1.0)
                                            .border_color(Color::from_rgb8(239, 68, 68))
                                            .padding_horiz(8.0)
                                            .padding_vert(2.0)
                                            .border_radius(4.0)
                                            .font_size(11.0)
                                            .cursor(CursorStyle::Pointer)
                                            .display(if is_owner { Display::None } else { Display::Flex })
                                    }),
                            ))
                            .style(move |s| {
                                s.items_center()
                                    .padding_vert(6.0)
                                    .border_bottom(1.0)
                                    .border_color(Color::from_rgba8(255, 255, 255, 20))
                                    .width_pct(100.0)
                            })
                        }
                    },
                )
                .style(|s| s.flex_col().margin_bottom(16.0)),
            ))
            .style(|s| s.flex_col()),
            stack((
                // Section 2: Add Person to Workspace
                label(|| "Add Person to Workspace".to_string()).style(move |s| {
                    s.font_bold()
                        .margin_bottom(6.0)
                        .color(config.get().color(LapceColor::EDITOR_FOREGROUND))
                }),
                text_input(ahead_state.new_collaborator_input)
                    .placeholder("GitHub username only (e.g. octocat)")
                    .style(move |s| {
                        s.width(420.0)
                            .padding(6.0)
                            .border(1.0)
                            .border_color(Color::from_rgb8(120, 120, 120))
                            .border_radius(4.0)
                            .margin_bottom(10.0)
                            .background(config.get().color(LapceColor::EDITOR_BACKGROUND))
                            .color(config.get().color(LapceColor::EDITOR_FOREGROUND))
                    }),
                label(|| "Permissions Role:".to_string()).style(move |s| {
                    s.font_size(12.0)
                        .margin_bottom(6.0)
                        .color(config.get().color(LapceColor::EDITOR_DIM))
                }),
                stack((
                    role_picker_pill(ahead_state, SessionRole::Editor, "Editor"),
                    role_picker_pill(ahead_state, SessionRole::Reviewer, "Reviewer"),
                    role_picker_pill(ahead_state, SessionRole::Viewer, "Viewer"),
                ))
                .style(|s| s.margin_bottom(8.0)),
                label(move || ahead_state.collab_error.get().unwrap_or_default())
                    .style(move |s| {
                        let has_err = ahead_state.collab_error.get().is_some();
                        s.font_size(12.0)
                            .margin_bottom(if has_err { 8.0 } else { 0.0 })
                            .color(Color::from_rgb8(248, 113, 113))
                            .display(if has_err { Display::Flex } else { Display::None })
                    }),
                // Action Buttons
                stack((
                    label(|| "+ Add to Workspace".to_string())
                        .on_click_stop({
                            let proxy = proxy.clone();
                            move |_| {
                                let handle = ahead_state.new_collaborator_input.get_untracked().trim().to_string();
                                if handle.is_empty() {
                                    ahead_state.collab_error.set(Some("Enter a GitHub username first.".to_string()));
                                    return;
                                }
                                ahead_state.collab_error.set(None);
                                let role = ahead_state.new_collaborator_role.get_untracked();
                                let send = create_ext_action(scope, move |res: Result<serde_json::Value, lapce_rpc::RpcError>| {
                                    match res {
                                        Ok(val) => match serde_json::from_value::<Vec<SessionParticipantRecord>>(val) {
                                            Ok(parts) => {
                                                ahead_state.workspace_participants.set(parts);
                                                ahead_state.new_collaborator_input.set(String::new());
                                            }
                                            Err(_) => ahead_state.collab_error.set(Some("Add returned an unreadable member list.".to_string())),
                                        },
                                        Err(e) => ahead_state.collab_error.set(Some(format!("Add failed: {}", e.message))),
                                    }
                                });
                                proxy.ahead_request(
                                    lapce_rpc::ahead::AheadRequest::AddWorkspaceParticipant {
                                        user_handle: handle,
                                        role,
                                    },
                                    move |res| send(res),
                                );
                            }
                        })
                        .style(move |s| {
                            s.background(Color::from_rgb8(37, 99, 235))
                                .color(Color::from_rgb8(255, 255, 255))
                                .padding_horiz(14.0)
                                .padding_vert(6.0)
                                .border_radius(4.0)
                                .cursor(CursorStyle::Pointer)
                                .margin_right(8.0)
                                .font_bold()
                        }),
                    label(|| "Close".to_string())
                        .on_click_stop(move |_| {
                            ahead_state.show_collab_modal.set(false);
                            ahead_state.collab_error.set(None);
                        })
                        .style(move |s| {
                            s.background(Color::from_rgb8(75, 85, 99))
                                .color(Color::from_rgb8(255, 255, 255))
                                .padding_horiz(14.0)
                                .padding_vert(6.0)
                                .border_radius(4.0)
                                .cursor(CursorStyle::Pointer)
                        }),
                )),
            ))
            .style(|s| s.flex_col()),
        ))
        .style(move |s| {
            s.flex_col()
                .padding(24.0)
                .background(config.get().color(LapceColor::PANEL_BACKGROUND))
                .border(1.0)
                .border_color(Color::from_rgb8(80, 80, 80))
                .border_radius(8.0)
                .min_width(460.0)
        }),
    )
    .style(move |s| {
        let is_open = ahead_state.show_collab_modal.get();
        s.position(Position::Absolute)
            .size_pct(100.0, 100.0)
            .justify_content(Some(JustifyContent::Center))
            .items_center()
            .background(Color::from_rgba8(0, 0, 0, 180))
            .display(if is_open { Display::Flex } else { Display::None })
            .z_index(100)
    })
}

fn role_picker_pill(
    ahead_state: AheadState,
    role: SessionRole,
    label_text: &'static str,
) -> impl View {
    label(move || label_text.to_string())
        .on_click_stop(move |_| {
            ahead_state.new_collaborator_role.set(role);
        })
        .style(move |s| {
            let is_selected = ahead_state.new_collaborator_role.get() == role;
            let bg = if is_selected {
                Color::from_rgb8(37, 99, 235)
            } else {
                Color::from_rgb8(45, 55, 72)
            };
            s.background(bg)
                .color(Color::from_rgb8(255, 255, 255))
                .padding_horiz(8.0)
                .padding_vert(4.0)
                .border_radius(4.0)
                .margin_right(6.0)
                .cursor(CursorStyle::Pointer)
                .font_size(12.0)
        })
}

/// AHEAD AI Agent Sidebar / Sidecar Panel View
pub fn ahead_agent_panel(
    window_tab_data: Rc<WindowTabData>,
    _position: PanelPosition,
) -> impl View {
    let ahead_state = window_tab_data.ahead;
    let config = window_tab_data.common.config;
    let scope = window_tab_data.scope;
    let proxy = window_tab_data.common.proxy.clone();
    let session_signal = ahead_state.active_session;

    container(
        scroll(
            stack((
                // 1. Panel Header & Assistance Mode Selector
                container(
                    stack((
                        stack((
                            label(|| "⚡ AHEAD AI PAIRING AGENT".to_string()).style(move |s| {
                                s.font_bold()
                                    .font_size(13.0)
                                    .color(Color::from_rgb8(245, 158, 11))
                            }),
                            label(move || {
                                if let Some(view) = session_signal.get() {
                                    format!("{} · {}", view.session.work_kind.display_name(), view.session.title)
                                } else {
                                    "No Active Session".to_string()
                                }
                            })
                            .style(move |s| {
                                s.font_size(11.0)
                                    .color(Color::from_rgb8(156, 163, 175))
                                    .margin_top(2.0)
                            }),
                        ))
                        .style(|s| s.flex_col().flex_grow(1.0)),

                        // Mode control: explicit host-backed switch, never a silent toggle.
                        label(move || {
                            if let Some(view) = session_signal.get() {
                                match view.session.mode {
                                    AssistanceMode::Learn => "🟢 Learn (Socratic) — switch to Assist",
                                    AssistanceMode::Assist => "🔵 Assist (Scaffold) — switch to Learn",
                                }
                            } else {
                                "Start Session"
                            }
                        })
                        .on_click_stop({
                            let ahead_state = ahead_state;
                            let proxy = proxy.clone();
                            move |_| {
                            let current = ahead_state.active_session.get_untracked();
                            if let Some(view) = current {
                                let next = match view.session.mode {
                                    AssistanceMode::Learn => AssistanceMode::Assist,
                                    AssistanceMode::Assist => AssistanceMode::Learn,
                                };
                                let session_id = view.session.id.clone();
                                ahead_state.session_busy.set(true);
                                ahead_state.session_error.set(None);
                                let send = create_ext_action(scope, move |res: Result<serde_json::Value, lapce_rpc::RpcError>| {
                                    match res {
                                        Ok(val) => match serde_json::from_value::<SessionView>(val) {
                                            Ok(updated) => ahead_state.apply_remote_mode(updated),
                                            Err(_) => ahead_state.session_error.set(Some("Mode change returned an unreadable session.".to_string())),
                                        },
                                        Err(e) => ahead_state.session_error.set(Some(format!("Mode change failed: {}", e.message))),
                                    }
                                    ahead_state.session_busy.set(false);
                                });
                                proxy.ahead_request(
                                    lapce_rpc::ahead::AheadRequest::SetMode { session_id, mode: next },
                                    move |res| send(res),
                                );
                            } else {
                                ahead_state.show_start_work_modal.set(true);
                            }
                            }
                        })
                        .style(move |s| {
                            let mode = session_signal.get().map(|v| v.session.mode).unwrap_or(AssistanceMode::Learn);
                            let bg = match mode {
                                AssistanceMode::Learn => Color::from_rgb8(22, 101, 52),
                                AssistanceMode::Assist => Color::from_rgb8(30, 64, 175),
                            };
                            s.background(bg)
                                .color(Color::from_rgb8(255, 255, 255))
                                .padding_horiz(10.0)
                                .padding_vert(5.0)
                                .border_radius(6.0)
                                .cursor(CursorStyle::Pointer)
                                .font_bold()
                                .font_size(11.0)
                        }),
                    ))
                    .style(|s| s.items_center().width_pct(100.0)),
                )
                .style(move |s| {
                    s.padding(12.0)
                        .background(Color::from_rgb8(15, 23, 42))
                        .border_bottom(1.0)
                        .border_color(Color::from_rgb8(30, 41, 59))
                }),

                // 2. Full-Duplex Voice Chat Hub & Barge-In Preemption
                container(
                    stack((
                        stack((
                            label(|| "🎙️ REAL-TIME VOICE CHAT".to_string()).style(|s| {
                                s.font_bold()
                                    .font_size(11.0)
                                    .color(Color::from_rgb8(148, 163, 184))
                            }),
                            label(move || {
                                if ahead_state.voice_active.get() {
                                    if ahead_state.voice_speaking.get() {
                                        "🔊 Voice: Agent Speaking".to_string()
                                    } else {
                                        "🎙️ Voice: Live (Listening...)".to_string()
                                    }
                                } else {
                                    "🎙️ Connect Voice (Hands-Free)".to_string()
                                }
                            })
                            .on_click_stop({
                                let ahead_state = ahead_state;
                                move |_| {
                                    ahead_state.toggle_voice();
                                }
                            })
                            .style(move |s| {
                                let is_active = ahead_state.voice_active.get();
                                let is_speaking = ahead_state.voice_speaking.get();
                                let bg = if is_active {
                                    if is_speaking {
                                        Color::from_rgb8(2, 132, 199)
                                    } else {
                                        Color::from_rgb8(16, 185, 129)
                                    }
                                } else {
                                    Color::from_rgb8(51, 65, 85)
                                };
                                s.background(bg)
                                    .color(Color::from_rgb8(255, 255, 255))
                                    .padding_horiz(12.0)
                                    .padding_vert(6.0)
                                    .border_radius(6.0)
                                    .margin_top(6.0)
                                    .cursor(CursorStyle::Pointer)
                                    .font_bold()
                                    .font_size(12.0)
                            }),
                        ))
                        .style(|s| s.flex_col()),

                        // Voice Controls: Barge-In Interrupt (<50ms) and Mic Mute
                        stack((
                            label(|| "⏹️ Barge In (Interrupt Audio)".to_string())
                                .on_click_stop({
                                    let ahead_state = ahead_state;
                                    let proxy = proxy.clone();
                                    move |_| {
                                        ahead_state.interrupt_voice_playback();
                                        proxy.ahead_request(
                                            lapce_rpc::ahead::AheadRequest::VoiceControl {
                                                control: lapce_rpc::ahead::VoiceControl::InterruptPlayback {
                                                    generation: ahead_state.voice_generation.get(),
                                                },
                                            },
                                            |_| {},
                                        );
                                    }
                                })
                                .style(move |s| {
                                    let is_active = ahead_state.voice_active.get();
                                    s.background(Color::from_rgb8(185, 28, 28))
                                        .color(Color::from_rgb8(255, 255, 255))
                                        .padding_horiz(8.0)
                                        .padding_vert(4.0)
                                        .border_radius(4.0)
                                        .margin_right(6.0)
                                        .cursor(CursorStyle::Pointer)
                                        .font_size(11.0)
                                        .display(if is_active { Display::Flex } else { Display::None })
                                }),
                            label(move || {
                                if ahead_state.voice_mic_muted.get() {
                                    "🔇 Unmute Mic".to_string()
                                } else {
                                    "🎤 Mute Mic".to_string()
                                }
                            })
                            .on_click_stop({
                                let ahead_state = ahead_state;
                                let proxy = proxy.clone();
                                move |_| {
                                    let muted = !ahead_state.voice_mic_muted.get();
                                    ahead_state.voice_mic_muted.set(muted);
                                    proxy.ahead_request(
                                        lapce_rpc::ahead::AheadRequest::VoiceControl {
                                            control: lapce_rpc::ahead::VoiceControl::SetMicMuted { muted },
                                        },
                                        |_| {},
                                    );
                                }
                            })
                            .style(move |s| {
                                let is_active = ahead_state.voice_active.get();
                                s.background(Color::from_rgb8(71, 85, 105))
                                    .color(Color::from_rgb8(255, 255, 255))
                                    .padding_horiz(8.0)
                                    .padding_vert(4.0)
                                    .border_radius(4.0)
                                    .cursor(CursorStyle::Pointer)
                                    .font_size(11.0)
                                    .display(if is_active { Display::Flex } else { Display::None })
                            }),
                        ))
                        .style(|s| s.margin_top(8.0)),

                        // Live Voice Transcript box
                        container(
                            stack((
                                label(|| "💬 Live Speech Feed:".to_string()).style(|s| {
                                    s.font_size(10.0)
                                        .font_bold()
                                        .color(Color::from_rgb8(100, 116, 139))
                                        .margin_bottom(4.0)
                                }),
                                dyn_stack(
                                    move || ahead_state.voice_transcripts.get(),
                                    |t: &lapce_rpc::ahead::VoiceTranscriptUpdate| format!("{}-{}", t.epoch, t.text),
                                    |t: lapce_rpc::ahead::VoiceTranscriptUpdate| {
                                        let is_agent = t.speaker_id == "agent";
                                        let is_system = t.speaker_id == "system";
                                        let text = t.text;
                                        stack((
                                            label(move || {
                                                if is_system {
                                                    "⚙️ System".to_string()
                                                } else if is_agent {
                                                    "⚡ Ahead".to_string()
                                                } else {
                                                    "👤 You".to_string()
                                                }
                                            })
                                            .style(move |s| {
                                                s.font_bold()
                                                    .font_size(10.0)
                                                    .color(if is_agent {
                                                        Color::from_rgb8(56, 189, 248)
                                                    } else {
                                                        Color::from_rgb8(148, 163, 184)
                                                    })
                                                    .margin_right(6.0)
                                            }),
                                            label(move || text.clone()).style(|s| {
                                                s.font_size(11.0)
                                                    .color(Color::from_rgb8(226, 232, 240))
                                            }),
                                        ))
                                        .style(|s| s.margin_vert(2.0))
                                    },
                                ),
                            ))
                            .style(|s| s.flex_col()),
                        )
                        .style(move |s| {
                            s.margin_top(8.0)
                                .padding(8.0)
                                .background(Color::from_rgb8(15, 23, 42))
                                .border(1.0)
                                .border_color(Color::from_rgb8(30, 41, 59))
                                .border_radius(6.0)
                        }),
                    ))
                    .style(|s| s.flex_col()),
                )
                .style(move |s| {
                    s.padding(12.0)
                        .background(Color::from_rgb8(17, 24, 39))
                        .border_bottom(1.0)
                        .border_color(Color::from_rgb8(31, 41, 55))
                }),

                // 3. Workflow Phase Progression Pipeline (The Process Outlined)
                container(
                    stack((
                        stack((
                            label(|| "🔄 WORKFLOW PIPELINE".to_string()).style(|s| {
                                s.font_bold()
                                    .font_size(11.0)
                                    .color(Color::from_rgb8(148, 163, 184))
                            }),
                            label(move || {
                                if let Some(view) = session_signal.get() {
                                    format!("Current Phase: {} (Visit {})", view.workflow.phase.title, view.workflow.phase.visit)
                                } else {
                                    "No Active Phase".to_string()
                                }
                            })
                            .style(|s| {
                                s.font_bold()
                                    .font_size(12.0)
                                    .color(Color::from_rgb8(245, 158, 11))
                                    .margin_top(2.0)
                            }),
                        ))
                        .style(|s| s.flex_col()),

                        // 5-Step Process Stepper Visualizer
                        stack((
                            phase_step_chip("1. Plan", "plan", ahead_state),
                            phase_step_chip("2. Invariants", "invariants", ahead_state),
                            phase_step_chip("3. Implement", "implement", ahead_state),
                            phase_step_chip("4. Verify", "verify", ahead_state),
                            phase_step_chip("5. Review", "review", ahead_state),
                        ))
                        .style(|s| s.margin_top(8.0)),

                        // Phase Invariants & Objectives Summary Card
                        container(
                            label(move || {
                                let phase_id = session_signal
                                    .get()
                                    .map(|v| v.workflow.phase.id)
                                    .unwrap_or_else(|| "plan".to_string());
                                match phase_id.as_str() {
                                    "plan" | "hypothesize" => {
                                        "📌 Phase Goal: Define scope, map affected crates, and author problem framing."
                                    }
                                    "invariants" => {
                                        "🛡️ Phase Goal: Document state preconditions, postconditions, and idempotency guarantees."
                                    }
                                    "implement" => {
                                        "⚡ Phase Goal: Author minimal, strictly-bounded code changes adhering to invariants."
                                    }
                                    "verify" => {
                                        "🧪 Phase Goal: Run cargo test --workspace, inspect edge cases, and verify zero regressions."
                                    }
                                    "review" => {
                                        "📋 Phase Goal: Peer inspection, human verification signoff, and tracker outbox sync."
                                    }
                                    _ => "🚀 Milestone Complete. Session ready for archive or new iteration.",
                                }
                                .to_string()
                            })
                            .style(|s| {
                                s.font_size(11.0)
                                    .color(Color::from_rgb8(203, 213, 225))
                            }),
                        )
                        .style(|s| {
                            s.margin_top(8.0)
                                .padding(8.0)
                                .background(Color::from_rgb8(30, 41, 59))
                                .border_radius(6.0)
                        }),

                        // Advance Phase Gate Button (host is authority; no local pre-advance)
                        stack((
                            label(move || {
                                if ahead_state.session_busy.get() {
                                    "Working…".to_string()
                                } else {
                                    "Advance Phase ➔".to_string()
                                }
                            })
                                .on_click_stop({
                                    let ahead_state = ahead_state;
                                    let proxy = proxy.clone();
                                    move |_| {
                                        if ahead_state.session_busy.get() {
                                            return;
                                        }
                                        if let Some(view) = ahead_state.active_session.get_untracked() {
                                            let session_id = view.session.id.clone();
                                            let expected_revision = view.workflow.revision;
                                            let (next_id, next_title) = AheadState::next_phase(&view.workflow.phase.id);
                                            ahead_state.session_busy.set(true);
                                            ahead_state.session_error.set(None);
                                            let send = create_ext_action(scope, move |res: Result<serde_json::Value, lapce_rpc::RpcError>| {
                                                match res {
                                                    Ok(val) => match serde_json::from_value::<WorkflowState>(val) {
                                                        Ok(next_wf) => {
                                                            let title = next_wf.phase.title.clone();
                                                            ahead_state.active_session.update(|opt| {
                                                                if let Some(v) = opt {
                                                                    v.workflow = next_wf;
                                                                    v.session.revision += 1;
                                                                }
                                                            });
                                                            ahead_state.session_error.set(None);
                                                            ahead_state.push_message("AHEAD Agent", format!("✅ Advanced to {}. Invariants check active.", title), false);
                                                        }
                                                        Err(_) => ahead_state.session_error.set(Some("Advance returned an unreadable workflow.".to_string())),
                                                    },
                                                    Err(e) => ahead_state.session_error.set(Some(format!("Advance failed: {}", e.message))),
                                                }
                                                ahead_state.session_busy.set(false);
                                            });
                                            proxy.ahead_request(
                                                lapce_rpc::ahead::AheadRequest::AdvancePhase {
                                                    session_id,
                                                    expected_revision,
                                                    target_phase_id: next_id.to_string(),
                                                },
                                                move |res| send(res),
                                            );
                                            let _ = next_title;
                                        }
                                    }
                                })
                                .style(move |s| {
                                    s.background(Color::from_rgb8(13, 148, 136))
                                        .color(Color::from_rgb8(255, 255, 255))
                                        .padding_horiz(14.0)
                                        .padding_vert(6.0)
                                        .border_radius(6.0)
                                        .cursor(CursorStyle::Pointer)
                                        .font_bold()
                                        .font_size(12.0)
                                }),
                            label(|| "🔒 Human Gatekeeper Enforced".to_string()).style(|s| {
                                s.font_size(10.0)
                                    .color(Color::from_rgb8(148, 163, 184))
                                    .margin_left(10.0)
                            }),
                        ))
                        .style(|s| s.margin_top(10.0).items_center()),
                    ))
                    .style(|s| s.flex_col()),
                )
                .style(move |s| {
                    s.padding(12.0)
                        .background(Color::from_rgb8(15, 23, 42))
                        .border_bottom(1.0)
                        .border_color(Color::from_rgb8(30, 41, 59))
                }),

                // 4. Agent Pairing Feed & Socratic / Scaffolding Stream
                container(
                    stack((
                        label(|| "💬 AGENT PAIRING FEED".to_string()).style(|s| {
                            s.font_bold()
                                .font_size(11.0)
                                .color(Color::from_rgb8(148, 163, 184))
                                .margin_bottom(8.0)
                        }),
                        // Chat turns
                        dyn_stack(
                            move || ahead_state.chat_messages.get(),
                            |m: &crate::ahead::state::AgentChatMessage| m.id.clone(),
                            |m: crate::ahead::state::AgentChatMessage| {
                                let is_challenge = m.is_challenge;
                                let is_human = m.sender == "Human";
                                container(
                                    stack((
                                        stack((
                                            label(move || m.sender.clone()).style(move |s| {
                                                s.font_bold()
                                                    .font_size(11.0)
                                                    .color(if is_human {
                                                        Color::from_rgb8(96, 165, 250)
                                                    } else if is_challenge {
                                                        Color::from_rgb8(251, 191, 36)
                                                    } else {
                                                        Color::from_rgb8(52, 211, 153)
                                                    })
                                            }),
                                            label(move || m.timestamp.clone()).style(|s| {
                                                s.font_size(9.0)
                                                    .color(Color::from_rgb8(100, 116, 139))
                                                    .margin_left(6.0)
                                            }),
                                        ))
                                        .style(|s| s.items_center()),
                                        label(move || m.text.clone()).style(move |s| {
                                            s.font_size(12.0)
                                                .color(Color::from_rgb8(241, 245, 249))
                                                .margin_top(3.0)
                                        }),
                                    ))
                                    .style(|s| s.flex_col()),
                                )
                                .style(move |s| {
                                    s.padding(8.0)
                                        .margin_vert(3.0)
                                        .border_radius(6.0)
                                        .background(if is_human {
                                            Color::from_rgb8(30, 41, 59)
                                        } else if is_challenge {
                                            Color::from_rgb8(41, 37, 36)
                                        } else {
                                            Color::from_rgb8(15, 23, 42)
                                        })
                                        .border(1.0)
                                        .border_color(if is_challenge {
                                            Color::from_rgb8(217, 119, 6)
                                        } else {
                                            Color::from_rgb8(51, 65, 85)
                                        })
                                })
                            },
                        ),

                        // Code Proposals & Human Authorization Gate (in Assist mode)
                        dyn_stack(
                            move || ahead_state.pending_proposals.get(),
                            |p: &lapce_rpc::ahead::ChangeProposal| p.id.clone(),
                            move |p: lapce_rpc::ahead::ChangeProposal| {
                                let p_id = p.id.clone();
                                let ahead_state = ahead_state;
                                container(
                                    stack((
                                        label(|| "📄 MECHANICAL CODE PROPOSAL".to_string()).style(|s| {
                                            s.font_bold()
                                                .font_size(10.0)
                                                .color(Color::from_rgb8(96, 165, 250))
                                        }),
                                        label(move || format!("File: {}", p.path)).style(|s| {
                                            s.font_bold()
                                                .font_size(11.0)
                                                .color(Color::from_rgb8(226, 232, 240))
                                                .margin_top(2.0)
                                        }),
                                        label(move || p.description.clone()).style(|s| {
                                            s.font_size(11.0)
                                                .color(Color::from_rgb8(148, 163, 184))
                                                .margin_top(2.0)
                                        }),
                                        // Monospace Diff Preview
                                        container(
                                            label(move || p.patch.clone()).style(|s| {
                                                s.font_size(11.0)
                                                    .color(Color::from_rgb8(134, 239, 172))
                                            }),
                                        )
                                        .style(|s| {
                                            s.margin_top(6.0)
                                                .padding(6.0)
                                                .background(Color::from_rgb8(10, 15, 26))
                                                .border_radius(4.0)
                                        }),
                                        // Checkpoint Approval / Rejection Gate (host-backed)
                                        stack((
                                            label(|| "✅ Authorize & Apply".to_string())
                                                .on_click_stop({
                                                    let p_id = p_id.clone();
                                                    let ahead_state = ahead_state;
                                                    let proxy = proxy.clone();
                                                    let session_id = p.session_id.clone();
                                                    move |_| {
                                                        let pid = p_id.clone();
                                                        let send = create_ext_action(scope, move |res: Result<serde_json::Value, lapce_rpc::RpcError>| {
                                                            match res {
                                                                Ok(_) => {
                                                                    ahead_state.drop_proposal(&pid);
                                                                    ahead_state.push_message("AHEAD Agent", format!("✅ Proposal {} authorized and applied by human engineer.", pid), false);
                                                                }
                                                                Err(e) => ahead_state.session_error.set(Some(format!("Accept failed: {}", e.message))),
                                                            }
                                                        });
                                                        proxy.ahead_request(
                                                            lapce_rpc::ahead::AheadRequest::AcceptProposal { session_id: session_id.clone(), proposal_id: p_id.clone() },
                                                            move |res| send(res),
                                                        );
                                                    }
                                                })
                                                .style(|s| {
                                                    s.background(Color::from_rgb8(22, 163, 74))
                                                        .color(Color::from_rgb8(255, 255, 255))
                                                        .padding_horiz(10.0)
                                                        .padding_vert(5.0)
                                                        .border_radius(4.0)
                                                        .cursor(CursorStyle::Pointer)
                                                        .font_bold()
                                                        .font_size(11.0)
                                                        .margin_right(6.0)
                                                }),
                                            label(|| "❌ Reject".to_string())
                                                .on_click_stop({
                                                    let p_id = p_id.clone();
                                                    move |_| {
                                                        ahead_state.drop_proposal(&p_id);
                                                        ahead_state.push_message("AHEAD Agent", format!("❌ Proposal {} rejected.", p_id), false);
                                                    }
                                                })
                                                .style(|s| {
                                                    s.background(Color::from_rgb8(220, 38, 38))
                                                        .color(Color::from_rgb8(255, 255, 255))
                                                        .padding_horiz(10.0)
                                                        .padding_vert(5.0)
                                                        .border_radius(4.0)
                                                        .cursor(CursorStyle::Pointer)
                                                        .font_bold()
                                                        .font_size(11.0)
                                                }),
                                        ))
                                        .style(|s| s.margin_top(8.0)),
                                    ))
                                    .style(|s| s.flex_col()),
                                )
                                .style(|s| {
                                    s.margin_top(8.0)
                                        .padding(10.0)
                                        .background(Color::from_rgb8(23, 37, 84))
                                        .border(1.0)
                                        .border_color(Color::from_rgb8(59, 130, 246))
                                        .border_radius(6.0)
                                })
                            },
                        ),

                        // Quick prompt shortcuts
                        stack((
                            quick_action_chip("🛡️ Edge Cases", ahead_state, "Challenge my edge cases for this phase"),
                            quick_action_chip("📋 Invariants", ahead_state, "What invariants must hold here?"),
                            quick_action_chip("⚡ Scaffold", ahead_state, "Scaffold boilerplate for the active task"),
                        ))
                        .style(|s| s.margin_top(8.0)),

                        // User Chat Input Box
                        stack((
                            text_input(ahead_state.chat_input)
                                .style(move |s| {
                                    s.flex_grow(1.0)
                                        .padding_horiz(8.0)
                                        .padding_vert(6.0)
                                        .background(Color::from_rgb8(15, 23, 42))
                                        .color(Color::from_rgb8(255, 255, 255))
                                        .border(1.0)
                                        .border_color(Color::from_rgb8(51, 65, 85))
                                        .border_radius(4.0)
                                        .font_size(12.0)
                                }),
                            label(|| "Send".to_string())
                                .on_click_stop(move |_| {
                                    let text = ahead_state.chat_input.get();
                                    ahead_state.send_chat(text);
                                })
                                .style(|s| {
                                    s.background(Color::from_rgb8(37, 99, 235))
                                        .color(Color::from_rgb8(255, 255, 255))
                                        .padding_horiz(12.0)
                                        .padding_vert(6.0)
                                        .border_radius(4.0)
                                        .margin_left(6.0)
                                        .cursor(CursorStyle::Pointer)
                                        .font_bold()
                                        .font_size(11.0)
                                }),
                        ))
                        .style(|s| s.margin_top(8.0).items_center()),
                    ))
                    .style(|s| s.flex_col()),
                )
                .style(move |s| {
                    s.padding(12.0)
                        .background(Color::from_rgb8(17, 24, 39))
                        .border_bottom(1.0)
                        .border_color(Color::from_rgb8(31, 41, 55))
                }),

                // 5. Tracker Outbox & GitHub Synchronization Section
                container(
                    stack((
                        label(|| "📦 TRACKER OUTBOX (GITHUB ISSUE SYNC)".to_string()).style(|s| {
                            s.font_bold()
                                .font_size(11.0)
                                .color(Color::from_rgb8(148, 163, 184))
                                .margin_bottom(6.0)
                        }),
                        dyn_stack(
                            move || ahead_state.tracker_outbox.get(),
                            |item: &crate::ahead::state::TrackerOutboxEntry| item.id.clone(),
                            move |item: crate::ahead::state::TrackerOutboxEntry| {
                                let item_id = item.id.clone();
                                let ahead_state = ahead_state;
                                container(
                                    stack((
                                        stack((
                                            label(move || item.target.clone()).style(|s| {
                                                s.font_bold()
                                                    .font_size(11.0)
                                                    .color(Color::from_rgb8(56, 189, 248))
                                            }),
                                            label(move || item.status.clone()).style(|s| {
                                                s.font_size(10.0)
                                                    .color(Color::from_rgb8(245, 158, 11))
                                                    .margin_left(6.0)
                                            }),
                                        ))
                                        .style(|s| s.items_center()),
                                        label(move || item.description.clone()).style(|s| {
                                            s.font_size(11.0)
                                                .color(Color::from_rgb8(203, 213, 225))
                                                .margin_top(2.0)
                                        }),
                                        label(|| "🚀 Authorize Dispatch to GitHub".to_string())
                                            .on_click_stop({
                                                let item_id = item_id.clone();
                                                move |_| {
                                                    ahead_state.dispatch_tracker_item(&item_id);
                                                }
                                            })
                                            .style(|s| {
                                                s.margin_top(6.0)
                                                    .background(Color::from_rgb8(14, 165, 233))
                                                    .color(Color::from_rgb8(255, 255, 255))
                                                    .padding_horiz(10.0)
                                                    .padding_vert(4.0)
                                                    .border_radius(4.0)
                                                    .cursor(CursorStyle::Pointer)
                                                    .font_bold()
                                                    .font_size(11.0)
                                            }),
                                    ))
                                    .style(|s| s.flex_col()),
                                )
                                .style(|s| {
                                    s.padding(8.0)
                                        .margin_vert(4.0)
                                        .background(Color::from_rgb8(15, 23, 42))
                                        .border(1.0)
                                        .border_color(Color::from_rgb8(30, 41, 59))
                                        .border_radius(6.0)
                                })
                            },
                        ),
                    ))
                    .style(|s| s.flex_col()),
                )
                .style(move |s| {
                    s.padding(12.0)
                        .background(Color::from_rgb8(15, 23, 42))
                }),
            ))
            .style(|s| s.flex_col().width_pct(100.0)),
        )
        .style(|s| s.size_pct(100.0, 100.0)),
    )
    .style(move |s| {
        s.size_pct(100.0, 100.0)
            .background(config.get().color(LapceColor::PANEL_BACKGROUND))
    })
}

fn phase_step_chip(
    label_text: &'static str,
    phase_id: &'static str,
    ahead_state: AheadState,
) -> impl View {
    label(move || label_text.to_string())
        .style(move |s| {
            let current_id = ahead_state
                .active_session
                .get()
                .map(|v| v.workflow.phase.id)
                .unwrap_or_default();
            let is_active = current_id == phase_id;
            let bg = if is_active {
                Color::from_rgb8(245, 158, 11)
            } else {
                Color::from_rgb8(30, 41, 59)
            };
            let fg = if is_active {
                Color::from_rgb8(0, 0, 0)
            } else {
                Color::from_rgb8(203, 213, 225)
            };
            s.background(bg)
                .color(fg)
                .padding_horiz(8.0)
                .padding_vert(4.0)
                .border_radius(4.0)
                .margin_right(4.0)
                .margin_bottom(4.0)
                .font_bold()
                .font_size(11.0)
        })
}

fn quick_action_chip(
    label_text: &'static str,
    ahead_state: AheadState,
    prompt: &'static str,
) -> impl View {
    label(move || label_text.to_string())
        .on_click_stop(move |_| {
            ahead_state.send_chat(prompt.to_string());
        })
        .style(move |s| {
            s.background(Color::from_rgb8(30, 41, 59))
                .color(Color::from_rgb8(148, 163, 184))
                .border(1.0)
                .border_color(Color::from_rgb8(51, 65, 85))
                .padding_horiz(8.0)
                .padding_vert(3.0)
                .border_radius(4.0)
                .margin_right(4.0)
                .margin_bottom(4.0)
                .cursor(CursorStyle::Pointer)
                .font_size(10.0)
        })
}

