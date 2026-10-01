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
    pub location: Option<OpenLocation>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenColumn {
    Character(usize),
    Utf8Byte(usize),
    Utf16(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpenLocation {
    pub line: usize,
    pub column: OpenColumn,
    pub end_line: usize,
    pub end_column: OpenColumn,
}

/// Explorer with EntityId `id` wants `path` opened.
pub fn request_open(id: usize, path: &str) {
    request_open_with_mode(id, path, false, None);
}

pub fn request_open_at(id: usize, path: &str, location: OpenLocation) {
    request_open_with_mode(id, path, false, Some(location));
}

/// Open an explorer path as a real tab instead of a preview tab.
pub fn request_open_permanent(id: usize, path: &str) {
    request_open_with_mode(id, path, true, None);
}

pub fn request_open_permanent_at(id: usize, path: &str, location: OpenLocation) {
    request_open_with_mode(id, path, true, Some(location));
}

fn request_open_with_mode(
    id: usize,
    path: &str,
    permanent: bool,
    location: Option<OpenLocation>,
) {
    PENDING.lock().unwrap_or_else(|e| e.into_inner()).insert(
        id,
        OpenRequest {
            path: path.to_string(),
            permanent,
            location,
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
    use super::{
        OpenColumn, OpenLocation, request_open, request_open_at,
        request_open_permanent, request_open_permanent_at, take_open,
    };

    #[test]
    fn pending_open_round_trips_per_explorer() {
        request_open(7, "src/main.rs");
        request_open_permanent(9, "other.rs");
        request_open_permanent_at(
            12,
            "src/lib.rs",
            OpenLocation {
                line: 7,
                column: OpenColumn::Utf8Byte(2),
                end_line: 7,
                end_column: OpenColumn::Utf8Byte(6),
            },
        );
        assert_eq!(
            take_open(7),
            Some(super::OpenRequest {
                path: "src/main.rs".into(),
                permanent: false,
                location: None,
            })
        );
        assert_eq!(take_open(7), None);
        assert_eq!(
            take_open(12),
            Some(super::OpenRequest {
                path: "src/lib.rs".into(),
                permanent: true,
                location: Some(OpenLocation {
                    line: 7,
                    column: OpenColumn::Utf8Byte(2),
                    end_line: 7,
                    end_column: OpenColumn::Utf8Byte(6),
                }),
            })
        );
        assert_eq!(
            take_open(9),
            Some(super::OpenRequest {
                path: "other.rs".into(),
                permanent: true,
                location: None,
            })
        );
    }

    #[test]
    fn search_open_carries_match_location() {
        let location = OpenLocation {
            line: 12,
            column: OpenColumn::Utf8Byte(7),
            end_line: 13,
            end_column: OpenColumn::Utf8Byte(4),
        };
        request_open_at(11, "src/lib.rs", location);
        assert_eq!(
            take_open(11),
            Some(super::OpenRequest {
                path: "src/lib.rs".into(),
                permanent: false,
                location: Some(location),
            })
        );
    }
}
