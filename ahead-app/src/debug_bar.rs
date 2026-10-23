//! DAP debug control bar: breakpoint list + step controls.
//!
//! Backed by `ProxyClient` + `ahead-proxy` DAP dispatch. Gutter breakpoint
//! toggling lives in `CodePanel`'s gutter rail state; this bar surfaces the
//! same breakpoint set with start/stop/step controls and stop reason.

use ahead_rpc::dap_types::{DapId, DapSessionState, ThreadId};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    rc::Rc,
    sync::Arc,
};

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::dock::{
    BasePanel, Panel, PanelControl, PanelEvent, PanelId, TabGroup,
};
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::{ActiveTheme, Disableable, IndexPath, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use gpui_kit_assets::IconName;

use crate::proxy_client::ProxyClient;

type SpeechInterruptionHandler = Rc<dyn Fn()>;
type DebugSourceNavigationHandler = Rc<dyn Fn(String, u32, &mut Window, &mut App)>;
type DebugFrameScopes = Vec<(
    ahead_rpc::dap_types::Scope,
    Vec<ahead_rpc::dap_types::Variable>,
)>;
type DebugVariables = Vec<ahead_rpc::dap_types::Variable>;

#[derive(PartialEq, Eq)]
struct DebugSelectionKey {
    dap_id: Option<DapId>,
    state: DapSessionState,
    thread_id: Option<ThreadId>,
    thread_frame_ids: Vec<(ThreadId, Vec<usize>)>,
}

fn debug_config_label(config: &ahead_rpc::dap_types::RunDebugConfig) -> String {
    format!(
        "{} · {} · {}",
        config.name,
        config.ty.as_deref().unwrap_or("DAP"),
        config.program,
    )
}

fn render_debug_variables(
    variables: &[ahead_rpc::dap_types::Variable],
    depth: usize,
    path: &str,
    remaining: &mut usize,
    expanded: &HashSet<usize>,
    loaded: &HashMap<usize, DebugVariables>,
    loading: &HashSet<usize>,
    errors: &HashMap<usize, String>,
    cx: &Context<DebugBar>,
) -> Vec<AnyElement> {
    let mut rows = Vec::new();
    for (index, variable) in variables.iter().take(200).enumerate() {
        if *remaining == 0 {
            break;
        }
        *remaining -= 1;
        let reference = variable.variables_reference;
        let is_expandable = reference != 0;
        let is_expanded = expanded.contains(&reference);
        let row_path =
            format!("{path}-{depth}-{index}-{reference}-{}", variable.name);
        let mut row = h_flex()
            .min_w_0()
            .h(px(25.))
            .pl(px((depth * 14) as f32))
            .gap_1()
            .items_center();
        if is_expandable {
            let icon = if is_expanded {
                IconName::ChevronDown
            } else {
                IconName::ChevronRight
            };
            row = row.child(
                Button::new(format!("debug-variable-{row_path}"))
                    .icon(icon)
                    .ghost()
                    .tooltip(if is_expanded {
                        "Collapse variable"
                    } else {
                        "Expand variable"
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.toggle_variable(reference, window, cx);
                    })),
            );
        } else {
            row = row.child(div().w(px(20.)));
        }
        row = row
            .child(
                div()
                    .max_w(px(180.))
                    .truncate()
                    .text_size(px(11.))
                    .text_color(cx.theme().sidebar_foreground)
                    .child(variable.name.clone()),
            )
            .when_some(variable.ty.clone(), |row, ty| {
                row.child(
                    div()
                        .max_w(px(140.))
                        .truncate()
                        .text_size(px(10.))
                        .text_color(cx.theme().muted_foreground)
                        .child(ty),
                )
            })
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(cx.theme().muted_foreground)
                    .child("="),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .text_size(px(11.))
                    .text_color(cx.theme().sidebar_foreground)
                    .child(variable.value.clone()),
            );
        rows.push(row.into_any_element());

        if !is_expanded {
            continue;
        }
        if depth >= 32 {
            rows.push(
                div()
                    .pl(px(((depth + 1) * 14) as f32))
                    .text_size(px(10.))
                    .text_color(cx.theme().muted_foreground)
                    .child("Maximum variable depth reached")
                    .into_any_element(),
            );
            continue;
        }
        if let Some(children) = loaded.get(&reference) {
            if children.is_empty() {
                rows.push(
                    div()
                        .pl(px(((depth + 1) * 14) as f32))
                        .text_size(px(10.))
                        .text_color(cx.theme().muted_foreground)
                        .child("No child variables")
                        .into_any_element(),
                );
            } else {
                rows.extend(render_debug_variables(
                    children,
                    depth + 1,
                    &row_path,
                    remaining,
                    expanded,
                    loaded,
                    loading,
                    errors,
                    cx,
                ));
            }
        } else if loading.contains(&reference) {
            rows.push(
                div()
                    .pl(px(((depth + 1) * 14) as f32))
                    .text_size(px(10.))
                    .text_color(cx.theme().muted_foreground)
                    .child("Loading variables…")
                    .into_any_element(),
            );
        } else if let Some(error) = errors.get(&reference) {
            rows.push(
                div()
                    .pl(px(((depth + 1) * 14) as f32))
                    .text_size(px(10.))
                    .text_color(cx.theme().danger)
                    .child(format!("Could not load variables: {error}"))
                    .into_any_element(),
            );
        }
    }
    if variables.len() > 200 {
        rows.push(
            div()
                .pl(px((depth * 14) as f32))
                .text_size(px(10.))
                .text_color(cx.theme().muted_foreground)
                .child(format!("Showing 200 of {} variables", variables.len()))
                .into_any_element(),
        );
    }
    rows
}

pub struct DebugBar {
    pub focus: FocusHandle,
    pub proxy: Arc<ProxyClient>,
    pub file_path: String,
    pub status: SharedString,
    pub active_line: u32,
    configurations: Vec<ahead_rpc::dap_types::RunDebugConfig>,
    selected_configuration: Option<usize>,
    configuration_select: Entity<SelectState<Vec<String>>>,
    panel_id: PanelId,
    tab_group: Option<WeakEntity<TabGroup>>,
    close_handler: Option<Rc<dyn Fn(&mut Window, &mut App)>>,
    new_terminal_handler: Option<Rc<dyn Fn(&mut Window, &mut App)>>,
    source_navigation_handler: Option<DebugSourceNavigationHandler>,
    speech_interruption_handler: Option<SpeechInterruptionHandler>,
    debug_selection_key: Option<DebugSelectionKey>,
    selected_thread_id: Option<ThreadId>,
    selected_frame_id: Option<usize>,
    selected_frame_scopes: Option<DebugFrameScopes>,
    scope_request_generation: u64,
    scopes_loading: bool,
    scopes_error: Option<String>,
    expanded_variables: HashSet<usize>,
    loaded_variables: HashMap<usize, DebugVariables>,
    loading_variables: HashSet<usize>,
    variable_errors: HashMap<usize, String>,
    _debug_updates: Task<()>,
}

impl DebugBar {
    pub fn new(
        proxy: Arc<ProxyClient>,
        file_path: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let receiver = proxy.subscribe_diagnostics();
        let configurations = proxy.stored_configs();
        let configuration_labels = configurations
            .iter()
            .map(debug_config_label)
            .collect::<Vec<_>>();
        let selected_configuration = (!configurations.is_empty()).then_some(0);
        let configuration_select = cx.new(|cx| {
            SelectState::new(
                configuration_labels.clone(),
                selected_configuration.map(IndexPath::new),
                _window,
                cx,
            )
        });
        cx.subscribe_in(
            &configuration_select,
            _window,
            |this: &mut Self,
             _select,
             event: &SelectEvent<Vec<String>>,
             _window,
             cx| {
                if let SelectEvent::Confirm(Some(label)) = event {
                    this.selected_configuration =
                        this.configurations.iter().position(|config| {
                            debug_config_label(config).as_str() == label.as_str()
                        });
                    cx.notify();
                }
            },
        )
        .detach();
        let debug_updates = cx.spawn(async move |this, cx| {
            while receiver.recv().await.is_ok() {
                if this
                    .update(cx, |this, cx| {
                        this.sync_debug_selection();
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            focus: cx.focus_handle(),
            proxy,
            file_path: file_path.to_string(),
            status: "Debugger idle · F9 toggles breakpoint".into(),
            active_line: 3,
            configurations,
            selected_configuration,
            configuration_select,
            panel_id: PanelId::from(cx.entity_id()),
            tab_group: None,
            close_handler: None,
            new_terminal_handler: None,
            source_navigation_handler: None,
            speech_interruption_handler: None,
            debug_selection_key: None,
            selected_thread_id: None,
            selected_frame_id: None,
            selected_frame_scopes: None,
            scope_request_generation: 0,
            scopes_loading: false,
            scopes_error: None,
            expanded_variables: HashSet::new(),
            loaded_variables: HashMap::new(),
            loading_variables: HashSet::new(),
            variable_errors: HashMap::new(),
            _debug_updates: debug_updates,
        }
    }

    pub fn set_close_handler<F>(&mut self, handler: F)
    where
        F: Fn(&mut Window, &mut App) + 'static,
    {
        self.close_handler = Some(Rc::new(handler));
    }

    pub fn set_new_terminal_handler<F>(&mut self, handler: F)
    where
        F: Fn(&mut Window, &mut App) + 'static,
    {
        self.new_terminal_handler = Some(Rc::new(handler));
    }

    pub fn set_source_navigation_handler(
        &mut self,
        handler: impl Fn(String, u32, &mut Window, &mut App) + 'static,
    ) {
        self.source_navigation_handler = Some(Rc::new(handler));
    }

    pub fn set_speech_interruption_handler(&mut self, handler: impl Fn() + 'static) {
        self.speech_interruption_handler = Some(Rc::new(handler));
    }

    fn interrupt_speech(&self) {
        if let Some(handler) = self.speech_interruption_handler.as_ref() {
            handler();
        }
    }

    fn open_source(
        &mut self,
        path: String,
        line: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(handler) = self.source_navigation_handler.as_ref() {
            handler(path, line, window, cx);
        }
    }

    fn sync_debug_selection(&mut self) {
        let debug = self.proxy.debug();
        let mut thread_frame_ids = debug
            .thread_frames
            .iter()
            .map(|(thread_id, frames)| {
                (*thread_id, frames.iter().map(|frame| frame.id).collect())
            })
            .collect::<Vec<_>>();
        thread_frame_ids.sort_by_key(|(thread_id, _)| *thread_id);
        let key = DebugSelectionKey {
            dap_id: debug.dap_id,
            state: debug.state.clone(),
            thread_id: debug.thread_id,
            thread_frame_ids,
        };
        if self.debug_selection_key.as_ref() == Some(&key) {
            return;
        }
        self.debug_selection_key = Some(key);
        self.scope_request_generation =
            self.scope_request_generation.wrapping_add(1);
        self.selected_frame_scopes = None;
        self.scopes_loading = false;
        self.scopes_error = None;
        self.expanded_variables.clear();
        self.loaded_variables.clear();
        self.loading_variables.clear();
        self.variable_errors.clear();
        self.selected_thread_id = (debug.state == DapSessionState::Stopped)
            .then_some(debug.thread_id)
            .flatten();
        self.selected_frame_id = (debug.state == DapSessionState::Stopped)
            .then(|| debug.frames.first().map(|frame| frame.id))
            .flatten();
    }

    fn select_frame(
        &mut self,
        frame_id: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let debug = self.proxy.debug();
        let selected_thread_id = self.selected_thread_id.or(debug.thread_id);
        let frames = selected_thread_id
            .and_then(|thread_id| debug.thread_frames.get(&thread_id))
            .unwrap_or(&debug.frames);
        if debug.state != DapSessionState::Stopped
            || !frames.iter().any(|frame| frame.id == frame_id)
        {
            return;
        }
        self.selected_frame_id = Some(frame_id);
        self.selected_frame_scopes = None;
        self.scopes_error = None;
        self.expanded_variables.clear();
        self.loaded_variables.clear();
        self.loading_variables.clear();
        self.variable_errors.clear();
        self.scope_request_generation =
            self.scope_request_generation.wrapping_add(1);
        let generation = self.scope_request_generation;
        let is_initial_frame = selected_thread_id == debug.thread_id
            && frames.first().is_some_and(|frame| frame.id == frame_id);
        if is_initial_frame {
            self.scopes_loading = false;
            cx.notify();
            return;
        }
        let Some(dap_id) = debug.dap_id else {
            self.scopes_loading = false;
            self.scopes_error = Some("Debug session is no longer available".into());
            cx.notify();
            return;
        };

        self.scopes_loading = true;
        let receiver = self.proxy.dap_scopes(dap_id, frame_id);
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = receiver
                .recv()
                .await
                .unwrap_or_else(|error| Err(error.to_string()));
            if let Err(error) = this.update(cx, |this, cx| {
                let current = this.proxy.debug();
                if this.scope_request_generation != generation
                    || this.selected_frame_id != Some(frame_id)
                    || current.dap_id != Some(dap_id)
                    || current.state != DapSessionState::Stopped
                    || !selected_thread_id.is_some_and(|thread_id| {
                        current.thread_frames.get(&thread_id).is_some_and(|frames| {
                            frames.iter().any(|frame| frame.id == frame_id)
                        })
                    })
                {
                    return;
                }
                this.scopes_loading = false;
                match result {
                    Ok(scopes) => this.selected_frame_scopes = Some(scopes),
                    Err(error) => this.scopes_error = Some(error),
                }
                cx.notify();
            }) {
                eprintln!("Updating selected debugger frame failed: {error}");
            }
        })
        .detach();
    }

    fn select_thread(
        &mut self,
        thread_id: ThreadId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let debug = self.proxy.debug();
        if debug.state != DapSessionState::Stopped {
            return;
        }
        let Some(frames) = debug.thread_frames.get(&thread_id) else {
            return;
        };
        self.selected_thread_id = Some(thread_id);
        if let Some(frame_id) = frames.first().map(|frame| frame.id) {
            self.select_frame(frame_id, window, cx);
        } else {
            self.selected_frame_id = None;
            self.selected_frame_scopes = None;
            self.scopes_loading = false;
            self.scopes_error = None;
            self.scope_request_generation =
                self.scope_request_generation.wrapping_add(1);
            self.expanded_variables.clear();
            self.loaded_variables.clear();
            self.loading_variables.clear();
            self.variable_errors.clear();
            cx.notify();
        }
    }

    fn toggle_variable(
        &mut self,
        reference: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if reference == 0 {
            return;
        }
        if !self.expanded_variables.insert(reference) {
            self.expanded_variables.remove(&reference);
            cx.notify();
            return;
        }
        if self.loaded_variables.contains_key(&reference)
            || self.loading_variables.contains(&reference)
        {
            cx.notify();
            return;
        }
        let debug = self.proxy.debug();
        let Some(dap_id) = debug.dap_id.filter(|_| {
            debug.state == DapSessionState::Stopped
                && self.selected_frame_id.is_some()
        }) else {
            self.variable_errors
                .insert(reference, "Debug frame is no longer available".into());
            cx.notify();
            return;
        };
        let generation = self.scope_request_generation;
        let frame_id = self.selected_frame_id;
        self.variable_errors.remove(&reference);
        self.loading_variables.insert(reference);
        let receiver = self.proxy.dap_variables(dap_id, reference);
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = receiver
                .recv()
                .await
                .unwrap_or_else(|error| Err(error.to_string()));
            if let Err(error) = this.update(cx, |this, cx| {
                let current = this.proxy.debug();
                if this.scope_request_generation != generation
                    || this.selected_frame_id != frame_id
                    || current.dap_id != Some(dap_id)
                    || current.state != DapSessionState::Stopped
                {
                    return;
                }
                this.loading_variables.remove(&reference);
                match result {
                    Ok(variables) => {
                        this.loaded_variables.insert(reference, variables);
                    }
                    Err(error) => {
                        this.variable_errors.insert(reference, error);
                    }
                }
                cx.notify();
            }) {
                eprintln!("Updating expanded debugger variable failed: {error}");
            }
        })
        .detach();
    }

    fn breakpoints(&self) -> Vec<u32> {
        let mut bps: Vec<u32> = self
            .proxy
            .breakpoints_for(PathBuf::from(&self.file_path).as_path())
            .into_iter()
            .collect();
        bps.sort_unstable();
        bps
    }

    fn status_text(&self) -> SharedString {
        let debug = self.proxy.debug();
        if let Some(error) = debug.error {
            return error.into();
        }
        if let Some(error) = self.proxy.debug_config_error() {
            return error.into();
        }
        if self.configurations.is_empty() {
            return "Add a [[configurations]] entry in .ahead/run.toml to debug this project."
                .into();
        }
        match debug.state {
            DapSessionState::Idle => self.status.clone(),
            DapSessionState::Starting => "Starting debug adapter…".into(),
            DapSessionState::Running => "Debug session running".into(),
            DapSessionState::Stopped => format!("Stopped: {}", debug.reason).into(),
            DapSessionState::Stopping => "Stopping debug adapter…".into(),
            DapSessionState::Terminated => "Debug session ended".into(),
            DapSessionState::Failed(message) => message.into(),
        }
    }

    fn toggle_current(&mut self, cx: &mut Context<Self>) {
        self.interrupt_speech();
        self.proxy.toggle_breakpoint(
            PathBuf::from(&self.file_path).as_path(),
            self.active_line,
        );
        self.status = format!("Breakpoint Ln {}", self.active_line).into();
        cx.notify();
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        self.interrupt_speech();
        let Some(config) = self
            .selected_configuration
            .and_then(|index| self.configurations.get(index))
            .cloned()
        else {
            self.status = self
                .proxy
                .debug_config_error()
                .unwrap_or_else(|| "No debug configuration is available".into())
                .into();
            cx.notify();
            return;
        };
        let bps: Vec<u32> = self.breakpoints();
        let mut map: HashMap<PathBuf, Vec<ahead_rpc::dap_types::SourceBreakpoint>> =
            HashMap::new();
        map.insert(
            PathBuf::from(&self.file_path),
            bps.into_iter()
                .map(|line| ahead_rpc::dap_types::SourceBreakpoint {
                    line: line as usize,
                    ..Default::default()
                })
                .collect(),
        );
        if self.proxy.dap_start(config, map) {
            self.status = "Debug session starting via ahead-proxy".into();
        }
        cx.notify();
    }

    pub(crate) fn handle_shortcut(
        &mut self,
        key: &str,
        shift: bool,
        cx: &mut Context<Self>,
    ) {
        let debug = self.proxy.debug();
        let can_step = debug.state == DapSessionState::Stopped
            && self.selected_thread_id.or(debug.thread_id).is_some();
        match (key, shift) {
            ("f5", false)
                if !debug.state.is_active() && !debug.connection_closed =>
            {
                self.start(cx)
            }
            ("f5", false) if can_step => {
                self.interrupt_speech();
                if self.proxy.dap_continue_for_thread(self.selected_thread_id) {
                    self.status = "Continue requested".into();
                }
                cx.notify();
            }
            ("f9", false) => self.toggle_current(cx),
            ("f10", false) if can_step => {
                self.interrupt_speech();
                if self.proxy.dap_step_over_for_thread(self.selected_thread_id) {
                    self.status = "Step over requested".into();
                }
                cx.notify();
            }
            ("f11", false) if can_step => {
                self.interrupt_speech();
                if self.proxy.dap_step_into_for_thread(self.selected_thread_id) {
                    self.status = "Step into requested".into();
                }
                cx.notify();
            }
            ("f11", true) if can_step => {
                self.interrupt_speech();
                if self.proxy.dap_step_out_for_thread(self.selected_thread_id) {
                    self.status = "Step out requested".into();
                }
                cx.notify();
            }
            ("f5", true) if debug.state.can_stop() => {
                self.interrupt_speech();
                if self.proxy.dap_stop() {
                    self.status = "Stop requested".into();
                }
                cx.notify();
            }
            _ => {}
        }
    }
}

impl BasePanel for DebugBar {
    fn panel_name(&self) -> &'static str {
        "ahead_debug_bar"
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

impl Panel for DebugBar {
    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let group = self.tab_group.clone();
        let panel_id = self.panel_id;
        let close_handler = self.close_handler.clone();
        h_flex().items_center().gap_1().child("Debug").child(
            Button::new("close_debug_panel")
                .icon(IconName::X)
                .ghost()
                .tooltip("Close Debug")
                .on_click(move |_, window, cx| {
                    if let Some(handler) = close_handler.as_ref() {
                        handler(window, cx);
                    } else if let Some(group) = group.as_ref() {
                        let is_last = group
                            .read_with(cx, |group, _| group.panels().len() == 1)
                            .unwrap_or(false);
                        if !is_last {
                            _ = group.update(cx, |group, cx| {
                                group.close_panel(panel_id, cx);
                            });
                        }
                    }
                }),
        )
    }
    fn toolbar_buttons(
        &mut self,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Vec<Button>> {
        let handler = self.new_terminal_handler.clone();
        Some(vec![
            Button::new("new_terminal_from_debug")
                .icon(IconName::Plus)
                .tooltip("New Terminal")
                .on_click(move |_, window, cx| {
                    if let Some(handler) = handler.as_ref() {
                        handler(window, cx);
                    }
                }),
        ])
    }
    fn zoom_control(&self, _: &App) -> Option<PanelControl> {
        None
    }
}

impl EventEmitter<PanelEvent> for DebugBar {}

impl Focusable for DebugBar {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for DebugBar {
    fn render(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let text = cx.theme().sidebar_foreground;
        let border = cx.theme().border;
        let panel = cx.theme().sidebar;
        let muted = cx.theme().muted_foreground;
        let dbg = self.proxy.debug();
        let can_step = dbg.state == DapSessionState::Stopped
            && self.selected_thread_id.or(dbg.thread_id).is_some();
        let bps = self.breakpoints();
        let mut thread_ids = dbg.thread_frames.keys().copied().collect::<Vec<_>>();
        thread_ids.sort_unstable();
        let selected_thread_id = self.selected_thread_id.or(dbg.thread_id);
        let selected_frames = selected_thread_id
            .and_then(|thread_id| dbg.thread_frames.get(&thread_id))
            .cloned()
            .unwrap_or_else(|| dbg.frames.clone());
        let selected_frame_id = self.selected_frame_id;
        let initial_frame_id = selected_frames.first().map(|frame| frame.id);
        let scopes_to_render = if selected_thread_id == dbg.thread_id
            && selected_frame_id == initial_frame_id
        {
            dbg.scopes.clone()
        } else {
            self.selected_frame_scopes.clone().unwrap_or_default()
        };
        let scopes_loading = self.scopes_loading;
        let scopes_error = self.scopes_error.clone();
        let expanded_variables = &self.expanded_variables;
        let loaded_variables = &self.loaded_variables;
        let loading_variables = &self.loading_variables;
        let variable_errors = &self.variable_errors;
        let state_label = if dbg.connection_closed {
            "Disconnected".to_string()
        } else {
            match &dbg.state {
                DapSessionState::Idle => "Idle".into(),
                DapSessionState::Starting => "Starting".into(),
                DapSessionState::Running => "Running".into(),
                DapSessionState::Stopped => format!("Stopped: {}", dbg.reason),
                DapSessionState::Stopping => "Stopping".into(),
                DapSessionState::Terminated => "Session ended".into(),
                DapSessionState::Failed(_) => "Failed".into(),
            }
        };
        let status = self.status_text();

        v_flex()
            .size_full()
            .min_h_0()
            .bg(panel)
            .track_focus(&self.focus)
            .child(
                h_flex()
                    .h(px(40.))
                    .px_3()
                    .gap_2()
                    .items_center()
                    .border_b_1()
                    .border_color(border)
                    .child(IconName::Bug)
                    .child(
                        div()
                            .font_weight(gpui_kit::FontWeight::BOLD)
                            .text_size(px(12.))
                            .text_color(text)
                            .child("Debug"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(10.))
                            .text_color(muted)
                            .child(format!("{state_label} · {} breakpoint{}", bps.len(), if bps.len() == 1 { "" } else { "s" })),
                    )
                    .child(div()
                        .w(px(250.))
                        .min_w_0()
                        .child(
                            Select::new(&self.configuration_select)
                                .placeholder("Select debug configuration")
                                .accessibility_label("Debug configuration")
                                .cleanable(false)
                                .disabled(dbg.state.is_active() || self.configurations.is_empty()),
                        ))
                    .child(
                        Button::new("dbg_start")
                            .disabled(
                                dbg.state.is_active()
                                    || dbg.connection_closed
                                    || self.configurations.is_empty(),
                            )
                            .icon(IconName::Play)
                            .label("Start")
                            .tooltip("Start Debug Session (F5)")
                            .on_click(cx.listener(|this: &mut Self, _, _, cx| this.start(cx))),
                    )
                    .child(
                        Button::new("dbg_toggle_bp")
                            .icon(IconName::CircleDot)
                            .label("Breakpoint")
                            .tooltip("Toggle Breakpoint at current line (F9)")
                            .on_click(cx.listener(|this: &mut Self, _, _, cx| this.handle_shortcut("f9", false, cx))),
                    )
                    .child(
                        Button::new("dbg_continue")
                            .disabled(!can_step)
                            .icon(IconName::Play)
                            .label("Continue")
                            .tooltip("Continue (F5)")
                            .on_click(cx.listener(|this: &mut Self, _, _, cx| this.handle_shortcut("f5", false, cx))),
                    )
                    .child(
                        Button::new("dbg_step_over")
                            .disabled(!can_step)
                            .icon(IconName::StepForward)
                            .label("Step Over")
                            .tooltip("Step Over (F10)")
                            .on_click(cx.listener(|this: &mut Self, _, _, cx| this.handle_shortcut("f10", false, cx))),
                    )
                    .child(
                        Button::new("dbg_step_into")
                            .disabled(!can_step)
                            .icon(IconName::ArrowDownToLine)
                            .label("Step Into")
                            .tooltip("Step Into (F11)")
                            .on_click(cx.listener(|this: &mut Self, _, _, cx| this.handle_shortcut("f11", false, cx))),
                    )
                    .child(
                        Button::new("dbg_step_out")
                            .disabled(!can_step)
                            .icon(IconName::ArrowUpFromLine)
                            .label("Step Out")
                            .tooltip("Step Out (⇧F11)")
                            .on_click(cx.listener(|this: &mut Self, _, _, cx| this.handle_shortcut("f11", true, cx))),
                    )
                    .child(
                        Button::new("dbg_stop")
                            .disabled(!dbg.state.can_stop())
                            .icon(IconName::Square)
                            .tooltip("Stop Session (⇧F5)")
                            .on_click(cx.listener(|this: &mut Self, _, _, cx| this.handle_shortcut("f5", true, cx))),
                    ),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        v_flex()
                            .w(px(260.))
                            .min_h_0()
                            .p_3()
                            .gap_2()
                            .border_r_1()
                            .border_color(border)
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(IconName::CircleDot)
                                    .child(
                                        div()
                                            .font_weight(gpui_kit::FontWeight::BOLD)
                                            .text_size(px(11.))
                                            .text_color(text)
                                            .child("Breakpoints"),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(muted)
                                    .child("Click a breakpoint to focus its source line."),
                            )
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_h_0()
                                    .overflow_y_scrollbar()
                                    .when(bps.is_empty(), |el| {
                                        el.child(
                                            div()
                                                .py_2()
                                                .text_size(px(11.))
                                                .text_color(muted)
                                                .child("No breakpoints"),
                                        )
                                    })
                                    .children(bps.iter().map(|line| {
                                        let line = *line;
                                        h_flex()
                                            .h(px(28.))
                                            .px_2()
                                            .gap_2()
                                            .items_center()
                                            .id(("breakpoint", line as usize))
                                            .cursor(CursorStyle::PointingHand)
                                            .on_click(cx.listener(move |this: &mut Self, _, window, cx| {
                                                this.active_line = line;
                                                this.status = format!("Breakpoint Ln {line}").into();
                                                this.open_source(
                                                    this.file_path.clone(),
                                                    line,
                                                    window,
                                                    cx,
                                                );
                                                cx.notify();
                                            }))
                                            .child(IconName::CircleDot)
                                            .child(
                                                div()
                                                    .text_size(px(11.))
                                                    .text_color(text)
                                                    .child(format!("{}:{}", self.file_path, line)),
                                            )
                                    })),
                            ),
                    )
                    .child(
                        v_flex()
                            .w(px(360.))
                            .min_w_0()
                            .min_h_0()
                            .p_4()
                            .gap_2()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(IconName::Bug)
                                    .child(
                                        div()
                                            .font_weight(gpui_kit::FontWeight::BOLD)
                                            .text_size(px(12.))
                                            .text_color(text)
                                            .child("Call Stack"),
                                    ),
                            )
                            .when(thread_ids.len() > 1, |panel| {
                                panel.child(
                                    h_flex()
                                        .gap_1()
                                        .children(thread_ids.iter().map(|thread_id| {
                                            let thread_id = *thread_id;
                                            let selected =
                                                selected_thread_id == Some(thread_id);
                                            Button::new(format!("debug-thread-{thread_id}"))
                                                .icon(if selected {
                                                    IconName::CircleDot
                                                } else {
                                                    IconName::Circle
                                                })
                                                .label(format!("Thread {thread_id}"))
                                                .ghost()
                                                .on_click(cx.listener(
                                                    move |this: &mut Self, _, window, cx| {
                                                        this.select_thread(
                                                            thread_id, window, cx,
                                                        );
                                                    },
                                                ))
                                        })),
                                )
                            })
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(muted)
                                    .child(selected_thread_id.map_or_else(
                                        || "Pause execution to inspect stack frames.".to_string(),
                                        |thread_id| {
                                            selected_frame_id
                                                .and_then(|frame_id| {
                                                    selected_frames
                                                        .iter()
                                                        .position(|frame| frame.id == frame_id)
                                                })
                                                .map_or_else(
                                                    || format!("Thread {thread_id}"),
                                                    |index| format!("Thread {thread_id} · Frame {}", index + 1),
                                                )
                                        },
                                    )),
                            )
                            .child(v_flex()
                                .flex_1()
                                .min_h_0()
                                .overflow_y_scrollbar()
                                .when(selected_frames.is_empty(), |list| {
                                    list.child(
                                        div()
                                            .py_2()
                                            .text_size(px(11.))
                                            .text_color(muted)
                                            .child(if dbg.connection_closed {
                                                "AHEAD proxy connection closed.".to_string()
                                            } else {
                                                state_label.clone()
                                            }),
                                    )
                                })
                                .children(selected_frames.iter().enumerate().map(|(index, frame)| {
                                    let line = frame.line.min(u32::MAX as usize) as u32;
                                    let source_path = frame
                                        .source
                                        .as_ref()
                                        .and_then(|source| source.path.as_ref())
                                        .map(|path| path.to_string_lossy().into_owned());
                                    let source_label = source_path
                                        .clone()
                                        .or_else(|| {
                                            frame.source.as_ref().and_then(|source| source.name.clone())
                                        })
                                        .unwrap_or_else(|| "Unknown source".to_string());
                                    let selected = selected_frame_id == Some(frame.id);
                                    let selected_frame_id = frame.id;
                                    let frame_source = source_path.clone();
                                    h_flex()
                                        .min_w_0()
                                        .h(px(32.))
                                        .px_2()
                                        .gap_2()
                                        .items_center()
                                        .id(("debug-frame", index))
                                        .debug_selector(move || {
                                            format!("debug-frame-{selected_frame_id}")
                                        })
                                        .cursor(CursorStyle::PointingHand)
                                        .when(selected, |row| {
                                            row.bg(cx.theme().accent.opacity(0.12))
                                        })
                                        .on_click(cx.listener(move |this: &mut Self, _, window, cx| {
                                            this.select_frame(selected_frame_id, window, cx);
                                            if let Some(path) = frame_source.clone() {
                                                this.open_source(path, line, window, cx);
                                            }
                                        }))
                                        .child(div()
                                            .text_size(px(10.))
                                            .text_color(muted)
                                            .child(format!("{}.", index + 1)))
                                        .child(div()
                                            .min_w_0()
                                            .flex_1()
                                            .truncate()
                                            .text_size(px(11.))
                                            .text_color(text)
                                            .child(format!("{} · {}:{line}", frame.name, source_label)))
                                }))),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .min_h_0()
                            .p_4()
                            .gap_2()
                            .border_l_1()
                            .border_color(border)
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(IconName::Braces)
                                    .child(
                                        div()
                                            .font_weight(gpui_kit::FontWeight::BOLD)
                                            .text_size(px(12.))
                                            .text_color(text)
                                            .child("Variables"),
                                    ),
                            )
                            .when(scopes_loading, |panel| {
                                panel.child(
                                    div()
                                        .text_size(px(10.))
                                        .text_color(muted)
                                        .child("Loading the selected frame’s variables…"),
                                )
                            })
                            .when_some(scopes_error, |panel, error| {
                                panel.child(
                                    div()
                                        .text_size(px(10.))
                                        .text_color(cx.theme().danger)
                                        .child(format!("Could not load frame variables: {error}")),
                                )
                            })
                            .child(v_flex()
                                .flex_1()
                                .min_h_0()
                                .overflow_y_scrollbar()
                                .when(scopes_to_render.is_empty(), |list| {
                                    list.child(
                                        div()
                                            .py_2()
                                            .text_size(px(11.))
                                            .text_color(muted)
                                            .child(if dbg.state == DapSessionState::Stopped {
                                                "No variables were returned for this frame."
                                            } else {
                                                "Variables appear when execution pauses."
                                            }),
                                    )
                                })
                                .children(scopes_to_render.iter().flat_map(|(scope, variables)| {
                                    let mut rows = vec![
                                        div()
                                            .font_weight(gpui_kit::FontWeight::BOLD)
                                            .text_size(px(11.))
                                            .text_color(text)
                                            .child(scope.name.clone())
                                            .into_any_element(),
                                    ];
                                    let mut remaining = 2_000;
                                    rows.extend(render_debug_variables(
                                        variables,
                                        0,
                                        &format!("scope-{}", scope.name),
                                        &mut remaining,
                                        expanded_variables,
                                        loaded_variables,
                                        loading_variables,
                                        variable_errors,
                                        cx,
                                    ));
                                    if remaining == 0 {
                                        rows.push(
                                            div()
                                                .text_size(px(10.))
                                                .text_color(muted)
                                                .child("Showing the first 2,000 variables")
                                                .into_any_element(),
                                        );
                                    }
                                    rows
                                }))),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(muted)
                                    .child(status),
                            ),
                    )
    }
}

#[cfg(test)]
mod tests {
    use super::{DebugBar, ProxyClient};
    use ahead_rpc::{
        core::CoreNotification,
        dap_types::DapSessionState,
        proxy::{ProxyNotification, ProxyRpc},
    };
    use gpui_kit::TestAppContext;
    use std::{cell::Cell, collections::HashMap, rc::Rc};

    #[gpui_kit::test(iterations = 20)]
    fn debugger_shows_failures_and_waits_for_confirmed_cleanup(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::component::init);
        let directory = tempfile::tempdir().expect("workspace");
        let proxy = ProxyClient::new_for_test(directory.path().to_path_buf());
        let (bar, cx) = cx.add_window_view(|window, cx| {
            DebugBar::new(proxy.clone(), "src/main.ts", window, cx)
        });
        bar.update(cx, |bar, cx| bar.handle_shortcut("f5", false, cx));
        let first = proxy.debug().dap_id.expect("session");
        assert_eq!(
            bar.read_with(cx, |bar, _| bar.status_text()),
            "Starting debug adapter…"
        );
        proxy.route_core(CoreNotification::DapSessionState {
            dap_id: first,
            state: DapSessionState::Failed("Adapter not installed".into()),
        });
        cx.run_until_parked();
        assert_eq!(
            bar.read_with(cx, |bar, _| bar.status_text()),
            "Adapter not installed"
        );
        assert!(!proxy.debug().state.is_active());
        cx.update(|window, cx| window.draw(cx).clear(cx));

        bar.update(cx, |bar, cx| bar.handle_shortcut("f5", false, cx));
        let second = proxy.debug().dap_id.expect("retry session");
        assert_ne!(first, second);
        proxy.route_core(CoreNotification::DapSessionState {
            dap_id: second,
            state: DapSessionState::Running,
        });
        proxy.route_core(CoreNotification::DapSessionState {
            dap_id: first,
            state: DapSessionState::Failed("Old failure".into()),
        });
        proxy.route_core(CoreNotification::DapError {
            dap_id: first,
            message: "Old command error".into(),
        });
        assert_eq!(
            bar.read_with(cx, |bar, _| bar.status_text()),
            "Debug session running"
        );
        proxy.route_core(CoreNotification::DapError {
            dap_id: second,
            message: "Pause rejected by adapter".into(),
        });
        cx.run_until_parked();
        assert_eq!(
            bar.read_with(cx, |bar, _| bar.status_text()),
            "Pause rejected by adapter"
        );

        bar.update(cx, |bar, cx| bar.handle_shortcut("f5", true, cx));
        assert_eq!(
            bar.read_with(cx, |bar, _| bar.status_text()),
            "Stopping debug adapter…"
        );
        bar.update(cx, |bar, cx| bar.handle_shortcut("f5", false, cx));
        assert_eq!(
            proxy.debug().dap_id,
            Some(second),
            "do not restart before cleanup"
        );
        proxy.route_core(CoreNotification::DapSessionState {
            dap_id: second,
            state: DapSessionState::Running,
        });
        assert_eq!(
            proxy.debug().state,
            DapSessionState::Stopping,
            "late launch ack"
        );
        proxy.route_core(CoreNotification::DapSessionState {
            dap_id: second,
            state: DapSessionState::Terminated,
        });
        cx.run_until_parked();
        assert_eq!(
            bar.read_with(cx, |bar, _| bar.status_text()),
            "Debug session ended"
        );
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    #[gpui_kit::test(iterations = 20)]
    fn debugger_shortcuts_use_the_stopped_session_and_notifications_wake_the_bar(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::component::init);
        let directory = tempfile::tempdir().expect("workspace");
        let proxy = ProxyClient::new_for_test(directory.path().to_path_buf());
        let rpc = proxy.rpc_for_test();
        let notifications = rpc.rx();
        let (bar, cx) = cx.add_window_view(|window, cx| {
            DebugBar::new(proxy.clone(), "src/main.rs", window, cx)
        });
        let updates = Rc::new(Cell::new(0));
        let _subscription = cx.update(|_, cx| {
            cx.observe(&bar, {
                let updates = updates.clone();
                move |_, _| updates.set(updates.get() + 1)
            })
        });
        bar.update(cx, |bar, cx| {
            for (key, shift) in
                [("f10", false), ("f11", false), ("f11", true), ("f5", true)]
            {
                bar.handle_shortcut(key, shift, cx);
            }
        });
        assert!(
            notifications.try_recv().is_err(),
            "idle shortcuts must not send controls"
        );
        bar.update(cx, |bar, cx| bar.handle_shortcut("f5", false, cx));
        let ProxyRpc::Notification(ProxyNotification::DapStart { config, .. }) =
            notifications.try_recv().expect("start")
        else {
            panic!("start notification");
        };
        bar.update(cx, |bar, cx| bar.handle_shortcut("f5", false, cx));
        assert!(
            notifications.try_recv().is_err(),
            "do not start duplicate sessions"
        );
        cx.run_until_parked();
        let previous_updates = updates.get();
        proxy.route_core(CoreNotification::DapStopped {
            dap_id: config.dap_id,
            stopped: serde_json::from_value(serde_json::json!({
                "reason": "breakpoint", "threadId": 51,
            }))
            .expect("stopped event"),
            stack_frames: HashMap::new(),
            variables: Vec::new(),
        });
        cx.run_until_parked();
        assert!(
            updates.get() > previous_updates,
            "debug notifications must wake the view"
        );
        cx.update(|window, cx| window.draw(cx).clear(cx));
        bar.update(cx, |bar, cx| bar.handle_shortcut("f10", false, cx));
        let ProxyRpc::Notification(ProxyNotification::DapStepOver {
            dap_id,
            thread_id,
        }) = notifications.try_recv().expect("step")
        else {
            panic!("step notification");
        };
        assert_eq!(dap_id, config.dap_id);
        assert_eq!(serde_json::to_value(thread_id).expect("thread ID"), 51);
        bar.update(cx, |bar, cx| bar.handle_shortcut("f5", true, cx));
        assert!(matches!(notifications.try_recv().expect("stop"),
            ProxyRpc::Notification(ProxyNotification::DapStop { dap_id }) if dap_id == config.dap_id
        ));
        bar.update(cx, |bar, cx| bar.handle_shortcut("f11", false, cx));
        assert!(notifications.try_recv().is_err());
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    #[gpui_kit::test]
    fn selecting_a_stack_frame_loads_scopes_and_discards_stale_reply(
        cx: &mut TestAppContext,
    ) {
        use ahead_rpc::{
            core::CoreNotification,
            dap_types::{DapSessionState, StackFrame, ThreadId},
            proxy::{ProxyRequest, ProxyResponse, ProxyRpc},
        };

        cx.update(gpui_kit::component::init);
        let directory = tempfile::tempdir().expect("workspace");
        let proxy = ProxyClient::new_for_test(directory.path().to_path_buf());
        let rpc = proxy.rpc_for_test();
        let messages = rpc.rx();
        let (bar, cx) = cx.add_window_view(|window, cx| {
            DebugBar::new(proxy.clone(), "src/main.rs", window, cx)
        });
        bar.update(cx, |bar, cx| bar.handle_shortcut("f5", false, cx));
        let ProxyRpc::Notification(ProxyNotification::DapStart { config, .. }) =
            messages.try_recv().expect("debug start")
        else {
            panic!("expected DAP start notification");
        };
        proxy.route_core(CoreNotification::DapSessionState {
            dap_id: config.dap_id,
            state: DapSessionState::Running,
        });
        let thread_id: ThreadId =
            serde_json::from_value(serde_json::json!(51)).expect("thread id");
        let other_thread_id: ThreadId =
            serde_json::from_value(serde_json::json!(52)).expect("other thread id");
        let source_path = directory.path().join("src/main.rs");
        let source_path_string = source_path.to_string_lossy().into_owned();
        let frame = |id, name: &str, line| {
            serde_json::from_value::<StackFrame>(serde_json::json!({
                "id": id,
                "name": name,
                "line": line,
                "column": 1,
                "source": {
                    "name": "main.rs",
                    "path": source_path_string.clone(),
                },
            }))
            .expect("stack frame")
        };
        proxy.route_core(CoreNotification::DapStopped {
            dap_id: config.dap_id,
            stopped: serde_json::from_value(serde_json::json!({
                "reason": "breakpoint",
                "threadId": 51,
            }))
            .expect("stopped event"),
            stack_frames: HashMap::from([
                (
                    thread_id,
                    vec![
                        frame(101, "main", 3),
                        frame(202, "worker", 12),
                        frame(303, "helper", 18),
                    ],
                ),
                (other_thread_id, vec![frame(404, "other", 27)]),
            ]),
            variables: Vec::new(),
        });
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));

        bar.update_in(cx, |bar, window, cx| {
            bar.select_frame(202, window, cx);
        });
        let request_id = messages
            .try_iter()
            .find_map(|message| match message {
                ProxyRpc::Request(
                    request_id,
                    ProxyRequest::DapGetScopes { dap_id, frame_id },
                ) if dap_id == config.dap_id && frame_id == 202 => Some(request_id),
                _ => None,
            })
            .expect("selected frame scopes request");
        bar.update_in(cx, |bar, window, cx| {
            bar.select_frame(303, window, cx);
        });
        let current_request_id = messages
            .try_iter()
            .find_map(|message| match message {
                ProxyRpc::Request(
                    request_id,
                    ProxyRequest::DapGetScopes { dap_id, frame_id },
                ) if dap_id == config.dap_id && frame_id == 303 => Some(request_id),
                _ => None,
            })
            .expect("latest frame scopes request");
        rpc.handle_response(
            request_id,
            Ok(ProxyResponse::DapGetScopesResponse { scopes: Vec::new() }),
        );
        cx.run_until_parked();
        bar.read_with(cx, |bar, _| {
            assert_eq!(bar.selected_frame_id, Some(303));
            assert!(
                bar.scopes_loading,
                "stale result must not finish the latest request"
            );
            assert!(bar.selected_frame_scopes.is_none());
        });
        let scope = serde_json::from_value(serde_json::json!({
            "name": "Locals",
            "variablesReference": 1,
            "expensive": false,
        }))
        .expect("scope");
        let variable = serde_json::from_value(serde_json::json!({
            "name": "value",
            "value": "42",
            "variablesReference": 70,
        }))
        .expect("variable");
        rpc.handle_response(
            current_request_id,
            Ok(ProxyResponse::DapGetScopesResponse {
                scopes: vec![(scope, vec![variable])],
            }),
        );
        cx.run_until_parked();

        bar.read_with(cx, |bar, _| {
            assert_eq!(bar.selected_frame_id, Some(303));
            assert!(!bar.scopes_loading);
            let scopes = bar.selected_frame_scopes.as_ref().expect("loaded scopes");
            assert_eq!(scopes[0].0.name, "Locals");
            assert_eq!(scopes[0].1[0].name, "value");
        });

        bar.update_in(cx, |bar, window, cx| {
            bar.toggle_variable(70, window, cx);
        });
        let variable_request_id = messages
            .try_iter()
            .find_map(|message| match message {
                ProxyRpc::Request(
                    request_id,
                    ProxyRequest::DapVariable { dap_id, reference },
                ) if dap_id == config.dap_id && reference == 70 => Some(request_id),
                _ => None,
            })
            .expect("expanded variable request");
        bar.read_with(cx, |bar, _| {
            assert!(bar.expanded_variables.contains(&70));
            assert!(bar.loading_variables.contains(&70));
        });
        let child_variable = serde_json::from_value(serde_json::json!({
            "name": "nested",
            "value": "ready",
            "variablesReference": 0,
        }))
        .expect("child variable");
        rpc.handle_response(
            variable_request_id,
            Ok(ProxyResponse::DapVariableResponse {
                varialbes: vec![child_variable],
            }),
        );
        cx.run_until_parked();
        bar.read_with(cx, |bar, _| {
            assert!(!bar.loading_variables.contains(&70));
            assert_eq!(bar.loaded_variables[&70][0].name, "nested");
        });
        bar.update_in(cx, |bar, window, cx| {
            bar.toggle_variable(70, window, cx);
        });
        bar.read_with(cx, |bar, _| {
            assert!(!bar.expanded_variables.contains(&70));
            assert!(bar.loaded_variables.contains_key(&70));
        });

        bar.update_in(cx, |bar, window, cx| {
            bar.select_thread(other_thread_id, window, cx);
        });
        let other_thread_scopes_request = messages
            .try_iter()
            .find_map(|message| match message {
                ProxyRpc::Request(
                    request_id,
                    ProxyRequest::DapGetScopes {
                        dap_id,
                        frame_id: 404,
                    },
                ) if dap_id == config.dap_id => Some(request_id),
                _ => None,
            })
            .expect("selected thread's initial frame scopes request");
        bar.read_with(cx, |bar, _| {
            assert_eq!(bar.selected_thread_id, Some(other_thread_id));
            assert_eq!(bar.selected_frame_id, Some(404));
        });
        bar.update(cx, |bar, cx| bar.handle_shortcut("f10", false, cx));
        assert!(matches!(
            messages.try_recv().expect("selected-thread step"),
            ProxyRpc::Notification(ProxyNotification::DapStepOver {
                thread_id,
                dap_id,
            }) if thread_id == other_thread_id && dap_id == config.dap_id
        ));
        rpc.handle_response(
            other_thread_scopes_request,
            Ok(ProxyResponse::DapGetScopesResponse { scopes: Vec::new() }),
        );
        cx.run_until_parked();
        bar.read_with(cx, |bar, _| {
            assert!(!bar.scopes_loading);
            assert!(
                bar.selected_frame_scopes
                    .as_ref()
                    .is_some_and(Vec::is_empty)
            );
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }
}
