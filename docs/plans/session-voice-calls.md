# Session voice calls

Implementation plan, 2026-10-07. The source now includes an initial GPT-Live
client, self-hosted vLLM-Omni and experimental Realtime-Venus adapters, native
audio capture/playback, an icon-only call control with the “Voice call”
tooltip, and provider settings. Neither route has passed the live
provider/device acceptance matrix, and no self-hosted model is qualified. The
Realtime-Venus adapter follows a separate HTTP session contract and remains an
evaluation path. The AHEAD-shaped model benchmark remains outstanding.
Finalized caller and spoken-agent transcript
segments now enter the durable session conversation after a short pause, with
pending text flushed on hangup and session change. Raw audio is not persisted.
Each segment carries a stable UUID; repeating its RPC returns the existing row,
and reusing its ID with different content or timing is rejected. Provider-relative
start/end offsets and call identity are included in the message DTO and durably
recorded with the session event log; message loads and notifications retain
that metadata. Session-owned call start/end and transport epochs are still open.
The work list lives in
[TODO.md](../../TODO.md#session-voice-calls);
the owning workflow is [workflow atlas §6](../development/ahead-workflows.md#6-teaching-and-talking-while-working).

## Outcome and decisions

Replace the session composer's dictation control with an icon-only phone button
at the same position; its tooltip is **Voice call**. The user talks with AHEAD
while reading, typing and running existing session work.
Speech appears in that session's conversation automatically. Remove the
**Add transcript** step from this experience. Keep the text composer usable
throughout a call.

The agreed requirements are conversation quality first, support for cloud and
self-hosted GPU servers, and a speech frontend that delegates
engineering work to the existing AHEAD agent. Voice uses the same session,
task policy, permissions, code context and durable work state as text.

The voice frontend receives the active unsaved file's path, caret or selected
range, and a bounded nearby excerpt when a call starts. A debounced context
update follows editor navigation during the call, while the managed AHEAD agent
still reads the exact open buffer and verifies quotes before it highlights or
moves its pointer. Private-file contents are withheld from the voice provider.

Use OpenAI as the cloud route and default. Its source adapter, the
self-hosted vLLM-Omni adapter, and a Realtime-Venus evaluation adapter are
present; certify the cloud path first, then qualify a compatible GPU model
before recommending the self-hosted route. Both belong in the completed
feature. Mac-local model inference is out of scope by
the user's 2026-10-07 decision; the Mac still owns
microphone capture, playback and the call UI. Start with one human
calling the managed AHEAD agent. External ACP voice integration and group audio
need separate certification; existing sharing and participant mentions keep
their current behavior.

| Route | Planned choice | Decision status |
|---|---|---|
| Cloud | OpenAI `gpt-live-1`, client delegation, native WebSocket transport | The only hosted provider in scope; ship after the acceptance checks below |
| Self-hosted GPU server | Realtime-Venus-Audio 9B and Qwen3-Omni-30B-A3B-Instruct | Benchmark both against AHEAD's duplex and agent-delegation requirements before locking a default; Venus is a provisional evaluation candidate |

### Self-host adapter currently in source

The self-hosted client connects to a vLLM-Omni Realtime duplex endpoint and
offers one `delegate_to_ahead` function through the standard session `tools`
and `tool_choice` fields. The server's `extra_body.auto_response` enables the
duplex response loop.
The Nemotron VoiceChat profile streams 16 kHz float32 input in 80 ms frames and
declares its 22.05 kHz output; the custom profile streams 16 kHz PCM16 in 200 ms
frames. Provider PCM16 output is resampled to the native 24 kHz playback rate.
AHEAD pads and flushes the last partial model frame, then commits each locally
detected speech turn so an auto-response deployment can begin its reply. This
matches the Realtime duplex turn contract, but the VAD threshold, frame padding
and live model response still need a GPU-backed walkthrough.
AHEAD sends vLLM-Omni `playback.ack` only after the native player reports a
buffer as played, using cumulative per-response positions and a playback
generation fence so callbacks from interrupted audio cannot acknowledge a new
response. Unit tests cover the cumulative position and stale-callback cases;
wire behavior still needs a live duplex server walkthrough.
A model-generated function call becomes a request to the active managed AHEAD
session; AHEAD executes the request and returns its result to the voice model.
The Nemotron profile is source-tested, not qualified on a GPU deployment.
Configured custom models must actually support this protocol and function
calling.

Realtime-Venus uses a separate HTTP session/audio/output API, so it has its own
experimental adapter in `ahead-voice`, independent of the vLLM-Omni route. The
adapter checks the server's single-session capability and incarnation, uses the
matching tokenizer to decode `<delegate>…</delegate>` spans, sends those spans
to the managed AHEAD agent, and returns results through `/prefill`. It requires
HTTPS and a bearer token; because the released model server does not authenticate
requests itself, remote deployments must put it behind an authenticated TLS
proxy. This source path still needs a pinned-server walkthrough for framing,
tokenizer/model compatibility, interruption, cleanup and rejection of stale
sessions. It is an evaluation path, not a qualified or generally available
self-hosted default.
Its bounded upload and playback-ack queues fail fast instead of stalling
capture; queued microphone frames become silence if mute is enabled, preserving
the server's contiguous input sequence. A loopback contract test checks session
open, audio upload, output polling, managed-result prefill, playback
acknowledgment, bearer authentication and cleanup. That verifies AHEAD's HTTP
mapping against a mock; it does not qualify the upstream model server.

Qwen3-Omni's current vLLM-Omni duplex route does not establish model function
calls. vLLM-Omni documents tool calls for Nemotron VoiceChat, whose input profile
uses float32 audio and whose current capability table does not advertise
model-native barge-in. AHEAD implements its model-specific audio profile and
locally cancels output on detected speech, but the interaction and playback-ack
behavior still require a real server test. Therefore there is no self-hosted
model default yet: qualify a model that can both delegate to AHEAD and sustain
the required conversational overlap, or add a model-specific adapter and run
the same acceptance benchmark before recommending it.

GPT-Live supports simultaneous speech/listening and delegation to an
application-owned agent, matching this architecture. This is a fit assessment,
not a measured quality ranking. See the [model documentation](https://developers.openai.com/api/docs/models/gpt-live-1)
and [client delegation guide](https://developers.openai.com/api/docs/guides/live-delegation).
Keep the coding model independently selectable. No additional Responses backend
or Codex App Server is needed.

## Self-hosted selection and benchmark evidence

Use **Realtime-Venus-Audio 9B** as the provisional evaluation candidate because
its published results directly measure full-duplex interaction and agent tool use.
Add **Qwen3-Omni-30B-A3B-Instruct** as a first-class self-hosted challenger: its
open weights can run on a GPU server, but the published serving example does not
demonstrate native streaming-input decoding. Do not confuse it with Alibaba's
hosted `qwen-audio-3.0-realtime-plus`; that is a different product/model route.
Neither published leaderboard scores nor a serving demo establishes which one
will produce better AHEAD calls, so choose the shipped server model from the
same AHEAD-shaped benchmark and acceptance gates. Research checked 2026-10-07.

| Model | Published evidence relevant to AHEAD | Selection implication |
|---|---|---|
| Realtime-Venus-Audio 9B | FDB-v3: 85.0% tool-selection F1, 54.2% argument accuracy, 48.0% Pass@1. FDB-v1.5: 75% response to interruptions, 97% continuation through backchannels. | Preferred native speech frontend; validate correction and interruption quality carefully. |
| Qwen3-Omni-30B-A3B-Instruct | Apache-2.0 open weights; Qwen publishes a GPU-serving path, and vLLM-Omni demonstrates chunked output and interruption. Its VAD demo submits each committed utterance as a new generation request and does not use native streaming-input KV decoding; no directly comparable FDB-v3 tool-use result was found. | Serious self-hosted candidate, but verify continuous input during generated speech and benchmark tool delegation, task success, latency, and serving cost in AHEAD. |
| NemotronLabs VoiceChat 11B | FDB-v3: 82.5% F1, 42.2% argument accuracy, 33.0% Pass@1. Its paper documents unavailable user barge-in during tool execution. | Useful comparison model; the current tool-execution limitation conflicts with conversation during coding work. |

These are author-reported system results from [Realtime-Venus tables 6–7](https://arxiv.org/html/2609.13814v1)
and [VoiceChat table 4 and limitations](https://arxiv.org/html/2609.21967v1).
Qwen's [Apache-licensed model card](https://huggingface.co/Qwen/Qwen3-Omni-30B-A3B-Instruct),
[official repository](https://github.com/QwenLM/Qwen3-Omni), and
[vLLM-Omni serving example](https://github.com/vllm-project/vllm-omni/blob/main/examples/online_serving/qwen3_omni/README.md)
describe a separate open-weight route. Treat its general audio benchmarks as
capability evidence, not proof of AHEAD-style continuous duplex or delegation.
Venus also reports 68.89% routing accuracy on its internal audio delegation
benchmark. That measures the decision to delegate, not successful task execution.
None of these figures measures the proposed AHEAD integration, and they do not
establish an independently reproduced overall winner. Alibaba's
[Qwen Audio 3.0 Realtime Plus](https://www.alibabacloud.com/help/en/model-studio/qwen-audio-3-0-realtime-plus)
is a hosted Model Studio service; it is not the Qwen3-Omni checkpoint selected
for self-host evaluation.

The [Realtime-Venus model server](https://github.com/inclusionAI/Realtime-Venus/blob/main/demos/model/server.py)
exposes a custom HTTP serving API: `POST /sessions`, `DELETE
/sessions/{sid}`, and session-scoped `/audio`, `/output`, `/prefill`, and
`/playback_ack` routes under `/sessions/{sid}`. It accepts both `audio` and
`omni` model types, requires 16 kHz input and advertises one active model
session. The [released online demo requires at least one NVIDIA A100](https://github.com/inclusionAI/Realtime-Venus#3-talk-to-realtime-venus).
It is not an OpenAI-compatible Realtime WebSocket endpoint and is not
implemented by the vLLM-Omni adapter. A separate experimental AHEAD adapter now
maps this session API to microphone streaming, generated audio, transcripts,
managed-agent delegation and `/prefill` results. Opening a session returns an
incarnation and special-token IDs; AHEAD checks the tokenizer's special-token
IDs before decoding delegation spans. The released server accepts one active
model session. It has no built-in authentication, so remote deployments need
an authenticated TLS proxy; AHEAD also requires HTTPS and a configured bearer
token. These source boundaries are not live proof. The adapter remains an
evaluation candidate until a pinned GPU server verifies wire framing, tokenizer
and model compatibility, delegation, interruption, cleanup and single-session
admission behavior.

[PersonaPlex](https://research.nvidia.com/labs/adlr/personaplex/) is a useful
conversation baseline. Its published FDB-v1.0 figures include 170 ms smooth-turn
and 240 ms interruption latency, as reproduced in [VoiceChat table 1](https://arxiv.org/html/2609.21967v1).
Those timing results do not establish reliable delegation or background-result
delivery. Moshi remains a reference architecture; FireRedChat is a toolkit,
so it must be evaluated with a specified model configuration rather than ranked
as a checkpoint.

The newer [Speakrail pipeline](https://github.com/speakrail/speakrail) reports a
94.0 conversational-dynamics score using the Artificial Analysis formula. It is
a useful challenger, but its default Breeze TTS 2 weights/output are restricted
to research and non-commercial use by its documentation. Replacing TTS would
require fresh measurements; do not select its published configuration as the
AHEAD default. This is also a multi-model pipeline, not a native S2S checkpoint.

Before release, run [FDB-v1.5 and FDB-v3](https://github.com/DanielLin94144/Full-Duplex-Bench)
with pinned revisions, prompts, decoding settings and evaluation code. Compare
Venus and Qwen3-Omni with VoiceChat and a conversational baseline where the
benchmark supports them; document unsupported cases rather than treating them
as passes. For AHEAD integration comparisons, hold the coding backend and task
set fixed, and report GPU capacity, serving cost and latency alongside quality.
Keep published scores separate from reruns. Add blinded listening for naturalness
and code-name pronunciation, plus the actual slow-task/correction cases below.

Select on successful requests and corrections, natural overlap handling, then
latency and serving cost. Require the call acceptance gates before considering
the tradeoffs; do not average an inability to interrupt during work into a
passing score. Record per-scenario failures and uncertainty rather than a single
unexplained aggregate. Revisit the recommendation if controlled results disagree.

## Call experience

These are proposed interaction defaults for implementation, beyond the agreed
replacement of dictation. Use existing GPUI/gpui-kit controls and theme tokens.
The compact call strip belongs immediately above the existing composer; keep
the editor visible and preserve keyboard focus and the human caret.

| State | What the user sees and can do |
|---|---|
| Idle | Start voice call in the former microphone position; requires an active managed session |
| Setup needed | Select/configure the voice connection in existing Settings; return to the same session |
| Connecting | Inline connecting status and Cancel; no audio sent before the route is ready |
| Connected | Call duration, listening/speaking activity, microphone mute and End call; both speakers can be active together |
| Microphone muted | Clear muted state; incoming speech and coding work continue; local mic samples are discarded |
| Reconnecting | Explicit connection status; stop forwarding and discard unsent mic audio; End call remains available |
| Failed | Inline reason and Retry or Settings action; text conversation stays usable |
| Ending | Stop capture/playback immediately, finish provider cleanup asynchronously, then return to Idle |

Use Lucide icons with accessible labels and tooltips. Expose named call actions
through the existing command/keybinding system; follow established shortcuts
where applicable rather than assigning keys during this plan. Mic mute and
speaker mute are independent; speaker/device controls can live in the call
menu. The existing task stop action keeps its current meaning.

- Barge-in cuts obsolete playback locally while input and coding work continue.
  For GPT-Live, also append a trusted `session.instructions.append` event asking
  it to stop the current spoken response and listen. Keep playback gated until
  an assistant transcript begins after the caller's last transcript timestamp;
  the primary WebSocket's audio chunks have no IDs or timing fields, so verify
  late-audio handling and transcript/audio ordering in real conversations.
  Short acknowledgments should not repeatedly cut the assistant off, so
  evaluate this distinction with real conversations.
- Ending the call stops voice transport and audio resources. Already submitted
  coding work continues and posts results into the same conversation. Cancel
  work is a separate task-specific action. Clarify an ambiguous spoken "stop."
- Keep the current behavior where editor/composer typing and debugger controls
  interrupt speech; they leave the call connected. Test this during sustained
  typing so the assistant does not repeatedly restart an interrupted explanation.
- Proposed default: switching sessions/workspaces, closing the session or app,
  and system sleep end the call. No background transfer or automatic restart.
  Permit one call per app instance. A brief network reconnect remains bound to
  its originating session and never resubmits work.
- Reuse one conversation timeline. Update human and assistant caption bubbles
  independently during overlap; show coding progress through existing task
  entries. Keep source/timing metadata so a spoken summary does not create a
  duplicate coding turn. Typed messages sent during a call reach the normal
  agent path once and update the voice context without a second dispatch.
- Allow transcript corrections. A correction affects queued or subsequent work;
  it cannot retroactively change a request that already ran. Distinguish speech
  generated from audio actually played, including interrupted output.

Show the selected voice destination before starting; a cloud call must never
retain the current "transcripts stay on this Mac" label. Use existing BYOK
settings and credential ownership. Do not silently change providers after an
error. Retain submitted text under session retention rules, without raw audio
recording by default. Partial captions stay private to the caller and configured
provider until committed to the conversation; group audio is outside this slice.

## Implementation boundaries

| Area | Files to extend | Responsibility |
|---|---|---|
| Audio and voice transport | `ahead-voice/src/lib.rs`, `ahead-voice/src/macos_voice.m`; add a provider module here when needed | Capture/playback, echo control, resampling, bounded audio queues and provider event translation; no engineering tools |
| Call state | `ahead-viewmodel/src/voice.rs` | Call lifecycle, mute state and allowed transitions; input activity and output playback remain independent |
| Session messages | `ahead-rpc/src/ahead.rs` | Explicit start/end/status and stream events, identities, transcript timing and playback receipts |
| Session host | `ahead-proxy/src/ahead/{host,voice}.rs` | Own the active call, route configuration and session checks; connect voice requests to the existing agent controller |
| Agent work | `ahead-agent/src/session.rs`, `ahead-agent/src/native_client.rs` | Existing turn serialization, context, policy, cancellation and durable results; add narrow steering support only if the runtime supports it |
| Native UI | `ahead-app/src/{session_panel,proxy_client,app,settings_panel}.rs` | Thin controls, captions, settings and presentation/audio routing |

Instantiate device capture/playback on the Mac and provider networking in the
trusted local session host. Reuse the existing transport and WebSocket stack;
audio is transient traffic, never an ordinary persisted/logged RPC payload.
If linking `ahead-voice` into the host, separate device initialization from
provider transport so the host does not open a second microphone.

Extend existing voice DTOs rather than creating another parallel bus. Track
session ID, call ID, connection epoch and sequence numbers; use separate input
identity and output generation. An output interruption must not invalidate the
new user speech that caused it. Playback receipts also need the connection
epoch. Reject stale callbacks, close/connect races and events after hangup.

The present `VoiceSession` queue and task list in `ahead-proxy` are probes, and
`VoiceIntent` is a small local state object. Neither establishes working call
transport or real task cancellation. The host now issues an owner-checked lease
for each call, with a host-generated call ID and per-session connection epoch;
only one lease may be active for a session, and stale controls cannot affect a
later call. The app acquires the lease before provider setup and releases it on
hangup, session switch or setup failure. This binds identity and lifecycle to
the managed session while audio still flows directly between the native app and
the selected voice server. Connect the remaining status and stream events to
this boundary, and do not treat the lease as a live transport or provider
qualification result.
Preserve the retained native harness's tools, instruction layering and effect
boundary; this feature does not require changes under `ahead-agent-engine/`.

## GPT-Live adapter contract

Follow the **Live** API, not the Realtime API's differently named events.
The initial session configuration should contain:

```json
{
  "model": "gpt-live-1",
  "delegation": { "type": "client" },
  "audio": {
    "format": { "type": "audio/pcm", "rate": 24000 },
    "output": { "voice": "marin" }
  },
  "store": false
}
```

Use the authenticated primary WebSocket at
`wss://api.openai.com/v1/live/sessions`; send `session.start` and await
`session.started`. Stream mono PCM16LE through `session.input_audio.append`
and play `session.output_audio.delta`. Resample outside the audio callback and
pace capture continuously. Target 20 ms capture frames with bounded buffering.
Implement native echo cancellation; muting input during output is not duplex.
See the [connection and audio format contract](https://developers.openai.com/api/docs/guides/voice-websockets?api=live).

Mute and unmute are provider-confirmed state changes. Stop local microphone
samples immediately when muting, send an event with an `event_id`, and update
the call UI only after the matching `session.input_audio.muted` acknowledgment.
Keep local capture stopped while unmuting until the matching
`session.input_audio.unmuted` acknowledgment arrives. A rejected command ends
the call through the existing error path so the app does not resume sending
audio under an uncertain provider state.

GPT-Live has two integration constraints to resolve in the first spike:

1. `session.delegation.created` supplies `delegation.id`, target and `offset_ms`,
   but no task text. Input/output transcript deltas carry text and time ranges,
   with no completed-turn event or transcript item ID. Maintain the timeline
   locally and assemble the intended request from speech plus current session
   context. Caption grouping alone cannot authorize an effect.
2. Primary WebSocket audio deltas have no timestamps or audio-done event.
   Count played samples locally. Do not label delayed old audio with a new
   output generation after interruption, or claim exact phrase/cue alignment
   from packet arrival time. Prove a recovery boundary using output activity,
   transcript timing and provider behavior before accepting this transport.

The [session guide](https://developers.openai.com/api/docs/guides/live-conversations)
documents these timing constraints. If the primary connection cannot meet the
interruption/presentation checks, evaluate the provider's WebRTC media and
sideband route before making the default available. Local output suppression
must work even when a provider instruction is delayed; see
[playback controls](https://developers.openai.com/api/docs/guides/voice-server-controls?api=live).

On hangup, stop local audio immediately, send `session.close`, await
`session.closed` with a bounded timeout and release the connection. Record
reported usage; mark it incomplete if finalization fails. Muting keeps the
provider session open. Show duration and explain ongoing voice charges in call
settings; backend usage is separate. End calls on lifecycle exits rather than
leaving paid sessions open invisibly.

## Delegation and presentation

Persist the relationship between a call's delegation, accepted utterance,
existing session task and managed turn. Use it to deduplicate work across
repeated events, reconnect and retries. There is one durable work record;
voice completion and backend completion are separate events.

1. Seed voice with a bounded selection of this session's conversation, current
   task and verified state. Refresh selected code/context when it changes;
   never carry another session's context into this call.
2. When delegation arrives, collect the relevant transcript interval and recent
   corrections. Wait for sufficient context instead of dispatching a fragment.
   A transcript-delivery gap is not proof the user finished. For an unclear
   reference or effectful request, ask a natural clarification; an unambiguous
   spoken instruction needs no extra approval card or Send click.
3. Dispatch the accepted request through the existing managed turn path with
   current editor context and policy. Keep user speech, voice-model paraphrases
   and verified tool results distinguishable in context. A speech-model claim
   cannot grant permissions or establish that work succeeded.
4. The controller currently rejects a second active turn. Initially queue new
   work visibly with edit/remove controls and recheck scope when it starts.
   Answer progress questions from actual state without another coding turn.
   Use native steering only when supported and tested. Explicit cancellation
   calls the real controller and reports its acknowledgment.
5. Return concise verified progress through `session.thinking.append` and
   speakable results through `session.commentary.append`, preserving the
   original `delegation_id`. Each append is limited to 500 tokens. Reserve
   `session.instructions.append` for trusted application directions; code,
   transcripts and tool output remain data. Keep full results in AHEAD.
6. Correlate presentation with the existing `present_code`/pointer/render-ack
   path. Speak a location-specific phrase only after its cue renders. An append
   acknowledgment is not a playback receipt. Route `speak_text` through the
   active call's audio owner so `/usr/bin/say` and model speech cannot overlap.
   Preserve ordinary presentation outside calls and external-agent tool schemas.

The [client delegation guide](https://developers.openai.com/api/docs/guides/live-delegation)
defines the result/context events. [Qwen Audio Agent's architecture](https://github.com/QwenAudio/qwen-audio-agent/blob/main/docs/architecture/deep-dive.md)
is a useful reference for responsive dialogue while backend work runs; reuse
AHEAD's controller rather than importing its executor.

## Self-hosted server integration

The source adapter targets vLLM-Omni's Realtime duplex WebSocket contract and
has explicit Nemotron VoiceChat and custom PCM16 profiles. It does not make the
candidate routes below ready by configuration alone. vLLM-Omni documents tool
calls as a Nemotron VoiceChat integration that is still awaiting its follow-up
port into the duplex plugin; the current capability table does not describe a
tool-support flag. Its table also does not advertise model-native barge-in for
that model. Validate the exact checkpoint/runtime combination before enabling
it as a recommended route.

Evaluate the open-weight [Qwen3-Omni-30B-A3B-Instruct checkpoint](https://huggingface.co/Qwen/Qwen3-Omni-30B-A3B-Instruct)
with the [vLLM-Omni serving path](https://github.com/vllm-project/vllm-omni/blob/main/examples/online_serving/qwen3_omni/README.md).
Pin both the checkpoint and runtime. This is distinct from Alibaba's hosted
`qwen-audio-3.0-realtime-plus`. The vLLM example streams generated audio and
supports interruptions in VAD mode, but commits each user utterance as a new
generation request and does not use native streaming-input KV decoding. Prove
that it passes AHEAD's continuous-input/full-duplex gate before treating it as
a release-ready route; don't infer compatibility from an OpenAI-style API.

Pin the `Realtime-Venus-Audio` checkpoint within
[`inclusionAI/Realtime-Venus`](https://huggingface.co/inclusionAI/Realtime-Venus)
and the matching serving-code revision. Deploy the upstream audio inference
service on Linux/CUDA, with `model.type: "audio"`. The
[upstream demo](https://github.com/inclusionAI/Realtime-Venus) specifies at least
one A100; establish actual VRAM, cold start and per-call capacity before
advertising a smaller server or concurrency level.

Adapt its model service to AHEAD's call events. Route delegation requests back
to AHEAD and return verified backend results to the originating voice session.
The model server hosts inference; AHEAD retains project files, tools, task
execution and credentials. Do not adopt the demo's Codex executor.

The [current model adapter](https://github.com/inclusionAI/Realtime-Venus/blob/main/demos/model/adapter.py)
supports the audio checkpoint despite its Omni class name, uses 16 kHz input,
and limits each instance to one live session. Preserve its session/incarnation
and event sequencing; reject capacity overflow visibly. The adapter needs a
measured isolation/scaling design before multiple callers share an instance.

Add a self-hosted connection in existing Settings with endpoint and private
authentication configuration. Use authenticated TLS or an explicitly configured
private tunnel; probe readiness and capabilities before opening the microphone.
Publish a pinned deployment recipe with model/dependency licenses, hardware
measurements, health checks, connection limits and clean session teardown. No
generic OpenAI-compatible claim substitutes for the actual streaming protocol.

## Implementation sequence and exit checks

1. **Prove audio and the provider contract.** Build a small path through the
   existing voice crate with a real microphone, speakers/headphones and the
   proposed adapter. Exercise overlap, echo, interruption, transcript timing,
   delegation and close/usage events. No feature announcement until this works.
   Certify OpenAI `gpt-live-1` as the cloud route; record voice quality and
   latency separately from coding-task performance.
2. **Connect session-owned work.** Add call lifecycle/identities, transcript
   storage and delegation dispatch. Test duplicate requests, late corrections,
   busy turns, real cancel and completion after hangup with controlled events.
3. **Replace the composer experience.** Add settings and call controls, then
   remove the dictation-only Add transcript flow. GPT-Live requires microphone
   permission but must not depend on Apple's speech-recognition permission.
   Remove dictation-only recognizer dependencies and permissions after checking
   remaining callers. Remove stale labels and adapt existing voice tests.
4. **Verify the native journey.** Run the matrix below and document actual
   devices, model/configuration, timing and failures. Update
   `docs/guide/voice-and-sharing.md` with rendered screenshots only when the
   replacement is implemented. Keep the current guide truthful until then.
5. **Ship the self-hosted server route.** Benchmark Realtime-Venus-Audio against
   Qwen3-Omni on the same AHEAD task set and acceptance matrix, then deploy the
   qualified candidate and publish its pinned server recipe. The feature is
   complete when OpenAI cloud and one self-hosted GPU route pass. Extract only
   the small shared interface needed by the second adapter; a provider plugin
   framework is unnecessary.

| Acceptance case | Required evidence |
|---|---|
| Normal call | Start from an existing session, discuss current code, see both speakers in its timeline, and return to text after hangup |
| Full duplex | User speech is processed during model speech; correction is understood; speakers do not cause a feedback loop |
| Concurrent work | Start a real slow task, ask about its progress, continue typing, queue a follow-up, then hang up while work completes |
| Interrupt vs cancel | Local obsolete playback stops within the existing 150 ms target; backend continues until explicitly cancelled; no delayed old speech returns |
| Transcript integrity | Partial words, overlapping speech, delayed deltas and corrections cause no duplicate or accidental work; typed messages dispatch once |
| Presentation | The referenced range actually renders before its dependent phrase; stale/failed cues are handled and the caret stays put |
| Lifecycle | Mute, denied permission, device change, network loss, retry, sleep and session switching release audio and reject stale events |
| Isolation and privacy | No cross-session audio/context, keys in transcripts/logs, raw audio persistence, or silent cloud fallback; mute discards mic samples locally |
| Responsiveness and cost | Measure typing/frame impact and actual response latency; preserve the existing p95 16 ms typing target; closed calls stop accumulating provider usage |

Use focused state/adapter/controller tests for deterministic failures and GPUI
tests for control behavior. Follow the shared Cargo build-loop rules when
implementing. Tests and source inspection do not replace the rendered native
walkthrough or measured voice quality. Keep unfinished checks in TODO.md.
