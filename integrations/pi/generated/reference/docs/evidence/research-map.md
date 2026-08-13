# AHEAD Research Map

Audience: AHEAD evidence readers and framework maintainers

Status: living evidence review
Last reviewed: 2026-08-12

This document maps current AHEAD design claims to available evidence. It is not a bibliography of everything related to software engineering. It focuses on claims that materially shape the methodology.

Evidence classes are defined in the [AHEAD Evidence Standard](evidence-standard.md).

## Human-led, AI-amplified engineering

| AHEAD claim | Evidence | Class | What it supports | Important limitation |
|---|---|---:|---|---|
| AI can create meaningful developer leverage. | [Cui et al., *The Effects of Generative AI on High-Skilled Work*](https://doi.org/10.1287/mnsc.2025.00535) combined randomized field experiments at three companies and 4,867 developers; access to a coding assistant was associated with a 26.08% increase in completed tasks, with larger gains among less-experienced developers. | E1 | AHEAD should use AI materially rather than treating it only as a risk. | The intervention was code completion, the experiments were noisy, and completed tasks do not capture understanding or long-term quality. |
| AI's productivity effect is contextual and must be measured. | [Becker et al., *Measuring the Impact of Early-2025 AI on Experienced Open-Source Developer Productivity*](https://arxiv.org/abs/2507.09089) randomized 246 tasks performed by 16 experienced developers in familiar repositories; AI access increased completion time by 19% even though developers believed it had saved time. | E1 | Measure outcomes instead of assuming felt productivity equals actual productivity. Allow AI use to vary by task and expertise. | Small sample, early-2025 tools, experienced open-source developers, and mature familiar repositories. It does not negate larger positive field studies. |
| Human-independent thought before AI may preserve diversity. | [Doshi and Hauser, *Generative AI enhances individual creativity but reduces the collective diversity of novel content*](https://doi.org/10.1126/sciadv.adn5290) experimentally studied 293 short stories. AI ideas improved several individual evaluations but made outputs more similar to one another. | E2 | Provides a mechanism for testing human-first option generation: AI suggestions may improve individual output while narrowing the collective search space. | Creative writing is not software design. This does not directly validate AHEAD's exact sequencing or a mandatory human-first gate. |
| Human capability and automation should improve together. | [Toyota Production System](https://global.toyota/en/company/vision-and-philosophy/production-system/) and [Toyota's account of skilled manufacturing](https://global.toyota/en/newsroom/corporate/35433493.html). | E4 | Establishes a durable practitioner analogy for human-centered automation and improvement through direct mastery. | Manufacturing is not software engineering, and an organizational philosophy is not controlled evidence. |

### Current conclusion

The evidence supports using AI and evaluating it contextually. It gives indirect support for protecting independent thought, but the exact rule “human produces the first option or plan” remains an AHEAD design hypothesis. AHEAD should pilot that rule and measure option diversity, decision quality, explanation quality, and workflow burden.

## Acceptable AI use

| AHEAD claim | Evidence | Class | What it supports | Important limitation |
|---|---|---:|---|---|
| AI assistance should build comprehension, not only produce code. | [Shen and Tamkin, *How AI Impacts Skill Formation*](https://arxiv.org/abs/2601.20245) randomized 52 mostly junior software engineers learning an unfamiliar Python library. The AI-assisted group scored lower on a subsequent mastery assessment; interaction style mattered, and explanation/conceptual use was associated with stronger mastery than delegation. | E1 | Prefer explanations, conceptual questions, prediction, and modification when learning; require engineers to understand accepted work. | One short task, one unfamiliar library, mostly junior participants, and a specific assistant. It does not establish long-term skill effects or justify banning AI for learning. |
| AI can materially assist human test generation and defect discovery. | [Ramler et al., *Unit Testing Past vs. Present*](https://arxiv.org/abs/2502.09801) compared an LLM-supported testing experiment with prior manual-testing experiments and found more tests, coverage, and detected defects, alongside more false positives. | E1 | Allow candidate test generation while requiring human-defined behavior, oracle review, and validation that tests can fail. | Nonconcurrent historical control, master’s-level testing students, seeded defects, and a bounded Java system. More tests and coverage do not prove better production assurance. |
| Test-guided human interaction can improve evaluation of generated code. | [Fakhoury et al., *LLM-Based Test-Driven Interactive Code Generation*](https://www.microsoft.com/en-us/research/publication/llm-based-test-driven-interactive-code-generation-user-study-and-empirical-evaluation/) studied 15 programmers and found the test-guided workflow improved correct evaluation of generated code; the authors also evaluated generation accuracy across models and datasets. | E1 | Define and refine intent through tests rather than asking AI to write code and then trusting tests derived from the same assumptions. | Small user study and an idealized proxy in the scaled evaluation. It does not require strict TDD for all engineering. |
| AI review findings require human validation and executable evidence. | [Jin and Chen, *Are LLMs Reliable Code Reviewers?*](https://arxiv.org/abs/2603.00539) found systematic overcorrection when models judged requirement conformance, including misclassifying correct implementations as defective. | E1 | Use AI review as a first pass; treat findings as hypotheses and retain final human review. | Benchmark-based study of a specific review judgment. It does not measure every type of real-world review or prove human reviewers are sufficient. |
| Generated factual and technical content must be verified. | [NIST AI 600-1](https://doi.org/10.6028/NIST.AI.600-1) identifies confabulation, privacy, information integrity, intellectual-property, and related generative-AI risks. | E3 | Verify claims, citations, summaries, code assumptions, and provenance; prohibit AI from inventing evidence or authority. | Cross-sector risk guidance, not a comparative software-development experiment. |
| AI coding tools require bounded context, permissions, and human validation. | [NIST's DevSecOps reference model](https://pages.nist.gov/nccoe-devsecops/notational-reference-model.html#artificial-intelligence) calls for human monitoring and validation of AI-generated content. The [OWASP Secure Coding with AI Cheat Sheet](https://cheatsheetseries.owasp.org/cheatsheets/Secure_Coding_with_AI_Cheat_Sheet.html) documents practical context-leakage, agent-permission, test, supply-chain, and CI/CD risks. | E3, E4 | Restrict sensitive context and operational authority; review generated dependencies, commands, pipelines, and security-critical work. | Guidance and threat modeling rather than controlled evidence of AHEAD's specific controls. |
| Disposable prototypes can accelerate exploration and validation. | [Bjarnason, Lang, and Mjöberg, *An empirically based model of software prototyping*](https://doi.org/10.1007/s10664-023-10331-w) combines a systematic mapping study of 33 primary studies with empirical data from 12 companies. It identifies prototypes used to explore and validate requirements, technical feasibility, business viability, market desirability, and usability. | E1 | Permit rapid, fully AI-generated prototypes when the explicit objective is learning rather than producing production code. | The study concerns prototyping, not generative AI. AHEAD's vibe-coding exception, isolation rules, and mandatory disposal or re-entry are policy choices requiring evaluation. |

### Current conclusion

The evidence supports substantial but bounded AI assistance and supports prototyping as a learning mechanism. It does not establish that AI-generated prototypes are production-ready or that vibe coding is safe outside the prototype boundary. AHEAD's acceptable-use rules combine direct evidence, risk guidance, and constitutional choices; the rules should be reevaluated as tools and work practices change.

## Feature and change work

| AHEAD claim | Evidence | Class | What it supports | Important limitation |
|---|---|---:|---|---|
| Implementation is decision-bearing work, not mechanical translation of a plan. | [Liang et al., *A Qualitative Study on the Implementation Design Decisions of Developers*](https://arxiv.org/abs/2301.09789) used 46 survey responses and 14 interviews with professional developers. The study found that implementation decisions require ongoing attention to requirements and architecture and that developers share a general structure without following one identical process. | E1 | Keep humans engaged during implementation; allow iteration back to problem, decision, and plan rather than enforcing a one-way pipeline. | Qualitative and self-reported evidence describes practice; it does not compare AHEAD with another method. |
| Connecting requirements to implementation can improve maintenance work. | [Egyed and Mäder, *Do developers benefit from requirements traceability when evolving and maintaining a software system?*](https://doi.org/10.1007/s10664-014-9314-z) used a controlled experiment with 71 subjects performing real maintenance tasks on third-party projects; traceability users were faster and produced more correct solutions on average. | E1 | Preserve lightweight links among problem, decision, plan, code, tests, and outcome. | Participants sketched solutions rather than implementing them, and mandatory traceability can become expensive process theater. AHEAD must test a lightweight form. |
| Human review provides a meaningful quality defense. | [McIntosh et al., *An Empirical Study of the Impact of Modern Code Review Practices on Software Quality*](https://doi.org/10.1007/s10664-015-9381-9) studied Qt, VTK, and ITK and found significant relationships among review coverage, participation, reviewer expertise, and post-release quality. | E1 | Retain accountable human review and treat reviewer expertise and participation as relevant—not just the presence of an approval. | Observational relationships do not establish causality, and the study does not compare AI review with human review. |

### Current conclusion

Evidence supports traceability, iterative decision-making, and substantive human review. The complete feature sequence—especially its exact gates and first-pass-plan rule—still needs AHEAD pilot evidence.

## Debugging

| AHEAD claim | Evidence | Class | What it supports | Important limitation |
|---|---|---:|---|---|
| Professional debugging is iterative mental-model construction. | [Li and Coblenz, *A Grounded Theory of Debugging in Professional Software Engineering Practice*](https://arxiv.org/abs/2602.11435) observed 12 professionals on 17 tasks in their own codebases and modeled debugging as iterative diagnosis in which developers update mental models to guide information gathering. | E1 | Make evidence and mental-model revision the center of debugging; do not reduce the workflow to locating code. | Recent qualitative study with a small purposive sample. It describes practice rather than proving one prescribed workflow is superior. |
| Hypotheses are useful debugging objects, and tools can broaden them. | [Alaboudi and LaToza, *Using Hypotheses as a Debugging Aid*](https://doi.org/10.1109/VL/HCC50065.2020.9127273) found in a controlled experiment with 20 developers that early correct hypotheses predicted success and supplied potential hypotheses made success six times more likely; supplying fault locations did not have the same effect. | E1 | Record competing hypotheses and use AI to propose additional explanations rather than merely pointing at likely files. | Small experiment and specific tasks. AI-generated hypotheses may also be wrong or anchoring; humans must evaluate and test them. |

### Current conclusion

The core loop—evidence → mental model → hypotheses → discriminating test → updated evidence—is directly aligned with empirical descriptions of professional debugging. Human selection and interpretation remain constitutional choices. The amount of required recording must be tested so it does not interrupt reasoning.

## Operational issues and incidents

| AHEAD claim | Evidence | Class | What it supports | Important limitation |
|---|---|---:|---|---|
| Incident work contains distinct investigative and mitigative activity. | [Sillito and Kutomi, *Failures and Fixes: A Study of Software System Incident Response*](https://doi.org/10.1109/ICSME46990.2020.00027) qualitatively analyzed 30 incidents using engineer interviews and public incident reports, categorizing investigative and mitigative strategies. | E1 | Keep investigation and service restoration distinct and allow them to proceed in parallel. Capture actions with actor, purpose, result, and evidence. | Qualitative sample of 30 incidents; public postmortems have selection and reconstruction bias. |
| Production incidents are not limited to application-code bugs. | [Ghosh et al., *How to Fight Production Incidents?*](https://doi.org/10.1145/3542929.3563482) studied hundreds of high-severity incidents in a large cloud service and explicitly included software and non-code causes while separating detection, root-cause work, and mitigation. | E1 | Maintain an operational-issue process separate from bug debugging; represent infrastructure, configuration, dependency, capacity, and emergent-system causes. | One large cloud service; internal taxonomies and operating environment may not generalize directly. |
| Incident response requires preparation, detection/analysis, response, recovery, and improvement. | [NIST SP 800-61 Rev. 3](https://doi.org/10.6028/NIST.SP.800-61r3) integrates incident response across cybersecurity risk management. | E3 | Supports preparation and learning outside the active incident and the separation of response and recovery concerns. | Cybersecurity guidance, not an experiment and not a complete model for every reliability incident. |

### Current conclusion

Operational issues merit a distinct process. “Incident” is best treated as an urgency and coordination mode that adds containment, recovery, communication, and decision logging to the underlying bug, operational, security, or data work.

## Security and assurance

| AHEAD claim | Evidence | Class | What it supports | Important limitation |
|---|---|---:|---|---|
| Human and automated review are complementary. | [Yu et al., *Security Defect Detection via Code Review*](https://arxiv.org/abs/2307.02326) analyzed 20,995 candidate comments from OpenStack and Qt, identifying 614 security-related comments; the authors conclude that security practice should combine context-sensitive manual review with automated detection. | E1 | Treat AI and automation as additional assurance rather than replacements for contextual human security review. | Repository study of four open-source projects; it studied automated tools generally, not current generative-AI review. |

### Current conclusion

Security should normally be an overlay adding confidentiality, evidence handling, threat modeling, disclosure, and approval rules. The underlying work may still be corrective debugging, operational response, investigation, decision-making, or planned change.

## Process taxonomy

[ISO/IEC/IEEE 12207:2026](https://standards.ieee.org/ieee/12207/11416/) describes software life-cycle processes across conception, development, operation, maintenance, support, and retirement, and explicitly allows processes to be concurrent, iterative, recursive, and incremental. [ISO/IEC/IEEE 14764:2022](https://www.iso.org/standard/80710.html) separately details software maintenance and its types.

These standards support broad coverage and a composable lifecycle. They do not prescribe AHEAD's workflow count. The proposed six-family taxonomy is therefore an AHEAD design decision informed by standards, empirical work, and the desire to minimize workflow proliferation.

## Research gaps AHEAD should test

1. Does recording a human option before showing AI alternatives increase option diversity or decision quality?
2. Does a human first-pass plan improve later explanation, defect rate, or plan compliance compared with an AI-first plan reviewed by a human?
3. How much debugging structure helps professionals before the recording burden disrupts their mental-model work?
4. Does AI-generated hypothesis support reproduce the benefits of researcher-supplied hypotheses without causing anchoring or excess testing?
5. Does a separate operational-issue workflow reduce time to mitigation or improve causal accuracy compared with treating every production problem as a bug?
6. Does AI review find additional important issues without weakening the attention or independence of subsequent human reviewers?
7. Which gates improve outcomes, and which produce only approval or documentation theater?
