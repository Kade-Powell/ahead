//! AHEAD file explorer panel - left sidebar file tree.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::dock::{BasePanel, Panel, PanelControl, PanelEvent};
use gpui_kit::component::list::ListItem;
use gpui_kit::component::menu::PopupMenuItem;
use gpui_kit::component::tree::{TreeEvent, TreeItem, TreeState, tree};
use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use gpui_kit_assets::IconName;
use std::{
    collections::{HashMap, HashSet},
    io::Write,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
};

use crate::proxy_client::ProxyClient;

pub struct ExplorerPanel {
    pub focus: FocusHandle,
    pub root: String,
    pub entries: Vec<ExplorerEntry>,
    pub status: SharedString,
    git_badges: std::collections::HashMap<String, GitStatuses>,
    git_badge_task: Option<Task<()>>,
    workspace_change_task: Option<Task<()>>,
    ignored_paths: HashSet<String>,
    pub mailbox_id: usize,
    pub tree_state: Entity<TreeState>,
    _tree_subscription: Subscription,
    trash_handler: Option<std::rc::Rc<dyn Fn(PathBuf, &mut Window, &mut App)>>,
}

#[derive(Clone, Debug)]
pub struct ExplorerEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub depth: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct GitStatuses(u8);

impl GitStatuses {
    const UNTRACKED: u8 = 1 << 0;
    const ADDED: u8 = 1 << 1;
    const MODIFIED: u8 = 1 << 2;
    const DELETED: u8 = 1 << 3;
    const CONFLICT: u8 = 1 << 4;

    fn add_code(&mut self, code: char) {
        self.0 |= match code {
            '?' => Self::UNTRACKED,
            'A' | 'R' | 'C' => Self::ADDED,
            'M' => Self::MODIFIED,
            'D' => Self::DELETED,
            'U' => Self::CONFLICT,
            _ => 0,
        };
    }

    fn contains(self, status: u8) -> bool {
        self.0 & status != 0
    }

    fn display_badge(self) -> Option<char> {
        if self.contains(Self::CONFLICT) {
            Some('!')
        } else if self.contains(Self::UNTRACKED) {
            Some('U')
        } else if self.contains(Self::DELETED) {
            Some('D')
        } else if self.contains(Self::ADDED) {
            Some('A')
        } else if self.contains(Self::MODIFIED) {
            Some('M')
        } else {
            None
        }
    }

    fn colors(self, green: Hsla, yellow: Hsla, red: Hsla) -> Vec<Hsla> {
        let mut colors = Vec::new();
        if self.contains(Self::UNTRACKED) || self.contains(Self::ADDED) {
            colors.push(green);
        }
        if self.contains(Self::MODIFIED) {
            colors.push(yellow);
        }
        if self.contains(Self::DELETED) || self.contains(Self::CONFLICT) {
            colors.push(red);
        }
        colors
    }
}

fn git_badge_map(root: &str) -> std::collections::HashMap<String, GitStatuses> {
    let mut map = std::collections::HashMap::new();
    let root_path = std::path::Path::new(root);
    let out = std::process::Command::new("git")
        .args(["status", "--porcelain", "-uall"])
        .current_dir(root)
        .output();
    let Ok(out) = out else { return map };
    if !out.status.success() {
        return map;
    }
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if line.len() < 4 {
            continue;
        }
        let x = line.as_bytes()[0] as char;
        let y = line.as_bytes()[1] as char;
        let raw_path = line[3..].trim();
        let path = raw_path
            .rsplit_once(" -> ")
            .map(|(_, path)| path)
            .unwrap_or(raw_path)
            .trim_matches('"');
        let mut statuses = GitStatuses::default();
        statuses.add_code(x);
        statuses.add_code(y);
        let path = std::path::Path::new(path);
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            root_path.join(path)
        };
        record_git_badge(&mut map, root_path, &path, statuses);
    }
    map
}

fn record_git_badge(
    badges: &mut std::collections::HashMap<String, GitStatuses>,
    root: &std::path::Path,
    path: &std::path::Path,
    statuses: GitStatuses,
) {
    let mut current = path.to_path_buf();
    loop {
        let key = current.to_string_lossy().into_owned();
        badges
            .entry(key)
            .and_modify(|existing| existing.0 |= statuses.0)
            .or_insert(statuses);

        if current == root || !current.pop() {
            break;
        }
    }
}

impl ExplorerPanel {
    pub fn new(root: &str, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let badges = git_badge_map(root);
        let tree_state = cx.new(|cx| TreeState::new(cx));
        let tree_subscription = cx.subscribe(&tree_state, |_: &mut Self, _, event, cx| {
            if matches!(event, TreeEvent::Expanded(_)) {
                let panel = cx.entity().downgrade();
                cx.defer(move |cx| {
                    if let Err(error) = panel.update(cx, |panel, cx| panel.refresh(cx)) {
                        eprintln!("Explorer panel closed during tree expansion: {error}");
                    }
                });
            }
        });
        let mut panel = Self {
            focus: cx.focus_handle(),
            root: root.to_string(),
            entries: Vec::new(),
            status: "Explorer ready".into(),
            git_badges: badges,
            git_badge_task: None,
            workspace_change_task: None,
            ignored_paths: HashSet::new(),
            mailbox_id: cx.entity_id().as_u64() as usize,
            tree_state,
            _tree_subscription: tree_subscription,
            trash_handler: None,
        };
        panel.refresh(cx);
        panel
    }

    pub fn set_trash_handler(
        &mut self,
        handler: impl Fn(PathBuf, &mut Window, &mut App) + 'static,
    ) {
        self.trash_handler = Some(std::rc::Rc::new(handler));
    }

    pub fn watch_workspace_changes(
        &mut self,
        proxy: Arc<ProxyClient>,
        cx: &mut Context<Self>,
    ) {
        let updates = proxy.subscribe_workspace_file_changes();
        let mut generation = proxy.workspace_file_generation();
        self.workspace_change_task = Some(cx.spawn(async move |this, cx| {
            while updates.recv().await.is_ok() {
                let current = proxy.workspace_file_generation();
                if current == generation {
                    continue;
                }
                generation = current;
                if this.update(cx, |panel, cx| panel.refresh(cx)).is_err() {
                    break;
                }
            }
        }));
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.git_badge_task = None;
        let (expanded, selected) = tree_state_snapshot(self.tree_state.read(cx));
        let mut entries = Vec::new();
        Self::collect(&std::path::PathBuf::from(&self.root), 0, &mut entries);
        entries.sort_by(|a: &ExplorerEntry, b: &ExplorerEntry| {
            b.is_dir.cmp(&a.is_dir).then_with(|| a.name.cmp(&b.name))
        });
        self.entries = entries;
        self.git_badges = git_badge_map(&self.root);
        let mut tree_paths = HashSet::new();
        let tree_items = build_tree_items(
            std::path::Path::new(&self.root),
            0,
            &mut tree_paths,
            None,
            Some(&expanded),
        );
        self.ignored_paths =
            git_ignored_paths(std::path::Path::new(&self.root), &tree_paths);
        self.tree_state.update(cx, |state, cx| {
            state.set_items(tree_items, cx);
            if let Some(selected) = selected {
                let index = state.index_of(&selected);
                state.set_selected_index(index, cx);
            }
        });
        self.status = format!("{} entries", self.entries.len()).into();
        cx.notify();
    }

    pub fn refresh_git_badges(&mut self, cx: &mut Context<Self>) {
        let root = self.root.clone();
        self.git_badge_task = Some(cx.spawn(async move |this, cx| {
            let badges = cx
                .background_spawn(async move { git_badge_map(&root) })
                .await;
            if let Err(error) = this.update(cx, |panel, cx| {
                panel.git_badges = badges;
                cx.notify();
            }) {
                eprintln!("Explorer panel closed during Git refresh: {error}");
            }
        }));
    }

    fn set_all_expanded(&mut self, expanded: bool, cx: &mut Context<Self>) {
        let mut tree_paths = HashSet::new();
        let tree_items = build_tree_items(
            Path::new(&self.root),
            0,
            &mut tree_paths,
            Some(expanded),
            None,
        );
        self.tree_state.update(cx, |state, cx| {
            state.set_items(tree_items, cx);
        });
        cx.notify();
    }

    #[allow(clippy::only_used_in_recursion)]
    fn collect(dir: &std::path::Path, depth: usize, out: &mut Vec<ExplorerEntry>) {
        let Ok(read) = std::fs::read_dir(dir) else {
            return;
        };
        let mut names: Vec<std::path::PathBuf> =
            read.filter_map(|e| e.ok().map(|e| e.path())).collect();
        names.sort();
        for path in names.into_iter().take(400) {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("?")
                .to_string();
            if name == ".git" {
                continue;
            }
            let is_dir = path.is_dir();
            out.push(ExplorerEntry {
                name,
                path: path.to_string_lossy().to_string(),
                is_dir,
                depth,
            });
            if is_dir && depth < 2 {
                Self::collect(&path, depth + 1, out);
            }
        }
    }
}

fn build_tree_items(
    dir: &std::path::Path,
    depth: usize,
    tree_paths: &mut HashSet<String>,
    expansion: Option<bool>,
    expanded_paths: Option<&HashMap<String, bool>>,
) -> Vec<TreeItem> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut entries: Vec<std::path::PathBuf> = read
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect();
    entries.sort();
    entries
        .into_iter()
        .take(400)
        .filter_map(|path| {
            let name = path.file_name()?.to_str()?.to_string();
            if name == ".git" {
                return None;
            }
            let path_string = path.to_string_lossy().into_owned();
            tree_paths.insert(path_string.clone());
            let is_dir = path.is_dir();
            let expanded = expansion
                .or_else(|| {
                    expanded_paths.and_then(|paths| paths.get(&path_string).copied())
                })
                .unwrap_or(depth == 0)
                && !(expansion == Some(true) && depth >= 2);
            let mut children = if is_dir && (depth < 2 || expanded) {
                build_tree_items(
                    &path,
                    depth + 1,
                    tree_paths,
                    expansion,
                    expanded_paths,
                )
            } else {
                Vec::new()
            };
            if is_dir && children.is_empty() {
                children.push(
                    TreeItem::new(format!("empty:{path_string}"), "Empty folder")
                        .disabled(true),
                );
            }
            let item = TreeItem::new(path_string, name)
                .children(children)
                .expanded(expanded);
            Some(item)
        })
        .collect()
}

fn tree_state_snapshot(
    state: &TreeState,
) -> (HashMap<String, bool>, Option<SharedString>) {
    let mut expanded = HashMap::new();
    let mut index = 0;
    while let Some(entry) = state.entry(index) {
        if entry.is_root() {
            record_tree_expansion(entry.item(), &mut expanded);
        }
        index += 1;
    }
    (expanded, state.selected_item().map(|item| item.id.clone()))
}

fn record_tree_expansion(item: &TreeItem, expanded: &mut HashMap<String, bool>) {
    expanded.insert(item.id.to_string(), item.is_expanded());
    for child in &item.children {
        record_tree_expansion(child, expanded);
    }
}

fn git_ignored_paths(
    root: &std::path::Path,
    paths: &HashSet<String>,
) -> HashSet<String> {
    let Ok(mut child) = std::process::Command::new("git")
        .args(["check-ignore", "--stdin", "-z"])
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return HashSet::new();
    };

    let Some(mut stdin) = child.stdin.take() else {
        return HashSet::new();
    };
    for path in paths {
        let Ok(relative) = std::path::Path::new(path).strip_prefix(root) else {
            continue;
        };
        if stdin
            .write_all(relative.to_string_lossy().as_bytes())
            .and_then(|_| stdin.write_all(&[0]))
            .is_err()
        {
            return HashSet::new();
        }
    }
    drop(stdin);

    let Ok(output) = child.wait_with_output() else {
        return HashSet::new();
    };
    output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| {
            root.join(String::from_utf8_lossy(path).as_ref())
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}

fn lerp_color(from: Hsla, to: Hsla, amount: f32) -> Hsla {
    Hsla {
        h: from.h + (to.h - from.h) * amount,
        s: from.s + (to.s - from.s) * amount,
        l: from.l + (to.l - from.l) * amount,
        a: from.a + (to.a - from.a) * amount,
    }
}

fn gradient_color(colors: &[Hsla], amount: f32) -> Hsla {
    if colors.len() < 2 {
        return colors.first().copied().unwrap_or_default();
    }
    let scaled = amount.clamp(0., 1.) * (colors.len() - 1) as f32;
    let index = scaled.floor() as usize;
    let next = (index + 1).min(colors.len() - 1);
    lerp_color(colors[index], colors[next], scaled - index as f32)
}

pub(crate) fn relative_path(root: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(root)
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| path.to_path_buf())
}

pub(crate) fn add_to_gitignore(root: &Path, path: &Path) -> Result<bool, String> {
    let relative = relative_path(root, path);
    if relative.as_os_str().is_empty() || relative == Path::new(".") {
        return Err("The workspace root cannot be added to .gitignore".to_string());
    }
    let mut entry = relative.to_string_lossy().replace('\\', "/");
    if path.is_dir() {
        entry.push('/');
    }
    let ignore_path = root.join(".gitignore");
    let existing = match std::fs::read_to_string(&ignore_path) {
        Ok(existing) => existing,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error.to_string()),
    };
    if existing.lines().any(|line| line.trim() == entry) {
        return Ok(false);
    }
    let mut updated = existing;
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str(&entry);
    updated.push('\n');
    std::fs::write(ignore_path, updated).map_err(|error| error.to_string())?;
    Ok(true)
}

pub(crate) fn duplicate_path(path: &Path) -> Result<PathBuf, String> {
    let parent = path
        .parent()
        .ok_or_else(|| "The file has no parent directory".to_string())?;
    let stem = path
        .file_stem()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "The file name is not valid UTF-8".to_string())?;
    let extension = path.extension().and_then(|value| value.to_str());
    for index in 1..10_000 {
        let name = match extension {
            Some(extension) => format!("{} copy {}.{}", stem, index, extension),
            None => format!("{} copy {}", stem, index),
        };
        let destination = parent.join(name);
        if destination.exists() {
            continue;
        }
        if path.is_dir() {
            copy_directory(path, &destination)?;
        } else {
            std::fs::copy(path, &destination).map_err(|error| error.to_string())?;
        }
        return Ok(destination);
    }
    Err("Could not find an unused duplicate name".to_string())
}

fn copy_directory(source: &Path, destination: &Path) -> Result<(), String> {
    std::fs::create_dir(destination).map_err(|error| error.to_string())?;
    for entry in std::fs::read_dir(source).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if source_path.is_dir() {
            copy_directory(&source_path, &destination_path)?;
        } else {
            std::fs::copy(source_path, destination_path)
                .map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

fn colored_directory_name(
    name: SharedString,
    statuses: GitStatuses,
    fallback: Hsla,
    green: Hsla,
    yellow: Hsla,
    red: Hsla,
) -> impl IntoElement {
    let colors = statuses.colors(green, yellow, red);
    let characters: Vec<char> = name.chars().collect();
    let last_index = characters.len().saturating_sub(1) as f32;
    let mut label = h_flex().gap_0().items_center();
    for (index, character) in characters.into_iter().enumerate() {
        let amount = if last_index == 0. {
            0.
        } else {
            index as f32 / last_index
        };
        label = label.child(
            div()
                .text_size(px(12.))
                .text_color(gradient_color(&colors, amount))
                .when(colors.is_empty(), |label| label.text_color(fallback))
                .child(character.to_string()),
        );
    }
    label
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
    fn render(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let text = cx.theme().sidebar_foreground;
        let green = cx.theme().green;
        let yellow = cx.theme().yellow;
        let red = cx.theme().red;
        v_flex()
            .size_full()
            .p_2()
            .gap_1()
            .track_focus(&self.focus)
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .px_2()
                    .py_1()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_ellipsis_start()
                            .text_size(px(11.))
                            .text_color(muted)
                            .child(self.root.clone()),
                    )
                    .child(
                        Button::new("explorer_refresh")
                            .icon(IconName::RefreshCw)
                            .ghost()
                            .flex_shrink_0()
                            .tooltip("Refresh Files (⌘⇧E)")
                            .on_click(cx.listener(|this: &mut Self, _, _, cx| {
                                this.refresh(cx)
                            })),
                    ),
            )
            .child({
                let explorer = cx.entity();
                let tree_explorer = explorer.clone();
                let badges = self.git_badges.clone();
                let ignored_paths = self.ignored_paths.clone();
                tree(
                    &self.tree_state,
                    move |ix, entry, selected, _window, _cx| {
                        let explorer = tree_explorer.clone();
                        let item = entry.item();
                        let path = item.id.to_string();
                        let is_dir = entry.is_folder();
                        let ignored = ignored_paths.contains(&path);
                        let statuses =
                            badges.get(&path).copied().unwrap_or_default();
                        let badge =
                            (!is_dir).then(|| statuses.display_badge()).flatten();
                        let badge_color = if statuses
                            .contains(GitStatuses::UNTRACKED)
                            || statuses.contains(GitStatuses::ADDED)
                        {
                            green
                        } else if statuses.contains(GitStatuses::DELETED)
                            || statuses.contains(GitStatuses::CONFLICT)
                        {
                            red
                        } else if statuses.contains(GitStatuses::MODIFIED) {
                            yellow
                        } else {
                            muted
                        };
                        let label = if ignored {
                            div()
                                .text_size(px(12.))
                                .text_color(muted)
                                .child(item.label.clone())
                                .into_any_element()
                        } else if is_dir && !entry.is_expanded() {
                            colored_directory_name(
                                item.label.clone(),
                                statuses,
                                text,
                                green,
                                yellow,
                                red,
                            )
                            .into_any_element()
                        } else {
                            div()
                                .text_size(px(12.))
                                .text_color(text)
                                .child(item.label.clone())
                                .into_any_element()
                        };
                        let path_for_click = path.clone();
                        ListItem::new(ix)
                            .selected(selected)
                            .pl(px(12. + entry.depth() as f32 * 16.))
                            .child(
                                h_flex()
                                    .gap_1()
                                    .items_center()
                                    .child(if is_dir {
                                        (if entry.is_expanded() {
                                            IconName::FolderOpen
                                        } else {
                                            IconName::Folder
                                        })
                                        .into_any_element()
                                    } else {
                                        IconName::File.into_any_element()
                                    })
                                    .child(label)
                                    .when(badge.is_some(), |row| {
                                        row.child(
                                            div()
                                                .text_size(px(10.))
                                                .font_weight(
                                                    gpui_kit::FontWeight::BOLD,
                                                )
                                                .text_color(badge_color)
                                                .child(
                                                    badge
                                                        .unwrap_or_default()
                                                        .to_string(),
                                                ),
                                        )
                                    }),
                            )
                            .on_click({
                                let explorer = explorer.clone();
                                move |event, window, cx| {
                                    if !is_dir {
                                        explorer.update(cx, |this, cx| {
                                            if event.click_count() > 1 {
                                                crate::ross::request_open_permanent(
                                                    this.mailbox_id,
                                                    &path_for_click,
                                                );
                                            } else {
                                                crate::ross::request_open(
                                                    this.mailbox_id,
                                                    &path_for_click,
                                                );
                                            }
                                            cx.focus_self(window);
                                        });
                                    }
                                }
                            })
                    },
                )
                .context_menu(
                    move |_ix, entry, menu, window, cx| {
                        let path = entry.item().id.to_string();
                        let is_dir = entry.is_folder();
                        let explorer = explorer.clone();
                        let root = explorer.read(cx).root.clone();
                        let relative = relative_path(Path::new(&root), Path::new(&path))
                            .to_string_lossy()
                            .into_owned();
                        let absolute = path.clone();
                        let reveal_path = path.clone();
                        let ignore_path = path.clone();
                        let duplicate_source = path.clone();
                        let delete_source = path.clone();
                        let mut menu = menu.item(PopupMenuItem::new("Open").on_click(
                            window.listener_for(
                                &explorer,
                                move |this, _, window, cx| {
                                    if !is_dir {
                                        crate::ross::request_open_permanent(
                                            this.mailbox_id,
                                            &path,
                                        );
                                        cx.focus_self(window);
                                    }
                                },
                            ),
                        ));
                        menu = menu
                            .separator()
                            .item(PopupMenuItem::new("Copy Relative Path").on_click(
                                move |_, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        relative.clone(),
                                    ));
                                },
                            ))
                            .item(PopupMenuItem::new("Copy Absolute Path").on_click(
                                move |_, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        absolute.clone(),
                                    ));
                                },
                            ))
                            .item(PopupMenuItem::new("Reveal in File Manager").on_click(
                                move |_, _, cx| {
                                    cx.reveal_path(Path::new(&reveal_path));
                                },
                            ))
                            .item(PopupMenuItem::new("Add to .gitignore").on_click(
                                window.listener_for(
                                    &explorer,
                                    move |this, _, _, cx| {
                                        let status = match add_to_gitignore(
                                            Path::new(&this.root),
                                            Path::new(&ignore_path),
                                        ) {
                                            Ok(true) => "Added to .gitignore".into(),
                                            Ok(false) => "Already in .gitignore".into(),
                                            Err(error) => format!(
                                                "Could not update .gitignore: {error}"
                                            )
                                            .into(),
                                        };
                                        this.status = status;
                                        cx.notify();
                                    },
                                ),
                            ))
                            .item(PopupMenuItem::new("Duplicate").on_click(
                                window.listener_for(
                                    &explorer,
                                    move |this, _, _, cx| {
                                        let status = match duplicate_path(
                                            Path::new(&duplicate_source),
                                        ) {
                                            Ok(destination) => {
                                                format!("Duplicated {}", destination.display())
                                            }
                                            Err(error) => {
                                                format!("Could not duplicate: {error}")
                                            }
                                        };
                                        this.refresh(cx);
                                        this.status = status.into();
                                        cx.notify();
                                    },
                                ),
                            ))
                            .item(PopupMenuItem::new("Move to Trash").on_click(
                                window.listener_for(
                                    &explorer,
                                    move |this, _, window, cx| {
                                        if let Some(handler) = this.trash_handler.clone() {
                                            handler(PathBuf::from(&delete_source), window, cx);
                                        }
                                    },
                                ),
                            ));
                        if is_dir {
                            menu = menu
                                .separator()
                                .item(PopupMenuItem::new("Expand All").on_click(
                                    window.listener_for(&explorer, |this, _, _, cx| {
                                        this.set_all_expanded(true, cx);
                                    }),
                                ))
                                .item(PopupMenuItem::new("Collapse All").on_click(
                                    window.listener_for(&explorer, |this, _, _, cx| {
                                        this.set_all_expanded(false, cx);
                                    }),
                                ));
                        }
                        menu
                    },
                )
            })
            .child(
                div()
                    .pt_1()
                    .text_size(px(10.))
                    .text_color(text)
                    .child(self.status.clone()),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ExplorerPanel, GitStatuses, build_tree_items, record_git_badge,
        relative_path,
    };
    use crate::proxy_client::ProxyClient;
    use ahead_rpc::core::CoreNotification;
    use gpui_kit::TestAppContext;
    use gpui_kit::component::tree::TreeEvent;

    #[gpui_kit::test]
    fn nested_and_empty_directories_remain_expandable(cx: &mut TestAppContext) {
        let workspace = tempfile::tempdir().expect("workspace");
        let assets = workspace.path().join("app/assets");
        let nested = assets.join("images");
        std::fs::create_dir_all(&nested).expect("nested directory");
        std::fs::write(nested.join("logo.svg"), "<svg/>").expect("nested file");
        let empty = assets.join("empty");
        std::fs::create_dir(&empty).expect("empty directory");
        let root = workspace.path().to_string_lossy().into_owned();
        let assets_id = assets.to_string_lossy().into_owned().into();
        let nested_id = nested.to_string_lossy().into_owned().into();
        let empty_id = empty.to_string_lossy().into_owned().into();
        let logo_id = nested
            .join("logo.svg")
            .to_string_lossy()
            .into_owned()
            .into();

        cx.update(gpui_kit::component::init);
        let (panel, cx) =
            cx.add_window_view(|window, cx| ExplorerPanel::new(&root, window, cx));
        panel.update(cx, |panel, cx| {
            panel.tree_state.update(cx, |state, cx| {
                let assets = state
                    .entry(state.index_of(&assets_id).expect("assets entry"))
                    .expect("assets entry");
                assert!(assets.is_folder());
                let item = assets.item().clone().expanded(true);
                cx.emit(TreeEvent::Expanded(item.id));
            });
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            let state = panel.read(cx).tree_state.read(cx);
            let nested = state
                .entry(state.index_of(&nested_id).expect("nested entry"))
                .expect("nested entry");
            assert!(nested.is_folder());
            assert!(
                state
                    .entry(state.index_of(&empty_id).expect("empty entry"))
                    .expect("empty entry")
                    .is_folder()
            );
        });
        panel.update(cx, |panel, cx| {
            panel.tree_state.update(cx, |state, cx| {
                let nested = state
                    .entry(state.index_of(&nested_id).expect("nested entry"))
                    .expect("nested entry");
                let item = nested.item().clone().expanded(true);
                cx.emit(TreeEvent::Expanded(item.id));
            });
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            let state = panel.read(cx).tree_state.read(cx);
            assert!(state.index_of(&logo_id).is_some());
        });
    }

    #[gpui_kit::test]
    fn external_file_set_changes_refresh_tree_without_resetting_state(
        cx: &mut TestAppContext,
    ) {
        let workspace = tempfile::tempdir().expect("workspace");
        let directory = workspace.path().join("src");
        std::fs::create_dir(&directory).expect("src directory");
        std::fs::write(directory.join("old.rs"), "old\n").expect("old file");
        let keep = workspace.path().join("keep.rs");
        std::fs::write(&keep, "keep\n").expect("selected file");
        let root = workspace.path().to_string_lossy().into_owned();
        let src_id: gpui_kit::SharedString =
            directory.to_string_lossy().into_owned().into();
        let keep_id = keep.to_string_lossy().into_owned().into();
        let mut tree_paths = std::collections::HashSet::new();
        let expanded_paths =
            std::collections::HashMap::from([(src_id.to_string(), false)]);
        let tree_items = build_tree_items(
            workspace.path(),
            0,
            &mut tree_paths,
            None,
            Some(&expanded_paths),
        );
        let proxy = ProxyClient::new_for_test(workspace.path().to_owned());

        cx.update(gpui_kit::component::init);
        let (panel, cx) =
            cx.add_window_view(|window, cx| ExplorerPanel::new(&root, window, cx));
        panel.update(cx, |panel, cx| {
            panel.watch_workspace_changes(proxy.clone(), cx);
            panel.tree_state.update(cx, |state, cx| {
                state.set_items(tree_items, cx);
                state.set_selected_index(state.index_of(&keep_id), cx);
            });
        });

        let added = directory.join("new.rs");
        std::fs::write(&added, "new\n").expect("external create");
        proxy.route_core(CoreNotification::WorkspaceFileChange {
            generation: 1,
            paths: Vec::new(),
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            let panel = panel.read(cx);
            let state = panel.tree_state.read(cx);
            let src = state
                .entry(state.index_of(&src_id).expect("src after create"))
                .expect("src entry");
            assert!(!src.is_expanded());
            assert!(
                src.item()
                    .children
                    .iter()
                    .any(|item| item.label.as_ref() == "new.rs")
            );
            assert_eq!(state.selected_item().map(|item| &item.id), Some(&keep_id));
        });

        std::fs::remove_file(&added).expect("external remove");
        proxy.route_core(CoreNotification::WorkspaceFileChange {
            generation: 2,
            paths: Vec::new(),
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            let panel = panel.read(cx);
            let state = panel.tree_state.read(cx);
            let src = state
                .entry(state.index_of(&src_id).expect("src after remove"))
                .expect("src entry");
            assert!(
                !src.item()
                    .children
                    .iter()
                    .any(|item| item.label.as_ref() == "new.rs")
            );
            assert_eq!(state.selected_item().map(|item| &item.id), Some(&keep_id));
        });

        panel.update(cx, |panel, _| panel.status = "No tree rebuild".into());
        proxy.route_core(CoreNotification::WorkspaceFileChange {
            generation: 2,
            paths: Vec::new(),
        });
        cx.run_until_parked();
        assert_eq!(
            cx.update(|_, cx| panel.read(cx).status.to_string()),
            "No tree rebuild"
        );
    }

    #[gpui_kit::test]
    fn git_badges_refresh_after_external_edit(cx: &mut TestAppContext) {
        let workspace = tempfile::tempdir().expect("workspace");
        assert!(
            std::process::Command::new("git")
                .args(["init", "-q"])
                .current_dir(workspace.path())
                .status()
                .expect("git init")
                .success()
        );
        let root = workspace.path().to_string_lossy().into_owned();
        cx.update(gpui_kit::component::init);
        let (panel, cx) =
            cx.add_window_view(|window, cx| ExplorerPanel::new(&root, window, cx));
        let path = workspace
            .path()
            .join("new.rs")
            .to_string_lossy()
            .into_owned();
        assert!(cx.update(|_, cx| !panel.read(cx).git_badges.contains_key(&path)));

        std::fs::write(&path, "new\n").expect("external edit");
        panel.update(cx, |panel, cx| panel.refresh_git_badges(cx));
        cx.run_until_parked();
        assert_eq!(
            cx.update(|_, cx| panel.read(cx).git_badges.get(&path).copied()),
            Some(GitStatuses(GitStatuses::UNTRACKED))
        );
    }

    #[test]
    fn propagates_strongest_badge_to_directory_ancestors() {
        let root = std::path::Path::new("/workspace");
        let mut badges = std::collections::HashMap::new();

        let mut modified = GitStatuses::default();
        modified.add_code('M');
        let mut untracked = GitStatuses::default();
        untracked.add_code('?');
        record_git_badge(&mut badges, root, &root.join("src/lib.rs"), modified);
        record_git_badge(&mut badges, root, &root.join("src/main.rs"), untracked);

        assert_eq!(badges.get("/workspace/src/lib.rs"), Some(&modified));
        let src_statuses = badges.get("/workspace/src").copied().unwrap_or_default();
        assert!(src_statuses.contains(GitStatuses::MODIFIED));
        assert!(src_statuses.contains(GitStatuses::UNTRACKED));
        assert_eq!(badges.get("/workspace"), Some(&src_statuses));
    }

    #[test]
    fn computes_workspace_relative_paths() {
        assert_eq!(
            relative_path(
                std::path::Path::new("/workspace"),
                std::path::Path::new("/workspace/src/lib.rs"),
            ),
            std::path::PathBuf::from("src/lib.rs")
        );
    }
}
