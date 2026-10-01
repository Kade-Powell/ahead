use super::ContextWindowGuidanceState;
use super::REMOVAL_NOTICE;
use crate::context::ContextWindowGuidance;
use crate::context::ContextualUserFragment;
use crate::context::world_state::WorldState;
use crate::context::world_state::WorldStateSnapshot;
use codex_protocol::models::ResponseItem;
use pretty_assertions::assert_eq;
use serde_json::json;

#[test]
fn guidance_transitions_render_once() {
    let cases = [
        ("absent guidance stays silent", None, None, None),
        ("initial guidance", None, Some("guidance"), Some("guidance")),
        (
            "unchanged guidance",
            Some("guidance"),
            Some("guidance"),
            None,
        ),
        (
            "changed guidance",
            Some("original guidance"),
            Some("refreshed guidance"),
            Some(
                "This context-window guidance replaces all previously provided context-window guidance.\n\nrefreshed guidance",
            ),
        ),
        (
            "removed guidance",
            Some("guidance"),
            None,
            Some(REMOVAL_NOTICE),
        ),
        (
            "blank guidance",
            Some("guidance"),
            Some("  "),
            Some(REMOVAL_NOTICE),
        ),
    ];

    for (case, previous, current, expected) in cases {
        let mut original = WorldState::default();
        original.add_section(ContextWindowGuidanceState::new(previous));

        let mut refreshed = WorldState::default();
        refreshed.add_section(ContextWindowGuidanceState::new(current));
        let fragments = refreshed.render_diff(&original.snapshot());
        assert_eq!(
            fragments
                .iter()
                .map(|fragment| fragment.render())
                .collect::<Vec<_>>(),
            expected
                .map(|message| ContextWindowGuidance::new(message).render())
                .into_iter()
                .collect::<Vec<_>>(),
            "{case}"
        );

        // Empty guidance must survive persistence as a known state, not a deleted section.
        let snapshot: WorldStateSnapshot =
            serde_json::from_value(serde_json::to_value(refreshed.snapshot()).unwrap()).unwrap();
        assert!(refreshed.render_diff(&snapshot).is_empty());
    }
}

#[test]
fn legacy_guidance_is_reconciled_once() {
    let cases = [
        (
            "legacy history is replaced",
            false,
            Some("current guidance"),
            Some(
                "This context-window guidance replaces all previously provided context-window guidance.\n\ncurrent guidance",
            ),
        ),
        (
            "legacy history is cleared",
            false,
            None,
            Some(REMOVAL_NOTICE),
        ),
        (
            "legacy string snapshot is cleared",
            true,
            None,
            Some(REMOVAL_NOTICE),
        ),
        (
            "existing string snapshot is unchanged",
            true,
            Some("previous guidance"),
            None,
        ),
    ];

    for (case, has_snapshot, current, expected) in cases {
        let mut state = WorldState::default();
        state.add_section(ContextWindowGuidanceState::new(current));
        let snapshot: Option<WorldStateSnapshot> = has_snapshot.then(|| {
            serde_json::from_value(json!({"context_window_guidance": "previous guidance"})).unwrap()
        });
        let retained: ResponseItem =
            ContextualUserFragment::into(ContextWindowGuidance::new("previous guidance"));
        let fragments =
            state.render_history_diff(snapshot.as_ref(), std::slice::from_ref(&retained));
        assert_eq!(
            fragments
                .iter()
                .map(|fragment| fragment.render())
                .collect::<Vec<_>>(),
            expected
                .map(|message| ContextWindowGuidance::new(message).render())
                .into_iter()
                .collect::<Vec<_>>(),
            "{case}"
        );

        let mut history = vec![retained];
        if let Some(message) = expected {
            history.push(ContextualUserFragment::into(ContextWindowGuidance::new(
                message,
            )));
        }
        assert!(
            state
                .render_history_diff(Some(&state.snapshot()), &history)
                .is_empty()
        );
    }
}
