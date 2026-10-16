# Plugin lifecycle and resource admission

Installed plugins remain independent projects and releases. A plugin owns its WASM, native
worker, dependencies, licenses and assets. PhotoCraft never shares a worker process or a
writable runtime directory between plugin identities. The SDK is optional and has no shared
runtime service.

## Discovery and activation

Installations persist the validated embedded panel descriptor in the host's installation
record and immutable ownership record. Startup reads those records instead of loading every
WASM module or hashing every model. A record without a cached descriptor requires
validation of its panel module before discovery. Loose descriptor sidecars are not
installation authority.

`extension.list` includes `lifecycle`: `disabled`, `idle`, `active`, or `failed`. Discovery and
compatibility validation precede the idle state. The first panel action verifies only its
bounded module and compares the embedded descriptor with the cache. Scheduled processing
verifies the full owned closure in cancellable 64 KiB
reads before native execution or filter compilation. Model hashing and filter loading never
run in UI preparation. No native process starts during discovery.
Three failed deferred activations quarantine that handle; `extension.enable` with
`enabled:true` refreshes it and provides an explicit retry. Recovery startup with
`--disable-plugins` remains available.

The registry accepts at most 4096 installed plugins. The thousand-descriptor regression
checks dormant discovery and verifies that payloads are still unopened. This is a functional
scale gate, not a claim that the later release-performance budgets have been certified on
every operating system.

Release checks and verified downloads run in background jobs and do not start plugin workers.
They prepare a reviewed candidate; publication uses the existing managed store transaction.
Optional GitHub metadata is owned by each independent plugin. See [updates](plugin-updates.md)
for version, identity and review rules. A future [marketplace](plugin-marketplace.md) will use
the same lifecycle rather than introduce a separate runtime or installer.

## Scheduling

Native image jobs and maintenance tasks enter a process-wide bounded scheduler. It permits
one active job per private plugin runtime, rotates eligible plugin owners fairly, and prioritizes interactive
previews while admitting background work after a bounded interactive burst. A plugin cannot
take over another plugin's private process. Waiting jobs check cancellation without holding
the editor's document locks.

The initial policy allows four running jobs, eight resident persistent workers, 64 queued jobs,
and eight queued jobs per private plugin runtime, plus eight bounded resident-start waiters. Admission has a
five-minute deadline. Aggregate reservations are four CPU units, 8 GiB of memory
on 64-bit builds (1 GiB on 32-bit), 4 GiB GPU memory, 16 GiB temporary storage and 8 GiB managed
cache. `extension.commands` reports the current policy and usage. A job exceeding an individual
budget is rejected before starting; a queued job waits for an available reservation.

Persistent workers declare their working estimates in
[worker protocol 2](plugin-worker-v2.md). Resident memory, GPU and cache reservations remain
charged while the private process is idle. Job reservations cover CPU, temporary storage,
host image buffers and the filter VM memory limit; releasing a reservation does not depend
on admitting the result.
Protocol-1 workers retain their launch arguments and receive conservative admission estimates.

These reservations limit admitted work and host-owned buffers. Trusted native code still has
the user's operating-system privileges and can exceed its declared estimates. Restricted
native containment is unavailable; it is a separate trust-phase requirement. Sandboxed WASM
retains its enforced fuel and linear-memory limits.

## Preview retention and revocation

Comparisons are associated with their plugin, source document and invocation. Retention
allows 16 comparisons within a 512 MiB conservative snapshot budget. It accounts
for retained source/result tiles and document blobs rather than claiming an exact OS heap limit.
Compiled WASM/module caches remain bounded per guest/package; these native admission figures
do not claim to measure every compiled-code allocation in the editor. Creating another
plugin's preview cannot silently evict an unapplied result. When the budget is exhausted,
the command reports an error so the caller
can apply or discard a comparison explicitly. Calls without a preview token continue
to address the current comparison for the requested plugin.

Disable, removal, replacement and rollback revoke queued/running host work and future result
commits. Source revisions and installation activation epochs are checked independently for
every completed result and Apply. A short shared store guard spans final authority validation
and document/state admission, preventing disable from racing that commit. Persistent workers
receive bounded stop/cleanup. Existing
worker-1 cancellation remains cooperative, followed by direct-process termination after the
documented grace period. Neither transport is an operating-system process-tree sandbox.

`extension.runtime` reports persistent-worker status and accepts explicit `retry` or `reset`
actions. Reset uses a scheduled background job and bounded guest acknowledgement. Re-enabling
a plugin creates a fresh activation and clears its deferred-activation quarantine.

## Store transactions and recovery

Updates use the same plugin ID through local package selection or explicit GitHub release
checks and verified downloads. The manager reviews access before applying either source
through `extension.install`. The embedded manifest records the plugin author's release
version. See [plugin updates](plugin-updates.md) for source metadata and command details.

An operating-system store lock serializes installation and publication. Separate per-generation reader
leases protect immutable code while it is in use, including an idle persistent worker.
Updating another plugin remains possible. Copying managed data or reusing a destination
requires that generation to be idle. Removing or publishing the selected record revokes
old work immediately; leased old bytes are retained until safe collection, with a warning
when cleanup must be deferred. Lock files retain their filesystem identity and are never
replaced as a way to break a lease.

An idle processing engine can therefore temporarily block an update or reinstall until its
declared idle timeout expires (1–300 seconds). Wait and retry, or disable the plugin, let its
engine stop, then inspect the file or check/download its update again. Disabling changes the
activation and invalidates the previous review. Command clients can use
`extension.runtime {"id":"org.example.plugin","action":"retry"}` to retire an idle engine
without changing activation, then retry once cleanup releases its lease. Runtime `reset`
keeps the engine alive and does not release this lease.

Updates prepare [managed storage](plugin-managed-storage.md) in a separate generation, validate
the complete package, preserve a previous code/state record, and publish the active record
last. `extension.rollback` restores the verified previous pair. A failed preparation or an
interrupted staging transaction leaves the previous working pair selected. A bounded rollback
journal becomes authoritative only when its target matches the active record, preserving the
earlier rollback choice if publication fails. Read-only stores
can discover already initialized records; installation and recovery changes require a writable
store. Unsupported locking filesystems fail closed.

Uninstall preserves managed data by default. `extension.remove` with `purgeData:true` explicitly
requests removal of host-managed state whose generation is safe to collect. One previous
code/state pair is retained for rollback; older unleased owned pairs are eligible for
collection. The host never
deletes a plugin's external caches, downloaded assets or arbitrary native side effects.
Other already-running editor instances must refresh their registry by restarting; an obsolete
instance cannot commit a stale native result.
