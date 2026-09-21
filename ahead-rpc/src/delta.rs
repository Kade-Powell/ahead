//! Document deltas over [`ropey::Rope`] text.
//!
//! Replaces `lapce-xi-rope`'s `RopeDelta` with a minimal operation list over
//! byte offsets. Byte indexing matches ropey's primary metric; every offset
//! is floored to a character boundary on apply so malformed deltas cannot
//! panic. App and proxy ship as one binary pair, so no wire versioning is
//! needed.

use ropey::Rope;
use serde::{Deserialize, Serialize};

/// A single document edit operation. Counts are bytes in the base (for
/// `Retain`/`Delete`) or new (for `Insert`) text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeltaOp {
    Retain(usize),
    Insert(String),
    Delete(usize),
}

/// An ordered list of [`DeltaOp`] transforming a base document of
/// `base_len` bytes. Constructed by the editor; applied by the proxy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AheadDelta {
    ops: Vec<DeltaOp>,
    base_len: usize,
}

impl AheadDelta {
    pub fn new(base_len: usize, ops: Vec<DeltaOp>) -> Self {
        Self { ops, base_len }
    }

    /// A no-op delta over an empty document.
    pub fn empty() -> Self {
        Self {
            ops: Vec::new(),
            base_len: 0,
        }
    }

    pub fn ops(&self) -> &[DeltaOp] {
        &self.ops
    }

    pub fn base_len(&self) -> usize {
        self.base_len
    }

    /// Length of the document after applying this delta.
    pub fn new_len(&self) -> usize {
        self.base_len
            .saturating_add(self.inserted_len())
            .saturating_sub(self.deleted_len())
    }

    fn inserted_len(&self) -> usize {
        self.ops.iter().fold(0, |acc, op| match op {
            DeltaOp::Insert(text) => acc + text.len(),
            _ => acc,
        })
    }

    fn deleted_len(&self) -> usize {
        self.ops.iter().fold(0, |acc, op| match op {
            DeltaOp::Delete(n) => acc + n,
            _ => acc,
        })
    }

    /// Applies the delta to `base`, returning the new document. Offsets are
    /// clamped to the base length and floored to character boundaries, so a
    /// malformed delta degrades to a partial application instead of a panic.
    pub fn apply(&self, base: &Rope) -> Rope {
        let base_text = base.to_string();
        let base_len = base_text.len();
        let mut out = String::with_capacity(base_len + self.inserted_len());
        let mut cursor: usize = 0;
        for op in &self.ops {
            match op {
                DeltaOp::Retain(n) => {
                    let end = floor_boundary(&base_text, cursor.saturating_add(*n).min(base_len));
                    let start = floor_boundary(&base_text, cursor.min(base_len));
                    if end > start {
                        out.push_str(&base_text[start..end]);
                    }
                    cursor = end;
                }
                DeltaOp::Insert(text) => out.push_str(text),
                DeltaOp::Delete(n) => {
                    cursor = floor_boundary(
                        &base_text,
                        cursor.saturating_add(*n).min(base_len),
                    );
                }
            }
        }
        if cursor < base_len {
            out.push_str(&base_text[cursor..]);
        }
        Rope::from(out)
    }

    /// Inverts the delta against the original document, producing the delta
    /// that restores it. Deleted spans are captured from `base`.
    pub fn invert(&self, base: &Rope) -> AheadDelta {
        let base_text = base.to_string();
        let base_len = base_text.len();
        let mut ops = Vec::with_capacity(self.ops.len());
        let mut cursor: usize = 0;
        for op in &self.ops {
            match op {
                DeltaOp::Retain(n) => {
                    let end =
                        floor_boundary(&base_text, cursor.saturating_add(*n).min(base_len));
                    ops.push(DeltaOp::Retain(end.saturating_sub(cursor)));
                    cursor = end;
                }
                DeltaOp::Insert(text) => ops.push(DeltaOp::Delete(text.len())),
                DeltaOp::Delete(n) => {
                    let end =
                        floor_boundary(&base_text, cursor.saturating_add(*n).min(base_len));
                    ops.push(DeltaOp::Insert(base_text[cursor.min(base_len)..end].to_string()));
                    cursor = end;
                }
            }
        }
        AheadDelta {
            ops,
            base_len: self.new_len(),
        }
    }

    /// Changed range in base coordinates: `(start, end)`. Starts at the
    /// first changed byte; ends where the last deletion ends (a pure
    /// insertion reports an empty range), matching how callers derive
    /// LSP change ranges.
    pub fn summary(&self) -> (usize, usize) {
        let mut cursor: usize = 0;
        let mut start: Option<usize> = None;
        let mut end = 0;
        for op in &self.ops {
            match op {
                DeltaOp::Retain(n) => cursor += n,
                DeltaOp::Insert(_) => {
                    if start.is_none() {
                        start = Some(cursor);
                    }
                    end = end.max(cursor);
                }
                DeltaOp::Delete(n) => {
                    if start.is_none() {
                        start = Some(cursor);
                    }
                    cursor += n;
                    end = cursor;
                }
            }
        }
        match start {
            Some(start) => (start, end.min(self.base_len)),
            None => (self.base_len, self.base_len),
        }
    }

    /// Returns the inserted text when the delta is a single insertion.
    pub fn as_simple_insert(&self) -> Option<&str> {
        if self.ops.len() == 3 {
            if let [DeltaOp::Retain(_), DeltaOp::Insert(text), DeltaOp::Retain(_)] =
                &self.ops[..]
            {
                return Some(text);
            }
        }
        if self.ops.len() == 2 {
            if let [DeltaOp::Retain(_), DeltaOp::Insert(text)] = &self.ops[..] {
                return Some(text);
            }
            if let [DeltaOp::Insert(text), DeltaOp::Retain(_)] = &self.ops[..] {
                return Some(text);
            }
        }
        if let [DeltaOp::Insert(text)] = &self.ops[..] {
            return Some(text);
        }
        None
    }

    /// True when the delta only deletes a single span.
    pub fn is_simple_delete(&self) -> bool {
        let mut seen_delete = false;
        for op in &self.ops {
            match op {
                DeltaOp::Retain(_) => {}
                DeltaOp::Insert(_) => return false,
                DeltaOp::Delete(_) => {
                    if seen_delete {
                        return false;
                    }
                    seen_delete = true;
                }
            }
        }
        seen_delete
    }
}

fn floor_boundary(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rope(text: &str) -> Rope {
        Rope::from(text)
    }

    #[test]
    fn apply_inserts_and_deletes() {
        let base = rope("hello world");
        let delta = AheadDelta::new(
            base.len(),
            vec![
                DeltaOp::Retain(6),
                DeltaOp::Delete(5),
                DeltaOp::Insert("there".to_string()),
            ],
        );
        assert_eq!(delta.apply(&base).to_string(), "hello there");
    }

    #[test]
    fn apply_clamps_instead_of_panicking() {
        let base = rope("hi");
        let delta = AheadDelta::new(
            100,
            vec![DeltaOp::Retain(50), DeltaOp::Insert("!".to_string())],
        );
        assert_eq!(delta.apply(&base).to_string(), "hi!");
    }

    #[test]
    fn invert_restores_the_original() {
        let base = rope("hello world");
        let delta = AheadDelta::new(
            base.len(),
            vec![
                DeltaOp::Retain(6),
                DeltaOp::Delete(5),
                DeltaOp::Insert("there".to_string()),
            ],
        );
        let new = delta.apply(&base);
        let back = delta.invert(&base).apply(&new);
        assert_eq!(back.to_string(), "hello world");
    }

    #[test]
    fn serde_round_trip() {
        let delta = AheadDelta::new(
            11,
            vec![
                DeltaOp::Retain(6),
                DeltaOp::Delete(5),
                DeltaOp::Insert("there".to_string()),
            ],
        );
        let json = serde_json::to_string(&delta).unwrap();
        assert_eq!(serde_json::from_str::<AheadDelta>(&json).unwrap(), delta);
    }

    #[test]
    fn summary_reports_changed_range() {
        let delta = AheadDelta::new(
            11,
            vec![
                DeltaOp::Retain(6),
                DeltaOp::Delete(5),
                DeltaOp::Insert("there".to_string()),
            ],
        );
        assert_eq!(delta.summary(), (6, 11));

        let insert = AheadDelta::new(
            11,
            vec![
                DeltaOp::Retain(6),
                DeltaOp::Insert("!".to_string()),
                DeltaOp::Retain(5),
            ],
        );
        assert_eq!(insert.summary(), (6, 6));

        let empty = AheadDelta::new(4, vec![DeltaOp::Retain(4)]);
        assert_eq!(empty.summary(), (4, 4));
    }

    #[test]
    fn simple_insert_and_delete_queries() {
        let insert = AheadDelta::new(
            3,
            vec![
                DeltaOp::Retain(1),
                DeltaOp::Insert("xy".to_string()),
                DeltaOp::Retain(2),
            ],
        );
        assert_eq!(insert.as_simple_insert(), Some("xy"));
        assert!(!insert.is_simple_delete());

        let delete = AheadDelta::new(
            5,
            vec![DeltaOp::Retain(2), DeltaOp::Delete(3)],
        );
        assert_eq!(delete.as_simple_insert(), None);
        assert!(delete.is_simple_delete());

        let complex = AheadDelta::new(
            5,
            vec![
                DeltaOp::Delete(2),
                DeltaOp::Insert("ab".to_string()),
                DeltaOp::Delete(1),
            ],
        );
        assert_eq!(complex.as_simple_insert(), None);
        assert!(!complex.is_simple_delete());
    }
}
