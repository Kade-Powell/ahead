use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct GitFileState {
    pub hunks: Vec<DiffHunk>,
    pub blame: Vec<BlameHunk>,
}

/// One-indexed buffer rows; a deletion has no live rows and marks its boundary.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiffHunk {
    pub start: u32,
    pub len: u32,
    pub kind: DiffHunkKind,
    pub old_start: u32,
    pub old_lines: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum DiffHunkKind {
    Added,
    Modified,
    Deleted,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BlameHunk {
    pub start: usize,
    pub len: usize,
    /// None denotes uncommitted buffer content.
    pub commit: Option<BlameCommit>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BlameCommit {
    pub id: String,
    pub author: String,
    pub timestamp: i64,
    pub subject: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct DiffInfo {
    pub head: String,
    pub branches: Vec<String>,
    pub tags: Vec<String>,
    pub diffs: Vec<FileDiff>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum FileDiff {
    Modified(PathBuf),
    Added(PathBuf),
    Deleted(PathBuf),
    Renamed(PathBuf, PathBuf),
}

impl FileDiff {
    pub fn path(&self) -> &PathBuf {
        match &self {
            FileDiff::Modified(p)
            | FileDiff::Added(p)
            | FileDiff::Deleted(p)
            | FileDiff::Renamed(_, p) => p,
        }
    }

    pub fn kind(&self) -> FileDiffKind {
        match self {
            FileDiff::Modified(_) => FileDiffKind::Modified,
            FileDiff::Added(_) => FileDiffKind::Added,
            FileDiff::Deleted(_) => FileDiffKind::Deleted,
            FileDiff::Renamed(_, _) => FileDiffKind::Renamed,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileDiffKind {
    Modified,
    Added,
    Deleted,
    Renamed,
}
