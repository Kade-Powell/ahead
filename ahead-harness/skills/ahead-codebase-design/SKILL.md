---
name: ahead-codebase-design
description: Provide AHEAD's vocabulary and reasoning aids for the shape of a module, interface, adapter, or seam during design and architecture work. Use as a reference layer; do not invoke it as an autonomous refactoring process.
---

# AHEAD codebase design

Read [../references/human-led-policy.md](../references/human-led-policy.md)
before proceeding.

This is a reference skill, not a driver workflow. Loading it must not start a
refactor, spawn design agents, create a module, or change the user's plan.

Use precise terms:

- **Module:** a unit whose public interface hides meaningful implementation
  decisions.
- **Interface:** the smallest useful surface through which callers depend on
  the module.
- **Seam:** a boundary where behavior can be observed or varied without
  reaching through the interface.
- **Depth:** how much useful behavior an interface hides relative to its size.
- **Adapter:** translation at a real external or varying boundary.

When discussing a proposed extraction, ask:

- Does the deletion test show that the module earns its existence?
- Is the interface deep enough to hide useful policy or complexity?
- Are invariants, ordering, errors and version boundaries explicit?
- Is there a real second implementation or variation that justifies an adapter?
- Can callers and tests cross the same seam?
- Does this fit AHEAD's session host, viewmodel, GPUI and policy boundaries?

The human chooses the design. Record the selected interface, rejected options,
invariants and open questions in the current design artifact; this skill does
not decide whether the change should be implemented.
