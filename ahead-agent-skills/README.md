# AHEAD agent skills

This root workspace crate owns AHEAD's skill discovery, loading, prompt context,
and provenance. `builtins/` contains the skill packages shipped with the editor.
The retained engine's `codex-skills` package embeds that directory at compile
time and installs it into AHEAD's system-skill cache.

Project and user skills use their own `.agents/skills/` roots. See the
[agent skills design](../docs/development/ahead-agent-skills.md) for discovery
and policy details.
