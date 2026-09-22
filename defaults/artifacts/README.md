# AHEAD artifact templates

These are the built-in prompts for the living Markdown records AHEAD produces
while engineering work happens. The editor creates `session.md` first and creates
the other documents only when they have meaningful content.

Projects may override one template at a time with a tracked file of the same name
under `.ahead/templates/`. Keep overrides concise and non-secret. Template fields
use `{{name}}`; unknown fields remain visible instead of being silently removed.

The templates guide capture; they are not completion gates. Delete unused
sections, link evidence in its native system, and distinguish human decisions,
retrieved facts, inferences and AI contributions.
