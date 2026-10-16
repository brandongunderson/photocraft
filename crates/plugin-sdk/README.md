# PhotoCraft plugin guest SDK

This crate owns only safe ABI storage and public serialized contracts. It has no editor,
UI, worker, model or sibling-plugin dependency. `photocraft-plugin-api` is its data-only
development dependency. Neither crate requires an additional installed runtime service.

Use `photocraft-plugin new authoring.json fresh-project` to generate a standalone project
with private copies of both SDK crates, licenses, generated manifests/bindings and a lockfile.
That project builds with the editor and sibling sources absent. `photocraft-plugin sdk
fresh-directory` exports just the public SDK sources for other build layouts.

`export_plugin!(MANIFEST_JSON, handle)` exports panel API 2 using the existing four `pce_*`
functions. Handlers receive `api::EventRequest` and return `api::Result<api::PanelResponse>`.
Use `api::LayersRead` / `api::LayersEdit` to create bounded broker requests, and decode
`api::BrokerReply` into `api::LayersSnapshot`. Handles remain opaque references checked by
PhotoCraft. Capability discovery does not grant permission.

The SDK checks request/response bounds, accepts only its own request allocation, and never
dereferences ABI pointers. Handler errors preserve incoming state and request no actions.
Malformed/oversized requests or invalid responses fail closed; guests should still validate
their own settings and algorithms. Host memory/fuel limits remain authoritative.

The macro's narrowly allowed Rust 2024 export-name attributes name symbols only. Generated
guest crates use `deny(unsafe_code)`; no unsafe functions, blocks or pointer reads are added.
API 1, filter ABI 1, package 2 and native worker 1 remain independent supported contracts.
