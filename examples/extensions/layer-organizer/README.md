# Layer Organizer

An independently built current API WASM plugin. Its panel and contributed command number
and name all layers (or only selected layers), using one atomic host edit request.
PhotoCraft validates the complete request against a scoped document handle and
revision before applying it as one Undo step. This example handles up to 128 layers;
larger snapshots are rejected before requesting any edit.

The plugin owns its naming behavior. The host supplies generic document-read and
document-edit capabilities. There are no host, sibling-plugin, native worker, model,
service, or runtime dependencies.

## Build and verify

Agents: read the local [AGENTS.md](AGENTS.md) for scoped handles, atomic edits, and verification.

Install Rust 1.95+ with the `wasm32-unknown-unknown` target and Python 3.11+.
Copy this entire directory anywhere, then run `python build.py --check`.
Use `--offline` once the locked crates are cached. Outputs go into `dist/`.
The script compiles, tests, checks, packages and includes dependency licenses using
only this project's files and published crates.io dependencies. The independent
Cargo workspace and Cargo.lock deliberately do not inherit editor configuration.

Install `dist/Layer-Organizer-1.0.0.pcplugin` in PhotoCraft's Plugins manager. Open
the plugin panel to select a prefix/scope, or run **Plugins > Layer Organizer >
Organize layer names** with default settings. The command also appears in the
command palette and can receive a user shortcut. Undo reverses every renamed layer.

The current API panel is carried by the existing panel-only package format 2. Package
format 2 remains the container. Older hosts that support only panel API 1
reject this package without affecting existing plugins.

## Tests

Guest tests verify a multi-layer rename produces one edit request, selection logic,
opaque handles, revision propagation, malformed snapshots, paging refusal and errors.
Actual host integration tests cover authority, stale revisions, rollback and Undo.
No raw engine document or layer IDs are used in this plugin.
