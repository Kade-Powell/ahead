//! Editor-context bridge shared by Floem and GPUI shells.
//!
//! Both shells must produce identical governed-turn context
//! (workspace-relative path, UTF-16 caret line/col, selection, content)
//! from different buffer primitives. This module owns the conversions:
//! - Floem shell path: UTF-8 byte offset → UTF-16 line/col via
//!   `lapce_core::rope_text_pos::RopeTextPosition` (implemented in
//!   `lapce-app`, which owns the Floem dependency).
//! - GPUI path: ropey byte offset → `Point{row,column}` via
//!   `offset_to_point`, then column mapped the same UTF-16 way the host
//!   already uses for display positions.
//! - Shared: line/col pair → `DisplayPosition`, selection range assembly,
//!   workspace-relative path reduction, empty-context fallback.

use lapce_rpc::ahead::{DisplayPosition, DisplayRange};

/// Caret/selection in shell-neutral line/col (UTF-16 col, matching the
/// host's display-position contract).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditorCursor {
    pub line: u32,
    pub col: u32,
}

/// Builds a display position from a line/col pair.
pub fn caret_to_display(line: u32, col: u32) -> DisplayPosition {
    DisplayPosition { line, col }
}

/// Converts a UTF-8 byte offset into a UTF-16 line/col pair using a
/// caller-supplied line/col resolver. Both shells pass their own rope's
/// `offset_to_line_col`; the UTF-8→UTF-16 column fold lives here once.
pub fn offset_to_display(
    offset: usize,
    line_col: impl FnOnce(usize) -> (usize, usize),
    utf16_col: impl FnOnce(usize) -> usize,
) -> DisplayPosition {
    let (line, _utf8_col) = line_col(offset);
    DisplayPosition {
        line: u32::try_from(line).unwrap_or(u32::MAX),
        col: u32::try_from(utf16_col(offset)).unwrap_or(u32::MAX),
    }
}

/// Assembles an optional selection range from anchor + head positions.
pub fn selection_range(
    anchor: Option<DisplayPosition>,
    head: DisplayPosition,
) -> Option<DisplayRange> {
    anchor.map(|start| DisplayRange { start, end: head })
}

/// Reduces an absolute path to workspace-relative when possible.
pub fn relative_path(workspace_root: &str, absolute: &str) -> String {
    if workspace_root.is_empty() {
        return absolute.to_string();
    }
    let root = workspace_root.trim_end_matches('/');
    match absolute.strip_prefix(root) {
        Some(rest) => rest.trim_start_matches('/').to_string(),
        None => absolute.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caret_passthrough() {
        assert_eq!(caret_to_display(10, 4), DisplayPosition { line: 10, col: 4 });
    }

    #[test]
    fn offset_delegates_to_shell_resolvers() {
        let pos = offset_to_display(7, |_| (3, 7), |_| 5);
        assert_eq!(pos, DisplayPosition { line: 3, col: 5 });
    }

    #[test]
    fn selection_needs_anchor() {
        let head = caret_to_display(1, 2);
        assert!(selection_range(None, head).is_none());
        let range = selection_range(Some(caret_to_display(0, 0)), head).unwrap();
        assert_eq!(range.end, head);
    }

    #[test]
    fn relative_reduction() {
        assert_eq!(relative_path("/ws", "/ws/src/a.rs"), "src/a.rs");
        assert_eq!(relative_path("/ws", "/other/a.rs"), "/other/a.rs");
        assert_eq!(relative_path("", "/ws/a.rs"), "/ws/a.rs");
    }
}
