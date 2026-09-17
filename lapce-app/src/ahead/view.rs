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
    views::{container, dyn_stack, label, stack, text_input, Decorators},
    View,
};
use lapce_rpc::ahead::{
    AssistanceMode, SessionParticipantRecord, SessionRole, WorkKind,
};

use crate::{
    ahead::state::AheadState,
    config::{color::LapceColor, LapceConfig},
    window_tab::WindowTabData,
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

