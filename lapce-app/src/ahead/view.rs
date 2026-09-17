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
    AssistanceMode, SessionParticipantRecord, SessionRole, WorkKind,
};

use crate::{
    ahead::state::AheadState,
    config::{color::LapceColor, LapceConfig},
    panel::{kind::PanelKind, position::PanelPosition},
    window_tab::WindowTabData,
};

/// Status bar indicator for active AHEAD session
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
        // Workspace Team / Collaboration affordance
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
                .color(Color::from_rgb8(56, 189, 248))
                .border(1.0)
                .border_color(Color::from_rgb8(56, 189, 248))
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

/// GitHub Account Flow Modal
pub fn github_auth_modal(
    window_tab_data: Rc<WindowTabData>,
    config: ReadSignal<Arc<LapceConfig>>,
) -> impl View {
    let ahead_state = window_tab_data.ahead;
    let scope = window_tab_data.scope;
    let proxy = window_tab_data.common.proxy.clone();

    container(
        stack((
            label(|| "GitHub Authentication".to_string()).style(move |s| {
                s.font_size(18.0)
                    .font_bold()
                    .margin_bottom(12.0)
                    .color(config.get().color(LapceColor::EDITOR_FOREGROUND))
            }),
            // User status info
            label(move || {
                if let Some(user) = ahead_state.authenticated_user.get() {
                    let auth_str = if user.is_authenticated {
                        "✓ Authenticated with GitHub OAuth"
                    } else {
                        "Local Git Identity (unauthenticated)"
                    };
                    format!(
                        "Signed in as @{} ({})\nEmail: {}\nStatus: {}",
                        user.login,
                        user.name.unwrap_or_default(),
                        user.email.unwrap_or_else(|| "none".into()),
                        auth_str
                    )
                } else if let Some(code) = ahead_state.pending_device_code.get() {
                    format!("Enter code at {}:\n\n{}", code.verification_uri, code.user_code)
                } else {
                    "Not connected to GitHub OAuth. Select an option below:".to_string()
                }
            })
            .style(move |s| {
                s.font_size(13.0)
                    .margin_bottom(16.0)
                    .color(config.get().color(LapceColor::EDITOR_FOREGROUND))
            }),
            // Token input if toggled
            stack((
                text_input(ahead_state.token_input)
                    .placeholder("GitHub Personal Access Token (ghp_... or gho_...)")
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
            // Action Buttons
            stack((
                // When authenticated: Sign Out
                label(|| "Sign Out".to_string())
                    .on_click_stop({
                        let proxy = proxy.clone();
                        move |_| {
                            let send = create_ext_action(scope, move |res: Result<serde_json::Value, lapce_rpc::RpcError>| {
                                if let Ok(val) = res {
                                    if let Ok(user) = serde_json::from_value::<lapce_rpc::ahead::GitHubUser>(val) {
                                        ahead_state.authenticated_user.set(Some(user));
                                    }
                                }
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
                label(|| "Connect with GitHub CLI (gh)".to_string())
                    .on_click_stop({
                        let proxy = proxy.clone();
                        move |_| {
                            let send = create_ext_action(scope, move |res: Result<serde_json::Value, lapce_rpc::RpcError>| {
                                if let Ok(val) = res {
                                    if let Ok(user) = serde_json::from_value::<lapce_rpc::ahead::GitHubUser>(val) {
                                        ahead_state.authenticated_user.set(Some(user));
                                        ahead_state.show_auth_modal.set(false);
                                    }
                                }
                            });
                            proxy.ahead_request(
                                lapce_rpc::ahead::AheadRequest::GitHubAuthDetectCli,
                                move |res| send(res),
                            );
                        }
                    })
                    .style(move |s| {
                        let is_auth = ahead_state.authenticated_user.get().map(|u| u.is_authenticated).unwrap_or(false);
                        s.background(Color::from_rgb8(37, 99, 235))
                            .color(Color::from_rgb8(255, 255, 255))
                            .padding_horiz(14.0)
                            .padding_vert(6.0)
                            .border_radius(4.0)
                            .cursor(CursorStyle::Pointer)
                            .margin_right(8.0)
                            .display(if !is_auth && !ahead_state.show_token_input.get() { Display::Flex } else { Display::None })
                    }),
                // When NOT authenticated: Enter Token toggle / Submit
                label(move || {
                    if ahead_state.show_token_input.get() {
                        "Submit Token".to_string()
                    } else {
                        "Sign in with Token".to_string()
                    }
                })
                .on_click_stop({
                    let proxy = proxy.clone();
                    move |_| {
                        if ahead_state.show_token_input.get() {
                            let token = ahead_state.token_input.get_untracked().trim().to_string();
                            if !token.is_empty() {
                                let send = create_ext_action(scope, move |res: Result<serde_json::Value, lapce_rpc::RpcError>| {
                                    if let Ok(val) = res {
                                        if let Ok(user) = serde_json::from_value::<lapce_rpc::ahead::GitHubUser>(val) {
                                            ahead_state.authenticated_user.set(Some(user));
                                            ahead_state.show_token_input.set(false);
                                            ahead_state.show_auth_modal.set(false);
                                        }
                                    }
                                });
                                proxy.ahead_request(
                                    lapce_rpc::ahead::AheadRequest::GitHubAuthSignInWithToken { token },
                                    move |res| send(res),
                                );
                            }
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
                .min_width(400.0)
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

/// Workspace Team & Collaboration Modal (Where to Add People to Workspace)
pub fn workspace_collab_modal(
    window_tab_data: Rc<WindowTabData>,
    config: ReadSignal<Arc<LapceConfig>>,
) -> impl View {
    let ahead_state = window_tab_data.ahead;
    let scope = window_tab_data.scope;
    let proxy = window_tab_data.common.proxy.clone();

    container(
        stack((
            // Header
            label(|| "Workspace Team & Collaborators".to_string()).style(move |s| {
                s.font_size(18.0)
                    .font_bold()
                    .margin_bottom(6.0)
                    .color(config.get().color(LapceColor::EDITOR_FOREGROUND))
            }),
            label(|| "Add teammates and manage role permissions (Editor, Reviewer, Viewer) in this workspace.".to_string())
                .style(move |s| {
                    s.font_size(13.0)
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
                                        let send = create_ext_action(scope, move |res: Result<serde_json::Value, lapce_rpc::RpcError>| {
                                            if let Ok(val) = res {
                                                if let Ok(parts) = serde_json::from_value::<Vec<SessionParticipantRecord>>(val) {
                                                    ahead_state.workspace_participants.set(parts);
                                                }
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

            // Section 2: Add Person to Workspace
            label(|| "Add Person to Workspace".to_string()).style(move |s| {
                s.font_bold()
                    .margin_bottom(6.0)
                    .color(config.get().color(LapceColor::EDITOR_FOREGROUND))
            }),
            text_input(ahead_state.new_collaborator_input)
                .placeholder("GitHub username or email (e.g. octocat, coworker@company.com)")
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
            .style(|s| s.margin_bottom(16.0)),

            // Action Buttons
            stack((
                label(|| "+ Add to Workspace".to_string())
                    .on_click_stop({
                        let proxy = proxy.clone();
                        move |_| {
                            let handle = ahead_state.new_collaborator_input.get_untracked().trim().to_string();
                            if !handle.is_empty() {
                                let role = ahead_state.new_collaborator_role.get_untracked();
                                let send = create_ext_action(scope, move |res: Result<serde_json::Value, lapce_rpc::RpcError>| {
                                    if let Ok(val) = res {
                                        if let Ok(parts) = serde_json::from_value::<Vec<SessionParticipantRecord>>(val) {
                                            ahead_state.workspace_participants.set(parts);
                                            ahead_state.new_collaborator_input.set(String::new());
                                        }
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

                        // Mode Selector Pill
                        label(move || {
                            if let Some(view) = session_signal.get() {
                                match view.session.mode {
                                    AssistanceMode::Learn => "🟢 Learn (Socratic)",
                                    AssistanceMode::Assist => "🔵 Assist (Scaffold)",
                                }
                            } else {
                                "Start Session"
                            }
                        })
                        .on_click_stop(move |_| {
                            ahead_state.toggle_mode();
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

                        // Advance Phase Gate Button
                        stack((
                            label(|| "Advance Phase ➔".to_string())
                                .on_click_stop({
                                    let ahead_state = ahead_state;
                                    let proxy = proxy.clone();
                                    move |_| {
                                        ahead_state.advance_phase();
                                        if let Some(view) = ahead_state.active_session.get() {
                                            proxy.ahead_request(
                                                lapce_rpc::ahead::AheadRequest::AdvancePhase {
                                                    session_id: view.session.id.clone(),
                                                    expected_revision: view.workflow.revision,
                                                    target_phase_id: view.workflow.phase.id.clone(),
                                                },
                                                |_| {},
                                            );
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
                                        // Checkpoint Approval / Rejection Gate
                                        stack((
                                            label(|| "✅ Authorize & Apply".to_string())
                                                .on_click_stop({
                                                    let p_id = p_id.clone();
                                                    move |_| {
                                                        ahead_state.authorize_proposal(&p_id);
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
                                                        ahead_state.reject_proposal(&p_id);
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

