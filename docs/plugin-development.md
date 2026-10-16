# PhotoCraft plugin developer handbook

Build a plugin as a separate project, compile it to WebAssembly, and install it in PhotoCraft.
You do not link against PhotoCraft's Rust crates or rebuild the editor for a plugin that uses
the public APIs. Independent image processors can supply their own native worker or
packaged WASM filter through package version 2. This handbook takes you from a new project to a tested release using
neutral starters. It does not depend on any AI plugin project.

**API status:** [the PhotoCraft Plugin API](plugin-api.md) is the only supported authoring contract.
Its internal panel discriminator is `2`; obsolete version-1 panels must be rebuilt. Filter ABI
version 1 and general package format 2 serve their existing layers. Native worker 1
is preserved; [worker protocol 2](plugin-worker-v2.md) is an explicit persistent-worker opt-in.
Use a build
containing these APIs; an older release with the same application version number may not contain
this contribution. Check that Plugins contains Manage Plugins. API versions, native package
versions and your plugin's release version are different numbers.

## Contents

- [Standalone current API SDK](plugin-sdk.md)
- [Lifecycle, scheduling and recovery](plugin-lifecycle.md)
- [Persistent native workers](plugin-worker-v2.md)
- [Managed storage and migrations](plugin-managed-storage.md)
- [Installation and GitHub release updates](plugin-updates.md)
- [Future marketplace plan](plugin-marketplace.md)
- [Development with agents](#development-with-agents)
- [Choose a plugin type](#choose-a-plugin-type)
- [Prepare your tools](#prepare-your-tools)
- [Build a first panel plugin](#build-a-first-panel-plugin)
- [Understand the project files](#understand-the-project-files)
- [Design a manifest and native controls](#design-a-manifest-and-native-controls)
- [Write a handler](#write-a-handler)
- [Use commands and permissions](#use-commands-and-permissions)
- [Manage state and document targeting](#manage-state-and-document-targeting)
- [Build a pixel filter](#build-a-pixel-filter)
- [Build an independent image processor](#build-an-independent-image-processor)
- [Run native maintenance tasks](#run-native-maintenance-tasks)
- [Understand processing and previews](#understand-processing-and-previews)
- [Test and debug](#test-and-debug)
- [Release and update](#release-and-update)
- [Package native code](#package-native-code)
- [Compatibility and current limits](#compatibility-and-current-limits)
- [Host architecture and new capabilities](#host-architecture-and-new-capabilities)
- [Companion applications](#companion-applications)
- [Troubleshooting](#troubleshooting)
- [Reference documents](#reference-documents)

## Development with agents

The reusable [PhotoCraft Plugin Developer skill](../skills/photocraft-plugin-developer/SKILL.md)
guides an agent through current API selection, independent source ownership, builds,
packaging, and acceptance testing. Its bundled references work when the skill is installed
outside this checkout. Copy its complete folder into your agent's skill directory when
using a client that supports this skill format; it does not require an MCP connection.

Each of the five standalone starters below includes a local `AGENTS.md` with its own
contract, files, commands, and verification rules. Copy that file with the entire starter.
It remains useful after extraction without linking to editor-internal source files.
The editor's root `AGENTS.md` continues to describe core development; it is not a plugin SDK.

The [standalone current API SDK](plugin-sdk.md) provides typed messages and safe ABI exports. Its
`new` command creates a project with private SDK sources, one authoring manifest and generated
bindings. SDK use is optional; existing independent starters and binaries remain supported.

MCP can discover commands and inspect/test allowed behavior, while filesystem and build
tools create the plugin. Current restricted automation denies managed panel/package installation and
processing/task execution; these instructions do not expand that authority. Distinguish
package/handler validation from real-host acceptance, and report unavailable checks.

## Companion applications

A separate application can use the existing [authenticated local control protocol](control-protocol.md)
or its MCP bridge to discover commands, inspect documents and invoke permitted editor or plugin
contributions. Start PhotoCraft with control enabled and authenticate each connection with its
token. A companion can be written in any language that supports the documented JSON transport.

The guest SDK and its scoped handles belong to an installed plugin invocation; they are not an
external RPC endpoint. A companion uses the control command surface, or supplies an optional
standalone plugin adapter with its own packaged implementation. Keep every plugin usable without
the companion. Do not make an editor change for a particular companion application.

Automation policies still apply to command execution and file access. The current local bearer
token does not provide separate per-companion capability grants, and the control server is
loopback-only. Remote companion access, independent client consent/revocation, and forwarding
plugin-scoped subscriptions require additional public contracts; they are not SDK features.

## Choose a plugin type

| You want to build | Choose | Distributed file | Installation |
|---|---|---|---|
| A native panel combining supported editor actions | Panel extension API | `.pcext` | Plugins → Manage Plugins… |
| A custom algorithm changing a filter target's pixels | Filter ABI | `.wasm` | Plugins → Install Plug-in… |
| A panel with your own native image processor or packaged WASM filters | Package version 2 plus `plugin.processImage` | `.pcplugin` | Plugins → Manage Plugins…; native-code review when a worker is included |
| A panel distributed as a package without an image backend | Package version 2 plus panel API | `.pcplugin` | Plugins → Manage Plugins… |
| A panel managing its own assets or diagnostics without an image open | Package version 2 plus `plugin.runTask` and an owned native worker | `.pcplugin` | Plugins → Manage Plugins…; native-code review |
| Namespaced commands or atomic layer rename/visibility edits | PhotoCraft Plugin API; [Layer Organizer](../examples/extensions/layer-organizer/) | `.pcplugin` or `.pcext` | Plugins → Manage Plugins… |
| A live read-only native inspector | PhotoCraft Plugin API; [Document Inspector](../examples/extensions/document-inspector/) | `.pcplugin` or `.pcext` | Plugins → Manage Plugins… |

The WebAssembly APIs are different contracts. Panel guests export `pce_*`; filters export `pc_*`.
Their manifests, budgets and execution models differ. Renaming a file does not change its API.
Both appear under Plugins.

A panel can request a supported built-in filter command. To use its own custom WASM pixel
algorithm, bundle that filter in a version-2 package and request `plugin.processImage`.
The selected filter belongs to that installation; the panel cannot call `plugin.run` to
borrow an independently installed global filter or another package's backend.

Use the panel starter when permitted editor commands express your behavior, and the filter
starter when your primary work is pixel math. Check the capability table before designing
custom UI, network integration or a native engine.

## Prepare your tools

You need a text editor, Rust with Cargo, and a compatible PhotoCraft build. The starters use
Rust edition 2024 and declare their minimum version in `Cargo.toml`; the current editor source
requires Rust 1.95 or later. Building a raw WASM guest does not require the editor's graphics
dependencies. Native tests also need Rust's normal linker; on Windows see the
[host development prerequisites](development.md#windows-source-builds).

Install Rust with the [official rustup instructions](https://rust-lang.github.io/rustup/installation/index.html),
then check your tools and add the guest target:

```sh
rustc --version
cargo --version
rustup target add wasm32-unknown-unknown
```

Use `wasm32-unknown-unknown`, not WASI. PhotoCraft rejects guest imports, including filesystem,
network and WASI functions. Rust's minimal WASM target supports allocation and JSON processing,
but OS facilities do not behave like desktop facilities; see the
[Rust target documentation](https://doc.rust-lang.org/rustc/platform-support/wasm32-unknown-unknown.html).
Do not add a JavaScript loader, `wasm-bindgen` web wrapper or webview to these starters.

Develop in a separate directory or repository. The empty `[workspace]` keeps each starter
independent of the editor workspace. Copy lockfiles, license files, `rustfmt.toml` and
`clippy.toml` with the source so the checks also work outside the editor checkout.

## Build a first panel plugin

The neutral [panel starter](../examples/extensions/plugin-starter/README.md), **Hello PhotoCraft**,
has a Layer name dropdown, Show document info, and Create layer. It requests only document
summary access and `layer.new.layer`. It has no native engine or dependency on another plugin.

### Copy the starter

From a PhotoCraft source checkout on Windows, choose your development directory:

```powershell
New-Item -ItemType Directory -Path C:\work -Force | Out-Null
Copy-Item -LiteralPath .\examples\extensions\plugin-starter -Destination C:\work\hello-photocraft -Recurse
Set-Location C:\work\hello-photocraft
```

On macOS/Linux:

```sh
cp -R examples/extensions/plugin-starter ../hello-photocraft
cd ../hello-photocraft
```

Use a new destination folder so you do not merge the starter into an existing project.
The developer kit also includes this directory; you can work from the extracted starter.
Do not copy `target/` or another developer's output into your source repository.

### Check and build

Inside your copied project:

```sh
cargo fmt --all -- --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

Build the installable panel on Windows:

```powershell
.\build.ps1
```

On macOS/Linux:

```sh
sh build.sh
```

Both scripts produce **`Hello-PhotoCraft.pcext`** beside the manifest. The underlying build is:

```sh
cargo build --locked --release --target wasm32-unknown-unknown
```

The raw output is `target/wasm32-unknown-unknown/release/hello_photocraft.wasm`. The `.pcext`
copies those same bytes under a panel distribution name. `CARGO_TARGET_DIR` changes the raw
output directory; the scripts account for it.

### Install and try it

1. Start the compatible PhotoCraft build.
2. Open **Plugins → Manage Plugins…** and click **Browse…**, or drop the file into the installer.
3. Choose `Hello-PhotoCraft.pcext`, review its identity/access, and install.
4. Open **Plugins → Hello PhotoCraft**.
5. With no document open, Show document info should explain that an image is needed.
6. Create/open a disposable document, choose Notes, and select Create layer.
7. Check the new layer's name, then use Edit → Undo and Edit → Redo.

This exercises compilation, ABI loading, review, menu registration, panel generation and editing.
After installation you may move/delete the downloaded `.pcext`; the desktop uses a managed copy.

## Understand the project files

```text
hello-photocraft/
  Cargo.toml          Independent Rust cdylib and dependencies
  Cargo.lock          Exact dependency resolution
  manifest.json       Embedded identity, grants, fields and buttons
  src/lib.rs          Module entry and embedded manifest/constants
  src/abi.rs          ABI exports and bounded request/response memory handling
  src/handler.rs      Your behavior and native unit tests
  build.ps1           Windows build and local output copy
  build.sh            POSIX build and local output copy
  README.md           Starter usage and constraints
  LICENSE-MIT         Source license option
  LICENSE-APACHE      Source license option
  .gitignore          Generated modules and build directories
```

Most panel development changes **`manifest.json` and `src/handler.rs`**. Keep the ABI plumbing
unless you need to change its contract. It handles bounds, safe buffer ownership and packed
pointer/length results. Rust's `#[unsafe(no_mangle)]` attributes expose stable symbols; they
neither grant OS access nor require unsafe memory access in the host.

Before distribution, change the reverse-DNS id to one you control, name, author, description
and version. Renaming the Cargo package also changes the raw module filename: update both
scripts. Cargo turns hyphens into underscores in library filenames. Installed identity comes
from the embedded manifest, independently of the Cargo package and filename.

## Design a manifest and native controls

The complete panel starter manifest is:

```json
{
  "apiVersion": 2,
  "id": "org.example.hello-photocraft",
  "name": "Hello PhotoCraft",
  "version": "1.0.0",
  "author": "Example author",
  "description": "A starter panel that shows document information and creates a named layer.",
  "permissions": {
    "documentRead": true,
    "commands": ["layer.new.layer"]
  },
  "capabilities": {"required": [{"id": "host.commands.invoke", "minVersion": 1}], "optional": []},
  "panel": {
    "fields": [
      {"id": "layerName", "label": "Layer name", "type": "choice",
       "options": ["Notes", "Retouch", "Overlay"], "default": "Notes"}
    ],
    "buttons": [
      {"id": "info", "label": "Show document info"},
      {"id": "create", "label": "Create layer"}
    ]
  }
}
```

The manifest is compiled into the module. A loose JSON file beside an installed guest does
not change it; rebuild and replace the installation after editing.

| Control | Complete field specification |
|---|---|
| Float slider | `{"id":"strength","label":"Strength","type":"number","min":0,"max":1,"default":0.5}` |
| Integer slider | `{"id":"copies","label":"Copies","type":"int","min":1,"max":8,"default":1}` |
| Checkbox | `{"id":"enabled","label":"Enabled","type":"bool","default":false}` |
| Dropdown | `{"id":"mode","label":"Mode","type":"choice","options":["Soft","Hard"],"default":"Soft"}` |

Values arrive under `request.values` by field id. They do not automatically become command
parameters; the handler maps them. Missing/null values take defaults; numbers are clamped;
integer inputs are rounded then clamped. Unknown value keys, wrong types and unknown choices
fail. Do not rely on unknown field-schema keys being rejected: field parsing is not a full
external JSON Schema validator.

The current declarative panel has no free-text input, color/file picker, dynamic dropdown, arbitrary layout,
HTML/CSS, dockable guest panel or custom image widget. Use named choices in the manifest or
contribute a host control type. Changing a field does not invoke the guest; buttons do.

Plugin ids are lowercase ASCII, 1–64 bytes, begin with a letter/digit, and use letters, digits,
dots, underscores or hyphens. Field/button ids are 1–64 ASCII letters/digits/underscores, cannot
begin with `_`, and are unique within each list. Field ids `id` and `layer` are reserved.
There may be at most 32 fields and 1–16 buttons. See [manifest validation](extensions.md#manifest)
for exact text, numeric and version bounds.

## Write a handler

The pure `handle(&Value) -> Value` in `src/handler.rs` can be unit-tested without PhotoCraft.
The ABI wrapper deserializes, calls it and serializes its response. Keep behavior separate from
memory plumbing.

A Create layer click receives this request shape:

```json
{
  "apiVersion": 2,
  "host": {"version": 1, "capabilities": [{"id": "host.commands.invoke", "version": 1}]},
  "event": "create",
  "values": {"layerName": "Notes"},
  "state": null,
  "document": {"id": 1, "name": "Untitled", "width": 1920, "height": 1080,
               "depth": 8, "mode": "Rgb", "revision": 0, "activeLayer": 1}
}
```

Ids/dimensions are illustrative. `document` and `activeLayer` can be null. Mode is a host name
such as `Rgb` or `Grayscale`, not an invented lowercase enum. A response may propose:

```json
{
  "state": {"createdLayers": 1},
  "message": "Created the requested layer.",
  "commands": [{"command": "layer.new.layer", "params": {"name": "Notes"}}]
}
```

The host validates/executes the command; the guest never directly mutates the document. Message
and state are proposed before execution. Failure becomes a host error; state commits only on
success. The starter preserves state for info/error responses.

To add a feature:

1. Choose a supported command and inspect its parameters.
2. Declare the exact id in `permissions.commands`.
3. Add supported fields and a button id to the manifest.
4. Handle that event, validate values and emit object-valued `params`.
5. Return all next state and a concise status message.
6. Add tests, rebuild, replace the installation and test the real edit/Undo.

Avoid panics, unchecked conversions, unbounded loops and large responses. Unknown events or
missing documents should give a useful response with no commands. Response fields are optional,
but omitted `state` becomes null after success; omitted `message`/`commands` become empty.
Each command must include `params`, even `{}`. Unknown response/invocation fields are rejected.

### The event is one way

The guest receives `event`, `values`, `state` and `document`, not a host API object, promises,
callbacks or a canvas. It returns its whole list before any command runs. Results go to the
trusted caller of `extension.invoke`, not back into this invocation. You cannot interpolate a
first command's newly created layer id into the next command. Use documented active-target
behavior or propose a host capability for a different interaction.

## Use commands and permissions

Editor automation exposes more commands than panel plugins. A grant must pass the explicit
extension allowlist; an engine command is not automatically a plugin capability.

The [panel command reference](plugin-command-reference.md) lists supported ids and parameter
descriptions. The running build is authoritative. With `photocraft-cli`:

```sh
photocraft-cli run --new '{"width":64,"height":64}' --cmd extension.commands
photocraft-cli commands --json --filter layer.new.layer
```

These JSON-bearing examples use POSIX shell or PowerShell 7.3+ native argument handling. Older
Windows PowerShell handles quotes differently; use PowerShell 7.3+ or the Python subprocess
example in [testing](plugin-testing.md). Build scripts do not need JSON arguments.

`extension.commands` returns `{apiVersion,commands,maxJsonBytes}`; `command.list` returns an
array of engine descriptions. Its `params` strings are human-readable, not machine-enforced
JSON Schemas. The guest cannot call these discovery tools; they are outside its allowlist.

Three gates must pass: the host allows the id, the manifest grants it, and the session authorizes
it. The full id/grant/authorization list is checked before the first command runs. Each command
then validates its parameters and enabled state as it executes. Locks, selection, layer types
and color modes can still make a permitted command fail.

### Targeting and Undo

Commands keep normal editor behavior: active document, active/selected layers, target channel
and selection. They do not all process the whole visible composite. Read each one's contract.

A multi-command response executes sequentially and is **not atomic**. A later failure does not
roll back earlier steps. Commands retain normal history behavior; some create undo steps,
others select objects, move history or open documents. Do not promise one Undo per button unless
the host command itself supplies that transaction. Start with one edit per action.

Do not use `edit.undo` as improvised rollback: your plugin cannot safely guess which user history
step to reverse. Handle no-op/errors by returning prior state and no commands.

## Manage state and document targeting

| Data | Lifetime |
|---|---|
| Guest globals and memory | One event's fresh WASM instance |
| Returned JSON state | Installed plugin in this editor session; shared across documents |
| Fields and panel visibility | Saved/revalidated host UI state |
| Installed bytes and enabled flag | Persistent desktop plugin store |
| Preview token/result | Session only; invalidated by lifecycle rules |

State starts null. It survives panel close, disable/re-enable and ordinary document Undo, but
resets on replacement, removal, reload or restart. It is not saved in image files. A counter
counts successful events, not live layers after Undo or other edits. Key document-specific state
by the snapshot document id and check its revision. The next click may target another document.

`documentRead` exposes only id, name, width, height, depth, mode, revision and active layer id.
It does not expose paths, pixels, a layer tree, selection details or arbitrary `document.inspect`
calls. Without permission or a document, `document` is null.

Serialized state must fit 128 KiB; an oversized proposal is rejected before any returned commands.
The next complete request must fit 256 KiB including state, values and document information.
Keep state small; do not store images, big caches or native pointers. A command-list failure
leaves proposed state uncommitted even if earlier steps edited the document. State is not an
authoritative audit log for partial batches.

## Build a pixel filter

The independent [filter starter](../examples/plugins/filter-starter/README.md), **Channel Gain**,
contains a small custom RGB/grayscale algorithm and parameter dialog without a panel or native
engine. It is a neutral starting point for your own pixel math.

Copy `examples/plugins/filter-starter` to your own repository, as with the panel starter. Inside
that copied directory, run the same native checks and the platform's build script:

```sh
cargo fmt --all -- --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

```powershell
.\build.ps1
```

```sh
sh build.sh
```

The distributed file is **`Channel-Gain.wasm`**. Open **Plugins → Install Plug-in…**, click
**Browse…** and choose the file (or drop it into the installer), then confirm **Install Filter**.
Choose **Plugins → Channel Gain**. Set Gain and apply to a disposable pixel layer; test
Undo. The additional plug-ins folder preference enables filter discovery at desktop startup;
it is distinct from the panel manager's persistent store.

A filter manifest has `kind: "filter"`, `params`, `overlap` and `area`, not panel permissions
or button events. Its exports are `memory`, `pc_abi_version`, `pc_manifest`, `pc_alloc` and
`pc_filter`. See the [filter ABI](plugins.md#abi-v1) for exact signatures.

### Pixel-buffer rules

The host supplies bands of interleaved **little-endian f32** samples, including in 8/16-bit
documents. `format` describes depth, straight alpha and source mode. Channels use the document's
own color model, not implicitly sRGB. Alpha is last when present. A targeted alpha channel or
Quick Mask arrives as one grayscale channel.

A fresh instance processes each band; globals cannot accumulate across an image. Use `_image`
in parameter JSON for document-relative geometry. The host applies selection coverage after
filtering. For neighborhood algorithms, declare required `overlap` (0–256), process the supplied
margins, and let the host keep the inner result. Stateful scans/whole-image statistics cannot
assume pixels arrive in one invocation or in order.

Channel Gain supports RGB/grayscale only, preserves alpha, preserves finite HDR/negative colors
at float32 depth, and clamps 8/16-bit color results. Other modes return a nonzero result. Change
the pure processor and its tests together. Useful tests include several depths, alpha, selection,
margins, unsupported modes, tiny images, malformed parameters and failed-run document preservation.

Filter limits differ from panel limits: 32 MiB default module cap, 512 MiB linear memory, plus
a native wall-clock budget and instruction fuel. Read [the sandbox](plugins.md#the-sandbox)
before writing costly algorithms. Filter application is a transaction with normal target/history
semantics; this does not make panel command batches atomic.

## Build an independent image processor

Use the [image-processing tutorial](plugin-image-processing.md) and independent
[Image Gain starter](../examples/extensions/image-plugin-starter/README.md). Its one package
contains a WASM panel, its own native executable and a packaged WASM filter, with both backends
demonstrating the same gain operation. No core code changes, host path dependencies, existing
AI plugin or companion app are needed. `build.ps1 -Check` or `sh build.sh --check` builds,
tests and packages it using Rust and Python's standard library.

The panel grants `plugin.processImage`, selects `native` or a packaged filter manifest id,
and supplies an options object. PhotoCraft binds the owning installation automatically,
normalizes its visible composite to 8-bit RGBA sRGB, runs the selected backend and stages a
comparison. Native workers can return a requested size; bundled filters retain source size.
At matching dimensions Apply can create an undoable layer; it can also open a new result tab.
The tutorial defines the exact job, pixel, selection, size, cancellation and trust contracts.

## Run native maintenance tasks

A native package can request `plugin.runTask` for explicit operations such as managing its
assets or checking its runtime. These actions need no open image. The host invokes only that
installation's worker with `--task`, provides bounded JSON request/result paths, and owns
progress, cancellation and installation leases. The task cannot return document edits or
arbitrary host commands. Read the [complete native task protocol](plugin-native-tasks.md)
before implementing it, including cache ownership, download verification, side-effect cleanup
and the native-code trust boundary. A task must be the sole command returned by its button.

## Understand processing and previews

Panel guests should finish quickly. Ordinary returned commands run synchronously; costly ones
can delay the app. There is no general timer, document-change subscription, field-change callback,
async result callback, guest progress stream or guest cancel callback.

`panel.preview: true` selects the host's comparison layout. It does not let the guest upload
arbitrary preview pixels. A supported image service must create a staged preview. The layout
flag alone does not make ordinary command edits temporary. To preview your own packaged WASM
filter or native image worker, request `plugin.processImage` as described in the image tutorial.

For services supporting it, generation stages a result without editing. The host supplies
Fit/100%, comparison, Apply/Discard and compatible destinations. Apply rechecks source
identity/revision, plugin lifecycle and guest state. Same-size layer output keeps originals hidden in one
Undo step so alpha matches the preview. Proposed state commits on valid Apply, not generation;
Discard keeps prior state. Closing a preview panel cancels/discards its work. A compact
non-preview panel can leave supported background work running after closing; Cancel is separate.

General image processing is supplied by `plugin.processImage`; capabilities outside that
contract and general background execution still need host work. Read the
[native package reference](plugin-native-packages.md) for the boundary and the
[shared preview lifecycle](plugin-image-services.md) for Apply/Discard and history details.
Algorithm settings and models are defined and documented by each plugin.

## Test and debug

Use three validation levels:

1. **Pure code:** native tests for handler decisions/pixel math. These do not prove WASM exports.
2. **Compiled module:** load the release artifact in the actual host to catch wrong exports,
   imports, memory, instruction support, manifest/grant errors and runtime budgets.
3. **Editor behavior:** verify menu/panel/dialog, targets, edits, errors, Undo/Redo, replacement,
   restart and supported lifecycle states.

The [testing guide](plugin-testing.md) supplies CLI smoke steps, a Python subprocess runner for
both starters, failure cases, CI steps and logging guidance. The desktop manager is sufficient
for ordinary manual testing; the CLI is optional.

Use the response `message` and host action errors for user-facing diagnostics. No guest
`console.log`, stdout or debugger import exists. Native unit tests can log on your machine;
WASM `println!` is not a plugin logging channel. See [host logs](development.md#logs) for app
installation/runtime diagnostics. Avoid logging image data, private paths or credentials.

There is no watcher/hot reload. Edit, build and choose the new file. The manager offers **Update**
for a newer version of the same identity; use the secondary **Reinstall / Repair** action when
rebuilding the same version. Reopen a panel when controls change. Keep the id for updates. Editing
only `manifest.json` without rebuilding changes nothing.

## Release and update

Distribute a panel's `.pcext` or a filter's `.wasm`. Keep source, locks, tests, scripts and
licenses in your own source repository/ZIP. Exclude generated modules and `target/` unless
you deliberately track release artifacts. A version-2 `.pcplugin` can distribute a panel alone,
with packaged filters, with a native image worker, or with both backends. A raw `.pcext` remains
the simplest distribution for a panel needing no bundled files.

A release workflow:

1. Set a unique id, truthful identity and a documented plugin version.
2. Commit source and lockfiles in your repository.
3. Run checks, build release, and install the exact artifact you will publish.
4. Test on every platform/PhotoCraft build you claim to support.
5. Include install/update/uninstall instructions, source license and dependency notices.
6. Publish the artifact with its source revision and an optional independent checksum.

Local installation compares semantic release versions to distinguish an update from maintenance.
An older version requires an explicit maintenance action. Plugins may opt into checks and
downloads from their own public GitHub Releases using optional `repository` and `updates`
metadata. The manager shows available updates, prepares the selected package for access review,
and clears the selected file after successful installation. See [plugin updates](plugin-updates.md)
for metadata, commands and limits. Minimum application-version constraints, dependency resolution
and publisher signatures are not implemented. The manager's hash pins reviewed bytes, not
publisher identity; the [marketplace](plugin-marketplace.md) remains future work.

Desktop panel installations/payloads/enable state persist; guest state does not survive restart.
Standalone filters register per process and can reload from their additional folder; they do not use panel
manager enable records. Version-2 packaged filters belong to their managed installation and
are invoked through its panel, without global registration. A document's plugin Smart Filter needs its standalone filter installed for fresh
evaluation. Cached pixels do not distribute the required plugin. See [filter installation](plugins.md#using-plug-ins).

Use the manager's Remove for panels. For filter folder discovery, remove the file to prevent
future loading, then call `plugin.remove` for its id in the current process or restart the editor.
`plugin.reload` loads/replaces files it finds; it does not unregister missing files. Do not edit
PhotoCraft's internal installation records manually.

## Package native code

A version-2 `.pcplugin` may bundle your independent native image executable, its dependencies
and optional WASM filters. It uses the public `plugin.processImage` job and PNG contract;
adding a new algorithm within that contract does not require an editor change. Package native
dependencies for the declared Windows/Linux/macOS x86-64 or ARM64 target. Accepted platform
metadata does not establish compatibility; Windows x64 is the verified target here.

Read [Native plugin package author reference](plugin-native-packages.md) before this route.
It covers `package.json`, required paths, bounds, ZIP rules, hashes, native-code consent,
integrity, cancellation, updates and the worker protocol. Earlier fixed-service packages
must be rebuilt as version 2; algorithm adapters and model registries are not part of the core.

Keep all algorithm code, model choices, option validation, inference libraries and redistributable
runtime dependencies in your plugin project and package. The editor supplies only shared APIs.
Verify a clean extracted package with a restricted PATH so an installed development runtime
does not conceal a missing dependency. Record ordinary OS and hardware-driver prerequisites.

## Compatibility and current limits

| Capability | Independent author support |
|---|---|
| Generated panel: number/int/bool/choice and buttons | Yes |
| Supported command combinations | Yes, with grants and normal preconditions |
| Session JSON state and limited document summary | Yes |
| Custom pixel math | Yes, through the separate filter ABI |
| Panel invoking its own packaged custom filters | Yes, package version 2 and `plugin.processImage`; no access to another installation/global filter |
| Filesystem/network/process access from WASM | No imports/grants for these |
| Independent native image processor | Yes, package version 2 and the generic PNG/job contract |
| Owned native maintenance tasks | Yes, `plugin.runTask`; other native capability families require host work |
| Contributed commands, layer snapshots and atomic rename/visibility batches | PhotoCraft Plugin API; bounded single-document batches with one Undo step |
| Native read-only inspector and document invalidations | PhotoCraft Plugin API; one explicitly opened inspector/subscription per plugin |
| Free-text/dynamic/custom controls, docking, overlays | Host UI/schema work required |
| Startup subscriptions or general async callbacks | Not exposed; the inspector invalidation/query loop is a limited separate contract |
| Independent data-contract crate and conformance/build tool | Experimental `photocraft-plugin-api`; [Rust developer tool](../apps/photocraft-plugin/README.md), with independently exported starter builds |
| Typed guest SDK and scaffolding CLI | Available; privately included sources, one authoring input and generated bindings |
| Plugin release updates | Optional public GitHub Releases; reviewed installation through the existing store |
| Marketplace and hot reload | Planned marketplace; no watcher/hot reload |
| Raw guests on desktop/browser engine | Runtime supported; browser installation/persistence UI differs |
| Native platform targets | Windows/Linux/macOS x86-64/ARM64 declared; only Windows x64 verified; no browser packages |
| Photoshop UXP, CEP or .8BF compatibility | Port to a PhotoCraft API |

This is an initial API rather than a mature Photoshop-scale SDK. Starters remove ABI boilerplate
for supported projects, including independent image processing; integrations outside these APIs still need host changes. Design within the current
capabilities first, then propose a focused addition if needed.

## Host architecture and new capabilities

Independent authors normally work in their plugin repository. Host contributors work across
PhotoCraft's existing layers:

| Source area | Responsibility |
|---|---|
| `crates/plugin-api` and `schemas/plugins` | Independent serialized contracts, capability negotiation and generated schemas |
| `crates/engine/src/plugin_host` | Host-created identity, scoped handles, command effects, dynamic commands, atomic edits and deferred inspector events |
| `crates/plugins/src/extension.rs` and `extension/runtime.rs` | Manifest/response validation, panel ABI and budgets |
| `crates/plugins/src/lib.rs` and `manifest.rs` | Filter ABI, execution limits and manifests |
| `crates/engine/src/extension_cmds.rs` | Registry, raw guest persistence, grants, dispatch and state |
| `crates/engine/src/extension_package.rs` | Native ZIP validation, generations and integrity |
| `crates/engine/src/plugin_cmds.rs` | Filter installation/application |
| `crates/engine/src/plugin_preview.rs` | Staged results, lifecycle checks, Apply/Discard |
| `crates/ui-egui/src/extension_ui.rs` and submodules | Manager, native panels, comparison |
| `apps/photocraft/src/main.rs` | Attaches desktop settings plugin store |
| `crates/ui-egui/src/menus.rs`, `native_menu.rs`, `menu_keys.rs` | Menu, native and keyboard routing |
| `crates/automation/src/workspace.rs` | Restricted automation file, management and native authority |

New editor commands belong in the engine with typed validation, preconditions, history semantics
and tests. Register them, then review panel allowlisting. Adding an id does not implement a
missing command. New field types need schema/value validation, native rendering, persistence
and tests. Native image algorithms use the generic processor contract. Capabilities outside
that contract need host support, owning-installation binding, bounded
requests/results, source validation, jobs/cancellation and automation authorization.

Use [architecture](architecture.md), [contribution rules](contributing.md) and the
[control protocol](control-protocol.md) for host contributions. Keep the framework in the editor
repository and your plugin separate.

## Troubleshooting

| Symptom | Next check |
|---|---|
| Missing manager/menu | Verify that the running host contains this API |
| Missing pce_* / wrong ABI | Wrong target or a filter installed as panel |
| Missing pc_* | A panel was sent through the filter installer |
| Forbidden imports | Remove WASI, OS/network/random dependencies or JS bindings |
| Unsupported WASM instruction | Avoid forced SIMD/experimental target features |
| Unsupported permission at install | Compare with this host's extension.commands |
| Ungranted command at run | Declare exact id, rebuild and replace |
| Permitted action still fails | Check params, locks, targets, selection, modes and preconditions |
| Old panel controls | Embedded manifest needs rebuild/replacement; reopen panel |
| State disappeared | Return explicitly; replacement/reload/restart/remove reset it |
| Counter differs from layer count | State is not tied to Undo or external edits |
| Memory/fuel/JSON error | Reduce allocation/loops/state/response/commands; check per-ABI limits |
| Earlier edit survives later error | Panel batches are sequential, not atomic |
| Custom filter unavailable to panel | Bundle it in a version-2 package and select its manifest id through plugin.processImage; global plugin.run is not granted |
| Preview layout but no result | A supported service must stage a result |
| Native update/remove is busy | Wait for worker/cancel drain |
| Native package fails on another OS | Match the version-2 declared target, executable and dependencies; rebuild old fixed-service packages as version 2 |

A useful bug report includes id/version, host build/platform, reproducible steps, exact error,
and a minimal non-private source/artifact. Say whether failure happened during build, inspection,
install, execution or Apply, and which target you expected.

## Reference documents

- [Panel API](extensions.md): manifest, ABI, messages, limits and management.
- [Panel commands](plugin-command-reference.md): supported ids and parameter descriptions.
- [Testing and debugging](plugin-testing.md): actual module checks, CLI smoke and CI workflow.
- [Filter ABI](plugins.md): formats, geometry, selection, overlap and budgets.
- [Native packages](plugin-native-packages.md): version-2 schema, migration and ZIP/install contract.
- [Independent image processing](plugin-image-processing.md): own native/WASM backends and the neutral starter.
- [Native tasks](plugin-native-tasks.md): document-independent operations in an owned worker, progress, cancellation and bounded JSON results.
- [Safety and recovery](plugin-safety.md): validation, sandbox boundaries, recovery launch and repair.
- [Installation and updates](plugin-updates.md): optional per-plugin GitHub Releases, review and maintenance.
- [Marketplace plan](plugin-marketplace.md): future catalog, publisher trust and release gates.
- [Panel starter](../examples/extensions/plugin-starter/README.md): complete source/tests.
- [Filter starter](../examples/plugins/filter-starter/README.md): complete source/tests.
- [Independent image starter](../examples/extensions/image-plugin-starter/README.md): own native engine and packaged filter, complete build/package scripts.
