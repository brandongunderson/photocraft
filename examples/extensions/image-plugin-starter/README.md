# Image Gain: a complete independent image plugin

This starter builds one installable `.pcplugin` with its own native executable, a sandboxed
WASM pixel processor, and a native PhotoCraft comparison panel. It has **no PhotoCraft crate
dependencies**. Copy the entire directory into a new repository; no editor source checkout,
existing AI plugin, model, companion app or service registration is required.

It uses the **PhotoCraft Plugin API**, **package format 2** and **`plugin.processImage`**.
The manifest/export discriminator is 2; the filter ABI and native worker transport remain 1.
The panel declares `host.commands.invoke` and parses the typed host event envelope before
returning its scoped command. Old panel API 1 modules are unsupported and must be rebuilt.
Native packages require a desktop persistent plugin store; browser installation is unsupported.

## Build

Agents: read the local [AGENTS.md](AGENTS.md) for the panel, worker, and bundled filter boundaries.

Install Rust 1.95+ with its normal native linker, Python 3.11+, and the WASM target:

```sh
rustup target add wasm32-unknown-unknown
```

Windows:

```powershell
.\build.ps1 -Check
```

macOS/Linux:

```sh
sh build.sh --check
```

The scripts build release modules, collect their dependency licenses and create
`dist/Image-Gain-<platform>.pcplugin`. `-Offline` / `--offline` uses cached dependencies only;
omit `-Check` / `--check` for a build without repeating tests and Clippy. Choose a Python
executable with `build.ps1 -Python C:\path\python.exe` or `PYTHON=/path/python3 sh build.sh`.
`CARGO_TARGET_DIR` is honored. Build natively on the target OS/architecture; these scripts do
not cross-compile the worker. Windows x64 is the verified configuration. Other accepted
platform identifiers require their own build and real-host testing.

## Install and try

1. In PhotoCraft, open **Plugins → Manage Plugins…**, browse to the generated package and review it.
2. Allow native code only if you trust this build, then install it. Installing does not run the worker.
3. Open a disposable 8- or 16-bit RGB/grayscale document and select **Plugins → Image Gain**.
4. Choose a Gain and **Native worker**, then **Generate preview**. Drag the comparison divider.
5. **Apply** opens a result tab. At matching dimensions, the host also offers an undoable new layer.
6. Repeat with **Sandboxed filter**. This backend is the packaged `org.example.channel-gain` module;
   it is not looked up in a global filter registry or another plugin installation.
7. Check Discard, cancellation, closing the panel, Undo/Redo for new layers, disable, restart and remove.

Changing the source while work is pending prevents an outdated result being applied. Source
layers stay intact. The native example can also resize to a host-requested `outputSize`, using
nearest-neighbor interpolation for clarity; it is a teaching implementation, not a photographic
resampling recommendation. The panel requests the original size by omitting `width` and `height`.

## Files to customize

| Path | Purpose |
|---|---|
| `manifest.json` | Plugin identity, native controls, command grant and comparison layout |
| `src/handler.rs` | Validate fields and request `plugin.processImage` |
| `src/abi.rs` | Bounded panel ABI storage; normally leave it unchanged |
| `native/src/main.rs` | Independent worker: JSON job, PNG processing, progress, cancellation |
| `filter/` | Independent filter ABI workspace, copied from the neutral Channel Gain starter |
| `package.json` | Version-2 metadata; build helper selects this machine's platform/entry suffix |
| `build.py`, `build.ps1`, `build.sh` | Build, license collection and staging |
| `tools/package-plugin.py` | Standalone deterministic ZIP helper; no editor dependencies |
| `tools/test_package.py` | Structural helper regressions |

Rename the panel id, displayed metadata, Cargo package names and corresponding module filenames
before distributing your plugin. If you rename the filter's manifest id, update the handler's
`processor` string too. The reserved processor alias `native` selects only this installation's
worker. Change the copied filter algorithm or remove it when using only a native backend.

For a WASM-only package, remove `native` from package metadata, remove Native worker from the
panel and handler, and adjust the build helper to omit the worker build/copy. For a native-only
package, remove `filters` and its panel option/build/copy. Both retain `plugin.pcext`; "native-only"
means one native image backend with a panel, not a plugin that starts a process on installation.

## Exact worker boundary

PhotoCraft launches the installed entry directly, with `--job <absolute-job-json-path>` and the
executable's containing directory as its working directory. This starter places the executable
at the package root; a subdirectory entry uses that subdirectory instead. The JSON contains:

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

Input is a host-flattened **8-bit RGBA sRGB PNG**, converted from supported 8/16-bit RGB/grayscale
documents. This example writes a regular **8-bit RGBA sRGB PNG** at `outputSize`. The host also
accepts 8-bit RGB PNG results, which are opaque; prefer RGBA to retain alpha. The native worker
sees the whole composite; a document selection is not passed as a separate mask. The WASM
backend uses the same normalized composite and the host applies source selection coverage.
The standalone filter ABI still supports its usual 8/16/32-bit targets when installed separately.

Write progress as JSON lines on stderr, for example `{"progress":50,"message":"Processing"}`.
Exit zero only after finishing the PNG; errors exit nonzero. The worker must stop on stdin
`cancel\n` **or EOF**, and stop/reap any children it created. The host supervises a 45-minute
deadline with a 10-second termination grace. Workers run with the user's OS privileges,
outside the WASM sandbox; installation review is not publisher certification or containment.

## Test separately

Each of `.`, `native/` and `filter/` is an independent Rust workspace. Run its unit tests and
Clippy, then build/install the exact packaged artifact. Helper tests are:

```sh
python tools/test_package.py
```

The native worker tests include actual PNG read/process/write, alpha, sRGB marking, requested
dimensions, cancellation and malformed jobs. Panel tests check scoped backend requests and
invalid values. Filter tests cover sample formats and failure behavior. These do not replace
the desktop acceptance steps above or establish compatibility on untested operating systems.

The general guide is `docs/plugin-image-processing.md` in the PhotoCraft source/developer kit.
The panel contract is [PhotoCraft Plugin API](../../../docs/plugin-api.md). The documentation
links refer to the source/developer kit; building this copied project requires no host checkout.
