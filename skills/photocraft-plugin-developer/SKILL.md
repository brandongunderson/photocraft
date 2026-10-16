---
name: photocraft-plugin-developer
description: Build, modify, package, and test independent PhotoCraft plugins using its panel, filter, and package APIs. Use for PhotoCraft plugin projects and standalone starters; not editor-core development or installing Photoshop plugins.
---

# PhotoCraft Plugin Developer

Deliver an independent plugin project and the appropriate `.pcext`, `.wasm`, or
`.pcplugin` artifact, with evidence of the checks actually performed. Preserve the
user's requested feature, project location, and existing authorization.

## Establish the available contract

Read the project's `AGENTS.md`, `README.md`, manifests, and build scripts first.
For an existing plugin, retain its stable identity and public settings unless the
task calls for a migration. For a new project, use a compatible complete starter
from the supplied PhotoCraft source or developer kit. Copy it to a fresh destination,
excluding generated `target/`, `dist/`, and Git metadata; preserve locks and licenses.
If no starter is available, obtain the matching source/kit before depending on its
scaffold. Do not substitute upstream-only WASM filter documentation for these APIs.

For new plugins, the optional `photocraft-plugin new AUTHORING_JSON FRESH_PROJECT`
command exports a complete project with private API/SDK 0.1 sources, licenses and locks.
The executable embeds these public sources; no editor checkout is needed to export or
build the project. Read its generated README and `AGENTS.md`. Edit `authoring.json` and
the handler; Cargo generates manifests/bindings in `OUT_DIR`. SDK adoption is optional, and
plugin projects own their privately pinned SDK releases and independent builds.

The PhotoCraft Plugin API is the only supported authoring contract. Its current internal
panel discriminator is `2`; old version `1` is rejected and must be rebuilt. Evolve this API
until a genuine breaking change needs a new major; milestones do not create parallel APIs.
Package format 2, filter ABI 1 and native worker protocols 1/2 serve independent layers.
Select worker 2 only through a verified runtime/worker-v2.json declaration. The application
release number alone does not establish API support.

Use the running host's `extension.capabilities`, `extension.commands`, and command
descriptions where available. Discovery reports availability, not permission. An
offline build can use the supplied reference snapshot, but report host compatibility
as unverified until exercised against the intended build.

Read [API contracts](references/api-contracts.md) for version boundaries, payloads,
ownership, and current limits. In a PhotoCraft source/kit, consult `docs/plugin-development.md`
and the relevant `docs/extensions.md`, `docs/plugins.md`, `docs/plugin-api.md`,
`docs/plugin-image-processing.md`, or `docs/plugin-native-tasks.md` for full details.
The matching `docs/plugin-sdk.md` covers typed events/replies, document builders,
single-manifest authoring, source exports and the preview compatibility policy.
Those documents are authoring references, not runtime source dependencies.

## Choose the smallest suitable starting point

| Feature | Starter path in a source checkout or developer kit | Result |
|---|---|---|
| Typed new panel/command guest with single source metadata | `photocraft-plugin new` from a reviewed authoring input | current API project with privately included SDK/API sources |
| Panel combining permitted editor commands | `examples/extensions/plugin-starter` | Current API `.pcext` |
| Pixel algorithm using the filter target's depth and channels | `examples/plugins/filter-starter` | Filter ABI 1 `.wasm` |
| Panel with its own executable and/or bundled WASM algorithm | `examples/extensions/image-plugin-starter` | Package 2 `.pcplugin` |
| Namespaced command and atomic layer rename/visibility edits | `examples/extensions/layer-organizer` | PhotoCraft Plugin API `.pcplugin` |
| Native read-only inspector refreshed by document changes | `examples/extensions/document-inspector` | PhotoCraft Plugin API `.pcplugin` |

Use the PhotoCraft Plugin API for all panels and migrate obsolete owned panels in their
independent projects. The package container does not change for that migration. Native maintenance tasks extend a
package with an owned worker; they are not a sixth starter or a separate runtime.
Identify unsupported requirements early: dynamic/free-text controls, custom docking,
canvas tools, general callbacks, and high-precision native layer/region transport
need additional host APIs. Report the missing contract instead of inventing one or
adding a plugin-specific adapter to the editor.

## Implement inside the plugin project

- Keep the plugin independent of editor crates, sibling plugins, companion apps, and
  inherited editor workspaces. Local dependencies belong within this project;
  published crates or pinned Git dependencies may be used. Retain dependency locks.
- Own the identity, behavior, option validation, algorithms, model catalogs, native
  dependencies, and asset provenance here. Record platform/driver requirements and
  include redistributable dependencies and their notices where applicable.
- Change manifest fields and matching handler logic together. New identities require
  updating current API contribution namespaces and bundled processor references as well.
  Rebuild embedded manifests; editing a loose installed JSON file does not update WASM.
- In SDK projects, change the source `authoring.json`, not generated files. Preserve
  private SDK licenses and `SDK-RELEASE.json`; audit hashes and lock resolution when
  updating it. Typed requests and generated bindings never grant host authority.
- Preserve the starter's bounded ABI helpers and error handling. Core WASM has no
  imports or WASI. Required Rust export attributes do not justify unsafe buffer access.
- Declare only used command grants and capabilities. Resolve native workers and
  bundled filters through the invoking installation; guests never choose process paths.
- Implement algorithm-specific tests and useful failures for invalid values, unsupported
  image modes, missing assets, and cancellation relevant to the feature. Follow the
  selected starter's state, precision, alpha, selection, and history contracts.

## Build, validate, and exercise

Follow the copied project's actual commands; starter scripts have different flags.
Run meaningful handler/algorithm tests, formatting, Clippy, release WASM/native builds,
and packaging for each component changed. Native-only unit tests do not verify WASM
exports or installed behavior. Keep source instructions and README usage in sync.

Current desktop hosts discover cached descriptors lazily, use per-generation OS leases and
activation epochs, and bound native queues, resident workers and resource reservations.
Panel preparation verifies bounded panel code; bulk closure checks and packaged filter
compilation run in scheduled processing jobs. A private persistent worker may opt into protocol 2; old workers retain their exact launch
contract. Never share a process, runtime, writable cache or data migration with another plugin.
Managed settings/data/cache/secrets references are generation-scoped; updates stage and validate
a quiescent snapshot, and rollback selects a preserved code/data pair. Keep host lock files and
activation tombstones intact. Reservations and managed validation quotas do not sandbox trusted
native processes or enforce kernel memory/GPU/disk limits. Restricted native is unavailable.
Read docs/plugin-lifecycle.md, docs/plugin-worker-v2.md and docs/plugin-managed-storage.md in the
matching source/kit before adopting these optional facilities. Preserve existing external caches.
For release updates, optionally declare the plugin's own public GitHub repository and exact
release asset in its embedded descriptor; never hard-code a sibling or editor repository.
Read the release-update section of [API contracts](references/api-contracts.md) before adding
this metadata and [update acceptance](references/testing-and-mcp.md#release-update-acceptance)
before claiming it works. Marketplace discovery remains planned and cannot be a runtime requirement.
Companion apps use the authenticated local control API/MCP rather than guest handles;
plugins remain usable without a companion and automation restrictions still apply.

When supplied, use the `photocraft-plugin` tool for project ownership checks,
package validation, exported builds, and handler probes. It is optional author tooling;
plugins must not require it or the editor source at runtime. Its `build-export` layout
support and the distinction between structural and functional validation are described
in [Testing and MCP](references/testing-and-mcp.md).

Exercise the exact artifact against a compatible host on disposable documents and an
isolated test store when available. Check expected pixels or edits, applicable precision
and transparency, cancellation, stale results, Apply/Discard, Undo/Redo, and relevant
disable/update/remove behavior. A native worker's successful exit alone is not proof
of correct output, bundled dependencies, or clean cancellation.

MCP is optional. Discover its tools and respect the host's automation policy; current
restricted automation denies managed panel/package installation and `plugin.processImage`/`plugin.runTask`, including
indirect invocation. Do not bypass a refusal through another command or transport.
Use an already authorized desktop/local test route or report that acceptance check
as blocked while finishing source and package validation.

Finish with the artifact and source paths, required host/API/platform versions, the
checks performed and their results, and any untested or blocked acceptance cases.
Building a plugin does not itself authorize replacing a user's installation or publishing it.
