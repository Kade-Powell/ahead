use std::{env, ffi::OsStr, path::Path, process::Command};

const PATH_CAPTURE_START: &str = "__AHEAD_LOGIN_PATH_START__";
const PATH_CAPTURE_END: &str = "__AHEAD_LOGIN_PATH_END__";
const LOGIN_PATH_CAPTURED: &str = "AHEAD_INTERNAL_LOGIN_PATH_CAPTURED";

pub fn initialize_login_shell_path() {
    #[cfg(unix)]
    if let Err(error) = initialize_unix_login_shell_path() {
        log::warn!(
            "Could not load login-shell PATH; keeping inherited PATH: {error:#}"
        );
    }
}

#[cfg(unix)]
fn initialize_unix_login_shell_path() -> anyhow::Result<()> {
    use std::os::unix::process::CommandExt;

    if env::var_os(LOGIN_PATH_CAPTURED).is_some() {
        return Ok(());
    }

    let shell = env::var_os("SHELL")
        .filter(|shell| !shell.is_empty())
        .unwrap_or_else(|| {
            if cfg!(target_os = "macos") {
                "/bin/zsh".into()
            } else {
                "/bin/sh".into()
            }
        });
    let command = login_path_command(&shell).ok_or_else(|| {
        anyhow::anyhow!("login-shell PATH capture is unsupported for this shell")
    })?;
    let mut shell_command = Command::new(&shell);
    if let Some(home) = env::var_os("HOME") {
        if Path::new(&home).is_dir() {
            shell_command.current_dir(home);
        }
    }
    let output = shell_command
        .args(["-l", "-i", "-c", command])
        .output()
        .map_err(|error| {
            anyhow::anyhow!("failed to capture PATH from the login shell: {error}")
        })?;
    let output_text = String::from_utf8_lossy(&output.stdout);
    let login_path = parse_login_path(&output_text)
        .ok_or_else(|| anyhow::anyhow!("login shell did not return a valid PATH"))?;

    if !output.status.success() {
        log::warn!("Login shell exited unsuccessfully after returning PATH");
    }

    if env::var_os("PATH").as_deref() == Some(OsStr::new(&login_path)) {
        return Ok(());
    }

    let executable = env::current_exe()?;
    let mut command = Command::new(executable);
    command
        .args(env::args_os().skip(1))
        .env("PATH", login_path)
        .env(LOGIN_PATH_CAPTURED, "1");
    for (name, value) in env::vars_os() {
        if name.to_string_lossy().starts_with("AHEAD_") {
            command.env(name, value);
        }
    }

    Err(command.exec().into())
}

#[cfg(unix)]
fn login_path_command(shell: &OsStr) -> Option<&'static str> {
    match Path::new(shell).file_name()?.to_str()? {
        "bash" | "dash" | "ksh" | "sh" | "zsh" => Some(
            "printf '\\n__AHEAD_LOGIN_PATH_START__%s__AHEAD_LOGIN_PATH_END__\\n' \"$PATH\"",
        ),
        "fish" => Some(
            "printf '\\n__AHEAD_LOGIN_PATH_START__%s__AHEAD_LOGIN_PATH_END__\\n' (string join : $PATH)",
        ),
        _ => None,
    }
}

fn parse_login_path(output: &str) -> Option<String> {
    let after_start = output.rsplit_once(PATH_CAPTURE_START)?.1;
    let path = after_start.split_once(PATH_CAPTURE_END)?.0;
    if path.is_empty() || path.contains(['\0', '\n', '\r']) {
        return None;
    }
    Some(path.to_owned())
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::{PATH_CAPTURE_END, PATH_CAPTURE_START, parse_login_path};

    #[test]
    fn extracts_path_after_login_shell_startup_noise() {
        let output = format!(
            "shell startup notice\n{PATH_CAPTURE_START}/usr/bin:/opt/homebrew/bin{PATH_CAPTURE_END}\n"
        );
        assert_eq!(
            parse_login_path(&output).as_deref(),
            Some("/usr/bin:/opt/homebrew/bin")
        );
    }

    #[test]
    fn rejects_missing_empty_and_multiline_paths() {
        assert_eq!(parse_login_path("no marker"), None);
        assert_eq!(
            parse_login_path(&format!("{PATH_CAPTURE_START}{PATH_CAPTURE_END}")),
            None
        );
        assert_eq!(
            parse_login_path(&format!(
                "{PATH_CAPTURE_START}/usr/bin\n/tmp{PATH_CAPTURE_END}"
            )),
            None
        );
    }

    #[cfg(unix)]
    #[test]
    fn supports_common_posix_shells_and_fish() {
        use super::login_path_command;

        for shell in ["bash", "dash", "ksh", "sh", "zsh", "fish"] {
            assert!(login_path_command(OsStr::new(shell)).is_some());
        }
        assert_eq!(login_path_command(OsStr::new("nu")), None);
    }
}
