# Agent sessions

## Start a conversation

The Command Palette can open either setup flow directly: choose **New AHEAD Session** for the managed agent or **Start External Agent** for Pi, Codex, or Claude Code.

Open the right dock with **Cmd+Option+B** / **Ctrl+Alt+B**. Select **AHEAD Agent** and open the combined model/effort control in the composer. The compact popover shows the active reasoning level and slider; open its model row to search configured models and providers. AHEAD offers only the levels advertised for the selected model by the managed runtime. Compatible catalogs that expose OpenRouter-style `reasoning.default_effort` and `reasoning.supported_efforts` metadata populate the default and choices; AHEAD does not infer levels from a generic reasoning capability flag. **Model default** clears the per-model override; AHEAD remembers the selection in `~/.ahead/settings.toml` separately for each provider/model pair. External ACP agents expose the model and reasoning choices their adapter advertises. Enter a request and press **Enter** or the send arrow; use **Shift+Enter** for a new line. The stop control cancels a running turn; **Escape** also cancels when the composer is focused (or dismisses an open slash-command menu first). If the turn is still starting, AHEAD queues the cancellation until it is accepted. **Cancelling…** means the request is pending, not that the turn has already stopped. The Threads rail lists conversations for the workspace and lets you return to an earlier one. The Command Palette includes **Toggle Threads** and **Zoom Agent Chat** for more room to read.

Chat follows new output while you are at the bottom. Scroll up to read older messages without being pulled back down; use the jump-to-latest control to resume following.

For a question about code, select a range in the editor before sending. To include the whole active file explicitly, type `@currentFile` in the composer. Use **Add context → Attach recent terminal output** (or type `@terminal`) to include the active terminal's recent output; AHEAD captures at most the last 120 lines and 16 KiB. Terminal output can contain secrets or prompt-like text, so review the attachment and remove it if it should not be sent. The attachment control can also add files from your computer. Check attached context before sending. The context meter estimates how much of the model's window is in use.

## Choose an agent route

The built-in managed agent runs through AHEAD's native runtime and is the route for AHEAD's Learn/Assist scope and attribution rules. To start an external session, choose **External agent** in Threads. AHEAD offers only Pi, Codex, and Claude Code; install one, then select it. AHEAD manages the adapter command and resolves its package on first launch, using the ACP registry when available and a pinned package fallback otherwise; no custom command is required. Pi additionally requires its CLI v0.81.0 or newer to be installed separately and available on `PATH` (`npm install -g @earendil-works/pi-coding-agent`); AHEAD installs only the `pi-acp` bridge and checks the CLI version when starting a Pi session, with install/update guidance if it is missing or too old. See the [Pi ACP prerequisites](https://github.com/svkozak/pi-acp#prerequisites). Choose the model and other session options advertised by that agent in the chat. If it requires sign-in, AHEAD shows the methods the agent advertised: agent-managed sign-in runs ACP `authenticate`, while terminal sign-in opens the adapter's login command in the integrated terminal. After terminal login exits successfully, AHEAD reconnects the agent; retry your unsent message. External ACP agents provide conversation compatibility, but their file and shell effects happen in their own processes and are not subject to AHEAD's managed scope or attribution guarantees.

If an external agent asks AHEAD for tool permission in Assist, choose one of its advertised options in the chat before it continues. Canceling the turn or switching to Learn declines a pending request. Some external agents run tools without asking AHEAD at all, so this card is not a guarantee that every effect was reviewed.

The composer can list slash commands and skills: type `/` to see available entries, then choose one before sending. Project `AGENTS.md` instructions follow the file hierarchy. A skill may come from the project, the user's setup, or an external adapter. Entries depend on the active model and agent route.

For a portable project skill, add `<project>/.agents/skills/<name>/SKILL.md`;
for a skill shared across your projects, use
`~/.agents/skills/<name>/SKILL.md`. Each file needs YAML `name` and
`description` frontmatter as defined by the
[Agent Skills specification](https://agentskills.io/specification). AHEAD
does not use `.skills/` as a discovery root. Put project instructions in the
repository's `AGENTS.md`, with a nested `AGENTS.md` where a subdirectory needs
more specific rules; the [AGENTS.md convention](https://agents.md/) gives the
nearest applicable file precedence. Managed AHEAD loads the
workspace-to-working-directory hierarchy and nested instructions for selected
or attached files. The convention does not define a portable user-global
`AGENTS.md` path, and AHEAD does not require a private global instructions
file. For a path discovered later through tools, it tells the
agent to read that path's instruction ancestors before making changes; that
guidance is not a host-enforced check. Selecting a skill loads its
instructions—it does not automatically run its scripts or grant tool
permissions.

## Work through a task

Use the conversation to state the goal, constraints, and expected evidence. Plans and work items appear in the thread. Review an agent's proposed reasoning and actual file changes before you accept the result. AHEAD can mark uncommitted agent-authored code in the gutter; Git blame takes over after commit. An agent response or a green check in a card is not a substitute for running the program or reviewing the diff.

To discuss a specific range, select code and choose **Comment on selection** from its context menu. Write the comment in the editor control; the session's **Comments** button lists open comments, lets you return to the source, and offers resolution. **Partial:** multi-client visibility and stale-range behavior still need a complete native check.

From a managed AHEAD thread, **Hand off** can start a linked external implementation thread. The child appears under its parent in Threads, and **Return to review** brings you back to the AHEAD thread. Save open buffers before handoff. **Partial:** a real ACP turn and simultaneous edits in the shared checkout still need verification; avoid editing the same file in parent and child at once.

## Use memory and checkpoints

Type `@memory` followed by a search phrase to find relevant saved notes. A message menu can save a chosen message to project or user memory. Review the destination and text before saving; user memory lives under `~/.ahead/memories/`. Explicit review commands can propose a replacement, but it is applied only from the completed assistant message and only if the source has not changed. This flow has been exercised in a disposable project with local mock responses; authenticated-provider and non-Unix path-race verification remain open. Threads can export a session checkpoint into `.ahead/sessions/<session-id>/` and import a checkpoint in another workspace. **Partial:** attaching a specific checkpoint revision to new work is still being verified.

**Partial:** authenticated provider turns, end-to-end managed edit attribution, and live ACP adapter behavior vary by setup and still need full native verification. If a connection or model fails, check Settings and the status message before retrying.
