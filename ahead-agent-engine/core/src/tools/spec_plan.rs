use crate::environment_selection::TurnEnvironmentSnapshot;
use crate::image_preparation::unified_image_budget_enabled;
use crate::session::session::Session;
use crate::session::turn_context::TurnContext;
use crate::tools::code_mode::execute_spec::create_code_mode_tool;
use crate::tools::effective_tool_mode;
use crate::tools::handlers::ApplyPatchHandler;
use crate::tools::handlers::CodeModeExecuteHandler;
use crate::tools::handlers::CodeModeWaitHandler;
use crate::tools::handlers::CurrentTimeHandler;
use crate::tools::handlers::DynamicToolHandler;
use crate::tools::handlers::ExecCommandHandler;
use crate::tools::handlers::ExecCommandHandlerOptions;
use crate::tools::handlers::GetContextRemainingHandler;
use crate::tools::handlers::ListMcpResourceTemplatesHandler;
use crate::tools::handlers::ListMcpResourcesHandler;
use crate::tools::handlers::NewContextWindowHandler;
use crate::tools::handlers::PlanHandler;
use crate::tools::handlers::ReadMcpResourceHandler;
use crate::tools::handlers::RequestPermissionsHandler;
use crate::tools::handlers::RequestUserInputHandler;
use crate::tools::handlers::SendUserMessageAsyncHandler;
use crate::tools::handlers::SleepHandler;
use crate::tools::handlers::ToolSearchHandlerCache;
use crate::tools::handlers::ViewImageHandler;
use crate::tools::handlers::WriteStdinHandler;
use crate::tools::handlers::tool_search_spec::ToolSearchSourceListing;
use crate::tools::handlers::view_image_spec::ViewImageToolOptions;
use crate::tools::hosted_spec::WebSearchToolOptions;
use crate::tools::hosted_spec::create_web_search_tool;
#[cfg(test)]
use crate::tools::registry::RegisteredTool;
use crate::tools::registry::ToolExposure;
use crate::tools::registry::ToolRegistry;
use crate::tools::router::CodeModeAvailability;
use crate::tools::router::ToolRouter;
use crate::tools::tool_namespaces_info::collect_tool_namespaces_info;
use codex_features::Feature;
use codex_features::SleepToolMode;
use codex_protocol::DEFAULT_FUNCTION_NAMESPACE;
use codex_protocol::dynamic_tools::DynamicToolNamespaceTool;
use codex_protocol::dynamic_tools::DynamicToolSpec;
use codex_protocol::error::CodexErrorDetails;
use codex_protocol::error::Result as CodexResult;
use codex_protocol::openai_models::ConfigShellToolType;
use codex_protocol::openai_models::ModelInfo;
use codex_protocol::openai_models::ModelMessages;
use codex_protocol::openai_models::ToolMode;
use codex_tools::ResponsesApiNamespaceTool;
use codex_tools::TOOL_SEARCH_TOOL_NAME;
use codex_tools::ToolExposures;
use codex_tools::ToolName;
use codex_tools::ToolSpec;
use codex_tools::can_request_original_image_detail;
use codex_tools::collect_code_mode_exec_prompt_tool_definitions;
use codex_tools::default_namespace_description;
use codex_tools::request_user_input_available_modes;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::btree_map::Entry;
use std::sync::Arc;
use tracing::instrument;

#[derive(Clone, Copy)]
struct CoreToolPlanContext<'a> {
    turn_context: &'a TurnContext,
    model_info: &'a ModelInfo,
    model_messages: Option<&'a ModelMessages>,
    environments: &'a TurnEnvironmentSnapshot,
    mcp: &'a codex_mcp::McpBinding,
}

#[instrument(level = "trace", skip_all)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_tool_router(
    session: &Session,
    turn_context: &TurnContext,
    model_info: &ModelInfo,
    model_messages: Option<&ModelMessages>,
    environments: &TurnEnvironmentSnapshot,
    mcp: &Arc<codex_mcp::McpBinding>,
) -> CodexResult<ToolRouter> {
    if model_info.supports_tool_calls == Some(false) {
        return Ok(ToolRouter::from_parts(
            ToolRegistry::default(),
            Vec::new(),
            ToolMode::Direct,
            CodeModeAvailability::Unavailable,
            BTreeMap::new(),
            None,
        ));
    }

    let context = CoreToolPlanContext {
        turn_context,
        model_info,
        model_messages,
        environments,
        mcp,
    };
    let mut registry = ToolRegistry::default();
    add_core_tool_sources(&context, &mut registry);

    let registered_mcp_tools = session.services.mcp_handler_cache.append_mcp_tools(
        mcp,
        search_tool_enabled(turn_context, model_info),
        &mut registry,
    );
    apply_mcp_tool_exposure_policy(
        turn_context,
        model_info,
        mcp,
        &registered_mcp_tools,
        &mut registry,
    );
    append_dynamic_tool_runtimes(&turn_context.dynamic_tools, &mut registry);
    let hosted_specs = hosted_model_tool_specs(turn_context, model_info);

    finalize_tool_router(
        turn_context,
        model_info,
        registry,
        hosted_specs,
        &session.services.tool_search_handler_cache,
    )
}

fn apply_mcp_tool_exposure_policy(
    turn_context: &TurnContext,
    model_info: &ModelInfo,
    mcp: &codex_mcp::McpBinding,
    registered_mcp_tools: &HashSet<ToolName>,
    registry: &mut ToolRegistry,
) {
    let mut omitted_exposures_by_tool = HashMap::new();
    for tool in mcp.tools() {
        let tool_name = tool.canonical_tool_name();
        if !registered_mcp_tools.contains(&tool_name) {
            continue;
        }
        let Some(server) = mcp.config().mcp_server_catalog.server(&tool.server_name) else {
            continue;
        };
        omitted_exposures_by_tool
            .entry(tool_name)
            .or_insert_with(|| {
                server
                    .config()
                    .omit_tools_from
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .copied()
                    .collect::<ToolExposures>()
            });
    }

    for tool in registry.entries_mut() {
        let tool_name = tool.runtime.tool_name();
        let Some(omitted_exposures) = omitted_exposures_by_tool.get(&tool_name) else {
            continue;
        };
        let tool_name = tool_name.with_default_namespace();

        let mut exposures = ToolExposures::ALL.difference(*omitted_exposures);
        if tool_name.namespace.as_ref().is_some_and(|namespace| {
            turn_context
                .config
                .code_mode
                .direct_only_tool_namespaces
                .contains(namespace)
        }) {
            exposures = exposures.difference(ToolExposures::DEFERRED | ToolExposures::CODE_MODE);
        }

        exposures = if search_tool_enabled(turn_context, model_info)
            && exposures.contains(ToolExposures::DEFERRED)
            && (effective_tool_mode(turn_context, model_info) != ToolMode::CodeModeOnly
                || exposures.contains(ToolExposures::CODE_MODE))
        {
            exposures.difference(ToolExposures::DIRECT)
        } else {
            exposures.difference(ToolExposures::DEFERRED)
        };

        tool.exposure = match (
            exposures.contains(ToolExposures::DIRECT),
            exposures.contains(ToolExposures::DEFERRED),
            exposures.contains(ToolExposures::CODE_MODE),
        ) {
            (false, false, false) => ToolExposure::Hidden,
            (false, false, true) => ToolExposure::CodeModeOnly,
            (true, false, false) => ToolExposure::DirectModelOnly,
            (true, false, true) => ToolExposure::Direct,
            (false, true, false) => ToolExposure::DeferredModelOnly,
            (false, true, true) => ToolExposure::Deferred,
            (true, true, _) => unreachable!("direct and deferred exposure are mutually exclusive"),
        };
    }
}

#[cfg(test)]
#[instrument(level = "trace", skip_all)]
pub(crate) fn build_core_tool_registry(
    turn_context: &TurnContext,
    model_info: &ModelInfo,
    environments: &TurnEnvironmentSnapshot,
    mcp: &codex_mcp::McpBinding,
) -> ToolRegistry {
    let context = CoreToolPlanContext {
        turn_context,
        model_info,
        model_messages: model_info.model_messages.as_ref(),
        environments,
        mcp,
    };
    let mut registry = ToolRegistry::default();
    add_core_tool_sources(&context, &mut registry);
    registry
}

#[cfg(test)]
#[instrument(level = "trace", skip_all)]
pub(crate) fn append_source_tools(
    turn_context: &TurnContext,
    model_info: &ModelInfo,
    registry: &mut ToolRegistry,
    mcp_tools: Vec<RegisteredTool>,
    dynamic_tools: &[DynamicToolSpec],
) -> Vec<ToolSpec> {
    for tool in mcp_tools {
        registry.register_external_with_exposure(tool.runtime, tool.exposure);
    }
    append_dynamic_tool_runtimes(dynamic_tools, registry);
    hosted_model_tool_specs(turn_context, model_info)
}

#[instrument(level = "trace", skip_all)]
pub(crate) fn finalize_tool_router(
    turn_context: &TurnContext,
    model_info: &ModelInfo,
    mut registry: ToolRegistry,
    hosted_specs: Vec<ToolSpec>,
    tool_search_handler_cache: &ToolSearchHandlerCache,
) -> CodexResult<ToolRouter> {
    apply_direct_model_only_namespace_overrides(turn_context, &mut registry);
    let tool_mode = effective_tool_mode(turn_context, model_info);
    let code_mode_enabled = matches!(tool_mode, ToolMode::CodeMode | ToolMode::CodeModeOnly);
    if code_mode_enabled {
        for tool_name in [
            ToolName::plain(codex_code_mode::PUBLIC_TOOL_NAME),
            ToolName::plain(codex_code_mode::WAIT_TOOL_NAME),
        ] {
            if registry.remove(&tool_name).is_some() {
                registry.record_collision(tool_name);
            }
        }
    }
    let tool_search_name = ToolName::plain(TOOL_SEARCH_TOOL_NAME);
    if search_tool_enabled(turn_context, model_info)
        && registry.entries().any(|tool| {
            tool.runtime.tool_name() != tool_search_name
                && tool.exposure.is_deferred()
                && tool.runtime.search_info().is_some()
        })
    {
        if registry.remove(&tool_search_name).is_some() {
            registry.record_collision(tool_search_name.clone());
        }
        // Special model tools own the namespace matching their wire identity, so
        // regular namespace tools cannot advertise that same model-visible surface.
        let conflicting_tool_names = registry
            .entries()
            .filter_map(|tool| {
                let owned_spec;
                let spec = if let Some(spec) = tool.runtime.immutable_spec() {
                    spec.as_ref()
                } else {
                    owned_spec = tool.runtime.spec();
                    &owned_spec
                };
                let ToolSpec::Namespace(namespace) = spec else {
                    return None;
                };
                (namespace.name == tool_search_name.name).then(|| tool.runtime.tool_name())
            })
            .collect::<Vec<_>>();
        for tool_name in conflicting_tool_names {
            if registry.remove(&tool_name).is_some() {
                registry.record_collision(tool_name);
            }
        }
        append_tool_search_executor(turn_context, &mut registry, tool_search_handler_cache);
    }

    let code_mode_tool_names =
        register_code_mode_executors(turn_context, model_info, &mut registry);
    let include_tool_namespaces_info = turn_context
        .config
        .tool_registry
        .turn_metadata_includes_tool_info
        && model_info.use_responses_lite;

    if turn_context.config.tool_registry.error_on_tool_collisions {
        if let Some(tool_name) = registry.first_collision() {
            let namespace = tool_name.namespace.as_deref().unwrap_or("functions");
            let name = format!("{namespace}.{}", tool_name.name);
            return Err(CodexErrorDetails::ToolCollision(name).into());
        }

        let mut namespace_descriptions = BTreeMap::new();
        let mut namespace_owners = BTreeMap::new();
        for tool in registry.entries() {
            let owned_spec;
            let spec = if let Some(spec) = tool.runtime.immutable_spec() {
                spec.as_ref()
            } else {
                owned_spec = tool.runtime.spec();
                &owned_spec
            };
            if include_tool_namespaces_info && tool.exposure != ToolExposure::Hidden {
                let namespace_name = match spec {
                    ToolSpec::Namespace(namespace) => namespace.name.as_str(),
                    ToolSpec::Function(_) | ToolSpec::Freeform(_) => DEFAULT_FUNCTION_NAMESPACE,
                    ToolSpec::ToolSearch { .. } => TOOL_SEARCH_TOOL_NAME,
                    ToolSpec::WebSearch { .. } => continue,
                };
                let owner = tool.runtime.mcp_server_name();
                match namespace_owners.get(namespace_name) {
                    Some(existing_owner) if existing_owner != &owner => {
                        return Err(
                            CodexErrorDetails::ToolCollision(namespace_name.to_string()).into()
                        );
                    }
                    Some(_) => {}
                    None => {
                        namespace_owners.insert(namespace_name.to_string(), owner);
                    }
                }
            }

            let ToolSpec::Namespace(namespace) = spec else {
                continue;
            };
            if namespace.description.trim().is_empty() {
                continue;
            }
            match namespace_descriptions.entry(namespace.name.clone()) {
                Entry::Vacant(entry) => {
                    entry.insert(namespace.description.clone());
                }
                Entry::Occupied(entry) if entry.get() != &namespace.description => {
                    return Err(CodexErrorDetails::ToolCollision(entry.key().clone()).into());
                }
                Entry::Occupied(_) => {}
            }
        }
    }

    let model_visible_specs = build_model_visible_specs(
        turn_context,
        model_info,
        &registry,
        &code_mode_tool_names,
        hosted_specs,
    );
    let tool_namespaces_info = include_tool_namespaces_info
        .then(|| {
            collect_tool_namespaces_info(&registry, &code_mode_tool_names, &model_visible_specs)
        })
        .filter(|info| !info.is_empty());
    Ok(ToolRouter::from_parts(
        registry,
        model_visible_specs,
        tool_mode,
        if turn_context.code_mode_available {
            CodeModeAvailability::Available
        } else {
            CodeModeAvailability::Unavailable
        },
        code_mode_tool_names,
        tool_namespaces_info,
    ))
}

fn apply_direct_model_only_namespace_overrides(
    turn_context: &TurnContext,
    registry: &mut ToolRegistry,
) {
    if turn_context
        .config
        .code_mode
        .direct_only_tool_namespaces
        .is_empty()
    {
        return;
    }

    for tool in registry.entries_mut() {
        let configured = tool
            .runtime
            .tool_name()
            .with_default_namespace()
            .namespace
            .as_ref()
            .is_some_and(|namespace| {
                turn_context
                    .config
                    .code_mode
                    .direct_only_tool_namespaces
                    .contains(namespace)
            });
        if configured && tool.exposure.is_available_in_code_mode() {
            tool.exposure = ToolExposure::DirectModelOnly;
        }
    }
}

#[instrument(level = "trace", skip_all)]
fn build_model_visible_specs(
    turn_context: &TurnContext,
    model_info: &ModelInfo,
    registry: &ToolRegistry,
    code_mode_tool_names: &BTreeMap<String, ToolName>,
    hosted_specs: Vec<ToolSpec>,
) -> Vec<ToolSpec> {
    let mut specs = Vec::new();
    for tool in registry.entries() {
        let exposure = tool.exposure;
        if !exposure.is_direct() {
            continue;
        }

        let tool_name = tool.runtime.tool_name();
        if is_hidden_by_code_mode_only(turn_context, model_info, &tool_name, exposure) {
            continue;
        }

        let spec = tool.runtime.spec();
        specs.push(spec_for_model_request(
            turn_context,
            model_info,
            exposure,
            &tool_name,
            code_mode_tool_names,
            spec,
        ));
    }
    specs.extend(hosted_specs);

    merge_into_namespaces(specs)
        .into_iter()
        .filter(|spec| {
            namespace_tools_enabled(turn_context) || !matches!(spec, ToolSpec::Namespace(_))
        })
        .collect()
}

fn spec_for_model_request(
    turn_context: &TurnContext,
    model_info: &ModelInfo,
    exposure: ToolExposure,
    tool_name: &ToolName,
    code_mode_tool_names: &BTreeMap<String, ToolName>,
    spec: ToolSpec,
) -> ToolSpec {
    let tool_mode = effective_tool_mode(turn_context, model_info);
    if matches!(tool_mode, ToolMode::CodeMode | ToolMode::CodeModeOnly)
        && exposure.is_available_in_code_mode()
        && !is_excluded_from_code_mode(turn_context, tool_name)
        && codex_code_mode::is_code_mode_nested_tool(spec.name())
        && code_mode_tool_names
            .get(&codex_code_mode::normalize_code_mode_identifier(
                &codex_tools::code_mode_name_for_tool_name(tool_name),
            ))
            .is_some_and(|winner| winner == tool_name)
    {
        codex_tools::augment_tool_spec_for_code_mode(spec)
    } else {
        spec
    }
}

#[instrument(level = "trace", skip_all)]
fn hosted_model_tool_specs(turn_context: &TurnContext, model_info: &ModelInfo) -> Vec<ToolSpec> {
    // Responses Lite accepts schemas for client-executed tools, not hosted Responses tools.
    if model_info.use_responses_lite {
        return Vec::new();
    }

    let mut specs = Vec::new();
    let web_search_mode = turn_context
        .provider
        .capabilities()
        .web_search
        .then_some(turn_context.config.web_search_mode.value());
    let web_search_config = web_search_mode
        .as_ref()
        .and(turn_context.config.web_search_config.as_ref());
    if let Some(hosted_web_search_tool) = create_web_search_tool(WebSearchToolOptions {
        web_search_mode,
        web_search_config,
        web_search_tool_type: model_info.web_search_tool_type,
    }) {
        specs.push(hosted_web_search_tool);
    }
    specs
}

pub(crate) fn search_tool_enabled(turn_context: &TurnContext, model_info: &ModelInfo) -> bool {
    model_info.supports_search_tool && namespace_tools_enabled(turn_context)
}

fn namespace_tools_enabled(turn_context: &TurnContext) -> bool {
    turn_context.provider.capabilities().namespace_tools
}

fn is_hidden_by_code_mode_only(
    turn_context: &TurnContext,
    model_info: &ModelInfo,
    tool_name: &ToolName,
    exposure: ToolExposure,
) -> bool {
    let tool_mode = effective_tool_mode(turn_context, model_info);
    tool_mode == ToolMode::CodeModeOnly
        && exposure.is_available_in_code_mode()
        && codex_code_mode::is_code_mode_nested_tool(&codex_tools::code_mode_name_for_tool_name(
            tool_name,
        ))
}

fn is_excluded_from_code_mode(turn_context: &TurnContext, tool_name: &ToolName) -> bool {
    let tool_name = tool_name.clone().with_default_namespace();
    tool_name.namespace.as_ref().is_some_and(|namespace| {
        turn_context
            .config
            .code_mode
            .excluded_tool_namespaces
            .contains(namespace)
    })
}

fn register_code_mode_executors(
    turn_context: &TurnContext,
    model_info: &ModelInfo,
    registry: &mut ToolRegistry,
) -> BTreeMap<String, ToolName> {
    let tool_mode = effective_tool_mode(turn_context, model_info);
    if !turn_context.code_mode_available
        || !matches!(tool_mode, ToolMode::CodeMode | ToolMode::CodeModeOnly)
    {
        return BTreeMap::new();
    }

    let mut code_mode_tool_names = BTreeMap::new();
    let mut code_mode_nested_tool_specs = Vec::new();
    let mut exec_prompt_tool_specs = Vec::new();
    let mut deferred_exec_prompt_tool_specs = Vec::new();
    let mut included_deferred_mcp_output_schema = false;
    let deferred_tools_guidance_enabled = search_tool_enabled(turn_context, model_info);
    for tool in registry.entries() {
        let exposure = tool.exposure;
        if !exposure.is_available_in_code_mode() {
            continue;
        }

        let tool_name = tool.runtime.tool_name();
        if is_excluded_from_code_mode(turn_context, &tool_name) {
            continue;
        }

        let immutable_spec = tool.runtime.immutable_spec();
        let cached_runtime = immutable_spec.map(|_| Arc::clone(&tool.runtime));
        let spec = immutable_spec
            .map(Arc::clone)
            .unwrap_or_else(|| Arc::new(tool.runtime.spec()));
        // Derive the name without serializing and augmenting every tool schema.
        let code_mode_name = match spec.as_ref() {
            ToolSpec::Function(_) | ToolSpec::Freeform(_) => {
                codex_tools::code_mode_name_for_tool_name(&tool_name)
            }
            ToolSpec::Namespace(namespace) if !namespace.tools.is_empty() => {
                codex_tools::code_mode_name_for_tool_name(&tool_name)
            }
            ToolSpec::Namespace(_) | ToolSpec::ToolSearch { .. } | ToolSpec::WebSearch { .. } => {
                continue;
            }
        };
        if !codex_code_mode::is_code_mode_nested_tool(&code_mode_name) {
            continue;
        }
        match code_mode_tool_names.entry(codex_code_mode::normalize_code_mode_identifier(
            &code_mode_name,
        )) {
            Entry::Vacant(entry) => {
                entry.insert(tool_name);
            }
            Entry::Occupied(_) => {
                tracing::warn!(
                    tool_name = %tool_name,
                    code_mode_name = %code_mode_name,
                    "skipping tool with a duplicate normalized code-mode name"
                );
                continue;
            }
        }

        if exposure == ToolExposure::Deferred {
            if deferred_tools_guidance_enabled
                && (cached_runtime.is_none() || !included_deferred_mcp_output_schema)
            {
                if cached_runtime.is_some() {
                    included_deferred_mcp_output_schema = true;
                }
                deferred_exec_prompt_tool_specs.push(Arc::clone(&spec));
            }
        } else {
            exec_prompt_tool_specs.push(spec.as_ref().clone());
        }
        code_mode_nested_tool_specs.push((spec, cached_runtime));
    }

    let namespace_descriptions = code_mode_namespace_descriptions(&exec_prompt_tool_specs);
    let mut enabled_tools =
        collect_code_mode_exec_prompt_tool_definitions(exec_prompt_tool_specs.iter());
    let deferred_tools = collect_code_mode_exec_prompt_tool_definitions(
        deferred_exec_prompt_tool_specs.iter().map(Arc::as_ref),
    );
    enabled_tools
        .sort_by(|left, right| compare_code_mode_tools(left, right, &namespace_descriptions));
    let execute_handler = CodeModeExecuteHandler::new(
        create_code_mode_tool(
            &enabled_tools,
            &deferred_tools,
            &namespace_descriptions,
            turn_context.config.code_mode.default_exec_yield_time_ms,
            tool_mode == ToolMode::CodeModeOnly,
            if unified_image_budget_enabled(&turn_context.config.features, model_info) {
                codex_code_mode::ImageDetailVisibility::Hidden
            } else {
                codex_code_mode::ImageDetailVisibility::Visible
            },
        ),
        code_mode_nested_tool_specs,
    );

    registry.prepend_trusted(Arc::new(CodeModeWaitHandler));
    registry.prepend_trusted(Arc::new(execute_handler));

    code_mode_tool_names
}

#[instrument(level = "trace", skip_all, fields(tool_spec_count = specs.len()))]
fn merge_into_namespaces(specs: Vec<ToolSpec>) -> Vec<ToolSpec> {
    let mut merged_specs = Vec::with_capacity(specs.len());
    let mut namespace_indices = BTreeMap::<String, usize>::new();
    for spec in specs {
        match spec {
            ToolSpec::Namespace(mut namespace) => {
                if let Some(index) = namespace_indices.get(&namespace.name).copied() {
                    let ToolSpec::Namespace(existing_namespace) = &mut merged_specs[index] else {
                        unreachable!("namespace index must point to a namespace spec");
                    };
                    if existing_namespace.description.trim().is_empty()
                        && !namespace.description.trim().is_empty()
                    {
                        existing_namespace.description = namespace.description;
                    }
                    existing_namespace.tools.append(&mut namespace.tools);
                    continue;
                }

                namespace_indices.insert(namespace.name.clone(), merged_specs.len());
                merged_specs.push(ToolSpec::Namespace(namespace));
            }
            spec => merged_specs.push(spec),
        }
    }

    for spec in &mut merged_specs {
        let ToolSpec::Namespace(namespace) = spec else {
            continue;
        };

        namespace.tools.sort_by(|left, right| {
            let left_name = match left {
                ResponsesApiNamespaceTool::Function(tool) => &tool.name,
                ResponsesApiNamespaceTool::Custom(tool) => &tool.name,
            };
            let right_name = match right {
                ResponsesApiNamespaceTool::Function(tool) => &tool.name,
                ResponsesApiNamespaceTool::Custom(tool) => &tool.name,
            };
            left_name.cmp(right_name)
        });

        if namespace.description.trim().is_empty() {
            namespace.description = default_namespace_description(&namespace.name);
        }
    }

    merged_specs
}

fn code_mode_namespace_descriptions(
    specs: &[ToolSpec],
) -> BTreeMap<String, codex_code_mode::ToolNamespaceDescription> {
    let mut namespace_descriptions = BTreeMap::new();
    for spec in specs {
        let ToolSpec::Namespace(namespace) = spec else {
            continue;
        };

        let entry = namespace_descriptions
            .entry(namespace.name.clone())
            .or_insert_with(|| codex_code_mode::ToolNamespaceDescription {
                name: namespace.name.clone(),
                description: namespace.description.clone(),
            });
        if entry.description.trim().is_empty() && !namespace.description.trim().is_empty() {
            entry.description = namespace.description.clone();
        }
    }
    namespace_descriptions
}

#[instrument(level = "trace", skip_all)]
fn add_core_tool_sources(context: &CoreToolPlanContext<'_>, registry: &mut ToolRegistry) {
    add_shell_tools(context, registry);
    add_mcp_resource_tools(context, registry);
    add_core_utility_tools(context, registry);
}

fn primary_environment_allows_login_shell(environments: &TurnEnvironmentSnapshot) -> bool {
    environments
        .primary()
        .is_some_and(|environment| environment.config().allow_login_shell)
}

fn should_include_windows_shell_guidance(environments: &TurnEnvironmentSnapshot) -> bool {
    let Some(environment) = environments.primary() else {
        return false;
    };
    match environment.executor_platform_os.as_deref() {
        Some(platform_os) => platform_os == "windows",
        None => cfg!(windows),
    }
}

#[instrument(level = "trace", skip_all)]
fn add_shell_tools(context: &CoreToolPlanContext<'_>, registry: &mut ToolRegistry) {
    let turn_context = context.turn_context;
    let features = turn_context.config.features.get();
    if context.environments.primary().is_none()
        || !features.enabled(Feature::ShellTool)
        || matches!(context.model_info.shell_type, ConfigShellToolType::Disabled)
    {
        return;
    }

    let allow_login_shell = primary_environment_allows_login_shell(context.environments);
    let exec_permission_approvals_enabled = features.enabled(Feature::ExecPermissionApprovals);
    let options = ExecCommandHandlerOptions {
        allow_login_shell,
        exec_permission_approvals_enabled,
        include_shell_parameter: true,
        include_windows_shell_guidance: should_include_windows_shell_guidance(context.environments),
    };
    if features.enabled(Feature::UnifiedExec) {
        registry.add(ExecCommandHandler::new(options));
        registry.add(WriteStdinHandler);
    } else {
        // Managed requirements are the only configuration path that can keep
        // unified exec disabled. Preserve command execution without exposing a
        // resumable process or write_stdin authority prohibited by policy.
        registry.add(ExecCommandHandler::one_shot(options));
    }
}

#[instrument(level = "trace", skip_all)]
fn add_mcp_resource_tools(context: &CoreToolPlanContext<'_>, registry: &mut ToolRegistry) {
    if context.mcp.has_servers() {
        registry.add(ListMcpResourcesHandler);
        registry.add(ListMcpResourceTemplatesHandler);
        registry.add(ReadMcpResourceHandler);
    }
}

#[instrument(level = "trace", skip_all)]
fn add_core_utility_tools(context: &CoreToolPlanContext<'_>, registry: &mut ToolRegistry) {
    let turn_context = context.turn_context;
    let features = turn_context.config.features.get();
    if turn_context.config.update_plan_enabled {
        registry.add(PlanHandler);
    }

    if turn_context.config.experimental_request_user_input_enabled {
        registry.add_with_exposure(
            RequestUserInputHandler {
                available_modes: request_user_input_available_modes(features),
            },
            ToolExposure::DirectModelOnly,
        );
    }

    if !turn_context.session_source.is_non_root_agent()
        && context
            .model_info
            .experimental_supported_tools
            .iter()
            .any(|tool| tool == "send_user_message_async")
    {
        registry.add_with_exposure(
            SendUserMessageAsyncHandler {
                description: context
                    .model_messages
                    .and_then(|messages| messages.tools.as_ref())
                    .and_then(|tools| tools.send_user_message_async.as_ref())
                    .and_then(|tool| tool.description.clone()),
            },
            ToolExposure::DirectModelOnly,
        );
    }

    if context.environments.primary().is_some() && features.enabled(Feature::RequestPermissionsTool)
    {
        registry.add(RequestPermissionsHandler);
    }

    if features.enabled(Feature::TokenBudget) {
        registry.add_with_exposure(NewContextWindowHandler, ToolExposure::DirectModelOnly);
        registry.add(GetContextRemainingHandler);
    }

    let current_time_reminder_enabled = features.enabled(Feature::CurrentTimeReminder);
    let model_has_clock = context
        .model_info
        .experimental_supported_tools
        .iter()
        .any(|tool| tool == "clock");
    if current_time_reminder_enabled || model_has_clock {
        registry.add(CurrentTimeHandler);
    }
    if features.enabled(Feature::SleepTool)
        && match turn_context.config.sleep_tool_mode {
            SleepToolMode::AlwaysOn => true,
            SleepToolMode::ModelDriven => {
                if current_time_reminder_enabled {
                    turn_context
                        .config
                        .current_time_reminder
                        .as_ref()
                        .is_some_and(|config| config.sleep_tool)
                } else {
                    model_has_clock
                }
            }
        }
    {
        registry.add(SleepHandler);
    }

    if context.environments.primary().is_some()
        && let Some(tool_type) = context.model_info.apply_patch_tool_type.as_ref()
    {
        registry.add(ApplyPatchHandler::new(tool_type.clone()));
    }

    if context.environments.primary().is_some() && features.enabled(Feature::ViewImage) {
        registry.add(ViewImageHandler::new(ViewImageToolOptions {
            can_request_original_image_detail: can_request_original_image_detail(
                context.model_info,
            ),
            unified_image_budget: unified_image_budget_enabled(
                &turn_context.config.features,
                context.model_info,
            ),
        }));
    }
}

#[instrument(level = "trace", skip_all, fields(dynamic_tool_count = dynamic_tools.len()))]
fn append_dynamic_tool_runtimes(dynamic_tools: &[DynamicToolSpec], registry: &mut ToolRegistry) {
    for spec in dynamic_tools {
        match spec {
            DynamicToolSpec::Function(tool) => {
                let Some(handler) = DynamicToolHandler::new(tool) else {
                    tracing::error!(
                        "Failed to convert dynamic tool {:?} to OpenAI tool",
                        tool.name
                    );
                    continue;
                };
                registry.register_external(Arc::new(handler));
            }
            DynamicToolSpec::Namespace(namespace) => {
                for tool in &namespace.tools {
                    let DynamicToolNamespaceTool::Function(tool) = tool;
                    let Some(handler) = DynamicToolHandler::new_in_namespace(namespace, tool)
                    else {
                        tracing::error!(
                            "Failed to convert dynamic tool {:?}.{:?} to OpenAI tool",
                            namespace.name,
                            tool.name
                        );
                        continue;
                    };
                    registry.register_external(Arc::new(handler));
                }
            }
        }
    }
}

#[instrument(level = "trace", skip_all)]
fn append_tool_search_executor(
    turn_context: &TurnContext,
    registry: &mut ToolRegistry,
    tool_search_handler_cache: &ToolSearchHandlerCache,
) {
    let source_listing = if turn_context
        .config
        .features
        .enabled(Feature::DeferredToolWorldState)
    {
        ToolSearchSourceListing::Omit
    } else {
        ToolSearchSourceListing::Include
    };
    let handler = tool_search_handler_cache.get_or_build(registry, source_listing);
    registry.register_trusted(handler);
}

fn compare_code_mode_tools(
    left: &codex_code_mode::ToolDefinition,
    right: &codex_code_mode::ToolDefinition,
    namespace_descriptions: &BTreeMap<String, codex_code_mode::ToolNamespaceDescription>,
) -> std::cmp::Ordering {
    let left_namespace = code_mode_namespace_name(left, namespace_descriptions);
    let right_namespace = code_mode_namespace_name(right, namespace_descriptions);

    left_namespace
        .cmp(&right_namespace)
        .then_with(|| left.tool_name.name.cmp(&right.tool_name.name))
        .then_with(|| left.name.cmp(&right.name))
}

fn code_mode_namespace_name<'a>(
    tool: &codex_code_mode::ToolDefinition,
    namespace_descriptions: &'a BTreeMap<String, codex_code_mode::ToolNamespaceDescription>,
) -> Option<&'a str> {
    tool.tool_name
        .namespace
        .as_ref()
        .and_then(|namespace| namespace_descriptions.get(namespace))
        .map(|namespace_description| namespace_description.name.as_str())
}

#[cfg(test)]
#[path = "spec_plan_tests.rs"]
mod tests;
