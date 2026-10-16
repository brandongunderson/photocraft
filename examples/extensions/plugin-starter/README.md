# Hello PhotoCraft: native-panel plugin starter

Copy this folder to start an independent plugin project. It builds a single
`Hello-PhotoCraft.pcext`: ordinary core WebAssembly with an embedded manifest.
It implements the PhotoCraft Plugin API directly, with no dependency on the editor
workspace or an external SDK. The manifest/export discriminator is 2; obsolete
panel API 1 modules must be rebuilt for the current host.
PhotoCraft draws the native panel from `manifest.json` and runs the guest only
when a declared button is clicked.

The panel offers a **Layer name** dropdown, **Show document info**, and
**Create layer**. Information is shown as a status message. Create requests one
`layer.new.layer` command with a string `name`; the host adds a regular empty
pixel layer through its engine and normal Undo history. Both actions explain
when an image must be opened first.

## Project layout

Agents: read the local [AGENTS.md](AGENTS.md) for project boundaries and acceptance checks.

```text
plugin-starter/
  Cargo.toml            independent cdylib project and release settings
  Cargo.lock            exact dependency resolution; keep this committed
  manifest.json         identity, permissions, native fields and buttons
  src/
    lib.rs              module setup, limits and embedded manifest
    abi.rs              required exports and bounded request/response buffers
    handler.rs          pure event logic and its unit tests
  build.ps1             Windows build and artifact copy
  build.sh              macOS/Linux build and artifact copy
  .gitignore            excludes build output and the packaged binary
  rustfmt.toml          formatting settings, kept when copying the project
  clippy.toml           allows ordinary assertions/unwraps only in test code
  LICENSE-MIT
  LICENSE-APACHE
  README.md
```

## Build and install

Install a Rust 1.95+ toolchain with Cargo, rustfmt and Clippy. Native unit tests
also need your platform's Rust linker toolchain. Add the import-free target:

```sh
rustup target add wasm32-unknown-unknown
```

Windows PowerShell, from this folder:

```powershell
cargo fmt -- --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
.\build.ps1
```

macOS/Linux, from this folder:

```sh
cargo fmt -- --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
sh build.sh
```

The scripts run `cargo build --locked --release --target wasm32-unknown-unknown`
and copy `target/wasm32-unknown-unknown/release/hello_photocraft.wasm` to
`Hello-PhotoCraft.pcext` in this folder. They also respect `CARGO_TARGET_DIR`,
including an absolute directory or one relative to this project. Keep
`Cargo.lock` to reproduce dependency versions. After dependencies have been
cached, use `.\build.ps1 -Offline` or `sh build.sh --offline`; the same Cargo
checks support `--offline` too. Different Rust/compiler versions can still
produce different binary bytes.

In PhotoCraft, open **Plugins > Manage Plugins…**, choose
`Hello-PhotoCraft.pcext`, review its identity and permissions, and install it.
Open **Plugins > Hello PhotoCraft**. Open an image, try each button, then undo
the created layer. Rebuild and explicitly replace the installed version to
test a change; replacing it resets guest state. The manager copies the module
into its managed installation store.

## Customize it

1. Change `id`, `name`, `author`, `description`, and `version` in
   `manifest.json`. Replace the example id with a reverse-DNS id you control.
   Manifest ids are lowercase ASCII, start with a letter or digit, use only
   letters, digits, dots, underscores or hyphens, and contain at most 64 bytes.
2. Edit `layerName.options` and its `default` in the manifest to change the
   available names. The handler reads both from the embedded manifest. This
   starter additionally accepts only nonblank names of at most 128 characters
   without control characters; the engine command itself requires a string.
   Keep the test's expected choices/default in sync when you change them.
3. Add or change button ids and match those ids in `handler::handle`. Declare
   every editing command under `permissions.commands`, retain the required
   `host.commands.invoke` capability, then return command
   entries with an object-valued `params`. Discover supported commands with
   `extension.commands` and inspect their parameters with `command.list`.
4. Keep permissions narrow. This example needs only `documentRead` for the
   metadata snapshot and `layer.new.layer` to create the layer. A guest receives
   no pixels or document file paths through `documentRead`.
5. Keep pure logic in `handler.rs`, add tests for invalid input and state, and
   run the checks above before rebuilding. The ABI helper normally needs no
   changes. If you rename the Cargo package/library, update the `.wasm` file
   name in both build scripts; the output `.pcext` name is otherwise independent
   of the manifest id.

This starter uses compact `number`, `int`, `bool`, and `choice` controls. A free-text field
is not supported, so layer names are a dropdown. Arbitrary layouts, HTML,
JavaScript, webviews, file/network APIs, custom canvas overlays and general
asynchronous callbacks are outside this native-panel contract. Use the
separate filter ABI when implementing a custom pixel filter.

## ABI, memory and state

The required exports are `memory`, `pce_api_version() -> i32`,
`pce_manifest() -> i64`, `pce_alloc(i32) -> i32`, and
`pce_handle(i32, i32) -> i64`. The Rust toolchain exports linear `memory` for
this `wasm32-unknown-unknown` cdylib; no manual memory export or WASI target is
needed. The guest uses no imports, including WASI or host functions. Unsigned
Rust integer types have the same WebAssembly bit representation as those ABI
signatures. `pce_manifest` and `pce_handle` pack UTF-8 JSON as
`(length << 32) | pointer` using 32-bit offsets into that memory.

Rust 2024 requires `#[unsafe(no_mangle)]` on exported symbol names. The source
allows that attribute only on the four ABI functions while denying unsafe
code elsewhere. There are no unsafe blocks, unsafe functions, pointer
dereferences, production `unwrap`/`expect`, or panic paths. The helper keeps
request/response buffers owned by the guest, caps each at 256 KiB, uses
fallible buffer reservation, checks pointer identity and exact length, and
bounds response serialization. It never turns a supplied integer/pointer
into a Rust slice. A zero allocation or zero packed response on an ABI/buffer
failure is rejected by the host before an edit or state commit.

For every click PhotoCraft creates a **fresh guest instance**. Requests use a typed envelope with the current `apiVersion` discriminator, `host`
capability metadata, optional negotiated `compatibility`, `event`, validated/defaulted
`values`, `state`, `document` and `responses`. The document is null without an active
document or read permission. Additive envelope metadata is accepted. Wrong protocol
versions or missing command capability produce a bounded failure without commands. The handler proposes a response with
`state`, `message`, and `commands`. Its state starts at null, then holds
`{"createdLayers": 1}` after the first successful create. The counter is
session-wide for this installed plugin, shared across documents, and saturates
at `u64::MAX`. It counts successful create actions, not the document's current
layers; Undo does not rewind this guest counter. Information and
validation-error responses return the existing
state explicitly and no commands. Malformed state is reported and preserved.
The host cannot provide state for unparseable JSON, so that error response has
null state. This malformed-JSON path is defensive; the host generates valid
JSON requests.

PhotoCraft validates the complete command list against declared permissions
and the caller's authorization before the first edit. It commits returned
state only after all commands succeed. Omitting `state` would reset it to null
on success. This starter does not request broker queries or continuation events and must not
rely on globals surviving the click. Multiple commands would run sequentially
and would not form one atomic transaction; earlier edits could remain if a
later command failed. This starter emits at most one editing command.

The host limits the module to 8 MiB, manifest to 64 KiB, linear memory to
32 MiB, call depth to 256, fuel to 5 million instructions per instance,
request/response JSON to 256 KiB each, responses to 32 commands, and status
messages to 4 KiB. Serialized session state additionally has a 128 KiB host limit,
checked before returned commands execute. The information message truncates user-provided text by
Unicode character boundaries to remain within the status budget. The tests
cover malformed JSON, invalid buffers/values/events/state/document metadata,
no active document, command shape, state preservation and response limits.

For the complete host contract, see
[PhotoCraft Plugin API](../../../docs/plugin-api.md),
[`docs/extensions.md`](../../../docs/extensions.md) and the general
[`docs/plugin-development.md`](../../../docs/plugin-development.md). These
links refer to the PhotoCraft source checkout; the starter itself can be
built after copying it elsewhere.

This starter is licensed under MIT OR Apache-2.0; both license files are
included.
