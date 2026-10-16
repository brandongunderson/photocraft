# PhotoCraft Plugin API

An independently publishable, data-only contract crate for standalone PhotoCraft plugins.
It depends on serialization libraries, never on the editor, document model, UI or sibling plugins.

`Descriptor::parse`, `Capabilities::validate`, `Contributions::validate`, `negotiate` and
`PanelResponse::parse` validate the current descriptor, permissions, capabilities and messages.
Authors set the internal panel discriminator to `2`; old panel version `1` is unsupported.
Optional `repository` and `updates` metadata describe a plugin-owned public GitHub release
source. `github_repository_parts`, `UpdateSource::validate` and `asset_for_platform` validate
the canonical repository and exact portable/platform asset name without performing I/O.
Declaring an update source grants no network or installation authority to a guest.

`EventRequest`, `BrokerReply`, `LayersRead`, `LayersEdit` and the corresponding snapshot
types describe the implemented broker wire messages. `AuthoringManifest` validates the
single source input for package-2 authoring. These types expose no internal Rust layouts.
The optional [guest SDK](../plugin-sdk/README.md) adds bounded ABI storage and manifest
generation; SDK adoption is optional for conforming plugins. The matching developer kit includes
the full SDK guide in `docs/plugin-sdk.md`.

Capability discovery describes availability; it grants no authority. Only the host creates a
principal and binds a `ScopedHandle` to the correct plugin, generation and invocation. A plugin
cannot grant itself permission by manufacturing a descriptor or JSON handle.

Enable `schema` to generate Draft 2020-12 JSON Schema with the `export_schemas` example. The
checked-in schemas are generated contracts, with additional cross-field checks documented in the
API RFC. Versions of this crate, the host metadata, panels and packages are independent.

The crate release is experimental (`0.1.0`). PhotoCraft supports one public Plugin API and
evolves it through compatible extensions; a new major is reserved for a genuine breaking change.
Older owned panels must be rebuilt for the current API, without acquiring editor or sibling
runtime dependencies. Filter ABI 1, package format 2 and native transports remain independent.
