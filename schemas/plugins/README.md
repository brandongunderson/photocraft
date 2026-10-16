# Plugin contract schemas

These Draft 2020-12 JSON Schemas are generated from the independent `photocraft-plugin-api`
crate. Use the `export_schemas` example with its optional `schema` feature; the crate's
all-features contract test checks exact generation consistency.

- `panel-v2.schema.json`: complete API-2 descriptor.
- `panel-v2-response.schema.json`: bounded API-2 response structure.
- `host-api-v1.schema.json`: capability-discovery metadata, independently versioned.
- `panel-v2-event.schema.json`: typed host event and correlated broker replies.
- `authoring-v1.schema.json`: single source descriptor and package authoring input.
- `package-v2.schema.json`: current package metadata; native target compatibility is checked by the host.
- `layers-read-v1.schema.json`, `layers-edit-v1.schema.json`: current document broker requests.
- `layers-snapshot-v1.schema.json`, `layers-edited-v1.schema.json`: document broker success results.

Schemas validate structural shape, not authority. Native validators also enforce cross-field
constraints, byte budgets and owner namespaces. The host checks grants and scoped handles.
See [panel API 2](../../docs/plugin-api-v2.md) and the [standalone SDK](../../docs/plugin-sdk.md). Existing panel-1/package-2 readers remain
separate. Package format 3 is an RFC and has no supported schema yet.
