# Plugin managed storage and migration

Host-managed storage is optional infrastructure for independent plugins. It does not move
plugin algorithms, assets, dependencies or releases into PhotoCraft. Existing plugins keep
their current APIs and their external caches. Installing a package never imports, moves or
deletes an arbitrary native cache or model download.

## Generation layout

The host separates mutable state from immutable package payloads:

```text
Extensions/
  <id>.installation.json              # selects code and managed state by hash
  <id>.rollback.json                  # previous installation, when available
  bundles/<id>/<archive-blake3>/      # immutable package members
  managed/<id>/<archive-blake3>/
    .photocraft-managed.json          # completed generation and schema identity
    .photocraft-managed-stage.json    # ownership for interrupted staging recovery
    settings.json                    # bounded JSON object
    data/                            # durable plugin-owned files
    cache/                           # disposable files, separately accounted
    secrets-refs.json                # opaque references, never secret values
```

The active installation record selects both directories with the same archive hash. There is
no independently mutable managed-state pointer. A rollback selects the previous code and its
previous managed state together; it does not run a reverse migration or merge newer changes
into an older schema. Native side effects outside these directories cannot be rolled back.

Each plugin and code generation has its own paths. Worker protocol 2 receives the selected
paths, schema version and quotas in its host handshake. Paths are host metadata rather than
WASM filesystem imports or a broker capability. A portable guest still needs an implemented,
authorized broker operation to access storage. Trusted native code retains its OS access;
these paths do not turn a native process into a filesystem sandbox.

## Separate bounds

Default limits apply to each completed managed generation:

| Area | Default byte limit | Additional bounds |
|---|---:|---|
| Settings | 1 MiB | One JSON object; keys at most 128 bytes |
| Durable data | 64 MiB | At most 2048 files |
| Cache | 128 MiB | At most 2048 files |
| Secrets references | 64 KiB | At most 256 named references |

Data and cache trees allow eight directory levels, safe relative ASCII path components and
bounded total entries. Symlinks, Windows reparse points, device files and unknown files at
the managed-generation root fail validation. JSON is limited to 16 levels and 8192 values.
Byte counts, file counts and schema validation are checked before publishing or accepting
managed state. `extension.storage` reports each area's bytes and data/cache file counts.

These are host validation and admission limits. They are not kernel disk quotas, continuous
monitoring of native writes, or limits on plugin-owned external caches. A trusted native
worker can exceed its declared resource use or write outside managed storage using its OS
privileges; a later host validation rejects invalid managed state. Process-wide scheduling
reservations are described in [lifecycle and resource admission](plugin-lifecycle.md).

`secrets-refs.json` maps names to ASCII tokens prefixed with `photocraft-secret:`, for example
`{"account":"photocraft-secret:opaque-123"}`. Tokens are bounded to 256 bytes and permit only
ASCII letters, digits, `.`, `_`, `:`, and `-`. This module validates and migrates references;
it does not implement an OS credential store, resolve tokens or authorize account access.
Never put passwords, API keys or refresh tokens in this file or in a migration plan.

## Declarative migration contract

A package can include `runtime/managed-v1.json` as a recorded archive member. PhotoCraft
validates it during inspection and reads the verified installed member for an update.
Migration is data transformation in the host; no worker, script, shell or WASM migration
callback runs. The file has a maximum size of 64 KiB and at most 64 operations across all
areas. Unknown fields and operations are rejected.

```json
{
  "formatVersion": 1,
  "fromSchema": 0,
  "toSchema": 1,
  "settings": [
    {"op": "rename", "from": "oldQuality", "to": "quality"},
    {"op": "setDefault", "key": "mode", "value": "balanced"},
    {"op": "remove", "key": "obsolete"}
  ],
  "data": [
    {"op": "move", "from": "preferences.bin", "to": "v1/preferences.bin"},
    {"op": "remove", "path": "obsolete.bin"}
  ],
  "secretsReferences": [
    {"from": "account", "to": "primaryAccount"}
  ],
  "copyCache": false
}
```

`fromSchema` must match the previous completed generation exactly. `toSchema` must be greater
than `fromSchema`. Missing legacy managed storage is treated as an empty schema-0 state;
an existing corrupt generation is rejected rather than treated as empty. A first installation
also begins with empty schema 0.

Settings operations address top-level keys. `setDefault` leaves an existing value intact.
Renames require an existing source and an absent destination. Data moves address regular
files, create bounded parent directories and refuse to overwrite a destination. Removals are
idempotent for missing keys/files. Secret-reference operations rename existing references
without resolving their values. A failure in any operation aborts the entire prepared state.

An update without a migration plan preserves the source schema, copies settings, durable
data and references, and starts with an empty cache. Cache is copied only when a plan sets
`copyCache:true`; it must still fit its independent limit. A plugin needing conditional logic,
an unsupported transformation or multiple historical input schemas must publish an explicit
supported upgrade path. The host does not guess a migration or execute arbitrary code.

## Transaction, retry and rollback

The installer holds the cooperative mutation locks and keeps the source generation quiescent
while preparing a snapshot. Preparation writes an ownership-marked temporary sibling under
`managed/<id>/staging-*`, streams copies with byte/file bounds, applies the declarative plan,
then validates settings, references and every managed file. Source state remains unchanged.

Commit synchronizes new files, writes a completion marker containing identity, schema,
source hash, plan digest and quotas, and atomically renames the staging directory into its
hash-named destination. Unix directory entries are synchronized as well. The installer
publishes the active installation record only after code and managed state are ready.
A bounded host-owned rollback journal records the intended target and prior selection.
It changes the rollback choice only once that target is the active record; failed publication
therefore preserves both the current selection and its earlier rollback target. After a
successful publication, a matching journal remains usable across restart even if finalizing
the ordinary rollback record fails.
Filesystem rename/flush semantics still apply; no cross-device rename is used. Windows'
safe standard file API does not provide a directory flush, so this is not a stronger
power-loss guarantee than the underlying filesystem supplies.

A validation error, write failure or interrupted staging operation leaves the old selected
pair available. Recovery removes only bounded `staging-*` directories with valid matching
host ownership metadata; unknown or malformed directories are preserved for investigation.
Finished hash-named generations are never mistaken for abandoned staging. Failed normal
preparation drops its own temporary directory. Simulated write failures test the full-disk
path without filling a real user disk.

Existing completed state is reused without writing or rerunning a migration. This supports
read-only access and identical reinstallation. It also defines A → B → A behavior: returning
to A selects A's retained state rather than migrating B over it. Returning to B later reuses
B's snapshot if it is still retained. A supplied plan must match a reused generation's schema and recorded
plan digest. A completed but unselected generation left by an interrupted publication is
subject to the same explicit reuse rule.

Retention intentionally covers the current installation and one previous rollback pair.
After a successful third update, older owned code and its managed state are eligible for
collection when their leases permit it. This is not a historical archive, and returning to a
collected generation starts a new preparation from the selected source and its declared
migration. Preserve important user documents through the normal document/export workflow.

`extension.rollback` verifies and selects the retained previous installation. The caller
also revokes old activation epochs and outstanding result commits. See
[lifecycle and revocation](plugin-lifecycle.md) for cross-process leases and worker shutdown.

Uninstall preserves the current and retained previous managed states by default.
`extension.remove` with `purgeData:true` requests
deletion of validated host-managed generations the installer can safely collect. Code and
state still leased by another process can be retained with a warning. The host does not
follow links, recursively delete unknown neighboring files, or delete external native
caches. Explicit removal is confined to the `managed/` namespace.

## Implementation boundary

[`extension_package/managed.rs`](../crates/engine/src/extension_package/managed.rs) owns bounded
preparation, validation, migration, usage accounting and managed-state deletion.
[`extension_package.rs`](../crates/engine/src/extension_package.rs) owns installation records,
source/destination leases, publication ordering and rollback selection. Module callers must
hold the mutation lock through prepare, commit and record publication; the module does not
take a second lock or independently change the selected installation.

The module's regressions cover copy and migration, source preservation, schema mismatch,
late-operation failure, simulated full disk, quotas, interrupted staging recovery, read-only
reuse semantics, A → B → A, opaque references and external-cache preservation. The existing
package suites cover the surrounding installation and runtime behavior.


For persistent workers the effective cache quota is no greater than their declared cache-byte
reservation. A zero cache declaration therefore permits no cache-file bytes. The host validates
managed state before native work and before accepting a successful result; a quota failure
retires the persistent worker and prevents that result from changing the document or guest state.
