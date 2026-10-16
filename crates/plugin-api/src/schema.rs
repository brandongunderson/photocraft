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
        ("panel-v2.schema.json", descriptor()),
        ("panel-v2-response.schema.json", panel_response()),
        ("host-api-v1.schema.json", host_api()),
        ("panel-v2-event.schema.json", schema_for!(crate::EventRequest)),
        ("authoring-v1.schema.json", authoring),
        ("package-v2.schema.json", schema_for!(crate::PackageV2)),
        ("layers-read-v1.schema.json", schema_for!(crate::LayersRead)),
        ("layers-edit-v1.schema.json", schema_for!(crate::LayersEdit)),
        ("layers-snapshot-v1.schema.json", schema_for!(crate::LayersSnapshot)),
        ("layers-edited-v1.schema.json", schema_for!(crate::LayersEdited)),
        ("worker-v2.schema.json", schema_for!(crate::WorkerV2Descriptor)),
        ("worker-v2-host-frame.schema.json", schema_for!(crate::HostWorkerFrame)),
        ("worker-v2-guest-frame.schema.json", schema_for!(crate::WorkerHostFrame)),
    ]
}
