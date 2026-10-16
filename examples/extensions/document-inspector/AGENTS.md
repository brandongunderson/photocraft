# Document Inspector project

This file applies to this project and standalone copies. Read [README.md](README.md),
the manifests, `src/handler.rs`, and `build.py` before changing the inspector.

## Project ownership

Keep the independent Cargo workspace, lockfile, `src/abi.rs` bounds, lint/format
files, build helpers, and licenses. Do not add dependencies on editor crates,
another plugin, or an inherited editor workspace. Published/pinned dependencies
and local code within this project are permitted. This inspector needs no native
worker, shared runtime, or models.

When creating a derivative, change the embedded identity/authorship/version and
rename inspector/subscription IDs into the new plugin namespace. Keep handler event
names and matching subscription handlers consistent. Rebuild after manifest changes.

## Read-only API-2 behavior

This guest uses experimental panel API 2 and existing package format 2. Its required
capabilities are `host.document.read`, `host.events`, and `host.ui.views`; it needs
no document-edit capability or legacy command grants. Core WASM remains import-free,
with `pce_*` exports and fresh instances using host-carried JSON state. Package 3,
custom layouts, and general asynchronous plugin instances are not current contracts.

Opening the inspector activates the document-change subscription; closing/disabling/
removing it ends that subscription. The host coalesces notifications. Render bounded
label/value rows returned by the handler; do not put plugin logic in frame painting
or add a polling loop that bypasses the event lifecycle.

Read through `documents.layers.read` and handle `host_response`. Treat handles as
opaque; do not expose them in displayed rows or replace them with engine IDs.
The current host supports one inspector and one matching subscription per plugin.
Read-only observer handlers cannot return edits, engine commands, or native jobs.

Snapshots are paginated, with a maximum page size of 128. Label partial totals such
as visible/selected counts as applying to the snapshot when only one page is read.
Keep the explicit total distinct from that page. Clear old information when a document
closes or a read fails. Bound and sanitize user-provided names for native view output;
do not silently truncate the snapshot while presenting it as a complete document.

## Build and verify

Use Rust 1.95+, the WASM target and native test linker, plus Python 3.11+. From here:

```sh
python build.py --check
```

Add `--offline` after dependencies are cached. This runs formatting, tests, Clippy,
release WASM compilation, and packaging with dependency licenses. Current output is
`dist/Document-Inspector-1.0.0.pcplugin`; identity/version changes affect its name.
The optional conformance executable can audit source and check the package.

Test metadata formatting, empty/closed documents, errors, printable bounded names,
page truncation, and the absence of editing requests or leaked handles. In a compatible
desktop host, use scratch documents to verify updates after edits, selection, Undo/Redo,
and document switching; closing/disabling/replacing the plugin must stop stale updates.
Guest tests and screenshots alone do not establish subscription cleanup or host isolation.
Restricted MCP denies installation; use an authorized local test route or report it
untested. Deliver source/package paths, API-2 requirements, checks, and limitations.
