use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{Arc, Weak},
};

use crossbeam_channel::{Receiver, unbounded};
use notify::{
    Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher,
    event::{ModifyKind, RenameMode},
    recommended_watcher,
};
use parking_lot::Mutex;

/// Wrapper around a `notify::Watcher`. It runs the inner watcher
/// in a separate thread, and communicates with it via a [crossbeam channel].
/// [crossbeam channel]: https://docs.rs/crossbeam-channel
pub struct FileWatcher {
    rx_event: Option<Receiver<Result<Event, notify::Error>>>,
    inner: Arc<Mutex<RecommendedWatcher>>,
    state: Arc<Mutex<WatcherState>>,
}

// Weak references prevent the notification thread from keeping its own watcher alive.
#[derive(Clone)]
pub struct FileWatcherHandle {
    inner: Weak<Mutex<RecommendedWatcher>>,
    state: Weak<Mutex<WatcherState>>,
}

#[derive(Debug, Default)]
struct WatcherState {
    events: EventQueue,
    watchees: Vec<Watchee>,
}

/// Tracks a registered 'that-which-is-watched'.
#[doc(hidden)]
struct Watchee {
    path: PathBuf,
    recursive: bool,
    token: WatchToken,
    filter: Option<Box<PathFilter>>,
}

/// Token provided to `FileWatcher`, to associate events with
/// interested parties.
///
/// Note: `WatchToken`s are assumed to correspond with an
/// 'area of interest'; that is, they are used to route delivery
/// of events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WatchToken(pub usize);

/// A trait for types which can be notified of new events.
/// New events are accessible through the `FileWatcher` instance.
pub trait Notify: Send {
    fn notify(&self, events: Vec<(WatchToken, Event)>);
}

pub type EventQueue = VecDeque<(WatchToken, Event)>;

pub type PathFilter = dyn Fn(&Path) -> bool + Send + 'static;

impl FileWatcher {
    pub fn new() -> Self {
        let (tx_event, rx_event) = unbounded();

        let state = Arc::new(Mutex::new(WatcherState::default()));

        let inner = recommended_watcher(tx_event).expect("watcher should spawn");

        FileWatcher {
            rx_event: Some(rx_event),
            inner: Arc::new(Mutex::new(inner)),
            state,
        }
    }

    pub fn handle(&self) -> FileWatcherHandle {
        FileWatcherHandle {
            inner: Arc::downgrade(&self.inner),
            state: Arc::downgrade(&self.state),
        }
    }

    pub fn notify<T: Notify + 'static>(&mut self, peer: T) {
        let rx_event = self.rx_event.take().unwrap();
        let state = self.state.clone();
        std::thread::spawn(move || {
            while let Ok(event) = rx_event.recv() {
                let event = match event {
                    Ok(event) => event,
                    Err(error) => {
                        tracing::error!(?error, "receiving filesystem watch event");
                        continue;
                    }
                };
                let mut events = Vec::new();
                {
                    let mut state = state.lock();
                    let WatcherState {
                        ref mut watchees, ..
                    } = *state;

                    watchees
                        .iter()
                        .filter(|w| w.wants_event(&event))
                        .map(|w| w.token)
                        .for_each(|t| events.push((t, event.clone())));
                }

                peer.notify(events);
            }
        });
    }

    /// Begin watching `path`. As `Event`s (documented in the
    /// [notify](https://docs.rs/notify) crate) arrive, they are stored
    /// with the associated `token` and a task is added to the runloop's
    /// idle queue.
    ///
    /// Delivery of events then requires that the runloop's handler
    /// correctly forward the `handle_idle` call to the interested party.
    pub fn watch(&mut self, path: &Path, recursive: bool, token: WatchToken) {
        watch_impl(&self.inner, &self.state, path, recursive, token, None);
    }

    /// Like `watch`, but taking a predicate function that filters delivery
    /// of events based on their path.
    pub fn watch_filtered<F>(
        &mut self,
        path: &Path,
        recursive: bool,
        token: WatchToken,
        filter: F,
    ) where
        F: Fn(&Path) -> bool + Send + 'static,
    {
        let filter = Box::new(filter) as Box<PathFilter>;
        watch_impl(
            &self.inner,
            &self.state,
            path,
            recursive,
            token,
            Some(filter),
        );
    }

    /// Removes the provided token/path pair from the watch list.
    /// Does not stop watching this path, if it is associated with
    /// other tokens.
    pub fn unwatch(&mut self, path: &Path, token: WatchToken) {
        unwatch_impl(&self.inner, &self.state, path, token);
    }

    /// Takes ownership of this `Watcher`'s current event queue.
    pub fn take_events(&self) -> VecDeque<(WatchToken, Event)> {
        let mut state = self.state.lock();
        let WatcherState { ref mut events, .. } = *state;
        std::mem::take(events)
    }
}

impl FileWatcherHandle {
    pub fn watch_filtered<F>(
        &self,
        path: &Path,
        recursive: bool,
        token: WatchToken,
        filter: F,
    ) where
        F: Fn(&Path) -> bool + Send + 'static,
    {
        let (Some(inner), Some(state)) =
            (self.inner.upgrade(), self.state.upgrade())
        else {
            return;
        };
        let filter = Box::new(filter) as Box<PathFilter>;
        watch_impl(&inner, &state, path, recursive, token, Some(filter));
    }

    pub fn unwatch(&self, path: &Path, token: WatchToken) {
        if let (Some(inner), Some(state)) =
            (self.inner.upgrade(), self.state.upgrade())
        {
            unwatch_impl(&inner, &state, path, token);
        }
    }
}

fn watch_impl(
    inner: &Arc<Mutex<RecommendedWatcher>>,
    shared_state: &Arc<Mutex<WatcherState>>,
    path: &Path,
    recursive: bool,
    token: WatchToken,
    filter: Option<Box<PathFilter>>,
) {
    let path = match path.canonicalize() {
        Ok(path) => path,
        Err(_) => return,
    };

    let mut state = shared_state.lock();
    let watchee = Watchee {
        path,
        recursive,
        token,
        filter,
    };
    let mode = mode_from_bool(watchee.recursive);
    if !state.watchees.iter().any(|existing| {
        existing.path == watchee.path && (existing.recursive || !recursive)
    }) {
        if let Err(error) = inner.lock().watch(&watchee.path, mode) {
            tracing::error!("{:?}", error);
            return;
        }
    }
    state.watchees.push(watchee);
}

fn unwatch_impl(
    inner: &Arc<Mutex<RecommendedWatcher>>,
    shared_state: &Arc<Mutex<WatcherState>>,
    path: &Path,
    token: WatchToken,
) {
    let mut state = shared_state.lock();
    let index = state
        .watchees
        .iter()
        .position(|watchee| watchee.token == token && watchee.path == path);

    if let Some(index) = index {
        let removed = state.watchees.remove(index);
        if !state
            .watchees
            .iter()
            .any(|watchee| watchee.path == removed.path)
        {
            let mut inner = inner.lock();
            if let Err(error) = inner.unwatch(&removed.path) {
                tracing::error!("{:?}", error);
            }
            if removed.recursive {
                let to_add = state
                    .watchees
                    .iter()
                    .filter(|watchee| watchee.path.starts_with(&removed.path))
                    .map(|watchee| {
                        (watchee.path.to_owned(), mode_from_bool(watchee.recursive))
                    })
                    .collect::<Vec<_>>();

                for (path, mode) in to_add {
                    if let Err(error) = inner.watch(&path, mode) {
                        tracing::error!("{:?}", error);
                    }
                }
            }
        } else if removed.recursive
            && !state
                .watchees
                .iter()
                .any(|watchee| watchee.path == removed.path && watchee.recursive)
        {
            if let Err(error) = inner
                .lock()
                .watch(&removed.path, RecursiveMode::NonRecursive)
            {
                tracing::error!(?error, "restoring nonrecursive filesystem watch");
            }
        }
    }
}

impl Default for FileWatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl Watchee {
    fn wants_event(&self, event: &Event) -> bool {
        match &event.kind {
            EventKind::Modify(ModifyKind::Name(RenameMode::Both)) => {
                if event.paths.len() == 2 {
                    //There will be two paths. First is "from" and other is "to".
                    self.applies_to_path(&event.paths[0])
                        || self.applies_to_path(&event.paths[1])
                } else {
                    false
                }
            }
            EventKind::Create(_) | EventKind::Remove(_) | EventKind::Modify(_)
                if event.paths.len() == 1 =>
            {
                self.applies_to_path(&event.paths[0])
            }
            _ => false,
        }
    }

    fn applies_to_path(&self, path: &Path) -> bool {
        let normalized_path;
        let path = if self.filter.is_some() && !path.starts_with(&self.path) {
            let (Some(parent), Some(file_name)) = (path.parent(), path.file_name())
            else {
                return false;
            };
            let Ok(parent) = parent.canonicalize() else {
                return false;
            };
            // Some filesystem backends report the caller's symlinked path spelling.
            normalized_path = parent.join(file_name);
            normalized_path.as_path()
        } else {
            path
        };
        let general_case = if path.starts_with(&self.path) {
            (self.recursive || self.path == path)
                || path.parent() == Some(self.path.as_path())
        } else {
            false
        };

        if let Some(ref filter) = self.filter {
            general_case && filter(path)
        } else {
            general_case
        }
    }
}
impl std::fmt::Debug for Watchee {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "Watchee path: {:?}, r {}, t {} f {}",
            self.path,
            self.recursive,
            self.token.0,
            self.filter.is_some()
        )
    }
}

fn mode_from_bool(is_recursive: bool) -> RecursiveMode {
    if is_recursive {
        RecursiveMode::Recursive
    } else {
        RecursiveMode::NonRecursive
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam_channel::unbounded;

    struct HandlePeer {
        _handle: FileWatcherHandle,
    }

    impl Notify for HandlePeer {
        fn notify(&self, _: Vec<(WatchToken, Event)>) {}
    }

    struct EventPeer(crossbeam_channel::Sender<Vec<(WatchToken, Event)>>);

    impl Notify for EventPeer {
        fn notify(&self, events: Vec<(WatchToken, Event)>) {
            self.0.send(events).expect("event receiver remains alive");
        }
    }

    #[test]
    fn watch_errors_do_not_stop_later_event_delivery() {
        let directory = tempfile::tempdir().expect("watch directory");
        let mut watcher = FileWatcher::new();
        let (events, receiver) = unbounded();
        watcher.rx_event = Some(receiver);
        let (sender, received) = unbounded();
        let token = WatchToken(23);
        watcher.state.lock().watchees.push(Watchee {
            path: directory.path().to_owned(),
            recursive: true,
            token,
            filter: None,
        });
        watcher.notify(EventPeer(sender));
        events
            .send(Err(notify::Error::generic("transient backend error")))
            .expect("send error");
        let event = Event::new(EventKind::Modify(ModifyKind::Any))
            .add_path(directory.path().join("ruff.toml"));
        events.send(Ok(event.clone())).expect("send event");
        let observed = received
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("event after backend error");
        assert_eq!(observed.len(), 1);
        assert_eq!(observed[0].0, token);
        assert_eq!(observed[0].1, event);
    }

    #[test]
    fn notification_peer_handle_does_not_keep_watcher_alive() {
        let mut watcher = FileWatcher::new();
        let handle = watcher.handle();
        watcher.notify(HandlePeer {
            _handle: handle.clone(),
        });

        drop(watcher);

        assert!(handle.inner.upgrade().is_none());
    }

    #[test]
    #[cfg(unix)]
    fn filtered_watch_accepts_paths_reported_through_symlinked_parent() {
        use std::{fs, os::unix::fs::symlink};

        let directory = tempfile::tempdir().expect("temporary directory");
        let watched_directory = directory.path().join("watched");
        fs::create_dir(&watched_directory).expect("create watched directory");
        let path_alias = directory.path().join("alias");
        symlink(&watched_directory, &path_alias).expect("create directory symlink");

        let canonical_directory =
            watched_directory.canonicalize().expect("canonical path");
        let expected_path = canonical_directory.join(".ahead");
        let filter_path = expected_path.clone();
        let watchee = Watchee {
            path: canonical_directory,
            recursive: false,
            token: WatchToken(11),
            filter: Some(Box::new(move |path| path == filter_path)),
        };
        let aliased_path = path_alias.join(".ahead");
        let event = Event::new(EventKind::Create(notify::event::CreateKind::Folder))
            .add_path(aliased_path);

        assert!(watchee.wants_event(&event));
    }

    #[test]
    fn filtered_nonrecursive_watch_delivers_a_native_file_event() {
        use std::time::{Duration, Instant};

        let directory = tempfile::tempdir().expect("temporary directory");
        let file = directory.path().join(".ahead");
        let (sender, receiver) = unbounded();
        let mut watcher = FileWatcher::new();
        let token = WatchToken(17);
        watcher.notify(EventPeer(sender));
        watcher.watch_filtered(directory.path(), false, token, |_| true);
        std::fs::write(&file, "settings = true\n").expect("create watched file");
        let expected_path = file.canonicalize().expect("canonical file path");
        let deadline = Instant::now() + Duration::from_secs(5);

        let observed = loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break false;
            }
            let Ok(events) = receiver.recv_timeout(remaining) else {
                break false;
            };
            if events.iter().any(|(event_token, event)| {
                *event_token == token
                    && event.paths.iter().any(|path| path == &expected_path)
            }) {
                break true;
            }
        };

        assert!(observed, "native watcher did not deliver the created file");
    }
}
