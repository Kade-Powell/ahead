//! AHEAD file explorer panel - left sidebar file tree.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::dock::{BasePanel, Panel, PanelControl, PanelEvent};
use gpui_kit::component::list::ListItem;
use gpui_kit::component::menu::PopupMenuItem;
use gpui_kit::component::tree::{tree, TreeItem, TreeState};
use gpui_kit::component::{h_flex, v_flex, ActiveTheme};
use gpui_kit::prelude::*;
use gpui_kit::*;
use gpui_kit_assets::IconName;
use std::{collections::HashSet, io::Write, process::Stdio};

pub struct ExplorerPanel {
    pub focus: FocusHandle,
    pub root: String,
    pub entries: Vec<ExplorerEntry>,
    pub status: SharedString,
    git_badges: std::collections::HashMap<String, GitStatuses>,
    ignored_paths: HashSet<String>,
    pub mailbox_id: usize,
    pub tree_state: Entity<TreeState>,
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
        let mut panel = Self {
            focus: cx.focus_handle(),
            root: root.to_string(),
            entries: Vec::new(),
            status: "Explorer ready".into(),
            git_badges: badges,
            ignored_paths: HashSet::new(),
            mailbox_id: cx.entity_id().as_u64() as usize,
            tree_state: cx.new(|cx| TreeState::new(cx)),
        };
        panel.refresh(cx);
        panel
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let mut entries = Vec::new();
        Self::collect(&std::path::PathBuf::from(&self.root), 0, &mut entries);
        entries.sort_by(|a: &ExplorerEntry, b: &ExplorerEntry| {
            b.is_dir.cmp(&a.is_dir).then_with(|| a.name.cmp(&b.name))
        });
        self.entries = entries;
        self.git_badges = git_badge_map(&self.root);
        let mut tree_paths = HashSet::new();
        let tree_items =
            build_tree_items(std::path::Path::new(&self.root), 0, &mut tree_paths);
        self.ignored_paths =
            git_ignored_paths(std::path::Path::new(&self.root), &tree_paths);
        self.tree_state.update(cx, |state, cx| {
            state.set_items(tree_items, cx);
        });
        self.status = format!("{} entries", self.entries.len()).into();
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
            let children = if is_dir && depth < 2 {
                build_tree_items(&path, depth + 1, tree_paths)
            } else {
                Vec::new()
            };
            let item = TreeItem::new(path_string, name)
                .children(children)
                .expanded(depth == 0);
            Some(item)
        })
        .collect()
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
                let text = text;
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
                                        if entry.is_expanded() {
                                            IconName::FolderOpen
                                        } else {
                                            IconName::Folder
                                        }
                                    } else {
                                        IconName::File
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
                    move |_ix, entry, menu, window, _cx| {
                        let path = entry.item().id.to_string();
                        let is_dir = entry.is_folder();
                        let explorer = explorer.clone();
                        menu.item(PopupMenuItem::new("Open").on_click(
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
                        ))
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
    use super::{record_git_badge, GitStatuses};

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
}
