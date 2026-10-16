# Standalone plugin API and guest SDK

The API crate contains public serialized contracts. The guest SDK adds typed events/replies,
document request builders, bounded ABI storage, export helpers and optional build-time manifest
generation. Neither depends on the editor, UI, another plugin, an inference library or a shared
runtime. SDK/API 0.1 are a foundation preview; platform-wide stable certification is a later gate.

Native plugins may independently opt into [persistent worker protocol 2](plugin-worker-v2.md)
and [managed storage migrations](plugin-managed-storage.md). The public API crate contains
the worker descriptor and typed frames. The SDK is optional: current panels, filter ABI 1 and
native worker transports 1/2 can be implemented without it. See [lifecycle and budgets](plugin-lifecycle.md).

## One source manifest

Keep identity, UI, grants and contributions in `authoring.json`:

```json
{
  "formatVersion": 1,
  "descriptor": {
    "apiVersion": 1,
    "id": "org.example.my-plugin",
    "name": "My Plugin",
    "version": "1.0.0",
    "permissions": {"documentRead": false, "commands": []},
    "panel": {"fields": [], "buttons": [{"id": "info", "label": "Info"}]}
  },
  "package": {"packageVersion": 2, "module": "plugin.pcext"}
}
```

With the optional developer executable:

```sh
photocraft-plugin new authoring.json fresh-plugin
cd fresh-plugin
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked --release --target wasm32-unknown-unknown
```

Creation resolves a lockfile offline, so Rust and cached published dependencies must already be
available. A failed lock resolution leaves reviewable source and reports failure. `new` creates
a complete minimal WASM panel; implement its feature locally. Native/filter projects use their
own source layout and the same manifest generator. Existing starters need no SDK migration.

The generated project has its own workspace, lockfile, licenses, README, `AGENTS.md`, handler,
build script and privately included SDK/API source under `vendor/`. `vendor/SDK-RELEASE.json`
records the SDK files and hashes. Its build script reads `authoring.json` and generates the
embedded manifest and Rust constants into Cargo's `OUT_DIR`. Editing metadata never requires
hand-editing generated Rust. The guest is still import-free core WASM with the same four exports.

`photocraft-plugin generate authoring.json fresh-output` emits the corresponding `manifest.json`,
`package.json` and `bindings.rs` for custom build layouts. It refuses an existing output directory.
`photocraft-plugin sdk fresh-sdk` exports only the two public SDK source packages and their
workspace/release index. The executable embeds these sources; it does not read an editor
checkout while exporting. Vendored SDK copies are private development dependencies, not another
plugin installation. Authors may instead use appropriately pinned published SDK dependencies.

## Typed handlers and broker replies

Handlers receive `api::EventRequest`, including host version/capability metadata, state, values
and typed broker replies. They return `api::Result<api::PanelResponse>`. A handler error preserves
incoming state and proposes no effects. Malformed input, invalid effects, oversized state/output,
foreign request pointers and mismatched allocation lengths fail closed.

`api::LayersRead::request("layers")` builds a layer query. On `host_response`, decode
`request.reply("layers")?.decode::<api::LayersSnapshot>()`. Continue paging using its opaque
cursor. Use `api::LayersEdit` and `api::LayerEdit` for one revision-checked rename/visibility batch.
The host remains responsible for permissions, handle ownership, locks, revisions and Undo.

Query continuations remain bounded and host-driven. The SDK does not provide persistent guest
instances, arbitrary callbacks or implicit asynchronous/native authority. Module globals do not
persist between events. A successful atomic edit is terminal; no guest continuation runs after
commit. Command lists remain sequential and are not edit transactions.

## Clean builds and real-host checks

Set `PHOTOCRAFT_PLUGIN_CARGO` to the absolute Cargo executable and
`PHOTOCRAFT_PLUGIN_BUILD_PATH` to the intended Rust/linker/system tool PATH. Then:

```sh
photocraft-plugin project fresh-plugin
photocraft-plugin build-export fresh-plugin fresh-independent-export
photocraft-plugin check fresh-independent-export/dist/exported.pcplugin
```

The exported build uses only copied source and `--locked --offline`. When `authoring.json` is
present, packaging reads it directly rather than a separate `package.json`. The embedded
descriptor and archive metadata therefore share one input. Dependency source/notice ownership
is checked; arbitrary third-party Cargo build scripts are still trusted author tooling.

The existing `scripts/plugins/test-platform-examples.py` also builds a generated SDK guest outside
the editor and exercises its compiled package through the actual broker, contributed command,
atomic edit and Undo. Generated validation projects stay temporary; independently released
plugin binaries, catalogs and tests belong in their own repositories.

## Compatibility and evolution policy

- Plugin release versions, SDK crate versions, panel ABI, package version, host metadata and
  capability versions are separate. An SDK upgrade never silently upgrades a plugin's wire ABI.
- Target the PhotoCraft Plugin API, the only supported authoring API. Its internal panel
  discriminator is `1`. Package format 2, filter ABI 1 and native worker protocols 1/2 remain
  independent. Implementation phases do not create parallel APIs or new majors.
- The implemented capability methods and message/error semantics retain their meaning.
  Extend this one current API during development. A new major requires an actual unavoidable
  breaking wire/behavior change, rather than a completed implementation phase.
- Unknown required capabilities fail clearly; optional ones negotiate unavailable and need a
  deliberate fallback. Negotiation/discovery does not grant permission.
- Descriptors, effect messages and package metadata remain strict. The event envelope and
  read-only snapshots may carry additional optional metadata that readers can ignore.
  Existing strict host-metadata fields and capability error codes do not change in place.
- Generate JSON Schema snapshots from the shared Rust types and run consistency tests. Schema
  shape never replaces byte-budget, cross-field, authority or runtime validation.
- Deprecation/removal needs an explicit migration, compatibility evidence and release notes.
  This preview does not promise a calendar support window or certify future categories/targets.

Richer widgets, high-precision tile transport, tools and live effects remain later capabilities
of the same developing API. Worker 2 is an optional transport described separately. The SDK cannot make unavailable host APIs available.
