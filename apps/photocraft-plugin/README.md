# PhotoCraft plugin conformance tool

This Rust executable is an optional developer tool shipped by the host project. Plugins do not
depend on it, the editor source tree, or another plugin. Existing Python packaging/build helpers
remain supported.

```text
photocraft-plugin check PACKAGE
photocraft-plugin project SOURCE
photocraft-plugin build-export SOURCE FRESH_DESTINATION
photocraft-plugin pack STAGED_DIRECTORY FRESH_PACKAGE
photocraft-plugin hash FILE
photocraft-plugin probe PANEL REQUEST_JSON
photocraft-plugin sdk FRESH_DIRECTORY
photocraft-plugin generate AUTHORING_JSON FRESH_DIRECTORY
photocraft-plugin new AUTHORING_JSON FRESH_PROJECT
```

Every command emits a versioned JSON report and returns a nonzero exit status on failure.
The [SDK guide](../../docs/plugin-sdk.md) explains SDK export, a complete authoring manifest,
typed handlers, the compatibility policy and independent-build acceptance.

`sdk` exports privately usable public API/SDK sources, licenses and a SHA-256 release index.
`generate` derives package metadata, an embedded descriptor and Rust bindings from one source.
`new` creates an independent WASM project with those SDK sources, a Cargo build-time generator,
local agent instructions and an offline-resolved lockfile. It does not build or certify that
plugin. Existing destinations are never overwritten. Native/filter layouts use `generate`
with their own sources. For SDK projects, `build-export` packages from `authoring.json`.
`check` uses the real host validators: archive limits and safe paths, required files and CRCs,
platform, panel/filter manifests and actual WASM exports, and command grants. It also checks
the PE, ELF or Mach-O worker header against the declared operating system and architecture. It loads bounded
sandboxed WASM to verify its ABI. It never starts a native worker or installs into a user store.
Native dynamic-library closure, publisher identity and native behavioral correctness are
separate checks; this tool does not claim to prove them.

`project` runs Cargo's locked, offline dependency resolution for all source projects and checks
the resolved local dependency paths and workspace root. Each local dependency must live inside
the plugin's own root. Published crates and pinned Git resolutions are allowed. Local packages
must declare a license. Symlinks and Windows reparse points are rejected. This establishes Rust
dependency ownership; arbitrary build-script side effects require separate review.

`build-export` copies source to a fresh destination outside the original source, omitting
`target`, `dist`, `.git` and Python caches, audits the copied project, builds it with Cargo and
packages it in Rust. It currently supports the independent starter layout: root panel crate,
optional `filter/` crate and optional `native/` crate. For arbitrary layouts, stage artifacts
with your own build and use `pack`. Dependency license files are collected into the archive.
Failed validation never overwrites an existing package.

The clean build requires an explicit Cargo executable and tool-only PATH. For the repository's
Windows toolchain, after loading its normal development environment:

```powershell
$env:PHOTOCRAFT_PLUGIN_CARGO = 'C:/path/to/.tools/cargo/bin/cargo.exe'
$env:PHOTOCRAFT_PLUGIN_BUILD_PATH = 'C:/path/to/.tools/cargo/bin;C:/path/to/.tools/w64devkit/bin;C:/Windows/System32;C:/Windows'
photocraft-plugin build-export ./examples/extensions/image-plugin-starter ./target/fresh-image-gain-export
```

The Rust target `wasm32-unknown-unknown`, the native linker, and cached published dependencies
must already be installed. Cargo uses `--locked --offline`, with no Python, Node or companion
worker build dependency. Offline here means no Cargo network resolution; the tool does not
sandbox arbitrary third-party Cargo build scripts. Keep the JSON report with the platform's
native dependency audit and actual engine compatibility-test results before advertising a
target as supported.

`hash` streams both SHA-256 and BLAKE3. `probe` runs a panel's sandboxed JSON handler directly;
it does not dispatch requested commands. It is useful for recording a new immutable compatibility
fixture. Never update an old fixture's expected response to hide a host regression.
