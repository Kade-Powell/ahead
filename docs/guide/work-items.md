# Work Items and Planning Handoffs

Audience: AHEAD practitioners

Status: pilot v0.1

## Purpose

A work item is the team's coordination surface: a GitHub issue, Jira issue, Azure Boards item, Linear issue, or another durable URL. AHEAD owns the workflow state, evidence chain, human gates, and resumable handoff. Linking the two avoids making either system pretend to be the other.

A work item is optional unless the repository's AHEAD configuration requires one. Labels and issue templates may inform workflow selection, but the human still chooses the flow by its dominant outcome.

## Existing and new work items

An AHEAD run may link an existing work-item URL at startup or later. The link is provider-neutral and visible in the active header. Pi can also create a GitHub issue in the current repository after a human reviews its body, including any available approved plan, and explicitly confirms the external write. Creating, replacing, or linking a work item is always a human-attributed action.

The work item may summarize or link the approved outcome, decision, and plan. It does not replace the AHEAD run record, and AHEAD does not silently rewrite it as the workflow changes.

## Configurable boundary

A repository may add `.ahead/config.json` and require a work item before a selected phase in each workflow:

```json
{
  "api_version": "ahead.config/v0",
  "work_items": {
    "required_before_phase": {
      "product-change": "implement",
      "corrective-debugging": "implement",
      "internal-improvement": "implement",
      "decision": "publish",
      "investigation": "conclude",
      "operational-stabilization": "outcome"
    }
  }
}
```

The map is deliberately explicit. A team may configure only the flows it uses, choose a different phase, or omit the file entirely. Operational teams should place the boundary after urgent stabilization if creating a work item first could delay safe recovery.

The resolved requirement is copied into each new run. Later configuration changes do not silently change an active or saved run's gates.

## Project setup and configuration changes

When Pi starts new AHEAD work in a project with no `.ahead/config.json`, it offers to run a setup wizard or continue without project policy. Run `/ahead-config` at any time to:

- choose AHEAD's recommended planning boundaries;
- choose a different boundary for each workflow;
- explicitly make work items optional for every workflow;
- view the current configuration; or
- rerun the wizard to replace an existing or unsupported configuration.

Replacement is deliberate and confirmed. Before writing the current schema, AHEAD copies the exact prior file to `.ahead/backups/`, including an invalid file or one with an unsupported API version. This provides a recoverable configuration-migration path without guessing how old policy should map to new semantics. Existing and explicitly saved runs keep their original policy snapshot.

## Sprint-ahead planning

For work planned before implementation:

1. Start AHEAD with the work-item URL, or link/create the item during the run.
2. Complete the selected workflow through its human-approved plan.
3. Satisfy the configured work-item boundary and enter `implement`.
4. Save the ready-to-implement handoff instead of beginning implementation.
5. In the later sprint, the implementing engineer resumes the same run and inherits the approved evidence, decisions, plan, link, and open implementation gate.

Saving this handoff is an explicit human choice. It is not workflow completion, and it does not claim that implementation, review, deployment, or verification occurred.
