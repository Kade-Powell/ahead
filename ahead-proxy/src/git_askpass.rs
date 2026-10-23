use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::Path,
    process::{Command, Output},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use ahead_rpc::core::{CoreRequest, CoreResponse, CoreRpcHandler};
use anyhow::{Context, Result, anyhow, ensure};
use serde::{Deserialize, Serialize};
#[cfg(unix)]
use std::fs::OpenOptions;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

const ADDRESS_ENV: &str = "AHEAD_GIT_ASKPASS_ADDRESS";
const TOKEN_ENV: &str = "AHEAD_GIT_ASKPASS_TOKEN";
#[cfg(windows)]
const HELPER_ENV: &str = "AHEAD_GIT_ASKPASS_HELPER";
const MAX_MESSAGE_BYTES: usize = 64 * 1024;
const PROMPT_TIMEOUT: Duration = Duration::from_secs(5 * 60);

#[derive(Serialize, Deserialize)]
struct AskPassRequest {
    token: String,
    prompt: String,
}

#[derive(Serialize, Deserialize)]
struct AskPassResponse {
    token: String,
    answer: Option<String>,
}

/// Re-enters the AHEAD executable as Git's askpass program when requested.
pub fn run_if_requested() -> bool {
    let mut arguments = std::env::args_os();
    let _executable = arguments.next();
    let Some(argument) = arguments.next() else {
        return false;
    };
    let prompt = if argument == std::ffi::OsStr::new("--git-askpass") {
        arguments.next()
    } else {
        #[cfg(windows)]
        {
            if std::env::var_os(HELPER_ENV).is_none()
                || argument.to_string_lossy().starts_with('-')
            {
                return false;
            }
            Some(argument)
        }
        #[cfg(not(windows))]
        return false;
    }
    .map(|value| value.to_string_lossy().into_owned())
    .unwrap_or_default();
    match ask_for_credential(&prompt) {
        Ok(Some(answer)) => {
            if let Err(error) = std::io::stdout().write_all(answer.as_bytes()) {
                eprintln!("Could not return Git credential: {error}");
                std::process::exit(1);
            }
            std::process::exit(0);
        }
        Ok(None) => std::process::exit(1),
        Err(error) => {
            eprintln!("AHEAD Git authentication prompt failed: {error:#}");
            std::process::exit(1);
        }
    }
}

/// Runs Git with an in-app askpass bridge unless the user configured one.
pub fn run_git_command(
    command: &mut Command,
    core_rpc: CoreRpcHandler,
    configured_git_askpass: bool,
) -> Result<Output> {
    let (configure_git_askpass, configure_ssh_askpass) = askpass_overrides(
        has_nonempty_environment_variable("GIT_ASKPASS"),
        has_nonempty_environment_variable("SSH_ASKPASS"),
        configured_git_askpass,
    );
    if !configure_git_askpass && !configure_ssh_askpass {
        return command.output().context("Could not run Git command");
    }

    let executable = std::env::current_exe()
        .context("Could not locate the AHEAD executable for Git authentication")?;
    let askpass_program = create_askpass_program(&executable)?;
    let listener = TcpListener::bind("127.0.0.1:0")
        .context("Could not start the Git credential prompt service")?;
    listener
        .set_nonblocking(true)
        .context("Could not configure the Git credential prompt service")?;
    let address = listener.local_addr()?;
    let token = uuid::Uuid::new_v4().to_string();
    let stopped = Arc::new(AtomicBool::new(false));
    let askpass_thread = {
        let stopped = stopped.clone();
        let server_token = token.clone();
        thread::spawn(move || {
            serve_prompts(listener, core_rpc, server_token, stopped)
        })
    };

    command
        .env(ADDRESS_ENV, address.to_string())
        .env(TOKEN_ENV, token)
        .env("GIT_TERMINAL_PROMPT", "0");
    if configure_git_askpass {
        command.env("GIT_ASKPASS", &askpass_program.git_command);
    }
    if configure_ssh_askpass {
        command
            .env("SSH_ASKPASS", &askpass_program.ssh_command)
            .env("SSH_ASKPASS_REQUIRE", "force");
        #[cfg(windows)]
        command.env(HELPER_ENV, "1");
    }

    let output = command.output().context("Could not run Git command");
    stopped.store(true, Ordering::Release);
    let server_result = askpass_thread.join().map_err(|_| {
        anyhow!("Git credential prompt service stopped unexpectedly")
    })?;
    server_result.context("Git credential prompt service failed")?;
    output
}

fn askpass_overrides(
    git_askpass_configured: bool,
    ssh_askpass_configured: bool,
    core_askpass_configured: bool,
) -> (bool, bool) {
    // Git falls back to SSH_ASKPASS after GIT_ASKPASS and core.askPass. Do
    // not install an AHEAD GIT_ASKPASS when the user has configured that
    // fallback; doing so would silently shadow their credential program.
    let configure_git_askpass = !git_askpass_configured
        && !ssh_askpass_configured
        && !core_askpass_configured;
    let configure_ssh_askpass = !ssh_askpass_configured;
    (configure_git_askpass, configure_ssh_askpass)
}

fn has_nonempty_environment_variable(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|value| !value.is_empty())
}

struct AskPassProgram {
    git_command: String,
    ssh_command: String,
    #[cfg(unix)]
    _directory: tempfile::TempDir,
}

fn create_askpass_program(executable: &Path) -> Result<AskPassProgram> {
    #[cfg(unix)]
    {
        let directory = tempfile::Builder::new()
            .prefix("ahead-git-askpass-")
            .tempdir()
            .context("Could not create a private Git askpass directory")?;
        let path = directory.path().join("askpass.sh");
        let script = askpass_script(executable)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .context("Could not create the Git askpass script")?;
        file.write_all(script.as_bytes())?;
        file.sync_all()?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
        let git_command = shell_quote(&path)?;
        let ssh_command = path
            .to_str()
            .context("Git askpass script path is not valid UTF-8")?
            .to_string();
        Ok(AskPassProgram {
            git_command,
            ssh_command,
            _directory: directory,
        })
    }
    #[cfg(windows)]
    {
        let executable = windows_askpass_executable(executable);
        let executable = executable
            .to_str()
            .context("AHEAD executable path is not valid UTF-8")?;
        Ok(AskPassProgram {
            git_command: format!("\"{executable}\" --git-askpass"),
            ssh_command: executable.to_string(),
        })
    }
}

#[cfg(windows)]
fn windows_askpass_executable(app_executable: &Path) -> std::path::PathBuf {
    let askpass_executable = app_executable
        .with_file_name(format!("ahead-askpass{}", std::env::consts::EXE_SUFFIX));
    if askpass_executable.is_file() {
        askpass_executable
    } else {
        app_executable.to_path_buf()
    }
}

#[cfg(unix)]
fn askpass_script(executable: &Path) -> Result<String> {
    let executable = shell_quote(executable)?;
    Ok(format!(
        "#!/bin/sh\nexec {executable} --git-askpass \"$@\"\n"
    ))
}

#[cfg(unix)]
fn shell_quote(path: &Path) -> Result<String> {
    let path = path
        .to_str()
        .context("Git askpass path is not valid UTF-8")?;
    Ok(format!("'{}'", path.replace('\'', "'\\''")))
}

fn serve_prompts(
    listener: TcpListener,
    core_rpc: CoreRpcHandler,
    token: String,
    stopped: Arc<AtomicBool>,
) -> Result<()> {
    while !stopped.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, _)) => {
                if let Err(error) = handle_prompt(stream, &core_rpc, &token) {
                    tracing::warn!(
                        "Git credential prompt request failed: {error:#}"
                    );
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(20));
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn handle_prompt(
    stream: TcpStream,
    core_rpc: &CoreRpcHandler,
    token: &str,
) -> Result<()> {
    stream.set_read_timeout(Some(PROMPT_TIMEOUT))?;
    stream.set_write_timeout(Some(PROMPT_TIMEOUT))?;
    let mut line = String::new();
    let bytes_read = BufReader::new(stream.try_clone()?)
        .take(MAX_MESSAGE_BYTES as u64)
        .read_line(&mut line)?;
    ensure!(
        bytes_read < MAX_MESSAGE_BYTES && line.ends_with('\n'),
        "Git credential prompt exceeded its size limit"
    );
    let request: AskPassRequest = serde_json::from_str(&line)?;
    ensure!(
        request.token == token,
        "invalid Git credential prompt token"
    );
    ensure!(
        request.prompt.len() <= 8 * 1024,
        "Git credential prompt exceeded its size limit"
    );
    let secret = is_secret_prompt(&request.prompt);
    let answer = match core_rpc
        .request_with_timeout(
            CoreRequest::GitCredentialPrompt {
                prompt: request.prompt,
                secret,
            },
            PROMPT_TIMEOUT,
        )
        .map_err(|error| {
            anyhow!("AHEAD credential request failed: {}", error.message)
        })? {
        CoreResponse::GitCredentialAnswer { answer } => answer,
        _ => return Err(anyhow!("unexpected response to Git credential prompt")),
    };
    let response = AskPassResponse {
        token: token.to_string(),
        answer,
    };
    let mut stream = stream;
    serde_json::to_writer(&mut stream, &response)?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    Ok(())
}

fn ask_for_credential(prompt: &str) -> Result<Option<String>> {
    ensure!(
        prompt.len() <= 8 * 1024,
        "Git credential prompt exceeded its size limit"
    );
    let address = parse_loopback_address(
        &std::env::var(ADDRESS_ENV)
            .context("Git credential prompt address is missing")?,
    )?;
    let token = std::env::var(TOKEN_ENV)
        .context("Git credential prompt token is missing")?;
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(5))
        .context("Could not connect to the AHEAD Git credential prompt")?;
    stream.set_read_timeout(Some(PROMPT_TIMEOUT))?;
    stream.set_write_timeout(Some(PROMPT_TIMEOUT))?;
    let request = AskPassRequest {
        token: token.clone(),
        prompt: prompt.to_string(),
    };
    serde_json::to_writer(&mut stream, &request)?;
    stream.write_all(b"\n")?;
    stream.flush()?;

    let mut line = String::new();
    let bytes_read = BufReader::new(stream)
        .take(MAX_MESSAGE_BYTES as u64)
        .read_line(&mut line)?;
    ensure!(
        bytes_read < MAX_MESSAGE_BYTES && line.ends_with('\n'),
        "AHEAD Git credential response exceeded its size limit"
    );
    let response: AskPassResponse = serde_json::from_str(&line)?;
    ensure!(
        response.token == token,
        "invalid Git credential response token"
    );
    Ok(response.answer)
}

fn parse_loopback_address(value: &str) -> Result<SocketAddr> {
    let address: SocketAddr = value
        .parse()
        .context("Git credential prompt address is invalid")?;
    ensure!(
        address.ip().is_loopback(),
        "Git credential prompt address must be local"
    );
    Ok(address)
}

fn is_secret_prompt(prompt: &str) -> bool {
    let prompt = prompt.to_ascii_lowercase();
    ["password", "passphrase", "token"]
        .iter()
        .any(|word| prompt.contains(word))
}

#[cfg(test)]
mod tests {
    use super::{askpass_overrides, is_secret_prompt, parse_loopback_address};
    use ahead_rpc::core::{CoreRequest, CoreResponse, CoreRpc};
    use std::{
        io::{BufRead, BufReader, Write},
        net::{TcpListener, TcpStream},
        thread,
        time::Duration,
    };

    #[cfg(windows)]
    use super::windows_askpass_executable;
    #[cfg(unix)]
    use super::{askpass_script, shell_quote};

    #[test]
    fn credential_prompts_mask_passwords_passphrases_and_tokens() {
        assert!(is_secret_prompt("Password for https://example.test"));
        assert!(is_secret_prompt("Enter passphrase for key"));
        assert!(is_secret_prompt("Personal access token"));
        assert!(!is_secret_prompt("Username for https://example.test"));
    }

    #[test]
    fn credential_bridge_rejects_non_loopback_addresses() {
        assert!(parse_loopback_address("127.0.0.1:4444").is_ok());
        assert!(parse_loopback_address("[::1]:4444").is_ok());
        assert!(parse_loopback_address("192.0.2.10:4444").is_err());
    }

    #[test]
    fn askpass_fallback_preserves_user_configured_precedence() {
        assert_eq!(askpass_overrides(false, false, false), (true, true));
        assert_eq!(askpass_overrides(true, false, false), (false, true));
        assert_eq!(askpass_overrides(false, true, false), (false, false));
        assert_eq!(askpass_overrides(false, false, true), (false, true));
        assert_eq!(askpass_overrides(true, true, true), (false, false));
    }

    #[test]
    fn credential_bridge_round_trips_a_secret_prompt() -> anyhow::Result<()> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        let rpc = super::CoreRpcHandler::new();
        let server_rpc = rpc.clone();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept()?;
            super::handle_prompt(stream, &server_rpc, "bridge-test-token")
        });

        let mut client = TcpStream::connect(address)?;
        serde_json::to_writer(
            &mut client,
            &super::AskPassRequest {
                token: "bridge-test-token".into(),
                prompt: "Password for https://example.test".into(),
            },
        )?;
        client.write_all(b"\n")?;
        client.flush()?;

        let rpc_request = rpc.rx().recv_timeout(Duration::from_secs(2))?;
        let request_id = match rpc_request {
            CoreRpc::Request(
                request_id,
                CoreRequest::GitCredentialPrompt { prompt, secret },
            ) => {
                assert_eq!(prompt, "Password for https://example.test");
                assert!(secret);
                request_id
            }
            _ => anyhow::bail!("bridge sent an unexpected core request"),
        };
        rpc.handle_response(
            request_id,
            Ok(CoreResponse::GitCredentialAnswer {
                answer: Some("temporary-secret".into()),
            }),
        );

        let mut response_line = String::new();
        BufReader::new(client).read_line(&mut response_line)?;
        let response: super::AskPassResponse = serde_json::from_str(&response_line)?;
        assert_eq!(response.token, "bridge-test-token");
        assert_eq!(response.answer.as_deref(), Some("temporary-secret"));
        server
            .join()
            .map_err(|_| anyhow::anyhow!("bridge server thread panicked"))??;
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn windows_ssh_askpass_prefers_the_adjacent_console_helper() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let app = directory.path().join("ahead.exe");
        let helper = directory.path().join("ahead-askpass.exe");
        std::fs::write(&app, b"app").expect("app fixture");
        std::fs::write(&helper, b"helper").expect("askpass fixture");

        assert_eq!(windows_askpass_executable(&app), helper);
        std::fs::remove_file(&helper).expect("remove helper fixture");
        assert_eq!(windows_askpass_executable(&app), app);
    }

    #[cfg(unix)]
    #[test]
    fn askpass_script_quotes_executable_and_forwards_all_prompt_arguments() {
        let executable = std::path::Path::new("/tmp/ahead's build/ahead");
        assert_eq!(
            shell_quote(executable).expect("shell-safe executable path"),
            "'/tmp/ahead'\\''s build/ahead'"
        );
        assert_eq!(
            askpass_script(executable).expect("askpass script"),
            "#!/bin/sh\nexec '/tmp/ahead'\\''s build/ahead' --git-askpass \"$@\"\n"
        );
    }
}
