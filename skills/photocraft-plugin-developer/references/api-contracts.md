# Authoring contracts

Snapshot: 2026-10-16 plugin-platform implementation. The supplied host's discovery
and validation remain authoritative; this is not a promise that stock upstream
releases contain the panel/native package platform.

## Versions and routes

| Contract | Implemented version | Important boundary |
|---|---|---|
| PhotoCraft Plugin API | Internal wire discriminator `1` | Import-free core WASM, `pce_*` exports |
| Standalone pixel filter | ABI 1 | Import-free core WASM, `pc_*` exports |
| Desktop package | 2 | ZIP, panel module, optional owned native/filter backends |
| Native image/task worker | 1; explicit opt-in 2 | Protocol 1 `--job`/`--task`; worker 2 uses bounded framed `--serve` |
| Shared contract crate | Experimental `photocraft-plugin-api` 0.1.0 | Serialized data, no editor linkage required |
| Guest SDK | Experimental `photocraft-plugin-sdk` 0.1.0 | Typed events/replies, bounded ABI buffers, optional authoring generator |

The SDK exports the four panel functions and owns its bounded buffers. The host
only writes the exact current allocation. Erroring handlers preserve state and propose no
effects. Descriptors/effects stay strict; event envelopes and read-only snapshots may ignore
additional optional metadata. Breaking wire behavior requires a newly negotiated major.
API/SDK releases and plugin/protocol versions are independent. This foundation preview
does not establish a calendar support window or certify future capability categories.

The authoring input is `{formatVersion: 1, descriptor: <current panel>, package: <package-2>}`.
Cargo generates the embedded descriptor and Rust constants; the package builder reads the
same input. New projects may privately include the exported API/SDK source. Nothing links
against engine/UI types, and no shared runtime installation is required.

There is one supported Plugin API; milestones do not create new APIs or major versions.
Add compatible capabilities and fields until a genuine breaking change requires another major.
The current panel API and native transports use package format 2.
Worker 2 requires the separately verified runtime/worker-v2.json member and cannot be
probed or inferred from an executable. Typed descriptor/frames ship in the optional API crate.
Photoshop UXP, CEP and `.8BF` files require porting. Renaming files does not convert their ABI.

Current desktop hosts use lazy validated descriptors, bounded per-plugin scheduling,
per-generation OS leases, aggregate native admission estimates and explicit preview retention
budgets. Panel activation checks only bounded panel code; cancellable bulk verification and
filter compilation run in scheduled jobs. Managed storage snapshots/migrations and rollback select the same generation as code.
Packages need no shared plugin runtime. Restricted native containment remains
unavailable; trusted processes can exceed estimates or access the user's OS resources.
See the matching source/kit lifecycle, worker-2 and managed-storage references for exact limits.

## Panels

### Optional release updates

The current descriptor accepts an optional canonical public GitHub `repository`, for example
`https://github.com/example-org/independent-plugin`, and
`updates: {"provider":"github","asset":"Independent-{platform}.pcplugin"}`. Updates require
the repository. The asset is one exact safe ASCII filename ending in `.pcplugin` or `.pcext`;
only one `{platform}` token is allowed. It expands to the host's Windows/Linux/macOS x86-64
or ARM64 package target. A fixed `.pcext` filename can serve portable panels. Rebuild embedded
metadata and publish matching semantic release/tag versions under the same plugin ID.

Desktop release discovery is enabled by default for opted-in installed plugins. It waits at
least 30 seconds after startup and until user jobs, file dialogs and installation review are
idle, then observes a 24-hour cooldown. The last attempt, including failures, persists with
preferences across restarts. **Automatically check for plugin updates** in the manager or
**Preferences → Plug-ins** controls `plugIns.automaticUpdateChecks`. Manual **Check for updates**
can run immediately. Automatic checks are silent; results appear in the manager and the Plugins
menu's available-update count. They do not download code, launch workers or grant permissions.

Scheduling belongs to the desktop shell; browser, CLI/headless, inline-job, plugin-recovery
and restricted automation sessions do not schedule checks. Authorized command clients still
explicitly invoke `extension.updates.check` and `extension.updates.prepare`; guests gain no
network imports or updater runtime. Public stable releases only; draft/prerelease/private
repositories and credentials are unsupported. Assets
require a usable SHA-256 digest. The host verifies downloaded bytes, identity, version and
compatibility, then presents normal permission/native-code review. Installation rechecks the
reviewed package hash and installed revision. No automatic grant expansion or unattended
installation is implied. GitHub metadata/digests are not publisher signatures.

Download and installation remain explicit: **Update…** refreshes the short-lived candidate,
prepares the package and opens permission review; the user then confirms installation.
`extension.updates.prepare` returns inspection and reviewed input, never an installation.

Local/offline installation remains available. Newer same-ID packages offer Update; same-version
reinstall/repair and older-version installation are deliberate secondary maintenance actions.
No metadata is required for plugins distributed only as local files. Each repository and release
is owned by that independent plugin; PhotoCraft does not need source changes for a new publisher.
Marketplace discovery/signing remains future work. Consult docs/plugin-updates.md in a matching
source/kit for the complete contract.

A raw panel is conventionally `.pcext`. Required exports are `memory`,
`pce_api_version() -> i32`, `pce_manifest() -> i64`, `pce_alloc(i32) -> i32`, and
`pce_handle(i32, i32) -> i64`. Packed JSON results are `(length << 32) | pointer`.
Use the starter's owned, bounded ABI storage. The manifest is embedded at build time.

Controls are `number`, `int`, `bool`, and `choice`, plus buttons. Native egui renders
them; there is no HTML/JS UI or arbitrary guest drawing. The event envelope includes
`apiVersion: 1`, host metadata, `event`, `values`, `state`, an optional `document` summary
and correlated broker `responses`. `documentRead` grants
summary metadata, not arbitrary pixels, paths, or the layer tree. Preserve state
explicitly when an action should not change it. Globals do not survive fresh instances.

Responses carry `state`, `message`, and `commands`. Each command must be in both the
manifest grants and the host allowlist. Lists run sequentially; a later failure does
not undo earlier successful commands. Atomic edits use the distinct document broker contract;
command lists are not transactions. Processing/task state is committed at the operation's defined
completion boundary, not merely when the handler returns.

Panel limits: module 8 MiB, manifest 64 KiB, memory 32 MiB, call depth 256, fuel
5 million per instance, request/response 256 KiB each, state 128 KiB, 32 commands,
message 4096 bytes. These differ from filter limits. Every supported panel uses the same parser.

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

## Document capabilities and native views

Declare required/optional versioned capabilities and plugin-namespaced contributions.
Capability negotiation does not grant extra permissions. Current API exports retain the
panel signatures. Responses can use broker `requests` or sequential `commands`, not both.
Broker queries resume through bounded `host_response` events, not persistent guests.

`documents.layers.read` returns paginated snapshots and opaque document/layer handles
(page limit 1-128). `documents.layers.edit` accepts 1-128 distinct rename/visibility
edits with `expectedRevision`. An edit must be the sole broker request and originate
from an explicit action. It is terminal and atomic, with one Undo step; stale/invalid
handles or failed edits leave history and proposed state unchanged.

The current host supports one native read-only inspector and one matching
`document.changed` subscription per plugin. Observer handlers can read and update
bounded label/value views; they cannot edit, run commands, or start native jobs.
Open/close controls the subscription. Handle page truncation explicitly. Current API
support does not imply general event callbacks, custom layouts, or all proposed SDK categories.
