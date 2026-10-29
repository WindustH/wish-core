# Extensions

`extensions` owns capabilities outside conversational LLM requests. A search query, a webpage read,
a reference-image edit and a durable video job have different contracts. They do not inherit a
universal model request or a common generation state machine.

- Supplier packages such as `openai`, `seedance`, `novelai`, `runway`, `tavily`, and `jina`
  own tool registrations, schemas and supplier request/response code. Search/fetch endpoint
  declarations live in each supplier's `sources.rs`.
- `shared` provides bounded read retries, artifact transfers, multipart encoding and durable
  record mechanics. It cannot import a supplier package. RunningHub uploads live in `runninghub/upload.rs`.
- `extension_host` contains host-side dispatch and the remaining image/media orchestration adapters.
  It is not an extension and is not imported by extensions. Some capability/options and polling
  decisions still live here and must be moved into supplier modules to complete the execution split.

Authentication, credential renewal and endpoint signing remain shared in `protocol/endpoint`.
Transport performs individual HTTP attempts; CPU/IO work uses the existing bounded worker utilities.
No extension imports application, server, session, executor or agent tool modules. Protocol must not
import extensions. Model/account clients also cannot import extensions. `tools/check_architecture.py` checks these dependency directions.


## Binary entry point

```sh
wish extension search image generation
wish extension inspect novelai_generate
wish extension call novelai_generate --json '{"prompt":"A watercolor landscape"}'
printf '%s' '{"action":"list_jobs"}' | wish extension call runway_video --stdin
```

All tags are matched (AND), case-insensitively. With no tags, search lists all available tools.
Inspection returns that tool's schema, examples, service IDs and capabilities in configured order.
`service` selects an exact authorized service; omitted selection uses the extension's configured priority.
Discovery is read-only and does not probe remote providers. Calls require the current session's shell
bridge credentials and the corresponding enabled capability. Job queries remain scoped to the tool's presets.
Unknown paid outcomes must be recovered through status/list_jobs rather than retried.

`wish websearch`, `webfetch`, `imagegen`, `videogen`, `audiogen`, `asr`, and direct extension-name
CLI commands have been removed. The model's direct discovery tool remains `extension_tools`;
its discovered tool names use their independent schemas. Core commands (`history`, `task`,
`askuser`, `session`, `skill`, `mcp`) keep their own CLI entries.

## Application boundary and migration

OpenAI speech and ElevenLabs speech/music/sound now retain separate typed `run`, `status`,
and `list_jobs` requests through catalog dispatch. Speech takes `text`; music and sound take
`prompt`. Recovery operations reject generation fields. Tags only support discovery; the host
does not infer speech/music/sound execution from them. Their application adapters still use
the durable media engine, which remains part of the execution-boundary migration below.
MiniMax speech now carries its native text, voice and advanced controls together in a supplier
execution plan. The plan resolves voice defaults, mixed-voice overrides and subtitle expectations;
the generic media parameter resolver no longer accepts MiniMax-specific blend flags or advanced
speech parameters. Its durable download/recovery integration is still hosted by the media engine.
xAI video's `generate_audio` belongs to its own video options and wire builder. Preflight and
resolved-asset submission use the same builder; the host no longer patches serialized requests
or stores a separate xAI audio flag in the generic media request.
xAI's video operation module also owns mode selection, image/keyframe binding and preset-voice
references. Audio and video application adapters construct internal execution requests directly;
the old media execution request no longer implements `Deserialize` and cannot serve as another
external tool schema. The remaining internal media envelope and durable engine are still pending
migration; removing its JSON entry does not make the runtime split complete.

OpenAI, SiliconFlow and ElevenLabs transcription also use separate typed invocations and
service scopes. The old `tool::audio_transcribe` facade and unscoped transcription entry have
been removed. The application still owns the bounded blob read and transcription execution
adapter; configuration lookup remains tied to the media hub.
The transcription adapter now retains the original supplier request in a closed routing enum;
there is no common transcription options bag or JSON re-encoding at the host boundary. Each
supplier validates and builds its multipart request and owns authentication and transcript
projection. Runtime transcription no longer dispatches through `MediaProtocol`. The service
scope is derived from the request's registered extension, with no empty-scope bypass. The host
owns transport, configuration lookup, bounded input loading and blob publication. Multipart
construction and response parsing run on the bounded CPU executor. Invalid paid responses are
reported as unknown outcomes; they never trigger another submission or supplier failover.

Supplier video contracts now own capability declarations, accepted input combinations, task IDs,
poll requests and terminal-state decoding. Seedance, MiniMax, Runway and Luma also own the
resolved-input wire encoding; the host delegates to these functions. Shared input validation
checks source identity and basic shape against capabilities supplied by the extension.
Seedance, MiniMax, Luma, RunningHub and Xiaoyunque invocations retain their supplier request
types through catalog dispatch. They no longer pass through the generic configured-argument
adapter. The obsolete `tool::media_generate` facade has been removed. Unscoped media generation
is rejected; the user-facing HTTP recovery entry accepts only an existing job identifier.
The application still adapts these requests to its durable media engine, so the execution
and configuration migration is not complete.
Runway owns its ephemeral-upload negotiation, inline-size threshold and signed multipart transfer;
RunningHub owns its shared image/video upload workflow. The host supplies bounded transports and
session-owned bytes, without constructing supplier upload endpoints or form fields. Signed
Runway storage uploads never receive the API token, and multipart bodies are built on the bounded
CPU executor. Input asset URI schemes are declared by supplier modules. The host consults the
extension's blob-input capabilities when deciding whether a recorded public URL is required.

Remaining execution-boundary work:

The four webpage readers now retain their individual request types through dispatch. The old
`tool::web_fetch` facade is removed; `application::pages` provides session-owned immutable page
snapshots and bounded pagination without accepting tool JSON or publishing a tool schema.
Continuation resolves the saved page's service instead of silently choosing the first service,
and validates that the service belongs to the invoked reader. Snapshots persist extension ownership
as well as service identity, so reconfiguring a service ID cannot transfer old cursors to another
extension. Continuation instructions use
`wish extension call TOOL`. Provider execution/configuration still use the old fetch hub.

- Remove the remaining generic configured invocation adaptation and category-specific execution hubs.
  The union `MediaOptions` has been removed: service configuration stores opaque JSON parameters,
  suppliers strictly deserialize their own types, and operation-specific default resolution is
  owned by each supplier. Shared parameter infrastructure only clones/overlays JSON and decodes
  the explicitly requested type. Configuration validation delegates to supplier implementations.
- Move remaining image capability/input routing and preparation out of the old host adapter
  where they describe a supplier rather than host storage/credential ownership. Image submission
  and result decoding now delegate to supplier modules; supplier endpoint modules declare their
  authentication/header/signing context, reusing protocol credential mechanics. BFL owns its
  regional polling-origin permission rule.
- Replace category-hub execution/configuration wiring with extension registrations and explicit
  host services; retain only shared transport, credential, blob, bounded-work and job primitives.
- Migrate external regression callers that still invoke removed category commands, then run the
  full image/media/search/fetch scenarios alongside the new extension CLI checks.


Application currently composes configured providers, selects account credentials and enforces session
ownership of blob inputs. Existing image/media hubs still own service configuration, input loading and result persistence;
media orchestration also remains in application. Image submission/polling now belongs to its extension;
these are migration work, not evidence that the capability boundary is complete. Move their feature
logic behind extension interfaces with explicit credential/transport/artifact ports, rather than
moving application account registries or HTTP errors into extensions. Tool adapters translate their
results to model-facing messages. Existing configuration storage has not yet been replaced; old tool/CLI aliases are removed.

Feature-specific settings, input preparation, polling and output interpretation belong to that
extension. Shared infrastructure may own mechanics such as bounded transfers, per-job locks and
atomic records, but must not select providers or invent common request parameters for all features.

The existing media hub now uses `extensions/shared/jobs` for per-path asynchronous locks and
atomic JSON records. This helper is generic over the extension's own record type: no shared
job phase, request schema or recovery policy is imposed. Records are bounded to 2 MiB on both
read and write; encoding/decoding uses the CPU pool, and abandoned waiters do not retain locks.
Existing journal locations and serialization stay unchanged. Generated media downloads use
`shared/artifacts::open_stream` with bounded redirects and no account headers; callers still
validate media types and enforce their artifact size limits before publishing blobs.


## Extension-owned tools and presentation

An extension is a complete functional unit; supplier boundaries are one useful packaging choice,
not a mandatory taxonomy. Each extension owns its tools' names, descriptions, JSON schemas,
examples, validation, execution and result semantics. The host must not require all suppliers to
accept `image_generate` or a common media request. Unified category tools are no longer advertised or callable. Permissions remain category settings; individual tools are registered by their extensions.

The intended discovery flow is two stages: search tags (for example `image`, `generation`, `editing`)
to receive compact extension/tool summaries, then inspect a selected tool's detailed instructions
and schema. Tool invocation uses that tool's own schema. The host owns namespacing, permission
checks, cancellation, lifecycle and dispatch. Discovery must only expose authorized/enabled tools;
execution must check authorization again. A catalog entry or previous inspection is not authority
to invoke a disabled extension. The first path is implemented for NovelAI: `extension_tools` searches tags or inspects
`novelai_generate`; the latter owns its schema and accepts a configured NovelAI service, prompt,
optional source image, precise V4.5 character/style references and NovelAI parameters.
References have independent strength/fidelity controls and can be combined with the source image. BFL also registers independent `bfl_generate` and `bfl_fill` schemas; discovery
filters services by both preset and configured model. Services are listed in configuration priority
order. The independent tools accept an optional service: omitted selection chooses the first
compatible authorized service and pins it before external work, with no paid-submission failover.
Discovery is a derived host capability, enabled when any registered extension permission is enabled;
it is advertised once and follows tool/command delivery of the enabled extensions. It is not tied
to image generation. Search and inspect filter each entry by its own permission; execution checks
that permission again. Image, video, audio generation, transcription, search and fetch permissions are implemented.

MiniMax registers `minimax_speech` with independent text/voice and run/status/list_jobs variants.
Its native `parameters` schema owns language boost, normalization, pronunciation rules, LaTeX,
audio encoding, voice effects, mixed voices and sentence/word subtitles. Validation and draft enrichment live in
the MiniMax extension. The application adapter carries a typed, non-deserializable parameter slot
during migration; legacy media JSON and other extensions do not inherit these fields.
Ordered service preflight checks these parameters before any paid request. Subtitle JSON downloads
are bounded to 2 MiB and saved as a separate timestamp blob; audio is journaled first, so subtitle
retry after restart does not redownload audio or resubmit synthesis. MiniMax now owns the complete speech JSON builder, voice validation and completion-envelope
decoder in `minimax/speech.rs`; the host delegates to this code. Mixed voices replace only
the configured single voice and preserve other default controls. Explicit single-voice IDs conflict
with blends; weights are relative (1..100 each, up to four distinct voices), not percentages that
must add to 100. Raw audio/streaming, voice cloning and voice management remain unimplemented.

The host's durable media infrastructure accepts a preset scope: selection, listing and recovery
all enforce it, including against a job's saved service snapshot after restart. Recovery remains
available when the original service is removed from the current discovery list, and never
resubmits a paid job. MiniMax's native tool still delegates to this application infrastructure;
moving the complete execution boundary into extensions remains migration work. Backend discovery, permission mapping, schema lookup and CLI routing use the extension-owned
registrations in `extensions/catalog.rs`; registrations retain independently typed requests. Additional
extensions and fully extension-owned execution remain migration work. The old image engine is reused behind the application adapter; no universal image
request is exposed by the NovelAI tool. NovelAI owns generation JSON encoding in
`novelai/wire.rs`, its per-call parameter allowlist, precise-reference kinds/model support, and
bounded reference-canvas preparation. The legacy image service delegates those decisions to the
extension; its compatibility reference types are re-exports, not separate definitions. Shared
image input/reply types, output decoding and application orchestration remain migration work.
The native tool also exposes sampler/noise-schedule identifiers, guidance rescaling and
Decrisper. These call overrides preserve explicit zero/false without changing service defaults;
the same type/range checks apply to configured defaults and invocation values. The host limits
serialized option bytes, not a hardcoded parameter count; the extension allowlist determines
which fields are legal. Sampler and
schedule strings are bounded locally; model-specific compatibility is left to NovelAI, whose
public API does not publish an exhaustive enum. This does not add mask/inpainting, Vibe Transfer
or Director Tools support. The native V4.5 tool now exposes structured
`character_prompts` (up to six) with per-character negative text and optional normalized centers.
`novelai/characters.rs` owns validation and construction of matching positive/negative captions;
the convenience field is consumed locally, never forwarded as an unknown upstream parameter.
Automatic positioning omits coordinate guidance; explicit positioning requires centers for every
character to avoid silently repositioning an unspecified subject. Empty character arrays clear
service defaults for one call. The frontend presents these descriptions as separate neutral cards.
V5-specific character limits/positioning remain unimplemented.


xAI registers `xai_transcribe` with a separate audio-blob contract and transcription controls.
The host ASR adapter enforces the extension's preset scope during both ordered selection and
explicit provider selection. It retains bounded container reads, session ownership, one upstream
attempt and transcript JSON persistence. The legacy `asr` CLI route has been removed. Raw audio and live streaming are not implemented by this tool.

xAI also registers `xai_speech` with its own `voice_id`, `output_format`, `replace`,
normalization and timestamp controls. Its run/status/list_jobs operations use a speech-only
preset scope; neither MiniMax nor other audio jobs can be recovered through this tool.
The application audio adapter translates this request for the existing durable engine.
Empty pronunciation overrides and false options are preserved. Audio and timestamp artifacts
are saved separately, with the latter downloadable from the extension result view.

xAI registers `xai_video` with separate generate/edit/extend/status/list_jobs actions.
Its schema names xAI image, last-frame, reference, keyframe and preset-voice inputs directly;
assets use `{blob: id}` or `{url: address}`. A host adapter resolves these through the existing
bounded input pipeline, enforcing xAI-only service/job scope. Video-only sessions can discover
the tool. Model compatibility preflight drives default selection, including classic-only editing
and extension. The native interface covers classic, 1.5 and1.5-lite with independent preset selection.
Lite supports text and a single first-image input, including upscaled1080p, but no references,
keyframes, last frames, editing or extension. Payload construction and capability metadata now
live in `xai/video_wire.rs`; host adapters delegate to them. The native `generate_audio`
control survives local-input resolution without becoming a universal media option. First-image
animation drops inherited aspect-ratio defaults while still rejecting explicit incompatible framing.
Input contract ownership, polling and runtime execution remain migration work. Its frontend owns invocation and result presentation, uses deferred local
image previews, and shows remote reference URLs without fetching them in the browser.

Frontend extension packages own their tool presentation manifests and components. An optional
invocation renderer and a result renderer each receive opaque host context. NovelAI, BFL, MiniMax and xAI own both;
its reference/source image previews are mounted only after the user expands them. The host resolves
a tool name in its registry and passes context and opaque arguments/results; it does not interpret
every supplier's result. Registrations may reuse shared visual primitives. Only trusted bundled
components are executable; backend metadata does not supply JavaScript/component paths. Unrecognized
outputs retain the host's raw-result fallback. Legacy unified-tool records keep their old renderers
in a compatibility package, rather than requiring new extensions to imitate their output shape.

### Runway video extension

`runway/operation.rs` converts the public request into Runway-owned submission/status/list
operations. Mode selection, first/last-frame shortcuts, temporal guidance construction and
their combination rules live there. The application receives a typed submission and only
adapts its resources and plan to the durable job engine; it no longer round-trips the request
through JSON or interprets the keyframe fields itself.

`runway_video` owns separate generate/edit/status/list contracts. Generate selects configured
Gen-4.5 services; edit selects Aleph 2, with one video and optional first/last guidance frames.
Selection, listing and recovery are scoped to Runway presets, and session-owned inputs still
pass through the host's bounded asset preparation/upload path. The generation and Aleph edit
JSON builders live in `runway/wire.rs`. Frontend Runway invocation/result renderers are lazy
bundled, preview local images only after expansion, and use shared durable-job playback.
Runway now owns a typed submission plan, including seed, outpainting ratio and Aleph's timed
keyframes/edit windows. The plan builds both preflight placeholders and the final resolved-asset
request; native parameters do not pass through the universal media option schema. Up to five
frames use either absolute seconds or normalized position, never both; windows are all-or-none,
end-exclusive, and bounded locally by Aleph's 30-second source limit. Fractional frame/window
compatibility and actual media duration remain upstream checks. Optional edit prompts are omitted
from the wire when absent. First/last shortcuts cannot mix with explicit keyframes. All input
identifiers/URLs are checked before uploading an earlier large asset.
The old media hub still supplies options/input transport types, capability metadata, polling and
orchestration; these are not yet fully extension-owned. Aleph output accepts MP4, ProRes/MOV (with optional ProRes profile) and 10-bit Rec.709
HEVC/MP4. The expected container MIME is persisted with the job, so restart/retry validates the
requested container before publishing a blob. Storage recognizes the QuickTime ftyp brand.
The frontend offers downloads and does not mount QuickTime as an MP4/audio player. Container
recognition is not a codec-content verifier. PNG-sequence output is saved as an owned ZIP blob,
with the same persisted expected-MIME check. Before publication, the Runway validator checks
central/local headers, paths, frame signatures, sizes and CRCs without extracting files or decoding
pixels. Downloads are limited to 512 MiB, directories to 8 MiB / 10,000 entries, expanded frame
bytes to 2 GiB total / 256 MiB per frame. Multi-disk and ZIP64 central directories are rejected.
Validation runs through the bounded CPU executor; failures remain resumable downloads and do not
resubmit generation. Frontend ZIP results offer a download rather than a media player.
Runway image/audio tools remain unimplemented. Do not describe this migration as complete Runway
feature coverage.
