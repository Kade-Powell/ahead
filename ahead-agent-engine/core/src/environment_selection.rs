use std::collections::HashSet;
use std::fmt;
use std::sync::Arc;

use arc_swap::ArcSwap;
use codex_exec_server::Environment;
use codex_exec_server::EnvironmentInfo;
use codex_exec_server::EnvironmentManager;
use codex_exec_server::ExecServerError;
use codex_exec_server::ExecutorFileSystem;
use codex_protocol::models::PermissionProfile;
use codex_protocol::protocol::EnvironmentConfig;
use codex_protocol::protocol::EnvironmentConfigState;
use codex_protocol::protocol::TurnEnvironmentSelection;
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_utils_path_uri::PathUri;
use futures::FutureExt;
use futures::future::BoxFuture;
use futures::future::Shared;
use tokio::sync::mpsc;
use tracing::Instrument;
use tracing::instrument::WithSubscriber;

use crate::session::turn_context::ShellSnapshotTask;
use crate::session::turn_context::TurnEnvironment;
use crate::shell::Shell;
use crate::shell_snapshot::ShellSnapshot;

/// Records whether a normalized config should follow later thread setting updates.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EnvironmentConfigOrigin {
    Thread,
    Owner,
}

impl EnvironmentConfigOrigin {
    /// Reconstructs the input form so another attachment boundary preserves config ownership.
    pub(crate) fn into_input_selection(
        self,
        mut selection: TurnEnvironmentSelection,
    ) -> TurnEnvironmentSelection {
        if self == Self::Thread {
            selection.config = EnvironmentConfigState::FromThread;
        }
        selection
    }
}

pub(crate) fn default_thread_environment_selections(
    environment_manager: &EnvironmentManager,
    cwd: &AbsolutePathBuf,
    workspace_roots: &[AbsolutePathBuf],
) -> Vec<TurnEnvironmentSelection> {
    let Some(environment_id) = environment_manager.default_environment_id() else {
        return Vec::new();
    };
    vec![TurnEnvironmentSelection {
        environment_id: environment_id.to_string(),
        cwd: PathUri::from_abs_path(cwd),
        workspace_roots: workspace_roots.iter().map(PathUri::from_abs_path).collect(),
        config: EnvironmentConfigState::FromThread,
    }]
}

type TurnEnvironmentResult = Result<ResolvedEnvironment, Arc<ExecServerError>>;
type TurnEnvironmentResolution = Shared<BoxFuture<'static, TurnEnvironmentResult>>;
type PendingConfigurationResult = Result<EnvironmentConfig, String>;

// Shared startup result used to build each turn's environment with its own config
// without restarting the connection or shell resolution.
#[derive(Clone)]
struct ResolvedEnvironment {
    environment: Arc<Environment>,
    shell: Option<Shell>,
    user_home_dir: Option<PathUri>,
    executor_platform_os: Option<String>,
    temporary_directories: Option<Vec<PathUri>>,
    shell_snapshot: ShellSnapshotTask,
    shell_snapshot_v2_supported: bool,
    installed_config: Option<EnvironmentConfig>,
}

#[derive(Clone)]
struct SelectedTurnEnvironment {
    selection: TurnEnvironmentSelection,
    config_origin: EnvironmentConfigOrigin,
    resolution: TurnEnvironmentResolution,
    pending_completion: Option<mpsc::Sender<PendingConfigurationResult>>,
}

#[derive(Clone)]
pub(crate) struct StartingTurnEnvironment {
    pub(crate) selection: TurnEnvironmentSelection,
    config_origin: EnvironmentConfigOrigin,
    resolution: TurnEnvironmentResolution,
}

/// Resolves config once at the attachment boundary and records who owns future updates.
fn resolve_selection_config(
    mut selection: TurnEnvironmentSelection,
    thread_config: &EnvironmentConfig,
) -> (TurnEnvironmentSelection, EnvironmentConfigOrigin) {
    let (config, origin) = match selection.config {
        EnvironmentConfigState::FromThread => (
            EnvironmentConfigState::Ready(thread_config_for_selection(
                &selection.workspace_roots,
                thread_config,
            )),
            EnvironmentConfigOrigin::Thread,
        ),
        config @ (EnvironmentConfigState::Ready(_)
        | EnvironmentConfigState::Pending
        | EnvironmentConfigState::Failed(_)) => (config, EnvironmentConfigOrigin::Owner),
    };
    selection.config = config;
    (selection, origin)
}

fn thread_config_for_selection(
    workspace_roots: &[PathUri],
    thread_config: &EnvironmentConfig,
) -> EnvironmentConfig {
    EnvironmentConfig {
        workspace_roots: workspace_roots.to_vec(),
        ..thread_config.clone()
    }
}

impl SelectedTurnEnvironment {
    fn refresh_thread_config(&mut self, config: &EnvironmentConfig) {
        if self.config_origin == EnvironmentConfigOrigin::Thread {
            self.selection.config = EnvironmentConfigState::Ready(thread_config_for_selection(
                &self.selection.workspace_roots,
                config,
            ));
        }
    }
}

impl fmt::Debug for StartingTurnEnvironment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StartingTurnEnvironment")
            .field("selection", &self.selection)
            .field("resolved", &self.resolution.peek().is_some())
            .finish_non_exhaustive()
    }
}

pub(crate) struct ThreadEnvironments {
    environment_manager: Arc<EnvironmentManager>,
    local_shell: Shell,
    shell_snapshot: ShellSnapshot,
    environments: ArcSwap<Vec<SelectedTurnEnvironment>>,
}

impl ThreadEnvironments {
    pub(crate) fn new(
        environment_manager: Arc<EnvironmentManager>,
        local_shell: Shell,
        thread_environment_config: EnvironmentConfig,
        shell_snapshot: ShellSnapshot,
        current: TurnEnvironmentSnapshot,
    ) -> Self {
        // Reuse only attached environments from the supplied snapshot; drop starting entries.
        let environments = current
            .environments
            .into_iter()
            .filter_map(|environment| {
                let TurnEnvironmentState::Ready(environment) = environment else {
                    return None;
                };
                let selection = environment.selection;
                let config_origin = environment.config_origin;
                let resolution: TurnEnvironmentResolution =
                    futures::future::ready(Ok(ResolvedEnvironment {
                        environment: environment.environment,
                        shell: environment.shell,
                        user_home_dir: environment.user_home_dir,
                        executor_platform_os: environment.executor_platform_os,
                        temporary_directories: environment.temporary_directories,
                        shell_snapshot: environment.shell_snapshot,
                        shell_snapshot_v2_supported: environment.shell_snapshot_v2_supported,
                        installed_config: None,
                    }))
                    .boxed()
                    .shared();
                let mut inherited_environment = SelectedTurnEnvironment {
                    selection,
                    config_origin,
                    resolution,
                    pending_completion: None,
                };
                // Child threads re-infer thread-owned config while preserving owner config.
                inherited_environment.refresh_thread_config(&thread_environment_config);
                Some(inherited_environment)
            })
            .collect();
        Self {
            environment_manager,
            local_shell,
            shell_snapshot,
            environments: ArcSwap::from_pointee(environments),
        }
    }

    pub(crate) fn update_selections(
        &self,
        environments: &[TurnEnvironmentSelection],
        thread_environment_config: &EnvironmentConfig,
    ) {
        let previous = self.environments.load();
        let mut seen_environment_ids = HashSet::with_capacity(environments.len());
        let mut next = Vec::with_capacity(environments.len());
        for selected_environment in environments {
            if !seen_environment_ids.insert(selected_environment.environment_id.as_str()) {
                continue;
            }
            let (selected_environment, config_origin) =
                resolve_selection_config(selected_environment.clone(), thread_environment_config);
            if let Some(environment) = previous.iter().find(|environment| {
                let previous = &environment.selection;
                previous.environment_id == selected_environment.environment_id
                    && previous.cwd == selected_environment.cwd
                    && previous.workspace_roots == selected_environment.workspace_roots
            }) {
                let failed =
                    matches!(
                        environment.selection.config,
                        EnvironmentConfigState::Failed(_)
                    ) || matches!(environment.resolution.clone().now_or_never(), Some(Err(_)));
                let restarting_as_pending =
                    matches!(selected_environment.config, EnvironmentConfigState::Pending)
                        && !matches!(
                            environment.selection.config,
                            EnvironmentConfigState::Pending
                        );

                if !failed && !restarting_as_pending {
                    let mut environment = environment.clone();
                    environment.selection = selected_environment;
                    environment.config_origin = config_origin;
                    next.push(environment);
                    continue;
                }
            }

            let environment_id = &selected_environment.environment_id;
            let Some(environment) = self.environment_manager.get_environment(environment_id) else {
                tracing::warn!("skipping unknown turn environment `{environment_id}`");
                continue;
            };
            let (pending_completion, configuration_ready) =
                if matches!(selected_environment.config, EnvironmentConfigState::Pending) {
                    let (sender, receiver) = mpsc::channel(/*buffer*/ 1);
                    (Some(sender), Some(receiver))
                } else {
                    (None, None)
                };
            let (resolution_task, resolution) = Self::resolve_environment(
                selected_environment.clone(),
                Arc::clone(&environment),
                self.local_shell.clone(),
                self.shell_snapshot.clone(),
                configuration_ready,
            )
            .remote_handle();
            drop(tokio::spawn(
                resolution_task.in_current_span().with_current_subscriber(),
            ));
            let resolution = resolution.boxed().shared();
            let selected = SelectedTurnEnvironment {
                selection: selected_environment,
                config_origin,
                resolution,
                pending_completion,
            };
            next.push(selected);
        }
        let next = Arc::new(next);
        self.environments.store(Arc::clone(&next));

        // Publish owner configuration before waking turns waiting on this attachment.
        for environment in next.iter() {
            let Some(completion) = &environment.pending_completion else {
                continue;
            };
            let result = match &environment.selection.config {
                EnvironmentConfigState::Ready(config) => Ok(config.clone()),
                EnvironmentConfigState::Failed(error) => Err(error.clone()),
                EnvironmentConfigState::FromThread | EnvironmentConfigState::Pending => continue,
            };
            let _ = completion.try_send(result);
        }
    }

    /// Projects canonical selections back to caller input form without losing config ownership.
    pub(crate) fn selections(&self) -> Vec<TurnEnvironmentSelection> {
        self.environments
            .load()
            .iter()
            .map(|environment| {
                environment
                    .config_origin
                    .into_input_selection(environment.selection.clone())
            })
            .collect()
    }

    pub(crate) fn primary_workspace_roots(&self) -> Vec<AbsolutePathBuf> {
        self.environments
            .load()
            .first()
            .map_or_else(Vec::new, |environment| {
                Self::primary_workspace_roots_for(std::slice::from_ref(&environment.selection))
            })
    }

    /// Returns installed owner configuration without treating pending attachments as ready.
    pub(crate) fn primary_config_for(
        selections: &[TurnEnvironmentSelection],
    ) -> Option<&EnvironmentConfig> {
        match &selections.first()?.config {
            EnvironmentConfigState::Ready(config) => Some(config),
            EnvironmentConfigState::FromThread
            | EnvironmentConfigState::Pending
            | EnvironmentConfigState::Failed(_) => None,
        }
    }

    pub(crate) fn primary_workspace_roots_for(
        selections: &[TurnEnvironmentSelection],
    ) -> Vec<AbsolutePathBuf> {
        selections
            .first()
            .map(|selection| {
                selection
                    .workspace_roots
                    .iter()
                    .filter_map(|workspace_root| workspace_root.to_abs_path().ok())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Refreshes attachments whose configuration is inferred from the thread.
    pub(crate) fn update_thread_config(&self, config: &EnvironmentConfig) {
        let environments = self
            .environments
            .load()
            .iter()
            .map(|environment| {
                let mut environment = environment.clone();
                environment.refresh_thread_config(config);
                environment
            })
            .collect();
        self.environments.store(Arc::new(environments));
    }

    #[tracing::instrument(
        name = "environments.resolve",
        skip_all,
        fields(
            environment_id = %selection.environment_id,
            configuration_pending = configuration_ready.is_some(),
        )
    )]
    async fn resolve_environment(
        selection: TurnEnvironmentSelection,
        environment: Arc<Environment>,
        local_shell: Shell,
        shell_snapshot: ShellSnapshot,
        configuration_ready: Option<mpsc::Receiver<PendingConfigurationResult>>,
    ) -> TurnEnvironmentResult {
        if let EnvironmentConfigState::Failed(error) = &selection.config {
            return Err(Arc::new(ExecServerError::Protocol(error.clone())));
        }

        // Wait for this thread attachment's owner configuration, if pending.
        let configuration_ready = async move {
            let Some(mut configuration_ready) = configuration_ready else {
                return Ok(None);
            };
            match configuration_ready.recv().await {
                Some(Ok(config)) => Ok(Some(config)),
                Some(Err(error)) => Err(Arc::new(ExecServerError::Protocol(error))),
                None => Err(Arc::new(ExecServerError::Protocol(
                    "environment configuration was canceled".to_string(),
                ))),
            }
        };
        let installed_config = configuration_ready.await?;
        let executor_platform_os = Some(std::env::consts::OS.to_string());
        let (shell, user_home_dir, temporary_dirs, snapshot_v2) = (
            Some(local_shell),
            PathUri::from_host_native_path("~").ok(),
            Some(EnvironmentInfo::local_temporary_directories()),
            cfg!(unix),
        );
        let task = shell_snapshot
            .build(selection.cwd, shell.clone())
            .boxed()
            .shared();
        drop(tokio::spawn(
            task.clone().in_current_span().with_current_subscriber(),
        ));
        Ok(ResolvedEnvironment {
            environment,
            shell,
            user_home_dir,
            executor_platform_os,
            temporary_directories: temporary_dirs,
            shell_snapshot: task,
            shell_snapshot_v2_supported: snapshot_v2,
            installed_config,
        })
    }

    #[tracing::instrument(
        name = "environments.snapshot",
        skip_all,
        fields(
            environment_count = self.environments.load().len(),
        )
    )]
    pub(crate) async fn snapshot(&self) -> TurnEnvironmentSnapshot {
        let selected = self.environments.load_full();
        let mut environments = Vec::with_capacity(selected.len());
        for environment in selected.iter() {
            if matches!(
                environment.selection.config,
                EnvironmentConfigState::Failed(_)
            ) {
                continue;
            }
            let pending = matches!(
                environment.selection.config,
                EnvironmentConfigState::Pending
            );
            let starting = StartingTurnEnvironment {
                selection: environment.selection.clone(),
                config_origin: environment.config_origin,
                resolution: environment.resolution.clone(),
            };
            let resolved = if pending {
                starting.resolution.clone().now_or_never()
            } else {
                Some(starting.resolution.clone().await)
            };
            if let Some(environment) = TurnEnvironmentState::from_resolution(starting, resolved) {
                environments.push(environment);
            }
        }
        TurnEnvironmentSnapshot { environments }
    }

    pub(crate) fn environment_manager(&self) -> Arc<EnvironmentManager> {
        Arc::clone(&self.environment_manager)
    }
}

#[derive(Clone, Debug)]
pub(crate) enum TurnEnvironmentState {
    Ready(TurnEnvironment),
    Starting(StartingTurnEnvironment),
}

impl TurnEnvironmentState {
    fn from_resolution(
        starting: StartingTurnEnvironment,
        resolved: Option<TurnEnvironmentResult>,
    ) -> Option<Self> {
        match resolved {
            Some(Ok(environment)) => {
                let mut selection = starting.selection;
                if matches!(selection.config, EnvironmentConfigState::Pending) {
                    selection.config = EnvironmentConfigState::Ready(environment.installed_config?);
                }
                let mut turn_environment = TurnEnvironment::new(
                    selection,
                    starting.config_origin,
                    environment.environment,
                    environment.shell,
                );
                turn_environment.executor_platform_os = environment.executor_platform_os;
                turn_environment.shell_snapshot = environment.shell_snapshot;
                turn_environment.shell_snapshot_v2_supported =
                    environment.shell_snapshot_v2_supported;
                turn_environment.user_home_dir = environment.user_home_dir;
                turn_environment.temporary_directories = environment.temporary_directories;
                Some(Self::Ready(turn_environment))
            }
            Some(Err(err)) => {
                tracing::debug!(
                    environment_id = %starting.selection.environment_id,
                    "skipping failed turn environment: {err}"
                );
                None
            }
            None => Some(Self::Starting(starting)),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct TurnEnvironmentSnapshot {
    // Keep ready and starting environments in their original selection order.
    pub(crate) environments: Vec<TurnEnvironmentState>,
}

impl TurnEnvironmentSnapshot {
    /// Promotes completed startup work without adopting newer thread selections.
    pub(crate) fn refresh_readiness(&self) -> Self {
        let environments = self
            .environments
            .iter()
            .filter_map(|environment| match environment {
                TurnEnvironmentState::Ready(environment) => {
                    Some(TurnEnvironmentState::Ready(environment.clone()))
                }
                TurnEnvironmentState::Starting(environment) => {
                    TurnEnvironmentState::from_resolution(
                        environment.clone(),
                        environment.resolution.clone().now_or_never(),
                    )
                }
            })
            .collect();
        Self { environments }
    }

    pub(crate) fn turn_environments(&self) -> impl Iterator<Item = &TurnEnvironment> {
        self.environments.iter().filter_map(|environment| {
            let TurnEnvironmentState::Ready(environment) = environment else {
                return None;
            };
            Some(environment)
        })
    }

    pub(crate) fn starting(&self) -> impl Iterator<Item = &StartingTurnEnvironment> {
        self.environments.iter().filter_map(|environment| {
            let TurnEnvironmentState::Starting(environment) = environment else {
                return None;
            };
            Some(environment)
        })
    }

    pub(crate) fn primary(&self) -> Option<&TurnEnvironment> {
        self.turn_environments().next()
    }

    /// Returns the primary environment's resolved permissions, or the provided fallback.
    pub(crate) fn permission_profile_or_else(
        &self,
        fallback: impl FnOnce() -> PermissionProfile,
    ) -> PermissionProfile {
        self.primary()
            .map(TurnEnvironment::permission_profile_with_workspace_roots)
            .unwrap_or_else(fallback)
    }

    pub(crate) fn local(&self) -> Option<&TurnEnvironment> {
        self.turn_environments().next()
    }

    pub(crate) fn local_environment_cwd(&self) -> Option<AbsolutePathBuf> {
        self.environments
            .iter()
            .find_map(|environment| match environment {
                TurnEnvironmentState::Ready(environment) => environment.cwd().to_abs_path().ok(),
                TurnEnvironmentState::Starting(environment) => {
                    environment.selection.cwd.to_abs_path().ok()
                }
            })
    }

    #[cfg(test)]
    pub(crate) fn primary_environment(&self) -> Option<Arc<codex_exec_server::Environment>> {
        self.primary()
            .map(|environment| Arc::clone(&environment.environment))
    }

    pub(crate) fn to_selections(&self) -> Vec<TurnEnvironmentSelection> {
        self.turn_environments()
            .map(TurnEnvironment::selection)
            .collect()
    }

    pub(crate) fn primary_filesystem(&self) -> Option<Arc<dyn ExecutorFileSystem>> {
        self.primary()
            .map(|environment| environment.environment.get_filesystem())
    }

    pub(crate) fn single_local_environment(&self) -> Option<&TurnEnvironment> {
        if self.starting().next().is_some() {
            return None;
        }
        let mut environments = self.turn_environments();
        let environment = environments.next()?;
        if environments.next().is_some() {
            return None;
        }

        Some(environment)
    }

    pub(crate) fn single_local_environment_cwd(&self) -> Option<AbsolutePathBuf> {
        // TODO(anp): Migrate local-environment consumers to PathUri so this compatibility
        // conversion can be removed.
        self.single_local_environment()?.cwd().to_abs_path().ok()
    }
}

#[cfg(test)]
mod tests {
    use crate::config::PermissionProfileSnapshot;
    use codex_exec_server::Environment;
    use codex_exec_server::EnvironmentManager;
    use codex_exec_server::LOCAL_ENVIRONMENT_ID;
    use codex_protocol::config_types::WindowsSandboxLevel;
    use codex_protocol::models::ActivePermissionProfile;
    use codex_protocol::models::PermissionProfile;
    use codex_protocol::permissions::FileSystemSandboxPolicyContext;
    use codex_protocol::protocol::TurnEnvironmentSelection;
    use codex_utils_absolute_path::AbsolutePathBuf;
    use codex_utils_path_uri::PathUri;
    use pretty_assertions::assert_eq;

    use super::*;

    fn test_environment_config() -> EnvironmentConfig {
        EnvironmentConfig {
            allow_login_shell: true,
            workspace_roots: Vec::new(),
            windows_sandbox_level: WindowsSandboxLevel::Disabled,
            windows_sandbox_private_desktop: true,
            use_legacy_landlock: false,
            permission_profile: PermissionProfileSnapshot::legacy(PermissionProfile::read_only()),
            shell_environment_policy: Default::default(),
            exec_policy: None,
            mcp_policy: None,
            network_policy: None,
        }
    }

    async fn resolve_turn_environments(
        environment_manager: Arc<EnvironmentManager>,
        selections: &[TurnEnvironmentSelection],
    ) -> Arc<ThreadEnvironments> {
        let turn_environments = Arc::new(ThreadEnvironments::new(
            environment_manager,
            crate::shell::default_user_shell(),
            test_environment_config(),
            ShellSnapshot::disabled(),
            TurnEnvironmentSnapshot::default(),
        ));
        turn_environments.update_selections(selections, &test_environment_config());
        turn_environments.snapshot().await;
        turn_environments
    }

    #[tokio::test]
    async fn default_thread_environment_selections_use_manager_default_id() {
        let cwd = AbsolutePathBuf::current_dir().expect("cwd");
        let cwd_uri = PathUri::from_abs_path(&cwd);
        let manager = EnvironmentManager::default_for_tests();

        assert_eq!(
            default_thread_environment_selections(&manager, &cwd, std::slice::from_ref(&cwd)),
            vec![TurnEnvironmentSelection {
                environment_id: LOCAL_ENVIRONMENT_ID.to_string(),
                cwd: cwd_uri.clone(),
                workspace_roots: vec![cwd_uri],
                config: EnvironmentConfigState::FromThread,
            }]
        );
    }

    #[tokio::test]
    async fn default_thread_environment_selections_empty_when_default_disabled() {
        let cwd = AbsolutePathBuf::current_dir().expect("cwd");
        let manager = EnvironmentManager::without_environments();

        assert_eq!(
            default_thread_environment_selections(&manager, &cwd, std::slice::from_ref(&cwd)),
            Vec::<TurnEnvironmentSelection>::new()
        );
    }

    #[tokio::test]
    async fn local_environment_uses_configured_shell_and_login_policy() {
        let cwd = AbsolutePathBuf::current_dir().expect("cwd");
        let local_shell = Shell {
            shell_type: crate::shell::ShellType::Zsh,
            shell_path: std::path::PathBuf::from("/configured/zsh"),
        };
        let expected_config = EnvironmentConfig {
            allow_login_shell: false,
            workspace_roots: Vec::new(),
            windows_sandbox_level: WindowsSandboxLevel::Disabled,
            windows_sandbox_private_desktop: true,
            use_legacy_landlock: false,
            permission_profile: PermissionProfileSnapshot::active_with_profile_workspace_roots(
                PermissionProfile::read_only(),
                ActivePermissionProfile::read_only(),
                vec![cwd.join("profile-root")],
            ),
            shell_environment_policy: Default::default(),
            exec_policy: None,
            mcp_policy: None,
            network_policy: None,
        };
        let turn_environments = ThreadEnvironments::new(
            Arc::new(EnvironmentManager::default_for_tests()),
            local_shell.clone(),
            expected_config.clone(),
            ShellSnapshot::disabled(),
            TurnEnvironmentSnapshot::default(),
        );
        turn_environments.update_selections(
            &[TurnEnvironmentSelection {
                environment_id: LOCAL_ENVIRONMENT_ID.to_string(),
                cwd: PathUri::from_abs_path(&cwd),
                workspace_roots: Vec::new(),
                config: EnvironmentConfigState::FromThread,
            }],
            &expected_config,
        );

        let snapshot = turn_environments.snapshot().await;
        let environment = snapshot.primary().expect("local environment");
        let expected_user_home_dir = PathUri::from_host_native_path("~").ok();
        let expected_temporary_directories = EnvironmentInfo::local_temporary_directories();

        assert_eq!(environment.shell.as_ref(), Some(&local_shell));
        assert_eq!(environment.config(), &expected_config);
        assert_eq!(
            environment
                .sandbox_context(/*additional_permissions*/ None)
                .policy_context()
                .expect("selected environment sandbox context has cwd"),
            FileSystemSandboxPolicyContext {
                cwd: environment.cwd(),
                workspace_roots: &[],
                user_home_dir: expected_user_home_dir.as_ref(),
                temporary_directories: Some(expected_temporary_directories.as_slice()),
            }
        );
    }

    #[tokio::test]
    async fn resolve_environment_selections_keeps_first_duplicate_id() {
        let cwd = AbsolutePathBuf::current_dir().expect("cwd");
        let cwd_uri = PathUri::from_abs_path(&cwd);
        let manager = Arc::new(EnvironmentManager::default_for_tests());
        let first = TurnEnvironmentSelection {
            environment_id: LOCAL_ENVIRONMENT_ID.to_string(),
            cwd: cwd_uri.clone(),
            workspace_roots: Vec::new(),
            config: EnvironmentConfigState::FromThread,
        };

        let resolved = resolve_turn_environments(
            manager,
            &[
                first.clone(),
                TurnEnvironmentSelection {
                    environment_id: LOCAL_ENVIRONMENT_ID.to_string(),
                    cwd: cwd_uri.join("other").expect("other cwd URI"),
                    workspace_roots: Vec::new(),
                    config: EnvironmentConfigState::FromThread,
                },
            ],
        )
        .await;

        assert_eq!(resolved.snapshot().await.to_selections(), vec![first]);
    }

    #[tokio::test]
    async fn resolved_environment_selections_use_first_selection_as_primary() {
        let cwd = AbsolutePathBuf::current_dir().expect("cwd");
        let selected_cwd = cwd.join("selected");
        let selected_cwd_uri = PathUri::from_abs_path(&selected_cwd);
        let manager = Arc::new(EnvironmentManager::default_for_tests());

        let resolved = resolve_turn_environments(
            Arc::clone(&manager),
            &[TurnEnvironmentSelection {
                environment_id: "local".to_string(),
                cwd: selected_cwd_uri,
                workspace_roots: Vec::new(),
                config: EnvironmentConfigState::FromThread,
            }],
        )
        .await;

        let resolved = resolved.snapshot().await;
        assert_eq!(
            resolved
                .primary()
                .expect("primary environment")
                .selection
                .environment_id,
            "local"
        );
        assert_eq!(
            resolved.primary().expect("primary environment").shell,
            Some(
                Shell::from_environment_shell_info(
                    manager
                        .get_environment("local")
                        .expect("local environment")
                        .info()
                        .await
                        .expect("local environment info")
                        .shell
                )
                .expect("resolved shell")
            )
        );
    }

    #[tokio::test]
    async fn unresolved_environment_selections_are_skipped() {
        let cwd = AbsolutePathBuf::current_dir().expect("cwd");
        let cwd_uri = PathUri::from_abs_path(&cwd);
        let manager = Arc::new(EnvironmentManager::default_for_tests());
        let local = TurnEnvironmentSelection {
            environment_id: LOCAL_ENVIRONMENT_ID.to_string(),
            cwd: cwd_uri.clone(),
            workspace_roots: Vec::new(),
            config: EnvironmentConfigState::FromThread,
        };

        let resolved = resolve_turn_environments(
            manager,
            &[
                TurnEnvironmentSelection {
                    environment_id: "missing".to_string(),
                    cwd: cwd_uri,
                    workspace_roots: Vec::new(),
                    config: EnvironmentConfigState::FromThread,
                },
                local.clone(),
            ],
        )
        .await;

        assert_eq!(resolved.snapshot().await.to_selections(), vec![local]);
    }

    #[tokio::test]
    async fn inherited_environment_reuses_parent_handle_and_uses_child_config() {
        let cwd = AbsolutePathBuf::current_dir().expect("cwd");
        let selection = TurnEnvironmentSelection {
            environment_id: LOCAL_ENVIRONMENT_ID.to_string(),
            cwd: PathUri::from_abs_path(&cwd),
            workspace_roots: Vec::new(),
            config: EnvironmentConfigState::FromThread,
        };
        let inherited_environment = Arc::new(Environment::default_for_tests());
        let mut inherited = TurnEnvironment::new(
            TurnEnvironmentSelection {
                config: EnvironmentConfigState::Ready(test_environment_config()),
                ..selection.clone()
            },
            EnvironmentConfigOrigin::Thread,
            Arc::clone(&inherited_environment),
            /*shell*/ None,
        );
        inherited.user_home_dir =
            Some(PathUri::parse("file:///home/inherited").expect("home directory"));
        inherited.temporary_directories = Some(vec![
            PathUri::parse("file:///tmp/inherited").expect("temporary directory"),
        ]);
        let manager = Arc::new(EnvironmentManager::default_for_tests());
        let child_config = EnvironmentConfig {
            allow_login_shell: false,
            workspace_roots: Vec::new(),
            windows_sandbox_level: WindowsSandboxLevel::Disabled,
            windows_sandbox_private_desktop: true,
            use_legacy_landlock: false,
            permission_profile: PermissionProfileSnapshot::active_with_profile_workspace_roots(
                PermissionProfile::read_only(),
                ActivePermissionProfile::read_only(),
                vec![cwd.join("child-profile-root")],
            ),
            shell_environment_policy: Default::default(),
            exec_policy: None,
            mcp_policy: None,
            network_policy: None,
        };
        let environments = ThreadEnvironments::new(
            manager,
            crate::shell::default_user_shell(),
            child_config.clone(),
            ShellSnapshot::disabled(),
            TurnEnvironmentSnapshot {
                environments: vec![TurnEnvironmentState::Ready(inherited)],
            },
        );

        environments.update_selections(std::slice::from_ref(&selection), &child_config);
        let snapshot = environments.snapshot().await;

        let inherited = snapshot.primary().expect("inherited environment");
        assert!(Arc::ptr_eq(&inherited.environment, &inherited_environment));
        assert_eq!(inherited.config(), &child_config);
        assert_eq!(
            inherited
                .sandbox_context(/*additional_permissions*/ None)
                .user_home_dir,
            Some(PathUri::parse("file:///home/inherited").expect("home directory")),
        );
        assert_eq!(
            inherited
                .sandbox_context(/*additional_permissions*/ None)
                .temporary_directories
                .as_deref(),
            Some(
                [PathUri::parse("file:///tmp/inherited").expect("temporary directory")].as_slice()
            )
        );
    }

    #[tokio::test]
    async fn owner_environment_config_is_inherited_and_reset_for_new_cwd() {
        let cwd = AbsolutePathBuf::current_dir().expect("cwd");
        let selection = TurnEnvironmentSelection {
            environment_id: LOCAL_ENVIRONMENT_ID.to_string(),
            cwd: PathUri::from_abs_path(&cwd),
            workspace_roots: Vec::new(),
            config: EnvironmentConfigState::FromThread,
        };
        let manager = Arc::new(EnvironmentManager::default_for_tests());
        let parent =
            resolve_turn_environments(Arc::clone(&manager), std::slice::from_ref(&selection)).await;
        let parent_owner_config = EnvironmentConfig {
            allow_login_shell: false,
            workspace_roots: selection.workspace_roots.clone(),
            windows_sandbox_level: WindowsSandboxLevel::Disabled,
            windows_sandbox_private_desktop: true,
            use_legacy_landlock: false,
            permission_profile: PermissionProfileSnapshot::legacy(PermissionProfile::read_only()),
            shell_environment_policy: Default::default(),
            exec_policy: None,
            mcp_policy: None,
            network_policy: None,
        };
        let mut owner_selection = selection.clone();
        owner_selection.config = EnvironmentConfigState::Ready(parent_owner_config.clone());
        parent.update_selections(
            std::slice::from_ref(&owner_selection),
            &test_environment_config(),
        );

        let child_thread_config = EnvironmentConfig {
            permission_profile: PermissionProfileSnapshot::legacy(
                PermissionProfile::workspace_write(),
            ),
            ..test_environment_config()
        };
        parent.update_thread_config(&child_thread_config);
        assert_eq!(
            parent
                .snapshot()
                .await
                .primary()
                .expect("owner-configured environment")
                .config(),
            &parent_owner_config
        );
        let child = ThreadEnvironments::new(
            manager,
            crate::shell::default_user_shell(),
            child_thread_config.clone(),
            ShellSnapshot::disabled(),
            parent.snapshot().await,
        );
        let child_snapshot = child.snapshot().await;
        let child_environment = child_snapshot.primary().expect("child environment");

        assert_eq!(child_environment.config(), &parent_owner_config);

        let changed_selection = TurnEnvironmentSelection {
            cwd: PathUri::from_abs_path(&cwd.join("changed")),
            ..selection
        };
        let new_thread_config = test_environment_config();
        parent.update_selections(std::slice::from_ref(&changed_selection), &new_thread_config);
        let changed_snapshot = parent.snapshot().await;
        let changed_environment = changed_snapshot.primary().expect("changed environment");
        assert_eq!(changed_environment.config(), &new_thread_config);
    }

    #[tokio::test]
    async fn single_local_environment_cwd_requires_exactly_one_local_environment() {
        let cwd = AbsolutePathBuf::current_dir().expect("cwd");
        let cwd_uri = PathUri::from_abs_path(&cwd);
        let local_manager = Arc::new(EnvironmentManager::default_for_tests());
        let local = resolve_turn_environments(
            Arc::clone(&local_manager),
            &[TurnEnvironmentSelection {
                environment_id: LOCAL_ENVIRONMENT_ID.to_string(),
                cwd: cwd_uri.clone(),
                workspace_roots: Vec::new(),
                config: EnvironmentConfigState::FromThread,
            }],
        )
        .await;
        let local = local.snapshot().await;
        let first = local.primary().expect("local environment").clone();
        let multiple = TurnEnvironmentSnapshot {
            environments: vec![
                TurnEnvironmentState::Ready(first.clone()),
                TurnEnvironmentState::Ready(TurnEnvironment::new(
                    TurnEnvironmentSelection {
                        environment_id: "second-local".to_string(),
                        cwd: PathUri::from_abs_path(&cwd.join("second")),
                        workspace_roots: Vec::new(),
                        config: EnvironmentConfigState::Ready(test_environment_config()),
                    },
                    EnvironmentConfigOrigin::Thread,
                    Arc::clone(&first.environment),
                    /*shell*/ None,
                )),
            ],
        };

        assert_eq!(local.single_local_environment_cwd(), Some(cwd.clone()));
        assert_eq!(multiple.single_local_environment_cwd(), None);
        assert_eq!(multiple.local_environment_cwd(), Some(cwd));
    }

    #[test]
    fn local_environment_cwd_uses_starting_local_selection() {
        let cwd = AbsolutePathBuf::current_dir()
            .expect("cwd")
            .join("starting-local");
        let snapshot = TurnEnvironmentSnapshot {
            environments: vec![TurnEnvironmentState::Starting(StartingTurnEnvironment {
                selection: TurnEnvironmentSelection {
                    environment_id: LOCAL_ENVIRONMENT_ID.to_string(),
                    cwd: PathUri::from_abs_path(&cwd),
                    workspace_roots: Vec::new(),
                    config: EnvironmentConfigState::Ready(test_environment_config()),
                },
                config_origin: EnvironmentConfigOrigin::Thread,
                resolution: futures::future::pending().boxed().shared(),
            })],
        };

        assert_eq!(snapshot.local_environment_cwd(), Some(cwd));
    }
}
