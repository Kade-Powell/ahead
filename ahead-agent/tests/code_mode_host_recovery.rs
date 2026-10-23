#[cfg(unix)]
use codex_code_mode::CodeModeSessionProvider;
#[cfg(unix)]
use codex_code_mode::ExecuteRequest;
#[cfg(unix)]
use codex_code_mode::NoopCodeModeSessionDelegate;
#[cfg(unix)]
use codex_code_mode::ProcessOwnedCodeModeSessionProvider;
#[cfg(unix)]
use codex_code_mode::RuntimeResponse;
#[cfg(unix)]
use codex_code_mode::WaitRequest;
#[cfg(unix)]
use std::path::Path;
#[cfg(unix)]
use std::process::Command;
#[cfg(unix)]
use std::sync::Arc;
#[cfg(unix)]
use std::time::Duration;

#[cfg(unix)]
#[tokio::test]
async fn process_host_exit_fails_active_cell_then_reopens_session() {
    use std::os::unix::fs::PermissionsExt;

    let test_executable = std::env::current_exe().expect("test executable path");
    let target_directory = test_executable
        .parent()
        .and_then(Path::parent)
        .expect("Cargo test executable is under target/debug/deps");
    let real_host_program = target_directory.join("ahead-code-mode-host");
    assert!(
        real_host_program.is_file(),
        "build ahead-code-mode-host before running this process test"
    );

    let test_directory =
        tempfile::tempdir().expect("create temporary test directory");
    let pid_file = test_directory.path().join("host-pids");
    let wrapper = test_directory.path().join("ahead-code-mode-host");
    let shell_quote =
        |path: &Path| format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"));
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$$\" >> {}\nexec {} \"$@\"\n",
            shell_quote(&pid_file),
            shell_quote(&real_host_program)
        ),
    )
    .expect("write host wrapper");
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700))
        .expect("make host wrapper executable");

    let provider = ProcessOwnedCodeModeSessionProvider::with_host_program(wrapper);
    let session = provider
        .create_session(Arc::new(NoopCodeModeSessionDelegate))
        .await
        .expect("open initial process-backed session");
    let first_host_pid = wait_for_host_pid_count(&pid_file, 1)
        .await
        .first()
        .copied()
        .expect("initial host PID");
    let started = session
        .execute(ExecuteRequest {
            tool_call_id: "call-active-cell".to_string(),
            enabled_tools: Vec::new(),
            source: "await new Promise(() => {})".to_string(),
            yield_time_ms: Some(1),
            max_output_tokens: None,
        })
        .await
        .expect("start long-running cell");
    let cell_id = started.cell_id.clone();
    let initial =
        tokio::time::timeout(Duration::from_secs(10), started.initial_response())
            .await
            .expect("cell should yield before the host is killed")
            .expect("initial cell response");
    assert!(
        matches!(&initial, RuntimeResponse::Yielded { .. }),
        "{initial:?}"
    );

    let pid_argument = first_host_pid.to_string();
    let kill_status = Command::new("kill")
        .args(["-9", pid_argument.as_str()])
        .status()
        .expect("run kill for the test-owned host");
    assert!(kill_status.success(), "host process should be killed");
    let active_cell_error = tokio::time::timeout(
        Duration::from_secs(10),
        session.wait(WaitRequest {
            cell_id,
            yield_time_ms: 1,
        }),
    )
    .await
    .expect("dead host should resolve the active-cell wait")
    .expect_err("active cell failure must reach the session caller");
    assert!(
        active_cell_error.contains("code-mode"),
        "unexpected active cell failure: {active_cell_error}"
    );

    let recovered = session
        .execute(ExecuteRequest {
            tool_call_id: "call-after-host-exit".to_string(),
            enabled_tools: Vec::new(),
            source: "text('recovered')".to_string(),
            yield_time_ms: Some(10_000),
            max_output_tokens: None,
        })
        .await
        .expect("next request should reopen the logical session on a new host");
    let recovered =
        tokio::time::timeout(Duration::from_secs(10), recovered.initial_response())
            .await
            .expect("recovered cell should complete")
            .expect("recovered cell response");
    assert!(
        matches!(
            &recovered,
            RuntimeResponse::Result {
                error_text: None,
                ..
            }
        ),
        "recovered cell did not complete successfully: {recovered:?}"
    );
    assert_eq!(
        wait_for_host_pid_count(&pid_file, 2).await.len(),
        2,
        "host recovery should launch exactly one replacement process"
    );
    session
        .shutdown()
        .await
        .expect("shutdown recovered session");

    drop(session);
    drop(provider);
}

#[cfg(unix)]
async fn wait_for_host_pid_count(pid_file: &Path, count: usize) -> Vec<u32> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let pids = std::fs::read_to_string(pid_file)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| line.parse().ok())
            .collect::<Vec<_>>();
        if pids.len() >= count {
            return pids;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {count} host processes; observed {pids:?}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
