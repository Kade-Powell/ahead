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

#[derive(Clone, Debug, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerId {
    pub author: String,
    pub name: String,
}
