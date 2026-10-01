# HumanLayer workflow research

Status: research and decision provenance. Observations and probes are dated
2026-09-18; document ownership was clarified 2026-09-22.

This note records what AHEAD learned from HumanLayer and from the external ACP
harness probes. It is not a second product specification.

- [AHEAD workflow atlas](ahead-workflows.md) owns current workflow behavior,
  human/AI roles, review policy, and the `.ahead` artifact convention.
- [AHEAD Editor MVP](ahead-editor-mvp.md) owns architecture, implementation
  boundaries, and acceptance criteria.
- [TODO.md](../../TODO.md) owns unfinished implementation work.
- The [Constitution](../../CONSTITUTION.md) owns the reasons for keeping human
  understanding, judgment, and accountability central.

If this note conflicts with one of those documents, the owning document wins.
Update this note only when new research changes the provenance or rationale.

## What HumanLayer contributed to the design

HumanLayer was useful because it treats engineering work as durable work rather
than one disposable prompt. Its task, workflow, artifact, and workspace ideas
gave AHEAD concrete product patterns to evaluate without requiring AHEAD to copy
HumanLayer's product model or interface.

| Observed HumanLayer pattern | What AHEAD retained | Canonical AHEAD home |
|---|---|---|
| A task can outlive an individual agent session. | A durable session contains tasks, conversation, decisions, artifacts, and evidence; a harness conversation is replaceable execution history. | [Atlas §1.1](ahead-workflows.md#11-task-intent-replaces-the-session-mode-switch) and [§9](ahead-workflows.md#9-storage-and-artifact-convention) |
| Structured work can move through questions, research, design, implementation, and review, while lighter paths remain available. | AHEAD varies structure by the work. It keeps research, design, implementation, verification, and review distinct where that improves judgment, without forcing every phase or document on every change. | [Atlas §§2–8](ahead-workflows.md#2-sdlc-map-and-responsibility-legend) |
| Artifacts can be revised and reviewed as the work evolves. | Research, design, plan, verification, and review records are low-friction byproducts of work. They have explicit authorship and revisions and are created only when useful. | [Atlas §9](ahead-workflows.md#9-storage-and-artifact-convention) |
| Workspace configuration has shared and local concerns. | Tracked project defaults and templates are separated from credentials, personal settings, runtime databases, and private working sessions. | [Atlas §9](ahead-workflows.md#9-storage-and-artifact-convention) |
| Prior work can be revisited, attached, and continued. | Published `.ahead/sessions/<id>/` checkpoints are readable without the original database or harness and can be attached by revision to new work. | [Atlas §9](ahead-workflows.md#9-storage-and-artifact-convention) |
| Artifact review, PR context, and CI evidence can stay connected to the task. | PR review is a distinct, revision-pinned work type. Reviewers assess code and explicitly shared artifacts while repository policy remains authoritative. | [Atlas §8.1](ahead-workflows.md#81-pull-request-review-and-merge-readiness) |
| A product can suggest next actions and support local, worktree, remote, or multi-repository execution. | AHEAD may offer the next useful action and preserve the actual checkout/host context. Remote daemons and multi-repository orchestration remain optional capabilities driven by real use cases. | [Atlas §§2–8](ahead-workflows.md#2-sdlc-map-and-responsibility-legend) and [TODO.md](../../TODO.md) |

The most valuable HumanLayer idea is the separation of research, decisions,
artifacts, and durable work from any one model conversation. AHEAD combines that
idea with its own human-led editor: the engineer writes business logic, accepts
FIM explicitly at the caret, and remains responsible for design and review.

## Deliberate differences

AHEAD does not mechanically reproduce HumanLayer. In particular, the current
design rejects:

- a mandatory document set or fixed ceremony for every change;
- a generic workflow builder as a prerequisite for useful work;
- per-edit approval cards after a human has already instructed an edit;
- automatic implementation authority inferred from starting research or
  accepting a plan;
- a cloud service, remote daemon, copied database, or original agent process as
  a requirement for reading shared work;
- GitHub state or an AI review as a substitute for the repository's review and
  merge rules;
- raw hidden reasoning or provider-private state as portable project context;
- a claim that an external agent is governed merely because it uses a familiar
  transport, mode name, or tool label.

These differences follow the Constitution: AI should increase human leverage
without replacing comprehension, judgment, skill formation, or accountability.

## Harness research

The harness question was investigated separately because durable task UX does
not prove that an agent transport can enforce AHEAD's lifecycle.

The initial candidate used the maintained
[`agentclientprotocol/codex-acp`](https://github.com/agentclientprotocol/codex-acp)
adapter, which launches Codex App Server and translates ACP requests and events.
The older Zed adapter is archived and points to that maintained replacement.
Preserving an upstream harness means preserving its model-facing tools,
descriptions, schemas, results, errors, instruction layering, tool-call history,
compaction, streaming, cancellation, and model-specific defaults—not merely
reusing protocol DTOs or familiar tool names.

### Transport probe, 2026-09-18

Against adapter `1.12.0` and the locally pinned runtime `0.152.0`, AHEAD
validated:

- a streamed turn from `session/prompt` through message deltas to `end_turn`;
- cancellation ending with `stopReason: cancelled` in about 2.6 seconds;
- `session/load` followed by a turn that recalled the prior conversation;
- persistence of streamed messages in `.ahead/session.db` and reopen from a
  fresh durable controller.

The opt-in evidence lives in `ahead-proxy/tests/harness_e2e.rs` and
`ahead-proxy/tests/harness_session_e2e.rs`.
This established transport, streaming, cancellation, resume, and persistence.
It did not establish AHEAD-owned enforcement or complete harness parity.

### Guardrail probe, 2026-09-18

A raw ACP client advertised client filesystem capabilities, selected each
adapter mode, and asked the agent to create a file at a fixed absolute path.

| Adapter mode | Permission requests | Client `fs/write_text_file` calls | File written |
|---|---:|---:|---:|
| default `agent` | 0 | 0 | yes |
| `read-only` | 0 | 0 | yes |
| `agent-full-access` | 0 | 0 | yes |

The observed agent used its own shell/edit tools. AHEAD received tool-event
intent but did not mediate the effect, so it could not enforce teaching
read-only behavior, mechanical edit scope, per-tool human decisions, or
`CodeAnchor` attribution. Advertising client filesystem capabilities and sending
an ACP mode therefore did not create an AHEAD-owned effect boundary.

This is a dated result for the tested adapter/runtime combination, not a claim
that every ACP implementation behaves identically. It is sufficient evidence
that AHEAD must test actual effects and must not advertise enforcement it does
not own.

### Harness decision, 2026-09-18

AHEAD has two explicit tiers:

1. **Managed agent.** AHEAD runs the managed runtime directly and owns the
   effect boundary. This is the default where AHEAD promises teaching-task
   read-only behavior, workspace-bounded assistance, and edit attribution. The
   user's instruction authorizes the requested work; the runtime allowlist and
   sandbox bound effects without per-tool approval cards. It preserves the
   pinned runtime's native harness contract rather than implementing a
   look-alike loop.
2. **External ACP agents.** AHEAD offers curated installable adapters for Pi,
   Codex, and Claude Code. ACP provides streaming, cancellation, resume, and
   conversation with these external agents, but their shell and file effects
   remain outside AHEAD's control. Keep these sessions visibly external; do not
   present them as governed Learn or Assist sessions. AHEAD's enforcement,
   attribution, and read-only guarantees apply only to the managed/native tier.

### Native integration cutover, 2026-09-22

The managed implementation now follows Zed's native-agent split at commit
`418f89714891f9d8105a3e92e60b9a7a5084d232`. Zed's
`crates/agent/src/native_agent_server.rs`, `crates/agent/src/agent.rs` and
`crates/agent/src/thread.rs` directly integrate the built-in loop; only
`crates/agent_servers/src/acp.rs` launches external ACP processes. AHEAD now does
the same: `NativeClient` calls the retained core `ThreadManager` directly in the
proxy process, while `HarnessClient` is external-agent ACP only. The Codex App
Server executable and its extra JSON-RPC hop are no longer part of AHEAD.

This cutover also corrected a policy bug found by the Pi probe. ACP session modes
belong to the external agent: Pi uses them for reasoning effort. AHEAD now keeps
its teaching/assistance policy in a separate map used for permission responses
and only sends an ACP mode when that exact id was advertised by the agent. An
external `current_mode_update` can no longer weaken AHEAD's policy state.

The production chat route now presents the current session policy hash and
fails closed before runtime startup if it is missing or stale. The controller
rejects overlapping turns for one session, preserves non-cancellation aborts as
failures, and settles an external ACP prompt immediately on cancel. Native
input requests retain blocking and multi-select semantics; secret answers are
declined rather than exposed in the transcript. Compaction and unsupported
dynamic client-tool calls are handled explicitly instead of disappearing into
the event catch-all.

On editor startup, the UI restores the durable session list and newest active
conversation from `.ahead/session.db` without blocking the GPUI foreground
thread. Session-list reads isolate unreadable legacy rows so one incomplete
record cannot hide healthy conversations. This path was exercised across two
clean native-app restarts on 2026-09-23. The thread-rail close action records a
non-destructive archive in the same database; a live create/archive/restart pass
verified that archived sessions stay hidden while their durable state remains.
Authenticated turn, stop, rendered failure and complete Markdown interaction
remain separate acceptance gates.

The Pi adapter is installed with the Zed-style pinned npm layout and successfully
completes initialize, new-session, command discovery and prompt lifecycle. The
local Pi model request currently returns `AccessDeniedException: Bearer Token has
expired`, so a successful model delta remains an authentication gate rather than
an AHEAD protocol failure.

The architecture and current acceptance gates belong in
[Editor MVP §§3.4–3.5](ahead-editor-mvp.md#34-own-the-policy-boundary-reuse-the-agent-machinery),
the user-visible requirement belongs in
[Atlas W2](ahead-workflows.md#1-requirements-and-decisions), and unfinished work
belongs in [TODO.md](../../TODO.md). This section remains only because it records
why the two-tier decision was made.

## Sources consulted

- HumanLayer: [skills and workflows](https://docs.humanlayer.com/reference/skills-workflows),
  [workflow phases](https://docs.humanlayer.com/explanation/workflow-phases),
  [task model](https://docs.humanlayer.com/explanation/tasks),
  [workspace model](https://docs.humanlayer.com/explanation/workspace-model),
  [release notes](https://docs.humanlayer.com/release-notes), and
  [remote daemons](https://docs.humanlayer.com/explanation/remote-daemons).
- Agent Client Protocol: [filesystem methods](https://agentclientprotocol.com/protocol/v1/file-system),
  [plans](https://agentclientprotocol.com/protocol/v1/agent-plan), and
  [session modes](https://agentclientprotocol.com/protocol/v1/session-modes).
- ACP Registry: [latest registry](https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json).
- Pi ACP: [source and protocol notes](https://github.com/svkozak/pi-acp).
- OpenAI: [Codex prompting guide](https://developers.openai.com/cookbook/examples/gpt-5/codex_prompting_guide),
  [harness integration rationale](https://openai.com/index/unlocking-the-codex-harness/),
  and [App Server reference](https://learn.chatgpt.com/docs/app-server).

These sources explain the ideas and historical comparison. They do not override
AHEAD's Constitution, workflow atlas, editor architecture, tests, or observed
runtime behavior.
