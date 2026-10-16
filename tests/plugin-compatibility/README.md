# Neutral plugin compatibility fixtures

These tests exercise the current PhotoCraft Plugin API, filter ABI 1, package format 2 and
native transport 1. The panel was explicitly migrated to the sole supported panel contract
on 2026-10-16. Its internal manifest/export discriminator is 2; obsolete panel modules are
not a compatibility requirement. The original Channel Gain filter and Image Gain worker
bytes remain unchanged to check their separate transport contracts.

`frozen/` contains fixed compiled modules. Normal tests never rebuild guest source. A future
explicit contract change requires a reviewed new fixture, provenance and catalog hashes.

Only neutral examples belong here. Independently released plugins keep their source, binary
fixtures, package hashes, model details, notices and package acceptance tests in their own
repositories.

`catalog.json` pins the two committed WASM modules and original neutral Image Gain native
executable. Its old archive hash is provenance only; that archive and its obsolete panel are
not test inputs. `panel-golden.json` records the current panel's manifest, typed host event
envelope and response. The
Channel Gain filter is exercised at 8-bit, 16-bit and float depths. Original notices are under
`licenses/`. None of these tests invokes a guest compiler.

## Portable compatibility

```sh
cargo test -p photocraft-engine --test plugin_compatibility
```

This self-contained target verifies fixed module bytes, panel responses, filter processing,
installation, restart after removing the downloaded copy, persistent enable state, previews,
Apply, one-step Undo/Redo, cancellation and repair of a corrupted installed module. The portable
package is a synthetic wrapper around the current panel and original filter.

## Neutral native worker

The original `Image-Gain-worker-windows-x86_64.exe` stays outside Git. Restore that exact
executable from a controlled artifact store. The runner verifies its pinned size and hashes; a missing
or mismatched artifact fails an explicitly selected native test. Default Rust runs report
the two native cases as **ignored**.

`PHOTOCRAFT_PLUGIN_COMPAT_ROOT` may point to a flat artifact directory or the outer
workspace containing `upstream/photocraft/`. The catalog records the retained workspace
location under `extension-review/plugin-current-api-fixtures/`.

```powershell
./scripts/plugins/test-compatibility.ps1 -ArtifactRoot C:/path/to/immutable-artifact-store
```

Tests wrap the pinned worker bytes with the current compiled panel, original filter and
neutral notices to create a temporary package. The runner exercises install/restart/cancel/
repair, PNG processing, preview, Apply and Undo/Redo. The worker targets Windows x86-64;
native cases reject another target. It needs no model download or inference runtime.

Tests use temporary installation stores. The runner isolates worker caches under
`target/plugin-compatibility-cache` and restores the shell's cache environment afterward.
Direct native processing tests require `PHOTOCRAFT_PLUGIN_COMPAT_CACHE` and an isolated
`LOCALAPPDATA` within it. No test attaches the user's real installed Extensions folder.

`native-v1.json` records generic protocol fields. These checks cover cancellation and result
rejection, without certifying a timed descendant-process cleanup guarantee or any independently
published plugin. Individual plugin repositories maintain their own release acceptance tests.
