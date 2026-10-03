# Agent sessions

## Start a conversation

Open the right dock with **Cmd+Option+B** / **Ctrl+Alt+B**. Select **AHEAD Agent** and choose a configured model in the composer. Enter a request and press **Enter** or the send arrow; use **Shift+Enter** for a new line. The stop control cancels a running turn. The Threads rail lists conversations for the workspace and lets you return to an earlier one. The Command Palette includes **Toggle Threads** and **Zoom Agent Chat** for more room to read.

For a question about code, select a range in the editor before sending. To include the whole active file explicitly, type `@currentFile` in the composer. The attachment control can add files from your computer. Check the attached context before sending. The context meter estimates how much of the model's window is in use.

## Choose an agent route

The built-in managed agent runs through AHEAD's native runtime and is the route for AHEAD's Learn/Assist scope and attribution rules. External ACP agents provide conversation compatibility; their file and shell effects happen in their own processes. Choose an external agent only after configuring its command and provider. AHEAD does not promise managed enforcement for external ACP processes.

If an external agent asks AHEAD for tool permission in Assist, choose one of its advertised options in the chat before it continues. Canceling the turn or switching to Learn declines a pending request. Some external agents run tools without asking AHEAD at all, so this card is not a guarantee that every effect was reviewed.

The composer can list slash commands and skills: type `/` to see available entries, then choose one before sending. Project `AGENTS.md` instructions follow the file hierarchy. A skill may come from the project, the user's setup, or an external adapter. Entries depend on the active model and agent route.

## Work through a task

Use the conversation to state the goal, constraints, and expected evidence. Plans and work items appear in the thread. Review an agent's proposed reasoning and actual file changes before you accept the result. AHEAD can mark uncommitted agent-authored code in the gutter; Git blame takes over after commit. An agent response or a green check in a card is not a substitute for running the program or reviewing the diff.

To discuss a specific range, select code and choose **Comment on selection** from its context menu. Write the comment in the editor control; the session's **Comments** button lists open comments, lets you return to the source, and offers resolution. **Partial:** multi-client visibility and stale-range behavior still need a complete native check.

From a managed AHEAD thread, **Hand off** can start a linked external implementation thread. The child appears under its parent in Threads, and **Return to review** brings you back to the AHEAD thread. Save open buffers before handoff. **Partial:** a real ACP turn and simultaneous edits in the shared checkout still need verification; avoid editing the same file in parent and child at once.

## Use memory and checkpoints

Type `@memory` followed by a search phrase to find relevant saved notes. A message menu can save a chosen message to project or user memory. Review the destination and text before saving; user memory lives under `~/.ahead/memories/`. Threads can export a session checkpoint into `.ahead/sessions/<session-id>/` and import a checkpoint in another workspace. **Partial:** attaching a specific checkpoint revision to new work and the full reviewed memory replacement journey are still being verified.

**Partial:** authenticated provider turns, end-to-end managed edit attribution, and live ACP adapter behavior vary by setup and still need full native verification. If a connection or model fails, check Settings and the status message before retrying.
