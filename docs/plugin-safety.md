# Plugin safety, compatibility and recovery

PhotoCraft checks plugin structure and restricts its WebAssembly guests. Installation success
does not prove a plugin's algorithm is correct, establish its publisher's identity, or sandbox
native processing code. This page describes the protections and their limits in this source tree.
See the [developer handbook](plugin-development.md) for authoring and [testing](plugin-testing.md)
for release checks. An older editor may not contain these additions.

## What installation checks

| Input or problem | Enforced behavior |
|---|---|
| Invalid WebAssembly, unsupported instructions or forbidden imports | Loading fails; no file/network/WASI imports are exposed |
| Wrong panel/filter ABI, export signatures or API version | Rejected by the corresponding loader; changing a filename does not convert formats |
| Malformed/oversized manifest, duplicate controls, invalid defaults | Rejected before registration |
| Unsupported panel command permission | Rejected against the host's explicit allowlist and command registry |
| File changed between manager review and install | Reviewed BLAKE3 hash must match |
| ZIP path traversal, absolute paths, unsafe names, duplicate paths, oversized content | Native package inspection rejects it within finite archive/member limits |
| Link-like ZIP attributes or managed filesystem links | ZIP members become regular files; checked managed paths refuse filesystem links |
| Corrupt ZIP content or installed payload | CRC/archive/per-file checks reject it; managed payload hashes are verified when loading |
| Wrong package version/platform/service or missing required members | Rejected against the version-2 schema; declared native target must match; old fixed-service packages require replacement |
| Invalid or conflicting packaged filters | Each module's ABI/manifest is validated; duplicate ids and the reserved `native` filter id are rejected |
| Failed replacement validation/extraction | A valid installation is published only after staging and validation |

Reading a manifest executes a guest's WASM initialization/version/manifest exports inside the
bounded interpreter. It does not call the action handler or start its native worker. Opening
a panel likewise does not run its button handler. An installer can detect a trap in code it
executes during inspection; a later handler failure is caught when that handler is used.

Author and version strings are self-declared metadata. There is no publisher signature check,
malware scan, executable architecture inspection or model-quality validation. A native worker
with invalid executable bytes or a corrupt model may pass structural installation and fail
when processing starts. Do not interpret a successful install as a compatibility certification.

## What execution restricts

Panel guests have an 8 MiB module limit, 32 MiB linear-memory limit, bounded call depth and
5 million units of instruction fuel per instance. Each event gets a fresh instance. Requests
and responses are limited to 256 KiB each; serialized session state to 128 KiB; responses to
32 commands and 4 KiB of status text. Bad pointers, traps, loops, malformed JSON, invalid field
values and oversized state return errors. Oversized state is rejected before any returned command.

The host preflights all returned command ids and grants before the first command, and runs each
through ordinary authorization, parameters, document/target checks and job conflicts. WASM has
no arbitrary document access, filesystem, networking, subprocess or indirect-command interface.
Its metadata read permission supplies a limited document summary, not pixels or file paths.
`plugin.processImage` resolves only the invoking installation's declared native worker or
packaged filter. A guest cannot select another installation, a global filter, or a process
path. Packaged filters remain sandboxed; a native worker has a different trust boundary.
The processing command grant itself authorizes visible image pixels through the selected
backend; it does not depend on the separate `documentRead` metadata grant.

`plugin.runTask` additionally permits document-independent work in the owning package's native
worker. It requires an explicit grant and native backend, rejects cross-installation ids and
guest process paths, and is denied by restricted automation and recovery mode. Task JSON is
bounded to 128 KiB, its message to 8192 UTF-8 bytes, and successful results cannot dispatch
commands or edit documents. Disabling/re-enabling revokes outstanding task state updates.
The shared native trust and cancellation boundaries still apply; see [native tasks](plugin-native-tasks.md).

These commands retain their normal behavior. A batch is sequential, not an atomic transaction:
if a later step fails, earlier successful actions remain. Undoable edits stay in History;
navigation, document duplication and explicit Undo/Redo commands have their own semantics.
Proposed guest state is committed only after the relevant successful operation.

API-2 broker edits use a distinct contract: a single bounded rename/visibility batch either
commits as one Undo step or changes no document/history/state. Host-created principals and
opaque handles bind requests to the session, owning installation, document incarnation and
revision. Captured and current automation restrictions both apply, including deferred-event
retries. Inspector events are read-only; they cannot execute editing commands or native jobs.
See [panel API 2](plugin-api-v2.md) for its experimental scope and reported limits.

Pixel filters use a different ABI and different fuel/memory limits. They process temporary
band data; a failed filter does not commit partially processed pixel bands. Output samples,
selection blending, supported modes/depths and alpha handling still need algorithm-specific
tests. See the [filter reference](plugins.md) for exact resource and pixel contracts.

## Native processing and preview results

Native capabilities belong to the installed package that owns them. A raw guest cannot borrow
another plugin's processing engine by requesting the same permission. Native-code installation
requires explicit review/consent, and restricted automation cannot grant itself that access.

Version-2 packages permit independently authored native image algorithms and packaged WASM
filters; compatibility checks do not establish their safety or correctness. Native workers
must validate options and write the promised sRGB PNG. Host flattening, per-operation image
bounds, result validation and owner binding protect editing transactions; they do not remove
the native process's OS privileges. See [the generic contract](plugin-image-processing.md).

A worker runs as a separate hidden process with a host-created job request and temporary paths.
It reports progress and receives cancellation; the host imposes a 45-minute native-worker execution deadline and
requests direct-process termination after a 10-second cancellation grace period. Diagnostics
use a temporary regular file with a 16 MiB checked limit and bounded reads/messages. The host
does not wait for stderr pipe EOF from inherited child handles. That avoids an exited worker
leaving a diagnostics thread and installation lease stuck indefinitely.

The output must match the expected bounded PNG dimensions/depth/byte limit before and after
decoding. A successful exit alone cannot admit a result. Source identity/revision and owning
installation checks reject stale results. Disable, replacement, removal and source edits revoke
pending previews; Apply is also blocked during modal editing or a pending crop. New-layer Apply
uses normal history, and temporary comparisons are discarded without editing the source.

Native code runs with the user's OS permissions. It is outside the WASM sandbox. Worker
cooperation is still responsible for stopping/reaping any engine descendants it starts; there
is no general OS process-tree sandbox. Integrity hashes detect changes relative to installation
records; they do not make a publisher trustworthy or prevent another local process from tampering
after a check. See the [native package reference](plugin-native-packages.md).

## Start without plugins

Launch the updated desktop editor with **`--disable-plugins`** for a recovery session:

```powershell
& 'C:\path\to\photocraft.exe' --disable-plugins
```

On macOS/Linux pass the same flag to the desktop executable. It can be combined with
`--safe-gpu` when graphics recovery is needed as well.

This skips the managed extension store and the additional filter folder before they are loaded.
Plugin installation, inspection, invocation and native processing are blocked for this launch.
Ordinary document editing remains available. Installed files, saved enable flags, plugin panel
settings and preferences are retained; starting normally restores the usual plugin configuration.
The manager shows that plugins were skipped, rather than claiming those installations were removed.

This is a manual recovery option, not automatic crash attribution or quarantine. A disabled
plugin in a normal launch still undergoes bounded startup validation; the recovery flag bypasses
loading altogether. Start a new process with the flag; it is not an in-app switch for already
running code. Cached Smart Filter rendering may differ when its required filter is unavailable.

## Repair a damaged installation

A bad managed installation is skipped at startup and reported under Manage Plugins; it does
not stop other valid installations from loading. To repair a corrupt installation record:

1. Start normally; the manager reports the failed installation.
2. Choose a known-good plugin file and review its identity and requested access again.
3. Select **Replace or repair an existing installation**, then **Install plugin**.
4. Enable the repaired installation explicitly if it was disabled before repair.

Explicit replacement can back up bounded corrupt metadata and publish a validated replacement.
Its backup uses a `.corrupt-<hash>.json` suffix. Unknown old payload directories and unrelated
files are retained because the damaged record cannot establish their ownership. Filesystem/link
errors, oversized records, conflicting backups or damaged ownership metadata fail conservatively;
they are not permission to recursively delete the plugin directory. The desktop settings directory
and portable-mode rules are described in [development](development.md).

In recovery mode installation is intentionally blocked. Close it and start normally to perform
a reviewed repair. If loading itself prevents normal startup, keep the recovery session for your
documents and investigate the indicated settings folder with the app closed. Back up files before
moving a suspect installation aside; do not delete the entire settings or recovery directory.

## Cooperative store locking

Desktop instances using this implementation share `.photocraft-store.lock` in the managed
plugin folder. Startup/loads and executing native jobs acquire shared OS locks; installation,
repair, enable/disable and removal require an exclusive lock. Contention returns a busy error
before changing files. Locks release when their owning file closes or the process exits. Never
replace/delete this file to clear a busy error. An existing store needs one writable initialization
to create it; filesystems without working locks fail closed.

Native image/task workers hold a read lease through execution. Completion and preview Apply
recheck the selected installation record's hash, package, activation epoch and disabled marker, holding
a read lease through result/state admission. An update/remove/disable in another current host
therefore rejects an obsolete result. Completed previews do not retain a long-running OS lease.
Each management operation advances a bounded per-plugin epoch in `<id>.activation.json`.
Retain this small host-owned tombstone after removal: disable/re-enable or reinstalling identical
bytes must not revive an old result in another process. Missing epochs in existing stores start
at zero. Malformed, oversized or exhausted epochs fail closed; do not delete them to reset a plugin.
If publication fails after revocation, the previous active record remains selected but other
sessions may need to reload its native identity before retrying work.

This first Phase 2 slice uses one lock for the entire store. Native work temporarily blocks
management operations for every plugin in that store; concurrent readers/jobs are allowed.
It coordinates cooperating current hosts, not old hosts, manual file edits or malicious native
code. It does not synchronize all running instances' in-memory registries or supervise worker
descendants after host death. Restart other desktop instances after changing installations to
reload their plugin registry; there is no live cross-process registry refresh yet. Per-generation
leases, migrations, quotas and stronger native supervision remain later work.

## Remaining limits and regression evidence

- No automatic plugin crash quarantine, publisher signing or malware certification.
- Native package compatibility checks are structural; runtime/model/driver behavior is validated on use.
- Memory and image bounds are per operation/instance, not an aggregate quota for all jobs, documents,
  filter threads, GPU allocations and other applications. Available memory can still be exhausted.
- Cooperative OS store locks protect current hosts; older instances must be closed before
  changing installations. Native process trees and aggregate resources remain uncontained.
- WASM fuel bounds guest execution, not the cost of requested editor commands. Heavy built-in
  commands have their normal engine bounds and can still take time.
- Damaged ownership metadata may require manual investigation rather than automatic removal.

The audit added focused regressions for corrupt-record repair, disabled-state preservation,
oversized panel state, diagnostic cleanup/log limits, modal/crop Apply gates, removed panel state,
recovery command policy and startup folder suppression. It also fixed two existing filter/format
issues found during review: finite HDR selection blending and forged stored-ZIP size allocation.
These checks complement the existing hostile-module, permissions, package, jobs, stale-result,
selection, alpha, Undo/Redo and automation tests. They do not prove the absence of every bug or
establish compatibility with every image, plugin, machine or driver.
