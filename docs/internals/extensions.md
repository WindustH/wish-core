# Extensions

Extensions own non-conversational capabilities. Each supplier package owns its public tool
schemas, argument validation, execution implementation, provider formats, capabilities, defaults
and result interpretation. Tags support discovery; they never determine how a request executes.

## Registration and execution

`extensions/package.rs` composes bundled packages once. Each supplier's `package.rs` registers
its tools, optional runners and `presets.json`. Tool discovery and runner lookup derive from
those packages; there are no parallel supplier lists in the application or protocol layers.

`Registration::parse` returns an erased `Invocation` containing the original typed request.
Each request implements `Executable` in its supplier package. There is no closed supplier
invocation enum and no generic configured-argument JSON rename adapter. New tools can define
independent parameters and results without changing the session dispatcher.

The session adapter captures the registration, allowed services and call identity before
execution. It enforces permissions, account/session ownership and cancellation, then calls the
extension. It never matches a supplier name or deserializes a supplier request.

## Optional runtime facilities

Extensions can reuse the image, durable-media, transcription and reader runners. Their internal
request types are resource-operation contracts, not external tool schemas. An extension opts
into a runner and performs its own typed conversion. Native media plans retain supplier-specific
state until the final build, including after bounded asset resolution; the host does not infer
Runway edits, MiniMax voice blends or xAI audio timing from a parameter bag.

- Image drivers own capability rules, parameter merging, input preprocessing, upload negotiation,
  submission/response handling and polling-origin policy. NovelAI owns reference-canvas preparation;
  OpenAI owns mask conversion; RunningHub owns its upload format.
- Media drivers own model/input compatibility, request defaults, source URI policies, upload
  thresholds, submission and polling formats, audio decoding and artifact validators.
- Transcription requests implement an open execution port. Their original supplier types own
  validation, multipart encoding and transcript projection; there is no supplier routing enum.
- Search and fetch codecs own endpoint declarations and request/response conversion. Host lookup
  obtains a registered codec rather than matching a protocol enum.

`application` composes configuration, accounts, resource limits and session storage. Its runtime
services supply durable jobs, immutable page snapshots, bounded blob access and publication.
These services are shared infrastructure; they contain no supplier-specific branches. Category
configuration remains a UI/permission grouping (images, video, audio, transcription, reading,
search), while the preset definitions and validation belong to extensions.

`extension_host` provides registry handles and mechanical runner plumbing. It cannot import a
supplier. Neither can application orchestration. `extensions/shared` cannot import suppliers;
extensions cannot import application, server, session, executor, tool or extension_host.
`protocol` owns shared credentials/signing and conversational LLM codecs, and cannot import
extensions. `tools/check_architecture.py` enforces these directions.

## Resource and recovery guarantees

Transport performs one HTTP attempt. Read retries are bounded; paid submissions never fail over
or automatically resubmit after an uncertain outcome. CPU and filesystem work use bounded worker
utilities. Requests retain their service scope during selection, job listing and recovery.

The durable-media runner stores an execution snapshot before submission. Status/download recovery
uses that snapshot and never creates a second generation. Per-path locks prevent simultaneous
recovery from downloading twice. Records are bounded to 2 MiB; serialization and parsing use the
CPU pool. Removed services do not transfer jobs to another extension.

Reader cursors resolve immutable session-owned snapshots with saved extension and service IDs.
A continuation cannot switch readers, even if a configured service ID is reassigned. New reads
respect configured priority within the chosen extension.

Artifact downloads enforce byte limits, redirect limits and credential isolation. Extensions
provide format validation where required. Runway ZIP validation checks directory/local headers,
paths, sizes, frame signatures and CRCs without extracting files or decoding pixels. The limits
are 512 MiB downloaded, 8 MiB directory / 10,000 entries, 2 GiB expanded total / 256 MiB per frame;
multi-disk and ZIP64 central directories are rejected. Audio timestamps/subtitles are separate
session blobs. Invalid paid responses remain uncertain outcomes, not another submission.

## Binary and model entry points

```sh
wish extension search image generation
wish extension inspect novelai_generate
wish extension call novelai_generate --json '{"prompt":"A watercolor landscape"}'
printf '%s' '{"action":"list_jobs"}' | wish extension call runway_video --stdin
```

Tags match with AND, case-insensitively. Search without tags lists available tools. Inspection
returns the selected schema, examples, service IDs and capabilities in configured order.
`service` pins an authorized service; omitted selection follows compatible configured priority
within the extension. Discovery never probes a remote provider.

Calls require session bridge credentials and the corresponding capability. Model calls use
`extension_tools` for discovery and the selected tool's own name/schema for execution. The old
`websearch`, `webfetch`, `imagegen`, `videogen`, `audiogen` and `asr` CLI entries and unified model
tool entry points are removed. Core commands (history, task, askuser, session, skill, mcp) remain.

## Frontend

Bundled frontend manifests own tool invocation/result renderers and declare their settings
categories. Category labels, permission keys and descriptions have one shared definition;
settings navigation and delivery controls derive from manifest declarations. A supplier may opt
into shared forms and render primitives without adopting another supplier's tool schema.

The host passes opaque arguments/results plus session context to a registered renderer. Only
bundled components can execute; backend metadata cannot supply JavaScript or component paths.
Unknown output has a raw fallback. Existing history renderers live in a presentation-only legacy
package and do not expose old execution entry points. Reference images mount lazily when expanded;
remote reference URLs are displayed without being fetched by the browser.

## Capability limits

Architecture completion does not imply every upstream product feature is implemented. NovelAI's
native tool supports generation, image-to-image, V4.5 precise references and structured character
prompts; mask/inpainting, Vibe Transfer, Director Tools and V5-specific character controls remain
outside its implemented schema. Runway currently exposes video generation/editing and durable
recovery, not Runway image/audio tools. Audio tools do not implement live streaming or voice cloning.
Actual model-specific constraints beyond local validation remain upstream checks.
