# PhotoCraft plugin panels

PhotoCraft extensions are installable tools with native panels and sandboxed WebAssembly logic.
They appear in the top-level **Plugins** menu and their panels execute inside PhotoCraft.
This is PhotoCraft's own API; Photoshop UXP, CEP and `.8BF` packages need to be ported.
The existing [filter plug-in ABI](plugins.md) remains available independently.

Start with the [plugin development guide](plugin-development.md) for the architecture, core
integration map, independent neutral starters, testing, releases and capability limits.
This page is the API reference.

This page documents panels using the [PhotoCraft Plugin API](plugin-api.md), the single
supported public authoring API. It includes namespaced commands, bounded layer queries,
atomic edits and native read-only inspectors. See the independent [Layer Organizer](../examples/extensions/layer-organizer/)
and [Document Inspector](../examples/extensions/document-inspector/) examples.

## Install and use an extension

1. Open **Plugins › Manage Plugins…** or **Plugins › Install Plug-in…**. Both open the same installer.
2. Click **Browse…** beside the path field, drop one plugin file into the window, or enter a
   path and select **Show permissions**. Choose a `.pcplugin` package or a `.pcext`/`.wasm` panel,
   review its author and requested access, and click **Install plugin**. A newer version of the
   same plugin offers **Update plugin**. Same-version reinstall/repair and older-version
   installation are under **Advanced**; a damaged installation offers **Repair plugin**.
   A desktop `.pcplugin` package includes its processing engine and assets; installation manages
   them automatically. No separate companion application or worker connection is needed.
3. Open the installed panel from **Plugins**, or click **Open** in the manager.
4. Use the manager's checkbox to enable/disable an extension, or **Remove** to uninstall it.
5. **Done** closes the manager or plugin panel. Closing a comparison-preview panel also
   cancels its processing and discards the unapplied comparison. Other compact panels can
   continue a supported background job after closing; use **Cancel processing** or the job's
   **Cancel** control to stop it.

Browsing and dropping inspect the file automatically; installation always requires confirmation.
After success the selected file clears and a status message reports the result. **Check for
updates** discovers new releases for plugins that declare an optional public GitHub release
source; an available update is prepared for review before installation. See [updates](plugin-updates.md).
The installer rejects multi-file drops and unsupported files. Plugin packages dropped over
an image are routed to review without placing a layer or committing an active transform.
Ordinary image drops outside the manager keep their normal open/place behavior. Standalone
`.wasm` pixel filters use a separate **Install Filter** confirmation after ABI validation;
they remain per-process installations through `plugin.install`.

Desktop installations are copied into `Extensions/` inside PhotoCraft's settings directory
(including portable mode). Enable/disable survives restarts. A bad installation is skipped at
startup and reported in the manager. A session without an attached settings directory keeps
raw WASM installations in memory; this includes the headless CLI and current browser integration.
Native `.pcplugin` installation requires a persistent store and is unavailable in the browser.
For recovery, the updated desktop supports `--disable-plugins`; see
[safety and recovery](plugin-safety.md) for exactly what it skips and how to repair damaged records.
Ordinary button handlers (`pce_handle`) do not run on installation, startup or every frame.
Inspectors activate on explicit opening and receive coalesced document invalidations.
Validation does instantiate sandboxed code and call its version/manifest exports; any WASM
start function runs within the same resource limits.

A basic extension is a single core WebAssembly binary, conventionally named `.pcext`, with its
manifest embedded. A desktop `.pcplugin` package bundles that guest with optional native
processing, packaged WASM filters and assets. Panels are native egui controls; neither format uses HTML,
JavaScript or a webview. Native package engines run in a managed background process.

## Create your first extension

Use the independent [Hello PhotoCraft starter](../examples/extensions/plugin-starter/README.md)
and follow the [developer handbook](plugin-development.md#build-a-first-panel-plugin). Its source
contains safe bounded ABI storage, a manifest, a pure handler, tests and Windows/POSIX build scripts.
It has no dependency on the editor crates or an existing product plugin.

```sh
rustup target add wasm32-unknown-unknown
cd examples/extensions/plugin-starter
cargo build --locked --release --target wasm32-unknown-unknown
```

Run `build.ps1` on Windows or `sh build.sh` on macOS/Linux to create `Hello-PhotoCraft.pcext`,
then install through Manage Plugins. The manifest is embedded; rebuild after every change.
Any language producing core WASM without imports can implement the ABI. The starter uses Rust's
required `unsafe(no_mangle)` export attributes, without unsafe memory-access blocks.

## Desktop plugin packages

A version-2 `.pcplugin` is a ZIP containing the panel guest and optional independent native
image worker and/or bundled WASM filters. A panel-only package is also valid. Declare
`plugin.processImage` to process pixels using only that installation's backends. No host
service registration or editor rebuild is needed for algorithms fitting its generic image
contract. See [independent image processing](plugin-image-processing.md) for the complete
tutorial and neutral Image Gain starter.

Native packages can also grant [`plugin.runTask`](plugin-native-tasks.md) to run their own
maintenance operations without an image open. Tasks show progress/cancellation and return
a bounded status message/data object; they do not create previews or edit documents.

Packages require a persistent desktop store. A native backend requires explicit native-code
consent and a matching declared platform; accepted targets are Windows/Linux/macOS x86-64 and
ARM64, with only Windows x64 verified here. Native code runs outside the WASM sandbox.

The [native package author reference](plugin-native-packages.md) specifies the complete schema,
schemas, required files, ZIP rules, limits, BLAKE3 integrity records, update/repair behavior
and worker protocol. Native packages must be provided by a filesystem `path`; raw guest `data`
accepts base64 WASM only. Inspect/install do not launch a native worker. New capabilities
outside generic image processing require explicit host implementation and API review.

## Manifest

```json
{
  "apiVersion": 1,
  "id": "org.example.tools",
  "name": "My Tools",
  "version": "1.0.0",
  "author": "Extension author",
  "description": "Useful tools for the active image.",
  "permissions": {
    "documentRead": true,
    "commands": ["image.adjustments.invert"]
  },
  "panel": {
    "fields": [
      {"id": "amount", "label": "Amount", "type": "int", "min": 1, "max": 100, "default": 50}
    ],
    "buttons": [{"id": "apply", "label": "Apply"}]
  }
}
```

Ids are lowercase ASCII, start with a letter/digit and use only letters, digits, dots, underscores
and hyphens (at most 64 characters). Use a reverse-DNS id you control. A panel has at most 32
fields and at most 16 buttons; at least one button or a contributed inspector is required.
Field and button ids are unique within their respective lists, use ASCII
letters/digits/underscores, and may not start with an underscore. Field ids also follow the filter
parameter rules (`id` and `layer` are reserved).

Exact manifest metadata bounds:

| Field | Validation |
|---|---|
| `apiVersion` | Required internal wire discriminator, exactly `1` |
| `id` | Required; rules above; 1–64 ASCII bytes |
| `name` | Required; trimmed form is nonempty and at most 64 Unicode characters; rejects controls/most invisible formatting characters |
| `version` | Required string; at most 32 UTF-8 bytes. Updates use semantic ordering; incomparable local labels require deliberate replacement. |
| `author` | Optional string, default empty; at most 128 UTF-8 bytes |
| `description` | Optional string, default empty; at most 1,024 UTF-8 bytes |
| `repository` | Optional canonical `https://github.com/owner/repo`; at most 160 ASCII bytes |
| `updates` | Optional `{provider:"github",asset:"exact-{platform}.pcplugin"}`; requires repository; see [updates](plugin-updates.md) |
| `permissions` | Required object; `documentRead` defaults false, `commands` defaults empty |
| `permissions.commands` | At most 128 unique ids; each 1–128 ASCII bytes using letters/digits/dots/underscores/hyphens; host must allow them |
| `panel` | Required object; `fields` defaults empty; `preview` defaults false; `buttons` required |
| Field/button ids | 1–64 ASCII bytes; rules above; uniqueness is per list |
| Field/button labels | Nonblank trimmed form, at most 128 Unicode characters, no controls |

Unknown manifest, permissions, panel, button and parameter-specification properties are rejected.
The manifest's exported UTF-8 byte length must fit 64 KiB.

Supported field types are `number` (floating point slider), `int` (integer slider), `bool`
(checkbox), and `choice` (dropdown), using the [filter parameter schema](plugins.md#manifest).
Values are validated, missing values receive defaults, numeric values are clamped, and unknown
fields/wrong types are rejected. Saved panel values are revalidated when an updated panel opens.

A panel may opt in to the host's native before/after comparison layout with `"preview": true`.
The optional boolean defaults to false, preserving existing compact panels. This contribution
selects a host-rendered image preview; it does not expose arbitrary layouts or image buffers to
WASM. A supported image command must produce a session preview before it can be applied.
For example, `"panel": {"preview": true, "fields": [...], "buttons": [...]}` selects the layout;
the guest separately requests `"preview": true` in `plugin.processImage` or another supported
image command's parameters (`plugin.processImage` defaults it to true).
The host supplies Apply, Discard, output destination and comparison controls. They are not guest
button events and do not call `pce_handle`.

`documentRead` supplies the active document's id, name, pixel dimensions, color mode, bit depth,
revision and active layer id. It does not supply file paths or pixels. Without this permission,
the request's `document` is `null`. A `plugin.processImage` command grant separately authorizes
visible pixels through the owning image backend; it does not require summary metadata access.

Every requested editing command must be declared explicitly. Call **`extension.commands`** to
discover this host's supported command ids; `command.list` supplies their parameter descriptions.
The allowlist intentionally excludes file access, preferences, script/action playback, plug-in
management, extension management and commands accepting ambient resource paths. Unknown or
unsupported permissions reject installation. Extending the allowlist is a host API review.

## Guest ABI

All pointers are 32-bit offsets into the exported `memory`. No WASI or host imports are accepted.

| Export | Signature | Contract |
|---|---|---|
| `memory` | memory | Guest linear memory |
| `pce_api_version` | `() -> i32` | Returns the internal wire discriminator, `1` |
| `pce_manifest` | `() -> i64` | UTF-8 manifest JSON packed as `(length << 32) \| pointer` |
| `pce_alloc` | `(length: i32) -> i32` | Returns writable request storage; `0` means allocation failure |
| `pce_handle` | `(pointer: i32, length: i32) -> i64` | Returns response JSON in the same packed form |

For each button click the host creates a fresh instance and calls `pce_handle` with:

```json
{
  "apiVersion": 1,
  "host": {"version": 1, "capabilities": []},
  "event": "apply",
  "values": {"amount": 50},
  "state": null,
  "document": {"id": 1, "name": "Untitled", "width": 1920, "height": 1080,
    "depth": 8, "mode": "Rgb", "revision": 0, "activeLayer": 1}
}
```

The abbreviated `host.capabilities` list above is populated by the running host; the envelope
may also include negotiated compatibility metadata and correlated broker `responses`.
`document` is null without permission or an active document. State is null on the first event.
Return:

```json
{
  "state": {"runs": 1},
  "message": "Colors inverted.",
  "commands": [{"command": "image.adjustments.invert", "params": {}}]
}
```

Responses may alternatively carry broker `requests` and a native inspector `view`, as described
in the [API reference](plugin-api.md). Command invocations and broker requests cannot be mixed.
All top-level response fields are optional. Each command's `command` and `params` are required;
`params` must be an object, including `{}` for an action without parameters. Unknown response and
invocation keys are rejected. The host validates the complete
command list against the manifest and the session's authorization gate before the first edit.
It then runs commands sequentially through the normal engine dispatcher. Each command retains
its usual undo behavior; a multi-command action is **not one atomic transaction**. If a command
fails, execution stops, earlier successful edits remain undoable, and the error identifies the
failed step. Guest state commits only after every command succeeds. Updates reset guest state.
State lives for this session; panel visibility and field values use the editor's saved UI state.
State belongs to the installed plugin, shared across documents. Omitting `state` replaces it with
null after a successful action; return the existing state explicitly when it should be preserved.

Handlers are event-driven and short. Ordinary command sequences execute synchronously, like a
script. The desktop UI runs `plugin.processImage`
as a background job with progress and cancellation. It must be the only command in its response. Guest state commits
after the result opens, or after Apply when the command requested a preview. Discarding
a preview never commits its proposed guest state. Rust callers use `Session::start` for asynchronous dispatch;
`Session::execute` runs these operations synchronously.
General asynchronous callbacks, canvas overlays, custom docking, text/image controls,
network/file capabilities, additional update providers and a marketplace remain future additions.
The manager already supports [opt-in GitHub release updates](plugin-updates.md).
Use the existing filter ABI for custom pixel filters; version-2 packages can bind their own
filter modules to their panel through `plugin.processImage`.

## Independent packaged image processors

See [the generic processing reference and tutorial](plugin-image-processing.md) for version-2
package metadata, owner binding, command parameters, native JSON/PNG transport, bundled filter
selection, preview transactions and the independent source starter. Both generic backends
receive a host-flattened 8-bit RGBA sRGB composite from supported 8/16-bit RGB/grayscale sources.
Native workers may return a requested size; bundled WASM backends retain source dimensions.

## Shared preview lifecycle

See [image processing and preview lifecycle](plugin-image-services.md) for the host/plugin
responsibility boundary, comparison, Apply/Discard, layer history and stale-result checks.
Algorithm settings and dependencies belong to the installed plugin, including the AI plugins.

## Resource and failure limits

| Resource | Limit |
|---|---|
| Extension binary | 8 MiB |
| Manifest | 64 KiB |
| Linear memory | 32 MiB |
| Call depth | 256 |
| Instructions | 5 million fuel per instance, shared by startup and exported calls |
| Event request / response JSON | 256 KiB each |
| Serialized session state | 128 KiB; rejected before commands to retain request headroom |
| Commands per response | 32 |
| Status message | 4 KiB |
| Installed extensions | 4096 |

Malformed modules, unsupported versions, imports, traps, infinite loops, invalid pointers,
oversized JSON, bad parameters and ungranted commands return errors. A runtime failure before
command execution leaves the document untouched. Installation validation completes before the
existing installation is replaced. The manager pins reviewed content by its BLAKE3 hash so a
file changed after review must be reviewed again. Engine installation also remains available
to trusted local callers; the human review is a manager workflow, not a signature system.

## Engine / automation API

| Command | Parameters | Purpose |
|---|---|---|
| `extension.commands` | `{}` | Supported command ids and API version |
| `extension.inspect` | `{"path":"…"}` or `{"data":"base64"}` | Validate and return manifest, content hash, selected file `downloadBytes` and `storage` sizes |
| `extension.install` | Same, plus `replace: false`, optional `expectedHash`, `allowNative: true` for native packages | Install; explicit replacement for updates and native-code consent |
| `extension.list` | `{}` | Installed manifests, enabled flags, `storage` sizes and startup errors |
| `extension.invoke` | `{"id":"…","event":"apply","values":{}}` | Run one declared button event |
| `extension.enable` | `{"id":"…","enabled":true}` | Enable/disable |
| `extension.remove` | `{"id":"…"}` | Uninstall |
| `plugin.processImage` | `processor: "native"` or own packaged filter id, `options: {}`, optional `width`, `height`, `preview: true` | General image job bound to the invoking installation; direct trusted callers also provide its `id` |
| `image.pluginPreview.apply` | `{"preview":1,"target":"document"}` or `target: "layer"`; target defaults to `document` | Admit a validated staged result; same-size layer output is undoable |
| `image.pluginPreview.discard` | `{"preview":1}` | Release a staged comparison without editing the source |

Untrusted automation cannot run native plugin engines, apply/discard their previews, install/remove/enable persistent extensions or read extension files
by ambient path. In-memory inspection and installed actions remain available, with each proposed
editing command checked by the caller's existing authorization hook. `window.extensions` opens
the manager; `window.extension.<id>` toggles an installed panel through `ui.menu.invoke`.
