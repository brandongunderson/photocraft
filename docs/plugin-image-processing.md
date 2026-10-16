# Independent image-processing plugins

Build an image processor in its own repository, package its implementation with a panel, and
install the result without modifying or rebuilding PhotoCraft. Package version 2 provides
**`plugin.processImage`**, a general host capability for an installation's own native worker
and/or bundled WASM filters. It does not depend on PhotoCraft's existing AI services.

Start with [Image Gain](../examples/extensions/image-plugin-starter/README.md), a complete
neutral project containing both backend types, tests, lockfiles, license collection and
Windows/POSIX build scripts. The [main handbook](plugin-development.md) covers panel controls,
state, permissions and the separate standalone filter ABI.

## Choose a backend

| Backend | Use it for | Boundary |
|---|---|---|
| Packaged WASM filter | Pixel algorithms that fit the existing filter ABI | Import-free sandbox, fixed source dimensions, bounded execution; host applies source selection coverage |
| Native worker | Your own library/runtime, large models or native algorithms | Separate OS process with the user's privileges; explicit native-code install consent; cooperative cancellation |
| Both | One panel offering alternative implementations | Each selection resolves only inside its owning installation |

This is a processing API, not an arbitrary process-launch command. The guest supplies options
and an installed processor name; it cannot supply executable paths, shell commands, job paths
or another plugin's id. Host-owned paths and validated output protect the document transaction;
they do not sandbox native code's filesystem or network access.

## Copy, build, install

Copy `examples/extensions/image-plugin-starter/` into a new directory or extract it from the
developer kit. It has independent workspaces and no host path dependencies. Install Rust 1.95+
with a native linker, Python 3.11+, then add the guest target:

```sh
rustup target add wasm32-unknown-unknown
```

On Windows, run `./build.ps1 -Check`; on macOS/Linux run `sh build.sh --check`. This builds the
WASM panel/filter and a native worker for the current machine, collects dependency licenses,
and writes `dist/Image-Gain-<platform>.pcplugin`. Offline switches use cached dependencies.
The script's Python and `CARGO_TARGET_DIR` options are documented in the starter README.
Windows x64 has been exercised here; acceptance of another platform id is not a claim that
its executable, libraries or graphics runtime have been tested.

Install through **Plugins → Manage Plugins…** in a build containing package version 2 and
`plugin.processImage`. Review the requested access and native-code warning. Open Image Gain,
generate a preview and apply/discard it. Try both backends before changing the example.

Most authors then edit the manifest, `src/handler.rs` and the chosen backend. Change ids,
metadata, licenses and module filenames before publishing. Your processor implementation can
use any language that produces the specified WASM ABI or a compatible native executable;
Rust is used by the provided starter and by the editor.

## Package version 2

A `.pcplugin` is a ZIP with these root files:

```text
package.json
plugin.pcext
LICENSE
THIRD-PARTY-NOTICES.md
image-worker.exe           # example native entry, if native is declared
filters/adjust.wasm        # example bundled filter, if filters is declared
```

Example metadata, for a Windows x64 package with both backends:

```json
{
  "packageVersion": 2,
  "module": "plugin.pcext",
  "native": {
    "service": "plugin.processImage",
    "platform": "windows-x86_64",
    "entry": "image-worker.exe"
  },
  "filters": [{"module": "filters/adjust.wasm"}]
}
```

`module` is required and exactly `plugin.pcext`; the panel's embedded manifest owns the plugin
identity. `native` and `filters` are optional. A panel-only package, a panel with only filters,
a panel with only a native backend, and a panel with both are supported. There is always a
panel module; a worker does not execute on install or startup. Unknown metadata fields are
rejected. There are no install scripts, implicit dependency downloads or shell hooks.

For `native`, `service` is exactly `plugin.processImage`. `entry` is a safe relative executable
path within the archive. The host must match the declared platform: `windows`, `linux` or
`macos`, followed by `-x86_64` or `-aarch64`. Ship the executable and libraries appropriate to
that OS/architecture; Windows entry names must end in `.exe`. Native code consent is required only when a native backend is present.
Package installation requires a persistent desktop store; it is unavailable in the browser.

There may be at most 16 filter entries, each naming a valid filter ABI module. The embedded
filter manifest id selects the processor; it is not the module filename. Ids must be unique
inside the package, and `native` is reserved for the native backend. Packaged filters are not
installed into the global filter registry. Another installation may use the same filter id
without allowing this panel to execute it.

The [package reference](plugin-native-packages.md) defines path, count, size, integrity and
update rules.

## Panel grant and request

Declare `plugin.processImage` in `permissions.commands`. This grant authorizes access to the
visible composite pixels through the processing backend; `documentRead` is a separate summary
metadata grant. Set `panel.preview` to `true` for the host comparison layout, and normally
request `documentRead` for friendly empty-document checks.
Return exactly one image-processing invocation from a button handler:

```json
{
  "state": null,
  "message": "Generating preview.",
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

| Parameter | Contract |
|---|---|
| `processor` | Optional; `native` or an embedded bundled filter manifest id in this installation. Omission chooses native when present, otherwise the sole bundled filter; ambiguous/no backend fails |
| `options` | Object, default `{}`, at most 128 KiB serialized; passed to the native worker, or validated/resolved using the chosen filter's parameter manifest |
| `width`, `height` | Optional positive output dimensions for native processing; omitted dimensions use source dimensions |
| `preview` | Optional boolean, default `true`; `false` opens a new result document directly |

Bundled WASM processing retains source dimensions; a conflicting requested size is rejected.
For a panel request the engine binds the owning installation automatically. Do not include
`id`, paths or launch arguments. A trusted direct CLI/engine invocation instead supplies an
explicit installed plugin `id` to select the owner. Restricted automation cannot launch native
plugin processing or bypass management authority. See [testing](plugin-testing.md).

Options for native code are your own versioned contract: validate their types, ranges and
unknown keys in the worker. A valid package does not prove that its options or algorithm work.
For filters, PhotoCraft applies the existing parameter schema and resource limits; the host's
band metadata is available through the normal `_image` convention.

## Pixels, dimensions and results

Both generic backends receive the **flattened visible composite as 8-bit RGBA sRGB**. The host
converts supported 8/16-bit RGB or grayscale source documents through its color-management
pipeline first. This boundary is intentionally different from the standalone filter API,
which retains its ordinary 8/16/32-bit target behavior. Neither generic backend receives the
editable layer tree or an arbitrary document filesystem path.

A native job processes the complete composite; selection coverage is not supplied as a mask.
For a bundled WASM backend, the host blends its result through the source selection coverage.
Native authors needing a mask-specific workflow require an additional host API; do not infer
selection pixels from the document summary.

The generic path caps source images at 100 million pixels, results at 256 million pixels,
and either image at 32,768 pixels per side. The working-set estimate is
`32 × (source pixels + output pixels) + 128 MiB`, capped at 8 GiB on 64-bit hosts and
2 GiB on 32-bit hosts. The memory bound can be reached before the pixel limit.
Workers must enforce any stricter algorithm-specific limits. These are per-operation bounds, not
a guarantee that enough system RAM/GPU memory is available. Input and expected output are
both bounded. Larger native output must match the requested dimensions exactly.

Results are flattened 8-bit sRGB. A preview leaves the source document untouched until
Apply. The comparison panel provides the divider, Apply, Discard and output destination.
Apply opens a new result tab; at matching dimensions an undoable new layer is also available.
Closing a comparison cancels processing and discards unapplied results. Document revision,
plugin ownership/enabled state and normal modal/editor preconditions are checked before Apply.
Stale, cancelled, malformed or discarded results do not commit proposed guest state.

## Native worker protocol

PhotoCraft starts the installed executable directly, without a shell:

```text
<installed-entry> --job <absolute-path-to-job.json>
```

The working directory is the installed executable's **containing directory**, which can be a
subdirectory of the package. Resolve packaged assets relative to that directory and your own
declared layout; it is not always the package root. The host creates job paths; the panel cannot choose them. Generic jobs use:

```json
{
  "apiVersion": 1,
  "input": "C:/temporary-job/source.png",
  "output": "C:/temporary-job/result.png",
  "options": {"gain": 1.25},
  "outputSize": {"width": 1920, "height": 1080},
  "colorSpace": "srgb"
}
```

Validate the request version. Read `input` as a single 8-bit RGBA sRGB PNG, process pixels,
and write a complete regular PNG at `output`, exactly matching `outputSize`. The host accepts
**8-bit RGB or RGBA PNG**; use **RGBA sRGB** to retain/compute alpha deliberately. An RGB result
is opaque. Do not label another color space as sRGB.
The starter marks its output with the PNG sRGB chunk. Exit zero only after flushing/closing
the result. The host checks bounded output, PNG structure, dimensions and bit depth before
admitting it; installation never runs the worker to establish compatibility.

Write newline-delimited JSON on **stderr** for progress:

```json
{"progress": 50, "message": "Processing image"}
```

Progress is a percentage from 0 to 100. Non-JSON lines are diagnostic text. stdout is not a
result channel. Diagnostics are bounded, including a monitored 16 MiB log limit; see the
[transport reference](plugin-native-packages.md#native-worker-transport-boundary).

Listen to stdin while working. Stop promptly on `cancel\n` **or EOF**, clean up/reap children,
and exit nonzero for cancellation/failure. The host requests cancellation at 45 minutes and
allows 10 seconds before terminating the direct worker. This is not OS process-tree or disk
containment. A worker that launches another engine owns that engine's cancellation and cleanup.
Do not launch visible consoles or interactive child apps as part of processing.

## Package without editor dependencies

Prepare a directory containing your completed files, then run the standalone Python helper:

```sh
python scripts/plugins/package-plugin.py --root path/to/prepared --output My-Plugin.pcplugin
```

The starter carries its own copy at `tools/package-plugin.py`, so an extracted project does
not need this repository. The helper uses only Python's standard library. It checks metadata,
safe paths, required files, module magic and finite archive limits; it writes sorted entries
with fixed timestamps and permissions. Equal input bytes with the same Python/zlib build
produce equal ZIP bytes. Different compressors/tool versions may produce different bytes.
It never runs binaries or proves WASM ABI, embedded manifest ids, publisher identity, model
compatibility, licensing accuracy or output quality. Install/inspect the exact artifact in
PhotoCraft for host validation. Dependency licenses are the author's responsibility.

## Test and publish

Test backend logic, invalid options, memory/dimension bounds, alpha, cancellation and actual
file IO. Build release WASM/native code and install the exact package through the manager.
Exercise both processors, previews, Apply/Discard, a same-size new layer and Undo/Redo,
changed-source rejection, disable/remove/repair, restart and offline operation. Verify the
correct executable/libraries on every platform you publish. Record package hashes and the
compatible PhotoCraft source/API version.

Keep generated models, build caches and distribution archives out of the source repository
unless your distribution policy intentionally includes them. Include pinned downloads and
license notices for dependencies you supply. Marketplace discovery and publisher signing
remain planned. Read [safety](plugin-safety.md) before publishing native code to users.
