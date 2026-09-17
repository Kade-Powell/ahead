use serde::{Deserialize, Serialize};
use strum_macros::EnumIter;

use super::{data::PanelOrder, position::PanelPosition};
use crate::config::icon::AheadIcons;

#[derive(
    Clone, Copy, PartialEq, Serialize, Deserialize, Hash, Eq, Debug, EnumIter,
)]
pub enum PanelKind {
    Terminal,
    FileExplorer,
    SourceControl,
    Plugin,
    Search,
    Problem,
    Debug,
    CallHierarchy,
    DocumentSymbol,
    References,
    Implementation,
    AheadAgent,
}

impl PanelKind {
    pub fn svg_name(&self) -> &'static str {
        match &self {
            PanelKind::Terminal => AheadIcons::TERMINAL,
            PanelKind::FileExplorer => AheadIcons::FILE_EXPLORER,
            PanelKind::SourceControl => AheadIcons::SCM,
            PanelKind::Plugin => AheadIcons::EXTENSIONS,
            PanelKind::Search => AheadIcons::SEARCH,
            PanelKind::Problem => AheadIcons::PROBLEM,
            PanelKind::Debug => AheadIcons::DEBUG,
            PanelKind::CallHierarchy => AheadIcons::TYPE_HIERARCHY,
            PanelKind::DocumentSymbol => AheadIcons::DOCUMENT_SYMBOL,
            PanelKind::References => AheadIcons::REFERENCES,
            PanelKind::Implementation => AheadIcons::IMPLEMENTATION,
            PanelKind::AheadAgent => AheadIcons::LIGHTBULB,
        }
    }

    pub fn position(&self, order: &PanelOrder) -> Option<(usize, PanelPosition)> {
        for (pos, panels) in order.iter() {
            let index = panels.iter().position(|k| k == self);
            if let Some(index) = index {
                return Some((index, *pos));
            }
        }
        None
    }

    pub fn default_position(&self) -> PanelPosition {
        match self {
            PanelKind::Terminal => PanelPosition::BottomLeft,
            PanelKind::FileExplorer => PanelPosition::LeftTop,
            PanelKind::SourceControl => PanelPosition::LeftTop,
            PanelKind::Plugin => PanelPosition::LeftTop,
            PanelKind::Search => PanelPosition::BottomLeft,
            PanelKind::Problem => PanelPosition::BottomLeft,
            PanelKind::Debug => PanelPosition::LeftTop,
            PanelKind::CallHierarchy => PanelPosition::BottomLeft,
            PanelKind::DocumentSymbol => PanelPosition::RightTop,
            PanelKind::References => PanelPosition::BottomLeft,
            PanelKind::Implementation => PanelPosition::BottomLeft,
            PanelKind::AheadAgent => PanelPosition::RightTop,
        }
    }
}
