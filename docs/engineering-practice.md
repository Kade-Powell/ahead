# AHEAD Engineering Practice

Status: proposed guidance

## Purpose

AHEAD is not only a sequence of AI gates. It is a way of practicing engineering. This guide distills general habits from the submitted starting list into a smaller set of principles that can be remembered, applied, and evaluated.

Many of these ideas come from practitioner literature rather than controlled experiments. The source type matters: a useful craft principle can guide work without being misrepresented as science.

The submitted notes include page-level references to *The Pragmatic Programmer*. Those locators are preserved in the [edition-specific page index](references/pragmatic-programmer-page-index.md) and grouped below so the distillation remains traceable to its source.

## 1. Care about the craft and own the result

Engineering quality begins with attention, pride, and accountability. Treat code, documentation, tests, operations, and communication as parts of one professional result. Do not submit work merely because a tool produced it or a check passed.

Basis: *The Pragmatic Programmer* tips 1 (p. xlx as submitted) and 70 (p. 258), Toyota's human-centered improvement philosophy, and AHEAD's constitution.

## 2. Think deliberately

Stay aware of what you are doing and why. Critique assumptions, inspect evidence, and resist autopilot—whether the automation is an IDE wizard, framework convention, copied snippet, or AI assistant.

When a tool proposes an answer, ask what would disprove it and what information it could not see.

Basis: *The Pragmatic Programmer* tips 2 (p. xlx as submitted), 9 (p. 16), 27 (p. 97), and 44 (p. 175), plus direct debugging research on mental models and hypothesis testing.

## 3. Start from users, domain, and real constraints

Requirements are discovered and refined, not merely received. Work with users and domain experts, use their language, identify the desired outcome, and separate real constraints from inherited habits.

Quality is contextual. Reliability, latency, accessibility, security, cost, maintainability, and delivery time do not have one universal ordering; accountable humans decide what the work requires.

Basis: requirements and traceability research, ISO/IEC/IEEE life-cycle standards, and *The Pragmatic Programmer* tips 7 (p. 11), 17 (p. 58), 51 (p. 202), 52 (p. 204), 54 (p. 210), and 55 (p. 213).

## 4. Make reasoning visible

Record important assumptions, options, decisions, tradeoffs, evidence, uncertainty, and changes in understanding. Link intent to implementation and verification without creating documents that nobody uses.

Traceability should help future engineers understand why and where—not become compliance theater.

Basis: controlled evidence that requirements-to-code traceability can improve maintenance-task performance, AHEAD's evidence standard, and *The Pragmatic Programmer* tips 10 (p. 21), 18 (p. 64), 19 (p. 69), 20 (p. 74), and 23 (p. 88).

## 5. Prefer simple, local reasoning

Choose designs that minimize the number of concepts a person must hold simultaneously. Keep unrelated concerns independently changeable. Make state explicit and contained. Keep policy distinct from mechanism. Prefer stable values, plain data, clear interfaces, and declarative rules when they fit the problem.

Modularity is not automatically simplicity: separate modules can remain tightly coupled through hidden assumptions, timing, shared state, or required call order.

Basis: Rich Hickey's *Simple Made Easy* and *The Pragmatic Programmer* tips 11 (p. 27), 13 (p. 35), 36 (p. 140), 41 (p. 156), and 42 (p. 161). These are design heuristics, not universal experimental laws.

## 6. Design for change without speculative machinery

Decisions can be revised, so record their rationale, reversibility, and review triggers. Avoid building generalized infrastructure for hypothetical futures. Make the current change coherent and create seams where evidence shows variation is likely.

Use a “rule of three” only as a prompt for judgment, not a mechanical law. Duplication of knowledge is more dangerous than superficially similar code; premature abstraction can couple cases that should evolve separately.

Basis: *The Pragmatic Programmer* tips 4 (p. 5), 12 (p. 33), 14 (p. 46), 47 (p. 186), and 53 (p. 209). The precise abstraction threshold is context-dependent.

## 7. Prototype to learn

Use prototypes, spikes, tracer paths, and small vertical experiments to test uncertain architecture, integrations, data, performance, and user interaction. State the learning question and disposal plan first.

This is a legitimate place for full “vibe coding”: AI may rapidly generate the whole experiment when speed of learning matters more than understanding or maintainability of the artifact. Keep it isolated, label it `PROTOTYPE — NOT FOR PRODUCTION`, exclude production credentials and sensitive data, and do not treat a convincing demo as evidence of correctness or readiness.

Prototype code does not silently become production code. Preserve the learning, then discard the implementation. If any code is retained, it leaves the prototype exception and enters the normal human-understanding, design, implementation, test, security, and review process.

Basis: empirical software-prototyping research, *The Pragmatic Programmer* tips 15 (p. 49) and 16 (p. 54), its prototyping checklist (p. 53), and AHEAD's Investigation process hypothesis. The evidence supports prototyping as a learning practice; the permission for full AI generation inside strict boundaries is AHEAD policy.

## 8. Automate repeatable mechanics

Use source control, shells, scripts, formatters, generators, CI, and other automation to make repeatable operations consistent and inspectable. Automation should remove drudgery while leaving intent, effects, and failures visible.

Do not automate a process you cannot evaluate. Judge tools by the long-lived artifacts and operational behavior they produce, not only authoring convenience or initial speed.

Basis: *The Pragmatic Programmer* tips 21 (p. 80), 22 (p. 85), 28 (p. 100), 29 (p. 103), and 61 (p. 231), NIST secure-development guidance, and AHEAD's Toyota analogy.

## 9. Design for testing and failure

Think about verification before implementation. Define observable behavior, invariants, boundaries, significant states, failure modes, resource exhaustion, recovery, and performance expectations.

Coverage is evidence about execution, not proof of correctness. Test the tests through mutation, fault injection, or known negative cases where proportionate. When a defect is fixed, preserve a regression check when one can reliably express the failure.

Basis: software-testing research and *The Pragmatic Programmer* tips 30–35 (pp. 107–129), 48–50 (pp. 192–199), and 62–66 (pp. 237–247). Evidence for specific methods such as strict test-driven development is mixed and context-dependent; AHEAD does not mandate one universal test-writing order.

## 10. Debug with evidence, not confidence or blame

Do not panic, guess from the loudest log, or assume the platform is broken. Establish the observation, characterize it, build a mental model, generate hypotheses, predict discriminating results, test safely, and update the model.

Treat application code, dependencies, infrastructure, configuration, data, operator actions, and external systems as candidates whose likelihood changes with evidence. Focus on restoring and improving the system rather than protecting or assigning personal blame.

Basis: direct empirical debugging and incident-response studies, plus *The Pragmatic Programmer* tips 24–27 (pp. 91–97) and debugging checklist (p. 98).

## 11. Review independently and communicate honestly

Review is a reasoning activity, not an approval button. Review the current artifact against intended behavior, architecture, risks, tests, operations, and maintainability. Automated and AI review add coverage; they do not replace accountable human judgment.

Technical communication should be clear, audience-aware, and grounded in the author's understanding. State observed fact, interpretation, uncertainty, decision, and request distinctly. Do not polish weak understanding into false confidence.

Basis: empirical code-review research, NIST generative-AI risk guidance, and *The Pragmatic Programmer* tips 3 (p. 3), 10 (p. 21), and 67–70 (pp. 248–258).

## 12. Learn continuously and measure the tools

Build breadth by learning new languages, paradigms, ecosystems, tools, and operational models. Reimplementing a small system in contrasting languages can reveal how type systems, concurrency models, package managers, and idioms change design choices.

Do not infer learning or productivity from ease, enjoyment, or generated volume. Measure relevant outcomes. AI studies currently show both productivity gains and losses in different settings, and emerging evidence shows that full delegation can reduce skill formation.

Basis: randomized AI/developer studies, *The Pragmatic Programmer* tips 8 (p. 14), 9 (p. 16), 58 (p. 220), and 59 (p. 222), and the Toyota principle that human skill and automation should improve together.

## Compact working checklist

Before implementation:

- What user or operational outcome are we changing?
- Which constraints are real, and which are assumptions?
- What is the simplest model of the problem?
- Which concerns can vary independently?
- What must remain invariant?
- How will we know the change works and fails safely?
- Are we prototyping to learn or building production code?
- If this is a disposable prototype, what is its learning question, isolation boundary, and disposal date?

Before accepting AI-assisted work:

- Can the responsible engineer explain and change it?
- Did the AI define behavior that a human should own?
- Are claims, sources, dependencies, and commands verified?
- Did AI-generated tests inherit the implementation's assumptions?
- Was sensitive context authorized for the selected tool?
- Is there independent human review proportionate to risk?

Before merge or delivery:

- Does the change trace to the approved problem and decision?
- Are tests meaningful, current, and capable of failing?
- Are failure, rollback, observation, and recovery understood?
- Did review examine system behavior rather than only style?
- Is the documentation close enough to the system to remain accurate?
- What accepted uncertainty or follow-up remains?

## Recommended reading and viewing

These are recommended practitioner sources, not scientific proof of AHEAD:

- David Thomas and Andrew Hunt, [*The Pragmatic Programmer: Your Journey to Mastery*, 20th Anniversary Edition](https://pragprog.com/titles/tpp20/the-pragmatic-programmer-20th-anniversary-edition/), ISBN 9780135957059.
  - AHEAD preserves the submitted print-page locators in its [Pragmatic Programmer page index](references/pragmatic-programmer-page-index.md).
- Luca Palmieri, [*Zero To Production In Rust*](https://www.zero2prod.com/), ISBN 9798847211437. This is a concrete production-backend learning path rather than a general philosophy source.
- Richard Hamming, [*The Art of Doing Science and Engineering: Learning to Learn*](https://press.stripe.com/the-art-of-doing-science-and-engineering), ISBN 9781732265172.
- Rich Hickey, [*Simple Made Easy*](https://www.youtube.com/watch?v=SxdOUGdseq4), Strange Loop 2011.

## Research sources

- [Liang et al., *A Qualitative Study on the Implementation Design Decisions of Developers*](https://arxiv.org/abs/2301.09789)
- [Egyed and Mäder, *Do developers benefit from requirements traceability when evolving and maintaining a software system?*](https://doi.org/10.1007/s10664-014-9314-z)
- [Li and Coblenz, *A Grounded Theory of Debugging in Professional Software Engineering Practice*](https://arxiv.org/abs/2602.11435)
- [Alaboudi and LaToza, *Using Hypotheses as a Debugging Aid*](https://doi.org/10.1109/VL/HCC50065.2020.9127273)
- [Sillito and Kutomi, *Failures and Fixes*](https://doi.org/10.1109/ICSME46990.2020.00027)
- [McIntosh et al., *The Impact of Modern Code Review Practices on Software Quality*](https://doi.org/10.1007/s10664-015-9381-9)
- [Shen and Tamkin, *How AI Impacts Skill Formation*](https://arxiv.org/abs/2601.20245)
- [Cui et al., *The Effects of Generative AI on High-Skilled Work*](https://doi.org/10.1287/mnsc.2025.00535)
- [Becker et al., *Measuring the Impact of Early-2025 AI on Experienced Open-Source Developer Productivity*](https://arxiv.org/abs/2507.09089)
- [Bjarnason, Lang, and Mjöberg, *An empirically based model of software prototyping*](https://doi.org/10.1007/s10664-023-10331-w)

Detailed applicability and limitations belong in the [AHEAD research map](evidence/research-map.md).
