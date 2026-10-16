# Build and acceptance workflow

Use the chosen starter's local `README.md`, `AGENTS.md`, and scripts. These instructions
also apply when the skill is installed without the PhotoCraft source checkout.

## Standalone build

Retain the starter's independent Cargo workspace, lockfiles, formatting, and licenses.
Use its declared Rust minimum (current starters: 1.95+) and the
`wasm32-unknown-unknown` target. Native tests/workers need the platform linker.
Python-based build/packaging helpers need Python 3.11+; do not introduce a Python
runtime requirement for users unless this plugin actually bundles and uses Python.

| Starter | Commands from the copied project root |
|---|---|
| Hello PhotoCraft panel | `cargo fmt -- --check`; `cargo test --locked`; `cargo clippy --locked --all-targets -- -D warnings`; then `./build.ps1` or `sh build.sh` |
| Channel Gain filter | Same three checks; then `./build.ps1` or `sh build.sh` |
| Image Gain package | `python build.py --check`; this checks/builds root, `native/`, and `filter/`; also `python tools/test_package.py` |
| Layer Organizer / Document Inspector | `python build.py --check` |

Hello PhotoCraft supports `-Offline` / `--offline`. Image Gain and the current API Python
builders accept `--offline`. Channel Gain's wrapper scripts currently take no
offline flag; invoke Cargo with appropriate flags when needed. Do not assume every
starter has the same command-line interface. Offline builds require cached crates
and toolchains. Do not relax locked resolution to conceal missing dependencies.

Use release modules to check ABI behavior. Copy the project to a fresh directory
outside the editor and build there to verify independence; omit build caches from
source exports. A native package must include its non-system dependency closure.
Test it with a controlled PATH and report driver/OS prerequisites. Accepted platform
metadata alone does not establish compatibility; only Windows x64 is verified by
the current source examples' recorded native-package acceptance.

## Optional conformance executable

These are command shapes; resolve the actual paths in the current environment:

```text
photocraft-plugin sdk FRESH_SDK_DIRECTORY
photocraft-plugin new AUTHORING_JSON FRESH_PROJECT
photocraft-plugin generate AUTHORING_JSON FRESH_OUTPUT
photocraft-plugin project SOURCE
photocraft-plugin check PACKAGE
photocraft-plugin build-export SOURCE FRESH_DESTINATION
photocraft-plugin pack STAGED_DIRECTORY FRESH_PACKAGE
photocraft-plugin probe PANEL REQUEST_JSON
photocraft-plugin hash FILE
```

`project` audits resolved Rust dependencies with locked offline Cargo metadata.
`check` validates package structure, actual WASM exports, grants, and native worker
headers. It does not run native code, install a package, prove dependency closure,
or certify algorithm quality. `probe` executes the sandboxed panel handler without
dispatching returned operations. Check each JSON report and exit status.

`build-export` accepts the standalone root-panel layout with optional `native/` and
`filter/` projects. It is not a universal builder for arbitrary plugin layouts.
Its clean build requires an explicit `PHOTOCRAFT_PLUGIN_CARGO` executable and
`PHOTOCRAFT_PLUGIN_BUILD_PATH` containing the required build tools. Outputs must be
fresh destinations. For other layouts, use the plugin's own build scripts and `pack`.
The developer tool may be supplied as a binary; plugins do not link to host source.

## MCP discovery and limits

Use an available PhotoCraft MCP connection; do not assume the server is connected
or create credentials/launch an editor unless the task calls for that setup.
Discover the connection's actual tool names. Current tools include `command_list`,
`command_run`, `command_batch`, `doc_inspect`, `render_preview`, `jobs_list`,
`jobs_cancel`, and desktop bridge `ui_inspect` / `ui_screenshot`.

Query host contracts through `command_run` where allowed:

```json
{"id":"extension.capabilities","params":{}}
```

```json
{"id":"extension.commands","params":{}}
```

`command_list` documents engine parameters and enabled states; `extension.commands`
is the panel grant allowlist. Neither discovery route supplies authority. The
current restricted automation policy denies managed extension installation/enable/removal,
`plugin.processImage`, `plugin.runTask`, and preview application, including those calls
nested in an otherwise allowed `extension.invoke`. A filesystem root does not
remove those restrictions. Do not retry through aliases, UI automation, an
unrestricted transport, or a different command to evade a refusal.

Use a separately authorized local/desktop test route if available, with explicit
native-code consent and a disposable store. Otherwise complete the build and
structural checks and identify runtime acceptance as untested/blocked. A raw WASM
inspection can be permitted from bytes; do not generalize that to native installation.
Standalone filter `plugin.install` from base64 `data` can also be permitted; its
filesystem `path` form is denied by the ambient-path policy. This is a different
operation from managed `extension.install`. Check the actual tool/command response
and authorization before using either route.
Headless and bridge sessions need not share installed plugins or document state.

## Functional evidence

Choose tests for the actual feature: no-document behavior, invalid settings and
assets, expected pixel/layer changes, alpha and selection behavior, supported depths,
progress and cancellation, source edits during processing, Apply/Discard, Undo/Redo,
disable/re-enable, replacement, restart, and removal as applicable.
Observer examples must remain read-only, and paginated reads must not silently
treat the first page as the entire document. Test the exact packaged artifact.

Use scratch documents. Inspect history after a failed command batch: MCP batches
are sequential and do not roll back earlier successes. File writes and native task
side effects are not reversed by editor Undo. Inspect effects before retrying a
request whose completion is uncertain. Report build, structural, handler-probe, and
real-host acceptance results separately, with host/API/platform versions.


For optional persistent workers, run real subprocess acceptance against the intended host:
strict framing/handshake, private process reuse, reset, cancel, deadlines, idle eviction and
quarantine recovery. Check `extension.commands` for scheduler limits and `extension.runtime`
for status. These are trusted host/control calls, outside guest command grants. Managed
migrations must leave the old code/data selected after failure; test explicit rollback and
source/destination lease contention. Preserve external caches and verify independent packaging.
Phase milestones evolve one current PhotoCraft Plugin API. Old panel version 1 is rejected;
test its migration to the current API in the plugin's own project. Keep filter/native transport
fixtures distinct. Native admission estimates are not OS hard limits.

## Release-update acceptance

When a plugin opts into GitHub Releases, test from an older installed version in a disposable
store. Check the exact stable asset name for every claimed platform, matching semantic tag
and embedded versions, SHA-256 metadata and downloaded bytes. Verify the available-update
indicator, preparation, permission/native review, install success/file selection reset and
paired data rollback. Treat network/rate-limit failures and missing assets/digests as errors
or unsupported updates rather than claiming the plugin is current. Local installation must
remain usable without the release service.

Exercise ID/version mismatch, incompatible packages, changed reviewed bytes and a competing
installation between review and install. Added permissions require review; a same-version
reinstall or intentional downgrade belongs to explicit maintenance. A successful HTTP response
does not establish installed behavior, native target compatibility or publisher signing trust.
Release checks/downloads are trusted host commands outside panel grants; current restricted
automation must not bypass its network/install restrictions through them.
