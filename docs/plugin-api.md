# PhotoCraft Plugin API

**Status:** current pre-release authoring contract, 2026-10-16, supplied by
`photocraft-plugin-api 0.1.0`. Package format 2 carries this API; filter ABI 1 remains current.
This is the public plugin API. Filter ABI 1 and native worker protocols 1 and 2
are independent contracts. A separate [future package-format proposal](plugin-package-v3.md)
describes possible packaging changes; it is not another implemented public API.

The [guest SDK](plugin-sdk.md) now provides the typed event envelope, broker replies, layer
requests/snapshots, bounded ABI exports and single-manifest authoring. These describe the
implemented capability messages. [Persistent native workers](plugin-worker-v2.md) add explicitly selected reusable transport;
SDK completion does not imply completion of later host capabilities.

## Version boundaries

The plugin's release version, contract crate version, package version, panel ABI, host metadata
version and each capability version are independent. Authors currently set `apiVersion: 1`
and export `pce_api_version() == 1`; these internal wire discriminators do not name separate
product APIs. Evolve the API through compatible
capabilities and optional fields; introduce another major only for a genuine breaking change.
The API uses four export signatures, import-free WASM, a fresh instance per event, fuel/memory bounds and JSON
transport. It does not introduce ambient filesystem, network, native or host-function imports.

The host metadata object has `version: 1` and `capabilities: [{id, version}]`. Capability discovery
describes implemented interfaces; it is not a permission grant. `photocraft-plugin-api` is an
independently publishable L0 crate containing only serialized data and validators. Guests may
instead implement these documented JSON contracts in another language.

## Descriptor

The descriptor contains identity, metadata, permissions, declarative fields and buttons, plus
optional capability requirements, contributions and plugin-owned release metadata:

```json
{
  "apiVersion": 1,
  "id": "org.example.layers",
  "name": "Layer Organizer",
  "version": "0.1.0",
  "permissions": {"documentRead": true, "commands": []},
  "panel": {"fields": [], "buttons": [{"id": "organize", "label": "Organize Layers"}]},
  "capabilities": {
    "required": [{"id": "host.document.edit", "minVersion": 1, "maxVersion": 1}],
    "optional": [{"id": "host.ui.views", "minVersion": 1}]
  },
  "contributes": {
    "commands": [{
      "id": "org.example.layers.organize",
      "label": "Organize Layers",
      "event": "organize",
      "params": {},
      "needsDocument": true
    }]
  }
}
```

`repository` may identify the plugin's canonical public GitHub repository. Optional
`updates: {"provider":"github","asset":"Plugin-{platform}.pcplugin"}` opts into explicit
release checks. Updates require a repository; only one `{platform}` token is supported and
the asset is an exact safe `.pcplugin` or `.pcext` filename. These additive fields do not
change the API major or grant network access to a WASM guest. See [updates](plugin-updates.md)
for publisher instructions and host validation.

Capabilities are extensible lowercase identifiers, limited to 96 ASCII bytes. Initially known
IDs are `host.commands.invoke`, `host.document.read`, `host.document.edit`, `host.events` and
`host.ui.views`. A requirement selects an inclusive `minVersion`/optional `maxVersion` range
within 1–1024. At most 32 total requirements are permitted, with no repeated IDs across either
list. An unknown required ID fails with `missingCapability`. An unknown optional ID is retained
and negotiated as unavailable, so a guest can follow its fallback. An existing ID outside the
range fails with `unsupportedVersion` when required. There is no automatic downgrade of a
capability's behavior and no wildcard authority.

Contribution IDs belong to the plugin's namespace: `pluginId.localId`, at most 128 ASCII bytes,
with a nonempty local portion of at most 63 bytes. IDs are unique across categories. Built-in
command collisions and granted authority remain host checks. The host reserves its engine/UI
root namespaces and rejects collisions at installation and startup, including shell commands
such as `help.about`. Initial contribution categories:

| Category | Descriptor | Limit |
|---|---|---|
| Commands | `id`, `label`, `event`, optional object `params`, optional `needsDocument` | 32 |
| Inspectors | `id`, `label`, `event` | 8 |
| Subscriptions | `id`, `event: "document.changed"`, `handler` | 16 |

Command `params` are bounded defaults, not a JSON Schema or arbitrary host command permission.
Events/handlers use the existing 1–64 byte alphanumeric/underscore event syntax. A read-only
inspector may use zero panel buttons; other panels retain the 1–16 button requirement. Fields
remain limited to 32, with the same numeric, integer, boolean and choice controls.

The first host slice accepts one inspector and one matching document subscription per plugin;
larger descriptor counts return `unsupportedOperation`. Schema capacity is not a promise that
every host implements every count or capability. Inspect `extension.capabilities` for host limits.

The descriptor is limited to 64 KiB before parsing. Unknown descriptor fields are errors.
Empty capability and contribution collections may be omitted.

## Responses and broker boundary

Responses carry optional `state`, `message`, `commands`, `requests` and `view`:

```json
{
  "state": {"lastRevision": 12},
  "message": "Layers inspected",
  "requests": [{"id": "read", "method": "documents.layers.read", "params": {}}],
  "view": {"title": "Document Inspector", "rows": [{"label": "Layers", "value": "3"}]}
}
```

Request IDs correlate replies; they are unique within the response. Method-specific parameters
and document replies are defined by the engine broker. They never carry an authoritative
plugin ID. Only the host binds a call to its principal, installed generation and permitted
document. Opaque document/layer handles are bearer references which must pass those checks;
successful JSON parsing does not establish a handle's validity. A plugin cannot select another
plugin's principal or increase automation authority using request data.

Initial broker methods are `documents.layers.read` (optional document handle, cursor and page
limit of 1–128) and `documents.layers.edit` (document handle, `expectedRevision` and 1–128
distinct layer edits, each carrying a layer handle and optional name/visibility). An edit must
be the sole broker request and runs only for an explicit user action. Observer callbacks can
read and update their native view, and cannot edit. Failed edit batches preserve the document
and history; successful batches create one Undo step. A sole edit is terminal: the host commits
the validated guest state and message after the edit succeeds, and does not execute a guest
continuation after commit. Required view permissions are checked before the edit begins.

Responses permit either sequential command invocations or broker requests, not both. Each response is at most
256 KiB, state at most 128 KiB, message at most 4096 bytes, commands at most 32 and requests at
most 8. Each broker parameter object is at most 64 KiB. Inspector views have at most 64 rows,
titles/labels of at most 128 characters and printable values of at most 1024 characters. Native
view rendering belongs to the UI layer; no guest UI code executes in the drawing path.

All supported panels use this response parser. The current implementation uses fresh WASM
instances and host-carried state; general asynchronous continuations and persistent interactive
guest instances remain future capabilities.

Document invalidations coalesce and use round-robin admission so early names cannot monopolize
the queue. Desktop inspector handlers run outside drawing, with four process-wide worker slots;
browser handlers retain bounded inline execution. Closing a receiver does not release a live
worker's slot. Events, pending query continuations and opaque handles retain captured automation
policies, including after busy admission or a concurrent guest-state change; the current caller's
policy also applies. A local retry cannot broaden the authority of an earlier restricted event.

## Errors, schemas and validation

`ApiError` contains a stable camelCase `code`, a human-readable `message` and structured
`details`. Initial codes include `invalidJson`, `invalidDescriptor`, `limitExceeded`,
`incompatibleApi`, `missingCapability`, `unsupportedVersion`, `permissionDenied`,
`invalidHandle`, `staleRevision`, `notFound`, `conflict` and `unsupportedOperation`.
Capability failures identify the requested ID, range and advertised version in `details`.
Consumers match codes instead of parsing prose.

The [schemas](../schemas/plugins/) are Draft 2020-12 JSON Schema generated from Rust contracts
with the optional `schema` feature. Regenerate from the repository root:

```text
cargo run -p photocraft-plugin-api --features schema --example export_schemas -- schemas/plugins crates/plugin-api/schemas
cargo test -p photocraft-plugin-api --all-features
```

JSON Schema checks structural shape and per-field limits. Native validators additionally check
serialized byte budgets, version-range ordering, cross-list duplicates, owned contribution
namespaces, field ranges/defaults and mutually exclusive response effects. Broker validation
then checks permission, capability availability, handle scope, revision and operation validity.
No schema or descriptor is an authority credential. Contract-generation consistency tests
prevent checked-in schema drift.
