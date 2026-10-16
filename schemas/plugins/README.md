# Plugin contract schemas

These Draft 2020-12 JSON Schemas describe the PhotoCraft Plugin API and are generated
from the independent `photocraft-plugin-api` crate. Use the `export_schemas` example with its optional `schema` feature; the crate's
all-features contract test checks exact generation consistency.

- `panel.schema.json`: plugin descriptor, including optional repository and update metadata.
- `panel-response.schema.json`: bounded panel responses.
- `panel-event.schema.json`: host events and correlated broker replies.
- `host-api.schema.json`: host capability discovery.
- `authoring.schema.json`: single source descriptor and package authoring input.
- `package.schema.json`: package metadata; the host also checks native target compatibility.
- `layers-read.schema.json`, `layers-edit.schema.json`: document broker requests.
- `layers-snapshot.schema.json`, `layers-edited.schema.json`: document broker results.
- `worker.schema.json`: private persistent-worker declaration and resource estimates.
- `worker-host-frame.schema.json`, `worker-guest-frame.schema.json`: bounded worker transport frames.

Schemas validate structural shape, not authority. Native validators also enforce cross-field
constraints, byte budgets and owner namespaces. The host checks grants and scoped handles.
See [the PhotoCraft Plugin API](../../docs/plugin-api.md) and the [standalone SDK](../../docs/plugin-sdk.md).
