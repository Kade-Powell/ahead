# Submitted Engineering Notes

Status: submitted source notes

## Purpose and provenance

This document preserves submitted engineering tips, checklists, and practices that are intentionally condensed in the [AHEAD Engineering Practice](../engineering-practice.md) guide. It is a source record, not a claim that every item is an experimentally validated rule.

Tips 1–70 and their submitted page locators from *The Pragmatic Programmer* are retained in the [edition-specific page index](pragmatic-programmer-page-index.md). Their accompanying submitted meanings are retained below, followed by the additional tips and the full submitted checklist and practice content. The submitted language-learning list is intentionally omitted.

## *The Pragmatic Programmer* tip notes

The page index is authoritative for the submitted locators. These concise notes preserve the submitted meaning associated with each title.

| No. | Submitted meaning |
|---:|---|
| 1 | Care about doing software development well. |
| 2 | Turn off autopilot; continually critique and appraise the work. |
| 3 | Offer viable options instead of excuses or an unsupported claim that something cannot be done. |
| 4 | Correct bad designs, wrong decisions, and poor code when they are found. |
| 5 | Catalyze change by showing people a possible future and helping them participate in creating it. |
| 6 | Keep checking the larger context rather than becoming trapped in implementation detail. |
| 7 | Involve users in deciding the quality the project actually requires. |
| 8 | Make continuous learning a regular investment. |
| 9 | Critically evaluate vendor claims, media, received wisdom, and dogma in the context of the actual project. |
| 10 | Effective communication depends on both the idea and how it is expressed. |
| 11 | Give each piece of knowledge one unambiguous, authoritative representation, while avoiding premature abstraction; use the rule of three as a judgment prompt rather than dogma. |
| 12 | Create an environment in which reuse is easy. |
| 13 | Design self-contained, independent components with a single well-defined purpose. |
| 14 | Treat decisions as revisable and plan for change. |
| 15 | Use tracer implementations to test direction and progressively find the target. |
| 16 | Treat prototypes as learning artifacts whose value lies in lessons rather than retained code. |
| 17 | Design and code using the language of the problem domain and its users. |
| 18 | Estimate before starting so potential problems become visible early. |
| 19 | Refine schedules with the experience gained during implementation. |
| 20 | Preserve knowledge in plain text so it remains portable, inspectable, testable, and debuggable. |
| 21 | Use command shells when graphical interfaces do not provide enough leverage. |
| 22 | Learn one configurable, extensible, programmable editor deeply enough that it becomes an extension of the hand. |
| 23 | Always use source control so work and decisions can be recovered and revisited. |
| 24 | Focus on fixing the problem rather than assigning blame. |
| 25 | Do not panic while debugging; stop and think about plausible causes. |
| 26 | Bugs are more often in the application than in foundational tools, but likelihood is not proof. |
| 27 | Test assumptions in the real environment with realistic data and boundary conditions. |
| 28 | Learn a text-manipulation language so repetitive text work can be automated. |
| 29 | Use code generation to improve consistency and reduce duplicated mechanics. |
| 30 | Perfect software is impossible; protect code and users from inevitable errors. |
| 31 | Use contracts to state and verify what code promises and requires. |
| 32 | Fail early when continuing would cause more damage or hide the real fault. |
| 33 | Use assertions to validate assumptions and prevent impossible states from proceeding unnoticed. |
| 34 | Reserve exceptions for exceptional conditions so control flow remains readable and maintainable. |
| 35 | Where possible, make the code that acquires a resource responsible for releasing it. |
| 36 | Minimize coupling and follow the principle of least knowledge. |
| 37 | Express technology choices as configuration rather than baking them into unrelated implementation. |
| 38 | Put general abstractions in code and variable details in metadata. |
| 39 | Analyze user workflows to find genuine opportunities for concurrency. |
| 40 | Design around independent, concurrent services behind consistent interfaces. |
| 41 | Designing for concurrency can produce cleaner interfaces with fewer timing assumptions. |
| 42 | Separate models from views where useful; this principle is broader than the MVC pattern. |
| 43 | Use blackboards to coordinate facts and agents while preserving their independence and isolation. |
| 44 | Do not rely on accidental behavior or confuse coincidence with deliberate design. |
| 45 | Estimate algorithmic order before implementation to understand likely scaling behavior. |
| 46 | Mathematical estimates are incomplete; measure behavior in the target environment. |
| 47 | Refactor early and repeatedly when the system needs it, addressing the underlying problem rather than its surface arrangement. |
| 48 | Think about testing before implementation so the design is observable and testable; this does not mandate or reject test-driven development. |
| 49 | Test aggressively rather than relying on users to discover defects. |
| 50 | Do not incorporate generated or wizard-produced code that the engineer does not understand. |
| 51 | Discover requirements beneath assumptions, misconceptions, and organizational pressures rather than merely collecting surface statements. |
| 52 | Work with users to understand how the system will actually be used. |
| 53 | Invest in stable abstractions rather than transient implementation details. |
| 54 | Maintain one authoritative glossary for project-specific vocabulary. |
| 55 | When a problem appears impossible, identify the real constraints and question whether the assumed method or task is necessary. |
| 56 | Respect accumulated experience and unresolved doubts when deciding whether the work is ready to begin. |
| 57 | Avoid an endless specification spiral; some understanding emerges only by doing the work. |
| 58 | Evaluate formal methods in the context of the team's practices and capabilities rather than following them blindly. |
| 59 | Expensive tools and vendor prestige do not guarantee better designs; judge tools on their merits. |
| 60 | Organize teams around delivered functionality instead of isolating design, coding, testing, and data work. |
| 61 | Replace repeatable manual procedures with inspectable automation that executes consistently. |
| 62 | Run automated tests early and frequently, preferably with every build. |
| 63 | Coding is not complete until all required tests run. |
| 64 | Deliberately introduce defects in an isolated copy to verify that tests can detect them. |
| 65 | Test significant program states, not only executed lines. |
| 66 | Preserve a reliable automated regression check after a human discovers a defect. |
| 67 | Treat documentation like code by applying authoritative sources, metadata, separation of model and view, and generation where useful. |
| 68 | Keep documentation close to the system so it is less likely to become incorrect or stale. |
| 69 | Understand user expectations and then exceed them gently rather than surprising users with unrequested complexity. |
| 70 | Take accountable pride in the work and be willing to sign it. |

## Additional tips

### 71. Create simple solutions, not easy ones

Take time to find the simplest solution. The easy solution may use a familiar tool or pattern, but familiarity can conceal complexity and deprive the engineer of learning.

Source: Rich Hickey, [*Simple Made Easy*](https://www.youtube.com/watch?v=SxdOUGdseq4), Strange Loop 2011.

### 72. KISS — Keep It Simple, Stupid

Simplicity is the ultimate sophistication. Simpler solutions are generally easier to understand, maintain, and extend.

Source: Rich Hickey, [*Simple Made Easy*](https://www.youtube.com/watch?v=SxdOUGdseq4), Strange Loop 2011.

### 73. The Locality of Behavior principle

The behavior of a unit of code should be as obvious as possible from that unit. Separation of concerns may favor splitting behavior into separate pieces, while locality of behavior may favor keeping the tasks for a feature or component together. Some designs can satisfy both; others require an explicit tradeoff.

No external source locator was included with this submitted note.

### 74. Judge tools by the artifacts they produce

Do not choose a language, library, framework, or pattern only because it is pleasant or familiar to use. Judge it by the long-lived software it helps create: reliability, changeability, debuggability, and the incidental complexity it leaves behind.

Source: Rich Hickey, [*Simple Made Easy*](https://www.youtube.com/watch?v=SxdOUGdseq4), Strange Loop 2011.

### 75. Avoid complecting independent concerns

Complexity comes from braiding together things that could be reasoned about independently. Watch for value tied to time, function tied to state, policy tied to mechanism, meaning tied to order, and implementation details leaking across boundaries.

Source: Rich Hickey, [*Simple Made Easy*](https://www.youtube.com/watch?v=SxdOUGdseq4), Strange Loop 2011.

### 76. Isolate state

State ties value to time. Keep mutable state explicit, minimal, and behind interfaces that let the rest of the system work with stable values when possible.

Source: Rich Hickey, [*Simple Made Easy*](https://www.youtube.com/watch?v=SxdOUGdseq4), Strange Loop 2011.

### 77. Prefer values, data, and functions

Prefer immutable values, plain data, functions, queues, set operations, declarative queries, and explicit rules where they fit. These can create simpler artifacts than objects, mutable variables, inheritance, loops, object-relational mapping, and scattered conditionals.

Source: Rich Hickey, [*Simple Made Easy*](https://www.youtube.com/watch?v=SxdOUGdseq4), Strange Loop 2011.

### 78. Modular does not automatically mean simple

Separate files, classes, modules, or services can still be deeply entangled. The real test is whether each part needs to know only stable abstractions rather than the hidden details or assumptions of other parts.

Source: Rich Hickey, [*Simple Made Easy*](https://www.youtube.com/watch?v=SxdOUGdseq4), Strange Loop 2011.

### 79. Tests are guardrails, not steering

Tests and type checkers help catch mistakes, but they do not replace design clarity or reasoning. A production defect already passed the checks that existed, so the program must remain simple enough to understand.

Source: Rich Hickey, [*Simple Made Easy*](https://www.youtube.com/watch?v=SxdOUGdseq4), Strange Loop 2011.

## Submitted checklists

### The Wisdom Acrostic

- **W**hat do you want them to learn?
- What is their **I**nterest in what you have to say?
- How **S**ophisticated are they?
- How much **D**etail do they want?
- Whom do you want to **O**wn the information?
- How can you **M**otivate them to listen?

Submitted locator: *The Pragmatic Programmer*, p. 20.

### How to maintain orthogonality

- Design independent, well-defined components.
- Keep code decoupled.
- Avoid global data.
- Refactor similar functions.

Submitted locator: *The Pragmatic Programmer*, p. 34.

### Things to prototype

- Architecture.
- New functionality in an existing system.
- The structure or contents of external data.
- Third-party tools or components.
- Performance issues.
- User-interface design.

The submitted note emphasizes that prototype code is intended to be thrown away and may be fully vibe coded under the AHEAD disposable-prototype policy.

Submitted locator: *The Pragmatic Programmer*, p. 53.

### Architectural questions

- Are responsibilities well defined?
- Are collaborations well defined?
- Is coupling minimized?
- Can potential duplication be identified?
- Are interface definitions and constraints acceptable?
- Can modules access the data they need when they need it?

Submitted locator: *The Pragmatic Programmer*, p. 55.

### Simplicity and complecting review

- Is this design simple, or only easy because it is familiar?
- What concepts are tied together that could vary independently?
- What incidental complexity are we accepting from the selected tools, libraries, frameworks, or patterns?
- Can this part be understood without loading the rest of the system into your head?
- Is state explicit, minimized, and isolated?
- Are policies and rules gathered somewhere clear, or scattered through unrelated code paths?
- Are we choosing this approach for authoring convenience or for the quality of the artifact it creates?
- Do tests support our reasoning, or are we depending on them to compensate for unclear design?

Source: Rich Hickey, [*Simple Made Easy*](https://www.youtube.com/watch?v=SxdOUGdseq4), Strange Loop 2011.

### Abstraction questions

- What is the operation or capability?
- Who owns the data or entity?
- How is the implementation separated from the interface?
- When does this happen, and is timing unnecessarily coupled to the caller?
- Where does this run, and is location unnecessarily coupled to the caller?
- Why does this rule or policy exist, and can it be represented declaratively?

Source: Rich Hickey, [*Simple Made Easy*](https://www.youtube.com/watch?v=SxdOUGdseq4), Strange Loop 2011.

### Debugging checklist

- Is the reported problem a direct result of the underlying defect, or merely a symptom?
- Is the defect really in the compiler, operating system, or another platform component—or is it in the application? Treat likelihood as a starting point and prove the conclusion.
- If you explained the problem in detail to a coworker, what would you say? Rubber-duck debugging can help make the model explicit.
- If the suspect code passes its unit tests, are the tests complete enough? What happens with different data?
- Do the conditions that caused this defect exist elsewhere in the system?

Submitted locator: *The Pragmatic Programmer*, p. 98. The submitted note also references [Rubber Duck Debugging](https://rubberduckdebugging.com/).

### Law of Demeter for functions

An object's method should call only methods belonging to:

- itself;
- parameters passed to it;
- objects it creates;
- its component objects.

Submitted locator: *The Pragmatic Programmer*, p. 141.

### How to program deliberately

- Stay aware of what you are doing.
- Do not code blindfolded.
- Proceed from a plan.
- Rely only on reliable things.
- Document assumptions.
- Test assumptions as well as code.
- Prioritize effort.
- Do not be a slave to history.

Submitted locator: *The Pragmatic Programmer*, p. 172.

### When to refactor

- A violation of the DRY principle is discovered.
- Things could be made more orthogonal.
- Knowledge improves.
- Requirements evolve.
- Performance needs improvement.

Submitted locator: *The Pragmatic Programmer*, p. 185.

### Cutting the Gordian knot

When a problem appears impossible, ask:

- Is there an easier way?
- Am I solving the right problem?
- Why is this a problem?
- What makes it hard?
- Must it be done this way?
- Must it be done at all?

Submitted locator: *The Pragmatic Programmer*, p. 212.

### Aspects of testing

- Unit testing.
- Integration testing.
- Validation and verification.
- Resource exhaustion, errors, and recovery.
- Performance testing.
- Usability testing.
- Testing the tests themselves.

No page locator was included with this submitted checklist.

## Submitted practices

### Simplicity pass before implementation

- Before building a long-lived feature, separate concerns that do not need to be tied together.
- Prefer more small, straight, independent pieces over fewer pieces tied into a knot.
- Name the parts representing problem complexity and those representing incidental complexity.

### Complexity ledger

- For meaningful design decisions, record the benefit, byproducts, what the choice ties together, and why the tradeoff is acceptable.
- Revisit the ledger when the system becomes difficult to change or debug.

### Quarantine state

- Make mutable state rare, named, easy to find, and surrounded by a simpler interface.
- Do not pass mutable references through the system when a stable value will do.

### Disentangle during refactoring

- Trace what is tied together, identify the separate concerns, and separate one concern at a time.
- Refactoring should reduce what a reader must hold in mind rather than merely moving code into different files.

Source for these submitted practices: Rich Hickey, [*Simple Made Easy*](https://www.youtube.com/watch?v=SxdOUGdseq4), Strange Loop 2011.
