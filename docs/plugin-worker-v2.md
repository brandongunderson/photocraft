# Persistent native worker protocol 2

**Status:** implementation contract, 2026-10-16. This protocol is opt-in. Packages without the
verified declaration below continue to use the unchanged `--job` / `--task` worker protocol 1.
Package format 2, panel API, task/image payload versions and this transport version are independent.

## Declaration and ownership

An existing package-2 native payload opts in by including the regular, integrity-indexed member
`runtime/worker-v2.json`. The host validates its bytes before accepting the installation and
loads the verified declaration with that immutable generation. It never trusts a loose external
sidecar or probes a legacy executable to guess its protocol. No new mandatory package-2 fields
or filenames for existing workers are introduced.

```json
{
  "protocolVersion": 2,
  "executionProfile": "trustedNative",
  "operations": ["image", "task"],
  "idleTimeoutMs": 60000,
  "requestTimeoutMs": 2700000,
  "resources": {
    "cpuUnits": 1,
    "memoryBytes": 536870912,
    "gpuBytes": 0,
    "tempBytes": 268435456,
    "cacheBytes": 0
  }
}
```

The declaration is at most 16 KiB. Unknown fields, duplicate operations and unsupported versions
or profiles fail installation. `operations` contains one or both of `image` and `task`.
`idleTimeoutMs` defaults to 60,000 and is within 1,000–300,000; `requestTimeoutMs` defaults to
2,700,000 and is within 100–2,700,000. CPU units are 1–64, memory is 1 byte–64 GiB, GPU storage
is 0–64 GiB, temporary disk is 0–256 GiB and managed cache is 0–1 TiB. Host admission may impose
lower aggregate limits. These are admission declarations, not OS enforcement of native memory,
CPU, GPU, filesystem or network use. The only available profile here is trusted native execution
with the user's OS privileges. Restricted containment remains unavailable.

Each plugin identity and immutable installed generation owns a supervisor within its host runtime.
Independent sessions receive separate scheduling and resident-resource identities even when they
open the same private store and generation. Processes and runtime state are never pooled across
unrelated plugins or generations. Cloning an installation may
share that installation's supervisor; reactivation creates a new supervisor and revokes the old
one. The process retains its generation lease and resident memory/GPU/cache admission until it
and its transport resources have exited. Per-job admission separately covers CPU and temporary
work. No editor/document lock is held while waiting for a worker or admission.

## Framing and handshake

Launch the installed executable directly, without a shell, with the sole transport argument
`--serve`. Set its working directory to its installed executable directory. Stdout is exclusively
the protocol channel; diagnostics go to stderr. Every stdin/stdout message is:

1. Exactly four bytes containing an unsigned little-endian payload length.
2. Exactly that many bytes of UTF-8 JSON, with no newline delimiter or trailing bytes.

Length must be 1–262,144 bytes. Reject oversized lengths before allocation, malformed UTF-8/JSON,
unknown fields/types, invalid values, unexpected IDs and out-of-order frames. There is one active
request per process and a bounded inbound queue. Bulk images travel through host-owned files,
never JSON or base64. A frame flood stops the process instead of growing an unbounded queue.

The host sends the first frame, within a 10-second handshake deadline:

```json
{"type":"hello","id":"0","protocolVersion":2,"pluginId":"org.example.effect","generation":"012345...","operations":["image","task"],"maxFrameBytes":262144}
```

The worker must reply with:

```json
{"type":"ready","id":"0","protocolVersion":2,"pluginId":"org.example.effect","generation":"012345...","operations":["image","task"]}
```

The version, ID and echoed owner/generation must match; all declared operations must be available.
This echo binds a conversation, not publisher identity or a new permission grant. A mismatch fails
without falling back to protocol 1 or a different execution profile. After `ready`, host request
IDs are increasing decimal strings, distinct for the lifetime of that process; `0` is reserved.
Perform expensive model or service initialization during the first `run`, with progress replies;
`ready` establishes protocol readiness within the short handshake deadline.

Hello/run frames may include optional `managedStorage`, an object of at most 32 KiB containing
host-selected generation-scoped settings/data/cache paths, schema and quota metadata, and secret
references when configured. It contains no new grant or plaintext secret requirement. The host
prepares managed storage before activating the generation. Legacy workers receive no added keys
and keep their existing storage behavior.
The host revalidates managed paths and quotas before each job and after a successful worker
reply, before decoding its output. A failed validation rejects the result and revokes that
runtime. The configured cache quota cannot exceed the worker's declared cache admission.

## Requests and replies

An image request uses operation `image` and the existing image job JSON as its `request` object.
A task uses `task` and the existing task JSON. The host owns input, output and request directories;
the process must finish all access to a job's files before its terminal reply.

```json
{"type":"run","id":"1","operation":"task","request":{"apiVersion":1,"options":{},"output":"C:/host-private/result.json"}}
```

While active, the worker may send bounded status messages:

```json
{"type":"progress","id":"1","fraction":0.5,"message":"Processing"}
```

`fraction` is finite and within 0–1; `message` is at most 1024 UTF-8 bytes. The terminal success
reply is `{"type":"result","id":"1"}`. The host then validates output files using the same
dimensions/depth/task-result rules as protocol 1 before accepting anything into the editor.
A normal feature failure is `{"type":"error","id":"1","error":{"code":"invalidJson","message":"Invalid options","details":{}}}`.
The error message is at most 8192 bytes and serialized details at most 16 KiB. A reported feature
error does not by itself declare the process corrupt. A wrong ID/type, malformed reply or process
failure invalidates the process; no stale reply can complete a later request.

## Cancellation, deadlines and reset

Cancellation sends `{"type":"cancel","id":"1"}` for the active request. The worker stops
using its job files and replies `{"type":"cancelled","id":"1"}` within two seconds. A result
arriving after cancellation is discarded. Cancellation without a clean acknowledgement retires
the process; it is never reused with ambiguous outstanding messages. EOF on host stdin means the
worker must stop work and exit. Requests have the declared wall-clock deadline; a timeout follows
the same cooperative cancellation then direct-child termination path and reports a timeout.

An idle reset sends `{"type":"reset","id":"2"}` and requires
`{"type":"resetComplete","id":"2"}` within ten seconds. It discards plugin-owned transient
runtime state; durable plugin data is not deleted. Reset is serialized with jobs. Explicit host
recovery also clears the supervisor's consecutive-failure quarantine and starts a fresh process.
The `extension.runtime` command takes `{"id":"org.example.effect","action":"status"}`;
`action` may also be `retry` or `reset`. Status and retry return immediately. Reset runs as
scheduled background work and rechecks installation authority before accepting completion.
Retry clears failure quarantine and retires the previous process; a revoked activation needs
disable/re-enable to obtain a new activation after its underlying problem has been corrected.

The host sends `{"type":"shutdown"}` during idle eviction/revocation and closes transport input,
then terminates an unresponsive direct child under a bounded deadline. Idle eviction runs
automatically without needing another plugin action. Disabling, removing or replacing a plugin
revokes outstanding results immediately; child cleanup proceeds separately. The trusted-native
profile does not promise descendant containment. Native plugins remain responsible for their
own descendants. Transport/process admission is bounded even if a descendant retains a pipe.

## Supervision and recovery

Three consecutive activation/transport/process failures quarantine that supervisor with the
plugin/generation and last cause visible in diagnostics. Successful requests clear the consecutive
counter. Cooperative user cancellation and ordinary reported feature errors do not count as
crashes. Explicit retry/re-enable creates a clean runtime; no automatic protocol or trust-mode
downgrade occurs. Idle timeout, reset, EOF, malformed framing, early process exit, cancellation,
handshake mismatch and repeated failure are exercised with real subprocess fixtures.

Stderr is read from a regular-file log with a 16 MiB diagnostic budget; observing that budget
exceeded stops the direct child. This is not an OS limit on writes by trusted code or its
descendants. A descendant retaining a write handle cannot block an EOF join. Native child
termination and transport readers/writers are supervised independently;
no synchronous pipe write, unbounded reader join or guest callback runs on the UI thread.


The optional API crate exports the descriptor and both frame enums. These are transport types
within the current developing plugin API; implementing another platform milestone does not
create another authoring API version. The host caps the managed cache quota by both its policy
and the worker's declared cache estimate, checks managed storage before a job and before
decoding a successful result, and retires a worker that violates those validation quotas.
These checks do not continuously limit OS writes made by trusted native code.
