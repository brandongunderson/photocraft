# Image Gain package project

This guide applies to the whole exported project, including `native/` and `filter/`.
Read [README.md](README.md), the manifests, and `build.py` before changing behavior.

## Ownership and files

Keep this plugin independent of PhotoCraft editor crates, sibling plugins, global
inference runtimes, and inherited editor workspaces. Root, `native/`, and `filter/`
are separate Cargo projects with locks. Own algorithms, option validation, assets,
dependencies, and notices here. Local dependencies belong within this source export.

`manifest.json` and `src/handler.rs` define the panel. `src/abi.rs` contains bounded
panel storage. `native/src/main.rs` owns native processing and cancellation; `filter/`
owns its WASM algorithm. `package.json` declares package 2 and its backends.
Keep changes to metadata, handler requests, processor IDs, and build artifacts aligned.
Before distributing a derivative, replace example IDs/authorship and release metadata.
Changing embedded JSON requires rebuilding. Preserve ABI helpers and license files.

## Current API boundaries

The panel uses the PhotoCraft Plugin API (`pce_*`, discriminator 2), the bundled
filter ABI 1 (`pc_*`), package format 2, and worker protocol 1. The handler parses
typed host event metadata and declares `host.commands.invoke` plus its exact command
grant. Additive envelope fields are allowed; incompatible protocol versions fail.
See `docs/plugin-api.md` in the matching source/kit. Obsolete panel modules must
be rebuilt; do not restore a compatibility adapter in the editor. WASM guests are import-free; no WASI, filesystem/network,
arbitrary drawing, or persistent instance state is provided. Native controls are
number/int/bool/choice fields and buttons. Do not infer proposed package/worker APIs.

The panel grants `plugin.processImage` and returns it as its sole command, normally
with `preview: true`. Omit the installation `id`, executable paths, and launch args.
`processor: "native"` chooses this package's worker; other processor strings must
match this package's filter manifest IDs. Private filters are not global plugins.

Both backends process the visible flattened 8-bit RGBA sRGB composite, converted
from supported 8/16-bit RGB/grayscale documents. Native input contains no separate
selection mask. The host blends bundled WASM output through selection coverage.
Standalone filter precision/target behavior is different. Do not advertise native
16/32-bit, per-layer, or mask transport that this interface does not expose.

The worker receives `--job` and a host-owned protocol-1 JSON request with input/output
paths, options, `outputSize`, and `colorSpace`. Validate it and write an exact-size
8-bit RGB/RGBA sRGB PNG. Preserve alpha deliberately. Report stderr JSON progress,
read stdin concurrently, stop on `cancel\n` or EOF, and stop/reap owned descendants.
The host's limits do not sandbox native OS access or replace algorithm-specific bounds.

Use `plugin.runTask` only when adding explicit maintenance to this same native
package, with that additional grant and the separate `--task` protocol. Tasks work
without documents and return bounded status/data, not editing commands or previews.
Do not add a model-specific service to the editor for either route.

## Build and acceptance

Use Rust 1.95+, its native linker, the WASM target, and Python 3.11+. From this root:

```sh
python build.py --check
python tools/test_package.py
```

`--check` checks/tests/lints all three Cargo projects before release builds and
packaging. `--offline` uses cached dependencies; Windows `./build.ps1 -Check` and
POSIX `sh build.sh --check` are wrappers. Output is
`dist/Image-Gain-<platform>.pcplugin`. Build natively for each declared platform;
accepted target names do not establish compatibility. Windows x64 is verified here.

Retain dependency/model licenses and bundle non-system native dependencies. Verify
an exported source build and an extracted package with a controlled PATH. When the
optional conformance executable is available, audit the project and check the
package; structural validation does not run the algorithm or prove dependency closure.

Test actual PNG input/output, dimensions, alpha, malformed options/jobs, missing
assets, and cancellation. In a compatible desktop host, use disposable documents
to exercise each backend, previews, Apply/Discard, stale-source refusal, Undo/Redo,
and relevant disable/update/remove behavior. Same-size layer Apply preserves the
original layers underneath and hides them in the same undo step to preserve alpha.
Restricted MCP denies installation and `plugin.processImage`/`plugin.runTask`, even
when nested in an invocation. Use a separately authorized local test route or
report blocked acceptance; do not evade policy. Deliver exact source/package paths,
requirements, verified checks, and untested behavior. Preserve existing user authorization.

## Optional typed SDK authoring

This project implements the current API directly; the SDK is optional.
For a new typed project, `photocraft-plugin new AUTHORING_JSON FRESH_PROJECT` supplies
private API/SDK 0.1 sources, locks, licenses and a local guide. Edit `authoring.json`
and the handler; generated manifests/bindings belong in Cargo `OUT_DIR`. Keep local
SDK dependencies inside that project and retain its release hash index. Do not link
to engine/UI sources or introduce another plugin or companion as a runtime dependency.
The SDK is optional author tooling; availability/discovery never grants authority.

Current desktop hosts use lazy descriptor discovery, bounded queues and native resource
reservations, per-generation OS leases, revocable activation epochs and bounded preview retention.
Worker 2 is optional, selected by runtime/worker-v2.json; worker 1 keeps its exact launch contract.
Each plugin owns its persistent process and generation-scoped managed data. Follow the matching
source/kit lifecycle, worker-2 and managed-storage docs before adopting migrations or rollback.
Never modify host lock files/tombstones or another plugin's runtime/data. Admission estimates
are not OS resource containment; restricted native remains unavailable. Keep this guide and
project docs aligned with completed features. Existing plugins require no SDK migration.
