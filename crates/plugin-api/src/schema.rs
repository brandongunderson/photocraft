//! Generated JSON Schema contracts. Cross-field/authority checks also require native validation.

use schemars::{Schema, schema_for};

pub fn descriptor() -> Schema {
    let mut schema = schema_for!(crate::Descriptor);
    // `flatten` combines the common field properties with a tagged parameter's oneOf.
    // Draft 2020-12 unevaluatedProperties closes that combined shape correctly.
    if let Some(field) = schema.get_mut("$defs").and_then(|defs| defs.get_mut("Field")).and_then(serde_json::Value::as_object_mut) {
        field.insert("unevaluatedProperties".into(), serde_json::Value::Bool(false));
    }
    schema
}

pub fn panel_response() -> Schema {
    schema_for!(crate::PanelResponse)
}

pub fn host_api() -> Schema {
    schema_for!(crate::HostApi)
}

pub fn all() -> Vec<(&'static str, Schema)> {
    let mut authoring = schema_for!(crate::AuthoringManifest);
    if let Some(field) = authoring.get_mut("$defs").and_then(|defs| defs.get_mut("Field")).and_then(serde_json::Value::as_object_mut) {
        field.insert("unevaluatedProperties".into(), serde_json::Value::Bool(false));
    }
    vec![
        ("panel.schema.json", descriptor()),
        ("panel-response.schema.json", panel_response()),
        ("host-api.schema.json", host_api()),
        ("panel-event.schema.json", schema_for!(crate::EventRequest)),
        ("authoring.schema.json", authoring),
        ("package.schema.json", schema_for!(crate::PackageV2)),
        ("layers-read.schema.json", schema_for!(crate::LayersRead)),
        ("layers-edit.schema.json", schema_for!(crate::LayersEdit)),
        ("layers-snapshot.schema.json", schema_for!(crate::LayersSnapshot)),
        ("layers-edited.schema.json", schema_for!(crate::LayersEdited)),
        ("worker.schema.json", schema_for!(crate::WorkerV2Descriptor)),
        ("worker-host-frame.schema.json", schema_for!(crate::HostWorkerFrame)),
        ("worker-guest-frame.schema.json", schema_for!(crate::WorkerHostFrame)),
    ]
}
