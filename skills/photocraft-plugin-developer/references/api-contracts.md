# Authoring contracts

Snapshot: 2026-10-15 native-extensions implementation. The supplied host's discovery
and validation remain authoritative; this is not a promise that stock upstream
releases contain the panel/native package platform.

## Versions and routes

| Contract | Implemented version | Important boundary |
|---|---|---|
| Panel | 1; additive experimental 2 | Import-free core WASM, `pce_*` exports |
| Standalone pixel filter | ABI 1 | Import-free core WASM, `pc_*` exports |
| Desktop package | 2 | ZIP, panel module, optional owned native/filter backends |
| Native image/task worker | 1 | Host-owned requests/results, `--job` or `--task` |
| Shared contract crate | Experimental `photocraft-plugin-api` 0.1.0 | Serialized data, no editor linkage required |

Package 3 and worker protocol 2 are proposals. API-2 panels still use package 2.
Retired package-1 fixed AI services are not an authoring route. Photoshop UXP, CEP,
and `.8BF` files require porting. Renaming files does not convert their ABI.

## Panels

A raw panel is conventionally `.pcext`. Required exports are `memory`,
`pce_api_version() -> i32`, `pce_manifest() -> i64`, `pce_alloc(i32) -> i32`, and
`pce_handle(i32, i32) -> i64`. Packed JSON results are `(length << 32) | pointer`.
Use the starter's owned, bounded ABI storage. The manifest is embedded at build time.

Controls are `number`, `int`, `bool`, and `choice`, plus buttons. Native egui renders
them; there is no HTML/JS UI or arbitrary guest drawing. API-1 events contain
`event`, `values`, `state`, and an optional `document` summary. `documentRead` grants
summary metadata, not arbitrary pixels, paths, or the layer tree. Preserve state
explicitly when an action should not change it. Globals do not survive fresh instances.

Responses carry `state`, `message`, and `commands`. Each command must be in both the
manifest grants and the host allowlist. Lists run sequentially; a later failure does
not undo earlier successful commands. Do not claim atomic batches or command-result
callbacks for API 1. Processing/task state is committed at the operation's defined
completion boundary, not merely when the handler returns.

Panel limits: module 8 MiB, manifest 64 KiB, memory 32 MiB, call depth 256, fuel
5 million per instance, request/response 256 KiB each, state 128 KiB, 32 commands,
message 4096 bytes. These differ from filter limits. API 1 rejects API-2-only fields.

## Package and native image boundary

A package ZIP has `package.json` and a panel such as `plugin.pcext`, plus its declared
payload, `LICENSE`, and `THIRD-PARTY-NOTICES.md`. Optional `native` metadata specifies
`service: "plugin.processImage"`, an exact OS/architecture, and a safe relative entry.
Optional `filters` refer to bundled filter modules; select them by their embedded
manifest IDs. `native` is a reserved processor alias. Private filters are not global.

The panel grants `plugin.processImage`, enables `panel.preview` for comparison UI,
and returns one processing command as the entire action:

```json
{
  "state": null,
  "commands": [{
    "command": "plugin.processImage",
    "params": {
      "processor": "native",
      "options": {"gain": 1.25},
      "preview": true
    }
  }]
}
```

Panel requests omit the installation `id`, executable paths, and launch arguments.
Trusted direct callers supply `id`. Options are an object of at most 128 KiB; the
worker defines and validates its schema. Optional `width`/`height` request native
output dimensions. Bundled filters retain source dimensions.

Native workers receive `--job` and an absolute JSON request path. Protocol 1 supplies
`apiVersion: 1`, `input`, `output`, `options`, `outputSize: {width, height}`, and `colorSpace: "srgb"`.
Input is a visible flattened 8-bit RGBA sRGB PNG converted from supported 8/16-bit
RGB/grayscale sources. Output is an exact-size 8-bit RGB/RGBA sRGB PNG. Preserve alpha
and DPI semantics. Native jobs get no separate selection mask; bundled WASM results
are blended by host selection coverage. These are not high-precision layer APIs.

Current image bounds are 100 MP input, 256 MP output, 32,768 pixels per side, and a
working-set estimate capped at 8 GiB on 64-bit hosts or 2 GiB on 32-bit hosts.
Algorithms may need stricter bounds. A preview is bound to source revision and plugin
installation. New-tab Apply opens a processed copy. Same-size layer Apply is undoable
and hides the original top-level stack to avoid double-compositing transparency.

Progress is newline-delimited stderr JSON, such as `{"progress":50,"message":"Processing"}`.
Read stdin concurrently and stop on `cancel\n` or EOF; stop/reap owned child processes.
The host has a 45-minute deadline and ten-second cancellation grace before terminating
the direct worker. Native code has the user's OS privileges; WASM bounds do not sandbox it.

## Document-independent tasks

`plugin.runTask` needs an owned native backend and its own grant in addition to the
backend's `plugin.processImage` grant. A panel returns it as the sole command, with
object `options` and no installation/process selector. No document is required.
The worker receives `--task` and JSON containing `apiVersion: 1`, `options`, and `output`.
Write a regular JSON result of at most 128 KiB:

```json
{"apiVersion":1,"message":"Assets are ready.","data":{"ready":true}}
```

The message is nonblank and at most 8192 UTF-8 bytes; `data` is an object. This data
does not become arbitrary UI, commands, or guest state. Validate options, fixed asset
sources, sizes, hashes, and licenses in the plugin. Task side effects may already
exist if later state/revocation checks fail; do not describe them as transactional Undo.

## Standalone filters

Exports are `memory`, `pc_abi_version`, `pc_manifest`, `pc_alloc`, and `pc_filter`.
The ABI supplies interleaved little-endian `f32` samples in the target's own color
model, with source depth encoded in `format`. Standalone targets may be 8/16/32 bit;
finite HDR/negative float samples need intentional handling. Do not reinterpret this
as RGBA8. Channel Gain supports RGB/grayscale only; other modes need an algorithm and tests.
The host handles selection blending and overlap. Use `_image` band geometry for
coordinate effects. Each band gets a fresh instance, without cross-band state.

## Experimental panel API 2

Declare required/optional versioned capabilities and plugin-namespaced contributions.
Capability negotiation does not grant extra permissions. API-2 exports retain the
panel signatures. Responses can use broker `requests` or legacy `commands`, not both.
Broker queries resume through bounded `host_response` events, not persistent guests.

`documents.layers.read` returns paginated snapshots and opaque document/layer handles
(page limit 1-128). `documents.layers.edit` accepts 1-128 distinct rename/visibility
edits with `expectedRevision`. An edit must be the sole broker request and originate
from an explicit action. It is terminal and atomic, with one Undo step; stale/invalid
handles or failed edits leave history and proposed state unchanged.

The current host supports one native read-only inspector and one matching
`document.changed` subscription per plugin. Observer handlers can read and update
bounded label/value views; they cannot edit, run commands, or start native jobs.
Open/close controls the subscription. Handle page truncation explicitly. API-2
support does not imply general event callbacks, custom layouts, or all proposed SDK categories.
