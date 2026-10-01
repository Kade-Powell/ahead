//! AHEAD Agent Threads Sidebar - GPUI Implementation
//!
//! Mirrors the multi-thread sidebar layout from Zed/AHEAD:
//! - Search threads input at top
//! - Unified thread list under the workspace root (`ahead`)
//! - Threads can be AHEAD collaborative threads or external agent threads
//! - Each thread shows title, relative time, a removal action, and active state

use gpui_kit::component::Disableable;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::dock::{BasePanel, Panel, PanelControl, PanelEvent};
use gpui_kit::component::input::{Input, InputState, Textarea, TextareaState};
use gpui_kit::component::menu::DropdownMenu;
use gpui_kit::component::popover::Popover;
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::stepper::{Stepper, StepperItem};
use gpui_kit::component::{ActiveTheme, IndexPath, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use gpui_kit_assets::IconName;
use gpui_util::ResultExt;

use crate::proxy_client::{CheckpointSummary, DurableSessionSummary, ProxyClient};
use crate::session_panel::SessionPanel;

actions!(threads_panel, [NewAheadThread, NewExternalAcpThread]);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThreadKind {
    Ahead { item_id: String, shared: bool },
    External { item_id: String, harness: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentThread {
    pub id: String,
    pub title: String,
    pub kind: ThreadKind,
    pub time_str: String,
    pub is_active: bool,
    pub parent_session_id: Option<String>,
}

fn title_from_request(request: &str) -> String {
    let title = request
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("AHEAD work session")
        .trim_matches(|character: char| {
            character.is_ascii_punctuation() || character.is_whitespace()
        });
    let mut title = title.chars().take(72).collect::<String>();
    if title.is_empty() {
        title = "AHEAD work session".to_string();
    }
    title
}

fn relative_time(updated_at: &str) -> String {
    let Ok(updated_at) = chrono::DateTime::parse_from_rfc3339(updated_at) else {
        return "earlier".to_string();
    };
    let seconds = (chrono::Utc::now() - updated_at.with_timezone(&chrono::Utc))
        .num_seconds()
        .max(0);
    match seconds {
        0..=59 => "now".to_string(),
        60..=3_599 => format!("{}m", seconds / 60),
        3_600..=86_399 => format!("{}h", seconds / 3_600),
        _ => format!("{}d", seconds / 86_400),
    }
}

fn thread_from_summary(
    summary: DurableSessionSummary,
    is_active: bool,
) -> AgentThread {
    let session_id = summary.session.id;
    let title = summary.session.title;
    let time_str = relative_time(&summary.session.updated_at);
    let (id_prefix, kind) = match summary.harness {
        ahead_rpc::ahead::HarnessKind::Ahead => (
            "ahead",
            ThreadKind::Ahead {
                item_id: session_id.clone(),
                shared: false,
            },
        ),
        ahead_rpc::ahead::HarnessKind::ExternalAcp => (
            "external",
            ThreadKind::External {
                item_id: session_id.clone(),
                harness: summary
                    .external_agent_name
                    .or(summary.external_agent_id)
                    .unwrap_or_else(|| "External agent".to_string()),
            },
        ),
    };
    AgentThread {
        id: format!("{id_prefix}-{session_id}"),
        title,
        kind,
        time_str,
        is_active,
        parent_session_id: summary.session.parent_session_id,
    }
}

fn order_linked_threads(threads: &mut Vec<AgentThread>) {
    let original = std::mem::take(threads);
    // ponytail: one-level scan is enough while only AHEAD sessions can parent handoffs.
    for thread in original.iter().filter(|thread| {
        thread.parent_session_id.as_ref().is_none_or(|parent_id| {
            !original.iter().any(|candidate| {
                matches!(
                    &candidate.kind,
                    ThreadKind::Ahead { item_id, .. } if item_id == parent_id
                )
            })
        })
    }) {
        threads.push(thread.clone());
        if let ThreadKind::Ahead { item_id, .. } = &thread.kind {
            threads.extend(
                original
                    .iter()
                    .filter(|child| {
                        child.parent_session_id.as_deref() == Some(item_id.as_str())
                    })
                    .cloned(),
            );
        }
    }
}

struct SessionCreation {
    generation: u64,
    title: String,
    starting_point: String,
    harness: ahead_rpc::ahead::HarnessKind,
    external_agent_id: Option<String>,
    external_agent_name: Option<String>,
    parent_session_id: Option<String>,
}

struct AdapterChange {
    generation: u64,
    id: String,
    display_name: String,
    installed: bool,
}

pub struct ThreadsPanel {
    pub focus: FocusHandle,
    pub search_input: Entity<InputState>,
    pub threads: Vec<AgentThread>,
    pub proxy: Option<std::sync::Arc<ProxyClient>>,
    pub session_panel: Option<Entity<SessionPanel>>,
    pub checkpoints: Vec<CheckpointSummary>,
    error: Option<SharedString>,
    external_adapters: Vec<ahead_rpc::ahead::ExternalAcpAdapter>,
    selected_external_adapter: Option<String>,
    external_adapter_catalog_loading: bool,
    new_session_generation: u64,
    session_creation_pending: bool,
    pending_external_adapter: Option<String>,
    external_adapter_select: Entity<SelectState<Vec<String>>>,
    starting_point_input: Entity<TextareaState>,
    wizard_step: usize,
    show_new_session: bool,
    new_thread_kind: ahead_rpc::ahead::HarnessKind,
    pending_thread_removal: Option<String>,
    handoff_parent_session_id: Option<String>,
}

impl ThreadsPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search_input = cx.new(|cx| InputState::new(window, cx));
        let starting_point_input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Describe what you want help with, in your own words")
                .auto_grow(3, 8)
                .submit_on_enter(false)
        });
        let external_adapter_select =
            cx.new(|cx| SelectState::new(Vec::<String>::new(), None, window, cx));
        cx.subscribe_in(
            &external_adapter_select,
            window,
            |this: &mut Self,
             _state,
             event: &SelectEvent<Vec<String>>,
             _window,
             cx| {
                if let SelectEvent::Confirm(Some(display_name)) = event {
                    if let Some(adapter) =
                        this.external_adapters.iter().find(|adapter| {
                            adapter.installed
                                && &adapter.display_name == display_name
                        })
                    {
                        this.selected_external_adapter = Some(adapter.id.clone());
                        cx.notify();
                    }
                }
            },
        )
        .detach();

        Self {
            focus: cx.focus_handle(),
            search_input,
            threads: Vec::new(),
            proxy: None,
            session_panel: None,
            checkpoints: Vec::new(),
            error: None,
            external_adapters: Vec::new(),
            selected_external_adapter: None,
            external_adapter_catalog_loading: false,
            new_session_generation: 0,
            session_creation_pending: false,
            pending_external_adapter: None,
            external_adapter_select,
            starting_point_input,
            wizard_step: 0,
            show_new_session: false,
            new_thread_kind: ahead_rpc::ahead::HarnessKind::Ahead,
            pending_thread_removal: None,
            handoff_parent_session_id: None,
        }
    }

    pub fn with_proxy(mut self, proxy: std::sync::Arc<ProxyClient>) -> Self {
        self.proxy = Some(proxy);
        self.refresh_checkpoints();
        self
    }

    pub fn with_session(mut self, session_panel: Entity<SessionPanel>) -> Self {
        self.session_panel = Some(session_panel);
        self
    }

    pub fn restore_durable_sessions(
        &mut self,
        result: Result<Vec<DurableSessionSummary>, ahead_rpc::RpcError>,
        cx: &mut Context<Self>,
    ) {
        let summaries = match result {
            Ok(summaries) => summaries,
            Err(error) => {
                self.error = Some(
                    format!("Could not load durable sessions: {}", error.message)
                        .into(),
                );
                cx.notify();
                return;
            }
        };
        let active_index = summaries.iter().position(|summary| {
            matches!(
                summary.session.lifecycle,
                ahead_rpc::ahead::SessionLifecycle::Active
            )
        });
        let restored = summaries
            .into_iter()
            .enumerate()
            .filter(|(_, summary)| {
                !matches!(
                    summary.session.lifecycle,
                    ahead_rpc::ahead::SessionLifecycle::Archived { .. }
                )
            })
            .map(|(index, summary)| {
                thread_from_summary(summary, Some(index) == active_index)
            })
            .collect::<Vec<_>>();
        for thread in restored {
            if !self.threads.iter().any(|existing| existing.id == thread.id) {
                self.threads.push(thread);
            }
        }
        order_linked_threads(&mut self.threads);
        self.error = None;
        cx.notify();
    }

    pub fn update_thread_title(
        &mut self,
        session_id: &str,
        title: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(thread) = self.threads.iter_mut().find(|thread| {
            matches!(
                &thread.kind,
                ThreadKind::Ahead { item_id, .. } | ThreadKind::External { item_id, .. }
                    if item_id == session_id
            )
        }) else {
            return;
        };
        let title = title.trim();
        if title.is_empty() || thread.title == title {
            return;
        }
        thread.title = title.to_string();
        cx.notify();
    }

    fn refresh_checkpoints(&mut self) {
        self.checkpoints = self
            .proxy
            .as_ref()
            .map(|proxy| proxy.checkpoints())
            .unwrap_or_default();
    }

    fn restore_checkpoint(
        &mut self,
        path: std::path::PathBuf,
        cx: &mut Context<Self>,
    ) {
        let Some(proxy) = self.proxy.clone() else {
            self.error =
                Some("History is unavailable until the proxy connects".into());
            cx.notify();
            return;
        };
        match proxy.restore_checkpoint(&path) {
            Ok(view) => {
                let session_id = view.session.id.clone();
                let thread_id = format!("shared-{session_id}");
                for thread in &mut self.threads {
                    thread.is_active = false;
                }
                if let Some(thread) = self
                    .threads
                    .iter_mut()
                    .find(|thread| thread.id == thread_id)
                {
                    thread.is_active = true;
                } else {
                    self.threads.push(AgentThread {
                        id: thread_id,
                        title: view.session.title.clone(),
                        kind: ThreadKind::Ahead {
                            item_id: session_id,
                            shared: true,
                        },
                        time_str: "imported".into(),
                        is_active: true,
                        parent_session_id: None,
                    });
                }
                if let Some(session_panel) = self.session_panel.clone() {
                    session_panel.update(cx, |panel, cx| {
                        panel.set_harness_kind(
                            ahead_rpc::ahead::HarnessKind::Ahead,
                            cx,
                        );
                        panel.attach_session(view.session.id.clone(), cx);
                    });
                }
                self.error = None;
                self.refresh_checkpoints();
            }
            Err(error) => {
                self.error =
                    Some(format!("Import failed: {}", error.message).into());
            }
        }
        cx.notify();
    }

    pub fn set_active_session(
        &mut self,
        session_id: &str,
        title: &str,
        harness: ahead_rpc::ahead::HarnessKind,
        cx: &mut Context<Self>,
    ) {
        let (id_prefix, kind) = match harness {
            ahead_rpc::ahead::HarnessKind::Ahead => (
                "ahead",
                ThreadKind::Ahead {
                    item_id: session_id.to_string(),
                    shared: false,
                },
            ),
            ahead_rpc::ahead::HarnessKind::ExternalAcp => (
                "external",
                ThreadKind::External {
                    item_id: session_id.to_string(),
                    harness: "External agent".into(),
                },
            ),
        };
        self.threads = vec![AgentThread {
            id: format!("{id_prefix}-{session_id}"),
            title: title.to_string(),
            kind,
            time_str: "now".into(),
            is_active: true,
            parent_session_id: None,
        }];
        if let Some(session_panel) = self.session_panel.clone() {
            session_panel.update(cx, |panel, cx| {
                panel.set_harness_kind(harness, cx);
                panel.attach_session(session_id.to_string(), cx);
            });
        }
        self.error = None;
        cx.notify();
    }

    pub fn select_thread(&mut self, thread_id: &str, cx: &mut Context<Self>) {
        let selected = self
            .threads
            .iter()
            .find(|thread| thread.id == thread_id)
            .cloned();
        for thread in &mut self.threads {
            thread.is_active = thread.id == thread_id;
        }
        if let Some(thread) = selected {
            if let Some(session_panel) = self.session_panel.clone() {
                match thread.kind {
                    ThreadKind::Ahead { item_id, .. } => {
                        session_panel.update(cx, |panel, cx| {
                            panel.set_harness_kind(
                                ahead_rpc::ahead::HarnessKind::Ahead,
                                cx,
                            );
                            panel.attach_session(item_id, cx);
                        });
                    }
                    ThreadKind::External { item_id, .. } => {
                        session_panel.update(cx, |panel, cx| {
                            panel.set_harness_kind(
                                ahead_rpc::ahead::HarnessKind::ExternalAcp,
                                cx,
                            );
                            panel.attach_session(item_id, cx);
                        });
                    }
                }
            }
        }
        self.error = None;
        cx.notify();
    }

    pub fn parent_thread_for(&self, session_id: &str) -> Option<String> {
        let parent_id =
            self.threads.iter().find_map(|thread| match &thread.kind {
                ThreadKind::External { item_id, .. } if item_id == session_id => {
                    thread.parent_session_id.as_deref()
                }
                _ => None,
            })?;
        self.threads.iter().find_map(|thread| match &thread.kind {
            ThreadKind::Ahead { item_id, .. } if item_id == parent_id => {
                Some(thread.id.clone())
            }
            _ => None,
        })
    }

    fn remove_thread(&mut self, thread_id: &str, cx: &mut Context<Self>) {
        self.threads.retain(|t| t.id != thread_id);
        let next = self.threads.first().cloned();
        if let Some(first) = self.threads.first_mut() {
            first.is_active = true;
        }
        if let (Some(thread), Some(session_panel)) =
            (next, self.session_panel.clone())
        {
            match thread.kind {
                ThreadKind::Ahead { item_id, .. } => {
                    session_panel.update(cx, |panel, cx| {
                        panel.set_harness_kind(
                            ahead_rpc::ahead::HarnessKind::Ahead,
                            cx,
                        );
                        panel.attach_session(item_id, cx);
                    })
                }
                ThreadKind::External { item_id, .. } => {
                    session_panel.update(cx, |panel, cx| {
                        panel.set_harness_kind(
                            ahead_rpc::ahead::HarnessKind::ExternalAcp,
                            cx,
                        );
                        panel.attach_session(item_id, cx);
                    })
                }
            };
        } else if let Some(session_panel) = self.session_panel.clone() {
            session_panel.update(cx, |panel, cx| panel.clear_session(cx));
        }
        self.error = None;
        cx.notify();
    }

    fn archive_thread(&mut self, thread_id: String, cx: &mut Context<Self>) {
        let Some(thread) = self
            .threads
            .iter()
            .find(|thread| thread.id == thread_id)
            .cloned()
        else {
            return;
        };
        let session_id = match thread.kind {
            ThreadKind::Ahead { item_id, .. }
            | ThreadKind::External { item_id, .. } => item_id,
        };
        let Some(proxy) = self.proxy.clone() else {
            self.error = Some("AHEAD host is unavailable".into());
            cx.notify();
            return;
        };
        self.error = None;
        let panel = cx.entity();
        cx.spawn(async move |_, cx| {
            let result = cx
                .background_spawn(async move { proxy.archive_session(&session_id) })
                .await;
            panel.update(cx, |panel, cx| match result {
                Ok(()) => panel.remove_thread(&thread_id, cx),
                Err(error) => {
                    panel.error = Some(
                        format!("Could not archive thread: {}", error.message)
                            .into(),
                    );
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn request_thread_removal(&mut self, thread_id: String, cx: &mut Context<Self>) {
        let Some(thread) = self
            .threads
            .iter()
            .find(|thread| thread.id == thread_id)
            .cloned()
        else {
            return;
        };

        if matches!(thread.kind, ThreadKind::External { .. }) {
            self.archive_thread(thread_id, cx);
            return;
        }

        self.pending_thread_removal = Some(thread_id);
        self.error = None;
        cx.notify();
    }

    fn cancel_thread_removal(&mut self, cx: &mut Context<Self>) {
        self.pending_thread_removal = None;
        self.error = None;
        cx.notify();
    }

    fn confirm_thread_removal(&mut self, cx: &mut Context<Self>) {
        let Some(thread_id) = self.pending_thread_removal.take() else {
            return;
        };
        self.archive_thread(thread_id, cx);
    }

    pub fn new_thread(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.open_new_thread(ahead_rpc::ahead::HarnessKind::Ahead, _window, cx);
    }

    pub fn new_external_thread(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_new_thread(
            ahead_rpc::ahead::HarnessKind::ExternalAcp,
            _window,
            cx,
        );
    }

    pub fn open_new_thread(
        &mut self,
        harness_kind: ahead_rpc::ahead::HarnessKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.handoff_parent_session_id = None;
        self.new_thread_kind = harness_kind;
        self.new_session_generation = self.new_session_generation.wrapping_add(1);
        self.external_adapter_catalog_loading = false;
        if harness_kind == ahead_rpc::ahead::HarnessKind::ExternalAcp {
            self.external_adapters.clear();
            self.selected_external_adapter = None;
            self.refresh_external_adapter_picker(window, cx);
        }
        self.starting_point_input.update(cx, |input, cx| {
            input.set_value("", window, cx);
        });
        self.wizard_step = 0;
        self.show_new_session = true;
        self.error = None;
        cx.notify();
        if harness_kind == ahead_rpc::ahead::HarnessKind::ExternalAcp {
            self.load_external_adapter_catalog(window, cx);
        }
    }

    pub fn open_implementation_handoff(
        &mut self,
        parent_session_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.threads.iter().any(|thread| {
            matches!(
                &thread.kind,
                ThreadKind::Ahead { item_id, .. } if item_id == parent_session_id
            )
        }) {
            self.error = Some("The originating AHEAD thread is unavailable".into());
            cx.notify();
            return;
        }
        self.open_new_thread(ahead_rpc::ahead::HarnessKind::ExternalAcp, window, cx);
        self.handoff_parent_session_id = Some(parent_session_id.to_string());
        cx.notify();
    }

    fn load_external_adapter_catalog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.external_adapter_catalog_loading = true;
        // An older installation may still be changing the catalogue on disk.
        // Wait for its reply before taking the new form's snapshot.
        if self.pending_external_adapter.is_some() {
            cx.notify();
            return;
        }
        let Some(proxy) = self.proxy.clone() else {
            self.external_adapter_catalog_loading = false;
            self.error.get_or_insert_with(|| {
                "Could not load supported agents: AHEAD host is unavailable".into()
            });
            cx.notify();
            return;
        };
        let generation = self.new_session_generation;
        cx.notify();

        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move { proxy.external_acp_adapters() })
                .await;
            this.update_in(cx, |panel, window, cx| {
                if panel.new_session_generation != generation
                    || !panel.show_new_session
                    || panel.new_thread_kind
                        != ahead_rpc::ahead::HarnessKind::ExternalAcp
                {
                    return;
                }
                panel.external_adapter_catalog_loading = false;
                match result {
                    Ok(adapters) => {
                        panel.external_adapters = adapters;
                        panel.refresh_external_adapter_picker(window, cx);
                    }
                    Err(error) => {
                        panel.error.get_or_insert_with(|| format!(
                            "Could not load supported agents: {}. Close and reopen this panel to retry.",
                            error.message
                        )
                        .into());
                    }
                }
                cx.notify();
            })
            .log_err();
        })
        .detach();
    }

    fn refresh_external_adapter_picker(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let installed = self
            .external_adapters
            .iter()
            .filter(|adapter| adapter.installed)
            .collect::<Vec<_>>();
        if !self
            .selected_external_adapter
            .as_ref()
            .is_some_and(|selected| {
                installed.iter().any(|adapter| &adapter.id == selected)
            })
        {
            self.selected_external_adapter =
                installed.first().map(|adapter| adapter.id.clone());
        }
        let selected_index =
            self.selected_external_adapter
                .as_ref()
                .and_then(|selected| {
                    installed
                        .iter()
                        .position(|adapter| &adapter.id == selected)
                        .map(IndexPath::new)
                });
        let display_names = installed
            .iter()
            .map(|adapter| adapter.display_name.clone())
            .collect();
        self.external_adapter_select.update(cx, |select, cx| {
            select.set_items(display_names, window, cx);
            select.set_selected_index(selected_index, window, cx);
        });
    }

    fn set_external_adapter_installed(
        &mut self,
        adapter_id: String,
        installed: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((proxy, change)) =
            self.prepare_adapter_change(adapter_id, installed, cx)
        else {
            return;
        };
        cx.spawn_in(window, async move |this, cx| {
            let (change, result) = cx
                .background_spawn(async move {
                    let result = proxy.set_external_acp_adapter_installed(
                        &change.id,
                        change.installed,
                    );
                    (change, result)
                })
                .await;
            this.update_in(cx, |panel, window, cx| {
                panel.finish_adapter_change(change, result, window, cx);
            })
            .log_err();
        })
        .detach();
    }

    fn prepare_adapter_change(
        &mut self,
        adapter_id: String,
        installed: bool,
        cx: &mut Context<Self>,
    ) -> Option<(std::sync::Arc<ProxyClient>, AdapterChange)> {
        if self.pending_external_adapter.is_some()
            || self.session_creation_pending
            || self.external_adapter_catalog_loading
        {
            return None;
        }
        let Some(proxy) = self.proxy.clone() else {
            self.error = Some("AHEAD host is unavailable".into());
            cx.notify();
            return None;
        };
        let display_name = self
            .external_adapters
            .iter()
            .find(|adapter| adapter.id == adapter_id)
            .map(|adapter| adapter.display_name.clone())
            .unwrap_or_else(|| adapter_id.clone());
        self.pending_external_adapter = Some(adapter_id.clone());
        self.error = None;
        cx.notify();
        Some((
            proxy,
            AdapterChange {
                generation: self.new_session_generation,
                id: adapter_id,
                display_name,
                installed,
            },
        ))
    }

    fn finish_adapter_change(
        &mut self,
        change: AdapterChange,
        result: Result<(), ahead_rpc::RpcError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pending_external_adapter = None;
        if change.generation != self.new_session_generation || !self.show_new_session
        {
            if let Err(error) = result {
                eprintln!(
                    "Adapter change finished after its form closed: {}",
                    error.message
                );
            }
            if self.show_new_session
                && self.new_thread_kind == ahead_rpc::ahead::HarnessKind::ExternalAcp
                && self.external_adapter_catalog_loading
            {
                self.load_external_adapter_catalog(window, cx);
            }
            cx.notify();
            return;
        }
        match result {
            Ok(()) => {
                if let Some(adapter) = self
                    .external_adapters
                    .iter_mut()
                    .find(|adapter| adapter.id == change.id)
                {
                    adapter.installed = change.installed;
                }
                if change.installed {
                    self.selected_external_adapter = Some(change.id);
                } else if self.selected_external_adapter.as_deref()
                    == Some(change.id.as_str())
                {
                    self.selected_external_adapter = None;
                }
                self.refresh_external_adapter_picker(window, cx);
                self.error = None;
            }
            Err(error) => {
                self.error = Some(
                    format!(
                        "Could not {} {}: {}",
                        if change.installed {
                            "install"
                        } else {
                            "remove"
                        },
                        change.display_name,
                        error.message
                    )
                    .into(),
                );
            }
        }
        cx.notify();
    }

    fn back_wizard(&mut self, cx: &mut Context<Self>) {
        if self.session_creation_pending {
            return;
        }
        self.error = None;
        if self.wizard_step == 0 {
            self.show_new_session = false;
            self.new_session_generation =
                self.new_session_generation.wrapping_add(1);
            self.external_adapter_catalog_loading = false;
        } else {
            self.wizard_step -= 1;
        }
        cx.notify();
    }

    fn close_new_session(&mut self, cx: &mut Context<Self>) {
        self.show_new_session = false;
        self.handoff_parent_session_id = None;
        self.handoff_parent_session_id = None;
        self.new_session_generation = self.new_session_generation.wrapping_add(1);
        self.external_adapter_catalog_loading = false;
        self.wizard_step = 0;
        self.error = None;
        cx.notify();
    }

    fn next_wizard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.session_creation_pending {
            return;
        }
        if self.new_thread_kind == ahead_rpc::ahead::HarnessKind::ExternalAcp {
            self.start_new_session(window, cx);
            return;
        }
        if self.wizard_step == 0
            && self.starting_point_input.read(cx).value().trim().is_empty()
        {
            self.error =
                Some("Describe what you want help with before continuing".into());
            cx.notify();
            return;
        }
        self.error = None;
        if self.wizard_step < 1 {
            self.wizard_step += 1;
        } else {
            self.start_new_session(window, cx);
            return;
        }
        cx.notify();
    }

    fn start_new_session(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some((proxy, request)) = self.prepare_session_creation(cx) else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let (request, result) = cx
                .background_spawn(async move {
                    let result = proxy.start_work_session(
                        &request.title,
                        &request.starting_point,
                        request.harness,
                        request.external_agent_id.as_deref(),
                        request.parent_session_id.as_deref(),
                    );
                    (request, result)
                })
                .await;
            this.update(cx, |panel, cx| {
                panel.finish_session_creation(request, result, cx);
            })
            .log_err();
        })
        .detach();
    }

    fn prepare_session_creation(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<(std::sync::Arc<ProxyClient>, SessionCreation)> {
        if self.session_creation_pending
            || (self.new_thread_kind == ahead_rpc::ahead::HarnessKind::ExternalAcp
                && (self.external_adapter_catalog_loading
                    || self.pending_external_adapter.is_some()))
        {
            return None;
        }
        let Some(proxy) = self.proxy.clone() else {
            self.error = Some("AHEAD host is unavailable".into());
            cx.notify();
            return None;
        };
        let starting_point = self
            .starting_point_input
            .read(cx)
            .value()
            .trim()
            .to_string();
        let new_thread_kind = self.new_thread_kind;
        let (title, starting_point, external_agent_id, external_agent_name) =
            if new_thread_kind == ahead_rpc::ahead::HarnessKind::ExternalAcp {
                let Some(adapter_id) = self.selected_external_adapter.clone() else {
                    self.error = Some(
                        "Choose an installed external agent before continuing"
                            .into(),
                    );
                    cx.notify();
                    return None;
                };
                let display_name = self
                    .external_adapters
                    .iter()
                    .find(|adapter| adapter.id == adapter_id)
                    .map(|adapter| adapter.display_name.clone())
                    .unwrap_or_else(|| adapter_id.clone());
                let request = starting_point.trim();
                let (title, starting_point) = if request.is_empty() {
                    if self.handoff_parent_session_id.is_some() {
                        (
                            "Implementation handoff".to_string(),
                            "Continue the agreed implementation".to_string(),
                        )
                    } else {
                        (
                            format!("{display_name} side thread"),
                            format!(
                                "External agent side thread using {display_name}"
                            ),
                        )
                    }
                } else {
                    (title_from_request(request), request.to_string())
                };
                (title, starting_point, Some(adapter_id), Some(display_name))
            } else {
                if starting_point.is_empty() {
                    self.error = Some(
                        "Describe what you want help with before continuing".into(),
                    );
                    cx.notify();
                    return None;
                }
                (
                    title_from_request(&starting_point),
                    starting_point,
                    None,
                    None,
                )
            };
        if title.is_empty() || starting_point.is_empty() {
            self.error =
                Some("Describe what you want help with before continuing".into());
            cx.notify();
            return None;
        }
        self.error = None;
        self.session_creation_pending = true;
        cx.notify();
        Some((
            proxy,
            SessionCreation {
                generation: self.new_session_generation,
                title,
                starting_point,
                harness: new_thread_kind,
                external_agent_id,
                external_agent_name,
                parent_session_id: self.handoff_parent_session_id.clone(),
            },
        ))
    }

    fn finish_session_creation(
        &mut self,
        request: SessionCreation,
        result: Result<String, ahead_rpc::RpcError>,
        cx: &mut Context<Self>,
    ) {
        self.session_creation_pending = false;
        let activate = self.show_new_session
            && request.generation == self.new_session_generation;
        match result {
            Ok(session_id) => {
                self.finish_started_session(session_id, request, activate, cx)
            }
            Err(error) if activate => {
                self.error = Some(
                    format!("Could not start session: {}", error.message).into(),
                );
            }
            Err(error) => {
                eprintln!(
                    "Session creation finished after its form closed: {}",
                    error.message
                );
            }
        }
        cx.notify();
    }

    fn finish_started_session(
        &mut self,
        session_id: String,
        request: SessionCreation,
        activate: bool,
        cx: &mut Context<Self>,
    ) {
        if activate {
            for thread in &mut self.threads {
                thread.is_active = false;
            }
        }
        let harness = request.harness;
        let thread_kind = match harness {
            ahead_rpc::ahead::HarnessKind::Ahead => ThreadKind::Ahead {
                item_id: session_id.clone(),
                shared: false,
            },
            ahead_rpc::ahead::HarnessKind::ExternalAcp => ThreadKind::External {
                item_id: session_id.clone(),
                harness: request
                    .external_agent_name
                    .unwrap_or_else(|| "External agent".into()),
            },
        };
        self.threads.insert(
            0,
            AgentThread {
                id: format!(
                    "{}-{session_id}",
                    if harness == ahead_rpc::ahead::HarnessKind::Ahead {
                        "ahead"
                    } else {
                        "external"
                    }
                ),
                title: request.title,
                kind: thread_kind,
                time_str: "now".into(),
                is_active: activate,
                parent_session_id: request.parent_session_id,
            },
        );
        order_linked_threads(&mut self.threads);
        if !activate {
            return;
        }
        if let Some(session_panel) = self.session_panel.clone() {
            session_panel.update(cx, |panel, cx| {
                panel.set_harness_kind(harness, cx);
                panel.attach_session(session_id, cx);
            });
        }
        self.show_new_session = false;
        self.handoff_parent_session_id = None;
        self.wizard_step = 0;
        self.error = None;
    }

    fn render_error(&self) -> Option<Div> {
        self.error.as_ref().map(|error| {
            div()
                .debug_selector(|| "thread-request-error".into())
                .child(Alert::error("thread-request-error", error.clone()))
        })
    }

    fn render_new_session(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let border = cx.theme().border;
        let text = cx.theme().sidebar_foreground;
        let muted = cx.theme().muted_foreground;
        let card = cx.theme().group_box;
        if self.new_thread_kind == ahead_rpc::ahead::HarnessKind::ExternalAcp {
            return self.render_external_picker(border, text, muted, card, cx);
        }
        let session_creation_pending = self.session_creation_pending;
        let step_content = match self.wizard_step {
            0 => v_flex()
                .gap_3()
                .child(
                    div()
                        .font_weight(gpui_kit::FontWeight::BOLD)
                        .text_color(text)
                        .child("What are you trying to accomplish?"),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(muted)
                        .child("Describe the goal, question, bug, or constraint in your own words. AHEAD will infer a starting work profile for you."),
                )
                .child(
                    Textarea::new(&self.starting_point_input)
                        .aria_label("What you want help with")
                        .disabled(session_creation_pending),
                )
                .into_any_element(),
            _ => v_flex()
                .gap_3()
                .child(
                    div()
                        .font_weight(gpui_kit::FontWeight::BOLD)
                        .text_color(text)
                        .child("Ready to start"),
                )
                .child(
                    v_flex()
                        .gap_1()
                        .p_3()
                        .bg(card)
                        .border_1()
                        .border_color(border)
                        .child(format!(
                            "{} · {} · {}",
                            ahead_rpc::ahead::WorkKind::infer_from_request(
                                self.starting_point_input.read(cx).value().as_ref(),
                            )
                                .display_name(),
                            ahead_rpc::ahead::TaskIntent::infer_from_request(
                                self.starting_point_input.read(cx).value().as_ref(),
                            )
                                .display_name(),
                            ahead_rpc::ahead::TaskIntent::infer_from_request(
                                self.starting_point_input.read(cx).value().as_ref(),
                            )
                            .assistance_mode()
                            .display_name(),
                        ))
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(muted)
                                .child(format!(
                                    "Session title: {}",
                                    title_from_request(
                                        self.starting_point_input.read(cx).value().as_ref(),
                                    )
                                )),
                        )
                        .child(self.starting_point_input.read(cx).value().to_string()),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(muted)
                        .child("This is a starting profile, not a lock. The agent will help clarify the problem statement while you remain in control of the scope and next step."),
                )
                .into_any_element(),
        };

        v_flex()
            .size_full()
            .p_3()
            .gap_3()
            .track_focus(&self.focus)
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .font_weight(gpui_kit::FontWeight::BOLD)
                            .text_color(text)
                            .child(
                                if self.new_thread_kind
                                    == ahead_rpc::ahead::HarnessKind::Ahead
                                {
                                    "New AHEAD Thread"
                                } else {
                                    "New External Agent Thread"
                                },
                            ),
                    )
                    .child(
                        Button::new("cancel_new_session")
                            .icon(IconName::X)
                            .tooltip("Close")
                            .on_click(cx.listener(|this: &mut Self, _, _, cx| {
                                this.close_new_session(cx);
                            })),
                    ),
            )
            .child(
                Stepper::new("new-session-stepper")
                    .disabled(session_creation_pending)
                    .selected_index(self.wizard_step)
                    .items(
                        ["Describe", "Review"]
                            .into_iter()
                            .map(|label| StepperItem::new().child(label)),
                    )
                    .on_click(cx.listener(
                        |this: &mut Self, index: &usize, _, cx| {
                            if this.session_creation_pending {
                                return;
                            }
                            this.error = None;
                            this.wizard_step = (*index).min(1);
                            cx.notify();
                        },
                    )),
            )
            .child(step_content)
            .children(self.render_error())
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .justify_between()
                    .child(
                        Button::new("new_session_back")
                            .icon(IconName::ChevronLeft)
                            .label("Back")
                            .disabled(
                                self.wizard_step == 0 || session_creation_pending,
                            )
                            .on_click(cx.listener(|this: &mut Self, _, _, cx| {
                                this.back_wizard(cx);
                            })),
                    )
                    .child(
                        Button::new("new_session_next")
                            .primary()
                            .icon(if self.wizard_step == 1 {
                                IconName::Play
                            } else {
                                IconName::ChevronRight
                            })
                            .disabled(session_creation_pending)
                            .label(if session_creation_pending {
                                "Starting…"
                            } else if self.wizard_step == 1 {
                                "Start Session"
                            } else {
                                "Next"
                            })
                            .on_click(cx.listener(
                                |this: &mut Self, _, window, cx| {
                                    this.next_wizard(window, cx);
                                },
                            )),
                    ),
            )
            .into_any_element()
    }

    fn render_external_picker(
        &self,
        border: gpui::Hsla,
        text: gpui::Hsla,
        muted: gpui::Hsla,
        card: gpui::Hsla,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let has_installed_agents = self
            .external_adapters
            .iter()
            .any(|adapter| adapter.installed);
        let session_creation_pending = self.session_creation_pending;
        let adapter_operation_pending = self.pending_external_adapter.is_some();
        let adapter_catalog_loading = self.external_adapter_catalog_loading;
        let agent_rows = self.external_adapters.iter().map(|adapter| {
            let agent_id = adapter.id.clone();
            let is_installed = adapter.installed;
            let is_pending = self.pending_external_adapter.as_deref()
                == Some(adapter.id.as_str());
            h_flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(text)
                        .child(adapter.display_name.clone()),
                )
                .child(
                    Button::new(format!("toggle-acp-agent-{agent_id}"))
                        .label(if is_pending {
                            if is_installed {
                                "Removing…"
                            } else {
                                "Adding…"
                            }
                        } else if is_installed {
                            "Remove"
                        } else {
                            "Install"
                        })
                        .disabled(
                            adapter_operation_pending
                                || adapter_catalog_loading
                                || session_creation_pending,
                        )
                        .when(!is_installed, |button| button.primary())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.set_external_adapter_installed(
                                agent_id.clone(),
                                !is_installed,
                                window,
                                cx,
                            );
                        })),
                )
        });
        let content = v_flex()
            .gap_3()
            .when(has_installed_agents, |content| {
                content
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(cx.theme().warning)
                            .child("External ACP agents own shell/file effects. AHEAD cannot enforce teaching read-only mode or path scopes, or attribute their edits with CodeAnchors."),
                    )
                    .child(
                        Textarea::new(&self.starting_point_input)
                            .aria_label(if self.handoff_parent_session_id.is_some() {
                                "What the external agent should implement"
                            } else {
                                "What the external side agent should do"
                            })
                            .disabled(session_creation_pending),
                    )
                    .when(self.handoff_parent_session_id.is_some(), |content| content.child(
                        div()
                            .text_size(px(11.))
                            .text_color(muted)
                            .child("The agent will receive the current AHEAD task, plan, and recent discussion. Return to the parent thread to verify and review its changes."),
                    ))
                    .child(
                        div().flex_1().min_w_0().max_w(px(420.)).child(
                            Select::new(&self.external_adapter_select)
                                .placeholder("Select an installed agent")
                                .accessibility_label("External agent")
                                .cleanable(false)
                                .disabled(session_creation_pending || adapter_operation_pending)
                                .icon(IconName::Bot),
                        ),
                    )
            })
            .when(!has_installed_agents, |content| {
                let message: SharedString = if adapter_catalog_loading {
                    "Loading supported agents…".into()
                } else if adapter_operation_pending {
                    "Adding the selected agent…".into()
                } else {
                    "Install a supported agent to start an external thread. The agent package is fetched on first launch; Node.js/npm is required.".into()
                };
                content.child(
                    div()
                        .text_size(px(11.))
                        .text_color(muted)
                        .child(message),
                )
            })
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(muted)
                    .child("Supported agents"),
            )
            .child(v_flex().gap_2().children(agent_rows))
            .into_any_element();

        v_flex()
            .size_full()
            .p_3()
            .gap_3()
            .track_focus(&self.focus)
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .font_weight(gpui_kit::FontWeight::BOLD)
                            .text_color(text)
                            .child(if self.handoff_parent_session_id.is_some() {
                                "Hand off implementation"
                            } else {
                                "New External Agent Thread"
                            }),
                    )
                    .child(
                        Button::new("cancel_external_session")
                            .icon(IconName::X)
                            .tooltip("Close")
                            .on_click(cx.listener(|this: &mut Self, _, _, cx| {
                                this.close_new_session(cx);
                            })),
                    ),
            )
            .child(
                v_flex()
                    .gap_2()
                    .p_3()
                    .bg(card)
                    .border_1()
                    .border_color(border)
                    .child(div().text_color(text).child("Choose an external agent"))
                    .child(content),
            )
            .children(self.render_error())
            .child(
                h_flex().justify_end().child(
                    Button::new("start_external_session")
                        .primary()
                        .icon(IconName::Play)
                        .label(if session_creation_pending {
                            "Starting…"
                        } else if self.handoff_parent_session_id.is_some() {
                            "Start implementation thread"
                        } else {
                            "Start external side thread"
                        })
                        .disabled(
                            self.selected_external_adapter.is_none()
                                || !has_installed_agents
                                || adapter_catalog_loading
                                || adapter_operation_pending
                                || session_creation_pending,
                        )
                        .on_click(cx.listener(|this: &mut Self, _, window, cx| {
                            this.next_wizard(window, cx);
                        })),
                ),
            )
            .into_any_element()
    }
}

impl BasePanel for ThreadsPanel {
    fn panel_name(&self) -> &'static str {
        "ahead_threads"
    }
}

impl Panel for ThreadsPanel {
    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        "Threads"
    }
    fn zoom_control(&self, _: &App) -> Option<PanelControl> {
        Some(PanelControl::Toolbar)
    }
}

impl EventEmitter<PanelEvent> for ThreadsPanel {}

impl Focusable for ThreadsPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ThreadsPanel {
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        if self.show_new_session {
            return self.render_new_session(window, cx);
        }
        let search_query = self.search_input.read(cx).value().to_lowercase();
        let border_color = cx.theme().border;
        let text_color = cx.theme().sidebar_foreground;
        let active_bg = cx.theme().sidebar_accent;
        let active_fg = cx.theme().sidebar_accent_foreground;
        let group_box = cx.theme().group_box;

        v_flex()
            .size_full()
            .min_h_0()
            .p_2()
            .gap_1()
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &NewAheadThread, window, cx| {
                this.new_thread(window, cx);
            }))
            .on_action(cx.listener(|this, _: &NewExternalAcpThread, window, cx| {
                this.new_external_thread(window, cx);
            }))
            // Top Search Bar & New Thread button
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(
                        Input::new(&self.search_input)
                            .aria_label("Search threads...")
                    )
                    .child(
                        Button::new("new_thread_icon_btn")
                            .icon(IconName::Plus)
                            .tooltip("New thread")
                            .dropdown_menu(|menu, _, _| {
                                menu.menu("New AHEAD thread", Box::new(NewAheadThread))
                                    .menu("New external agent thread", Box::new(NewExternalAcpThread))
                            })
                    )
            )
            .children(self.render_error())
            // Unified Thread List (scrollable)
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .gap_1()
                    .overflow_y_scrollbar()
                    .children(
                        self.checkpoints
                            .iter()
                            .filter({
                                let query = search_query.clone();
                                move |checkpoint| {
                                    query.is_empty()
                                        || checkpoint.title.to_lowercase().contains(&query)
                                }
                            })
                            .map(|checkpoint| {
                                let path = checkpoint.path.clone();
                                let button_id = format!("import_checkpoint_{}", checkpoint.session_id);
                                h_flex()
                                    .id(ElementId::Name(
                                        format!("checkpoint_{}", checkpoint.session_id).into(),
                                    ))
                                    .items_center()
                                    .justify_between()
                                    .px_2()
                                    .py_1()
                                    .border_1()
                                    .border_color(border_color)
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .items_center()
                                            .child(IconName::File)
                                            .child(
                                                v_flex()
                                                    .gap_1()
                                                    .child(
                                                        div()
                                                            .text_size(px(11.))
                                                            .text_color(text_color)
                                                            .child(checkpoint.title.clone()),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_size(px(9.))
                                                            .text_color(text_color)
                                                            .child(format!(
                                                                "Shared · {}",
                                                                checkpoint.phase
                                                            )),
                                                    ),
                                            ),
                                    )
                                    .child(
                                        Button::new(SharedString::from(button_id))
                                            .icon(IconName::ArrowDownToLine)
                                            .tooltip("Import Shared Session History")
                                            .on_click(cx.listener(move |this: &mut Self, _, _, cx| {
                                                this.restore_checkpoint(path.clone(), cx);
                                            })),
                                    )
                            }),
                    )
                    .children(
                        self.threads.iter()
                            .filter(move |t| search_query.is_empty() || t.title.to_lowercase().contains(&search_query))
                            .map(|thread| {
                                let thread_id = thread.id.clone();
                                let thread_id_remove = thread.id.clone();
                                let is_active = thread.is_active;
                                let is_external = matches!(
                                    thread.kind,
                                    ThreadKind::External { .. }
                                );
                                let is_child = thread.parent_session_id.is_some();
                                let panel = cx.entity();
                                let removal_action = if is_external {
                                    Button::new(SharedString::from(format!(
                                        "remove_{thread_id_remove}"
                                    )))
                                    .icon(IconName::X)
                                    .tooltip("Archive external thread")
                                    .on_click(cx.listener(move |this: &mut Self, _, _, cx| {
                                        this.request_thread_removal(
                                            thread_id_remove.clone(),
                                            cx,
                                        );
                                        cx.stop_propagation();
                                    }))
                                    .into_any_element()
                                } else {
                                    let popover_id = SharedString::from(format!(
                                        "remove-ahead-popover-{thread_id_remove}"
                                    ));
                                    let is_open = self.pending_thread_removal.as_deref()
                                        == Some(thread_id_remove.as_str());
                                    let open_panel = panel.clone();
                                    let open_thread_id = thread_id_remove.clone();
                                    let content_panel = panel.clone();
                                    let content_thread_id = thread_id_remove.clone();
                                    Popover::new(popover_id)
                                        .anchor(Anchor::BottomRight)
                                        .open(is_open)
                                        .on_open_change(move |open, _, cx| {
                                            let thread_id = open_thread_id.clone();
                                            open_panel.update(cx, |panel, cx| {
                                                if *open {
                                                    panel.pending_thread_removal =
                                                        Some(thread_id);
                                                    panel.error = None;
                                                } else if panel.pending_thread_removal.as_deref()
                                                    == Some(thread_id.as_str())
                                                {
                                                    panel.pending_thread_removal = None;
                                                    panel.error = None;
                                                }
                                                cx.notify();
                                            });
                                        })
                                        .trigger(
                                            Button::new(SharedString::from(format!(
                                                "remove_{thread_id_remove}"
                                            )))
                                            .icon(IconName::X)
                                            .tooltip("Archive AHEAD thread"),
                                        )
                                        .content(move |_, _, _| {
                                            let keep_panel = content_panel.clone();
                                            let keep_thread_id = content_thread_id.clone();
                                            let remove_panel = content_panel.clone();
                                            let remove_thread_id = content_thread_id.clone();
                                            v_flex()
                                                .min_w(px(220.))
                                                .max_w(px(320.))
                                                .gap_2()
                                                .child(
                                                    div()
                                                        .font_weight(gpui_kit::FontWeight::BOLD)
                                                        .child("Archive AHEAD thread?")
                                                )
                                                .child(
                                                    div()
                                                        .text_size(px(11.))
                                                        .child("The durable session, transcript, and checkpoint stay available. The thread stays hidden after restart."),
                                                )
                                                .child(
                                                    h_flex()
                                                        .justify_end()
                                                        .gap_2()
                                                        .child(
                                                            Button::new(SharedString::from(format!(
                                                                "keep_{keep_thread_id}"
                                                            )))
                                                            .label("Keep")
                                                            .on_click(move |_, _, cx| {
                                                                keep_panel.update(cx, |panel, cx| {
                                                                    panel.cancel_thread_removal(cx);
                                                                });
                                                            }),
                                                        )
                                                        .child(
                                                            Button::new(SharedString::from(format!(
                                                                "confirm_remove_{remove_thread_id}"
                                                            )))
                                                            .label("Archive")
                                                            .danger()
                                                            .on_click(move |_, _, cx| {
                                                                remove_panel.update(cx, |panel, cx| {
                                                                    panel.confirm_thread_removal(cx);
                                                                });
                                                            }),
                                                )
                                            )
                                        })
                                        .into_any_element()
                                };

                                h_flex()
                                    .id(ElementId::Name(thread_id.clone().into()))
                                    .items_center()
                                    .justify_between()
                                    .px_2()
                                    .py_1()
                                    .when(is_child, |row| row.ml_4())
                                    .bg(if is_active { active_bg } else { group_box })
                                    .border_1()
                                    .border_color(if is_active { active_bg } else { border_color })
                                    .cursor(gpui_kit::CursorStyle::PointingHand)
                                    .on_click(cx.listener(move |this: &mut Self, _, _, cx| {
                                        this.select_thread(&thread_id, cx);
                                    }))
                                    .child(
                                        h_flex()
                                            .flex_1()
                                            .min_w_0()
                                            .overflow_hidden()
                                            .gap_2()
                                            .items_center()
                                            .child(
                                                div()
                                                    .min_w_0()
                                                    .truncate()
                                                    .font_weight(if is_active { gpui_kit::FontWeight::BOLD } else { gpui_kit::FontWeight::NORMAL })
                                                    .text_size(px(12.))
                                                    .text_color(if is_active { active_fg } else { text_color })
                                                    .child(thread.title.clone())
                                            )
                                            .child(match &thread.kind {
                                                ThreadKind::Ahead { .. } => div()
                                                    .id(SharedString::from(format!("ahead-brand-{}", thread.id)))
                                                    .role(Role::Image)
                                                    .aria_label("AHEAD session")
                                                    .child(crate::app::ahead_icon(px(16.), cx))
                                                    .into_any_element(),
                                                ThreadKind::External { harness, .. } => div()
                                                    .text_size(px(9.))
                                                    .text_color(text_color)
                                                    .child(harness.clone())
                                                    .into_any_element(),
                                            })
                                    )
                                    .child(
                                        h_flex()
                                            .flex_shrink_0()
                                            .gap_1()
                                            .items_center()
                                            .child(
                                                div()
                                                    .text_size(px(10.))
                                                    .text_color(text_color)
                                                    .child(thread.time_str.clone())
                                            )
                                            .child(removal_action)
                                    )
                            })
                    )
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AgentThread, ThreadKind, ThreadsPanel, order_linked_threads, relative_time,
        thread_from_summary, title_from_request,
    };
    use crate::proxy_client::{DurableSessionSummary, ProxyClient};
    use ahead_rpc::ahead::{
        ExternalAcpAdapter, HarnessKind, SessionLifecycle, SessionListItem,
    };
    use gpui_kit::TestAppContext;

    #[test]
    fn implementation_child_stays_beneath_parent_after_restore() {
        let parent = AgentThread {
            id: "ahead-parent".into(),
            title: "Design search".into(),
            kind: ThreadKind::Ahead {
                item_id: "parent".into(),
                shared: false,
            },
            time_str: "1m".into(),
            is_active: false,
            parent_session_id: None,
        };
        let child = AgentThread {
            id: "external-child".into(),
            title: "Implement search".into(),
            kind: ThreadKind::External {
                item_id: "child".into(),
                harness: "Pi".into(),
            },
            time_str: "now".into(),
            is_active: true,
            parent_session_id: Some("parent".into()),
        };
        let mut threads = vec![child, parent];
        order_linked_threads(&mut threads);
        assert_eq!(
            threads
                .iter()
                .map(|thread| thread.id.as_str())
                .collect::<Vec<_>>(),
            ["ahead-parent", "external-child"]
        );
    }

    #[gpui_kit::test]
    fn failed_session_creation_renders_error_without_losing_input(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::component::init);
        let (panel, cx) = cx.add_window_view(ThreadsPanel::new);
        panel.update_in(cx, |panel, window, cx| {
            panel.new_thread(window, cx);
            panel.starting_point_input.update(cx, |input, cx| {
                input.set_value("Keep my work description", window, cx);
            });
            panel.wizard_step = 1;
            panel.start_new_session(window, cx);
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds("thread-request-error").is_some());
        panel.update(cx, |panel, cx| {
            assert!(panel.show_new_session);
            assert!(panel.threads.is_empty());
            assert_eq!(panel.error.as_deref(), Some("AHEAD host is unavailable"));
            assert_eq!(
                panel.starting_point_input.read(cx).value().as_ref(),
                "Keep my work description"
            );
            panel.close_new_session(cx);
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds("thread-request-error").is_none());
    }

    #[gpui_kit::test]
    fn failed_adapter_install_renders_error_in_external_picker(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::component::init);
        let (panel, cx) = cx.add_window_view(ThreadsPanel::new);
        panel.update_in(cx, |panel, window, cx| {
            panel.new_external_thread(window, cx);
            panel.set_external_adapter_installed("pi".into(), true, window, cx);
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds("thread-request-error").is_some());
        panel.update_in(cx, |panel, window, cx| {
            assert!(panel.pending_external_adapter.is_none());
            assert_eq!(panel.error.as_deref(), Some("AHEAD host is unavailable"));
            panel.new_thread(window, cx);
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds("thread-request-error").is_none());
    }

    #[gpui_kit::test]
    fn failed_history_load_renders_error_until_successful_reload(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::component::init);
        let (panel, cx) = cx.add_window_view(ThreadsPanel::new);
        panel.update(cx, |panel, cx| {
            panel.restore_durable_sessions(
                Err(ahead_rpc::RpcError {
                    code: 0,
                    message: "Session database is unavailable".into(),
                }),
                cx,
            );
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds("thread-request-error").is_some());
        panel.update(cx, |panel, cx| {
            assert_eq!(panel.error.as_deref(), Some("Could not load durable sessions: Session database is unavailable"));
            panel.restore_durable_sessions(Ok(Vec::new()), cx);
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds("thread-request-error").is_none());
    }

    #[gpui_kit::test]
    fn session_creation_blocks_duplicate_submission_and_allows_retry(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::component::init);
        let (panel, cx) = cx.add_window_view(ThreadsPanel::new);
        panel.update_in(cx, |panel, window, cx| {
            panel.proxy = Some(ProxyClient::new_for_test("/test".into()));
            panel.new_thread(window, cx);
            panel.starting_point_input.update(cx, |input, cx| {
                input.set_value("Create exactly once", window, cx);
            });
            panel.wizard_step = 1;
            let (_, request) =
                panel.prepare_session_creation(cx).expect("first request");
            assert_eq!(request.title, "Create exactly once");
            assert!(panel.prepare_session_creation(cx).is_none());
            panel.start_new_session(window, cx);
            panel.back_wizard(cx);
            assert_eq!(panel.wizard_step, 1);
            panel.finish_session_creation(
                request,
                Err(ahead_rpc::RpcError {
                    code: 0,
                    message: "Please retry".into(),
                }),
                cx,
            );
            assert!(!panel.session_creation_pending);
            assert!(panel.show_new_session);
            assert_eq!(
                panel.error.as_deref(),
                Some("Could not start session: Please retry")
            );
            assert_eq!(
                panel.starting_point_input.read(cx).value().as_ref(),
                "Create exactly once"
            );

            let (_, retry) = panel.prepare_session_creation(cx).expect("retry");
            assert!(panel.error.is_none());
            panel.finish_session_creation(retry, Ok("created-once".into()), cx);
            assert!(!panel.show_new_session);
            assert_eq!(panel.threads.len(), 1);
            assert!(panel.threads[0].is_active);
        });
    }

    #[gpui_kit::test]
    fn late_session_creation_preserves_new_form_and_active_thread(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::component::init);
        let (panel, cx) = cx.add_window_view(ThreadsPanel::new);
        panel.update_in(cx, |panel, window, cx| {
            panel.proxy = Some(ProxyClient::new_for_test("/test".into()));
            panel.threads.push(super::AgentThread {
                id: "ahead-active".into(),
                title: "Current session".into(),
                kind: super::ThreadKind::Ahead {
                    item_id: "active".into(),
                    shared: false,
                },
                time_str: "now".into(),
                is_active: true,
                parent_session_id: None,
            });
            for succeeded in [false, true] {
                panel.new_thread(window, cx);
                panel.starting_point_input.update(cx, |input, cx| {
                    input.set_value("Original request", window, cx);
                });
                let (_, request) =
                    panel.prepare_session_creation(cx).expect("first request");
                panel.close_new_session(cx);
                panel.new_thread(window, cx);
                panel.starting_point_input.update(cx, |input, cx| {
                    input.set_value("New request", window, cx);
                });
                panel.error = Some("New form error".into());
                assert!(panel.prepare_session_creation(cx).is_none());
                let result = if succeeded {
                    Ok("late-session".into())
                } else {
                    Err(ahead_rpc::RpcError {
                        code: 0,
                        message: "Old form error".into(),
                    })
                };
                panel.finish_session_creation(request, result, cx);
                assert!(!panel.session_creation_pending);
                assert!(panel.show_new_session);
                assert_eq!(panel.wizard_step, 0);
                assert_eq!(panel.error.as_deref(), Some("New form error"));
                assert_eq!(
                    panel.starting_point_input.read(cx).value().as_ref(),
                    "New request"
                );
                let active = panel
                    .threads
                    .iter()
                    .filter(|thread| thread.is_active)
                    .collect::<Vec<_>>();
                assert_eq!(active.len(), 1);
                assert_eq!(active[0].id, "ahead-active");
            }
            assert_eq!(panel.threads.len(), 2);
            assert_eq!(panel.threads[0].title, "Original request");
            assert!(!panel.threads[0].is_active);
        });
    }

    #[gpui_kit::test]
    fn late_external_session_keeps_original_agent_name_without_reopening_form(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::component::init);
        let (panel, cx) = cx.add_window_view(ThreadsPanel::new);
        panel.update_in(cx, |panel, window, cx| {
            panel.new_thread(window, cx);
            panel.proxy = Some(ProxyClient::new_for_test("/test".into()));
            panel.new_thread_kind = HarnessKind::ExternalAcp;
            panel.external_adapters = vec![ExternalAcpAdapter {
                id: "pi".into(),
                display_name: "Pi".into(),
                installed: true,
            }];
            panel.selected_external_adapter = Some("pi".into());
            let (_, request) = panel
                .prepare_session_creation(cx)
                .expect("external request");
            assert_eq!(request.external_agent_id.as_deref(), Some("pi"));
            panel.close_new_session(cx);
            panel.external_adapters.clear();
            panel.finish_session_creation(request, Ok("pi-session".into()), cx);
            assert!(!panel.show_new_session);
            assert_eq!(panel.threads.len(), 1);
            assert!(!panel.threads[0].is_active);
            assert_eq!(panel.threads[0].title, "Pi side thread");
            assert_eq!(
                panel.threads[0].kind,
                super::ThreadKind::External {
                    item_id: "pi-session".into(),
                    harness: "Pi".into(),
                }
            );
        });
    }

    #[gpui_kit::test]
    fn adapter_change_blocks_duplicates_and_updates_current_picker(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::component::init);
        let (panel, cx) = cx.add_window_view(ThreadsPanel::new);
        panel.update_in(cx, |panel, window, cx| {
            panel.new_external_thread(window, cx);
            panel.proxy = Some(ProxyClient::new_for_test("/test".into()));
            panel.external_adapters = vec![ExternalAcpAdapter {
                id: "pi".into(),
                display_name: "Pi".into(),
                installed: false,
            }];
            let (_, change) = panel
                .prepare_adapter_change("pi".into(), true, cx)
                .expect("install");
            assert!(
                panel
                    .prepare_adapter_change("pi".into(), true, cx)
                    .is_none()
            );
            panel.set_external_adapter_installed("pi".into(), true, window, cx);
            assert!(panel.prepare_session_creation(cx).is_none());
            panel.finish_adapter_change(
                change,
                Err(ahead_rpc::RpcError {
                    code: 0,
                    message: "offline".into(),
                }),
                window,
                cx,
            );
            assert!(panel.pending_external_adapter.is_none());
            assert_eq!(
                panel.error.as_deref(),
                Some("Could not install Pi: offline")
            );
            assert!(!panel.external_adapters[0].installed);

            let (_, retry) = panel
                .prepare_adapter_change("pi".into(), true, cx)
                .expect("retry");
            panel.finish_adapter_change(retry, Ok(()), window, cx);
            assert!(panel.external_adapters[0].installed);
            assert_eq!(panel.selected_external_adapter.as_deref(), Some("pi"));
            assert!(panel.error.is_none());

            let (_, remove) = panel
                .prepare_adapter_change("pi".into(), false, cx)
                .expect("remove");
            panel.finish_adapter_change(remove, Ok(()), window, cx);
            assert!(!panel.external_adapters[0].installed);
            assert!(panel.selected_external_adapter.is_none());
        });
    }

    #[gpui_kit::test]
    fn late_adapter_reply_preserves_new_form_errors_and_selection(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::component::init);
        let (panel, cx) = cx.add_window_view(ThreadsPanel::new);
        panel.update_in(cx, |panel, window, cx| {
            panel.proxy = Some(ProxyClient::new_for_test("/test".into()));
            for succeeded in [false, true] {
                panel.new_thread(window, cx);
                let (_, change) = panel
                    .prepare_adapter_change("pi".into(), true, cx)
                    .expect("install");
                panel.close_new_session(cx);
                panel.new_thread(window, cx);
                panel.selected_external_adapter = Some("codex".into());
                panel.error = Some("New form error".into());
                let result = if succeeded {
                    Ok(())
                } else {
                    Err(ahead_rpc::RpcError {
                        code: 0,
                        message: "Old install error".into(),
                    })
                };
                panel.finish_adapter_change(change, result, window, cx);
                assert!(panel.pending_external_adapter.is_none());
                assert!(panel.show_new_session);
                assert_eq!(panel.new_thread_kind, HarnessKind::Ahead);
                assert_eq!(panel.error.as_deref(), Some("New form error"));
                assert_eq!(
                    panel.selected_external_adapter.as_deref(),
                    Some("codex")
                );
            }
        });
    }

    #[gpui_kit::test]
    fn reopened_external_catalogue_waits_for_older_installation(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::component::init);
        let (panel, cx) = cx.add_window_view(ThreadsPanel::new);
        panel.update_in(cx, |panel, window, cx| {
            panel.new_thread(window, cx);
            panel.proxy = Some(ProxyClient::new_for_test("/test".into()));
            let (_, change) = panel
                .prepare_adapter_change("pi".into(), true, cx)
                .expect("install");
            panel.close_new_session(cx);
            // This must defer the actual catalogue RPC, not snapshot the old install state.
            panel.new_external_thread(window, cx);
            assert!(panel.external_adapter_catalog_loading);
            assert!(panel.external_adapters.is_empty());
            assert!(panel.prepare_session_creation(cx).is_none());
            assert!(
                panel
                    .prepare_adapter_change("codex".into(), true, cx)
                    .is_none()
            );
            // A missing host makes the deferred refresh observable without issuing an RPC.
            panel.proxy = None;
            panel.finish_adapter_change(change, Ok(()), window, cx);
            assert!(panel.pending_external_adapter.is_none());
            assert!(!panel.external_adapter_catalog_loading);
            assert_eq!(
                panel.error.as_deref(),
                Some("Could not load supported agents: AHEAD host is unavailable")
            );
            assert!(panel.show_new_session);
        });
    }

    #[test]
    fn derives_a_short_session_title_from_the_human_request() {
        assert_eq!(
            title_from_request("We have a production bug\nIt happens after deploy"),
            "We have a production bug"
        );
        assert_eq!(title_from_request("   "), "AHEAD work session");
    }

    #[test]
    fn truncates_long_request_titles() {
        let title = title_from_request(&"x".repeat(100));
        assert_eq!(title.len(), 72);
    }

    #[test]
    fn formats_durable_session_age_for_the_thread_rail() {
        let created_at =
            (chrono::Utc::now() - chrono::Duration::minutes(90)).to_rfc3339();
        assert_eq!(relative_time(&created_at), "1h");
        assert_eq!(relative_time("not-a-timestamp"), "earlier");
    }

    #[test]
    fn thread_rail_shows_last_message_age() {
        let updated_at =
            (chrono::Utc::now() - chrono::Duration::minutes(90)).to_rfc3339();
        let thread = thread_from_summary(
            DurableSessionSummary {
                session: SessionListItem {
                    id: "session-1".into(),
                    title: "Follow-up".into(),
                    lifecycle: SessionLifecycle::Active,
                    created_at: "2026-09-01T12:00:00Z".into(),
                    updated_at,
                    backend: None,
                    parent_session_id: None,
                },
                harness: HarnessKind::Ahead,
                external_agent_id: None,
                external_agent_name: None,
            },
            false,
        );
        assert_eq!(thread.time_str, "1h");
    }
}
