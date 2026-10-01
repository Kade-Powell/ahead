use crate::RolloutItem;
use crate::compression;
use crate::recorder::RolloutRecorder;
use crate::rollout_file_name::RolloutFileName;
use chrono::DateTime;
use chrono::NaiveDateTime;
use chrono::Timelike;
use chrono::Utc;
use codex_protocol::RolloutId;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::SandboxPolicy;
use codex_protocol::protocol::SessionMeta;
use codex_protocol::protocol::SessionMetaLine;
use codex_protocol::protocol::SessionSource;
use codex_thread_metadata::ExtractionOutcome;
use codex_thread_metadata::ThreadMetadataBuilder;
use codex_thread_metadata::apply_rollout_item;
use std::path::Path;

pub(crate) fn builder_from_session_meta(
    session_meta: &SessionMetaLine,
    rollout_path: &Path,
) -> Option<ThreadMetadataBuilder> {
    let created_at = parse_timestamp_to_utc(session_meta.meta.timestamp.as_str())?;
    let mut builder = ThreadMetadataBuilder::new(
        session_meta.meta.id,
        rollout_path.to_path_buf(),
        created_at,
        session_meta.meta.source.clone(),
    );
    builder.history_mode = session_meta.meta.history_mode;
    builder.model_provider = session_meta.meta.model_provider.clone();
    builder.agent_nickname = session_meta.meta.agent_nickname.clone();
    builder.agent_role = session_meta.meta.agent_role.clone();
    builder.agent_path = session_meta.meta.agent_path.clone();
    builder.cwd = session_meta.meta.cwd.clone();
    builder.cli_version = Some(session_meta.meta.cli_version.clone());
    builder.sandbox_policy = SandboxPolicy::new_read_only_policy();
    builder.approval_mode = AskForApproval::OnRequest;
    if let Some(git) = session_meta.git.as_ref() {
        builder.git_sha = git.commit_hash.as_ref().map(|sha| sha.0.clone());
        builder.git_branch = git.branch.clone();
        builder.git_origin_url = git.repository_url.clone();
    }
    Some(builder)
}

pub fn builder_from_items(
    items: &[RolloutItem],
    rollout_path: &Path,
) -> Option<ThreadMetadataBuilder> {
    if let Some(session_meta) = items.iter().find_map(|item| match item {
        RolloutItem::SessionMeta(meta_line) => Some(meta_line),
        RolloutItem::ResponseItem(_)
        | RolloutItem::InterAgentCommunication(_)
        | RolloutItem::InterAgentCommunicationMetadata { .. }
        | RolloutItem::Compacted(_)
        | RolloutItem::TurnContext(_)
        | RolloutItem::WorldState(_)
        | RolloutItem::RealtimeItem(_)
        | RolloutItem::SecurityRiskScore(_)
        | RolloutItem::EventMsg(_) => None,
    }) && let Some(builder) = builder_from_session_meta(session_meta, rollout_path)
    {
        return Some(builder);
    }

    let file_name = rollout_path.file_name()?.to_str()?;
    let file_name = RolloutFileName::parse(file_name)?;
    let created_ts = file_name.timestamp();
    let created_at =
        DateTime::<Utc>::from_timestamp(created_ts.unix_timestamp(), 0)?.with_nanosecond(0)?;
    Some(ThreadMetadataBuilder::new(
        file_name.thread_id(),
        rollout_path.to_path_buf(),
        created_at,
        SessionSource::default(),
    ))
}

/// Returns the rollout ID encoded in a canonical rollout filename.
///
/// Normal rollouts use `rollout-<timestamp>-<thread-id>.jsonl`, where the thread ID and rollout ID
/// are the same. Threads that have been `reverted` use
/// `rollout-<timestamp>-<thread-id>_<rollout-id>.jsonl`, where this returns the ID after `_`.
///
/// This can differ from [`SessionMeta::id`] when `thread/revert` keeps the thread ID stable while
/// switching to a new immutable rollout file.
pub fn rollout_id_from_path(rollout_path: &Path) -> Option<RolloutId> {
    let file_name = rollout_path.file_name()?.to_str()?;
    Some(RolloutFileName::parse(file_name)?.rollout_id())
}

/// Reads the logical fork cutoff without mistaking a revert's history base for its parent.
///
/// Older rollouts lack the explicit cutoff. Their history base is safe to use only when it
/// names the logical parent directly or the current file is the thread's original rollout.
/// An ambiguous legacy revert omits the cutoff rather than reporting another thread's boundary.
pub fn forked_from_ordinal_exclusive(
    meta: &SessionMeta,
    rollout_path: Option<&Path>,
) -> Option<u64> {
    let parent_id = meta.forked_from_id?;
    meta.forked_from_ordinal_exclusive.or_else(|| {
        meta.history_base
            .filter(|base| {
                base.thread_id == parent_id
                    || rollout_path.and_then(rollout_id_from_path) == Some(meta.id)
            })
            .map(|base| base.end_ordinal_exclusive)
    })
}

pub async fn extract_metadata_from_rollout(
    rollout_path: &Path,
    default_provider: &str,
) -> anyhow::Result<ExtractionOutcome> {
    let (items, _thread_id, parse_errors) =
        RolloutRecorder::load_rollout_items(rollout_path).await?;
    if items.is_empty() {
        return Err(anyhow::anyhow!(
            "empty session file: {}",
            rollout_path.display()
        ));
    }
    let builder = builder_from_items(items.as_slice(), rollout_path).ok_or_else(|| {
        anyhow::anyhow!(
            "rollout missing metadata builder: {}",
            rollout_path.display()
        )
    })?;
    let mut metadata = builder.build(default_provider);
    for item in &items {
        apply_rollout_item(&mut metadata, item, default_provider);
    }
    if let Some(updated_at) = file_modified_time_utc(rollout_path).await {
        metadata.updated_at = updated_at;
        metadata.recency_at = updated_at;
    }
    Ok(ExtractionOutcome {
        metadata,
        memory_mode: items.iter().rev().find_map(|item| match item {
            RolloutItem::SessionMeta(meta_line) => meta_line.meta.memory_mode.clone(),
            RolloutItem::ResponseItem(_)
            | RolloutItem::InterAgentCommunication(_)
            | RolloutItem::InterAgentCommunicationMetadata { .. }
            | RolloutItem::Compacted(_)
            | RolloutItem::TurnContext(_)
            | RolloutItem::WorldState(_)
            | RolloutItem::RealtimeItem(_)
            | RolloutItem::SecurityRiskScore(_)
            | RolloutItem::EventMsg(_) => None,
        }),
        parse_errors,
    })
}

async fn file_modified_time_utc(path: &Path) -> Option<DateTime<Utc>> {
    let modified = compression::file_modified_time(path).await.ok()??;
    DateTime::<Utc>::from_timestamp(modified.unix_timestamp(), modified.nanosecond())
}

fn parse_timestamp_to_utc(ts: &str) -> Option<DateTime<Utc>> {
    const FILENAME_TS_FORMAT: &str = "%Y-%m-%dT%H-%M-%S";
    if let Ok(naive) = NaiveDateTime::parse_from_str(ts, FILENAME_TS_FORMAT) {
        let dt = DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc);
        return dt.with_nanosecond(0);
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(ts) {
        return Some(dt.with_timezone(&Utc));
    }
    None
}

#[cfg(test)]
#[path = "metadata_tests.rs"]
mod tests;
