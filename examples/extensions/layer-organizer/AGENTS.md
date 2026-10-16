# Layer Organizer project

This guide applies to this project and to standalone copies. Read [README.md](README.md),
`manifest.json`, `package.json`, `src/handler.rs`, and `build.py` first.

## Project ownership

Retain the independent Cargo workspace, lockfile, bounded `src/abi.rs`, lint/format
settings, build/packaging helpers, and licenses. Use published/pinned dependencies
or local dependencies contained in this project; do not link to PhotoCraft's
engine/UI crates, an editor checkout, or another plugin. Algorithms and naming
policy belong here. The current example needs no native worker or models.

For a derivative, change metadata and use an ID you control. Update every contributed
command to the new `pluginId.localId` namespace and keep handler event IDs aligned.
Rebuild embedded manifests and keep the stable ID for updates to an existing plugin.

## Experimental API-2 contract

This is panel API 2, package format 2, and import-free core WASM with `pce_*` exports.
It requests `host.document.read` and `host.document.edit`; it does not need legacy
`documentRead` summary access or broad engine command grants. Capability availability
is not authority. Older panel-1-only hosts reject it; package 3 is not implemented.

Read snapshots through `documents.layers.read` and consume the `host_response`
continuation. Treat document/layer handles as opaque, scoped, revision-bound values.
Do not substitute engine layer IDs or manufacture handles. The current example
refuses a paginated/incomplete snapshot and limits edits to 128 layers. Supporting
larger documents requires an intentional paging/batching design and honest Undo semantics.

Return `documents.layers.edit` as the sole broker request for an explicit action,
with the snapshot's document handle, `expectedRevision`, and distinct layer handles.
Only rename/visibility changes are supported by this broker. A successful batch is
one undo step; a stale revision, bad handle, lock, or validation failure changes no
document/history/proposed state. A successful edit is terminal, not a new continuation.
Responses may use broker requests or legacy commands, not both. Fresh WASM instances
use host-carried JSON state; globals do not persist. Report missing host capabilities
instead of extending the editor for this example or silently changing its contract.

## Build and verify

Use Rust 1.95+, the WASM target and native test linker, plus Python 3.11+. Run from here:

```sh
python build.py --check
```

Add `--offline` after crates are cached. This runs formatting, unit tests, Clippy,
release WASM compilation, and packaging with dependency licenses. Current output is
`dist/Layer-Organizer-1.0.0.pcplugin`; identity/version changes affect its name.
The optional `photocraft-plugin` tool can audit source and validate the package.

Cover scope/selection/defaults, malformed or incomplete snapshots, host errors,
opaque handles, revision propagation, and exactly one intended edit request. Real
host acceptance must also check namespaced command discovery, user shortcuts where
configured, stale revisions, locked layers, and atomic Undo/Redo on scratch documents.
Current restricted MCP denies installation; an authorized local test route is needed
for that step. A passing guest test or package check is not real-host acceptance.
Report artifact/source paths, required API-2 host, checks, and remaining limitations.
