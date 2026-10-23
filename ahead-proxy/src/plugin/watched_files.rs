use std::{
    collections::{HashMap, HashSet},
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use ahead_rpc::plugin::PluginId;
use anyhow::{Context, Result, anyhow, ensure};
use globset::{GlobBuilder, GlobMatcher};
use lsp_types::{
    DidChangeWatchedFilesRegistrationOptions, FileChangeType, FileEvent,
    GlobPattern, OneOf, WatchKind,
};
use notify::{
    Event, EventKind,
    event::{ModifyKind, RenameMode},
};
use parking_lot::RwLock;

use crate::watcher::{FileWatcherHandle, WatchToken};

pub(crate) const LSP_FILE_EVENT_TOKEN: WatchToken = WatchToken(5);

pub struct WatchedFileRegistration {
    patterns: Vec<WatchedFilePattern>,
    workspace: Option<(PathBuf, PathBuf)>,
}

struct WatchedFilePattern {
    base: PathBuf,
    uri_base: PathBuf,
    matcher: GlobMatcher,
    kind: WatchKind,
    root: PathBuf,
}

impl WatchedFileRegistration {
    pub fn new(
        workspace: Option<&Path>,
        options: DidChangeWatchedFilesRegistrationOptions,
    ) -> Result<Self> {
        let patterns = options
            .watchers
            .into_iter()
            .map(|watcher| {
                let (base, pattern) = match watcher.glob_pattern {
                    GlobPattern::String(pattern)
                        if Path::new(&pattern).is_absolute() =>
                    {
                        let literal_end = pattern
                            .find(['*', '?', '[', '{'])
                            .unwrap_or(pattern.len());
                        let separator = pattern[..literal_end]
                            .rfind(['/', '\\'])
                            .ok_or_else(|| {
                                anyhow!("absolute watch pattern has no directory")
                            })?;
                        let base = PathBuf::from(if separator == 0 {
                            "/"
                        } else {
                            &pattern[..separator]
                        });
                        (base, pattern[separator + 1..].to_owned())
                    }
                    GlobPattern::String(pattern) => (
                        workspace
                            .ok_or_else(|| {
                                anyhow!("relative file watcher requires a workspace")
                            })?
                            .to_owned(),
                        pattern,
                    ),
                    GlobPattern::Relative(pattern) => {
                        let uri = match pattern.base_uri {
                            OneOf::Left(folder) => folder.uri,
                            OneOf::Right(uri) => uri,
                        };
                        (
                            uri.to_file_path().map_err(|_| {
                                anyhow!("file watcher base must be a file URI")
                            })?,
                            pattern.pattern,
                        )
                    }
                };
                ensure!(
                    !Path::new(&pattern).is_absolute()
                        && !Path::new(&pattern).components().any(
                            |component| matches!(component, Component::ParentDir)
                        ),
                    "file watch pattern must stay within its base directory"
                );
                let uri_base = base;
                let base = normalize_path(&uri_base)?;
                let root = base
                    .ancestors()
                    .find(|path| path.is_dir())
                    .ok_or_else(|| {
                        anyhow!("file watcher has no existing parent directory")
                    })?
                    .to_owned();
                // Never turn an unrooted server glob into a recursive filesystem-root watch.
                ensure!(
                    root.components()
                        .any(|component| matches!(component, Component::Normal(_))),
                    "file watcher cannot recursively watch a filesystem root"
                );
                let matcher = GlobBuilder::new(&pattern)
                    .literal_separator(true)
                    .backslash_escape(false)
                    .build()
                    .context("invalid language-server file watch glob")?
                    .compile_matcher();
                Ok(WatchedFilePattern {
                    base,
                    uri_base,
                    matcher,
                    kind: watcher.kind.unwrap_or(
                        WatchKind::Create | WatchKind::Change | WatchKind::Delete,
                    ),
                    root,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let workspace = workspace
            .map(|workspace| {
                normalize_path(workspace)
                    .map(|canonical| (canonical, workspace.to_owned()))
            })
            .transpose()?;
        Ok(Self {
            patterns,
            workspace,
        })
    }

    fn event(&self, path: &Path, kind: FileChangeType) -> Option<FileEvent> {
        let watch_kind = if kind == FileChangeType::CREATED {
            WatchKind::Create
        } else if kind == FileChangeType::DELETED {
            WatchKind::Delete
        } else {
            WatchKind::Change
        };
        let pattern = self.patterns.iter().find(|pattern| {
            pattern.kind.contains(watch_kind)
                && path
                    .strip_prefix(&pattern.base)
                    .is_ok_and(|relative| pattern.matcher.is_match(relative))
        })?;
        // LSP servers key workspaces by URI spelling, not filesystem identity.
        let uri_path = self
            .workspace
            .as_ref()
            .and_then(|(canonical, original)| {
                path.strip_prefix(canonical)
                    .ok()
                    .map(|relative| original.join(relative))
            })
            .or_else(|| {
                path.strip_prefix(&pattern.base)
                    .ok()
                    .map(|relative| pattern.uri_base.join(relative))
            })?;
        Some(FileEvent {
            uri: url::Url::from_file_path(uri_path).ok()?,
            typ: kind,
        })
    }
}

#[derive(Default)]
pub(super) struct WatchedFiles {
    registrations: Arc<RwLock<HashMap<(PluginId, String), WatchedFileRegistration>>>,
    watcher: Option<FileWatcherHandle>,
    roots: HashSet<PathBuf>,
}

impl WatchedFiles {
    pub fn root_paths(&self) -> Vec<PathBuf> {
        let mut roots = self.roots.iter().cloned().collect::<Vec<_>>();
        roots.sort();
        roots
    }

    pub fn set_watcher(&mut self, watcher: FileWatcherHandle) {
        self.watcher = Some(watcher);
        self.refresh_watches();
    }

    pub fn register(
        &mut self,
        plugin: PluginId,
        id: String,
        registration: WatchedFileRegistration,
    ) {
        self.registrations
            .write()
            .insert((plugin, id), registration);
        self.refresh_watches();
    }

    pub fn unregister(&mut self, plugin: PluginId, id: &str) {
        self.registrations.write().remove(&(plugin, id.to_owned()));
        self.refresh_watches();
    }

    pub fn remove_server(&mut self, plugin: PluginId) {
        self.registrations
            .write()
            .retain(|(owner, _), _| *owner != plugin);
        self.refresh_watches();
    }

    pub fn clear(&mut self) {
        self.registrations.write().clear();
        self.refresh_watches();
    }

    fn refresh_watches(&mut self) {
        let Some(watcher) = &self.watcher else { return };
        let mut candidates = self
            .registrations
            .read()
            .values()
            .flat_map(|registration| {
                registration
                    .patterns
                    .iter()
                    .map(|pattern| pattern.root.clone())
            })
            .collect::<Vec<_>>();
        candidates.sort_by_key(|path| path.components().count());
        let mut roots: HashSet<PathBuf> = HashSet::new();
        for root in candidates {
            if !roots.iter().any(|ancestor| root.starts_with(ancestor)) {
                roots.insert(root);
            }
        }
        for root in roots.difference(&self.roots) {
            let registrations = self.registrations.clone();
            watcher.watch_filtered(root, true, LSP_FILE_EVENT_TOKEN, move |path| {
                registrations.read().values().any(|registration| {
                    registration.patterns.iter().any(|pattern| {
                        path.strip_prefix(&pattern.base)
                            .is_ok_and(|relative| pattern.matcher.is_match(relative))
                    })
                })
            });
        }
        for root in self.roots.difference(&roots) {
            watcher.unwatch(root, LSP_FILE_EVENT_TOKEN);
        }
        self.roots = roots;
    }

    pub fn changes(&self, event: &Event) -> HashMap<PluginId, Vec<FileEvent>> {
        let mut changes = HashMap::<PluginId, Vec<FileEvent>>::new();
        let registrations = self.registrations.read();
        if registrations.is_empty() {
            return changes;
        }
        for (path, kind) in file_changes(event) {
            let path = match normalize_path(path) {
                Ok(path) => path,
                Err(error) => {
                    tracing::warn!(?error, path = %path.display(), "normalizing language-server file event");
                    continue;
                }
            };
            for ((plugin, _), registration) in &*registrations {
                if let Some(change) = registration.event(&path, kind) {
                    let server_changes = changes.entry(*plugin).or_default();
                    if !server_changes.contains(&change) {
                        server_changes.push(change);
                    }
                }
            }
        }
        changes
    }
}

impl Drop for WatchedFiles {
    fn drop(&mut self) {
        self.clear();
    }
}

fn normalize_path(path: &Path) -> Result<PathBuf> {
    // Deleted paths cannot be canonicalized; resolve their nearest surviving ancestor.
    for ancestor in path.ancestors() {
        if let Ok(base) = ancestor.canonicalize() {
            return Ok(base.join(path.strip_prefix(ancestor)?));
        }
    }
    Err(anyhow!(
        "file watch path has no accessible ancestor: {}",
        path.display()
    ))
}

fn file_changes(event: &Event) -> Vec<(&Path, FileChangeType)> {
    match event.kind {
        EventKind::Modify(ModifyKind::Name(RenameMode::Both)) => {
            match event.paths.as_slice() {
                [from, to] => vec![
                    (from, FileChangeType::DELETED),
                    (to, FileChangeType::CREATED),
                ],
                _ => Vec::new(),
            }
        }
        EventKind::Create(_)
        | EventKind::Modify(ModifyKind::Name(RenameMode::To)) => event
            .paths
            .iter()
            .map(|path| (path.as_path(), FileChangeType::CREATED))
            .collect(),
        EventKind::Remove(_)
        | EventKind::Modify(ModifyKind::Name(RenameMode::From)) => event
            .paths
            .iter()
            .map(|path| (path.as_path(), FileChangeType::DELETED))
            .collect(),
        EventKind::Modify(_) => event
            .paths
            .iter()
            .map(|path| (path.as_path(), FileChangeType::CHANGED))
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::watcher::{FileWatcher, Notify};
    use serde_json::json;
    use std::time::{Duration, Instant};

    fn registration(
        workspace: &Path,
        watchers: serde_json::Value,
    ) -> WatchedFileRegistration {
        WatchedFileRegistration::new(
            Some(workspace),
            serde_json::from_value(json!({ "watchers": watchers }))
                .expect("watch options"),
        )
        .expect("valid registration")
    }

    #[test]
    fn file_watchers_preserve_owners_globs_kinds_and_registration_lifetimes() {
        let project = tempfile::tempdir().expect("project");
        let workspace = project.path().canonicalize().expect("workspace");
        let nested = workspace.join("nested");
        std::fs::create_dir(&nested).expect("nested base");
        let mut watched = WatchedFiles::default();
        for id in ["first", "duplicate"] {
            watched.register(
                PluginId(1),
                id.into(),
                registration(
                    &workspace,
                    json!([
                        { "globPattern": "**/{pyproject,ruff}.toml", "kind": 2 },
                    ]),
                ),
            );
        }
        watched.register(PluginId(2), "first".into(), registration(&workspace, json!([
            { "globPattern": {
                "baseUri": { "uri": url::Url::from_directory_path(&nested).expect("folder URI"), "name": "nested" },
                "pattern": "*.toml"
            }, "kind": 1 }
        ])));
        let path = nested.join("ruff.toml");
        let changed =
            Event::new(EventKind::Modify(ModifyKind::Any)).add_path(path.clone());
        let changes = watched.changes(&changed);
        assert_eq!(changes.len(), 1);
        assert_eq!(
            changes[&PluginId(1)].len(),
            1,
            "overlapping registrations must not duplicate a change"
        );
        assert_eq!(changes[&PluginId(1)][0].typ, FileChangeType::CHANGED);
        let created = Event::new(EventKind::Create(notify::event::CreateKind::File))
            .add_path(path);
        assert_eq!(
            watched
                .changes(&created)
                .keys()
                .copied()
                .collect::<Vec<_>>(),
            vec![PluginId(2)]
        );
        assert!(
            watched
                .changes(
                    &Event::new(EventKind::Modify(ModifyKind::Any))
                        .add_path(workspace.join("README.md"))
                )
                .is_empty()
        );
        watched.unregister(PluginId(1), "first");
        assert_eq!(watched.changes(&changed)[&PluginId(1)].len(), 1);
        watched.remove_server(PluginId(1));
        assert!(watched.changes(&changed).is_empty());
        assert!(
            watched.changes(&created).contains_key(&PluginId(2)),
            "stopping one owner must preserve another's registration with the same ID"
        );
    }

    #[test]
    fn file_watchers_translate_renames_and_match_absolute_and_relative_uri_patterns()
    {
        let project = tempfile::tempdir().expect("project");
        let workspace = project.path().canonicalize().expect("workspace");
        let mut watched = WatchedFiles::default();
        watched.register(PluginId(1), "relative".into(), registration(&workspace, json!([
            { "globPattern": { "baseUri": url::Url::from_directory_path(&workspace).expect("URI"), "pattern": "**/*.toml" } }
        ])));
        watched.register(
            PluginId(2),
            "absolute".into(),
            registration(
                &workspace,
                json!([
                    { "globPattern": workspace.join("**/*.toml").to_string_lossy() }
                ]),
            ),
        );
        let old = workspace.join("old.toml");
        let new = workspace.join("new.toml");
        let rename =
            Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::Both)))
                .add_path(old.clone())
                .add_path(new.clone());
        for changes in watched.changes(&rename).values() {
            assert_eq!(
                changes,
                &vec![
                    FileEvent {
                        uri: url::Url::from_file_path(&old).expect("old URI"),
                        typ: FileChangeType::DELETED
                    },
                    FileEvent {
                        uri: url::Url::from_file_path(&new).expect("new URI"),
                        typ: FileChangeType::CREATED
                    },
                ]
            );
        }
        assert_eq!(watched.changes(&rename).len(), 2);
        for (mode, kind) in [
            (RenameMode::From, FileChangeType::DELETED),
            (RenameMode::To, FileChangeType::CREATED),
        ] {
            let event = Event::new(EventKind::Modify(ModifyKind::Name(mode)))
                .add_path(new.clone());
            assert_eq!(watched.changes(&event)[&PluginId(1)][0].typ, kind);
        }
        assert!(
            watched
                .changes(
                    &Event::new(EventKind::Access(notify::event::AccessKind::Any))
                        .add_path(new)
                )
                .is_empty()
        );
    }

    #[test]
    #[cfg(unix)]
    fn file_watchers_preserve_workspace_and_external_base_uri_aliases() {
        use std::os::unix::fs::symlink;
        let project = tempfile::tempdir().expect("project");
        let real = project.path().join("real");
        std::fs::create_dir(&real).expect("real directory");
        let alias = project.path().join("alias");
        symlink(&real, &alias).expect("workspace alias");
        let file = real.join("ruff.toml");
        std::fs::write(&file, "line-length = 88\n").expect("config");
        let native = file.canonicalize().expect("native event path");
        let mut watched = WatchedFiles::default();
        watched.register(
            PluginId(1),
            "workspace".into(),
            registration(
                &alias,
                json!([
                    { "globPattern": "**/ruff.toml" }
                ]),
            ),
        );
        watched.register(PluginId(2), "external".into(), registration(project.path(), json!([
            { "globPattern": { "baseUri": url::Url::from_directory_path(&alias).expect("alias URI"), "pattern": "*.toml" } }
        ])));
        let changed =
            Event::new(EventKind::Modify(ModifyKind::Any)).add_path(native.clone());
        assert_eq!(
            watched.changes(&changed)[&PluginId(1)][0].uri,
            url::Url::from_file_path(alias.join("ruff.toml"))
                .expect("workspace URI spelling")
        );
        // Outside the workspace, use the server's explicit relative-pattern base URI.
        let external_registration = WatchedFileRegistration::new(None, serde_json::from_value(json!({ "watchers": [
            { "globPattern": { "baseUri": url::Url::from_directory_path(&alias).expect("URI"), "pattern": "*.toml" } }
        ] })).expect("options")).expect("external registration");
        watched.register(PluginId(3), "external".into(), external_registration);
        assert_eq!(
            watched.changes(&changed)[&PluginId(3)][0].uri,
            url::Url::from_file_path(alias.join("ruff.toml"))
                .expect("external URI spelling")
        );
        std::fs::remove_file(file).expect("delete config");
        let removed = Event::new(EventKind::Remove(notify::event::RemoveKind::File))
            .add_path(native);
        assert_eq!(
            watched.changes(&removed)[&PluginId(1)][0].uri,
            url::Url::from_file_path(alias.join("ruff.toml"))
                .expect("deleted URI spelling")
        );
    }

    #[test]
    fn file_watchers_reject_invalid_globs_nonfile_bases_and_root_watches() {
        let project = tempfile::tempdir().expect("project");
        for glob in [
            json!("[invalid"),
            json!("../outside.toml"),
            json!("/**/*.toml"),
            json!({ "baseUri": "https://example.com/", "pattern": "*.toml" }),
        ] {
            let options = serde_json::from_value(
                json!({ "watchers": [{ "globPattern": glob }] }),
            )
            .expect("options");
            assert!(
                WatchedFileRegistration::new(Some(project.path()), options).is_err()
            );
        }
        let options = serde_json::from_value(
            json!({ "watchers": [{ "globPattern": "**/*.toml" }] }),
        )
        .expect("options");
        assert!(WatchedFileRegistration::new(None, options).is_err());
    }

    struct EventPeer(crossbeam_channel::Sender<Event>);

    impl Notify for EventPeer {
        fn notify(&self, events: Vec<(WatchToken, Event)>) {
            for (token, event) in events {
                if token == LSP_FILE_EVENT_TOKEN
                    && let Err(error) = self.0.send(event)
                {
                    tracing::error!(?error, "delivering test watch event");
                }
            }
        }
    }

    #[test]
    fn file_watchers_share_native_external_roots_and_release_them_by_owner() {
        let project = tempfile::tempdir().expect("project");
        let external = tempfile::tempdir().expect("external configuration");
        let base = external.path().canonicalize().expect("external base");
        let nested = base.join("nested");
        std::fs::create_dir(&nested).expect("nested external directory");
        let file = nested.join("ruff.toml");
        std::fs::write(&file, "line-length = 88\n").expect("configuration");
        let mut watcher = FileWatcher::new();
        watcher.watch(&base, false, WatchToken(17));
        let (sender, receiver) = crossbeam_channel::unbounded();
        watcher.notify(EventPeer(sender));
        let mut watched = WatchedFiles::default();
        watched.set_watcher(watcher.handle());
        for plugin in [PluginId(1), PluginId(2)] {
            watched.register(plugin, "config".into(), registration(project.path(), json!([
                { "globPattern": { "baseUri": url::Url::from_directory_path(&base).expect("URI"), "pattern": "**/*.toml" } }
            ])));
        }
        assert_eq!(watched.roots, HashSet::from([base]));
        watched.remove_server(PluginId(1));
        std::fs::write(nested.join("unrelated.rs"), "fn main() {}\n")
            .expect("unmatched build input");
        assert!(
            receiver.recv_timeout(Duration::from_millis(200)).is_err(),
            "unmatched files must be filtered before catalog delivery"
        );
        std::fs::write(&file, "line-length = 100\n").expect("updated configuration");
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let event = receiver
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("native configuration event");
            let changes = watched.changes(&event);
            if let Some(changes) = changes.get(&PluginId(2)) {
                assert!(
                    changes.iter().any(|change| change
                        .uri
                        .to_file_path()
                        .ok()
                        .as_deref()
                        == Some(file.as_path()))
                );
                assert!(!watched.changes(&event).contains_key(&PluginId(1)));
                break;
            }
        }
        watched.unregister(PluginId(2), "config");
        assert!(watched.roots.is_empty());
        assert!(watched.registrations.read().is_empty());
    }
}
