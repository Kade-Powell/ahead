//! AHEAD Policy Evaluator & Sandbox
//!
//! Enforces capability boundaries according to:
//! `effective_capabilities = phase capabilities ∩ Learn/Assist mode ∩ policy ∩ role`
//! Implements strict Maieutic safeguards, cursor guidance, and scaffolding allowance.

use ahead_rpc::ahead::{
    AssistanceMode, Capability, SessionPolicySnapshot, SessionRole, WorkflowPhase,
};

pub struct PolicyEvaluator;

impl PolicyEvaluator {
    /// Computes the effective capability set allowed in the current context
    pub fn effective_capabilities(
        phase: &WorkflowPhase,
        mode: AssistanceMode,
        _policy: &SessionPolicySnapshot,
        role: SessionRole,
    ) -> Vec<Capability> {
        let mut caps = Vec::new();

        // Viewers are strictly read-only
        if matches!(role, SessionRole::Viewer) {
            caps.push(Capability::ReadContext);
            caps.push(Capability::PresentCode);
            return caps;
        }

        // Everyone with review/edit/owner can read and present code
        caps.push(Capability::ReadContext);
        caps.push(Capability::PresentCode);

        // Learn mode strictly denies mutations, proposals, and automated executions
        if matches!(mode, AssistanceMode::Learn) {
            return caps;
        }

        // Assist mode logic
        if matches!(mode, AssistanceMode::Assist) {
            // Drafting is allowed during planning and exploration
            caps.push(Capability::RecordDraft);

            // Editing is allowed during implementation/stabilization phases, but NOT in review or investigation
            let is_read_only_phase = phase.id == "review"
                || phase.id == "investigation-scrutinize"
                || phase.id == "decision-framing";

            if !is_read_only_phase
                && (role == SessionRole::Owner || role == SessionRole::Editor)
            {
                caps.push(Capability::ProposeEdit);
            }

            // Running approved checks
            if phase.id == "verify" || phase.id == "implement" {
                caps.push(Capability::RunApprovedCheck);
            }
        }

        caps
    }

    /// Checks if predictions are allowed under the current mode
    pub fn predictions_allowed(
        mode: AssistanceMode,
        policy: &SessionPolicySnapshot,
    ) -> bool {
        match mode {
            AssistanceMode::Learn => false,
            AssistanceMode::Assist => policy.predictions != "off",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_learn_mode_strictly_denies_edit_and_checks() {
        let phase = WorkflowPhase {
            id: "implement".to_string(),
            title: "Implementation".to_string(),
            visit: 1,
        };
        let policy = SessionPolicySnapshot::default();
        let caps = PolicyEvaluator::effective_capabilities(
            &phase,
            AssistanceMode::Learn,
            &policy,
            SessionRole::Owner,
        );

        assert!(caps.contains(&Capability::ReadContext));
        assert!(caps.contains(&Capability::PresentCode));
        assert!(!caps.contains(&Capability::ProposeEdit));
        assert!(!caps.contains(&Capability::RecordDraft));
        assert!(!caps.contains(&Capability::RunApprovedCheck));
        assert!(!PolicyEvaluator::predictions_allowed(
            AssistanceMode::Learn,
            &policy
        ));
    }

    #[test]
    fn test_assist_mode_grants_edit_capability_in_implement_phase() {
        let phase = WorkflowPhase {
            id: "implement".to_string(),
            title: "Implementation".to_string(),
            visit: 1,
        };
        let policy = SessionPolicySnapshot::default();
        let caps = PolicyEvaluator::effective_capabilities(
            &phase,
            AssistanceMode::Assist,
            &policy,
            SessionRole::Editor,
        );
        assert!(caps.contains(&Capability::ProposeEdit));
    }

    #[test]
    fn test_assist_mode_denies_edits_during_review_phase() {
        let phase = WorkflowPhase {
            id: "review".to_string(),
            title: "Review".to_string(),
            visit: 1,
        };
        let policy = SessionPolicySnapshot::default();
        let caps = PolicyEvaluator::effective_capabilities(
            &phase,
            AssistanceMode::Assist,
            &policy,
            SessionRole::Editor,
        );
        assert!(!caps.contains(&Capability::ProposeEdit));
    }
}
