//! Panel events propose a command; pixels remain in the host and selected backend.
use serde_json::{Value, json};

use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EventEnvelope {
    api_version: u32,
    host: HostMetadata,
    event: String,
    values: Value,
    #[serde(default)]
    state: Value,
    #[serde(default)]
    document: Value,
    #[serde(default)]
    responses: Vec<Value>,
    #[serde(default)]
    compatibility: Option<Compatibility>,
}

#[derive(Deserialize)]
struct HostMetadata {
    version: u16,
    capabilities: Vec<Capability>,
}

#[derive(Deserialize)]
struct Capability {
    id: String,
    version: u16,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Compatibility {
    panel_api: u32,
    host_api: u16,
}

impl EventEnvelope {
    fn parse(request: &Value) -> Result<Self, &'static str> {
        let event: Self = serde_json::from_value(request.clone()).map_err(|_| "Invalid plugin event envelope.")?;
        let mut capabilities = std::collections::BTreeSet::new();
        if event.api_version != 2
            || event.host.version != 1
            || event.host.capabilities.len() > 32
            || event.responses.len() > 8
            || event.host.capabilities.iter().any(|offer| !(1..=1024).contains(&offer.version) || !capabilities.insert(&offer.id))
            || !event.host.capabilities.iter().any(|offer| offer.id == "host.commands.invoke" && offer.version >= 1)
            || event.compatibility.as_ref().is_some_and(|selected| selected.panel_api != 2 || selected.host_api != event.host.version)
        {
            return Err("The host does not support this plugin's event or command contract.");
        }
        Ok(event)
    }
}

fn reply(state: &Value, message: &str) -> Value {
    json!({"state": state, "message": message, "commands": [], "requests": []})
}

pub(crate) fn handle(request: &Value) -> Value {
    let state = request.get("state").unwrap_or(&Value::Null);
    let request = match EventEnvelope::parse(request) {
        Ok(request) => request,
        Err(message) => return reply(state, message),
    };
    let state = &request.state;
    if request.event != "process" {
        return reply(state, "Unknown action. Use Generate preview.");
    }
    let Some(values) = request.values.as_object() else { return reply(state, "Panel values must be an object.") };
    if values.keys().any(|key| !matches!(key.as_str(), "gain" | "backend")) {
        return reply(state, "Unknown panel field.");
    }
    let gain = match values.get("gain") {
        None => 1.25,
        Some(value) => match value.as_f64().filter(|gain| gain.is_finite() && (0.0..=2.0).contains(gain)) {
            Some(gain) => gain,
            None => return reply(state, "Gain must be a number from 0 to 2."),
        },
    };
    let processor = match values.get("backend").and_then(Value::as_str) {
        None if !values.contains_key("backend") => "native",
        Some("Native worker") => "native",
        Some("Sandboxed filter") => "org.example.channel-gain",
        _ => return reply(state, "Choose a declared processing backend."),
    };
    let Some(document) = request.document.as_object() else {
        return reply(state, "Open an RGB or grayscale image before generating a preview.");
    };
    if !["id", "width", "height", "revision"].iter().all(|key| document.get(*key).and_then(Value::as_u64).is_some()) {
        return reply(state, "The document summary is incomplete.");
    }
    json!({
        "state": state,
        "requests": [],
        "message": "Processing image. Compare the preview, then choose Apply or Discard.",
        "commands": [{"command": "plugin.processImage", "params": {
            "processor": processor, "options": {"gain": gain}, "preview": true
        }}]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_typed_envelope_accepts_additions_but_rejects_incompatible_versions() {
        let mut input = request();
        input["futureMetadata"] = json!({"optional":true});
        input["compatibility"] = json!({"panelApi":2,"hostApi":1,"negotiated":{"available":[],"unavailableOptional":[]}});
        assert!(!handle(&input)["commands"].as_array().unwrap().is_empty());
        for version in [0, 1, 3] {
            input["apiVersion"] = json!(version);
            let response = handle(&input);
            assert_eq!(response["commands"], json!([]));
            assert_eq!(response["state"], input["state"]);
        }
        input["apiVersion"] = json!(2);
        input["host"]["capabilities"] = json!([]);
        assert_eq!(handle(&input)["commands"], json!([]), "capability discovery is explicit");
    }

    fn request() -> Value {
        json!({"apiVersion":2,"host":{"version":1,"capabilities":[{"id":"host.commands.invoke","version":1}]},
            "event": "process", "values": {}, "state": {"custom": 1},
            "document": {"id": 1, "width": 8, "height": 6, "revision": 0}})
    }

    #[test]
    fn native_defaults_request_only_the_own_installation_processor() {
        let input = request();
        let result = handle(&input);
        assert_eq!(
            result["commands"],
            json!([{"command": "plugin.processImage", "params": {
                "processor": "native", "options": {"gain": 1.25}, "preview": true
            }}])
        );
        assert_eq!(result["state"], input["state"]);
        assert!(result["commands"][0]["params"].get("id").is_none());
    }

    #[test]
    fn packaged_filter_is_selected_by_its_embedded_manifest_id() {
        let mut input = request();
        input["values"] = json!({"backend": "Sandboxed filter", "gain": 0.5});
        let result = handle(&input);
        assert_eq!(result["commands"][0]["params"]["processor"], "org.example.channel-gain");
        assert_eq!(result["commands"][0]["params"]["options"], json!({"gain": 0.5}));
    }

    #[test]
    fn malformed_values_and_requests_never_start_processing() {
        for values in [
            json!([]),
            json!(null),
            json!({"gain": -1}),
            json!({"gain": 2.1}),
            json!({"gain": "1"}),
            json!({"backend": false}),
            json!({"backend": "another-plugin"}),
            json!({"path": "worker.exe"}),
        ] {
            let mut input = request();
            input["values"] = values;
            assert_eq!(handle(&input)["commands"], json!([]));
        }
        for input in [json!([]), json!(null), json!({"event": "delete"})] {
            assert_eq!(handle(&input)["commands"], json!([]));
        }
    }

    #[test]
    fn no_or_malformed_document_preserves_state_without_commands() {
        for document in [json!(null), json!({}), json!({"id": "1"})] {
            let mut input = request();
            input["document"] = document;
            let result = handle(&input);
            assert_eq!(result["commands"], json!([]));
            assert_eq!(result["state"], input["state"]);
        }
    }
}
