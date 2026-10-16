# PhotoCraft Plugin API

An independently publishable, data-only contract crate for standalone PhotoCraft plugins.
It depends on serialization libraries, never on the editor, document model, UI or sibling plugins.

`Descriptor::parse` validates panel-2 descriptors with the same declarative control vocabulary as
panel-1. `Capabilities::validate`, `Contributions::validate`, `negotiate` and `PanelResponse::parse`
also serve hosts which retain their original manifest/control implementations.

`EventRequest`, `BrokerReply`, `LayersRead`, `LayersEdit` and the corresponding snapshot
types describe the implemented broker wire messages. `AuthoringManifest` validates the
single source input for package-2 authoring. These types expose no internal Rust layouts.
The optional [guest SDK](../plugin-sdk/README.md) adds bounded ABI storage and manifest
generation; it is not required by existing binaries. The matching developer kit includes
the full SDK guide in `docs/plugin-sdk.md`.

Capability discovery describes availability; it grants no authority. Only the host creates a
principal and binds a `ScopedHandle` to the correct plugin, generation and invocation. A plugin
cannot grant itself permission by manufacturing a descriptor or JSON handle.

Enable `schema` to generate Draft 2020-12 JSON Schema with the `export_schemas` example. The
checked-in schemas are generated contracts, with additional cross-field checks documented in the
API RFC. Versions of this crate, the host metadata, panels and packages are independent.

The initial API is experimental (`0.1.0`). Current API-1 plugins continue to use the unchanged
legacy adapters and do not need this crate or a rebuild.
