use super::AgentsMdCache;
use super::AgentsMdManager;
use super::InstructionRefreshScope;

#[test]
fn instruction_cache_refreshes_for_a_new_turn_only() {
    let cache = AgentsMdCache {
        selections: Some(Vec::new()),
        refresh_scope: InstructionRefreshScope::Turn("turn-1".to_string()),
        loaded: None,
    };

    assert!(cache.matches(
        &[],
        None,
        &InstructionRefreshScope::Turn("turn-1".to_string()),
    ));
    assert!(!cache.matches(
        &[],
        None,
        &InstructionRefreshScope::Turn("turn-2".to_string()),
    ));
    assert!(!cache.matches(&[], None, &InstructionRefreshScope::SessionInitialization,));
}

#[tokio::test]
async fn project_provenance_distinguishes_missing_from_empty_snapshot() {
    let manager = AgentsMdManager::new(None);
    assert_eq!(manager.project_source_hashes_for_turn("turn-1").await, None);
    {
        let mut cache = manager.cache.lock().await;
        cache.refresh_scope = InstructionRefreshScope::Turn("turn-1".to_string());
        cache.selections = Some(Vec::new());
    }
    assert_eq!(
        manager.project_source_hashes_for_turn("turn-1").await,
        Some(Vec::new()),
    );
    assert_eq!(manager.project_source_hashes_for_turn("turn-2").await, None);
    manager.cache.lock().await.selections = None;
    assert_eq!(manager.project_source_hashes_for_turn("turn-1").await, None);
}
