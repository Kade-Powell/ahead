use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::hash_map::Entry;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::Weak;

use codex_mcp::McpBinding;
use codex_mcp::ToolInfo as McpToolInfo;
use codex_mcp::tool_is_model_visible;
use codex_tools::ToolExposure;
use codex_tools::ToolName;
use tracing::instrument;
use tracing::warn;

use crate::tools::handlers::McpHandler;
use crate::tools::registry::ToolRegistry;

#[derive(Default)]
pub(crate) struct McpHandlerCache {
    cached: Mutex<Option<CachedMcpHandlers>>,
}

struct CachedMcpHandlers {
    binding: Weak<McpBinding>,
    handlers: HashMap<ToolName, Arc<McpHandler>>,
}

impl McpHandlerCache {
    pub(crate) fn append_mcp_tools(
        &self,
        binding: &Arc<McpBinding>,
        search_tool_enabled: bool,
        registry: &mut ToolRegistry,
    ) -> HashSet<ToolName> {
        let mut cached = self
            .cached
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !cached
            .as_ref()
            .and_then(|cached| cached.binding.upgrade())
            .is_some_and(|cached_binding| Arc::ptr_eq(&cached_binding, binding))
        {
            *cached = None;
        }

        let cached = cached.get_or_insert_with(|| CachedMcpHandlers {
            binding: Arc::downgrade(binding),
            handlers: HashMap::new(),
        });
        append_mcp_tools(
            binding.tools(),
            search_tool_enabled,
            &mut cached.handlers,
            registry,
        )
    }
}

#[instrument(level = "trace", skip_all)]
fn append_mcp_tools(
    all_mcp_tools: &[McpToolInfo],
    search_tool_enabled: bool,
    handlers: &mut HashMap<ToolName, Arc<McpHandler>>,
    registry: &mut ToolRegistry,
) -> HashSet<ToolName> {
    let exposure = if search_tool_enabled {
        ToolExposure::Deferred
    } else {
        ToolExposure::Direct
    };
    let mut registered_tools = HashSet::new();
    for tool in all_mcp_tools
        .iter()
        .filter(|tool| tool_is_model_visible(tool))
    {
        let tool_name = tool.canonical_tool_name();
        let handler = match handlers.entry(tool_name.clone()) {
            Entry::Occupied(entry) => Arc::clone(entry.get()),
            Entry::Vacant(entry) => match McpHandler::new(tool.clone()) {
                Ok(handler) => Arc::clone(entry.insert(Arc::new(handler))),
                Err(err) => {
                    warn!("Skipping MCP tool `{tool_name}`: failed to build tool spec: {err}");
                    continue;
                }
            },
        };

        if registry.register_external_with_exposure(handler, exposure) {
            registered_tools.insert(tool_name);
        }
    }
    registered_tools
}

#[cfg(test)]
#[path = "mcp_tool_exposure_test.rs"]
mod tests;
