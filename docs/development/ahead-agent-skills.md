# AHEAD agent skills

Status: project `AGENTS.md` loading from root through turn cwd and Agent Skills
discovery are implemented in source, with focused library, native-reader, RPC
and headless composer tests passing. Built-in AHEAD skill packages live under
`ahead-agent/skills/`; project and user skills resolve from `.agents/skills/`
and `~/.agents/skills/`. Live managed-turn loading, persisted skill provenance,
target-scoped instruction discovery and rendered end-to-end behavior remain
open; see `TODO.md`.

This is the canonical overview of AHEAD's built-in and workspace skill model.
Individual `SKILL.md` files contain the detailed procedure for one skill.

## 1. Purpose

AHEAD uses Agent Skills as progressively disclosed procedures and reference
vocabulary for its built-in agent. Skills help the agent choose a useful way to
assist; they do not become a second policy engine, permission system or task
database.

AHEAD follows the portable Agent Skills shape:

```text
skill-name/
  SKILL.md             required metadata and instructions
  references/          conditional supporting material
  scripts/             optional audited helpers
```

The initial metadata contains each skill's name and description. The agent or
host loads the body only when the task matches, then loads references only when
the selected procedure needs them. This keeps unrelated procedures out of the
active context.

Reference inspiration: [Anthropic Agent Skills](https://platform.claude.com/docs/en/agents-and-tools/agent-skills/overview).
AHEAD's skills are reimplemented for AHEAD's session host, task intents, code
anchors, GPUI presentation and human-led policy; they are not assumed to be
drop-in copies of another agent's workflows.

## 2. Task intent, not session mode

Sessions do not have a binary Learn/Assist switch. A session is the durable
container; each request creates a task with one of two intents:

### Teaching

Created when the human explicitly asks to learn. Teaching tasks:

- use verified code and document anchors;
- teach one coherent concept at a time;
- use mission, calibration, retrieval, practice and learning evidence;
- preserve the human caret and selection;
- cannot execute commands, apply edits or use edit prediction;
- may create a linked learning arc during another task.

### Assistance

The default for every other request: feature work, investigation, debugging,
research, design, review, operations and maintenance. Assistance capabilities
remain bounded by the current phase, participant, scope, backend and host policy.

“Teach me while we debug this” creates an assistance task with a linked teaching
arc. Debugging remains the parent for hypotheses, experiments, breakpoints and
regression evidence; teaching explains verified observations without changing
the debugging policy.

## 3. Built-in AHEAD skills

These are shipped under `ahead-agent/skills/` and are AHEAD-owned versions:

| Skill | Role | Activation |
|---|---|---|
| `ahead-teaching` | Mission-based, code-grounded teaching and learning evidence | Explicit teaching task or linked teaching arc |
| `ahead-diagnosing-bugs` | Reproduction, minimization, falsifiable hypotheses, experiments and regression evidence | Assistance task with a hard bug, regression or performance symptom |
| `ahead-research` | Bounded primary-source research that leaves a cited artifact | A decision depends on external or version-sensitive facts |
| `ahead-codebase-design` | Vocabulary and design questions about modules, interfaces, seams and depth | Design/architecture work; reference layer, not a driver workflow |
| `ahead-code-review` | Automated Standards and Spec review of a frozen snapshot | Explicit review or normal end-of-session review |
| `ahead-triage` | Clarify and classify an unconfirmed report | Before deep diagnosis when evidence or scope is incomplete |
| `ahead-prototype` | Small disposable feasibility experiment | The human needs evidence that an approach works in this codebase |

The initial bundle does not include standalone `handoff` or `tdd` skills:

- Handoff is native AHEAD session checkpoint/resume behavior.
- TDD is an optional engineering method. AHEAD requires appropriate
  verification, not one universal test-first method. Diagnosis still requires
  regression evidence after a confirmed failure.

Other useful ideas can be folded into these skills or later AHEAD skills:

- domain modeling and grilling become vocabulary/decision behavior inside design;
- research feeds teaching, design and diagnosis;
- triage routes into diagnosis;
- codebase design informs design and architecture review;
- code review closes the session with findings and remaining risk.

## 4. Human-led policy

The human owns the outcome, business behavior, hypotheses, experiment selection,
final decisions and external publication.

The session host—not a skill—authorizes effects. Skills may recommend tools or
ask for evidence, but cannot grant command execution, file mutation, debugger
launch/stepping/evaluation, tracker writes, approvals or publication.

The effective policy is the intersection of workflow phase, task intent,
participant role, explicit scope, backend capability, current code revision and
host authorization. A skill cannot weaken AHEAD rules by giving the model a
different instruction, tool name or apparent approval.

See the shared policy in
[`ahead-agent/skills/references/human-led-policy.md`](../../ahead-agent/skills/references/human-led-policy.md).

## 5. Workspace instructions and skill discovery

Managed chats load project `AGENTS.md` files from the discovered workspace root
through the turn's working directory. Structured editor and attached-file
targets add applicable nested `AGENTS.md` files along those target paths. No
AHEAD-specific or user-global instruction file is required. External ACP
agents remain responsible for their own instruction discovery.

For a task spanning multiple directories, AHEAD assembles the applicable
instruction set for each tree and surfaces conflicts. More-specific instructions
add detail to broader instructions unless they conflict with a higher-priority
rule.

In addition to built-ins, discover workspace-local `SKILL.md` files from
`.agents/skills/` and user skills from `~/.agents/skills/`. The Agent Skills
format defines package contents, not discovery roots; its client guide
recommends these paths as cross-client conventions. AHEAD does not also scan
`.agent/skills/` or `.skills/` by default.

Keep same-named skills from different sources separately discoverable; the
composer labels their source and uses a source-qualified slash command to
preserve the human's choice. An unqualified name is activated only when it is
unique. This duplicate-selection and slash-command behavior belongs to AHEAD's
UI contract; the Agent Skills format does not define activation syntax or
collision handling.

AHEAD dogfoods this contract. Runtime-maintenance procedures adapted from the
Codex CLI live beside the other project skills under `.agents/skills/`, use an
`ahead-agent-` prefix, and are selected by the scoped `AGENTS.md`. They are not
part of the built-in AHEAD skill bundle.

Only skill metadata is read during discovery. Bodies and resources load after
selection or host activation. Do not recursively scan arbitrary directories,
install skills automatically or fetch skill content from the network.

The host-resource contract keeps handles opaque and source-qualified; they never
contain absolute skill paths. A selected host skill carries a
`<resource_access>` locator in turn context. AHEAD's native `skill_resource_read`
tool accepts that package handle and either the main resource or a
package-relative resource such as `<package>/references/guide.md`. The host
snapshot boundary rejects unknown or disabled skills, and the retained filesystem
boundary rejects absolute paths, parent traversal, Windows-separator aliases,
and resources whose canonical target leaves the skill directory. Resource
contents are bounded and returned in cursor-checked UTF-8 pages.
This follows Zed's global-skill read boundary in
`../zed/crates/agent/src/tools/read_file_tool.rs::read_global_skill_file`, while
keeping AHEAD's source-qualified package handle opaque instead of sending an
absolute user-home path to the model.

The copied generic Codex skills catalog/provider, extension installer, and
`skills.list`/`skills.read` adapters have been pruned. `ThreadManager::new_for_ahead`
also uses `empty_extension_registry()`. AHEAD exposes its own direct dynamic
reader through `HostSkillsSnapshot::read_package_resource`. Package-resource
containment and native UTF-8 paging/stale-cursor regressions passed on
2026-09-28; live managed-turn nested-reference verification remains open. See
`TODO.md` for the remaining gates.

`AGENTS.md` and workspace skills provide extra instructions, not capability
grants. AHEAD's host policy and explicit human scope always win.

## 6. Provenance and conflicts

Skill names are namespaced so local content cannot silently replace a built-in:

```text
ahead:<name>                    built-in AHEAD skill
workspace:<root>:<name>         project-local skill
user:<root-id>:<name>           configured user skill
```

Record the following in task/session context:

- skill ID and revision;
- source root and trust classification;
- content hash;
- selected-by actor (`human`, `agent` or `host`);
- loaded references;
- applicable `AGENTS.md` paths and hashes.

If a skill or instruction conflicts with AHEAD policy, keep it visible as
unavailable, explain the conflict and do not execute the conflicting step.
Scripts, network access, credentials, file changes, debugger effects and
external writes require their ordinary host authorization even when described by
a trusted skill.

## 7. Automated end-of-session review

At the normal end of a session, AHEAD offers `ahead-code-review` against an
immutable snapshot containing the selected base revision, current saved code,
unsaved buffers and relevant untracked files.

The review runs two axes:

- **Standards:** repository rules, ADRs, security/accessibility requirements,
  error handling and known invariants.
- **Spec:** the human-owned task outcome, plan, decisions, examples and evidence.

Automated review may inspect code and run already-authorized read-only checks. It
produces anchored findings, severity, evidence and recommended dispositions. It
does not auto-apply fixes, approve its own findings, close risk, publish a PR or
update a tracker. A human decides what to fix, accept as risk, defer or publish.

## 8. Implementation status

Implemented in source; the following focused regressions passed on 2026-09-28
(see `TODO.md` for command-level evidence):

- AHEAD-owned skill specifications under `ahead-agent/skills/`, separate from
  repository-maintenance skills under `.agents/skills/`;
- project-root `.agents/skills/` and user `~/.agents/skills/` resolution;
- source-qualified selection for same-named project and user skills, including
  duplicate-name handling in the native resolver;
- `skill_resource_read` through the host snapshot, including resource
  containment and bounded UTF-8 paging with cursor validation;
- Agent Skills package validation, project-source RPC privacy, and headless
  slash-palette filtering and selection.

Still incomplete or not demonstrated as live product behavior:

- a live managed-turn demonstration of selected-body and nested-reference
  loading, plus rendered invalid-package diagnostics;
- selected-skill revision/provenance persistence and complete skill/instruction
  conflict handling;
- applicable instruction path/hash capture, target-directory reload, and
  conflict presentation;
- skill selection events and task routing;
- immutable end-of-session review activation;
- rendered teaching, debugging and review journeys.

Implementation gaps are tracked in [`TODO.md`](../../TODO.md), especially the
harness integration and guided investigation sections.
