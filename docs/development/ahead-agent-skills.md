# AHEAD agent skills

Status: accepted product/architecture direction, implementation pending. The
skill specifications exist under `ahead-harness/skills/`; runtime discovery,
loading and host integration remain tracked in `TODO.md`.

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

These are shipped under `ahead-harness/skills/` and are AHEAD-owned versions:

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
[`ahead-harness/skills/references/human-led-policy.md`](../../ahead-harness/skills/references/human-led-policy.md).

## 5. Workspace instructions and skill discovery

Before selecting skills or taking task actions, the agent reads applicable
`AGENTS.md` files:

1. workspace/project root;
2. each more-specific ancestor directory of files being inspected or changed;
3. explicitly configured user instruction roots, if present.

For a task spanning multiple directories, AHEAD assembles the applicable
instruction set for each tree and surfaces conflicts. More-specific instructions
add detail to broader instructions unless they conflict with a higher-priority
rule.

In addition to built-ins, discover workspace-local `SKILL.md` files from:

1. `.agents/skills/`;
2. `.agent/skills/`;
3. `.skills/`;
4. explicitly configured user skill roots.

Only skill metadata is read during discovery. Bodies and resources load after
selection or host activation. Do not recursively scan arbitrary directories,
install skills automatically or fetch skill content from the network.

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

Specified now:

- AHEAD-owned skill files and shared human-led policy;
- task-local skill contracts and provenance fields;
- workspace skill and `AGENTS.md` discovery rules;
- teaching, diagnosis, research, design, triage, prototype and review policy;
- end-of-session automated review behavior.

Not yet demonstrated as live product behavior:

- runtime discovery and metadata registry;
- progressive body/reference loading;
- instruction hash capture and conflict UI;
- skill selection events and task routing;
- immutable end-of-session review activation;
- rendered teaching, debugging and review journeys.

Implementation gaps are tracked in [`TODO.md`](../../TODO.md), especially the
harness integration and guided investigation sections.
