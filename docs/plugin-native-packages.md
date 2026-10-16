# Plugin package author reference

Use a version-2 package for an independent panel, native image worker, packaged WASM filters
and their assets. The public `plugin.processImage` capability runs your own image algorithm
without host changes. The [image-processing tutorial](plugin-image-processing.md) and neutral
Image Gain starter provide the complete developer workflow. A panel invoking existing commands
can also remain a single `.pcext` file; a standalone [filter ABI](plugins.md) module can remain
a `.wasm` file. Start with [plugin development](plugin-development.md). The [extension API reference](extensions.md) defines the guest ABI,
manifest, permissions and event messages.

PhotoCraft's core supplies the shared host contract only. Algorithms, model names, option
validation, inference engines and non-system runtime dependencies belong to each package.

## General package version 2

Every package has `package.json`, `plugin.pcext`, `LICENSE` and `THIRD-PARTY-NOTICES.md` at its
root. All must be nonempty. Identity comes from the panel's embedded manifest. Metadata is:

```json
{
  "packageVersion": 2,
  "module": "plugin.pcext",
  "native": {
    "service": "plugin.processImage",
    "platform": "windows-x86_64",
    "entry": "my-image-worker.exe"
  },
  "filters": [{"module": "filters/my-filter.wasm"}]
}
```

`packageVersion` and `module` are required; `module` is exactly `plugin.pcext`. Both `native`
and `filters` are optional, so panel-only, native-only, filter-only and combined backend
packages are valid. "Native-only" and "filter-only" still include the panel module. Unknown
metadata fields are rejected; no install script or automatic dependency download is supported.

When `native` is present, all three of its fields are required. `service` is exactly
`plugin.processImage`, and `entry` is a safe relative archive path to your executable. It is
not tied to a built-in filename or model list. Windows entries must end in `.exe`. Accepted declared targets are `windows-x86_64`,
`windows-aarch64`, `linux-x86_64`, `linux-aarch64`, `macos-x86_64` and `macos-aarch64`; the host
must match. Only Windows x64 is verified here. Supply and test native dependencies for each
published target. Native-code consent is required; the process has the user's OS privileges.

`filters` is an array of at most 16 objects, each with exactly a `module` path. Each referenced
module is loaded/validated with the existing filter ABI and bounded sandbox. Its embedded
manifest supplies its processor id. Ids must be unique within the package, and `native` is
reserved. Modules remain private to this managed installation; no global registry insertion
or access to another plugin's backend is implied. Paths/required members must not conflict.
Panel modules retain the 8 MiB module limit; bundled filters use the filter loader's 32 MiB
default limit. These are separate from the archive's 256 MiB per-file maximum.

Processing panels declare `plugin.processImage` in `permissions.commands`. The host binds a
panel request to that installation, then resolves `processor: "native"` or a bundled filter
manifest id. Direct trusted engine callers additionally provide the installed panel `id`.
No guest-supplied paths, launch arguments, native service registration or cross-plugin id
selection are accepted. A raw panel cannot borrow this capability from another installation.

The generic command accepts `options: {}`, optional native output `width`/`height` and
`preview` (default true). Its native JSON request has `apiVersion: 1`, `input`, `output`,
`options`, `outputSize: {width,height}` and `colorSpace: "srgb"`. Input is 8-bit RGBA sRGB;
output accepts 8-bit RGB or RGBA PNG, with RGBA sRGB recommended to preserve alpha. The host
converts the flattened supported source before processing. A bundled
filter receives that same composite, retains its dimensions, and is blended through source
selection coverage. Native processing receives the whole composite without a selection mask.
See [the complete command/worker contract](plugin-image-processing.md) for limits and lifecycle.

Packages with a native worker may additionally request `plugin.runTask` for explicit
document-independent maintenance. It invokes that same owned executable using `--task`,
with bounded JSON options/results and the same cancellation/diagnostic limits; see
[native plugin tasks](plugin-native-tasks.md). The package descriptor remains version 2 and
its native service remains `plugin.processImage`. No raw panel can borrow another package's worker.

All packages use the safe ZIP paths and finite archive limits documented below. The common
managed installation, integrity, review-hash, replacement, repair, leases and removal rules
also apply. The Python helper at `scripts/plugins/package-plugin.py` creates version-2
packages from a prepared directory without running plugin code. It performs structural
checks; PhotoCraft performs actual WASM/API validation. Package install requires a persistent
desktop store, including panel-only version 2; browser package installation is unavailable.

## Identity and compatibility

A `.pcplugin` is an ordinary ZIP with `package.json` and `plugin.pcext` at the archive root.
Do not wrap them in an enclosing directory. The embedded panel manifest supplies the id,
display name, release version, author, controls and permissions. The selected native service
must also appear in `permissions.commands`. The manifest must implement the full panel ABI;
an external JSON file cannot replace its `pce_manifest` export.

The package schema version, panel API version, filter ABI version, native job protocol version
and author's release version are independent. General packages use `packageVersion: 2` while
the panel, filter and native job protocols remain version 1.

The earlier version-1 **package format** and its two fixed AI services have been removed.
Rebuild and reinstall those plugins as version-2 packages under the same panel id. Old managed
packages are skipped and reported at startup; their files are retained for replacement through
the manager. There is no silent conversion or fallback. Raw `.pcext` panels and standalone
`.wasm` filters retain their existing ABI version 1 support.

The worker and its dependencies are package-owned. The host does not know model filenames,
model ids, inference libraries, algorithm settings or per-algorithm limits. It validates the
shared input/output boundary and installation ownership. A plugin worker must validate its
own options and assets and fail clearly when they are incompatible. Bundle redistributable
non-system libraries next to the executable or according to the platform loader's documented
rules. Ordinary operating-system facilities and hardware drivers remain platform prerequisites.

## Archive paths and limits

Use ordinary single-disk ZIP files with STORE (method 0) or DEFLATE (method 8) members.
Encrypted entries are rejected; ZIP64 and other compression methods are outside the supported
format. Include file entries only, with forward-slash relative paths. Explicit directory
entries such as `runtime/` are invalid.

Every member name must meet all of these rules:

- Between 1 and 240 bytes, using only ASCII letters, digits, `.`, `_`, `-` and `/` separators.
- Each path component is 1–128 bytes. Empty components, `.` and `..` are rejected.
- No leading `/`, backslash or colon; no component ending in a dot or space.
- No Windows device-name stem, checked without case and before the first dot: `CON`, `PRN`,
  `AUX`, `NUL`, `CLOCK$`, `CONIN$`, `CONOUT$`, `COM1`–`COM9` or `LPT1`–`LPT9`.
- No duplicate or case-aliased full path, such as `LICENSE` and `license`.
- No member that is also another member's parent, such as files named `runtime` and
  `runtime/model.bin`.
- No root member named `.photocraft-install.json`; that exact name is host-owned metadata.

The installer creates regular files from member bytes. ZIP attributes do not grant a way to
create symlinks. A selected store root may itself resolve through a link, but the host
canonicalizes that root and rejects symlinks/reparse points in managed descendant paths.

| Resource | Limit |
|---|---|
| Entire source archive | 512 MiB, including ZIP overhead |
| Member count | 1–512; the selected required layout must also be present |
| Total decompressed member bytes | 1 GiB |
| Each non-module member | 256 MiB |
| `package.json` | 64 KiB |
| `plugin.pcext` | 8 MiB |
| Any member's decompressed content | Must be nonempty |
| Host installation record | 512 KiB |
| Installed extensions | 128 per session, including raw guests |

MiB and GiB here are binary units. Declared ZIP sizes are not trusted as proof of actual
size. Each member is decompressed within its applicable remaining budget, checked for exact
decompressed length and CRC-32, and hashed before extraction starts. Invalid central-directory
or local-header offsets, truncated data, incorrect lengths and CRC failures return errors.
The WASM module is separately validated after archive inspection. Package integrity checks do
not turn native code into sandboxed code.

## Build a package

First build and test the guest as a standalone extension. Its source can live outside the
PhotoCraft workspace; it communicates through the versioned guest ABI rather than linking to
engine or UI crates. Native worker implementation, runtime dependencies and assets must implement
the generic image contract and be complete within the package.

A packager should perform this sequence:

1. Assemble a clean staging tree containing the exact root metadata, guest, worker, runtime
   assets and notices. Resolve dependencies at build time; installation does not fetch them.
2. Parse `package.json` and validate its exact fields and declared backend paths. Read the guest's
   actual embedded manifest and verify identity and matching service permission.
3. Enumerate regular files, using forward-slash relative names. Reject source links, invalid
   paths, case aliases, empty files, missing required members and size/count violations.
4. Write each file as a STORE or DEFLATE ZIP member. Omit directory entries and an enclosing
   staging-folder prefix. Use stable member ordering and timestamps if reproducible archive
   hashes are desired. Close the ZIP before reviewing or hashing it.
5. On a supported PhotoCraft desktop build, run `extension.inspect` against the final local
   archive, then install that exact reviewed content through the manager.
6. Exercise the declared action, cancellation, worker failure, restart, update and removal
   with realistic source images. Layout validation alone cannot test native processing.

There is no author-maintained hash manifest to generate for `package.json`. ZIP members carry
their standard CRC-32 values. PhotoCraft computes **BLAKE3**, not SHA-256, for both the complete
archive and each decompressed file. An optional publisher-provided SHA-256 download checksum
is a separate distribution artifact; it cannot be supplied as `expectedHash`.

Changing ZIP compression, timestamps, ordering or any other archive bytes changes the archive's
BLAKE3 identity even when decompressed payloads are identical. Review the final distribution
archive after packaging is complete.

## Review, native-code grant and installation

The desktop manager's **Browse…**, file-drop and **Review plugin** flows call `extension.inspect`.
Its result contains `manifest`, the whole-input BLAKE3 `hash`, parsed `package` metadata,
the selected file's `downloadBytes`, and `storage: {installedBytes, modelBytes}`. Installed
bytes are the sum of the validated, uncompressed payload files. Model bytes are the subset
under `runtime/models/`, including model configuration files; they are already included in
the installed total. Manage Plugins shows these sizes during review and on installed entries.
Installed entries also expose `storage` through `extension.list`, including after a restart.
These are logical file sizes, excluding temporary processing files, installation bookkeeping
and filesystem allocation overhead. Standalone `.pcext` files report their module length and
zero model bytes. Model files downloaded separately outside the managed payload are not counted.
When model bytes are zero, the manager hides the Models text and still shows Installed size.
The manager shows the plugin identity, requested commands and the native-code disclosure. When
the user installs it, the manager passes that hash as `expectedHash`, `allowNative: true`, and
the selected replacement flag to `extension.install`.

Trusted engine callers can use the same two-step flow:

| Command | Inputs relevant to native packages | Result or rule |
|---|---|---|
| `extension.inspect` | `path` to the final local archive | Validates package and guest; returns manifest, BLAKE3 hash and package metadata |
| `extension.install` | `path`, `allowNative: true`, returned `expectedHash`; `replace: true` for an installed id | Validates again, checks reviewed bytes and installs into the attached store |
| `extension.list` | `{}` | Installed manifests, enabled state, package metadata and startup/cleanup errors |
| `extension.enable` | `id`, boolean `enabled` | Persists enabled state; disabling invalidates retained comparisons |
| `extension.remove` | `id` | Removes the active installation and its owned payload, subject to leases |

Native packages require a local `path` and an attached persistent plugin directory. The base64
`data` route is for raw guests; a ZIP supplied that way is rejected. Desktop PhotoCraft attaches
its `Extensions/` settings directory. A trusted Rust integration must attach its own persistent
store before installing a native package.

`allowNative: true` is required for every native installation, including replacement.
The engine checks `expectedHash` when supplied as a string and rejects content changed since
inspection. The manager always supplies it for reviewed content; trusted local engine callers
can omit it. Human review is therefore a manager workflow and an explicit native-code grant,
not publisher authentication or a signature system. Unknown permissions reject inspection;
native consent does not expand the guest command allowlist.

Restricted automation cannot inspect ambient extension paths, install/enable/remove persistent
extensions, invoke native image processing, or apply/discard native previews. It can inspect
raw guest data in memory and invoke permitted installed guest actions subject to the caller's
authorization hook. A guest action's proposed native command is checked by that hook as well.

## Storage, integrity and updates

The installer manages payload generations beneath the attached store. For a guest whose
embedded id is `org.example.native-tool`, the layout is:

```text
Extensions/
  org.example.native-tool.installation.json
  org.example.native-tool.disabled             # exists only when disabled
  bundles/
    org.example.native-tool/
      <whole-archive-blake3>/
        .photocraft-install.json
        package.json
        plugin.pcext
        ...recorded worker and assets...
```

The angle-bracket name above denotes the actual 64-character lowercase BLAKE3 hex digest,
not a literal directory name. These files are internal host metadata, not files to put in the
source archive or edit as an update mechanism.

The active record has format version 1, the guest id, archive hash, parsed package metadata,
and each owned member's relative path, decompressed byte size and BLAKE3 hash. The generation's
ownership marker must match the active record exactly. Installation writes a temporary sibling
generation, synchronizes its files and marker, renames it into place, verifies the result, and
publishes the small active record last with an atomic write. Validation or extraction failures
leave the previous active record selected. After successful publication, the host attempts to
clean the old generation; cleanup failures are reported as warnings.

At startup or explicit folder attachment, the host validates the active record and ownership
marker, then reads and verifies every recorded member's exact size and BLAKE3 hash. It reloads
and validates the guest, permissions and id against the record before registering the package.
Changed, missing or invalid payloads are skipped and reported. An authoritative installation
record prevents falling back to a neighboring legacy `<id>.pcext` after corruption. These checks
occur on installation and load; the loader does not promise to rehash every payload before
each worker launch.

Reinstalling the identical valid archive can repair its damaged owned generation, subject to
ownership/path checks. Do not modify installed assets in place. Publish a rebuilt package and
replace the installation instead. After successful installation, PhotoCraft uses its managed
copy; the original download can be moved or deleted.

An update is identified by the embedded guest id, not by comparing display-version strings.
An existing registered id requires explicit `replace: true`; there is no automatic semantic
version ordering or downgrade prohibition. Replacement preserves enabled/disabled state,
resets the guest's JSON state, and invalidates retained previews. Changing the id creates a
different plugin. Raw guests and native packages can replace one another under the same id,
with native consent required when the replacement contains native code.

Removal deletes any legacy standalone guest before removing the active record, then deletes
only files listed in the matching ownership record and their empty directories. It also removes
the disabled marker. Cleanup does not recursively delete arbitrary neighboring files. Unexpected
files, ownership conflicts or filesystem errors can leave leftovers and a reported warning;
they do not authorize deleting unrelated data.

## Installation leases and result lifetime

Current desktop hosts also coordinate through a shared OS store lock during native work,
with exclusive locking for installation/repair, enable/disable and removal. A busy operation
fails without changing the selected installation. The initial lock covers the whole store,
so wait for native work to finish before managing any plugin there. Completion and Apply
recheck the stored hash/package, activation epoch and disabled marker. Completed comparisons release the OS
lease. See [store safety and remaining limits](plugin-safety.md#cooperative-store-locking),
including older hosts, instance reloads and retained activation tombstones.

The registered package and jobs hold shared `Arc<InstalledNative>` handles. Replacement,
removal and reattaching the plugin folder require the registry to be the only remaining strong
owner. A running job keeps the installation leased through processing and its completion
closure, so this session's installer cannot remove its executable and assets underneath it. Requesting cancellation
does not immediately release the lease: wait for the worker and job completion to finish.

A completed comparison uses a weak native handle, so viewing an unapplied result does not
keep the installation leased. Disabling, replacing, removing or reloading its plugin invalidates
the retained result. Apply checks the enabled guest and native installation identity again,
as well as the source identity/revision. Result and proposed guest-state admission are handled
by the shared processing command; failed, cancelled or stale-source processing does not import a successful
result. The [extension reference](extensions.md) describes the host-owned preview workflow.

When a native command originates from a guest action, the host binds it to that exact enabled
guest instance and its own installed backend. A raw guest cannot borrow another
plugin's engine merely by declaring the same command, and a package cannot select another
installation's processor. Trusted direct calls supply the installed panel `id` explicitly.

## Native worker transport boundary

The shared transport is used by version-2 `plugin.processImage`.
Its supervision, cancellation and result validation are independent of the plugin algorithm.
Workers implement the generic contract in the
[image-processing guide](plugin-image-processing.md#native-worker-protocol).

The host validates the source dimensions and request structure before starting this transport. It creates
one temporary directory, exports the source snapshot as `source.png`, chooses `result.png`,
and writes `job.json`. The request fields are:

| Field | Value supplied by the host |
|---|---|
| `apiVersion` | `1` for this request format |
| `input` | Absolute path to the exported `source.png` |
| `output` | Absolute path selected for `result.png` |
| `options` | Plugin-owned JSON object; the worker validates its contents |
| `outputSize` | Exact requested `{width,height}` |
| `colorSpace` | `"srgb"` |

Input is normalized 8-bit RGBA sRGB, and the worker must return 8-bit RGB or
RGBA PNG at the specified size; use RGBA sRGB to preserve alpha. Options are an opaque
object to the core; each worker owns their schema, defaults, bounds and model selection.

This request version is separate from `packageVersion` and the guest `apiVersion`. The host
starts the installed worker directly with `--job` followed by the host-created JSON path,
using the installed worker executable's containing directory as its working directory. For a
version-2 entry in a subdirectory this is not the package root; resolve relative assets using
your declared layout. It does not use a shell.
On Windows it requests no console window. Standard input is piped; standard error is captured
in a temporary regular file and standard output is discarded. The guest cannot choose any of
these paths or process arguments.

Workers report progress on standard error as newline-delimited JSON containing numeric
`progress` and string `message` fields. Finite progress is interpreted as a percentage, clamped
to 0–100, and mapped into the processing part of the host job. Non-JSON lines are diagnostics,
not return values or commands. A structured `{"error":"…"}` diagnostic is retained as bounded
failure text too; workers can use it for an actionable message. A progress line has the structural form:

```json
{"progress": 50, "message": "Processing"}
```

Diagnostic collection is bounded: at most 8,192 bytes are buffered for an incoming line, each
poll reads at most 64 KiB, and failure context retains at most 16 lines / 8,192 bytes
with each retained line truncated to 2,048 bytes. The collector preserves the first line
containing `error` or `failed` and bounded following context/tail. A log above 16 MiB causes
processing to stop. The final drain snapshots one bounded file length; an inherited stderr
handle held by a child cannot leave the host waiting indefinitely for EOF.

On cancellation or after 45 minutes, the host writes `cancel\n` to worker stdin and closes
that pipe. A cooperative worker should stop processing, stop/reap any child engine, and exit;
it should also treat loss of host stdin as cancellation. After 10 seconds without exit, the
transport requests termination of the worker process. That is not a general descendant-process
sandbox: cancellation of any engine spawned by the worker is the worker's responsibility.
The diagnostic byte limit is monitored during the job; it is not an OS disk quota for descendants.
The host waits for worker termination and diagnostics before admitting a result or returning
the job error.

See [safety and recovery](plugin-safety.md) for the recovery launch option, damaged-record repair,
native trust boundaries and the current coarse cross-process store locking limits.

A successful exit is necessary but insufficient. The host requires a regular output file
within the shared byte limit, then checks the PNG header, exact expected dimensions,
8-bit depth and RGB/RGBA encoding before decoding. It reads within the same bounded size,
imports the PNG and verifies dimensions/depth again. The byte ceiling is
`min(1 GiB, 4 × expected pixels + 64 MiB)`. Source and target image/memory
limits are documented in [the processing contract](plugin-image-processing.md#pixels-dimensions-and-results).
The host supplies the result document name and retains source DPI. Temporary job files are
removed when the transport finishes.

No result PNG, status line or worker exit can add commands, permissions or services. Adding
a different file format, stream protocol or capability beyond the general image contract requires
an explicit host implementation and review.

## Source of truth

| Source | Contract |
|---|---|
| [`extension_package.rs`](../crates/engine/src/extension_package.rs) and [its tests](../crates/engine/src/extension_package/tests.rs) | Metadata, declared backends, required members, paths, extraction, generations and ownership |
| [`extension_cmds.rs`](../crates/engine/src/extension_cmds.rs) | Permission checks, inspection hash, native grant, store attachment, instance binding and leases |
| [`extension_ui.rs`](../crates/ui-egui/src/extension_ui.rs) | Manager review and installation workflow |
| [`plugin_runtime.rs`](../crates/engine/src/plugin_runtime.rs) | Shared native image transport, bounded diagnostics, cancellation and PNG validation |
| [`plugin_preview.rs`](../crates/engine/src/plugin_preview.rs) | Completed-result identity, source validation and weak installation binding |
| [`workspace.rs`](../crates/automation/src/workspace.rs) | Restricted automation's ambient-path/native-process authorization |

Return to the [plugin development guide](plugin-development.md) for authoring and iteration,
or the [extension API reference](extensions.md) for the exact guest ABI and command schemas.
