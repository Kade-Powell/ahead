# Developing AHEAD

<img src="../../extra/images/logo.svg" width="64" height="64" alt="AHEAD logo"/>

Audience: AHEAD editor maintainers

This repository is **AHEAD, the editor**, a maintained fork of Lapce. The editor design defines native human-led collaboration, full-duplex streaming voice, governed agent runtime, and contextual predictions.

## Principles, practice & evidence

- [AHEAD Constitution](../../CONSTITUTION.md): The small set of durable principles that defines AHEAD.
- [Engineering practice](../guide/engineering-practice.md): Practical engineering guidance distilled from the original notes and reading.
- [Evidence library](../evidence/README.md): Research mapping, evidence strength, limitations, and preserved source notes.

## Editor design & architecture

- [Brand and theme guide](branding.md): Approved logo, app icons, semantic colors, and theme sources.
- [Workflow atlas](ahead-workflows.md): Start here to review SDLC flows, human/AI roles, investigation, guided debugging, teaching/voice, decisions and standard artifact locations.
- [Artifact templates](../../defaults/artifacts/README.md): Built-in low-friction session, research, design, plan, verification and review records, with optional project overrides.
- [HumanLayer workflow research](ahead-humanlayer-workflows.md): Dated source observations, adopted and rejected ideas, harness probe evidence, and decision provenance. It is not a second workflow specification.
- [AHEAD Editor MVP](ahead-editor-mvp.md): The product specification and architecture proposal, including native collaboration, streaming voice, agent boundaries, contextual predictions, tracker integration, and delivery gates.
- [Decision 0005: Zed source reuse and editor parity](decisions/0005-zed-editor-source-reuse.md): Accepted scope for direct Zed source reuse, editor parity, terminal/tasks, debugging, agent context, voice, and deliberate deferrals.
- [Editor DTO draft](ahead-editor-contracts.ts): Self-contained, type-checkable editor contracts and protocol shapes for sessions, workflow state, anchors, voice events, predictions, and reviews.

## Agent Guidance & UI Components

See [AGENTS.md](../../AGENTS.md) for canonical agent instructions.
See the [agent standards boundary](ahead-agent-standards.md) for normative `AGENTS.md`, Agent Skills, ACP and MCP contracts; Zed is an implementation reference, not an instruction-format compatibility target.
All production UI is built using **GPUI** and **gpui-kit** (https://gpui-kit.com).
Make sure to use **gpui-kit components whenever possible** (https://gpui-kit.com/component/); it provides pre-built dock areas, editor states, inputs, buttons, flex layouts, and dialogs.

## Upstream maintenance

This repository preserves Lapce's Git ancestry and upstream source history. The active editor uses the renamed `ahead-app`, `ahead-core`, `ahead-proxy`, and `ahead-rpc` crates. AHEAD features are concentrated at explicit extension points:
- **Session host & policy**: Governed workflow phases, explicit teaching tasks, assistance-by-default tasks, and Turso/libSQL session persistence.
- **Built-in skills**: AHEAD-owned progressive-disclosure skills under `ahead-agent/skills/`; human-led teaching, diagnosis, research, design vocabulary, triage, prototyping and automated review.
- **Workspace skills**: Discover user-provided `SKILL.md` files under project `.agents/skills/` and user `~/.agents/skills/` as extra instructions, subject to AHEAD policy and host authorization.
- **Workspace instructions**: Managed chats load project `AGENTS.md` files from the discovered workspace root through the turn working directory. AHEAD also adds the applicable nested files for structured editor and attached-file targets; FIM receives the hierarchy for its active and relevant open-buffer targets. These are standard Markdown project instructions, not a required AHEAD-specific file. AHEAD records the loaded source paths and full-file hashes in Turso. External ACP agents retain their own instruction-discovery behavior; details and limits are in the [agent standards boundary](ahead-agent-standards.md).
- **Agent skill architecture**: [AHEAD agent skills](ahead-agent-skills.md) is the canonical catalog, policy, discovery, provenance and implementation-status document.
- **Voice runtime**: Full-duplex streaming audio and transcript bus with barge-in interruption.
- **Edit prediction**: Fast context assembler incorporating active work, unsaved buffers, and diagnostics.
- **Review & anchors**: Stable code anchors, snapshot-bound review, and GitHub tracker outbox.
