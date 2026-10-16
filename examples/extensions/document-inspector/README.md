# Document Inspector

An independently built current API WASM plugin with a native, read-only inspector view.
Opening the panel activates its document-change subscription. PhotoCraft coalesces
changes and invokes the handler; the guest requests a bounded snapshot and returns
label/value rows for native rendering. No guest code runs just to paint a frame.
Closing, disabling or removing the plugin stops its subscription.

The plugin owns the displayed information. It requires only generic document-read,
event and native-view capabilities. There are no host, sibling-plugin, native
worker, service, model or shared-runtime dependencies.

## Build and verify

Agents: read the local [AGENTS.md](AGENTS.md) for read-only events, paging, and verification.

Install Rust 1.95+ with the `wasm32-unknown-unknown` target and Python 3.11+.
Copy this entire directory anywhere, then run `python build.py --check`.
Use `--offline` once the locked crates are cached. Outputs go into `dist/`.
The independent Cargo workspace, lock, WASM ABI scaffold and standard-library-only
packager are all included. The package contains dependency licenses.

Install `dist/Document-Inspector-1.0.0.pcplugin` using PhotoCraft's Plugins manager.
Open **Plugins > Document Inspector** and edit, rename, select, hide or undo a layer.
The inspector updates from document notifications. With more than 128 layers the
view explicitly identifies its first-page snapshot; the layer total remains visible.
Closing the image clears the previous document information.

The PhotoCraft Plugin API panel is carried by a self-contained package format 2 archive.

## Tests

Guest tests verify read-only requests, rendering data, no leaked handles, empty
documents, errors and bounded printable user-supplied names. Actual host integration
tests exercise the compiled WASM, event updates and subscription cleanup.
