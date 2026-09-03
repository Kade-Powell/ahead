# AHEAD Practitioner Guide

Audience: AHEAD practitioners

This is the starting point for people applying AHEAD to engineering work. These documents explain what practitioners are expected to understand, decide, record, and review. They do not describe how the Rust engine, Pi adapter, instruction generator, or release process is implemented.

## Start here

1. [Why AHEAD](rationale.md) explains the philosophy and intended human–AI relationship.
2. [AHEAD Constitution](../../CONSTITUTION.md) defines the durable, non-negotiable principles.
3. [Acceptable AI use](acceptable-ai-use.md) defines binding authority boundaries.
4. [Engineering practice](engineering-practice.md) describes the habits AHEAD asks engineers to cultivate.
5. [Pilot workflows](workflows/README.md) explains how to choose and execute one of the six flows.
6. [Work items and planning handoffs](work-items.md) explains provider-neutral links, configurable gates, and sprint-ahead preparation.
7. [Durable references](durable-reference.md) defines the three durable artifact classes AHEAD standardizes — decision records, design docs, and runbooks — and adopts Diátaxis for the rest.
8. [Recommended skills](recommended-skills.md) lists optional, reviewed third-party aids.

## Authority

The Constitution, Acceptable AI Use policy, and active workflow contract are binding during an AHEAD pilot. The rationale explains why those rules exist. Engineering Practice, durable references, and work-item guidance are proposed guidance, and recommended skills are optional. Organization and repository rules may narrow AI authority but cannot broaden it beyond AHEAD policy.

## Scope

AHEAD prescribes run artifacts, workflow gates, decision records, design docs, and runbooks. It explicitly leaves tutorials, most how-to guides, user-facing product documentation, contributor references, and project-specific coding-agent instructions to each project. [Durable references](durable-reference.md) names what AHEAD covers and points at [Diátaxis](https://diataxis.fr/) for the rest.

## When you need the basis for a rule

Use the [evidence library](../evidence/README.md) for research, limitations, and original source notes. Evidence documents support and challenge the framework; they are not additional workflow steps unless a workflow explicitly requires them.

## Tooling

[AHEAD for Pi](https://github.com/Kade-Powell/ahead/tree/main/integrations/pi) explains installation, guided mode, commands, review handoff, identity, and current pilot limitations. Implementation architecture and release procedures belong in the repository's [development guide](https://github.com/Kade-Powell/ahead/blob/main/docs/development/README.md), which is intentionally not packaged as runtime guidance.

Stopping AHEAD does not imply completion. An integration should discard unfinished workflow records by default without touching engineering work, retain them only when the human explicitly chooses to save, and resume saved work at the same phase and gates.
