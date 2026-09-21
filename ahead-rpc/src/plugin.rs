use core::fmt;

use serde::{Deserialize, Serialize};

use crate::counter::Counter;

#[derive(Eq, PartialEq, Hash, Clone, Copy, Debug, Serialize, Deserialize)]
pub struct PluginId(pub u64);

impl PluginId {
    pub fn next() -> Self {
        static PLUGIN_ID_COUNTER: Counter = Counter::new();
        Self(PLUGIN_ID_COUNTER.next())
    }
}

/// Legacy server identity (`author.name`) inherited from the removed volt
/// plugin format. Still carried by the language-server host plumbing until
/// the Zed-model extension host replaces it (see TODO.md). Do not use for
/// anything new.
#[derive(Clone, Debug, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoltID {
    pub author: String,
    pub name: String,
}

impl fmt::Display for VoltID {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.author, self.name)
    }
}
