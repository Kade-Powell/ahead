//! Keyboard-driven file finder, following Zed's `crates/file_finder` boundary.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use crate::proxy_client::ProxyClient;
use ahead_core::search::{RankedPathMatch, rank_file_paths};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::list::ListItem;
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::{ActiveTheme, WindowExt, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use gpui_util::ResultExt;

const RESULT_LIMIT: usize = 40;
const RECENT_FILE_LIMIT: usize = 20;

fn close_dialog_on_escape_when_focused(
    focus: FocusHandle,
    cx: &mut App,
) -> Subscription {
    cx.intercept_keystrokes(move |event, window, cx| {
        if focus.is_focused(window) && event.keystroke.key.as_str() == "escape" {
            window.close_dialog(cx);
            window.prevent_default();
            cx.stop_propagation();
        }
    })
}

pub fn open(
    proxy: Arc<ProxyClient>,
    workspace: PathBuf,
    recent_files: Vec<PathBuf>,
    active_file: Option<PathBuf>,
    relative_to: Option<PathBuf>,
    on_open: Box<dyn Fn(crate::ross::OpenRequest, &mut Window, &mut App)>,
    window: &mut Window,
    cx: &mut App,
) {
    if window.has_active_dialog(cx) {
        return;
    }

    let picker = cx.new(|cx| {
        QuickOpen::new(
            proxy,
            workspace,
            recent_files,
            active_file,
            relative_to,
            on_open,
            window,
            cx,
        )
    });
    let query_focus = picker.read(cx).query.read(cx).focus_handle(cx);
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .title("Quick Open")
            .width(px(720.))
            .close_button(false)
            .on_ok({
                let picker = picker.clone();
                move |_, window, cx| {
                    picker.update(cx, |picker, cx| picker.open_selected(window, cx));
                    false
                }
            })
            .child(picker.clone())
    });
    window.focus(&query_focus, cx);
}

pub struct QuickOpen {
    proxy: Arc<ProxyClient>,
    workspace: PathBuf,
    recent_files: Vec<PathBuf>,
    active_file: Option<PathBuf>,
    relative_to: Option<PathBuf>,
    on_open: Box<dyn Fn(crate::ross::OpenRequest, &mut Window, &mut App)>,
    query: Entity<InputState>,
    _escape_interceptor: Subscription,
    paths: Option<(u64, Arc<Vec<PathBuf>>)>,
    active_request: Option<u64>,
    results: Vec<RankedPathMatch>,
    selected_index: usize,
    loading_paths: bool,
    searching: bool,
    error: Option<String>,
    search_generation: Arc<AtomicU64>,
    _file_changes: Task<()>,
}

impl QuickOpen {
    fn new(
        proxy: Arc<ProxyClient>,
        workspace: PathBuf,
        recent_files: Vec<PathBuf>,
        active_file: Option<PathBuf>,
        relative_to: Option<PathBuf>,
        on_open: Box<dyn Fn(crate::ross::OpenRequest, &mut Window, &mut App)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let query = cx.new(|cx| InputState::new(window, cx));
        let query_focus = query.read(cx).focus_handle(cx);
        let escape_interceptor =
            close_dialog_on_escape_when_focused(query_focus.clone(), cx);
        cx.subscribe(&query, |this: &mut Self, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.rank_query(cx);
            }
        })
        .detach();

        let receiver = proxy.subscribe_workspace_file_changes();
        let file_change_proxy = proxy.clone();
        let file_changes = cx.spawn(async move |this, cx| {
            while receiver.recv().await.is_ok() {
                let generation = file_change_proxy.workspace_file_generation();
                if this
                    .update(cx, |picker, cx| {
                        picker.workspace_files_changed(generation, cx)
                    })
                    .is_err()
                {
                    break;
                }
            }
        });

        let mut picker = Self {
            proxy,
            workspace,
            recent_files,
            active_file,
            relative_to,
            on_open,
            query,
            _escape_interceptor: escape_interceptor,
            paths: None,
            active_request: None,
            results: Vec::new(),
            selected_index: 0,
            loading_paths: true,
            searching: false,
            error: None,
            search_generation: Arc::new(AtomicU64::new(0)),
            _file_changes: file_changes,
        };
        picker.load_paths(cx);
        picker.load_recent_files(cx);
        picker
    }

    fn load_recent_files(&mut self, cx: &mut Context<Self>) {
        let proxy = self.proxy.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { proxy.recent_workspace_files() })
                .await;
            this.update(cx, |picker, cx| {
                match result {
                    Ok(paths) => merge_recent_paths(&mut picker.recent_files, paths),
                    Err(error) => eprintln!(
                        "AHEAD could not load recent workspace files: {}",
                        error.message
                    ),
                }
                picker.rank_query(cx);
            })
            .log_err();
        })
        .detach();
    }

    fn load_paths(&mut self, cx: &mut Context<Self>) {
        if self.active_request.is_some() {
            return;
        }

        let request_id = crate::proxy_client::next_workspace_file_request_id();
        self.active_request = Some(request_id);
        self.loading_paths = true;
        self.error = None;
        let proxy = self.proxy.clone();
        let request = proxy.workspace_files(request_id);
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { request.wait() }).await;
            this.update(cx, |picker, cx| {
                if picker.active_request != Some(request_id) {
                    return;
                }
                picker.active_request = None;
                picker.loading_paths = false;
                match result {
                    Ok((generation, paths))
                        if generation >= proxy.workspace_file_generation() =>
                    {
                        picker.paths = Some((generation, Arc::new(paths)));
                        picker.rank_query(cx);
                    }
                    Ok(_) => picker.load_paths(cx),
                    Err(error) => picker.error = Some(error.message),
                }
                cx.notify();
            })
            .log_err();
        })
        .detach();
    }

    fn workspace_files_changed(&mut self, generation: u64, cx: &mut Context<Self>) {
        if self
            .paths
            .as_ref()
            .is_some_and(|(cached_generation, _)| *cached_generation == generation)
        {
            return;
        }
        if self.paths.is_none() && self.active_request.is_some() {
            return;
        }
        if self.paths.take().is_some() {
            self.search_generation.fetch_add(1, Ordering::SeqCst);
            self.results.clear();
            self.selected_index = 0;
            self.loading_paths = true;
            self.load_paths(cx);
            cx.notify();
        } else {
            self.load_paths(cx);
        }
    }

    fn rank_query(&mut self, cx: &mut Context<Self>) {
        let raw_query = self.query.read(cx).value().to_string();
        let (path_query, _) = split_location_query(&raw_query);
        let query = path_query.to_string();
        let generation = self.search_generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.results.clear();
        self.selected_index = 0;
        self.error = None;
        let Some((snapshot_generation, paths)) = self.paths.clone() else {
            self.load_paths(cx);
            cx.notify();
            return;
        };
        if snapshot_generation < self.proxy.workspace_file_generation() {
            self.paths = None;
            self.load_paths(cx);
            cx.notify();
            return;
        }
        if query.trim().is_empty() {
            self.results = recent_path_matches(
                &self.workspace,
                &paths,
                &self.recent_files,
                self.active_file.as_deref(),
                RESULT_LIMIT,
            );
            self.searching = false;
            cx.notify();
            return;
        }

        self.searching = true;
        let workspace = self.workspace.clone();
        let relative_to = self.relative_to.clone();
        let active_file = self.active_file.clone();
        let search_generation = self.search_generation.clone();
        let active_generation = search_generation.clone();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(60))
                .await;
            if search_generation.load(Ordering::SeqCst) != generation {
                return;
            }
            let result = cx
                .background_spawn(async move {
                    let mut results = rank_file_paths(
                        &workspace,
                        paths.iter().cloned(),
                        &query,
                        relative_to.as_deref(),
                        RESULT_LIMIT,
                        || active_generation.load(Ordering::SeqCst) == generation,
                    )?;
                    promote_active_file(&mut results, active_file.as_deref());
                    Ok(results)
                })
                .await;
            this.update(cx, |picker, cx| {
                if picker.search_generation.load(Ordering::SeqCst) != generation {
                    return;
                }
                picker.searching = false;
                match result {
                    Ok(results) => picker.results = results,
                    Err(ahead_core::search::FileSearchError::Cancelled) => {}
                    Err(error) => picker.error = Some(error.to_string()),
                }
                cx.notify();
            })
            .log_err();
        })
        .detach();
    }

    fn handle_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event.keystroke.key.as_str() {
            "down" if !self.results.is_empty() => {
                self.selected_index = (self.selected_index + 1) % self.results.len();
                cx.notify();
            }
            "up" if !self.results.is_empty() => {
                self.selected_index = if self.selected_index == 0 {
                    self.results.len() - 1
                } else {
                    self.selected_index - 1
                };
                cx.notify();
            }
            "escape" => window.close_dialog(cx),
            _ => {}
        }
    }

    fn open_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self
            .results
            .get(self.selected_index)
            .map(|matched| matched.path.clone())
        else {
            return;
        };
        self.open_path(path, window, cx);
    }

    fn open_path(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(path) = path.to_str() else {
            self.error =
                Some("This file path cannot be represented as UTF-8.".to_string());
            cx.notify();
            return;
        };
        let query = self.query.read(cx).value().to_string();
        window.close_dialog(cx);
        (self.on_open)(
            crate::ross::OpenRequest {
                path: path.to_string(),
                permanent: true,
                location: split_location_query(&query).1,
            },
            window,
            cx,
        );
    }
}

fn merge_recent_paths(recent: &mut Vec<PathBuf>, restored: Vec<PathBuf>) {
    recent.truncate(RECENT_FILE_LIMIT);
    for path in restored {
        if recent.len() == RECENT_FILE_LIMIT {
            break;
        }
        if !recent.contains(&path) {
            recent.push(path);
        }
    }
}

fn recent_path_matches(
    workspace: &std::path::Path,
    paths: &[PathBuf],
    recent_files: &[PathBuf],
    active_file: Option<&std::path::Path>,
    max_results: usize,
) -> Vec<RankedPathMatch> {
    let mut recent_order = HashMap::with_capacity(
        recent_files.len() + usize::from(active_file.is_some()),
    );
    let mut next_order = 0;
    if let Some(active_file) = active_file {
        recent_order.insert(active_file.to_path_buf(), next_order);
        next_order += 1;
    }
    for path in recent_files {
        recent_order.entry(path.clone()).or_insert_with(|| {
            let order = next_order;
            next_order += 1;
            order
        });
    }

    let mut matches = paths
        .iter()
        .filter_map(|path| {
            let order = *recent_order.get(path)?;
            let relative = path.strip_prefix(workspace).ok()?;
            let relative_path = relative
                .components()
                .map(|component| component.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            Some((
                order,
                RankedPathMatch {
                    path: path.clone(),
                    relative_path,
                    score: 0.0,
                    positions: Vec::new(),
                    distance_to_relative_directory: 0,
                },
            ))
        })
        .collect::<Vec<_>>();
    matches.sort_unstable_by_key(|(order, _)| *order);
    matches
        .into_iter()
        .take(max_results)
        .map(|(_, matched)| matched)
        .collect()
}

fn promote_active_file(
    results: &mut Vec<RankedPathMatch>,
    active_file: Option<&std::path::Path>,
) {
    let Some(index) = active_file
        .and_then(|active| results.iter().position(|result| result.path == active))
    else {
        return;
    };
    if index > 0 {
        let active = results.remove(index);
        results.insert(0, active);
    }
}

fn split_location_query(query: &str) -> (&str, Option<crate::ross::OpenLocation>) {
    let query = query.trim();
    let Some((path, suffix)) = query.rsplit_once(':') else {
        return (query, None);
    };
    if path.is_empty() {
        return (query, None);
    }

    if let Some((first_line, last_line)) = suffix.split_once('-')
        && let (Ok(first_line), Ok(last_line)) =
            (first_line.parse::<usize>(), last_line.parse::<usize>())
        && first_line > 0
        && last_line >= first_line
    {
        return (
            path,
            Some(crate::ross::OpenLocation {
                line: first_line - 1,
                column: crate::ross::OpenColumn::Character(0),
                end_line: last_line,
                end_column: crate::ross::OpenColumn::Character(0),
            }),
        );
    }

    if let Ok(column) = suffix.parse::<usize>()
        && let Some((path, line)) = path.rsplit_once(':')
        && !path.is_empty()
        && let Ok(line) = line.parse::<usize>()
        && line > 0
    {
        let column = column.saturating_sub(1);
        return (
            path,
            Some(crate::ross::OpenLocation {
                line: line - 1,
                column: crate::ross::OpenColumn::Character(column),
                end_line: line - 1,
                end_column: crate::ross::OpenColumn::Character(column),
            }),
        );
    }

    if let Ok(line) = suffix.parse::<usize>()
        && line > 0
    {
        return (
            path,
            Some(crate::ross::OpenLocation {
                line: line - 1,
                column: crate::ross::OpenColumn::Character(0),
                end_line: line - 1,
                end_column: crate::ross::OpenColumn::Character(0),
            }),
        );
    }

    (query, None)
}

impl Drop for QuickOpen {
    fn drop(&mut self) {
        self.search_generation.fetch_add(1, Ordering::SeqCst);
        if let Some(request_id) = self.active_request.take() {
            self.proxy.cancel_workspace_files(request_id);
        }
    }
}

impl Render for QuickOpen {
    fn render(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let input_focus = self.query.read(cx).focus_handle(cx);
        let foreground = cx.theme().sidebar_foreground;
        let accent = cx.theme().primary;
        let muted = cx.theme().muted_foreground;
        let results = self.results.iter().enumerate().map(|(index, matched)| {
            let path = matched.path.clone();
            ListItem::new(index)
                .w_full()
                .selected(index == self.selected_index)
                .child(highlighted_path(
                    &matched.relative_path,
                    &matched.positions,
                    foreground,
                    accent,
                ))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_path(path.clone(), window, cx);
                }))
        });
        let message = if let Some(error) = &self.error {
            error.clone()
        } else if self.loading_paths {
            "Loading workspace files…".to_string()
        } else if self.searching {
            "Searching…".to_string()
        } else if self.query.read(cx).value().trim().is_empty() {
            if self.results.is_empty() {
                "Type to search files by name".to_string()
            } else {
                format!(
                    "{} recent files · ↑↓ navigate · ↵ open · Esc close",
                    self.results.len()
                )
            }
        } else if self.results.is_empty() {
            "No matching files".to_string()
        } else {
            format!(
                "{} files · ↑↓ navigate · ↵ open · Esc close",
                self.results.len()
            )
        };

        v_flex()
            .w_full()
            .track_focus(&input_focus)
            .on_key_down(cx.listener(Self::handle_key_down))
            .gap_2()
            .child(
                Input::new(&self.query)
                    .cleanable(true)
                    .aria_label("Quick Open files by name"),
            )
            .child(
                v_flex()
                    .max_h(px(420.))
                    .min_h(px(40.))
                    .overflow_y_scrollbar()
                    .children(results),
            )
            .child(
                h_flex()
                    .justify_between()
                    .text_size(px(11.))
                    .text_color(muted)
                    .child(message),
            )
    }
}

fn highlighted_path(
    path: &str,
    positions: &[usize],
    foreground: Hsla,
    accent: Hsla,
) -> AnyElement {
    h_flex()
        .children(path_segments(path, positions).into_iter().map(
            |(matched, text)| {
                let segment = div().child(text);
                if matched {
                    segment.text_color(accent).font_weight(FontWeight::BOLD)
                } else {
                    segment.text_color(foreground)
                }
            },
        ))
        .into_any_element()
}

fn path_segments(path: &str, positions: &[usize]) -> Vec<(bool, String)> {
    let mut segments: Vec<(bool, String)> = Vec::new();
    let mut next_match = positions.iter().copied().peekable();
    for (index, character) in path.chars().enumerate() {
        let matched = next_match.peek() == Some(&index);
        if matched {
            next_match.next();
        }
        if let Some((previous_match, text)) = segments.last_mut()
            && *previous_match == matched
        {
            text.push(character);
        } else {
            segments.push((matched, character.to_string()));
        }
    }
    segments
}

#[cfg(test)]
mod tests {
    use super::{
        AppContext, Arc, AtomicU64, Context, InputState, IntoElement, ParentElement,
        QuickOpen, RECENT_FILE_LIMIT, Render, Task, Window, WindowExt,
        close_dialog_on_escape_when_focused, div, merge_recent_paths, path_segments,
        promote_active_file, recent_path_matches, split_location_query,
    };
    use crate::ross::{OpenColumn, OpenLocation, OpenRequest};
    use ahead_core::search::RankedPathMatch;
    use gpui_kit::Focusable;
    use gpui_kit::TestAppContext;
    use std::{
        path::{Path, PathBuf},
        sync::Mutex,
    };

    struct QuickOpenTestRoot;

    impl Render for QuickOpenTestRoot {
        fn render(
            &mut self,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) -> impl IntoElement {
            div()
                .children(gpui_kit::component::Root::render_dialog_layer(window, cx))
        }
    }

    #[test]
    fn path_highlights_use_unicode_scalar_positions() {
        assert_eq!(
            path_segments("src/éclair.rs", &[4, 5, 6]),
            vec![
                (false, "src/".to_string()),
                (true, "écl".to_string()),
                (false, "air.rs".to_string()),
            ]
        );
    }

    #[test]
    fn quick_open_parses_line_column_and_line_range_suffixes() {
        let (path, location) = split_location_query("src/main.rs:12");
        assert_eq!(path, "src/main.rs");
        assert_eq!(
            location,
            Some(OpenLocation {
                line: 11,
                column: OpenColumn::Character(0),
                end_line: 11,
                end_column: OpenColumn::Character(0),
            })
        );

        let (path, location) = split_location_query("C:\\work\\main.rs:12:4");
        assert_eq!(path, "C:\\work\\main.rs");
        assert_eq!(
            location,
            Some(OpenLocation {
                line: 11,
                column: OpenColumn::Character(3),
                end_line: 11,
                end_column: OpenColumn::Character(3),
            })
        );

        let (path, location) = split_location_query("src/main.rs:3-5");
        assert_eq!(path, "src/main.rs");
        assert_eq!(
            location,
            Some(OpenLocation {
                line: 2,
                column: OpenColumn::Character(0),
                end_line: 5,
                end_column: OpenColumn::Character(0),
            })
        );
        assert_eq!(split_location_query("src/main.rs").1, None);
        assert_eq!(split_location_query("src/main.rs:0").1, None);
    }

    #[test]
    fn recent_files_follow_mru_order_and_only_show_indexed_paths() {
        let workspace = Path::new("/workspace");
        let active = workspace.join("src/active.rs");
        let recent = workspace.join("src/recent.rs");
        let matches = recent_path_matches(
            workspace,
            &[
                active.clone(),
                recent.clone(),
                workspace.join("src/not-open.rs"),
            ],
            std::slice::from_ref(&recent),
            Some(&active),
            40,
        );

        assert_eq!(
            matches
                .iter()
                .map(|matched| matched.relative_path.as_str())
                .collect::<Vec<_>>(),
            ["src/active.rs", "src/recent.rs"]
        );
    }

    #[test]
    fn restored_recent_files_preserve_current_order_and_respect_the_limit() {
        let mut recent = vec![
            PathBuf::from("/workspace/current.rs"),
            PathBuf::from("/workspace/shared.rs"),
        ];
        let restored =
            std::iter::once(PathBuf::from("/workspace/shared.rs"))
                .chain((0..25).map(|index| {
                    PathBuf::from(format!("/workspace/old-{index}.rs"))
                }))
                .collect();

        merge_recent_paths(&mut recent, restored);

        assert_eq!(recent[0], PathBuf::from("/workspace/current.rs"));
        assert_eq!(recent[1], PathBuf::from("/workspace/shared.rs"));
        assert_eq!(recent.len(), RECENT_FILE_LIMIT);
        assert_eq!(recent[2], PathBuf::from("/workspace/old-0.rs"));
    }

    #[test]
    fn active_file_is_promoted_only_when_it_matches() {
        let active = PathBuf::from("/workspace/src/active.rs");
        let mut results = ["src/other.rs", "src/active.rs"]
            .into_iter()
            .map(|relative_path| RankedPathMatch {
                path: PathBuf::from("/workspace").join(relative_path),
                relative_path: relative_path.to_string(),
                score: 1.0,
                positions: Vec::new(),
                distance_to_relative_directory: 0,
            })
            .collect::<Vec<_>>();

        promote_active_file(&mut results, Some(&active));
        assert_eq!(results[0].path, active);
        promote_active_file(&mut results, Some(Path::new("/workspace/missing.rs")));
        assert_eq!(results[0].path, active);
    }

    #[gpui_kit::test]
    fn escape_closes_quick_open(cx: &mut TestAppContext) {
        cx.update(gpui_kit::component::init);
        let proxy = crate::proxy_client::ProxyClient::new_for_test(PathBuf::new());
        let (_, cx) = cx.add_window_view(|window, cx| {
            let root = cx.new(|_| QuickOpenTestRoot);
            gpui_kit::component::Root::new(root, window, cx)
        });
        let query_focus = cx.update(move |window, cx| {
            let query = cx.new(|cx| InputState::new(window, cx));
            let query_focus = query.read(cx).focus_handle(cx);
            let escape_interceptor =
                close_dialog_on_escape_when_focused(query_focus.clone(), cx);
            let picker = cx.new(|_| QuickOpen {
                proxy,
                workspace: PathBuf::new(),
                recent_files: Vec::new(),
                active_file: None,
                relative_to: None,
                on_open: Box::new(|_, _, _| {}),
                query,
                _escape_interceptor: escape_interceptor,
                paths: Some((0, Arc::new(Vec::new()))),
                active_request: None,
                results: Vec::new(),
                selected_index: 0,
                loading_paths: false,
                searching: false,
                error: None,
                search_generation: Arc::new(AtomicU64::new(0)),
                _file_changes: Task::ready(()),
            });
            window.open_dialog(cx, move |dialog, _, _| {
                dialog
                    .title("Quick Open")
                    .close_button(false)
                    .child(picker.clone())
            });
            window.focus(&query_focus, cx);
            query_focus
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|window, cx| window.focus(&query_focus, cx));
        assert!(cx.update(|window, _| query_focus.is_focused(window)));
        assert!(cx.update(|window, cx| window.has_active_dialog(cx)));

        cx.simulate_keystrokes("escape");

        assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
    }

    #[gpui_kit::test]
    fn selected_file_reaches_open_callback(cx: &mut TestAppContext) {
        cx.update(gpui_kit::component::init);
        let proxy = crate::proxy_client::ProxyClient::new_for_test(PathBuf::new());
        let (_, cx) = cx.add_window_view(|window, cx| {
            let root = cx.new(|_| QuickOpenTestRoot);
            gpui_kit::component::Root::new(root, window, cx)
        });
        let opened = Arc::new(Mutex::new(None));
        let opened_for_callback = opened.clone();
        let focus = cx.update(move |window, cx| {
            let query = cx.new(|cx| InputState::new(window, cx));
            let focus = query.read(cx).focus_handle(cx);
            let picker = cx.new(|cx| QuickOpen {
                proxy,
                workspace: PathBuf::from("/workspace"),
                recent_files: Vec::new(),
                active_file: None,
                relative_to: None,
                on_open: Box::new(move |request, _, _| {
                    *opened_for_callback.lock().unwrap() = Some(request);
                }),
                query,
                _escape_interceptor: close_dialog_on_escape_when_focused(
                    focus.clone(),
                    cx,
                ),
                paths: None,
                active_request: None,
                results: vec![RankedPathMatch {
                    path: PathBuf::from("/workspace/main.rs"),
                    relative_path: "main.rs".to_string(),
                    score: 1.0,
                    positions: Vec::new(),
                    distance_to_relative_directory: 0,
                }],
                selected_index: 0,
                loading_paths: false,
                searching: false,
                error: None,
                search_generation: Arc::new(AtomicU64::new(0)),
                _file_changes: Task::ready(()),
            });
            window.open_dialog(cx, {
                let picker = picker.clone();
                move |dialog, _, _| {
                    dialog
                        .title("Quick Open")
                        .on_ok({
                            let picker = picker.clone();
                            move |_, window, cx| {
                                picker.update(cx, |picker, cx| {
                                    picker.open_selected(window, cx)
                                });
                                false
                            }
                        })
                        .child(picker.clone())
                }
            });
            window.focus(&focus, cx);
            focus
        });

        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|window, cx| window.focus(&focus, cx));
        cx.simulate_keystrokes("enter");

        assert_eq!(
            *opened.lock().unwrap(),
            Some(OpenRequest {
                path: "/workspace/main.rs".to_string(),
                permanent: true,
                location: None,
            })
        );
        assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
    }
}
