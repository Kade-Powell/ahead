use crate::session::tests::update_turn_settings_for_test;
use std::collections::BTreeMap;
use std::sync::Arc;

use ahead_model_auth::AheadAuth;
use ahead_model_auth::AuthManager;
use codex_features::Feature;
use codex_mcp::ToolInfo;
use codex_model_provider::create_model_provider;
use codex_model_provider_info::AMAZON_BEDROCK_GPT_5_5_MODEL_ID;
use codex_model_provider_info::AMAZON_BEDROCK_GPT_5_6_LUNA_MODEL_ID;
use codex_model_provider_info::AMAZON_BEDROCK_GPT_5_6_SOL_MODEL_ID;
use codex_model_provider_info::AMAZON_BEDROCK_PROVIDER_ID;
use codex_model_provider_info::ModelProviderInfo;
use codex_protocol::ThreadId;
use codex_protocol::config_types::WebSearchMode;
use codex_protocol::dynamic_tools::DynamicToolSpec;
use codex_protocol::error::CodexErrorDetails;
use codex_protocol::openai_models::ApplyPatchToolType;
use codex_protocol::openai_models::ConfigShellToolType;
use codex_protocol::openai_models::ModelInfo;
use codex_protocol::openai_models::ToolMode;
use codex_protocol::openai_models::WebSearchToolType;
use codex_protocol::protocol::EnvironmentConfigState;
use codex_tools::ResponsesApiNamespaceTool;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolExposure;
use codex_tools::ToolName;
use codex_tools::ToolSpec;
use pretty_assertions::assert_eq;
use serde_json::json;

use crate::config::CurrentTimeReminderConfig;
use crate::environment_selection::TurnEnvironmentState;
use crate::responses_metadata::CodexResponsesRequestKind;
use crate::responses_metadata::TurnToolFunctionInfo;
use crate::responses_metadata::TurnToolNamespacesInfo;
use crate::responses_metadata::TurnToolSource;
use crate::session::step_context::StepContext;
use crate::session::tests::make_session_and_context;
use crate::session::tests::mcp_config_for_test;
use crate::session::turn_context::TurnContext;
use crate::tools::handlers::McpHandler;
use crate::tools::handlers::ToolSearchHandlerCache;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::RegisteredTool;
use crate::tools::router::ToolRouter;
use crate::tools::spec_plan::append_source_tools;
use crate::tools::spec_plan::build_core_tool_registry;

#[derive(Default)]
struct ToolPlanInputs {
    tool_runtimes: Vec<RegisteredTool>,
    dynamic_tools: Vec<DynamicToolSpec>,
}

#[derive(Debug, PartialEq)]
struct ToolPlanProbe {
    visible_specs: Vec<ToolSpec>,
    visible_names: Vec<String>,
    namespace_functions: BTreeMap<String, Vec<String>>,
    registered_names: Vec<String>,
    exposures: BTreeMap<String, ToolExposure>,
    tool_namespaces_info: Option<TurnToolNamespacesInfo>,
    code_mode_tool_names: BTreeMap<String, ToolName>,
    tool_mode: ToolMode,
    requires_code_mode_worker: bool,
    has_terminal_controls: bool,
}

impl ToolPlanProbe {
    fn from_router(router: ToolRouter) -> Self {
        let visible_specs = router.model_visible_specs().to_vec();
        let visible_names = visible_specs
            .iter()
            .map(|spec| spec.name().to_string())
            .collect::<Vec<_>>();
        let namespace_functions = visible_specs
            .iter()
            .filter_map(|spec| match spec {
                ToolSpec::Namespace(namespace) => Some((
                    namespace.name.clone(),
                    namespace
                        .tools
                        .iter()
                        .map(|tool| match tool {
                            ResponsesApiNamespaceTool::Function(tool) => tool.name.clone(),
                            ResponsesApiNamespaceTool::Custom(tool) => tool.name.clone(),
                        })
                        .collect::<Vec<_>>(),
                )),
                ToolSpec::Function(_)
                | ToolSpec::ToolSearch { .. }
                | ToolSpec::WebSearch { .. }
                | ToolSpec::Freeform(_) => None,
            })
            .collect::<BTreeMap<_, _>>();
        let registered_tool_names = router.registered_tool_names_for_test();
        let registered_names = registered_tool_names
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        let exposures = registered_tool_names
            .iter()
            .filter_map(|name| {
                router
                    .tool_exposure_for_test(name)
                    .map(|exposure| (name.to_string(), exposure))
            })
            .collect::<BTreeMap<_, _>>();

        Self {
            visible_specs,
            visible_names,
            namespace_functions,
            registered_names,
            exposures,
            tool_namespaces_info: router.tool_namespaces_info().cloned(),
            code_mode_tool_names: router.code_mode_tool_names().clone(),
            tool_mode: router.tool_mode(),
            requires_code_mode_worker: router.requires_code_mode_worker(),
            has_terminal_controls: router.has_terminal_controls(),
        }
    }

    fn assert_visible_contains(&self, expected: &[&str]) {
        for name in expected {
            assert!(
                self.visible_names.iter().any(|visible| visible == name),
                "expected visible tool `{name}` in {:?}",
                self.visible_names
            );
        }
    }

    fn assert_visible_lacks(&self, expected_absent: &[&str]) {
        for name in expected_absent {
            assert!(
                !self.visible_names.iter().any(|visible| visible == name),
                "expected visible tool `{name}` to be absent from {:?}",
                self.visible_names
            );
        }
    }

    fn assert_registered_contains(&self, expected: &[&str]) {
        for name in expected {
            assert!(
                self.registered_names
                    .iter()
                    .any(|registered| registered == name),
                "expected registered tool `{name}` in {:?}",
                self.registered_names
            );
        }
    }

    fn assert_registered_lacks(&self, expected_absent: &[&str]) {
        for name in expected_absent {
            assert!(
                !self
                    .registered_names
                    .iter()
                    .any(|registered| registered == name),
                "expected registered tool `{name}` to be absent from {:?}",
                self.registered_names
            );
        }
    }

    fn namespace_function_names(&self, namespace: &str) -> &[String] {
        self.namespace_functions
            .get(namespace)
            .map_or(&[], Vec::as_slice)
    }

    fn visible_spec(&self, name: &str) -> &ToolSpec {
        self.visible_specs
            .iter()
            .find(|spec| spec.name() == name)
            .unwrap_or_else(|| panic!("expected visible spec `{name}` in {:?}", self.visible_names))
    }

    fn exposure(&self, name: &str) -> ToolExposure {
        *self
            .exposures
            .get(name)
            .unwrap_or_else(|| panic!("expected registered tool `{name}`"))
    }
}

async fn probe_with(
    configure_turn: impl FnOnce(&mut TurnContext),
    inputs: ToolPlanInputs,
) -> ToolPlanProbe {
    let (_session, mut turn) = make_session_and_context().await;
    configure_turn(&mut turn);
    ToolPlanProbe::from_router(plan_with_model(&turn, turn.model_info(), inputs))
}

fn plan_with_model(
    turn: &TurnContext,
    model_info: &ModelInfo,
    inputs: ToolPlanInputs,
) -> ToolRouter {
    let mcp = codex_mcp::McpBinding::empty(mcp_config_for_test(&turn.config));
    let mut registry = build_core_tool_registry(turn, model_info, &turn.environments, &mcp);
    let hosted_specs = append_source_tools(
        turn,
        model_info,
        &mut registry,
        inputs.tool_runtimes,
        &inputs.dynamic_tools,
    );
    ToolRouter::from_registry(
        turn,
        model_info,
        registry,
        hosted_specs,
        &Default::default(),
    )
}

async fn probe(configure_turn: impl FnOnce(&mut TurnContext)) -> ToolPlanProbe {
    probe_with(configure_turn, ToolPlanInputs::default()).await
}

#[tokio::test]
async fn catalog_models_without_tool_support_receive_no_tool_router() {
    let (session, mut turn) = make_session_and_context().await;
    update_turn_settings_for_test(&mut turn, |settings| {
        Arc::make_mut(&mut settings.model_info).supports_tool_calls = Some(false);
    });
    let model_info = turn.model_info().clone();
    let mcp = Arc::new(codex_mcp::McpBinding::empty(mcp_config_for_test(
        &turn.config,
    )));

    let router = super::build_tool_router(
        &session,
        &turn,
        &model_info,
        model_info.model_messages.as_ref(),
        &turn.environments,
        &mcp,
    )
    .expect("unsupported models should still produce a usable no-tools plan");
    let plan = ToolPlanProbe::from_router(router);

    assert!(plan.visible_specs.is_empty());
    assert!(plan.registered_names.is_empty());
    assert!(!plan.has_terminal_controls);
    assert!(!plan.requires_code_mode_worker);
    assert_eq!(plan.tool_mode, ToolMode::Direct);
}

fn set_feature(turn: &mut TurnContext, feature: Feature, enabled: bool) {
    let mut config = (*turn.config).clone();
    if enabled {
        config
            .features
            .enable(feature)
            .expect("test feature should be enableable in config");
    } else {
        config
            .features
            .disable(feature)
            .expect("test feature should be disableable in config");
    }
    turn.multi_agent_version = config.multi_agent_version_from_features();
    turn.config = Arc::new(config);
}

fn set_features(turn: &mut TurnContext, features: &[Feature]) {
    for feature in features {
        set_feature(turn, *feature, /*enabled*/ true);
    }
}

fn update_config(turn: &mut TurnContext, update: impl FnOnce(&mut crate::config::Config)) {
    let mut config = (*turn.config).clone();
    update(&mut config);
    turn.config = Arc::new(config);
}

fn set_web_search_mode(turn: &mut TurnContext, mode: WebSearchMode) {
    update_config(turn, |config| {
        config
            .web_search_mode
            .set(mode)
            .expect("test web search mode should be accepted");
    });
}

fn use_backend_auth(turn: &mut TurnContext) {
    turn.auth_manager = Some(AuthManager::from_auth_for_testing(
        AheadAuth::create_dummy_headers_auth_for_testing(),
    ));
    turn.provider = create_model_provider(
        turn.config.model_provider.clone(),
        turn.auth_manager.clone(),
    );
}

fn use_bedrock_provider(turn: &mut TurnContext) {
    let provider_info = ModelProviderInfo::create_amazon_bedrock_provider(/*aws*/ None);
    update_config(turn, |config| {
        config.model_provider_id = AMAZON_BEDROCK_PROVIDER_ID.to_string();
        config.model_provider = provider_info.clone();
    });
    turn.provider = create_model_provider(provider_info, turn.auth_manager.clone());
}

fn duplicate_primary_environment(turn: &mut TurnContext) {
    let mut second_environment = turn
        .environments
        .primary()
        .expect("primary environment")
        .clone();
    second_environment.selection.environment_id = "secondary".to_string();
    turn.environments
        .environments
        .push(TurnEnvironmentState::Ready(second_environment));
}

fn mcp_tool(server: &str, namespace: &str, name: &str) -> ToolInfo {
    ToolInfo {
        server_name: server.to_string(),
        supports_parallel_tool_calls: false,
        server_origin: None,
        callable_name: name.to_string(),
        callable_namespace: namespace.to_string(),
        namespace_description: Some(format!("Tools from {server}.")),
        tool: rmcp::model::Tool::new(
            name.to_string(),
            format!("{name} test tool"),
            Arc::new(rmcp::model::object(json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false,
            }))),
        ),
    }
}

fn mcp_runtime(
    server: &str,
    namespace: &str,
    name: &str,
    exposure: ToolExposure,
) -> RegisteredTool {
    let handler: Arc<dyn CoreToolRuntime> = Arc::new(
        McpHandler::new(mcp_tool(server, namespace, name)).expect("MCP tool spec should build"),
    );
    RegisteredTool {
        runtime: handler,
        exposure,
    }
}

fn dynamic_tool(namespace: Option<&str>, name: &str, defer_loading: bool) -> DynamicToolSpec {
    let function = codex_protocol::dynamic_tools::DynamicToolFunctionSpec {
        name: name.to_string(),
        description: format!("{name} dynamic tool"),
        input_schema: json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false,
        }),
        supports_parallel_tool_calls: false,
        defer_loading,
    };
    match namespace {
        Some(namespace) => {
            DynamicToolSpec::Namespace(codex_protocol::dynamic_tools::DynamicToolNamespaceSpec {
                name: namespace.to_string(),
                description: format!("{namespace} dynamic tools"),
                tools: vec![
                    codex_protocol::dynamic_tools::DynamicToolNamespaceTool::Function(function),
                ],
            })
        }
        None => DynamicToolSpec::Function(function),
    }
}

fn has_parameter(spec: &ToolSpec, parameter_name: &str) -> bool {
    serde_json::to_value(spec)
        .expect("tool spec should serialize")
        .pointer(&format!("/parameters/properties/{parameter_name}"))
        .is_some()
}

fn has_windows_shell_guidance(spec: &ToolSpec) -> bool {
    let ToolSpec::Function(tool) = spec else {
        return false;
    };
    tool.description.contains("Windows safety rules:")
}

#[tokio::test]
async fn request_user_input_tool_respects_experimental_config_gate() {
    let enabled = probe(|_| {}).await;
    enabled.assert_visible_contains(&["request_user_input"]);
    enabled.assert_registered_contains(&["request_user_input"]);
    assert_eq!(
        enabled.exposure("request_user_input"),
        ToolExposure::DirectModelOnly
    );

    let disabled = probe(|turn| {
        update_config(turn, |config| {
            config.experimental_request_user_input_enabled = false;
        });
    })
    .await;
    disabled.assert_visible_lacks(&["request_user_input"]);
    disabled.assert_registered_lacks(&["request_user_input"]);
}

#[tokio::test]
async fn update_plan_tool_respects_config_gate() {
    let disabled = probe(|_| {}).await;
    disabled.assert_visible_lacks(&["update_plan"]);
    disabled.assert_registered_lacks(&["update_plan"]);

    let enabled = probe(|turn| {
        update_config(turn, |config| {
            config.update_plan_enabled = true;
        });
    })
    .await;
    enabled.assert_visible_contains(&["update_plan"]);
    enabled.assert_registered_contains(&["update_plan"]);
}

#[tokio::test]
async fn request_user_input_stays_direct_in_code_mode_only() {
    let plan = probe(|turn| {
        set_features(turn, &[Feature::CodeMode, Feature::CodeModeOnly]);
    })
    .await;

    plan.assert_visible_contains(&[
        "request_user_input",
        codex_code_mode::PUBLIC_TOOL_NAME,
        codex_code_mode::WAIT_TOOL_NAME,
    ]);
    plan.assert_registered_contains(&["request_user_input"]);
    assert_eq!(
        plan.exposure("request_user_input"),
        ToolExposure::DirectModelOnly
    );

    let ToolSpec::Freeform(exec) = plan.visible_spec(codex_code_mode::PUBLIC_TOOL_NAME) else {
        panic!("expected code mode exec tool");
    };
    assert!(!exec.description.contains("request_user_input"));
}

#[tokio::test]
async fn shell_family_registers_only_unified_exec_tools() {
    let plan = probe(|turn| {
        set_features(turn, &[Feature::ShellTool]);
        update_turn_settings_for_test(turn, |settings| {
            Arc::make_mut(&mut settings.model_info).shell_type = ConfigShellToolType::UnifiedExec;
        });
    })
    .await;

    plan.assert_visible_contains(&["exec_command", "write_stdin"]);
    plan.assert_registered_contains(&["exec_command", "write_stdin"]);
    assert!(plan.has_terminal_controls);
    assert!(has_parameter(plan.visible_spec("exec_command"), "shell"));
}

#[tokio::test]
async fn exec_command_guidance_follows_executor_platform_and_fallbacks() {
    let opposite_host_os = if cfg!(windows) { "linux" } else { "windows" };
    for (platform_os, multiple_environments, expect_windows_guidance) in [
        (Some("windows"), false, true),
        (Some("linux"), false, false),
        (None, false, cfg!(windows)),
        (Some(opposite_host_os), true, cfg!(windows)),
    ] {
        let plan = probe(|turn| {
            set_features(turn, &[Feature::ShellTool, Feature::UnifiedExec]);
            update_turn_settings_for_test(turn, |settings| {
                Arc::make_mut(&mut settings.model_info).shell_type =
                    ConfigShellToolType::UnifiedExec;
            });
            let TurnEnvironmentState::Ready(environment) = turn
                .environments
                .environments
                .first_mut()
                .expect("primary environment")
            else {
                panic!("primary environment should be ready");
            };
            environment.executor_platform_os = platform_os.map(str::to_string);
            if multiple_environments {
                duplicate_primary_environment(turn);
            }
        })
        .await;

        assert_eq!(
            has_windows_shell_guidance(plan.visible_spec("exec_command")),
            expect_windows_guidance,
            "unexpected guidance for executor platform {platform_os:?} with multiple_environments={multiple_environments}"
        );
    }
}

#[tokio::test]
async fn login_shell_parameter_follows_selected_environment() {
    for allow_login_shell in [false, true] {
        let plan = probe(|turn| {
            set_feature(turn, Feature::ShellTool, /*enabled*/ true);
            update_turn_settings_for_test(turn, |settings| {
                Arc::make_mut(&mut settings.model_info).shell_type =
                    ConfigShellToolType::UnifiedExec;
            });
            update_config(turn, |config| {
                config.permissions.allow_login_shell = !allow_login_shell;
            });
            let TurnEnvironmentState::Ready(environment) = turn
                .environments
                .environments
                .first_mut()
                .expect("primary environment")
            else {
                panic!("primary environment should be ready");
            };
            environment.config_mut().allow_login_shell = allow_login_shell;
        })
        .await;

        assert_eq!(
            has_parameter(plan.visible_spec("exec_command"), "login"),
            allow_login_shell
        );
        assert!(plan.has_terminal_controls);
    }
}

#[tokio::test]
async fn login_shell_parameter_follows_primary_environment() {
    let plan = probe(|turn| {
        set_features(turn, &[Feature::ShellTool]);
        update_config(turn, |config| {
            config.permissions.allow_login_shell = false;
        });
        duplicate_primary_environment(turn);
        for (index, environment) in turn.environments.environments.iter_mut().enumerate() {
            let TurnEnvironmentState::Ready(environment) = environment else {
                panic!("environment should be ready");
            };
            environment.config_mut().allow_login_shell = index == 0;
        }
    })
    .await;

    assert!(has_parameter(plan.visible_spec("exec_command"), "login"));
}

#[tokio::test]
async fn disabling_shell_tools_disables_command_tools() {
    let plan = probe(|turn| {
        set_feature(turn, Feature::ShellTool, /*enabled*/ false);
        update_turn_settings_for_test(turn, |settings| {
            Arc::make_mut(&mut settings.model_info).shell_type = ConfigShellToolType::UnifiedExec;
        });
    })
    .await;
    plan.assert_visible_lacks(&["exec_command", "write_stdin"]);
    plan.assert_registered_lacks(&["exec_command", "write_stdin"]);
    assert!(!plan.has_terminal_controls);
}

#[tokio::test]
async fn dynamic_tools_cannot_reclaim_the_reserved_exec_command_name() {
    let plan = probe_with(
        duplicate_primary_environment,
        ToolPlanInputs {
            dynamic_tools: vec![
                dynamic_tool(
                    /*namespace*/ None,
                    "exec_command",
                    /*defer_loading*/ false,
                ),
                dynamic_tool(Some("client"), "exec_command", /*defer_loading*/ false),
            ],
            ..ToolPlanInputs::default()
        },
    )
    .await;

    plan.assert_visible_contains(&["exec_command"]);
    plan.assert_registered_contains(&["exec_command"]);
    plan.assert_visible_contains(&["client"]);
    plan.assert_registered_contains(&[&ToolName::namespaced("client", "exec_command").to_string()]);
    assert_eq!(
        plan.namespace_function_names("client"),
        &["exec_command".to_string()]
    );
}

#[tokio::test]
async fn environment_count_controls_environment_backed_tools() {
    let no_environment = probe(|turn| {
        turn.environments.environments.clear();
        set_feature(turn, Feature::ShellTool, /*enabled*/ true);
        set_feature(turn, Feature::RequestPermissionsTool, /*enabled*/ true);
        update_turn_settings_for_test(turn, |settings| {
            Arc::make_mut(&mut settings.model_info).apply_patch_tool_type =
                Some(ApplyPatchToolType::Freeform);
        });
    })
    .await;
    no_environment.assert_visible_lacks(&[
        "exec_command",
        "write_stdin",
        "apply_patch",
        "view_image",
        "request_permissions",
    ]);
    no_environment.assert_registered_lacks(&[
        "exec_command",
        "write_stdin",
        "apply_patch",
        "view_image",
        "request_permissions",
    ]);
    assert!(!no_environment.has_terminal_controls);

    let multiple_environments = probe(|turn| {
        duplicate_primary_environment(turn);
        set_feature(turn, Feature::ShellTool, /*enabled*/ true);
        set_feature(turn, Feature::RequestPermissionsTool, /*enabled*/ true);
        update_turn_settings_for_test(turn, |settings| {
            Arc::make_mut(&mut settings.model_info).apply_patch_tool_type =
                Some(ApplyPatchToolType::Freeform);
        });
    })
    .await;
    multiple_environments.assert_visible_contains(&[
        "exec_command",
        "apply_patch",
        "view_image",
        "request_permissions",
    ]);
    assert!(multiple_environments.has_terminal_controls);
    assert!(!has_parameter(
        multiple_environments.visible_spec("exec_command"),
        "environment_id"
    ));
    let ToolSpec::Freeform(apply_patch) = multiple_environments.visible_spec("apply_patch") else {
        panic!("expected apply_patch freeform tool");
    };
    assert!(!apply_patch.format.definition.contains("Environment ID"));
    assert!(!has_parameter(
        multiple_environments.visible_spec("view_image"),
        "environment_id"
    ));
    assert!(!has_parameter(
        multiple_environments.visible_spec("request_permissions"),
        "environment_id"
    ));
}

#[tokio::test]
async fn function_call_models_receive_function_schema_apply_patch() {
    let plan = probe(|turn| {
        set_feature(turn, Feature::ShellTool, /*enabled*/ true);
        update_turn_settings_for_test(turn, |settings| {
            Arc::make_mut(&mut settings.model_info).apply_patch_tool_type =
                Some(ApplyPatchToolType::Function);
        });
    })
    .await;

    let ToolSpec::Function(apply_patch) = plan.visible_spec("apply_patch") else {
        panic!("expected apply_patch function tool");
    };
    assert_eq!(apply_patch.name, "apply_patch");
    assert!(!apply_patch.strict);
    assert_eq!(
        apply_patch.parameters.required.as_deref(),
        Some(["patch".to_string()].as_slice())
    );
}

#[tokio::test]
async fn environment_tools_follow_the_step_context() {
    let (_session, mut turn) = make_session_and_context().await;
    update_turn_settings_for_test(&mut turn, |settings| {
        Arc::make_mut(&mut settings.model_info).apply_patch_tool_type =
            Some(ApplyPatchToolType::Freeform);
    });

    let environments = turn.environments.clone();
    turn.environments.environments.clear();
    let turn = Arc::new(turn);
    let mcp = Arc::new(codex_mcp::McpBinding::empty(mcp_config_for_test(
        &turn.config,
    )));

    let plan = ToolPlanProbe::from_router(ToolRouter::from_registry(
        turn.as_ref(),
        turn.model_info(),
        build_core_tool_registry(
            turn.as_ref(),
            turn.model_info(),
            &environments,
            mcp.as_ref(),
        ),
        super::hosted_model_tool_specs(turn.as_ref(), turn.model_info()),
        &Default::default(),
    ));

    plan.assert_visible_contains(&["exec_command", "apply_patch", "view_image"]);
}

#[tokio::test]
async fn sleep_tool_follows_current_time_config() {
    let disabled = probe(|turn| {
        set_feature(turn, Feature::CurrentTimeReminder, /*enabled*/ true);
    })
    .await;
    assert_eq!(disabled.namespace_function_names("clock"), ["curr_time"]);

    let enabled = probe(|turn| {
        set_feature(turn, Feature::CurrentTimeReminder, /*enabled*/ true);
        let mut config = (*turn.config).clone();
        config.current_time_reminder = Some(CurrentTimeReminderConfig {
            sleep_tool: true,
            ..CurrentTimeReminderConfig::default()
        });
        turn.config = Arc::new(config);
    })
    .await;
    assert_eq!(
        enabled.namespace_function_names("clock"),
        ["curr_time", "sleep"]
    );
}

#[tokio::test]
async fn sleep_tool_stays_direct_and_outside_code_mode() {
    for code_mode_only in [false, true] {
        let plan = probe(|turn| {
            set_features(turn, &[Feature::CodeMode, Feature::CurrentTimeReminder]);
            if code_mode_only {
                set_feature(turn, Feature::CodeModeOnly, /*enabled*/ true);
            }
            update_config(turn, |config| {
                config.current_time_reminder = Some(CurrentTimeReminderConfig {
                    sleep_tool: true,
                    ..CurrentTimeReminderConfig::default()
                });
            });
        })
        .await;

        assert!(
            plan.namespace_function_names("clock")
                .iter()
                .any(|name| name == "sleep")
        );
        let sleep_tool_name = ToolName::namespaced("clock", "sleep").to_string();
        assert_eq!(
            plan.exposure(&sleep_tool_name),
            ToolExposure::DirectModelOnly
        );

        let ToolSpec::Freeform(exec) = plan.visible_spec(codex_code_mode::PUBLIC_TOOL_NAME) else {
            panic!("expected code mode exec tool");
        };
        if code_mode_only {
            assert!(exec.description.contains("clock__curr_time"));
        }
        assert!(!exec.description.contains("clock__sleep"));
    }
}

#[tokio::test]
async fn mcp_and_tool_search_follow_direct_and_deferred_tool_exposure() {
    let direct_mcp = probe_with(
        |_| {},
        ToolPlanInputs {
            tool_runtimes: vec![mcp_runtime(
                "direct",
                "mcp__direct",
                "lookup",
                ToolExposure::Direct,
            )],
            ..ToolPlanInputs::default()
        },
    )
    .await;
    assert_eq!(
        direct_mcp.namespace_function_names("mcp__direct"),
        &["lookup".to_string()]
    );

    let searchable_mcp = || ToolPlanInputs {
        tool_runtimes: vec![mcp_runtime(
            "searchable",
            "mcp__searchable",
            "lookup",
            ToolExposure::Deferred,
        )],
        ..ToolPlanInputs::default()
    };

    let missing_model_capability = probe_with(
        |turn| {
            update_turn_settings_for_test(turn, |settings| {
                Arc::make_mut(&mut settings.model_info).supports_search_tool = false;
            });
        },
        searchable_mcp(),
    )
    .await;
    missing_model_capability.assert_visible_lacks(&["tool_search"]);

    let missing_deferred_tools = probe(|turn| {
        set_feature(turn, Feature::Collab, /*enabled*/ false);
        update_turn_settings_for_test(turn, |settings| {
            Arc::make_mut(&mut settings.model_info).supports_search_tool = true;
        });
    })
    .await;
    missing_deferred_tools.assert_visible_lacks(&["tool_search"]);
    missing_deferred_tools.assert_visible_lacks(&[
        "list_mcp_resources",
        "list_mcp_resource_templates",
        "read_mcp_resource",
    ]);

    let bedrock_namespace_capability = probe_with(
        |turn| {
            update_turn_settings_for_test(turn, |settings| {
                Arc::make_mut(&mut settings.model_info).supports_search_tool = true;
            });
            use_bedrock_provider(turn);
        },
        searchable_mcp(),
    )
    .await;
    bedrock_namespace_capability.assert_visible_contains(&["tool_search"]);

    let enabled = probe_with(
        |turn| {
            update_turn_settings_for_test(turn, |settings| {
                Arc::make_mut(&mut settings.model_info).supports_search_tool = true;
            });
        },
        searchable_mcp(),
    )
    .await;
    enabled.assert_visible_contains(&["tool_search"]);
    enabled.assert_registered_contains(&[
        "tool_search",
        &ToolName::namespaced("mcp__searchable", "lookup").to_string(),
    ]);

    let reserved_namespace = probe_with(
        |turn| {
            set_feature(turn, Feature::CodeMode, /*enabled*/ true);
            update_turn_settings_for_test(turn, |settings| {
                Arc::make_mut(&mut settings.model_info).supports_search_tool = true;
            });
        },
        ToolPlanInputs {
            tool_runtimes: vec![
                mcp_runtime(
                    "reserved_direct",
                    "tool_search",
                    "inspect",
                    ToolExposure::Direct,
                ),
                mcp_runtime(
                    "reserved_deferred",
                    "tool_search",
                    "tool_search_tool",
                    ToolExposure::Deferred,
                ),
                mcp_runtime(
                    "searchable",
                    "mcp__searchable",
                    "lookup",
                    ToolExposure::Deferred,
                ),
            ],
            ..ToolPlanInputs::default()
        },
    )
    .await;
    reserved_namespace.assert_visible_contains(&["tool_search"]);
    reserved_namespace.assert_registered_contains(&[
        "tool_search",
        &ToolName::namespaced("mcp__searchable", "lookup").to_string(),
    ]);
    reserved_namespace.assert_registered_lacks(&[
        &ToolName::namespaced("tool_search", "inspect").to_string(),
        &ToolName::namespaced("tool_search", "tool_search_tool").to_string(),
    ]);
    assert!(matches!(
        reserved_namespace.visible_spec("tool_search"),
        ToolSpec::ToolSearch { .. }
    ));
}

#[tokio::test]
async fn tool_namespaces_info_is_opt_in_and_tracks_mcp_exposure() {
    for (enabled, use_responses_lite) in [(false, true), (true, false), (true, true)] {
        let plan = probe_with(
            |turn| {
                update_config(turn, |config| {
                    config.tool_registry.turn_metadata_includes_tool_info = enabled;
                });
                set_feature(turn, Feature::CodeMode, /*enabled*/ true);
                update_turn_settings_for_test(turn, |settings| {
                    Arc::make_mut(&mut settings.model_info).supports_search_tool = true;
                    Arc::make_mut(&mut settings.model_info).use_responses_lite = use_responses_lite;
                });
            },
            ToolPlanInputs {
                tool_runtimes: vec![
                    mcp_runtime("registry", "mcp__registry", "direct", ToolExposure::Direct),
                    mcp_runtime(
                        "registry",
                        "mcp__registry",
                        "deferred",
                        ToolExposure::Deferred,
                    ),
                ],
                ..ToolPlanInputs::default()
            },
        )
        .await;

        let Some(namespaces) = plan.tool_namespaces_info else {
            assert!(
                !enabled || !use_responses_lite,
                "opted-in Responses Lite planning should return namespaces"
            );
            continue;
        };
        assert!(enabled && use_responses_lite);

        let namespace = namespaces
            .get("mcp__registry")
            .expect("MCP namespace should be included");
        assert_eq!(namespace.name, "mcp__registry");
        assert_eq!(
            namespace.functions.get("direct"),
            Some(&TurnToolFunctionInfo {
                name: "direct".to_string(),
                direct: true,
                code_mode_name: Some("mcp__registry__direct".to_string()),
                deferred: false,
                source: TurnToolSource::Mcp {
                    server_name: "registry".to_string(),
                },
            })
        );
        assert_eq!(
            namespace.functions.get("deferred"),
            Some(&TurnToolFunctionInfo {
                name: "deferred".to_string(),
                direct: false,
                code_mode_name: Some("mcp__registry__deferred".to_string()),
                deferred: true,
                source: TurnToolSource::Mcp {
                    server_name: "registry".to_string(),
                },
            })
        );
    }
}

#[tokio::test]
async fn candidate_model_plan_leaves_selected_model_and_inventory_unchanged() {
    let (_session, mut turn) = make_session_and_context().await;
    set_features(&mut turn, &[Feature::ShellTool, Feature::UnifiedExec]);
    update_config(&mut turn, |config| {
        config.tool_registry.turn_metadata_includes_tool_info = true;
    });
    update_turn_settings_for_test(&mut turn, |settings| {
        let model = Arc::make_mut(&mut settings.model_info);
        model.tool_mode = Some(ToolMode::Direct);
        model.use_responses_lite = true;
        model.shell_type = ConfigShellToolType::Disabled;
        model.apply_patch_tool_type = None;
    });
    let selected_model = Arc::clone(turn.model_info());
    let selected = ToolPlanProbe::from_router(plan_with_model(
        &turn,
        turn.model_info(),
        ToolPlanInputs::default(),
    ));
    let selected_inventory = selected
        .tool_namespaces_info
        .clone()
        .expect("selected plan inventory");
    turn.turn_metadata_state
        .set_tool_namespaces_info(selected_inventory.clone());

    let mut candidate_model = selected_model.as_ref().clone();
    candidate_model.tool_mode = Some(ToolMode::CodeModeOnly);
    candidate_model.shell_type = ConfigShellToolType::UnifiedExec;
    candidate_model.apply_patch_tool_type = Some(ApplyPatchToolType::Freeform);
    let candidate = ToolPlanProbe::from_router(plan_with_model(
        &turn,
        &candidate_model,
        ToolPlanInputs::default(),
    ));

    candidate.assert_visible_contains(&["exec", "wait"]);
    candidate.assert_visible_lacks(&["exec_command", "write_stdin", "apply_patch"]);
    candidate.assert_registered_contains(&["exec_command", "write_stdin", "apply_patch"]);
    assert_eq!(candidate.tool_mode, ToolMode::CodeModeOnly);
    assert!(candidate.requires_code_mode_worker);
    assert!(candidate.has_terminal_controls);
    assert_eq!(
        candidate
            .tool_namespaces_info
            .as_ref()
            .expect("candidate inventory")["functions"]
            .functions["apply_patch"],
        TurnToolFunctionInfo {
            name: "apply_patch".to_string(),
            direct: false,
            code_mode_name: Some("apply_patch".to_string()),
            deferred: false,
            source: TurnToolSource::Harness,
        }
    );
    let metadata = turn.turn_metadata_state.to_responses_metadata(
        "installation".to_string(),
        "window".to_string(),
        CodexResponsesRequestKind::Turn,
    );
    assert_eq!(metadata.tool_namespaces_info, Some(selected_inventory));
    assert_eq!(turn.model_info(), &selected_model);
    assert_eq!(
        ToolPlanProbe::from_router(plan_with_model(
            &turn,
            turn.model_info(),
            ToolPlanInputs::default(),
        )),
        selected,
    );
}

#[tokio::test]
async fn strict_namespace_ownership_requires_tool_namespace_inventory_opt_in() {
    for (enabled, second_exposure) in [
        (false, ToolExposure::Direct),
        (true, ToolExposure::Direct),
        (true, ToolExposure::Hidden),
    ] {
        let (_session, mut turn) = make_session_and_context().await;
        update_config(&mut turn, |config| {
            config.tool_registry.error_on_tool_collisions = true;
            config.tool_registry.turn_metadata_includes_tool_info = enabled;
        });
        update_turn_settings_for_test(&mut turn, |settings| {
            Arc::make_mut(&mut settings.model_info).use_responses_lite = true;
        });
        let step_context = StepContext::for_test(Arc::new(turn));
        let mut registry = build_core_tool_registry(
            step_context.turn.as_ref(),
            step_context.turn.model_info(),
            &step_context.environments,
            step_context.mcp.as_ref(),
        );
        let runtimes = [
            ("first", "lookup", ToolExposure::Direct),
            ("second", "list", second_exposure),
        ]
        .into_iter()
        .map(|(server_name, tool_name, exposure)| {
            let mut tool = mcp_tool(server_name, "shared", tool_name);
            tool.namespace_description = Some("Shared tools.".to_string());
            RegisteredTool {
                runtime: Arc::new(McpHandler::new(tool).expect("MCP tool spec should build")),
                exposure,
            }
        })
        .collect();
        let hosted_specs = append_source_tools(
            step_context.turn.as_ref(),
            step_context.turn.model_info(),
            &mut registry,
            runtimes,
            &[],
        );
        let result = super::finalize_tool_router(
            step_context.turn.as_ref(),
            step_context.turn.model_info(),
            registry,
            hosted_specs,
            &Default::default(),
        );

        if enabled && second_exposure != ToolExposure::Hidden {
            let error = result.err().expect("mixed namespace ownership should fail");
            assert!(matches!(
                error.details(),
                CodexErrorDetails::ToolCollision(name) if name == "shared"
            ));
        } else {
            assert!(result.is_ok(), "existing strict behavior should not change");
        }
    }
}

#[tokio::test]
async fn strict_tool_collisions_reject_external_and_synthetic_duplicates() {
    let cases = [
        (
            "functions.update_plan",
            ToolPlanInputs {
                dynamic_tools: vec![dynamic_tool(
                    /*namespace*/ None,
                    "update_plan",
                    /*defer_loading*/ false,
                )],
                ..ToolPlanInputs::default()
            },
            false,
            false,
        ),
        (
            "functions.exec",
            ToolPlanInputs {
                dynamic_tools: vec![dynamic_tool(
                    /*namespace*/ None,
                    codex_code_mode::PUBLIC_TOOL_NAME,
                    /*defer_loading*/ false,
                )],
                ..ToolPlanInputs::default()
            },
            true,
            false,
        ),
        (
            "functions.tool_search",
            ToolPlanInputs {
                tool_runtimes: vec![mcp_runtime(
                    "registry",
                    "mcp__registry",
                    "lookup",
                    ToolExposure::Deferred,
                )],
                dynamic_tools: vec![dynamic_tool(
                    /*namespace*/ None,
                    codex_tools::TOOL_SEARCH_TOOL_NAME,
                    /*defer_loading*/ false,
                )],
                ..ToolPlanInputs::default()
            },
            false,
            true,
        ),
        (
            "tool_search.tool_search_tool",
            ToolPlanInputs {
                tool_runtimes: vec![mcp_runtime(
                    "reserved",
                    "tool_search",
                    "tool_search_tool",
                    ToolExposure::Deferred,
                )],
                ..ToolPlanInputs::default()
            },
            false,
            true,
        ),
        (
            "tool_search.inspect",
            ToolPlanInputs {
                tool_runtimes: vec![
                    mcp_runtime("reserved", "tool_search", "inspect", ToolExposure::Direct),
                    mcp_runtime(
                        "searchable",
                        "mcp__searchable",
                        "lookup",
                        ToolExposure::Deferred,
                    ),
                ],
                ..ToolPlanInputs::default()
            },
            false,
            true,
        ),
    ];

    let namespace_cases = [
        (ToolExposure::Direct, ToolExposure::Direct, false),
        (ToolExposure::Direct, ToolExposure::Deferred, true),
        (ToolExposure::Deferred, ToolExposure::Deferred, true),
        (ToolExposure::Deferred, ToolExposure::Deferred, false),
    ]
    .map(|(first_exposure, second_exposure, search_enabled)| {
        (
            "shared",
            ToolPlanInputs {
                tool_runtimes: vec![
                    mcp_runtime("first", "shared", "lookup", first_exposure),
                    mcp_runtime("second", "shared", "list", second_exposure),
                ],
                ..ToolPlanInputs::default()
            },
            false,
            search_enabled,
        )
    });

    for (expected_name, inputs, code_mode_enabled, search_enabled) in
        cases.into_iter().chain(namespace_cases)
    {
        let (_session, mut turn) = make_session_and_context().await;
        update_config(&mut turn, |config| {
            config.tool_registry.error_on_tool_collisions = true;
            config.update_plan_enabled = true;
        });
        if code_mode_enabled {
            set_feature(&mut turn, Feature::CodeMode, /*enabled*/ true);
        }
        update_turn_settings_for_test(&mut turn, |settings| {
            Arc::make_mut(&mut settings.model_info).supports_search_tool = search_enabled;
        });
        let step_context = StepContext::for_test(Arc::new(turn));
        let mut registry = build_core_tool_registry(
            step_context.turn.as_ref(),
            step_context.turn.model_info(),
            &step_context.environments,
            step_context.mcp.as_ref(),
        );
        let hosted_specs = append_source_tools(
            step_context.turn.as_ref(),
            step_context.turn.model_info(),
            &mut registry,
            inputs.tool_runtimes,
            &inputs.dynamic_tools,
        );

        let error = super::finalize_tool_router(
            step_context.turn.as_ref(),
            step_context.turn.model_info(),
            registry,
            hosted_specs,
            &Default::default(),
        )
        .err()
        .expect("strict tool collision should fail tool planning");
        assert!(matches!(
            error.details(),
            CodexErrorDetails::ToolCollision(name) if name == expected_name
        ));
        assert_eq!(
            error.to_string(),
            format!("duplicate tool: {expected_name}")
        );
    }
}

#[tokio::test]
async fn strict_tool_collisions_allow_multiple_tools_in_one_namespace() {
    let mut undocumented_tool = mcp_tool("shared", "shared", "undocumented");
    undocumented_tool.namespace_description = None;
    let plan = probe_with(
        |turn| {
            update_config(turn, |config| {
                config.tool_registry.error_on_tool_collisions = true;
            });
        },
        ToolPlanInputs {
            tool_runtimes: vec![
                RegisteredTool {
                    runtime: Arc::new(
                        McpHandler::new(undocumented_tool).expect("MCP tool spec should build"),
                    ),
                    exposure: ToolExposure::Direct,
                },
                mcp_runtime("shared", "shared", "lookup", ToolExposure::Direct),
                mcp_runtime("shared", "shared", "list", ToolExposure::Direct),
            ],
            ..ToolPlanInputs::default()
        },
    )
    .await;

    assert_eq!(
        plan.namespace_function_names("shared"),
        ["list", "lookup", "undocumented"]
    );
    let ToolSpec::Namespace(namespace) = plan.visible_spec("shared") else {
        panic!("expected the shared namespace to stay visible");
    };
    assert_eq!(namespace.description, "Tools from shared.");
}

#[tokio::test]
async fn relaxed_tool_collisions_preserve_first_nonempty_namespace_description() {
    for (first_description, second_description, expected_description) in [
        (
            Some("First namespace description."),
            Some("Second namespace description."),
            "First namespace description.",
        ),
        (
            None,
            Some("Second namespace description."),
            "Second namespace description.",
        ),
    ] {
        let runtime = |name, description: Option<&str>| {
            let mut tool = mcp_tool("shared", "shared", name);
            tool.namespace_description = description.map(str::to_string);
            RegisteredTool {
                runtime: Arc::new(McpHandler::new(tool).expect("MCP tool spec should build")),
                exposure: ToolExposure::Direct,
            }
        };
        let plan = probe_with(
            |_| {},
            ToolPlanInputs {
                tool_runtimes: vec![
                    runtime("lookup", first_description),
                    runtime("list", second_description),
                ],
                ..ToolPlanInputs::default()
            },
        )
        .await;

        assert_eq!(plan.namespace_function_names("shared"), ["list", "lookup"]);
        let ToolSpec::Namespace(namespace) = plan.visible_spec("shared") else {
            panic!("expected the shared namespace to stay visible");
        };
        assert_eq!(namespace.description, expected_description);
    }
}

#[tokio::test]
async fn strict_tool_collisions_allow_identical_names_in_different_namespaces() {
    let plan = probe_with(
        |turn| {
            update_config(turn, |config| {
                config.tool_registry.error_on_tool_collisions = true;
            });
            set_features(turn, &[Feature::CodeMode, Feature::CodeModeOnly]);
        },
        ToolPlanInputs {
            dynamic_tools: vec![
                dynamic_tool(Some("first"), "lookup", /*defer_loading*/ false),
                dynamic_tool(Some("second"), "lookup", /*defer_loading*/ false),
            ],
            ..ToolPlanInputs::default()
        },
    )
    .await;

    plan.assert_registered_contains(&[
        &ToolName::namespaced("first", "lookup").to_string(),
        &ToolName::namespaced("second", "lookup").to_string(),
    ]);
}

#[tokio::test]
async fn code_mode_uses_the_first_normalized_tool_identity() {
    for (code_mode_only, winner_exposure, shadow_is_deferred) in [
        (false, ToolExposure::Direct, true),
        (false, ToolExposure::Deferred, false),
        (true, ToolExposure::Direct, true),
        (true, ToolExposure::Deferred, false),
    ] {
        let plan = probe_with(
            |turn| {
                update_config(turn, |config| {
                    config.tool_registry.turn_metadata_includes_tool_info = true;
                });
                set_feature(turn, Feature::CodeMode, /*enabled*/ true);
                if code_mode_only {
                    set_feature(turn, Feature::CodeModeOnly, /*enabled*/ true);
                }
                update_turn_settings_for_test(turn, |settings| {
                    Arc::make_mut(&mut settings.model_info).supports_search_tool = true;
                    Arc::make_mut(&mut settings.model_info).use_responses_lite = true;
                });
            },
            ToolPlanInputs {
                tool_runtimes: vec![mcp_runtime(
                    "winner",
                    "normalized-alias",
                    "lookup",
                    winner_exposure,
                )],
                dynamic_tools: vec![dynamic_tool(
                    Some("normalized_alias"),
                    "lookup",
                    shadow_is_deferred,
                )],
                ..ToolPlanInputs::default()
            },
        )
        .await;

        let winner_name = ToolName::namespaced("normalized-alias", "lookup");
        let shadow_name = ToolName::namespaced("normalized_alias", "lookup");
        plan.assert_registered_contains(&[&winner_name.to_string(), &shadow_name.to_string()]);
        assert_eq!(plan.exposure(&winner_name.to_string()), winner_exposure);
        assert_eq!(
            plan.exposure(&shadow_name.to_string()),
            if shadow_is_deferred {
                ToolExposure::Deferred
            } else {
                ToolExposure::Direct
            },
        );

        let tool_namespaces = plan
            .tool_namespaces_info
            .as_ref()
            .expect("opted-in Responses Lite plan should contain the supported tools");
        assert_eq!(
            plan.code_mode_tool_names.get("normalized_alias__lookup"),
            Some(&winner_name),
        );
        assert_eq!(
            tool_namespaces["normalized-alias"].functions["lookup"]
                .code_mode_name
                .as_deref(),
            Some("normalized_alias__lookup"),
        );
        assert!(
            tool_namespaces
                .get("normalized_alias")
                .and_then(|namespace| namespace.functions.get("lookup"))
                .and_then(|function| function.code_mode_name.as_ref())
                .is_none()
        );

        if !code_mode_only && !shadow_is_deferred {
            let ToolSpec::Namespace(namespace) = plan.visible_spec("normalized_alias") else {
                panic!("expected the shadowed dynamic tool to remain directly visible");
            };
            let [ResponsesApiNamespaceTool::Function(shadow)] = namespace.tools.as_slice() else {
                panic!("expected exactly one shadowed dynamic tool");
            };
            assert_eq!(shadow.description, "lookup dynamic tool");
        }

        let ToolSpec::Freeform(exec) = plan.visible_spec(codex_code_mode::PUBLIC_TOOL_NAME) else {
            panic!("expected code mode exec tool");
        };
        assert!(!exec.description.contains("lookup dynamic tool"));
        assert_eq!(
            exec.description.contains("lookup test tool"),
            code_mode_only && winner_exposure == ToolExposure::Direct,
        );
    }
}

#[tokio::test]
async fn tool_search_cache_rebuilds_when_deferred_sources_change() {
    let cache = ToolSearchHandlerCache::default();

    let (_session, mut first_turn) = make_session_and_context().await;
    update_turn_settings_for_test(&mut first_turn, |settings| {
        Arc::make_mut(&mut settings.model_info).supports_search_tool = true;
    });
    let first_turn = Arc::new(first_turn);
    let first_step_context = StepContext::for_test(Arc::clone(&first_turn));
    let mut first_registry = build_core_tool_registry(
        first_step_context.turn.as_ref(),
        first_step_context.turn.model_info(),
        &first_step_context.environments,
        first_step_context.mcp.as_ref(),
    );
    let first_tool = mcp_runtime("first", "mcp__first", "lookup", ToolExposure::Deferred);
    first_registry.register_external_with_exposure(first_tool.runtime, first_tool.exposure);
    let first_router = ToolRouter::from_registry(
        first_step_context.turn.as_ref(),
        first_step_context.turn.model_info(),
        first_registry,
        super::hosted_model_tool_specs(
            first_step_context.turn.as_ref(),
            first_step_context.turn.model_info(),
        ),
        &cache,
    );
    let first_plan = ToolPlanProbe::from_router(first_router);

    let (_session, mut second_turn) = make_session_and_context().await;
    update_turn_settings_for_test(&mut second_turn, |settings| {
        Arc::make_mut(&mut settings.model_info).supports_search_tool = true;
    });
    let second_turn = Arc::new(second_turn);
    let second_step_context = StepContext::for_test(Arc::clone(&second_turn));
    let mut second_registry = build_core_tool_registry(
        second_step_context.turn.as_ref(),
        second_step_context.turn.model_info(),
        &second_step_context.environments,
        second_step_context.mcp.as_ref(),
    );
    let second_tool = mcp_runtime("second", "mcp__second", "lookup", ToolExposure::Deferred);
    second_registry.register_external_with_exposure(second_tool.runtime, second_tool.exposure);
    let second_router = ToolRouter::from_registry(
        second_step_context.turn.as_ref(),
        second_step_context.turn.model_info(),
        second_registry,
        super::hosted_model_tool_specs(
            second_step_context.turn.as_ref(),
            second_step_context.turn.model_info(),
        ),
        &cache,
    );
    let second_plan = ToolPlanProbe::from_router(second_router);

    let ToolSpec::ToolSearch {
        description: first_description,
        ..
    } = first_plan.visible_spec("tool_search")
    else {
        panic!("expected first tool_search spec");
    };
    assert!(first_description.contains("- first: Tools from first."));
    assert!(!first_description.contains("- second: Tools from second."));

    let ToolSpec::ToolSearch {
        description: second_description,
        ..
    } = second_plan.visible_spec("tool_search")
    else {
        panic!("expected second tool_search spec");
    };
    assert!(second_description.contains("- second: Tools from second."));
    assert!(!second_description.contains("- first: Tools from first."));
}

#[tokio::test]
async fn tool_search_cache_rebuilds_when_deferred_world_state_changes() {
    let cache = ToolSearchHandlerCache::default();

    for world_state_enabled in [false, true, false] {
        let (_session, mut turn) = make_session_and_context().await;
        update_turn_settings_for_test(&mut turn, |settings| {
            Arc::make_mut(&mut settings.model_info).supports_search_tool = true;
        });
        set_feature(
            &mut turn,
            Feature::DeferredToolWorldState,
            world_state_enabled,
        );
        let turn = Arc::new(turn);
        let step_context = StepContext::for_test(Arc::clone(&turn));
        let mut registry = build_core_tool_registry(
            step_context.turn.as_ref(),
            step_context.turn.model_info(),
            &step_context.environments,
            step_context.mcp.as_ref(),
        );
        let tool = mcp_runtime(
            "calendar",
            "mcp__calendar",
            "lookup",
            ToolExposure::Deferred,
        );
        registry.register_external_with_exposure(tool.runtime, tool.exposure);
        let router = ToolRouter::from_registry(
            step_context.turn.as_ref(),
            step_context.turn.model_info(),
            registry,
            super::hosted_model_tool_specs(
                step_context.turn.as_ref(),
                step_context.turn.model_info(),
            ),
            &cache,
        );
        let plan = ToolPlanProbe::from_router(router);
        let ToolSpec::ToolSearch { description, .. } = plan.visible_spec("tool_search") else {
            panic!("expected visible tool_search spec");
        };

        assert_eq!(
            description.contains("- calendar: Tools from calendar."),
            !world_state_enabled,
            "tool search cache should follow the deferred world-state feature"
        );
    }
}

#[tokio::test]
async fn code_mode_only_exposes_code_executor_and_hides_nested_tools() {
    let input = ToolPlanInputs {
        dynamic_tools: vec![dynamic_tool(
            Some("codex_app"),
            "lookup",
            /*defer_loading*/ false,
        )],
        ..ToolPlanInputs::default()
    };
    let plain = probe_with(|_| {}, input).await;
    assert_eq!(
        plain.namespace_function_names("codex_app"),
        &["lookup".to_string()]
    );
    plain.assert_visible_lacks(&[
        codex_code_mode::PUBLIC_TOOL_NAME,
        codex_code_mode::WAIT_TOOL_NAME,
    ]);
    assert_eq!(
        (plain.tool_mode, plain.requires_code_mode_worker),
        (ToolMode::Direct, false)
    );

    let code_mode_only = probe_with(
        |turn| {
            set_features(turn, &[Feature::CodeMode, Feature::CodeModeOnly]);
        },
        ToolPlanInputs {
            dynamic_tools: vec![dynamic_tool(
                Some("codex_app"),
                "lookup",
                /*defer_loading*/ false,
            )],
            ..ToolPlanInputs::default()
        },
    )
    .await;
    code_mode_only.assert_visible_contains(&[
        codex_code_mode::PUBLIC_TOOL_NAME,
        codex_code_mode::WAIT_TOOL_NAME,
    ]);
    assert_eq!(
        (
            code_mode_only.tool_mode,
            code_mode_only.requires_code_mode_worker
        ),
        (ToolMode::CodeModeOnly, true),
    );
    assert_eq!(
        code_mode_only.namespace_function_names("codex_app"),
        Vec::<String>::new().as_slice()
    );

    let unavailable_code_mode = probe_with(
        |turn| {
            set_features(turn, &[Feature::CodeMode, Feature::CodeModeOnly]);
            turn.code_mode_available = false;
        },
        ToolPlanInputs {
            dynamic_tools: vec![dynamic_tool(
                Some("codex_app"),
                "lookup",
                /*defer_loading*/ false,
            )],
            ..ToolPlanInputs::default()
        },
    )
    .await;
    unavailable_code_mode.assert_visible_lacks(&[
        codex_code_mode::PUBLIC_TOOL_NAME,
        codex_code_mode::WAIT_TOOL_NAME,
        "codex_app",
    ]);
    assert_eq!(unavailable_code_mode.tool_mode, ToolMode::CodeModeOnly);
    assert!(!unavailable_code_mode.requires_code_mode_worker);
}

#[tokio::test]
async fn code_mode_config_updates_exec_description() {
    for (configured_yield_time_ms, expected_yield_time_ms) in
        [(None, 30_000), (Some(10_000), 10_000)]
    {
        let plan = probe(|turn| {
            set_features(turn, &[Feature::CodeMode]);
            if let Some(yield_time_ms) = configured_yield_time_ms {
                update_config(turn, |config| {
                    config.code_mode.default_exec_yield_time_ms = yield_time_ms;
                });
            }
        })
        .await;

        let ToolSpec::Freeform(exec) = plan.visible_spec(codex_code_mode::PUBLIC_TOOL_NAME) else {
            panic!("expected code mode exec tool");
        };
        assert!(
            exec.description
                .contains(&format!("Defaults to {expected_yield_time_ms} ms."))
        );
    }
}

#[tokio::test]
async fn code_mode_only_exposes_configured_dynamic_namespace_directly() {
    let plan = probe_with(
        |turn| {
            set_features(turn, &[Feature::CodeMode, Feature::CodeModeOnly]);
            update_turn_settings_for_test(turn, |settings| {
                Arc::make_mut(&mut settings.model_info).supports_search_tool = true;
            });
            update_config(turn, |config| {
                config.code_mode.direct_only_tool_namespaces = vec!["direct_only".to_string()];
            });
        },
        ToolPlanInputs {
            dynamic_tools: vec![dynamic_tool(
                Some("direct_only"),
                "lookup",
                /*defer_loading*/ true,
            )],
            ..ToolPlanInputs::default()
        },
    )
    .await;

    plan.assert_visible_contains(&[
        codex_code_mode::PUBLIC_TOOL_NAME,
        codex_code_mode::WAIT_TOOL_NAME,
        "direct_only",
    ]);
    plan.assert_visible_lacks(&["tool_search"]);
    assert_eq!(
        plan.exposure(&ToolName::namespaced("direct_only", "lookup").to_string()),
        ToolExposure::DirectModelOnly
    );
    let ToolSpec::Namespace(namespace) = plan.visible_spec("direct_only") else {
        panic!("expected direct-only namespace spec");
    };
    let ResponsesApiNamespaceTool::Function(tool) = &namespace.tools[0] else {
        panic!("expected direct-only namespace function tool");
    };
    assert_eq!(tool.defer_loading, None);
    let ToolSpec::Freeform(exec) = plan.visible_spec(codex_code_mode::PUBLIC_TOOL_NAME) else {
        panic!("expected code mode exec tool");
    };
    assert!(!exec.description.contains("direct_only_lookup(args:"));
}

#[tokio::test]
async fn code_mode_only_exposes_default_namespace_tools_directly() {
    let plan = probe(|turn| {
        set_features(turn, &[Feature::CodeMode, Feature::CodeModeOnly]);
        update_config(turn, |config| {
            config.update_plan_enabled = true;
            config.code_mode.direct_only_tool_namespaces = vec!["functions".to_string()];
        });
    })
    .await;

    plan.assert_visible_contains(&["update_plan"]);
    assert_eq!(plan.exposure("update_plan"), ToolExposure::DirectModelOnly);

    let ToolSpec::Freeform(exec) = plan.visible_spec(codex_code_mode::PUBLIC_TOOL_NAME) else {
        panic!("expected code mode exec tool");
    };
    assert!(!exec.description.contains("update_plan(args:"));
}

#[tokio::test]
async fn excluded_deferred_namespaces_do_not_enable_nested_tool_guidance() {
    let plan = probe_with(
        |turn| {
            set_features(turn, &[Feature::CodeMode, Feature::CodeModeOnly]);
            set_feature(turn, Feature::Collab, /*enabled*/ false);
            update_turn_settings_for_test(turn, |settings| {
                Arc::make_mut(&mut settings.model_info).supports_search_tool = true;
            });
            update_config(turn, |config| {
                config.code_mode.excluded_tool_namespaces = vec!["excluded".to_string()];
            });
        },
        ToolPlanInputs {
            dynamic_tools: vec![dynamic_tool(
                Some("excluded"),
                "lookup",
                /*defer_loading*/ true,
            )],
            ..ToolPlanInputs::default()
        },
    )
    .await;

    let ToolSpec::Freeform(exec) = plan.visible_spec(codex_code_mode::PUBLIC_TOOL_NAME) else {
        panic!("expected code mode exec tool");
    };
    assert!(
        !exec
            .description
            .contains("Some deferred nested tools may be omitted")
    );
    plan.assert_registered_contains(&[
        &ToolName::namespaced("excluded", "lookup").to_string(),
        "tool_search",
    ]);
}

#[tokio::test]
async fn code_mode_excludes_default_namespace_tools() {
    let plan = probe(|turn| {
        set_feature(turn, Feature::CodeMode, /*enabled*/ true);
        update_config(turn, |config| {
            config.update_plan_enabled = true;
            config.code_mode.excluded_tool_namespaces = vec!["functions".to_string()];
        });
    })
    .await;

    plan.assert_visible_contains(&["update_plan"]);
    plan.assert_registered_contains(&["update_plan"]);
    assert_eq!(plan.exposure("update_plan"), ToolExposure::Direct);

    let ToolSpec::Freeform(exec) = plan.visible_spec(codex_code_mode::PUBLIC_TOOL_NAME) else {
        panic!("expected code mode exec tool");
    };
    assert!(!exec.description.contains("update_plan(args:"));
}

#[tokio::test]
async fn tool_mode_selector_overrides_feature_flags() {
    let direct = probe(|turn| {
        set_features(turn, &[Feature::CodeMode, Feature::CodeModeOnly]);
        update_turn_settings_for_test(turn, |settings| {
            Arc::make_mut(&mut settings.model_info).tool_mode = Some(ToolMode::Direct);
        });
    })
    .await;
    direct.assert_visible_lacks(&[
        codex_code_mode::PUBLIC_TOOL_NAME,
        codex_code_mode::WAIT_TOOL_NAME,
    ]);
}

#[tokio::test]
async fn hosted_search_coexists_with_mcp_browser_tools() {
    let plan = probe_with(
        |turn| {
            set_web_search_mode(turn, WebSearchMode::Live);
        },
        ToolPlanInputs {
            tool_runtimes: vec![mcp_runtime(
                "browser_collision",
                "web",
                "run",
                ToolExposure::Direct,
            )],
            ..ToolPlanInputs::default()
        },
    )
    .await;

    let ToolSpec::Namespace(namespace) = plan.visible_spec("web") else {
        panic!("expected the winning browser namespace");
    };
    assert_eq!(namespace.description, "Tools from browser_collision.");
    plan.assert_visible_contains(&["web_search"]);
}

#[tokio::test]
async fn hosted_web_search_follows_provider_and_runtime_gates() {
    let live_web_search = probe(|turn| {
        set_web_search_mode(turn, WebSearchMode::Live);
        update_turn_settings_for_test(turn, |settings| {
            Arc::make_mut(&mut settings.model_info).web_search_tool_type =
                WebSearchToolType::TextAndImage;
        });
    })
    .await;
    assert_eq!(
        live_web_search.visible_spec("web_search"),
        &ToolSpec::WebSearch {
            external_web_access: Some(true),
            indexed_web_access: None,
            filters: None,
            user_location: None,
            search_context_size: None,
            search_content_types: Some(vec!["text".to_string(), "image".to_string()]),
        }
    );

    let code_mode_only = probe(|turn| {
        use_backend_auth(turn);
        set_feature(turn, Feature::CodeModeOnly, /*enabled*/ true);
        set_web_search_mode(turn, WebSearchMode::Live);
    })
    .await;
    assert_eq!(
        code_mode_only.visible_names,
        vec![
            // Code-mode entrypoints.
            codex_code_mode::PUBLIC_TOOL_NAME,
            codex_code_mode::WAIT_TOOL_NAME,
            "request_user_input",
            // Hosted Responses tools.
            "web_search",
        ]
    );

    let hosted_web_search_without_browser_tool = probe(|turn| {
        set_web_search_mode(turn, WebSearchMode::Live);
    })
    .await;
    hosted_web_search_without_browser_tool.assert_visible_contains(&["web_search"]);

    let hosted_web_search_with_dynamic_web_run = probe_with(
        |turn| {
            set_web_search_mode(turn, WebSearchMode::Live);
        },
        ToolPlanInputs {
            dynamic_tools: vec![dynamic_tool(
                Some("web"),
                "run",
                /*defer_loading*/ false,
            )],
            ..Default::default()
        },
    )
    .await;
    hosted_web_search_with_dynamic_web_run.assert_visible_contains(&["web", "web_search"]);

    let hosted_web_search_with_mcp_web_run = probe_with(
        |turn| {
            set_web_search_mode(turn, WebSearchMode::Live);
        },
        ToolPlanInputs {
            tool_runtimes: vec![mcp_runtime(
                "web_server",
                "web",
                "run",
                ToolExposure::Direct,
            )],
            ..Default::default()
        },
    )
    .await;
    hosted_web_search_with_mcp_web_run.assert_visible_contains(&["web", "web_search"]);

    let bedrock_cached_web_search = probe(|turn| {
        use_bedrock_provider(turn);
        update_turn_settings_for_test(turn, |settings| {
            Arc::make_mut(&mut settings.model_info).web_search_tool_type = WebSearchToolType::Text;
        });
    })
    .await;
    assert_eq!(
        bedrock_cached_web_search.visible_spec("web_search"),
        &ToolSpec::WebSearch {
            external_web_access: Some(false),
            indexed_web_access: None,
            filters: None,
            user_location: None,
            search_context_size: None,
            search_content_types: None,
        }
    );

    let bedrock_hosted_web_search = probe(|turn| {
        set_web_search_mode(turn, WebSearchMode::Cached);
        use_bedrock_provider(turn);
        update_turn_settings_for_test(turn, |settings| {
            Arc::make_mut(&mut settings.model_info).web_search_tool_type = WebSearchToolType::Text;
        });
    })
    .await;
    bedrock_hosted_web_search.assert_visible_contains(&["web_search"]);
    bedrock_hosted_web_search.assert_visible_lacks(&["web"]);
}
