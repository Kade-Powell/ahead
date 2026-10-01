//! Real end-to-end harness tests.
//!
//! These exercise the actual `codex-acp` adapter process and the real Codex
//! runtime. They are opt-in because they spawn a network-backed agent and
//! require an authenticated Codex install:
//!
//! ```text
//! AHEAD_E2E_HARNESS=1 cargo test -p ahead-proxy --test harness_e2e -- --nocapture
//! ```
//!
//! Set `AHEAD_ACP_COMMAND` / `AHEAD_ACP_ARGS` to test another ACP agent.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ahead_agent::{HarnessClient, HarnessClientConfig, HarnessEvent, HarnessSink};

fn enabled() -> bool {
    std::env::var("AHEAD_E2E_HARNESS").as_deref() == Ok("1")
}

fn config() -> HarnessClientConfig {
    let cwd = std::env::current_dir().unwrap_or_default();
    if let Ok(command) = std::env::var("AHEAD_ACP_COMMAND") {
        let mut config = HarnessClientConfig::new(command, cwd);
        config.args = std::env::var("AHEAD_ACP_ARGS")
            .unwrap_or_default()
            .split_whitespace()
            .map(str::to_string)
            .collect();
        return config;
    }
    HarnessClientConfig::external_agent(cwd)
}

fn sink(events: Arc<Mutex<Vec<HarnessEvent>>>) -> HarnessSink {
    Arc::new(move |event| {
        if let Ok(mut e) = events.lock() {
            e.push(event);
        }
    })
}

fn collected_text(events: &Arc<Mutex<Vec<HarnessEvent>>>) -> String {
    events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            HarnessEvent::AgentDelta { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// The foundation slice: a real streamed turn, cancel, and durable reopen.
#[test]
fn real_harness_streams_cancels_and_reopens() {
    if !enabled() {
        eprintln!("skipping: set AHEAD_E2E_HARNESS=1 to run the real harness test");
        return;
    }
    let events: Arc<Mutex<Vec<HarnessEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let harness = HarnessClient::spawn(&config(), sink(events.clone()))
        .expect("spawn codex-acp");

    let init = harness.initialize().expect("initialize");
    eprintln!("initialize: {}", serde_json::to_string(&init).unwrap());
    assert!(harness.is_running());

    let cwd = std::env::current_dir().unwrap();
    let session_id = harness
        .new_session(&cwd, "agent", None, None)
        .expect("session/new");
    eprintln!("acp session: {session_id}");

    // 1. A real streamed turn produces deltas.
    let stop = harness
        .prompt(&session_id, "Reply with exactly: PONG")
        .expect("prompt");
    let text = collected_text(&events);
    eprintln!("streamed text: {text:?} stop={stop}");
    assert!(
        text.contains("PONG"),
        "expected streamed PONG, got {text:?}"
    );

    // 2. Durable reopen: load the same harness conversation and continue.
    let loaded = harness.load_session(&session_id, &cwd, "agent", None, None);
    assert!(
        loaded.is_ok(),
        "session/load must succeed for reopen: {loaded:?}"
    );
    let stop2 = harness
        .prompt(&session_id, "Reply with exactly: PONG2")
        .expect("prompt after load");
    let text2 = collected_text(&events);
    eprintln!("text after reload: {text2:?} stop={stop2}");
    assert!(text2.contains("PONG2"));

    // 3. Cancellation: a long-running turn stops early.
    events.lock().unwrap().clear();
    let harness = Arc::new(harness);
    let prompt_harness = harness.clone();
    let prompt_session = session_id.clone();
    let started = Instant::now();
    let handle = std::thread::spawn(move || {
        prompt_harness.prompt(
            &prompt_session,
            "Count from 1 to 200, one number per line, thinking carefully between each.",
        )
    });
    std::thread::sleep(Duration::from_millis(2500));
    harness.cancel(&session_id).expect("session/cancel");
    let result = handle.join().expect("prompt thread");
    eprintln!(
        "cancelled turn stopReason={:?} elapsed={:?}",
        result.as_ref().ok(),
        started.elapsed()
    );
    // Either the prompt returns with a cancel stop reason or an error; the
    // important property is that it does not run to completion.
    assert!(started.elapsed() < Duration::from_secs(120));

    harness.shutdown();
}
