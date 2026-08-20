# Acceptable AI Use in AHEAD

Audience: AHEAD practitioners

Status: binding pilot policy v0.1

## Authority

This policy is binding for AHEAD pilot workflows. The [AHEAD Constitution](../../CONSTITUTION.md) is the highest authority; this policy interprets its human-ownership boundaries; workflow profiles apply both and may narrow AI permissions for a phase. A workflow phase, diagram, tool permission, or local convenience cannot broaden AI authority beyond this policy. Organization and repository policies may impose stricter controls. Because AHEAD is an AI-assisted methodology, it cannot be used as designed where AI use is prohibited entirely.

If two rules appear to conflict, apply the more protective rule and record the ambiguity for correction. No exception may transfer human authorship, judgment, approval, accountability, or unscoped consequential authority to AI.

## Purpose

AHEAD encourages substantial AI assistance while preserving human understanding, judgment, skill, and accountability. Acceptable use is determined by the role AI plays, the risk of the work, the information exposed, and the engineer's ability to verify and own the result—not simply by how many lines AI produced.

The default relationship is:

```text
Human supplies intent, context, and an initial model
→ AI retrieves, generates, compares, or challenges
→ Human verifies, decides, and owns the result
```

## Conditions for acceptable use

An AI-assisted contribution is acceptable only when all applicable conditions hold:

1. A human owns the problem, intended behavior, and consequences.
2. The AI's task is bounded and appropriate to the active workflow phase.
3. The model receives only information authorized for that provider, tool, and purpose.
4. Claims, citations, commands, dependencies, and generated artifacts can be checked against authoritative evidence.
5. The responsible engineer can explain, maintain, debug, test, and modify accepted code.
6. Verification is proportionate to risk and does not rely solely on the same AI that produced the work.
7. Material AI involvement remains visible when repository, organizational, legal, or review policy requires it.
8. The AI has no implicit authority to approve, merge, deploy, communicate commitments, accept risk, or act in production.

## Generally acceptable uses

### Research and information gathering

AI may locate, organize, compare, and summarize approved internal and external sources. The engineer follows important claims to their sources and distinguishes retrieved evidence from AI synthesis.

Why: AI increases search and synthesis capacity, while NIST identifies confident false content as an inherent generative-AI risk. Summaries are navigation aids, not new authorities.

For novel, complex, or unfamiliar work, prefer primary documentation, direct inspection, and experiments. State what the AI may not know and treat confident synthesis as unverified until evidence supports it.

### Boilerplate and repetitive structure

AI may generate scaffolding, serializers, routine handlers, adapters, setup code, repetitive mappings, and other bounded structure after the human identifies the intended pattern and integration boundary.

The engineer checks local conventions, error handling, dependencies, security posture, and whether generation introduced unnecessary abstraction. “Boilerplate” is not a label that makes risky code safe.

### Disposable prototypes and full vibe coding

Full “vibe coding”-prompting AI to produce most or all of an implementation without first understanding every detail-is acceptable for a disposable prototype whose purpose is to learn quickly whether an idea is feasible or how an experience might turn out.

This exception applies only when all of these boundaries are explicit:

- the learning question and prototype status are stated before generation;
- the prototype is isolated from production systems, credentials, customer data, and consequential decisions;
- nobody relies on its correctness, security, scalability, accessibility, or maintainability;
- generated dependencies, licenses, and external content remain subject to inspection;
- the prototype has an owner, an expiration or disposal decision, and a conspicuous `PROTOTYPE — NOT FOR PRODUCTION` label;
- demonstrations disclose material limitations rather than presenting simulated behavior as validated capability;
- any code selected for retention leaves the prototype exception and enters the normal AHEAD process for human understanding, design, implementation, testing, security review, and approval.

Throwaway describes the artifact, not the evidence learned from it. Preserve useful observations, constraints, failed approaches, and decisions as durable artifacts, then discard the code. A prototype must not become production software through incremental cleanup or repeated deployment.

This is an AHEAD operating policy supported by established prototyping practice, not evidence that unrestricted vibe coding produces production-quality systems.

### Candidate tests

AI may propose test cases, fixtures, mocks, generators, boundary values, and failure scenarios after a human defines the expected behavior, important invariants, and test oracle.

Tests are reviewed as production code. The engineer checks that they can fail, do not merely encode the current implementation, cover meaningful states and failure paths, and do not silently weaken or replace existing tests. Security-critical tests require independent human authorship or review.

Research has found that LLM support can increase generated tests and defect detection, but also test volume and false positives. Test-guided interaction can improve people's ability to evaluate generated code. These findings support AI-assisted testing with human-defined intent and independent verification—not treating passing AI-generated tests as proof.

### First-pass review

AI may perform an additional early review for likely defects, security concerns, missing tests, plan divergence, unclear code, and relevant edge cases.

AI review never constitutes final approval. Findings are hypotheses until a human validates them. Human review remains required because empirical work links substantive review participation and expertise with quality, while current LLM reviewers exhibit systematic false-positive and conformance-judgment failures.

For a lasting engineering change, independent human review means review by a person other than the implementer. The implementer and reviewer must both understand the behavior, risks, and evidence relevant to their responsibilities. Implementer self-review and AI review do not satisfy this gate. An emergency policy may defer independent review to restore service, but the gate remains unsatisfied until a named human reviewer completes it after stabilization.

### Refactoring proposals

AI may identify duplication, coupling, unclear names, complicated control flow, possible abstractions, or opportunities to isolate state. The human decides whether the proposed change makes the artifact simpler and verifies preserved behavior.

AI may not manufacture a justification for a refactor or substitute rearranged files for reduced conceptual complexity.

### Documentation and communication assistance

AI may organize engineer-provided facts, edit for clarity, create an outline, derive reference documentation from verified interfaces, and draft routine sections that the named author reviews.

The human supplies and verifies the reasoning, status, decisions, commitments, uncertainty, and audience judgment. AI must not fabricate citations, test results, incident facts, user claims, approvals, or confidence. It must not make weak understanding sound authoritative.

### Debugging support

AI may help organize evidence, explain unfamiliar mechanisms, generate competing hypotheses, identify contradicting evidence, and propose discriminating tests.

Before broad AI assistance in normal debugging, the human records at least the observed failure, evidence already checked, and an initial mental model or question. A team may use a timebox before asking AI, but the purpose is independent thought—not withholding useful tools. During incidents, AI may assist immediately once the current observation and safety constraints are established.

Debugging is not grunt work. The human remains the investigator: selecting or authorizing tests, interpreting results, deciding whether evidence supports a conclusion, and choosing the intervention.

### Design alternatives

After the human frames the problem and contributes an initial option, AI may broaden the option set, compare tradeoffs, find examples, expose assumptions, and argue against the favored approach.

The human-first sequence is an AHEAD policy and design hypothesis, not a proven universal optimum. Adjacent experimental evidence shows that generative-AI ideas can improve individual creative output while reducing collective diversity, giving AHEAD a reason to test this sequence rather than claim it is settled.

### Learning and onboarding

AI may explain code, concepts, patterns, libraries, tools, and unfamiliar terminology. Prefer explanations, questions, small examples, critiques, and hints that require the learner to retrieve and apply knowledge.

When skill formation is the goal, avoid full-solution delegation. A randomized study of developers learning an unfamiliar Python library found lower mastery for the AI-assisted group overall; participants who used AI to ask conceptual questions and build comprehension did better than those who delegated the work.

### Examples, mocks, and planning mechanics

AI may generate non-sensitive sample payloads, mock data, fixtures, command examples, ticket structure, acceptance-criteria candidates, and summaries of human-provided notes.

Generated examples are checked for realism, privacy, security, and accidental use of production identifiers. AI may organize a human first-pass plan; it may not create the reasoning and relabel it as human-authored.

## Uses requiring heightened review

AI may assist in these areas, but it does not originate the governing policy or act without explicit human control:

- authentication, authorization, cryptography, privacy, and security controls;
- business rules, financial calculations, safety constraints, and regulated behavior;
- database schemas, migrations, destructive maintenance, and data correction;
- concurrency, distributed consistency, recovery, and idempotency;
- dependencies, generated build steps, package scripts, and supply-chain changes;
- infrastructure, CI/CD, identity, permissions, and secrets;
- production commands, remediation, deployment, rollback, or containment;
- legal, compliance, personnel, customer, or public communication;
- generated licenses, attribution, or content with unclear provenance.

These uses require a named human owner, authoritative requirements, explicit scope, risk-appropriate tests, relevant specialist review, and separately authorized effects. Critical code and its only validation should not come from one AI context.

## Unacceptable uses

### Submitting work the engineer does not understand

Do not accept long-lived code that its responsible engineer cannot explain, maintain, debug, test, and safely change. Passing current tests is not sufficient.

### Outsourcing core business logic

AI may explain, challenge, review, or help test core business rules. Under AHEAD, the accountable engineer authors their implementation and traces it to human-approved requirements. This is a constitutional policy choice whose precise boundary must be refined by teams.

### Letting AI define behavior or policy

AI is not the authority for business rules, authorization, privacy, data retention, workflow behavior, acceptable risk, or production semantics. It may retrieve an authoritative rule; it may not become that authority.

### Blind copying or unverified execution

Do not copy generated code, commands, dependencies, citations, or configuration without inspecting their meaning and validating them in the relevant environment. Never treat confident explanation as evidence.

### Bypassing learning

Do not use AI to avoid acquiring skills needed to oversee the resulting system. When learning is the objective, the workflow should favor explanation, retrieval, prediction, and modification over full answer generation.

### AI-first debugging with no human observation

Do not reduce debugging to pasting an error into a model and following its first answer. Establish what was observed and what evidence exists. AI-generated hypotheses are candidates, not diagnoses.

### Outsourcing judgment or approval

AI may not select the team's technical opinion, accept tradeoffs, waive a gate, approve a change, declare an incident recovered, or perform final engineering review.

### Impersonating human communication

AI may help edit or structure communication, but it may not invent reasoning and present it as the named person's own understanding. Humans own PR rationales, incident updates, decisions, commitments, and explanations sent under their identity.

### Polishing over uncertainty

Do not use AI to make incomplete evidence appear conclusive, conceal disagreement, erase caveats, or make a person sound as though they understand work they have not examined.

### Unauthorized data or access

Do not expose secrets, credentials, personal data, confidential source, customer information, incident evidence, regulated data, or proprietary material to an unapproved model or provider. Do not assume `.gitignore` prevents an editor assistant from reading a file.

Do not give an AI agent broader filesystem, network, repository, CI, cloud, or production permissions than its bounded task requires. Repository text, issues, logs, web pages, and tool output are untrusted inputs that may attempt to redirect the agent.

## Verification checklist

Before accepting material AI-assisted work, the responsible engineer answers:

- What did the human decide before AI involvement?
- What did the AI contribute?
- Can I explain the result without appealing to the AI's authority?
- Which facts, requirements, and sources did I independently verify?
- What tests could disprove the implementation or conclusion?
- Did the same AI create both the work and all evidence used to validate it?
- What sensitive context was shared, with which provider, under what policy?
- Did the AI introduce dependencies, licenses, commands, or external content?
- What could fail outside the examples the AI saw?
- Am I willing and authorized to own this result?

For a disposable prototype, also answer:

- What specific question is this prototype meant to answer?
- Where is it isolated, and what production access or sensitive data is excluded?
- When will it be discarded or deliberately re-enter the normal engineering workflow?

## Evidence and sources

- [Cui et al., *The Effects of Generative AI on High-Skilled Work*](https://doi.org/10.1287/mnsc.2025.00535) — randomized field evidence that coding assistants can increase completed tasks in some organizational contexts.
- [Becker et al., *Measuring the Impact of Early-2025 AI on Experienced Open-Source Developer Productivity*](https://arxiv.org/abs/2507.09089) — randomized evidence that the same broad category of tool can slow experienced developers in familiar repositories, despite perceived speedups.
- [Shen and Tamkin, *How AI Impacts Skill Formation*](https://arxiv.org/abs/2601.20245) — randomized study of AI assistance, code understanding, and learning an unfamiliar library.
- [Ramler et al., *Unit Testing Past vs. Present*](https://arxiv.org/abs/2502.09801) — experiment on LLM-supported unit testing, defect detection, and false positives.
- [Fakhoury et al., *LLM-Based Test-Driven Interactive Code Generation*](https://www.microsoft.com/en-us/research/publication/llm-based-test-driven-interactive-code-generation-user-study-and-empirical-evaluation/) — user study and evaluation of test-guided intent clarification.
- [Jin and Chen, *Are LLMs Reliable Code Reviewers?*](https://arxiv.org/abs/2603.00539) — evidence of systematic overcorrection in LLM requirement-conformance review.
- [Doshi and Hauser, *Generative AI enhances individual creativity but reduces the collective diversity of novel content*](https://doi.org/10.1126/sciadv.adn5290) — adjacent experimental evidence relevant to AI anchoring and option diversity.
- [Bjarnason, Lang, and Mjöberg, *An empirically based model of software prototyping*](https://doi.org/10.1007/s10664-023-10331-w) — a systematic mapping study and multi-company study of prototypes used to explore and validate feasibility, desirability, usability, and requirements.
- [NIST AI 600-1, *Generative Artificial Intelligence Profile*](https://doi.org/10.6028/NIST.AI.600-1) — authoritative risk guidance on confabulation, privacy, provenance, testing, and governance.
- [NIST DevSecOps reference model: Artificial Intelligence](https://pages.nist.gov/nccoe-devsecops/notational-reference-model.html#artificial-intelligence) — human validation and verifiable-process guidance for AI-augmented software development.
- [OWASP Secure Coding with AI Cheat Sheet](https://cheatsheetseries.owasp.org/cheatsheets/Secure_Coding_with_AI_Cheat_Sheet.html) — practitioner security guidance for sensitive context, agent permissions, generated tests, supply chains, and CI/CD.
- [Google Summer of Code, *Guidance for GSoC Contributors using AI tooling in GSoC 2026*](https://developers.google.com/open-source/gsoc/resources/ai_guidance) — practitioner guidance summarizing mentor advice on human responsibility, understanding, research, human-defined test scope, and the limits of AI on complex work.

The [research map](../evidence/research-map.md) classifies these sources and records their limitations. Exact boundaries such as “core business logic” and the human-first debugging checkpoint remain AHEAD policy choices that should be tested and refined.
