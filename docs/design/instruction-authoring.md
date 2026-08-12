# Instruction Authoring

Status: initial authoring standard v0.1

AHEAD instructions are an executable interface to the framework. They should make the next valid behavior clear without duplicating the full Constitution, rationale, or workflow documentation into every model turn.

## Progressive disclosure

Use three layers:

1. The active profile supplies the binding human/AI authority boundary and live workflow state.
2. The generator adds the workflow phase fragment and only the method overlays mapped to that phase in `policy/methods/index.json`.
3. Humans and AI retrieve full framework Markdown on demand through the host integration.

The canonical workflow spec owns phases, artifacts, gates, transitions, and capabilities. Phase policies own local AI behavior. Method overlays own reusable practices such as evidence handling or debugging. Full documents explain why. Do not restate one rule in every layer.

## Authoring rules

- State the desired observable behavior in direct, positive language.
- Put human ownership and prohibited transfers of authority where a model cannot mistake them for suggestions.
- Give checkable completion criteria: an exact artifact, observation, identifier, or gate condition.
- Point to the canonical source instead of copying large passages.
- Keep host-specific UI out of core workflow semantics.
- Do not cache repository facts that tools can discover cheaply and reliably.
- Use examples to clarify a schema, not to narrow judgment to the example.
- Treat generated instructions as build artifacts and test the generated output for required boundaries.

The build fails when a method overlay names an unknown workflow or phase. Adding a method therefore requires its source fragment, an explicit mapping, generated-output coverage, and documentation when it changes user-visible expectations.
