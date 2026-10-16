# Quick Tools extension

A complete extension using the PhotoCraft Plugin API, with a native panel and WebAssembly event handler. Copy this
folder as a starter; change `manifest.json` and the `handle` function in `src/lib.rs`.

Install the Rust `wasm32-unknown-unknown` target, then run `./build.ps1` on Windows or
`sh build.sh` on macOS/Linux. Install the generated `quick-tools.pcext` through
**Plugins › Manage Plugins…** in a plugin-enabled PhotoCraft source build.

Quick Tools resizes the active document, adds a pixel layer, and inverts the active pixel layer.
Resize uses PhotoCraft's conventional resampling; it is not AI upscaling. Each edit supports
Undo. No external app or local control server is needed.

The guest runs without filesystem, network, native process or GPU access. See the complete
[API contract](../../../docs/plugin-api.md) for permissions, limits, state and exported functions.
The [development guide](../../../docs/plugin-development.md) covers the host architecture,
customizing this starter, rebuilding/replacing plugins and package-owned native workers.
