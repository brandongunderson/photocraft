# Channel Gain filter project

This file travels with this standalone project and applies to all its descendants.
Read [README.md](README.md), `manifest.json`, `src/lib.rs`, and the build scripts.

## Project and ABI boundary

- Keep the independent Cargo workspace, lockfile, lint/format files, and licenses.
  Do not link to editor crates or another plugin checkout. Keep local dependencies
  inside this project; published crates and pinned Git sources may be used.
- This is filter ABI 1 with `pc_*` exports, not the `pce_*` panel API. It produces
  `Channel-Gain.wasm`, installed through `plugin.install`. Renaming it to `.pcext`
  does not make a panel. Panel guests cannot invoke the global `plugin.run` registry.
- Change the embedded manifest identity/metadata, parameter definitions, algorithm,
  and tests together. If renaming the Cargo library, update both build scripts.
  Rebuild after manifest edits. Preserve source and dependency licensing.
- Preserve the owned allocation/range checks and import-free WASM boundary. Keep
  required export attributes without introducing unsafe slices or pointer access.
  No WASI, filesystem, network, or console imports are available.

## Pixel contract

The host supplies interleaved little-endian `f32` samples, even for 8/16-bit targets.
Decode depth, model, channels, and alpha from the documented format. This starter
supports RGB and grayscale, including channel/Quick Mask targets, and rejects the
other modes explicitly. Do not infer RGBA8 from the source depth or buffer size.

Gain changes only color samples and preserves alpha bytes. Clamp integer-depth
colors as specified; preserve finite HDR/negative float values in 32-bit documents.
Validate dimensions, byte counts, parameter JSON, finite samples, and allocations
before mutating a band. Error codes must fail clearly without partial band output.

The host handles selection coverage. `overlap` and `_image` describe padded bands
and document geometry; coordinate effects must use those values to avoid seams.
Each band has a fresh instance, so globals cannot accumulate image-wide statistics.
The normalized composite boundary of packaged image processing is a different API.

## Build and verify

Use Rust with the minimum version declared in Cargo.toml (currently 1.95), its native
linker, rustfmt/Clippy, and `wasm32-unknown-unknown`. From the project root:

```sh
cargo fmt -- --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

Build with `./build.ps1` on Windows or `sh build.sh` on macOS/Linux. They honor
`CARGO_TARGET_DIR` and copy the release artifact to `Channel-Gain.wasm`. These
wrappers do not currently accept an offline flag; use explicit Cargo commands if
an offline build is needed, and copy the matching release artifact for installation.

Cover supported depths/models, alpha, HDR samples, malformed parameters, overflow,
unsupported modes, and allocation failures relevant to the change. Test release
WASM in the real host, including selection coverage and Undo/Redo; native tests
alone do not prove the exports or band behavior. Use scratch documents and an
authorized install/test route. Restricted MCP denies filesystem-path installation;
standalone `plugin.install` from base64 `data` can be permitted. Check the actual
host policy and tool response. This is distinct from managed panel/package installation.
Report exact artifact/source paths, tested host/platform, checks, and limitations.

## Optional typed SDK authoring

This existing project keeps its supported wire versions and needs no SDK migration.
For a new API-2 project, `photocraft-plugin new AUTHORING_JSON FRESH_PROJECT` supplies
private API/SDK 0.1 sources, locks, licenses and a local guide. Edit `authoring.json`
and the handler; generated manifests/bindings belong in Cargo `OUT_DIR`. Keep local
SDK dependencies inside that project and retain its release hash index. Do not link
to engine/UI sources or introduce another plugin or companion as a runtime dependency.
The SDK is optional author tooling; availability/discovery never grants authority.

Current desktop hosts coordinate native work and store changes with OS locks and
per-plugin activation epochs. Management can return busy while native work runs.
Keep the host-owned store lock and activation tombstones intact. Completion/Apply
reject revoked native results; native containment, quotas and worker protocol 2
remain additional host work. Keep project docs and this guide aligned with changes.
