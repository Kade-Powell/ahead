#![allow(warnings, clippy::all)]

use super::*;
use crate::ResponseItemEnvelope;
use crate::RolloutItem;
use crate::RolloutLine;
use crate::config::RolloutConfig;
use chrono::TimeZone;
use codex_protocol::SanitizedGitUrl;
use codex_protocol::SessionId;
use codex_protocol::ThreadId;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::AgentMessageEvent;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::HistoryPosition;
use codex_protocol::protocol::SandboxPolicy;
use codex_protocol::protocol::SessionMeta;
use codex_protocol::protocol::SessionMetaLine;
use codex_protocol::protocol::SessionSource;
use codex_protocol::protocol::ThreadHistoryMode;
use codex_protocol::protocol::TurnContextItem;
use codex_protocol::protocol::UserMessageEvent;
use codex_protocol::security_risk::SecurityRiskScore;
use codex_utils_absolute_path::test_support::PathExt;
use pretty_assertions::assert_eq;
use std::collections::BTreeMap;
use std::fs;
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
use tempfile::TempDir;
use uuid::Uuid;

fn test_config(codex_home: &Path) -> RolloutConfig {
    RolloutConfig {
        codex_home: codex_home.to_path_buf(),
        cwd: codex_home.to_path_buf(),
        model_provider_id: "test-provider".to_string(),
        generate_memories: true,
    }
}

fn paginated_session_meta_item(thread_id: ThreadId, cwd: &Path) -> RolloutItem {
    RolloutItem::SessionMeta(SessionMetaLine {
        meta: SessionMeta {
            session_id: thread_id.into(),
            id: thread_id,
            timestamp: "2026-07-09T00:00:00Z".to_string(),
            cwd: cwd.to_path_buf(),
            originator: "test".to_string(),
            cli_version: "test".to_string(),
            source: SessionSource::Exec,
            history_mode: ThreadHistoryMode::Paginated,
            ..SessionMeta::default()
        },
        git: None,
    })
}

fn agent_message_item(message: &str) -> RolloutItem {
    RolloutItem::EventMsg(EventMsg::AgentMessage(AgentMessageEvent {
        message: message.to_string(),
        phase: None,
        memory_citation: None,
        delivery: None,
    }))
}

fn write_paginated_rollout(
    path: &Path,
    thread_id: ThreadId,
    subsequent_ordinals: &[u64],
) -> std::io::Result<()> {
    let mut records = vec![RolloutLine {
        timestamp: "2026-07-09T00:00:00Z".to_string(),
        ordinal: Some(0),
        item: paginated_session_meta_item(thread_id, path.parent().unwrap_or(path)),
    }];
    records.extend(
        subsequent_ordinals
            .iter()
            .enumerate()
            .map(|(index, ordinal)| RolloutLine {
                timestamp: format!("2026-07-09T00:00:{:02}Z", index + 1),
                ordinal: Some(*ordinal),
                item: agent_message_item(format!("message-{index}").as_str()),
            }),
    );
    let jsonl = records
        .iter()
        .map(serde_json::to_string)
        .collect::<Result<Vec<_>, _>>()?
        .join("\n");
    fs::write(path, format!("{jsonl}\n"))
}

fn read_rollout_lines(path: &Path) -> std::io::Result<Vec<RolloutLine>> {
    fs::read_to_string(path)?
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).map_err(std::io::Error::other))
        .collect()
}

fn write_session_file(root: &Path, ts: &str, uuid: Uuid) -> std::io::Result<PathBuf> {
    let day_dir = root.join("sessions/2025/01/03");
    fs::create_dir_all(&day_dir)?;
    let path = day_dir.join(format!("rollout-{ts}-{uuid}.jsonl"));
    let mut file = File::create(&path)?;
    let meta = serde_json::json!({
        "timestamp": ts,
        "type": "session_meta",
        "payload": {
            "session_id": uuid,
            "id": uuid,
            "timestamp": ts,
            "cwd": ".",
            "originator": "test_originator",
            "cli_version": "test_version",
            "source": "cli",
            "model_provider": "test-provider",
        },
    });
    writeln!(file, "{meta}")?;
    let user_event = serde_json::json!({
        "timestamp": ts,
        "type": "event_msg",
        "payload": {
            "type": "user_message",
            "message": "Hello from user",
            "kind": "plain",
        },
    });
    writeln!(file, "{user_event}")?;
    Ok(path)
}

#[test]
fn append_repair_terminates_nonempty_rollout_tail() -> std::io::Result<()> {
    let home = TempDir::new().expect("temp dir");
    let rollout_path = home.path().join("rollout.jsonl");
    fs::write(&rollout_path, b"{\"type\":\"event_msg\"}")?;
    drop(open_log_file(&rollout_path)?);
    drop(open_log_file(&rollout_path)?);

    assert_eq!(fs::read(&rollout_path)?, b"{\"type\":\"event_msg\"}\n");
    Ok(())
}

#[tokio::test]
async fn opening_existing_rollout_preserves_modified_time() -> std::io::Result<()> {
    let home = TempDir::new().expect("temp dir");
    let rollout_path = home.path().join("rollout.jsonl");
    write_paginated_rollout(&rollout_path, ThreadId::default(), &[])?;
    let modified = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    File::options()
        .write(true)
        .open(&rollout_path)?
        .set_times(std::fs::FileTimes::new().set_modified(modified))?;

    drop(open_log_file(&rollout_path)?);
    assert_eq!(fs::metadata(&rollout_path)?.modified()?, modified);

    drop(open_rollout_for_append(&rollout_path).await?);
    assert_eq!(fs::metadata(&rollout_path)?.modified()?, modified);
    Ok(())
}

#[tokio::test]
async fn load_rollout_items_defaults_legacy_session_id() -> std::io::Result<()> {
    let home = TempDir::new().expect("temp dir");
    let rollout_path = home.path().join("rollout.jsonl");
    let mut file = File::create(&rollout_path)?;
    let thread_id = ThreadId::new();
    let ts = "2025-01-03T12:00:00Z";

    writeln!(
        file,
        "{}",
        serde_json::json!({
            "timestamp": ts,
            "type": "session_meta",
            "payload": {
                "id": thread_id,
                "timestamp": ts,
                "cwd": ".",
                "originator": "test_originator",
                "cli_version": "test_version",
                "source": "cli",
                "model_provider": "test-provider",
            },
        })
    )?;
    writeln!(
        file,
        "{}",
        serde_json::json!({
            "timestamp": ts,
            "type": "response_item",
            "payload": {
                "type": "ghost_snapshot",
                "ghost_commit": {
                    "id": "deadbeef",
                    "preexisting_untracked_dirs": [],
                    "preexisting_untracked_files": [],
                },
            },
        })
    )?;
    writeln!(
        file,
        "{}",
        serde_json::json!({
            "timestamp": ts,
            "type": "response_item",
            "payload": {
                "type": "message",
                "role": "assistant",
                "content": [
                    {
                        "type": "output_text",
                        "text": "hello",
                    }
                ],
            },
        })
    )?;

    let (items, loaded_thread_id, parse_errors) =
        RolloutRecorder::load_rollout_items(&rollout_path).await?;

    assert_eq!(loaded_thread_id, Some(thread_id));
    assert_eq!(parse_errors, 0);
    assert_eq!(items.len(), 2);
    let RolloutItem::SessionMeta(session_meta) = &items[0] else {
        panic!("expected session metadata");
    };
    assert_eq!(session_meta.meta.session_id, SessionId::from(thread_id));
    assert!(matches!(
        items[1],
        RolloutItem::ResponseItem(ResponseItemEnvelope {
            item: ResponseItem::Message { .. },
            ..
        })
    ));

    Ok(())
}

#[tokio::test]
async fn load_rollout_items_ignores_unknown_fork_source_history_mode() -> std::io::Result<()> {
    let home = TempDir::new().expect("temp dir");
    let uuid = Uuid::new_v4();
    let thread_id = ThreadId::from_string(&uuid.to_string()).expect("thread id");
    let rollout_path = write_session_file(home.path(), "2025-01-03T12-00-00", uuid)?;
    let mut file = fs::OpenOptions::new().append(true).open(&rollout_path)?;
    let source_uuid = Uuid::new_v4();
    writeln!(
        file,
        "{}",
        serde_json::json!({
            "timestamp": "2025-01-03T12:00:01Z",
            "type": "session_meta",
            "payload": {
                "session_id": source_uuid,
                "id": source_uuid,
                "timestamp": "2025-01-03T12:00:01Z",
                "cwd": ".",
                "originator": "test_originator",
                "cli_version": "test_version",
                "source": "cli",
                "model_provider": "test-provider",
                "history_mode": "future",
            },
        })
    )?;

    let (items, loaded_thread_id, parse_errors) =
        RolloutRecorder::load_rollout_items(&rollout_path).await?;

    assert_eq!(loaded_thread_id, Some(thread_id));
    assert_eq!(parse_errors, 1);
    assert_eq!(items.len(), 2);
    Ok(())
}

#[tokio::test]
async fn load_rollout_items_rejects_removed_guardian_events() -> std::io::Result<()> {
    let home = TempDir::new().expect("temp dir");
    let uuid = Uuid::new_v4();
    let thread_id = ThreadId::from_string(&uuid.to_string()).expect("thread id");
    let rollout_path = write_session_file(home.path(), "2025-01-03T12-00-00", uuid)?;
    let mut file = fs::OpenOptions::new().append(true).open(&rollout_path)?;
    writeln!(
        file,
        "{}",
        serde_json::json!({
            "timestamp": "2025-01-03T12:00:01Z",
            "type": "event_msg",
            "payload": {
                "type": "guardian_assessment",
                "id": "removed-review",
                "status": "denied",
                "action": {
                    "type": "mcp_tool_call",
                    "server": "legacy-server",
                    "tool_name": "legacy-tool",
                },
            },
        })
    )?;

    let (items, loaded_thread_id, parse_errors) =
        RolloutRecorder::load_rollout_items(&rollout_path).await?;

    assert_eq!(loaded_thread_id, Some(thread_id));
    assert_eq!(parse_errors, 1);
    assert_eq!(items.len(), 2);
    Ok(())
}

#[tokio::test]
async fn load_rollout_items_preserves_security_risk_scores() -> std::io::Result<()> {
    let home = TempDir::new().expect("temp dir");
    let rollout_path = home.path().join("rollout.jsonl");
    let thread_id = ThreadId::new();
    let security_risk = SecurityRiskScore {
        scores: BTreeMap::from([
            ("action_risk".to_string(), 0.76),
            ("data_exfiltration".to_string(), 0.31),
        ]),
        call_id: Some("call-1".to_owned()),
        action: Some(serde_json::json!({"path": "README.md", "tool": "read_file"})),
        sampled_at: None,
    };
    let security_risk_item = RolloutItem::SecurityRiskScore(security_risk.clone());
    for history_mode in [ThreadHistoryMode::Legacy, ThreadHistoryMode::Paginated] {
        assert!(crate::is_persisted_rollout_item(
            &security_risk_item,
            history_mode
        ));
    }

    let mut file = File::create(&rollout_path)?;
    for (ordinal, item) in [
        paginated_session_meta_item(thread_id, home.path()),
        security_risk_item,
    ]
    .into_iter()
    .enumerate()
    {
        let line = RolloutLine {
            timestamp: "2026-07-09T00:00:00Z".to_string(),
            ordinal: Some(ordinal as u64),
            item,
        };
        writeln!(
            file,
            "{}",
            serde_json::to_string(&line).map_err(std::io::Error::other)?
        )?;
    }

    let (items, loaded_thread_id, parse_errors) =
        RolloutRecorder::load_rollout_items(&rollout_path).await?;

    assert_eq!(loaded_thread_id, Some(thread_id));
    assert_eq!(parse_errors, 0);
    assert_eq!(items.len(), 2);
    let RolloutItem::SecurityRiskScore(persisted_security_risk) = &items[1] else {
        panic!("expected security risk score rollout item");
    };
    assert_eq!(persisted_security_risk, &security_risk);

    Ok(())
}

#[tokio::test]
async fn load_rollout_items_filters_legacy_ghost_snapshots_from_compaction_history()
-> std::io::Result<()> {
    let home = TempDir::new().expect("temp dir");
    let rollout_path = home.path().join("rollout.jsonl");
    let mut file = File::create(&rollout_path)?;
    let thread_id = ThreadId::new();
    let ts = "2025-01-03T12:00:00Z";

    writeln!(
        file,
        "{}",
        serde_json::json!({
            "timestamp": ts,
            "type": "session_meta",
            "payload": {
                "session_id": thread_id,
                "id": thread_id,
                "timestamp": ts,
                "cwd": ".",
                "originator": "test_originator",
                "cli_version": "test_version",
                "source": "cli",
                "model_provider": "test-provider",
            },
        })
    )?;
    writeln!(
        file,
        "{}",
        serde_json::json!({
            "timestamp": ts,
            "type": "compacted",
            "payload": {
                "message": "summary",
                "replacement_history": [
                    {
                        "type": "message",
                        "role": "assistant",
                        "content": [
                            {
                                "type": "output_text",
                                "text": "kept",
                            }
                        ],
                    },
                    {
                        "type": "ghost_snapshot",
                        "ghost_commit": {
                            "id": "deadbeef",
                            "preexisting_untracked_dirs": [],
                            "preexisting_untracked_files": [],
                        },
                    }
                ],
            },
        })
    )?;

    let (items, loaded_thread_id, parse_errors) =
        RolloutRecorder::load_rollout_items(&rollout_path).await?;

    assert_eq!(loaded_thread_id, Some(thread_id));
    assert_eq!(parse_errors, 0);
    assert_eq!(items.len(), 2);
    let RolloutItem::Compacted(compacted) = &items[1] else {
        panic!("expected compacted rollout item");
    };
    let replacement_history = compacted
        .replacement_history
        .as_ref()
        .expect("replacement history");
    assert_eq!(replacement_history.len(), 1);
    assert!(matches!(
        &replacement_history[0],
        ResponseItemEnvelope {
            item: ResponseItem::Message { .. },
            ..
        }
    ));

    Ok(())
}

#[test]
fn strip_legacy_ghost_snapshot_keeps_checkpoint_metadata_aligned() {
    let mut value = serde_json::json!({
        "type": "compacted",
        "payload": {
            "message": "summary",
            "replacement_history": [
                {"type": "message", "role": "assistant", "content": []},
                {"type": "ghost_snapshot", "ghost_commit": {"id": "deadbeef"}},
                {"type": "message", "role": "user", "content": []}
            ],
            "replacement_history_metadata": [
                {"slot": "assistant"},
                {"slot": "ghost"},
                {"slot": "user"}
            ]
        }
    });

    assert!(!strip_legacy_ghost_snapshot_rollout_line(&mut value));
    assert_eq!(
        value["payload"]["replacement_history"],
        serde_json::json!([
            {"type": "message", "role": "assistant", "content": []},
            {"type": "message", "role": "user", "content": []}
        ])
    );
    assert_eq!(
        value["payload"]["replacement_history_metadata"],
        serde_json::json!([
            {"slot": "assistant"},
            {"slot": "user"}
        ])
    );
}

#[tokio::test]
async fn recorder_materializes_on_flush_with_pending_items() -> std::io::Result<()> {
    let home = TempDir::new().expect("temp dir");
    let config = test_config(home.path());
    let session_id = SessionId::default();
    let thread_id = ThreadId::new();
    let initial_window_id = Uuid::now_v7().to_string();
    let recorder = RolloutRecorder::new(
        &config,
        RolloutRecorderParams::new(
            thread_id,
            /*forked_from_id*/ None,
            /*parent_thread_id*/ None,
            SessionSource::Exec,
            /*thread_source*/ None,
            "test_originator".to_string(),
            BaseInstructions::default(),
            Vec::new(),
        )
        .with_session_id(session_id)
        .with_history_mode(ThreadHistoryMode::Paginated)
        .with_initial_window_id(initial_window_id.clone()),
    )
    .await?;

    let rollout_path = recorder.rollout_path().to_path_buf();
    assert!(
        !rollout_path.exists(),
        "rollout file should not exist before the first recordable item"
    );

    recorder
        .record_canonical_items(&[RolloutItem::EventMsg(EventMsg::AgentMessage(
            AgentMessageEvent {
                message: "buffered-event".to_string(),
                phase: None,
                memory_citation: None,
                delivery: None,
            },
        ))])
        .await?;
    recorder.flush().await?;
    assert!(
        rollout_path.exists(),
        "flush with pending items should materialize the rollout"
    );

    recorder
        .record_canonical_items(&[RolloutItem::EventMsg(EventMsg::UserMessage(
            UserMessageEvent {
                client_id: None,
                message: "first-user-message".to_string(),
                images: None,
                local_images: Vec::new(),
                text_elements: Vec::new(),
                ..Default::default()
            },
        ))])
        .await?;
    recorder.flush().await?;

    recorder.persist().await?;
    // Second call verifies `persist()` is idempotent after materialization.
    recorder.persist().await?;
    assert!(rollout_path.exists(), "rollout file should be materialized");

    let text = std::fs::read_to_string(&rollout_path)?;
    let lines = read_rollout_lines(&rollout_path)?;
    assert_eq!(
        lines.iter().map(|line| line.ordinal).collect::<Vec<_>>(),
        vec![Some(0), Some(1), Some(2)]
    );
    let first_line = text.lines().next().expect("session metadata line");
    let session_meta: RolloutLine = serde_json::from_str(first_line)?;
    let RolloutItem::SessionMeta(session_meta) = session_meta.item else {
        panic!("expected session metadata in rollout");
    };
    assert_eq!(session_meta.meta.session_id, session_id);
    assert_eq!(session_meta.meta.history_mode, ThreadHistoryMode::Paginated);
    assert_eq!(
        session_meta
            .meta
            .context_window
            .map(|window| window.window_id),
        Some(initial_window_id)
    );
    let buffered_idx = text
        .find("buffered-event")
        .expect("buffered event in rollout");
    let user_idx = text
        .find("first-user-message")
        .expect("first user message in rollout");
    assert!(
        buffered_idx < user_idx,
        "buffered items should preserve ordering"
    );
    let text_after_second_persist = std::fs::read_to_string(&rollout_path)?;
    assert_eq!(text_after_second_persist, text);

    recorder.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn referenced_paginated_rollout_starts_at_history_cutoff_and_resumes() -> std::io::Result<()>
{
    let home = TempDir::new().expect("temp dir");
    let config = test_config(home.path());
    let history_base = HistoryPosition {
        thread_id: ThreadId::new(),
        end_ordinal_exclusive: 41,
        end_byte_offset: 1,
    };
    let recorder = RolloutRecorder::new(
        &config,
        RolloutRecorderParams::new(
            ThreadId::new(),
            Some(history_base.thread_id),
            /*parent_thread_id*/ None,
            SessionSource::Exec,
            /*thread_source*/ None,
            "test_originator".to_string(),
            BaseInstructions::default(),
            Vec::new(),
        )
        .with_history_mode(ThreadHistoryMode::Paginated)
        .with_history_base(Some(history_base))
        .with_forked_from_ordinal_exclusive(Some(history_base.end_ordinal_exclusive)),
    )
    .await?;
    let rollout_path = recorder.rollout_path().to_path_buf();
    recorder.persist().await?;
    recorder.shutdown().await?;

    let meta = crate::read_session_meta_line(&rollout_path).await?.meta;
    assert_eq!(
        meta.forked_from_ordinal_exclusive,
        Some(history_base.end_ordinal_exclusive)
    );

    let resumed =
        RolloutRecorder::new(&config, RolloutRecorderParams::resume(rollout_path.clone())).await?;
    resumed
        .record_canonical_items(&[agent_message_item("first child record")])
        .await?;
    resumed.flush().await?;
    resumed.shutdown().await?;

    let resumed =
        RolloutRecorder::new(&config, RolloutRecorderParams::resume(rollout_path.clone())).await?;
    resumed
        .record_canonical_items(&[agent_message_item("second child record")])
        .await?;
    resumed.flush().await?;
    assert_eq!(
        read_rollout_lines(&rollout_path)?
            .into_iter()
            .map(|line| line.ordinal)
            .collect::<Vec<_>>(),
        vec![Some(41), Some(42), Some(43)]
    );
    resumed.shutdown().await
}

#[tokio::test]
async fn rollout_id_preserves_session_meta_thread_id() -> std::io::Result<()> {
    let home = TempDir::new().expect("temp dir");
    let config = test_config(home.path());
    let thread_id = ThreadId::new();
    let rollout_id = ThreadId::new();
    let recorder = RolloutRecorder::new(
        &config,
        RolloutRecorderParams::new(
            thread_id,
            /*forked_from_id*/ None,
            /*parent_thread_id*/ None,
            SessionSource::Exec,
            /*thread_source*/ None,
            "test_originator".to_string(),
            BaseInstructions::default(),
            Vec::new(),
        )
        .with_history_mode(ThreadHistoryMode::Paginated)
        .with_rollout_id(rollout_id),
    )
    .await?;
    let rollout_path = recorder.rollout_path().to_path_buf();
    recorder.persist().await?;
    recorder.shutdown().await?;

    let replacement_suffix = format!("-{thread_id}_{rollout_id}.jsonl");
    assert!(
        rollout_path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(replacement_suffix.as_str()))
    );
    assert_eq!(
        crate::rollout_id_from_path(rollout_path.as_path()),
        Some(rollout_id)
    );
    let RolloutItem::SessionMeta(meta_line) = &read_rollout_lines(&rollout_path)?[0].item else {
        panic!("first rollout item should be session metadata");
    };
    assert_eq!(meta_line.meta.id, thread_id);
    Ok(())
}

#[tokio::test]
async fn recorder_omits_ordinals_from_legacy_rollouts() -> std::io::Result<()> {
    let home = TempDir::new().expect("temp dir");
    let config = test_config(home.path());
    let recorder = RolloutRecorder::new(
        &config,
        RolloutRecorderParams::new(
            ThreadId::new(),
            /*forked_from_id*/ None,
            /*parent_thread_id*/ None,
            SessionSource::Exec,
            /*thread_source*/ None,
            "test_originator".to_string(),
            BaseInstructions::default(),
            Vec::new(),
        ),
    )
    .await?;
    recorder
        .record_canonical_items(&[agent_message_item("legacy")])
        .await?;
    recorder.flush().await?;

    let text = fs::read_to_string(recorder.rollout_path())?;
    let values = text
        .lines()
        .map(serde_json::from_str::<serde_json::Value>)
        .collect::<Result<Vec<_>, _>>()?;
    assert!(values.iter().all(|value| value.get("ordinal").is_none()));

    recorder.shutdown().await
}

#[tokio::test]
async fn resumed_empty_rollout_omits_ordinals() -> std::io::Result<()> {
    let home = TempDir::new().expect("temp dir");
    let config = test_config(home.path());
    let rollout_path = home.path().join("rollout.jsonl");
    File::create(&rollout_path)?;

    let recorder =
        RolloutRecorder::new(&config, RolloutRecorderParams::resume(rollout_path.clone())).await?;
    recorder
        .record_canonical_items(&[agent_message_item("legacy")])
        .await?;
    recorder.flush().await?;

    let text = fs::read_to_string(rollout_path)?;
    let values = text
        .lines()
        .map(serde_json::from_str::<serde_json::Value>)
        .collect::<Result<Vec<_>, _>>()?;
    assert!(values.iter().all(|value| value.get("ordinal").is_none()));

    recorder.shutdown().await
}

#[tokio::test]
async fn persist_reports_filesystem_error_and_retries_buffered_items() -> std::io::Result<()> {
    let home = TempDir::new().expect("temp dir");
    let config = test_config(home.path());
    let thread_id = ThreadId::new();
    let recorder = RolloutRecorder::new(
        &config,
        RolloutRecorderParams::new(
            thread_id,
            /*forked_from_id*/ None,
            /*parent_thread_id*/ None,
            SessionSource::Exec,
            /*thread_source*/ None,
            "test_originator".to_string(),
            BaseInstructions::default(),
            Vec::new(),
        ),
    )
    .await?;
    let rollout_path = recorder.rollout_path().to_path_buf();

    recorder
        .record_canonical_items(&[RolloutItem::EventMsg(EventMsg::AgentMessage(
            AgentMessageEvent {
                message: "buffered-before-persist".to_string(),
                phase: None,
                memory_citation: None,
                delivery: None,
            },
        ))])
        .await?;
    let sessions_blocker_path = home.path().join("sessions");
    File::create(&sessions_blocker_path)?;

    let err = recorder
        .persist()
        .await
        .expect_err("blocked sessions directory should fail persist");
    assert_ne!(err.kind(), std::io::ErrorKind::Interrupted);
    assert!(
        !rollout_path.exists(),
        "failed persist should keep the rollout deferred"
    );

    fs::remove_file(sessions_blocker_path)?;
    recorder.flush().await?;
    let text = std::fs::read_to_string(&rollout_path)?;
    assert!(
        text.contains("buffered-before-persist"),
        "retry should preserve items buffered before the failed persist"
    );

    recorder.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn writer_state_retries_write_error_before_reporting_flush_success() -> std::io::Result<()> {
    let home = TempDir::new().expect("temp dir");
    let rollout_path = home.path().join("rollout.jsonl");
    File::create(&rollout_path)?;
    let read_only_file = std::fs::OpenOptions::new().read(true).open(&rollout_path)?;
    let mut state = RolloutWriterState {
        writer: Some(JsonlWriter {
            file: tokio::fs::File::from_std(read_only_file),
        }),
        deferred_creation: false,
        pending_items: Vec::new(),
        meta: None,
        cwd: home.path().to_path_buf(),
        rollout_path: rollout_path.clone(),
        ordinal_state: RolloutOrdinalState::Legacy,
        last_logged_error: None,
    };
    state.add_items(vec![RolloutItem::EventMsg(EventMsg::AgentMessage(
        AgentMessageEvent {
            message: "queued-after-writer-error".to_string(),
            phase: None,
            memory_citation: None,
            delivery: None,
        },
    ))]);

    state.flush().await?;
    let text_after_retry = std::fs::read_to_string(&rollout_path)?;
    assert!(
        text_after_retry.contains("queued-after-writer-error"),
        "flush should retry after reopening and write buffered items"
    );
    Ok(())
}

#[tokio::test]
async fn resumed_paginated_rollout_continues_after_ordinal_gap() -> std::io::Result<()> {
    let home = TempDir::new().expect("temp dir");
    let config = test_config(home.path());
    let rollout_path = home.path().join("rollout.jsonl");
    write_paginated_rollout(&rollout_path, ThreadId::new(), &[4])?;

    let recorder =
        RolloutRecorder::new(&config, RolloutRecorderParams::resume(rollout_path.clone())).await?;
    recorder
        .record_canonical_items(&[agent_message_item("after-resume")])
        .await?;
    recorder.flush().await?;

    let lines = read_rollout_lines(&rollout_path)?;
    assert_eq!(
        lines.iter().map(|line| line.ordinal).collect::<Vec<_>>(),
        vec![Some(0), Some(4), Some(5)]
    );
    recorder.shutdown().await
}

#[tokio::test]
async fn resumed_paginated_rollout_repairs_unsafe_tail() -> std::io::Result<()> {
    let valid_unterminated = serde_json::to_string(&RolloutLine {
        timestamp: "2026-07-09T00:00:05Z".to_string(),
        ordinal: Some(5),
        item: agent_message_item("valid unterminated"),
    })?;
    for (name, tail, expected_ordinals) in [
        (
            "valid unterminated",
            valid_unterminated,
            vec![Some(0), Some(4), Some(5), Some(6)],
        ),
        (
            "invalid unterminated",
            "{\"timestamp\":\"unterminated\"".to_string(),
            vec![Some(0), Some(4), Some(5)],
        ),
    ] {
        let home = TempDir::new().expect("temp dir");
        let config = test_config(home.path());
        let rollout_path = home.path().join("rollout.jsonl");
        write_paginated_rollout(&rollout_path, ThreadId::new(), &[4])?;
        let mut file = fs::OpenOptions::new().append(true).open(&rollout_path)?;
        write!(file, "{tail}")?;
        drop(file);

        let recorder =
            RolloutRecorder::new(&config, RolloutRecorderParams::resume(rollout_path.clone()))
                .await?;
        recorder
            .record_canonical_items(&[agent_message_item("after-tail-repair")])
            .await?;
        recorder.flush().await?;

        let contents = fs::read_to_string(&rollout_path)?;
        assert!(contents.ends_with('\n'), "{name} tail should be terminated");
        let ordinals = contents
            .lines()
            .filter_map(|line| serde_json::from_str::<RolloutLine>(line).ok())
            .map(|line| line.ordinal)
            .collect::<Vec<_>>();
        assert_eq!(
            ordinals, expected_ordinals,
            "unexpected ordinals after repairing {name} tail"
        );
        recorder.shutdown().await?;
    }
    Ok(())
}

#[tokio::test]
async fn paginated_ordinal_overflow_fails_without_appending() -> std::io::Result<()> {
    let home = TempDir::new().expect("temp dir");
    let config = test_config(home.path());
    let rollout_path = home.path().join("rollout.jsonl");
    write_paginated_rollout(&rollout_path, ThreadId::new(), &[u64::MAX])?;
    let before = fs::read(&rollout_path)?;

    let recorder =
        RolloutRecorder::new(&config, RolloutRecorderParams::resume(rollout_path.clone())).await?;
    recorder
        .record_canonical_items(&[agent_message_item("overflow")])
        .await?;
    let err = recorder
        .flush()
        .await
        .expect_err("ordinal overflow should fail the append");
    assert!(err.to_string().contains("overflow"));
    assert_eq!(fs::read(&rollout_path)?, before);
    Ok(())
}

#[tokio::test]
async fn resumed_paginated_subagent_rollout_rejects_incomplete_prefix() -> std::io::Result<()> {
    let home = TempDir::new().expect("temp dir");
    let config = test_config(home.path());
    let rollout_path = home.path().join("rollout.jsonl");
    let thread_id = ThreadId::new();
    let mut session_meta = paginated_session_meta_item(thread_id, home.path());
    let RolloutItem::SessionMeta(meta_line) = &mut session_meta else {
        panic!("fixture should be session metadata");
    };
    meta_line.meta.subagent_history_start_ordinal = Some(3);
    let lines = [
        RolloutLine {
            timestamp: "2026-07-09T00:00:00Z".to_string(),
            ordinal: Some(0),
            item: session_meta,
        },
        RolloutLine {
            timestamp: "2026-07-09T00:00:01Z".to_string(),
            ordinal: Some(1),
            item: agent_message_item("partial inherited prefix"),
        },
    ];
    let jsonl = lines
        .iter()
        .map(serde_json::to_string)
        .collect::<Result<Vec<_>, _>>()?
        .join("\n");
    fs::write(&rollout_path, format!("{jsonl}\n"))?;

    let err = match RolloutRecorder::new(&config, RolloutRecorderParams::resume(rollout_path)).await
    {
        Ok(_) => panic!("incomplete prefix should fail resume"),
        Err(err) => err,
    };

    assert!(err.to_string().contains("incomplete"));
    Ok(())
}

#[tokio::test]
async fn append_rollout_item_to_path_assigns_next_paginated_ordinal() -> std::io::Result<()> {
    let home = TempDir::new().expect("temp dir");
    let rollout_path = home.path().join("rollout.jsonl");
    write_paginated_rollout(&rollout_path, ThreadId::new(), &[4])?;

    append_rollout_item_to_path(&rollout_path, &agent_message_item("offline")).await?;

    let lines = read_rollout_lines(&rollout_path)?;
    assert_eq!(lines.last().and_then(|line| line.ordinal), Some(5));
    Ok(())
}

#[tokio::test]
async fn resume_candidate_matches_cwd_reads_latest_turn_context() -> std::io::Result<()> {
    let home = TempDir::new().expect("temp dir");
    let stale_cwd = home.path().join("stale");
    let latest_cwd = home.path().join("latest");
    fs::create_dir_all(&stale_cwd)?;
    fs::create_dir_all(&latest_cwd)?;

    let path = write_session_file(home.path(), "2025-01-03T13-00-00", Uuid::from_u128(9012))?;
    let mut file = std::fs::OpenOptions::new().append(true).open(&path)?;
    let turn_context = RolloutLine {
        timestamp: "2025-01-03T13:00:01Z".to_string(),
        ordinal: None,
        item: RolloutItem::TurnContext(TurnContextItem {
            turn_id: Some("turn-1".to_string()),
            cwd: serde_json::from_value(serde_json::json!(&latest_cwd))
                .expect("absolute latest cwd"),
            workspace_roots: None,
            current_date: None,
            timezone: None,
            approval_policy: AskForApproval::Never,
            sandbox_policy: SandboxPolicy::new_read_only_policy(),
            permission_profile: None,
            active_permission_profile: None,
            network: None,
            file_system_sandbox_policy: None,
            model: "test-model".to_string(),
            comp_hash: None,
            personality: None,
            collaboration_mode: None,
            multi_agent_version: None,
            multi_agent_mode: None,
            realtime_active: None,
            cyber_access_program: None,
            effort: None,
            summary: codex_protocol::config_types::ReasoningSummary::Auto,
        }),
    };
    writeln!(file, "{}", serde_json::to_string(&turn_context)?)?;

    assert!(
        resume_candidate_matches_cwd(
            path.as_path(),
            Some(stale_cwd.as_path()),
            latest_cwd.as_path(),
            "test-provider",
        )
        .await
    );
    Ok(())
}
