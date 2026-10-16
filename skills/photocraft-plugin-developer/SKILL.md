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

Current authoring contracts are panel API 1, experimental panel API 2, filter ABI 1,
package format 2, and native worker protocol 1. Package format 3 and worker protocol 2
are proposals. The application release number alone does not establish API support.

Use the running host's `extension.capabilities`, `extension.commands`, and command
descriptions where available. Discovery reports availability, not permission. An
offline build can use the supplied reference snapshot, but report host compatibility
as unverified until exercised against the intended build.

Read [API contracts](references/api-contracts.md) for version boundaries, payloads,
ownership, and current limits. In a PhotoCraft source/kit, consult `docs/plugin-development.md`
and the relevant `docs/extensions.md`, `docs/plugins.md`, `docs/plugin-api-v2.md`,
`docs/plugin-image-processing.md`, or `docs/plugin-native-tasks.md` for full details.
Those documents are authoring references, not runtime source dependencies.

## Choose the smallest suitable starting point

| Feature | Starter path in a source checkout or developer kit | Result |
|---|---|---|
| Panel combining permitted editor commands | `examples/extensions/plugin-starter` | Panel API 1 `.pcext` |
| Pixel algorithm using the filter target's depth and channels | `examples/plugins/filter-starter` | Filter ABI 1 `.wasm` |
| Panel with its own executable and/or bundled WASM algorithm | `examples/extensions/image-plugin-starter` | Package 2 `.pcplugin` |
| Namespaced command and atomic layer rename/visibility edits | `examples/extensions/layer-organizer` | Experimental API 2 `.pcplugin` |
| Native read-only inspector refreshed by document changes | `examples/extensions/document-inspector` | Experimental API 2 `.pcplugin` |

Use API 1 when it covers the feature. Select API 2 deliberately for its implemented
capabilities, without changing package format. Native maintenance tasks extend a
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
  updating API-2 contribution namespaces and bundled processor references as well.
  Rebuild embedded manifests; editing a loose installed JSON file does not update WASM.
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
