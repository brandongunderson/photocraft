# Neutral plugin compatibility fixtures

These tests keep compiled Image Gain and Channel Gain examples from 2026-10-15 unchanged
to detect regressions in panel API 1, filter ABI 1, package format 2 and native protocol 1.
The `frozen/` directory means fixed test bytes: never rebuild a baseline from current source
to repair a failure. Add a new fixture with its own identity when a contract changes.

Only neutral examples belong here. Independently released plugins keep their source, binary
fixtures, package hashes, model details, notices and package acceptance tests in their own
repositories.

`catalog.json` pins the two committed WASM modules and the original neutral Image Gain native
archive. `panel-golden.json` records the neutral panel's manifest, request and response. The
Channel Gain filter is exercised at 8-bit, 16-bit and float depths. Original notices are under
`licenses/`. None of these tests invokes a guest compiler.

## Portable compatibility

```sh
cargo test -p photocraft-engine --test plugin_compatibility
```

This self-contained target verifies fixed module bytes, panel responses, filter processing,
installation, restart after removing the downloaded copy, persistent enable state, previews,
Apply, one-step Undo/Redo, cancellation and repair of a corrupted installed module. The portable
package is a synthetic wrapper around the original neutral panel and filter, rather than the
original native archive.

## Neutral native worker

The original `Image-Gain-windows-x86_64.pcplugin` stays outside Git. Restore the exact archive
from a controlled artifact store. The runner verifies its pinned size and hashes; a missing
or mismatched artifact fails an explicitly selected native test. Default Rust runs report
the two native cases as **ignored**.

`PHOTOCRAFT_PLUGIN_COMPAT_ROOT` may point to a flat artifact directory, or to the original
workspace containing `upstream/photocraft/examples/extensions/image-plugin-starter/dist/`.

```powershell
./scripts/plugins/test-compatibility.ps1 -ArtifactRoot C:/path/to/immutable-artifact-store
```

The runner exercises install/restart/cancel/repair and the neutral worker's PNG processing,
preview, Apply and Undo/Redo. This archive targets Windows x86-64; its tests reject another
target. The worker owns its dependencies and needs no model download or inference runtime.

Tests use temporary installation stores. The runner isolates worker caches under
`target/plugin-compatibility-cache` and restores the shell's cache environment afterward.
Direct native processing tests require `PHOTOCRAFT_PLUGIN_COMPAT_CACHE` and an isolated
`LOCALAPPDATA` within it. No test attaches the user's real installed Extensions folder.

`native-v1.json` records generic protocol fields. These checks cover cancellation and result
rejection, without certifying a timed descendant-process cleanup guarantee or any independently
published plugin. Individual plugin repositories maintain their own release acceptance tests.
