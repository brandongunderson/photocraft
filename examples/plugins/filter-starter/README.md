# Channel Gain: a standalone PhotoCraft filter starter

Copy this directory into your own repository to develop a pixel filter without changing or
linking the PhotoCraft editor. It is an independent Rust project using filter ABI v1 (`pc_*`).
For a native panel with buttons and supported editor commands, use
[`examples/extensions/plugin-starter`](../../extensions/plugin-starter) instead. The two APIs have
different manifests, exports and installation routes; a panel cannot currently call `plugin.run`.

Channel Gain multiplies RGB or grayscale color samples by `gain` (0–2, default 1.25), leaving
alpha bytes untouched. It accepts 8-, 16- and 32-bit source documents. The ABI always supplies
little-endian `f32` samples: the 8/16-bit paths clamp colors to 0–1, while the 32-bit path preserves
finite HDR values above 1 and negative values. Gain operates on samples in the document's own
color space; it does not perform a profile conversion or linear-light conversion. CMYK, Lab,
Bitmap, Indexed, Duotone and Multichannel modes are deliberately rejected.

## Build and install

Agents: read the local [AGENTS.md](AGENTS.md) for ABI, precision, and verification rules.

Install Rust and its core WebAssembly target:

```sh
rustup target add wasm32-unknown-unknown
```

From this directory, run one build script:

```powershell
.\build.ps1
```

```sh
sh build.sh
```

Both scripts build with the checked-in `Cargo.lock`, respect `CARGO_TARGET_DIR` when set, and copy
the result to `Channel-Gain.wasm` in this directory. They do not modify editor fixtures. The
generated file and `target/` are ignored by Git. The Cargo library output is
`wasm32-unknown-unknown/release/photocraft_filter_channel_gain.wasm` inside the target directory.

In PhotoCraft, choose **Plugins > Install Plug-in…**, select `Channel-Gain.wasm`, then run
**Plugins > Channel Gain** with an RGB or grayscale image and an active pixel layer. Set Gain
and confirm the generated dialog. Use **Edit > Undo** to check the result. A selection limits the
edit through the host's coverage blending; the filter does not receive or implement selections.
Targeted alpha channels and Quick Mask arrive as grayscale, so the filter also accepts them.

This filter is installed through `plugin.install`, not `extension.install` or Manage Plugins.
A trusted local CLI process can install and run it in the same session:

```sh
photocraft-cli run input.png \
  --cmd plugin.install --params '{"path":"/absolute/path/Channel-Gain.wasm"}' \
  --cmd plugin.run --params '{"id":"org.example.channel-gain","params":{"gain":1.5}}' \
  --out output.png
```

The example uses a POSIX shell continuation. In PowerShell 7.3+, put the command on one line or use
backtick continuations and a Windows path such as `C:/plugins/Channel-Gain.wasm`. Older Windows
PowerShell can strip JSON quotes; use the Python smoke runner in the
[testing guide](../../../docs/plugin-testing.md#cross-platform-python-smoke-runner) instead. Each CLI run has
its own installed registry. In the desktop app, configure the additional plug-ins folder if you
want raw filter modules discovered at startup. Install a rebuilt module again to replace its
current process installation; changing the source file alone does not update the running code.

## Customize the starter

| File | Purpose |
|---|---|
| `manifest.json` | Change the reverse-DNS id, display name, version, author, parameters, overlap and processing area. |
| `src/lib.rs` | Replace `parse_gain` and `process_band` with your parameter handling and sample algorithm. Keep the ABI boundary and buffer checks. |
| `Cargo.toml` | Rename the crate and adjust dependencies. Retain the empty `[workspace]` so the project builds independently. |
| `build.ps1`, `build.sh` | Update the library output filename if you rename the crate; choose your distributed `.wasm` filename. |
| `rustfmt.toml`, `clippy.toml` | Keep formatting and test lint settings with the copied project. |

Use an id you control before distribution. The manifest is embedded with `include_bytes!`, so
manifest edits also require a rebuild. The complete public contract is in the
[filter ABI reference](../../../docs/plugins.md); the
[general developer handbook](../../../docs/plugin-development.md) compares both APIs.

## Memory and ABI behavior

The exported functions are:

```text
pc_abi_version() -> i32
pc_manifest() -> i64
pc_alloc(size: i32) -> i32
pc_filter(buf: i32, buf_len: i32, width: i32, height: i32,
          channels: i32, format: i32, params: i32, params_len: i32) -> i32
```

Rust uses `u32`/`u64` for the same WebAssembly integer bit patterns. The compiler exports linear
`memory`; `pc_manifest` returns `(length << 32) | pointer`. The host allocates pixels and JSON
separately through `pc_alloc`, writes them into guest memory, then calls `pc_filter`. The guest
finds each exact allocation base and validates the requested slice before reading it. It never
constructs a slice from, or dereferences, a raw pointer. Rust's `unsafe(no_mangle)` export
attributes are required by edition 2024; there are no unsafe blocks.

The allocator retains independent `Vec<u8>` blocks in a `thread_local` `RefCell`, permits at most
eight allocations totaling 64 MiB, and returns zero for an invalid size or allocation failure.
Parameter JSON has an additional 64 KiB starter limit. The normal host band target is about
4 MiB, and this point filter declares `overlap: 0`. Increasing host bands or adding large
neighborhoods can require reconsidering the guest budget. These are starter limits; the host
also enforces its own instruction, memory and document limits. Do not add WASI, filesystem,
network or other imports: the host rejects imported modules.

The host instantiates a fresh guest for each band, so allocations are reclaimed together and
there is no cross-band or cross-run state. A global counter cannot accumulate image statistics.
`_image` in parameter JSON describes band/canvas geometry; this filter accepts that field but
does not need it. If you add a coordinate-dependent effect, use this metadata instead of treating
each band as a new image.

`pc_filter` returns zero on success. Errors return these numeric codes; the host reports a filter
failure and does not admit partially edited bands:

| Code | Meaning |
|---|---|
| 1 | Unknown allocation, invalid pointer/range, overlapping pixel/parameter allocation or unavailable allocator borrow. |
| 2 | Zero/overflowing dimensions or a pixel byte count that does not equal width × height × channels × 4. |
| 3 | Unsupported color mode, depth, channel count or reserved format bits. |
| 4 | Invalid/oversized JSON, a non-object parameter value, unknown key or gain outside 0–2. |
| 5 | A nonfinite color sample or multiplication that overflows a finite `f32`. |

Missing or null gain uses 1.25. An explicit gain of the wrong type or outside the range returns
code 4. The editor normally resolves defaults and clamps declared parameters before the guest
sees them; the guest validates again so direct callers receive a clear failure code. Alpha is
not validated or changed by the algorithm. All color samples are checked before any bytes are
changed. A guest has no status-message export or console import; return codes and host errors
are its diagnostic channel.

## Test before distributing

```sh
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo fmt -- --check
cargo build --locked --release --target wasm32-unknown-unknown
```

The native tests exercise the pure sample algorithm and allocation bookkeeping: RGB/grayscale,
with and without alpha, all three depths, HDR and negative float samples, gain defaults and
validation, malformed JSON, geometry overflow, unsupported modes, nonfinite samples and
allocation bounds. Native tests do not execute WebAssembly. Also install the release artifact
in PhotoCraft to verify the actual export signatures, import-free module, generated dialog,
selection handling and Undo. Test supported depths and the unsupported-mode failure path.

Retain source and dependency licenses when distributing your derivative. This example is
licensed under the repository's MIT OR Apache-2.0 terms; its dependency licenses still apply.
