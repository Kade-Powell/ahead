use codex_protocol::protocol::SessionMeta;

/// Reads the logical fork cutoff without mistaking a revert's history base for its parent.
pub fn forked_from_ordinal_exclusive(meta: &SessionMeta) -> Option<u64> {
    let parent_id = meta.forked_from_id?;
    meta.forked_from_ordinal_exclusive.or_else(|| {
        meta.history_base
            .filter(|base| base.thread_id == parent_id)
            .map(|base| base.end_ordinal_exclusive)
    })
}

#[cfg(test)]
mod tests {
    use codex_protocol::ThreadId;
    use codex_protocol::protocol::HistoryPosition;
    use codex_protocol::protocol::SessionMeta;

    use super::forked_from_ordinal_exclusive;

    #[test]
    fn fork_cutoff_ignores_a_history_base_from_another_thread() {
        let parent_id = ThreadId::new();
        let child_id = ThreadId::new();
        let other_rollout_id = ThreadId::new();
        let mut meta = SessionMeta {
            forked_from_id: Some(parent_id),
            history_base: Some(HistoryPosition {
                thread_id: other_rollout_id,
                end_ordinal_exclusive: 40,
                end_byte_offset: 0,
            }),
            id: child_id,
            ..SessionMeta::default()
        };
        assert_eq!(forked_from_ordinal_exclusive(&meta), None);

        meta.forked_from_ordinal_exclusive = Some(20);
        assert_eq!(forked_from_ordinal_exclusive(&meta), Some(20));
    }
}
