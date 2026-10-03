# Voice and sharing

## Dictate into a conversation

Use the microphone control under the Agent composer to start local voice input. Speak, then choose **Add transcript** to put recognized words in the composer. Edit the text before pressing Send. The microphone control stops capture; while capture is active, the mute control pauses or resumes listening. AHEAD displays listening, muted, and speaking states in the conversation area.

Voice input requires the operating system's microphone and speech permissions. The recognition path is intended to stay on this Mac. **Partial:** the complete permission, transcription, correction, speech interruption, and session-switch journey still needs rendered verification. If the microphone does not start, read the status message and check the Mac's permissions.

## Share a checkpoint

The Threads rail can export a session checkpoint under the workspace's `.ahead/sessions/<session-id>/` and discover an exported checkpoint for import. The bundle includes a readable session summary and conversation. Review the exported files before sharing them; they may contain prompts or attached context.

**Partial:** checkpoint artifacts, revision-specific code references, and teammate handoff are still being built. Import does not automatically restore an agent's original process or change the current editor file.

## Collaborate live

**Partial:** a signed-in session owner can share an active managed AHEAD session over a direct TLS connection. Add allowed GitHub accounts and default roles to `.ahead/team.toml`, invite selected people from the session, and copy the displayed invite to them. Each guest signs in to GitHub and joins with that invite. The host must be reachable over a LAN or VPN; AHEAD does not yet relay through NAT.

Conversation, session code comments, active participant file/line presence, and the latest host terminal scrollback synchronize. A message containing `@` followed by a current participant's GitHub name goes only to the humans; an unaddressed message may start the host's agent under the sender's role, using the host's provider account. Guests can open a host file; owners and editors can edit it, while reviewers and viewers get a read-only tab. Edits sync to the host after a short pause. If another participant changed the file first, the editor keeps your local text and offers Use latest or Keep mine. The host stores each accepted guest edit for crash recovery until the file is saved or explicitly discarded after sharing stops. Dirty guest tabs also keep recovery snapshots in the guest's private local workspace store. Rejoining that session opens recovered drafts and asks you to choose which version to keep before sending anything to the host. Recovery snapshots are periodic, so the most recent keystrokes may still be lost if the app stops before a snapshot or host edit is acknowledged. Comment links open the shared host file and find a moved quote when it is unique. The terminal mirror remains read-only. Select a participant in the presence row to follow their file and line; editing stops follow, and Return restores the previous file and line. The complete two-app interface has not yet had rendered verification.
