//! Ross bridge: explorer click-to-open without lifetime fights.
//!
//! The dock window closure hands out `&mut Window` / `&mut Context` with
//! local borrows, so an `Rc<dyn Fn(&str)>` capturing them cannot be
//! `'static`. Instead the explorer stores a pending path plus a dirty flag,
//! and the shell polls it from the render path. The explorer stores the
//! opener id; the shell opens the file where `window`/`cx` are already in
//! scope.
//!
//! Concretely: row click sets `pending_open`; the shell polls it where the
//! window and context are already in scope.

use std::{
    collections::HashMap,
    sync::{LazyLock, Mutex},
};

/// Pending open requests, keyed by explorer entity id.
static PENDING: LazyLock<Mutex<HashMap<usize, OpenRequest>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenRequest {
    pub path: String,
    pub permanent: bool,
}

/// Explorer with EntityId `id` wants `path` opened.
pub fn request_open(id: usize, path: &str) {
    request_open_with_mode(id, path, false);
}

/// Open an explorer path as a real tab instead of a preview tab.
pub fn request_open_permanent(id: usize, path: &str) {
    request_open_with_mode(id, path, true);
}

fn request_open_with_mode(id: usize, path: &str, permanent: bool) {
    PENDING.lock().unwrap_or_else(|e| e.into_inner()).insert(
        id,
        OpenRequest {
            path: path.to_string(),
            permanent,
        },
    );
}

/// Take a pending open for explorer `id`, if any.
pub fn take_open(id: usize) -> Option<OpenRequest> {
    PENDING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&id)
}

#[cfg(test)]
mod tests {
    use super::{request_open, request_open_permanent, take_open};

    #[test]
    fn pending_open_round_trips_per_explorer() {
        request_open(7, "src/main.rs");
        request_open_permanent(9, "other.rs");
        assert_eq!(
            take_open(7),
            Some(super::OpenRequest {
                path: "src/main.rs".into(),
                permanent: false,
            })
        );
        assert_eq!(take_open(7), None);
        assert_eq!(
            take_open(9),
            Some(super::OpenRequest {
                path: "other.rs".into(),
                permanent: true,
            })
        );
    }
}
