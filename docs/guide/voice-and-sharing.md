# Voice and sharing

## Voice calls

In the current source, the former dictation control has been replaced by an
icon-only phone button with the tooltip “Voice call.” It starts or ends a call
in the active, managed AHEAD session. The source includes an OpenAI GPT-Live
route selected by default, a self-hosted vLLM-Omni Realtime duplex adapter, and
an experimental Realtime-Venus HTTP session adapter, along with native Mac
microphone/speaker capture and playback. Calls request microphone access only;
AHEAD does not request Apple's speech-recognition permission or run a local
speech model. When switching to self-hosted, Settings selects the visibly marked
Realtime-Venus evaluation profile by default; this does not mean that model has
passed AHEAD qualification. Configure the provider in the workspace's private
voice settings. Self-hosted profiles need a GPU server URL and model ID; remote
servers must use HTTPS and a bearer token. The Realtime-Venus server must sit
behind an authenticated TLS proxy because its upstream service does not
authenticate requests. It allows one active call per server, and AHEAD
downloads the matching public checkpoint tokenizer at call startup. Settings
also include a Nemotron VoiceChat profile and a custom 16 kHz PCM16 profile.
The Nemotron profile matches the published audio framing. Current
[vLLM-Omni documentation](https://github.com/vllm-project/vllm-omni/blob/main/docs/serving/realtime_duplex_api.md)
says its Nemotron tool-call integration is still a follow-up, so stock
deployments do not yet establish managed agent delegation; the selected model
server must implement function calls before AHEAD can route engineering
requests through the same managed session. For teaching and questions about a
specific code section, the managed agent is instructed to verify and highlight
the exact range, then move a separate pointer as it explains the code while
preserving the human caret. During a call, the selected voice provider receives
the active file, caret or selection, and a short nearby excerpt; AHEAD refreshes
that context as the editor focus changes. Private-file contents are withheld.
Finalized caller and AHEAD transcript segments are saved as human/agent messages
in the same durable session conversation after a short pause; hangup and session
switch also flush pending text. Transcript entries keep the session's normal
message ordering and timestamps. AHEAD does not save raw microphone or speaker
audio.
OpenAI's reported cumulative voice duration appears during the call and after a
confirmed close. OpenAI voice usage, self-hosted GPU serving, and the managed
agent's model usage have separate costs.

**In progress:** this route has not completed a live provider and device
walkthrough, so the guide does not claim it is ready for everyday use. No
self-hosted model is benchmark-qualified yet. The Realtime-Venus adapter is
experimental and has not passed a GPU-backed AHEAD workflow test. Qwen3-Omni's
current vLLM path does not establish managed tool delegation. Mac-local speech
inference is out of scope. Until the call path is verified, use the text
composer for session work.

## Share a checkpoint

The Threads rail can export a session checkpoint under the workspace's `.ahead/sessions/<session-id>/` and discover an exported checkpoint for import. The bundle includes a readable session summary and conversation. Review the exported files before sharing them; they may contain prompts or attached context.

**Partial:** checkpoint artifacts, revision-specific code references, and teammate handoff are still being built. Import does not automatically restore an agent's original process or change the current editor file.

## Collaborate live

**Partial:** a signed-in session owner can share an active managed AHEAD session over a direct TLS connection. Add allowed GitHub accounts and default roles to `.ahead/team.toml`, invite selected people from the session, and copy the JSON invite from the host's Share session control. Each guest signs in to GitHub, then pastes that JSON into **Shared session invite (JSON)** under **Join an existing session**. This is an invite payload, not a web link. The host must be reachable over a LAN or VPN; AHEAD does not yet relay through NAT.

Conversation, session code comments, active participant file/line presence, and the latest host terminal scrollback synchronize. A message containing `@` followed by a current participant's GitHub name goes only to the humans; an unaddressed message may start the host's agent under the sender's role, using the host's provider account. Guests can open a host file; owners and editors can edit it, while reviewers and viewers get a read-only tab. Edits sync to the host after a short pause. If another participant changed the file first, the editor keeps your local text and offers Use latest or Keep mine. The host stores each accepted guest edit for crash recovery until the file is saved or explicitly discarded after sharing stops. Dirty guest tabs also keep recovery snapshots in the guest's private local workspace store. Rejoining that session opens recovered drafts and asks you to choose which version to keep before sending anything to the host. Recovery snapshots are periodic, so the most recent keystrokes may still be lost if the app stops before a snapshot or host edit is acknowledged. Comment links open the shared host file and find a moved quote when it is unique. The terminal mirror remains read-only. Select a participant in the presence row to follow their file and line; editing stops follow, and Return restores the previous file and line. The complete two-app interface has not yet had rendered verification.
