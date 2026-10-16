# Native plugin tasks

`plugin.runTask` runs document-independent maintenance in an installed version-2 package's
own native worker. Use it for explicit asset management, diagnostics or other operations that
do not need an image. It requires a host build containing this command; panel API version 1
and package format version 2 remain unchanged. Older hosts reject the unsupported grant.

## Grant and invoke

Include `plugin.runTask` in the panel's `permissions.commands`. A compatible native backend
must be included in the same package, with the existing `native.service: "plugin.processImage"`
descriptor and its corresponding grant. Installation still requires native-code consent.
Raw panels and packages without a native worker cannot request `plugin.runTask`.

A button handler returns exactly one command:

```json
{
  "state": {"lastAction": "refresh"},
  "commands": [{"command": "plugin.runTask", "params": {"options": {"action": "refresh"}}}]
}
```

`params` accepts only optional `id` and object `options` (default `{}`, at most 128 KiB of
serialized JSON). Panels omit `id`; the host binds the request to that invoking installation.
A panel cannot select another installation, supply executable paths or pass launch arguments.
Trusted direct engine/CLI callers must provide the installed `id`. Restricted automation
denies both direct calls and tasks nested in `extension.invoke`.

No open document is required. The host runs work asynchronously when the UI starts it, shows
progress and cancellation, and exposes the normal `jobs.list`/`jobs.cancel` APIs. A native
installation cannot start another task or native image operation until its current work and
cancel drain finish. Updating/removing the installation also waits for its native leases.
Current desktop instances hold a shared OS store lock while executing the worker and
recheck its stored generation before committing task state. Management operations on the
same store return busy during native work. See [store locking](plugin-safety.md#cooperative-store-locking)
for the first Phase 2 slice's coarse scope and remaining lifecycle limits.

## Worker protocol

The host launches only the owning package's installed executable, without a shell:

```text
worker --task <absolute path to host-owned task.json>
```

The current directory is the executable's directory. The request is UTF-8 JSON:

```json
{
  "apiVersion": 1,
  "options": {"action": "refresh"},
  "output": "<absolute host-owned path to result.json>"
}
```

Validate your options strictly. Write a regular UTF-8 JSON file to exactly `output`, then exit
successfully. The result must be at most 128 KiB, with no additional top-level fields:

```json
{"apiVersion":1,"message":"Assets are ready.","data":{"ready":true}}
```

`message` must contain non-whitespace text and fit within 8192 UTF-8 bytes. `data` must be an
object. The panel shows the message; the engine job result includes `id`, `task: true`,
`message` and `data`. Data is not injected into guest state or rendered as arbitrary UI.
The guest's proposed state is committed only after successful completion and installation
revalidation. Errors, cancellation and revoked results leave its previous state intact.
The task also captures the guest state at launch. An unchanged proposal does not write state;
if it proposes a change while a newer panel action changed the original state, completion
rejects that stale proposal instead of overwriting the newer state. Native side effects may
already have completed; the resulting error explains that the proposed state was not committed.

Progress uses the same stderr protocol as image workers:

```json
{"progress":45,"message":"Checking assets"}
```

The normal 45-minute deadline, 16 MiB diagnostic limit, bounded log parsing and cancellation
apply. Read stdin concurrently: stop on `cancel\n` or EOF, terminate/reap your own descendants,
remove partial output, then exit promptly. The host allows ten seconds for cooperative cleanup
before terminating the direct worker. See [the image transport contract](plugin-image-processing.md)
for the shared process and diagnostic behavior.

## Document and trust boundaries

Task results cannot dispatch editor commands, create previews, open images or alter history.
An existing comparison remains available while a task runs; image processing still uses the
separate `plugin.processImage`/Apply transaction. Closing a comparison panel cancels its
current task, as it does current image processing. Compact panels can continue jobs when closed.
Disabling/re-enabling a package revokes outstanding task results. Native side effects that
already happened are the plugin's responsibility and cannot be rolled back by the host.

Previews also bind to the guest state present when they were requested. A successful task
that changes that state makes the older preview stale, so Apply cannot overwrite the newer
task state with the preview's proposed state. Tasks returning the same state preserve a
valid comparison. Generate a fresh preview after an intentional state change.

Native code retains the user's operating-system privileges, including filesystem and network
access. This API is not a sandbox or a generic URL downloader. Plugin authors own download
consent, trusted endpoints, asset licensing, checksums, bounded streaming, disk-space failures,
safe cache paths, atomic installation and cleanup. Host package-size reporting covers the
immutable installed payload; separately downloaded assets must be reported by the plugin.

Test no-image operation, invalid options/results, cancellation/EOF, offline/full-disk failures,
update/remove leases, recovery mode, restricted automation, restart, and preservation of an
existing preview. Include independent worker tests and an actual installed-package smoke test.
