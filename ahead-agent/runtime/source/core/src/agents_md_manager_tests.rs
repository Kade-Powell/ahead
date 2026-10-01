use super::{AgentsMdCache, InstructionRefreshScope};

#[test]
fn instruction_cache_refreshes_for_a_new_turn_only() {
    let cache = AgentsMdCache {
        selections: Some(Vec::new()),
        active_project_trust_level: None,
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
