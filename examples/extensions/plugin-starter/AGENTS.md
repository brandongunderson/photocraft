# Hello PhotoCraft plugin project

This file applies to this directory and its descendants, including a copy exported
into a separate repository. Read [README.md](README.md) and the local build scripts.
PhotoCraft core contributor instructions are not required to build this standalone guest.

## Project boundary

- Keep the independent `[workspace]`, Cargo.lock, lint/format files, and licenses.
  Do not add dependencies on an editor checkout or sibling plugin. Local dependencies
  must stay within this project; published crates and pinned Git sources are valid.
- Put behavior in `src/handler.rs`, controls/grants in `manifest.json`, and module
  setup in `src/lib.rs`. Preserve `src/abi.rs` bounds and owned-buffer handling.
- Before distributing a derivative, replace the example identity and author, update
  the release version, and align renamed Cargo artifacts with both build scripts.
  Use PhotoCraft capitalization in displayed text. Embedded manifest changes need a rebuild.

## Supported contract

This starter uses panel API 1 and produces `Hello-PhotoCraft.pcext`. It exports
`memory` and `pce_api_version`, `pce_manifest`, `pce_alloc`, `pce_handle` from import-free
`wasm32-unknown-unknown` code. Keep the documented Rust export attributes; do not
introduce unsafe pointer access, WASI, filesystem/network imports, or guest UI frameworks.

Native controls are number/int/bool/choice fields and buttons. Free-text fields,
custom layouts, and general event callbacks are not provided. Keep field defaults,
allowed choices, button IDs, and handler validation consistent.

The host supplies `event`, `values`, `state`, and optional document summary metadata.
`documentRead` does not grant pixels, file paths, or an editable layer tree. Each
event uses a fresh instance; preserve unchanged state explicitly, including in
no-document/invalid-input replies. Do not rely on persistent guest globals.

Declare every requested engine command in `permissions.commands`. The host's
`extension.commands` allowlist and normal command preconditions also apply.
Use only APIs actually available in the target build. API-1 command lists are
sequential, not atomic; a later failure leaves earlier completed edits in history.
API-2 capabilities/requests are not valid API-1 fields. Report missing host support
instead of adding plugin-specific editor code or pretending an unsupported API exists.

## Build and verify

Use Rust 1.95+ with its native linker, rustfmt, Clippy, and the WASM target.
From this project root:

```sh
cargo fmt -- --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

Build with `./build.ps1` on Windows or `sh build.sh` on macOS/Linux. These wrappers
support `-Offline` / `--offline` after dependencies are cached. They generate the
release WASM and copy it to `Hello-PhotoCraft.pcext`; they honor `CARGO_TARGET_DIR`.

Test handler behavior for defaults, unknown events, invalid values/state, no document,
and the exact granted command shape. Native tests do not verify the actual WASM ABI.
Exercise the release artifact in a compatible host on a disposable document: info,
create, Undo/Redo, explicit replacement, disable, and remove as relevant to changes.
Current restricted MCP automation denies installation; use an authorized local test
route or report that acceptance as untested. Instructions do not authorize publishing
or replacing the user's existing plugins. Deliver source/artifact paths, host/API
requirements, checks performed, and any unverified behavior.

## Optional typed SDK authoring

This existing project keeps its supported wire versions and needs no SDK migration.
For a new API-2 project, `photocraft-plugin new AUTHORING_JSON FRESH_PROJECT` supplies
private API/SDK 0.1 sources, locks, licenses and a local guide. Edit `authoring.json`
and the handler; generated manifests/bindings belong in Cargo `OUT_DIR`. Keep local
SDK dependencies inside that project and retain its release hash index. Do not link
to engine/UI sources or introduce another plugin or companion as a runtime dependency.
The SDK is optional author tooling; availability/discovery never grants authority.

Current desktop hosts coordinate native work and store changes with OS locks and
per-plugin activation epochs. Management can return busy while native work runs.
Keep the host-owned store lock and activation tombstones intact. Completion/Apply
reject revoked native results; native containment, quotas and worker protocol 2
remain additional host work. Keep project docs and this guide aligned with changes.
