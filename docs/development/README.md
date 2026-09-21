# Developing AHEAD

Audience: AHEAD editor maintainers

This repository is **AHEAD, the editor**, a maintained fork of Lapce. The editor design defines native human-led collaboration, full-duplex streaming voice, governed agent runtime, and contextual predictions.

## Editor design & architecture

- [Workflow atlas](ahead-workflows.md): Start here to review SDLC flows, human/AI roles, investigation, guided debugging, teaching/voice, decisions and standard artifact locations.
- [HumanLayer workflow research](ahead-humanlayer-workflows.md): Product comparison and proposed adaptation, harness choice, portable session history and collaboration ideas.
- [AHEAD Editor MVP](ahead-editor-mvp.md): The product specification and architecture proposal, including native collaboration, streaming voice, agent boundaries, contextual predictions, tracker integration, and delivery gates.
- [Decision 0005: Zed source reuse and editor parity](decisions/0005-zed-editor-source-reuse.md): Accepted scope for direct Zed source reuse, editor parity, terminal/tasks, debugging, agent context, voice, and deliberate deferrals.
- [Editor DTO draft](ahead-editor-contracts.ts): Self-contained, type-checkable editor contracts and protocol shapes for sessions, workflow state, anchors, voice events, predictions, and reviews.

## Agent Guidance & UI Components

See [AGENTS.md](../../AGENTS.md) for canonical agent instructions.
All production UI is built using **GPUI** and **gpui-kit** (https://gpui-kit.com).
Make sure to use **gpui-kit components whenever possible** (https://gpui-kit.com/component/); it provides pre-built dock areas, editor states, inputs, buttons, flex layouts, and dialogs.

## Upstream maintenance

This repository preserves Lapce's Git ancestry, crate layout (`lapce-app`, `lapce-core`, `lapce-proxy`, `lapce-rpc`), and directory structure. AHEAD features are concentrated at explicit extension points:
- **Session host & policy**: Governed workflow phases, explicit teaching tasks, assistance-by-default tasks, and SQLite session persistence.
- **Built-in skills**: AHEAD-owned progressive-disclosure skills under `ahead-harness/skills/`; human-led teaching, diagnosis, research, design vocabulary, triage, prototyping and automated review.
- **Workspace skills**: Discover user-provided `SKILL.md` files under `.agents/skills/`, `.agent/skills/` and `.skills/` as extra instructions, subject to AHEAD policy and host authorization.
- **Workspace instructions**: Read applicable `AGENTS.md` files from the workspace root through each target directory before skill selection or task actions; record their hashes for resumable tasks.
- **Agent skill architecture**: [AHEAD agent skills](ahead-agent-skills.md) is the canonical catalog, policy, discovery, provenance and implementation-status document.
- **Voice runtime**: Full-duplex streaming audio and transcript bus with barge-in interruption.
- **Edit prediction**: Fast context assembler incorporating active work, unsaved buffers, and diagnostics.
- **Review & anchors**: Stable code anchors, snapshot-bound review, and GitHub tracker outbox.
