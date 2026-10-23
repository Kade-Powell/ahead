use anyhow::Result;
use codex_config::types::McpServerEnvVar;
use codex_protocol::shell_environment::is_non_inheritable_env_var;
use std::collections::HashMap;
use std::env;
use std::ffi::OsString;

pub const MCP_CA_CERTIFICATE_ENV_VARS: [&str; 10] = [
    "SSL_CERT_FILE",
    "REQUESTS_CA_BUNDLE",
    "CURL_CA_BUNDLE",
    "NODE_EXTRA_CA_CERTS",
    "GIT_SSL_CAINFO",
    "CARGO_HTTP_CAINFO",
    "PIP_CERT",
    "BUNDLE_SSL_CA_CERT",
    "npm_config_cafile",
    "NPM_CONFIG_CAFILE",
];

pub(crate) fn create_env_for_mcp_server(
    extra_env: Option<HashMap<OsString, OsString>>,
    env_vars: &[McpServerEnvVar],
) -> Result<HashMap<OsString, OsString>> {
    let additional_env_vars = env_vars
        .iter()
        .map(McpServerEnvVar::name)
        .filter(|name| !is_non_inheritable_env_var(name));
    let mut env: HashMap<OsString, OsString> = DEFAULT_ENV_VARS
        .iter()
        .copied()
        .chain(additional_env_vars)
        .filter_map(|var| env::var_os(var).map(|value| (OsString::from(var), value)))
        .collect();
    for name in MCP_CA_CERTIFICATE_ENV_VARS {
        let Some(value) = env::var_os(name) else {
            continue;
        };
        if value.is_empty() {
            continue;
        }
        let value = std::path::absolute(value)?.into_os_string();
        #[cfg(windows)]
        env.retain(|key, _| !key.to_string_lossy().eq_ignore_ascii_case(name));
        env.insert(OsString::from(name), value);
    }
    for (name, value) in extra_env.unwrap_or_default() {
        if cfg!(windows)
            || name.to_str().is_some_and(|name| {
                MCP_CA_CERTIFICATE_ENV_VARS
                    .iter()
                    .any(|ca_name| ca_name.eq_ignore_ascii_case(name))
            })
        {
            env.retain(|key, _| {
                !key.to_string_lossy()
                    .eq_ignore_ascii_case(&name.to_string_lossy())
            });
        }
        env.insert(name, value);
    }
    env.retain(|name, _| {
        name.to_str()
            .is_none_or(|name| !is_non_inheritable_env_var(name))
    });
    Ok(env)
}

pub(crate) fn create_env_overlay_for_executor_mcp_server(
    extra_env: Option<HashMap<OsString, OsString>>,
    env_vars: &[McpServerEnvVar],
) -> HashMap<OsString, OsString> {
    // The managed executor receives only explicitly named host variables and
    // literal config overrides; its standard environment comes from ExecEnvPolicy.
    let mut env: HashMap<OsString, OsString> = env_vars
        .iter()
        .filter_map(|var| env::var_os(var.name()).map(|value| (OsString::from(var.name()), value)))
        .chain(extra_env.unwrap_or_default())
        .collect();
    env.retain(|name, _| {
        name.to_str()
            .is_none_or(|name| !is_non_inheritable_env_var(name))
    });
    env
}

#[cfg(unix)]
pub(crate) const DEFAULT_ENV_VARS: &[&str] = &[
    "HOME",
    "LOGNAME",
    "PATH",
    "SHELL",
    "USER",
    "__CF_USER_TEXT_ENCODING",
    "LANG",
    "LC_ALL",
    "TERM",
    "TMPDIR",
    "TZ",
];

#[cfg(windows)]
pub(crate) const DEFAULT_ENV_VARS: &[&str] =
    codex_protocol::shell_environment::WINDOWS_CORE_ENV_VARS;

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    use std::ffi::OsStr;

    static EXTRA_RMCP_ENV_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct EnvVarGuard {
        key: String,
        original: Option<OsString>,
    }

    impl EnvVarGuard {
        fn set(key: &str, value: impl AsRef<OsStr>) -> Self {
            let original = std::env::var_os(key);
            unsafe {
                std::env::set_var(key, value.as_ref());
            }
            Self {
                key: key.to_string(),
                original,
            }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            if let Some(value) = &self.original {
                unsafe {
                    std::env::set_var(&self.key, value);
                }
            } else {
                unsafe {
                    std::env::remove_var(&self.key);
                }
            }
        }
    }

    #[tokio::test]
    async fn create_env_honors_overrides() {
        let value = "custom".to_string();
        let expected = OsString::from(&value);
        let env = create_env_for_mcp_server(
            Some(HashMap::from([
                (OsString::from("TZ"), expected.clone()),
                (
                    OsString::from("openai_identity_token_file"),
                    OsString::from("/run/identity-token"),
                ),
            ])),
            &[],
        )
        .expect("local MCP env should build");
        assert_eq!(env.get(OsStr::new("TZ")), Some(&expected));
        assert!(!env.contains_key(OsStr::new("openai_identity_token_file")));
    }

    #[test]
    fn create_env_includes_additional_whitelisted_variables() {
        let _serial = EXTRA_RMCP_ENV_TEST_LOCK
            .lock()
            .expect("environment test lock");
        let custom_var = "EXTRA_RMCP_ENV";
        let value = "from-env";
        let expected = OsString::from(value);
        let _guard = EnvVarGuard::set(custom_var, value);
        let env = create_env_for_mcp_server(/*extra_env*/ None, &[custom_var.into()])
            .expect("local MCP env should build");
        assert_eq!(env.get(OsStr::new(custom_var)), Some(&expected));
    }

    #[test]
    fn create_remote_env_overlay_only_forwards_explicit_variables() {
        let _serial = EXTRA_RMCP_ENV_TEST_LOCK
            .lock()
            .expect("environment test lock");
        let default_var = DEFAULT_ENV_VARS[0];
        let custom_var = "EXTRA_REMOTE_RMCP_ENV";
        let custom_value = OsString::from("from-env");
        let _default_guard = EnvVarGuard::set(default_var, "from-default");
        let _custom_guard = EnvVarGuard::set(custom_var, &custom_value);

        let env = create_env_overlay_for_executor_mcp_server(
            Some(HashMap::from([(
                OsString::from("OpenAI_Federation_Rule_Id"),
                OsString::from("rule"),
            )])),
            &[custom_var.into()],
        );

        assert_eq!(
            env,
            HashMap::from([(OsString::from(custom_var), custom_value)])
        );
    }

    #[test]
    fn create_executor_env_overlay_copies_named_host_variables() {
        let _serial = EXTRA_RMCP_ENV_TEST_LOCK
            .lock()
            .expect("environment test lock");
        let local_var = "LOCAL_RMCP_ENV";
        let local_value = OsString::from("from-local-env");
        let _local_guard = EnvVarGuard::set(local_var, &local_value);

        let env = create_env_overlay_for_executor_mcp_server(
            /*extra_env*/ None,
            &[local_var.into()],
        );

        assert_eq!(
            env,
            HashMap::from([(OsString::from(local_var), local_value)])
        );
    }

    #[cfg(unix)]
    #[test]
    fn create_env_preserves_path_when_it_is_not_utf8() {
        let _serial = EXTRA_RMCP_ENV_TEST_LOCK
            .lock()
            .expect("environment test lock");
        use std::os::unix::ffi::OsStrExt;

        let raw_path = std::ffi::OsStr::from_bytes(b"/tmp/codex-\xFF/bin");
        let expected = raw_path.to_os_string();
        let _guard = EnvVarGuard::set("PATH", raw_path);

        let env =
            create_env_for_mcp_server(/*extra_env*/ None, &[]).expect("local MCP env should build");

        assert_eq!(env.get(OsStr::new("PATH")), Some(&expected));
    }
}
