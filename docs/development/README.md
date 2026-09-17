# Developing AHEAD

Audience: AHEAD editor maintainers

This repository is **AHEAD, the editor**, a maintained fork of Lapce. The editor design defines native human-led collaboration, full-duplex streaming voice, governed agent runtime, and contextual predictions.

## Editor design & architecture

- [AHEAD Editor MVP](ahead-editor-mvp.md): The product specification and architecture proposal, including native collaboration, streaming voice, agent boundaries, contextual predictions, tracker integration, and delivery gates.
- [Editor DTO draft](ahead-editor-contracts.ts): Self-contained, type-checkable editor contracts and protocol shapes for sessions, workflow state, anchors, voice events, predictions, and reviews.

## Upstream maintenance

This repository preserves Lapce's Git ancestry, crate layout (`lapce-app`, `lapce-core`, `lapce-proxy`, `lapce-rpc`), and directory structure. AHEAD features are concentrated at explicit extension points:
- **Session host & policy**: Governed workflow phases, Learn/Assist modes, and SQLite session persistence.
- **Voice runtime**: Full-duplex streaming audio and transcript bus with barge-in interruption.
- **Edit prediction**: Fast context assembler incorporating active work, unsaved buffers, and diagnostics.
- **Review & anchors**: Stable code anchors, snapshot-bound review, and GitHub tracker outbox.
