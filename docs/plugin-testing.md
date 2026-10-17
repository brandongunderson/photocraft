# Plugin testing and debugging

Use this with the [developer handbook](plugin-development.md), [panel API](extensions.md) and
[filter ABI](plugins.md). The tests below use the neutral Hello PhotoCraft, Channel Gain and
[Image Gain](../examples/extensions/image-plugin-starter/README.md) starters. Their scripts are
optional developer tools, not a runtime dependency of your plugin.

For the [guest SDK](plugin-sdk.md), `scripts/plugins/test-platform-examples.py` generates a
neutral project outside the editor, audits its private dependencies, builds release WASM,
packages it, then invokes a real broker edit and Undo. It also retains the existing current API
starter checks. Use `--sdk-only` for the generated guest and `--export-parent` outside the
checkout when your normal temporary directory is inside it. No generated project is committed.

The existing engine package suite covers separate-process reader/writer contention, crash
release, failed-update preservation and native stale-generation/preview rejection. These
are platform contract regressions; algorithm tests for independently released plugins stay
in their own repositories. OS-specific checks require the relevant OS; a passing Windows
run alone does not certify macOS/Linux, filesystem locking or hardware drivers there.

## What each test level proves

| Level | Proves | Does not prove |
|---|---|---|
| Native unit tests | Handler decisions, numeric logic, validation and state proposals | A valid WASM module, host grants, menus or editing |
| Release WASM loaded by host | Actual exports, imports, manifest, memory access and interpreter compatibility | Every user target, GPU or lifecycle case |
| Headless editor smoke | Real module plus commands, edits, selection and history | Window layout, manager UX or persisted desktop settings |
| Desktop manual test | Install/review, native panel/dialog, controls, error recovery, saved installation | Every possible image or machine |

Keep fixtures disposable and non-private. Use tiny synthetic images for correctness, then a
representative large image for performance. Record the exact source revision and distributed
module hash. Loading the debug/native unit-test build is not a substitute for testing release WASM.

## Native checks

For installation changes, include an incompatible cached descriptor, malformed record and
unloadable loose module in a disposable store alongside a healthy plugin. Verify that the
manager can remove or review a compatible replacement, that rejected code never runs,
and that the healthy installation, enable state and managed settings remain unchanged.
Exercise same-version repair, newer-version update, restart, and a competing host changing
the failed installation after discovery or review. Never use a user's store as a fixture.

Run inside each independent starter or your own plugin project:

```sh
cargo fmt --all -- --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo clippy --locked --target wasm32-unknown-unknown --lib -- -D warnings
cargo build --locked --release --target wasm32-unknown-unknown
```

`cargo test` runs the pure logic on your development machine. Native unit tests do not execute
`pce_*`/`pc_*` inside PhotoCraft. The last two commands check/build the actual guest target.
Use the project's build script to copy that release output to its distributed filename.

The panel starter tests malformed requests, missing documents, unknown events/fields, choice
validation, document summaries, state preservation/counting and bounded ABI storage. The filter
starter tests parameters, geometry, color layouts, alpha, integer-depth clamping, float HDR,
nonfinite input and memory ownership. Keep these checks when changing the boilerplate; adapt
behavior tests when you change the manifest or algorithm.

## Independent image processor checks

The [Image Gain starter](../examples/extensions/image-plugin-starter/README.md) contains three
independent Cargo workspaces: its panel, `native/` worker and `filter/` module. Run
`build.ps1 -Check` on Windows or `sh build.sh --check` on macOS/Linux to check each project,
build release artifacts, collect licenses and package them. Check guest-target Clippy for
the panel/filter separately; the native worker is not a WASM guest. Run the stdlib packaging
regressions with `python tools/test_package.py`.

Native worker tests must include actual PNG read/write, output dimensions, color-space marking,
alpha, malformed options/jobs and cancellation. Gain's unit tests cover those boundaries and
nearest-neighbor output sizing. They do not prove that another processor, native dependency
or platform works. Native stderr is a bounded diagnostics/progress channel; stdout is not.

Install the exact generated version-2 package in a compatible desktop build. Compare both
Native worker and Sandboxed filter previews; test Apply to a new tab, same-size new layer and
Undo/Redo, Discard, cancel, closing the panel, changing the source, disable/remove, restart
and repair. Use 8/16-bit RGB/grayscale sources and transparent pixels; generic processing
normalizes them to 8-bit RGBA sRGB. Include a non-sRGB source to exercise host color conversion.
Bundled filters remain fixed-size and selection-aware; native processing receives the whole
visible composite without a separate selection mask. Test those distinctions deliberately.

Trusted direct `plugin.processImage` callers supply the installed panel `id`; panel actions
omit it because ownership is bound by `extension.invoke`. The ordinary CLI `run` session has
no persistent plugin store, so it cannot install a `.pcplugin` merely by running the raw guest
smoke chain below. Use desktop installation/acceptance, or a host integration harness attaching
a disposable persistent store. Never point tests at the user's real settings.

## Optional CLI setup

The desktop manager is enough for manual development. If you want repeatable headless checks,
use `photocraft-cli` from a compatible PhotoCraft build. From the editor source checkout:

```sh
cargo build --locked -p photocraft-cli --bin photocraft-cli
```

The result is `target/debug/photocraft-cli` (`.exe` on Windows). This is a host developer tool,
not a dependency linked into the guest. `run` creates a fresh in-memory session per invocation;
it does not load the desktop's managed plugin store. Install a raw test module and invoke it
within the **same** `run` process.

### Panel smoke from a shell

Run this from the editor repository after building the neutral panel. POSIX shell example:

```sh
photocraft-cli run --new '{"width":64,"height":48,"background":"transparent"}' \
  --cmd extension.install --params '{"path":"examples/extensions/plugin-starter/Hello-PhotoCraft.pcext"}' \
  --cmd extension.invoke --params '{"id":"org.example.hello-photocraft","event":"create","values":{"layerName":"Notes"}}' \
  --cmd document.inspect \
  --cmd edit.undo \
  --cmd document.inspect
```

Replace `photocraft-cli` with the path to your executable if it is not in PATH. PowerShell 7.3+
with normal native argument handling can use the same JSON strings on one line or with backtick
continuations. Older Windows PowerShell can strip native argument quotes; the Python runner
below avoids that problem.

Each successful command prints one JSON line: `{"command":"id","result":...}`. `extension.invoke`
returns `id`, `message` and a `results` list for its host commands. These results go to the CLI
caller, not back into the guest. A failed command exits nonzero, prints an error on stderr and
stops later commands in that `run` process. Do not assume it rolled back earlier edits.

### Filter smoke from a shell

```sh
photocraft-cli run --new '{"width":64,"height":48,"background":"transparent"}' \
  --cmd edit.fill --params '{"color":[0.2,0.4,0.6,0.5]}' \
  --cmd document.pixel --params '{"x":2,"y":2}' \
  --cmd plugin.install --params '{"path":"examples/plugins/filter-starter/Channel-Gain.wasm"}' \
  --cmd plugin.run --params '{"id":"org.example.channel-gain","params":{"gain":1.5}}' \
  --cmd document.pixel --params '{"x":2,"y":2}' \
  --cmd edit.undo \
  --cmd document.pixel --params '{"x":2,"y":2}'
```

The RGB samples should change from approximately `[0.2,0.4,0.6]` to `[0.3,0.6,0.9]`; alpha stays
unchanged and Undo restores the source. Integer-depth quantization means exact decimal matches
are inappropriate: use a tolerance consistent with 8- or 16-bit storage. For 32-bit samples use
a suitable floating-point tolerance.

## Cross-platform Python smoke runner

The complete script is [scripts/plugins/smoke-starters.py](../scripts/plugins/smoke-starters.py).
It uses Python's standard library and passes argument lists directly to the CLI, without shell
quoting/interpolation. Run it from the editor repository after building both starter modules:

```sh
python scripts/plugins/smoke-starters.py --cli target/debug/photocraft-cli --panel examples/extensions/plugin-starter/Hello-PhotoCraft.pcext --filter examples/plugins/filter-starter/Channel-Gain.wasm --logs target/plugin-smoke-logs
```

On Windows use `target/debug/photocraft-cli.exe`. In the portable developer kit the prebuilt
modules are in `artifacts/`; use those paths or build your copied starters first. Supply a
compatible CLI separately; the kit does not bundle the editor executable.

The script verifies 12 compiled-module cases:

- Panel install, info without edits, layer creation and Undo/Redo.
- Panel info after closing the active document.
- Invalid choice rejection, disabled invocation, removed invocation and replacement.
- Actual filter processing at 8-, 16- and 32-bit depth, preserving alpha and Undo/Redo.
- Selection coverage: an unselected pixel stays unchanged.
- Wrong-type gain rejection and the filter's intentional unsupported-mode error.

This runner expects the provided starter ids/events/algorithm. After customizing your plugin,
adapt the identifiers, parameters and expected results rather than assuming it is a general
validator for arbitrary plugins. Its failure checks match current host error text; update those
checks if the host's diagnostics change. Run Python without `-O` so its assertions remain active.

A minimal pattern for your own script is:

```python
import json
import subprocess

result = subprocess.run(
    ["photocraft-cli", "run", "--new", json.dumps({"width": 64, "height": 48}),
     "--cmd", "extension.commands"],
    capture_output=True, text=True, encoding="utf-8", check=True,
)
reply = json.loads(result.stdout)
allowed_ids = reply["result"]["commands"]
print(allowed_ids)
```

Use a list and `json.dumps`; do not concatenate user input into a shell command.

## Desktop acceptance checks

| Scenario | What to verify |
|---|---|
| First install | Correct identity, grants, content review, menu entry and no unsolicited edit |
| No active document | Helpful no-op/status or documented disabled action; no crash |
| Defaults and boundary values | Correct mapping, clamping, type errors and choices |
| A real action | Intended document/layer/channel/selection only; meaningful history behavior |
| Locked/incompatible target | Actionable error; no false success/state commit |
| Repeated actions/doc switching | State scope is intentional; no stale document assumptions |
| Undo/Redo | Pixel/document changes reverse normally; guest state is not silently tied to Undo |
| Multi-command failure | Earlier successful actions retain their normal behavior; no atomicity claim |
| Rebuild/replacement | Same id updates; new controls appear; guest state resets |
| Restart | Desktop installation/enable flag persist; session guest state resets |
| Disable/remove | No new invocation; menu/manager reflect current state |
| Panel close/Done | Panel closes; supported job behavior agrees with preview versus compact mode |
| Every claimed theme/platform | Controls legible, settings scroll, final actions reachable |

For native services also test integrity damage/repair, worker errors, source locking, cancellation
and drain, output validation, large bounds and installation-path handling. For supported previews,
test Discard, settings/source changes after generation, source close, disable/re-enable,
replacement/removal, Apply once only, both destinations where supported, alpha and Undo/Redo.
Those are native-service acceptance tests, not generic features supplied by a raw panel module.

For filters, test band boundaries and overlap seams, selection feathering, transparent pixels,
no active pixel layer, masks/alpha targets, all supported modes/depths, unsupported modes, partial
failure preservation and Smart Object/Smart Filter behavior you intend to support. Do not infer
whole-image support from a single tiny-band unit test.

## Debugging failures

Work from the failing boundary:

1. **Build:** resolve target/dependencies/import-generating features. Confirm the raw artifact
   filename after a Cargo package rename.
2. **Inspection/install:** check API signatures, embedded JSON, memory export, import-free module,
   allowed permissions and correct installer. A renamed file cannot fix a wrong ABI.
3. **Guest event:** reproduce the pure handler request in a unit test; check default/null values,
   mode spelling, state size and resource use.
4. **Host command:** read the exact error and parameters; check the current target's locks,
   layer type, selection and mode. The guest cannot query arbitrary host commands dynamically.
5. **Filter output:** check f32 byte order, alpha, color model, geometry, overlap and selection.
   Return nonzero for unsupported inputs instead of producing an unexplained image change.

Use `message` for panel status. WASM guests have no console/log import. The filter ABI reports
nonzero codes; the starter README defines its codes. Native unit-test logging is ordinary
local development logging and is not a runtime guest logging API. Host log locations and
`RUST_LOG` configuration are documented in [development](development.md#logs).

## Discovery and automation authority

Discover grants with `extension.commands` and parameters with `command.list` or CLI `commands`.
The [generated command reference](plugin-command-reference.md) records the current subset;
[generate-command-reference.py](../scripts/plugins/generate-command-reference.py) refreshes it.
Neither discovery command is itself guest-allowlisted. `command.list` is an array, with advisory
parameter strings, not an OpenAPI/JSON Schema document.

The direct CLI `run` path is a trusted local invocation for developer-owned files. It differs
from restricted TCP/MCP/live-control sessions: those cannot install, enable or remove persistent
extensions or start native plugin processing. Install using the desktop manager first when
testing a panel through a restricted bridge. Ordinary installed panel actions still pass the
caller's authorization gate. Raw in-memory inspection via base64 has different file authority
from a filesystem `path`.

For live UI testing use the existing [control protocol](control-protocol.md), including its
loopback authentication and explicit read/write roots. The control transport is a developer
option, not part of the plugin's own runtime and not required for installation/use.

## CI and release gate

The host's regular workspace tests exercise the pinned current-API panel and unchanged filter-1 compatibility corpus
and the capability/authority validators. Its CI also checks generated schema snapshots and
builds the Layer Organizer and Document Inspector examples in fresh directories outside the
checkout, then explicitly runs their four actual-package integration tests:

```sh
cargo test -p photocraft-plugin-api --locked --features schema
python scripts/plugins/test-platform-examples.py
```

The Python helper uses the current Python executable for each exported project's build,
omits existing build outputs, and points the host at only those newly built packages. Missing
packages or failed host tests fail the run. It requires the `wasm32-unknown-unknown` Rust target;
pass `--offline` when dependencies are already cached. PhotoCraft Plugin API is the current pre-release contract; these
tests do not establish a support window for future host releases.
For a local host built without optional HEIF support, pass `--host-features=`; CI retains the
default `heif` selection.

Full native package qualification remains separate. The pinned neutral Windows worker requires
the [native compatibility runner](../tests/plugin-compatibility/README.md) before advertising
that target as qualified. Each independent plugin maintains and runs its own package acceptance
tests. The ordinary CI sequence above does not download models or certify independently
published plugins.

A provider-independent CI sequence, inside each starter/project:

```sh
rustup target add wasm32-unknown-unknown
cargo fmt --all -- --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo clippy --locked --target wasm32-unknown-unknown --lib -- -D warnings
cargo build --locked --release --target wasm32-unknown-unknown
```

Then run a compatible host's compiled-module smoke tests. Retain native test logs, the WASM
artifact, its hash, host version/source revision and target platform. An offline build requires
all dependencies and the target toolchain already cached; `--locked` alone does not mean offline.

Before publishing, install the exact release artifact through the intended desktop route and
run its manual acceptance cases. A passing unit suite or a successful ZIP build is not a claim
that every input, host version or platform is bug-free.


## Lifecycle acceptance

Extend the existing neutral suites when changing lifecycle/store behavior. Cover lazy startup
with 1000 dormant descriptors, separate-process generation leases and crash release, exact
legacy artifacts, strict framed-worker handshake/reuse/reset/cancel/deadline/idle/quarantine,
queue fairness/resource release, and multiple previews without cross-owner eviction. Managed
storage tests must preserve the active code/data pair after interrupted stages, migration
errors, quota exhaustion and injected full-disk writes. Temporary native fixture sources and
executables are deleted; no plugin-specific model/runtime fixture belongs in the platform.

The scheduler admits estimates, not kernel containment. Test restricted native only when that
profile exists on the target OS; do not infer macOS/Linux certification from a Windows pass.
See [lifecycle](plugin-lifecycle.md), [worker protocol](plugin-worker-v2.md) and
[managed storage](plugin-managed-storage.md).
