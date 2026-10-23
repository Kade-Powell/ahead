use std::sync::Arc;

use crate::ExecServerError;
use crate::ExecutorFileSystem;
use crate::LocalAgentRuntimePaths;
use crate::local_file_system::LocalFileSystem;
use crate::local_process::LocalProcess;
use crate::process::ExecBackend;
use crate::protocol::EnvironmentInfo;

pub const LOCAL_ENVIRONMENT_ID: &str = "local";

/// Owns the editor-local execution environment.
#[derive(Debug)]
pub struct EnvironmentManager {
    local_environment: Option<Arc<Environment>>,
}

impl EnvironmentManager {
    /// Builds the editor-local execution environment without reading Codex files or environment variables.
    pub fn local_only(local_runtime_paths: LocalAgentRuntimePaths) -> Self {
        let local_environment = Arc::new(Environment::local(local_runtime_paths));
        Self {
            local_environment: Some(local_environment),
        }
    }

    /// Builds a test-only manager without configured sandbox helper paths.
    pub fn default_for_tests() -> Self {
        let local_environment = Arc::new(Environment::default_for_tests());
        Self {
            local_environment: Some(local_environment),
        }
    }

    /// Builds a manager with no configured execution environment.
    pub fn without_environments() -> Self {
        Self {
            local_environment: None,
        }
    }

    pub fn default_environment(&self) -> Option<Arc<Environment>> {
        self.local_environment.as_ref().map(Arc::clone)
    }

    pub fn default_environment_id(&self) -> Option<&str> {
        self.local_environment
            .as_ref()
            .map(|_| LOCAL_ENVIRONMENT_ID)
    }

    pub fn get_environment(&self, environment_id: &str) -> Option<Arc<Environment>> {
        if environment_id == LOCAL_ENVIRONMENT_ID {
            self.local_environment.as_ref().map(Arc::clone)
        } else {
            None
        }
    }
}

/// Concrete local execution/filesystem capabilities selected for a session.
#[derive(Clone)]
pub struct Environment {
    exec_backend: Arc<dyn ExecBackend>,
    filesystem: Arc<dyn ExecutorFileSystem>,
}

impl Environment {
    /// Builds a test-only local environment without configured sandbox helper paths.
    pub fn default_for_tests() -> Self {
        Self {
            exec_backend: Arc::new(LocalProcess::default()),
            filesystem: Arc::new(LocalFileSystem::unsandboxed()),
        }
    }

    pub(crate) fn local(local_runtime_paths: LocalAgentRuntimePaths) -> Self {
        Self {
            exec_backend: Arc::new(LocalProcess::with_local_runtime_paths(
                local_runtime_paths.clone(),
            )),
            filesystem: Arc::new(LocalFileSystem::with_runtime_paths(local_runtime_paths)),
        }
    }

    pub async fn info(&self) -> Result<EnvironmentInfo, ExecServerError> {
        Ok(EnvironmentInfo::local())
    }

    pub fn get_exec_backend(&self) -> Arc<dyn ExecBackend> {
        Arc::clone(&self.exec_backend)
    }

    pub fn get_filesystem(&self) -> Arc<dyn ExecutorFileSystem> {
        Arc::clone(&self.filesystem)
    }
}

impl std::fmt::Debug for Environment {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Environment")
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use codex_utils_path_uri::PathUri;
    use pretty_assertions::assert_eq;

    use super::EnvironmentManager;
    use super::LOCAL_ENVIRONMENT_ID;
    use crate::LocalAgentRuntimePaths;

    fn test_runtime_paths() -> LocalAgentRuntimePaths {
        LocalAgentRuntimePaths::new(
            std::env::current_exe().expect("current exe"),
            /*ahead_linux_sandbox_exe*/ None,
        )
        .expect("runtime paths")
    }

    #[test]
    fn local_environment_info_includes_current_directory() {
        let info = super::EnvironmentInfo::local();

        assert_eq!(
            info.cwd,
            Some(
                PathUri::from_host_native_path(std::env::current_dir().expect("current directory"))
                    .expect("cwd URI")
            )
        );
        assert_eq!(
            info.temp_dir,
            PathUri::from_host_native_path(std::env::temp_dir()).ok()
        );
    }

    #[test]
    fn local_only_manager_exposes_one_shared_editor_environment() {
        let manager = EnvironmentManager::local_only(test_runtime_paths());
        let default = manager.default_environment().expect("default environment");
        let local = manager
            .get_environment(LOCAL_ENVIRONMENT_ID)
            .expect("local environment");

        assert_eq!(manager.default_environment_id(), Some(LOCAL_ENVIRONMENT_ID));
        assert!(std::sync::Arc::ptr_eq(&default, &local));
        assert!(manager.get_environment("remote").is_none());
    }

    #[test]
    fn empty_manager_has_no_execution_environment() {
        let manager = EnvironmentManager::without_environments();

        assert!(manager.default_environment().is_none());
        assert!(manager.default_environment_id().is_none());
        assert!(manager.get_environment(LOCAL_ENVIRONMENT_ID).is_none());
    }
}
