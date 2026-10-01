#![allow(warnings, clippy::all)]

use super::*;
use crate::CompactedItem;
use crate::RolloutItem;
use crate::RolloutLine;
use chrono::DateTime;
use chrono::NaiveDateTime;
use chrono::Timelike;
use chrono::Utc;
use codex_protocol::SanitizedGitUrl;
use codex_protocol::ThreadId;
use codex_protocol::protocol::GitInfo;
use codex_protocol::protocol::HistoryPosition;
use codex_protocol::protocol::SessionMeta;
use codex_protocol::protocol::SessionMetaLine;
use codex_protocol::protocol::SessionSource;
use codex_protocol::protocol::ThreadHistoryMode;
use codex_utils_absolute_path::test_support::PathExt;
use pretty_assertions::assert_eq;
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use tempfile::tempdir;
use uuid::Uuid;

#[test]
fn fork_cutoff_distinguishes_logical_parent_from_reverted_rollout() {
    let parent_id = ThreadId::new();
    let thread_id = ThreadId::new();
    let physical_id = ThreadId::new();
    let replacement_id = ThreadId::new();
    let original_path = PathBuf::from(format!("rollout-2026-01-27T12-34-56-{thread_id}.jsonl"));
    let reverted_path = PathBuf::from(format!(
        "rollout-2026-01-27T12-34-56-{thread_id}_{replacement_id}.jsonl"
    ));

    for (name, parent, cutoff, base_id, path, expected) in [
        (
            "persisted revert",
            Some(parent_id),
            Some(20),
            thread_id,
            Some(reverted_path.as_path()),
            Some(20),
        ),
        (
            "legacy direct fork",
            Some(parent_id),
            None,
            parent_id,
            None,
            Some(40),
        ),
        (
            "legacy fork of reverted parent",
            Some(parent_id),
            None,
            physical_id,
            Some(original_path.as_path()),
            Some(40),
        ),
        (
            "legacy child revert",
            Some(parent_id),
            None,
            thread_id,
            Some(reverted_path.as_path()),
            None,
        ),
        (
            "legacy repeated revert",
            Some(parent_id),
            None,
            physical_id,
            Some(reverted_path.as_path()),
            None,
        ),
        (
            "missing parent",
            None,
            Some(20),
            parent_id,
            Some(original_path.as_path()),
            None,
        ),
    ] {
        let meta = SessionMeta {
            id: thread_id,
            forked_from_id: parent,
            forked_from_ordinal_exclusive: cutoff,
            history_base: Some(HistoryPosition {
                thread_id: base_id,
                end_ordinal_exclusive: 40,
                end_byte_offset: 100,
            }),
            ..SessionMeta::default()
        };
        assert_eq!(
            forked_from_ordinal_exclusive(&meta, path),
            expected,
            "{name}"
        );
    }
}

#[tokio::test]
async fn extract_metadata_from_rollout_uses_session_meta() {
    let dir = tempdir().expect("tempdir");
    let uuid = Uuid::new_v4();
    let id = ThreadId::from_string(&uuid.to_string()).expect("thread id");
    let path = dir
        .path()
        .join(format!("rollout-2026-01-27T12-34-56-{uuid}.jsonl"));

    let session_meta = SessionMeta {
        session_id: id.into(),
        id,
        forked_from_id: None,
        forked_from_ordinal_exclusive: None,
        parent_thread_id: None,
        timestamp: "2026-01-27T12:34:56Z".to_string(),
        cwd: dir.path().to_path_buf(),
        originator: "cli".to_string(),
        cli_version: "0.0.0".to_string(),
        source: SessionSource::default(),
        thread_source: None,
        agent_path: None,
        agent_nickname: None,
        agent_role: None,
        model_provider: Some("openai".to_string()),
        base_instructions: None,
        dynamic_tools: None,
        selected_capability_roots: Vec::new(),
        memory_mode: None,
        history_mode: ThreadHistoryMode::Paginated,
        history_base: None,
        subagent_history_start_ordinal: None,
        multi_agent_version: None,
        context_window: None,
    };
    let session_meta_line = SessionMetaLine {
        meta: session_meta,
        git: None,
    };
    let rollout_line = RolloutLine {
        timestamp: "2026-01-27T12:34:56Z".to_string(),
        ordinal: Some(0),
        item: RolloutItem::SessionMeta(session_meta_line.clone()),
    };
    let json = serde_json::to_string(&rollout_line).expect("rollout json");
    let mut file = File::create(&path).expect("create rollout");
    writeln!(file, "{json}").expect("write rollout");

    let outcome = extract_metadata_from_rollout(&path, "openai")
        .await
        .expect("extract");

    let builder = builder_from_session_meta(&session_meta_line, path.as_path()).expect("builder");
    let mut expected = builder.build("openai");
    apply_rollout_item(&mut expected, &rollout_line.item, "openai");
    expected.updated_at = file_modified_time_utc(&path).await.expect("mtime");
    expected.recency_at = expected.updated_at;

    assert_eq!(outcome.metadata, expected);
    assert_eq!(outcome.memory_mode, None);
    assert_eq!(outcome.parse_errors, 0);
}

#[tokio::test]
async fn extract_metadata_from_rollout_rejects_unknown_history_mode() {
    let dir = tempdir().expect("tempdir");
    let uuid = Uuid::new_v4();
    let id = ThreadId::from_string(&uuid.to_string()).expect("thread id");
    let path = dir
        .path()
        .join(format!("rollout-2026-01-27T12-34-56-{uuid}.jsonl"));
    let mut rollout_line = serde_json::to_value(RolloutLine {
        timestamp: "2026-01-27T12:34:56Z".to_string(),
        ordinal: None,
        item: RolloutItem::SessionMeta(SessionMetaLine {
            meta: SessionMeta {
                session_id: id.into(),
                id,
                timestamp: "2026-01-27T12:34:56Z".to_string(),
                cwd: dir.path().to_path_buf(),
                originator: "cli".to_string(),
                cli_version: "0.0.0".to_string(),
                ..SessionMeta::default()
            },
            git: None,
        }),
    })
    .expect("serialize rollout line");
    rollout_line["payload"]["history_mode"] = serde_json::json!("future");
    let mut file = File::create(&path).expect("create rollout");
    writeln!(file, "{rollout_line}").expect("write rollout");

    assert!(
        extract_metadata_from_rollout(&path, "openai")
            .await
            .is_err()
    );
}

#[tokio::test]
async fn extract_metadata_from_rollout_returns_latest_memory_mode() {
    let dir = tempdir().expect("tempdir");
    let uuid = Uuid::new_v4();
    let id = ThreadId::from_string(&uuid.to_string()).expect("thread id");
    let path = dir
        .path()
        .join(format!("rollout-2026-01-27T12-34-56-{uuid}.jsonl"));

    let session_meta = SessionMeta {
        session_id: id.into(),
        id,
        forked_from_id: None,
        forked_from_ordinal_exclusive: None,
        parent_thread_id: None,
        timestamp: "2026-01-27T12:34:56Z".to_string(),
        cwd: dir.path().to_path_buf(),
        originator: "cli".to_string(),
        cli_version: "0.0.0".to_string(),
        source: SessionSource::default(),
        thread_source: None,
        agent_path: None,
        agent_nickname: None,
        agent_role: None,
        model_provider: Some("openai".to_string()),
        base_instructions: None,
        dynamic_tools: None,
        selected_capability_roots: Vec::new(),
        memory_mode: None,
        history_mode: Default::default(),
        history_base: None,
        subagent_history_start_ordinal: None,
        multi_agent_version: None,
        context_window: None,
    };
    let polluted_meta = SessionMeta {
        memory_mode: Some("polluted".to_string()),
        multi_agent_version: None,
        ..session_meta.clone()
    };
    let lines = vec![
        RolloutLine {
            timestamp: "2026-01-27T12:34:56Z".to_string(),
            ordinal: None,
            item: RolloutItem::SessionMeta(SessionMetaLine {
                meta: session_meta,
                git: None,
            }),
        },
        RolloutLine {
            timestamp: "2026-01-27T12:35:00Z".to_string(),
            ordinal: None,
            item: RolloutItem::SessionMeta(SessionMetaLine {
                meta: polluted_meta,
                git: None,
            }),
        },
    ];
    let mut file = File::create(&path).expect("create rollout");
    for line in lines {
        writeln!(
            file,
            "{}",
            serde_json::to_string(&line).expect("serialize rollout line")
        )
        .expect("write rollout line");
    }

    let outcome = extract_metadata_from_rollout(&path, "openai")
        .await
        .expect("extract");

    assert_eq!(outcome.memory_mode.as_deref(), Some("polluted"));
}

#[test]
fn builder_from_items_falls_back_to_filename() {
    let dir = tempdir().expect("tempdir");
    let uuid = Uuid::new_v4();
    let path = dir
        .path()
        .join(format!("rollout-2026-01-27T12-34-56-{uuid}.jsonl"));
    let items = vec![RolloutItem::Compacted(CompactedItem {
        message: "noop".to_string(),
        replacement_history: None,
        window_number: None,
        first_window_id: None,
        previous_window_id: None,
        window_id: None,
    })];

    let builder = builder_from_items(items.as_slice(), path.as_path()).expect("builder");
    let naive = NaiveDateTime::parse_from_str("2026-01-27T12-34-56", "%Y-%m-%dT%H-%M-%S")
        .expect("timestamp");
    let created_at = DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc)
        .with_nanosecond(0)
        .expect("nanosecond");
    let expected = ThreadMetadataBuilder::new(
        ThreadId::from_string(&uuid.to_string()).expect("thread id"),
        path,
        created_at,
        SessionSource::default(),
    );

    assert_eq!(builder, expected);
}
