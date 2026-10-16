//! Pure event handling: no host calls, global state, filesystem, or network.
use serde_json::{Map, Value, json};

use crate::MANIFEST;

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

fn created_layers(state: &Value) -> Result<u64, &'static str> {
    if state.is_null() {
        return Ok(0);
    }
    let state = state.as_object().ok_or("Invalid plugin state: expected null or an object.")?;
    if state.len() != 1 {
        return Err("Invalid plugin state: expected only createdLayers.");
    }
    state.get("createdLayers").and_then(Value::as_u64).ok_or("Invalid plugin state: createdLayers must be an unsigned integer.")
}

// Read choices and the default from the embedded manifest so customization has one source.
fn layer_name(values: &Map<String, Value>) -> Result<String, &'static str> {
    if values.keys().any(|key| key != "layerName") {
        return Err("Unknown panel field. Only layerName is supported.");
    }
    let manifest: Value = serde_json::from_str(MANIFEST).map_err(|_| "Could not read the embedded manifest.")?;
    let field = manifest
        .get("panel")
        .and_then(|panel| panel.get("fields"))
        .and_then(Value::as_array)
        .and_then(|fields| fields.iter().find(|field| field.get("id").and_then(Value::as_str) == Some("layerName")))
        .ok_or("The manifest needs the layerName field.")?;
    if field.get("type").and_then(Value::as_str) != Some("choice") {
        return Err("The manifest's layerName field must be a choice.");
    }
    let selected = values.get("layerName").or_else(|| field.get("default")).and_then(Value::as_str).ok_or("Layer name must be a string choice.")?;
    let options = field.get("options").and_then(Value::as_array).ok_or("The layerName field needs options.")?;
    if !options.iter().any(|option| option.as_str() == Some(selected)) {
        return Err("Choose one of the declared layer names.");
    }
    // The engine requires a string. This starter additionally keeps names short and printable.
    if selected.trim().is_empty() || selected.chars().count() > 128 || selected.chars().any(char::is_control) {
        return Err("Layer names must contain 1–128 printable characters.");
    }
    Ok(selected.to_owned())
}

struct Document<'a> {
    id: u64,
    name: &'a str,
    width: u64,
    height: u64,
    depth: u64,
    mode: &'a str,
    revision: u64,
    active_layer: Option<u64>,
}

impl<'a> Document<'a> {
    fn parse(value: &'a Value) -> Result<Self, &'static str> {
        let number = |key| value.get(key).and_then(Value::as_u64).ok_or("Invalid document metadata: expected an unsigned integer.");
        let text = |key| value.get(key).and_then(Value::as_str).ok_or("Invalid document metadata: expected a string.");
        let active_layer = match value.get("activeLayer") {
            Some(Value::Null) => None,
            Some(layer) => Some(layer.as_u64().ok_or("Invalid document metadata: activeLayer must be an unsigned integer or null.")?),
            None => return Err("Invalid document metadata: activeLayer is missing."),
        };
        Ok(Self {
            id: number("id")?,
            name: text("name")?,
            width: number("width")?,
            height: number("height")?,
            depth: number("depth")?,
            mode: text("mode")?,
            revision: number("revision")?,
            active_layer,
        })
    }

    fn message(&self) -> String {
        // Document names can be user supplied. Keep the status below the host's 4 KiB budget.
        let short_text = |text: &str| text.chars().take(128).map(|c| if c.is_control() { ' ' } else { c }).collect::<String>();
        let layer = self.active_layer.map_or_else(|| "none".to_owned(), |id| id.to_string());
        format!(
            "{} — {} × {} px, {}-bit {}, document {}, revision {}, active layer {}.",
            short_text(self.name),
            self.width,
            self.height,
            self.depth,
            short_text(self.mode),
            self.id,
            self.revision,
            layer
        )
    }
}

/// A response is a proposal. PhotoCraft executes its commands and commits state only on success.
pub(crate) fn handle(request: &Value) -> Value {
    let state = request.get("state").unwrap_or(&Value::Null);
    let request = match EventEnvelope::parse(request) {
        Ok(request) => request,
        Err(message) => return reply(state, message),
    };
    let state = &request.state;
    let count = match created_layers(state) {
        Ok(count) => count,
        Err(message) => return reply(state, message),
    };
    let event = match request.event.as_str() {
        event @ ("info" | "create") => event,
        _ => return reply(state, "Unknown action. Use a declared panel button."),
    };
    let Some(values) = request.values.as_object() else {
        return reply(state, "Invalid panel values: expected an object.");
    };
    let name = match layer_name(values) {
        Ok(name) => name,
        Err(message) => return reply(state, message),
    };
    let document = match &request.document {
        Value::Null => return reply(state, "Open an image before using this panel."),
        value => match Document::parse(value) {
            Ok(document) => document,
            Err(message) => return reply(state, message),
        },
    };
    if event == "info" {
        return reply(state, &document.message());
    }
    json!({
        "state": {"createdLayers": count.saturating_add(1)},
        "requests": [],
        "message": format!("Created layer “{name}”. Use Edit > Undo to reverse this edit."),
        "commands": [{"command": "layer.new.layer", "params": {"name": name}}]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_typed_envelope_accepts_additions_but_rejects_incompatible_versions() {
        let mut input = request("create");
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

    fn request(event: &str) -> Value {
        json!({
            "apiVersion": 2, "host": {"version":1,"capabilities":[{"id":"host.commands.invoke","version":1}]},
            "event": event, "values": {"layerName": "Notes"}, "state": null,
            "document": {"id": 1, "name": "Untitled", "width": 640, "height": 480,
                "depth": 16, "mode": "Grayscale", "revision": 3, "activeLayer": 2}
        })
    }

    #[test]
    fn creates_exactly_one_granted_command_with_object_params() {
        let response = handle(&request("create"));
        assert_eq!(response["commands"], json!([{"command": "layer.new.layer", "params": {"name": "Notes"}}]));
        assert_eq!(response["state"], json!({"createdLayers": 1}));
    }

    #[test]
    fn each_manifest_choice_and_the_default_work() {
        for name in ["Notes", "Retouch", "Overlay"] {
            let mut input = request("create");
            input["values"]["layerName"] = json!(name);
            assert_eq!(handle(&input)["commands"][0]["params"]["name"], name);
        }
        let mut input = request("create");
        input["values"] = json!({});
        assert_eq!(handle(&input)["commands"][0]["params"]["name"], "Notes");
    }

    #[test]
    fn info_includes_metadata_and_preserves_state_without_commands() {
        let mut input = request("info");
        input["state"] = json!({"createdLayers": 7});
        let response = handle(&input);
        assert_eq!(response["state"], input["state"]);
        assert_eq!(response["commands"], json!([]));
        let message = response["message"].as_str().unwrap();
        for part in ["Untitled", "640 × 480", "16-bit Grayscale", "document 1", "revision 3", "active layer 2"] {
            assert!(message.contains(part), "{message}");
        }
    }

    #[test]
    fn counter_comes_from_returned_state_and_saturates() {
        for count in [7, u64::MAX] {
            let mut input = request("create");
            input["state"] = json!({"createdLayers": count});
            assert_eq!(handle(&input)["state"]["createdLayers"], count.saturating_add(1));
        }
    }

    #[test]
    fn no_document_does_not_change_state_or_request_an_edit() {
        for event in ["info", "create"] {
            let mut input = request(event);
            input["document"] = Value::Null;
            input["state"] = json!({"createdLayers": 2});
            let response = handle(&input);
            assert_eq!(response["state"], input["state"]);
            assert_eq!(response["commands"], json!([]));
            assert!(response["message"].as_str().unwrap().contains("Open an image"));
        }
    }

    #[test]
    fn unknown_events_preserve_state() {
        for event in [json!("delete"), json!(42), Value::Null] {
            let mut input = request("create");
            input["event"] = event;
            input["state"] = json!({"createdLayers": 5});
            let response = handle(&input);
            assert_eq!(response["state"], input["state"]);
            assert_eq!(response["commands"], json!([]));
        }
    }

    #[test]
    fn malformed_values_never_request_commands() {
        for values in [
            Value::Null,
            json!([]),
            json!("Notes"),
            json!({"unknown": 1}),
            json!({"layerName": false}),
            json!({"layerName": ""}),
            json!({"layerName": "Other"}),
            json!({"layerName": "x".repeat(1024)}),
        ] {
            let mut input = request("create");
            input["values"] = values;
            input["state"] = json!({"createdLayers": 9});
            let response = handle(&input);
            assert_eq!(response["commands"], json!([]));
            assert_eq!(response["state"], input["state"]);
        }
    }

    #[test]
    fn malformed_state_is_reported_and_preserved() {
        for state in [
            json!(3),
            json!({}),
            json!({"createdLayers": -1}),
            json!({"createdLayers": 1.5}),
            json!({"createdLayers": "1"}),
            json!({"createdLayers": 1, "extra": true}),
        ] {
            let mut input = request("create");
            input["state"] = state;
            let response = handle(&input);
            assert_eq!(response["state"], input["state"]);
            assert_eq!(response["commands"], json!([]));
            assert!(response["message"].as_str().unwrap().contains("Invalid plugin state"));
        }
    }

    #[test]
    fn malformed_document_metadata_never_requests_an_edit() {
        for document in [json!(false), json!({}), json!({"id": "one"})] {
            let mut input = request("create");
            input["document"] = document;
            assert_eq!(handle(&input)["commands"], json!([]));
        }
    }

    #[test]
    fn missing_and_unknown_request_fields_are_reported() {
        let mut input = request("create");
        input.as_object_mut().unwrap().remove("document");
        assert_eq!(handle(&input)["commands"], json!([]));
        input["unexpected"] = json!(true);
        assert_eq!(handle(&input)["commands"], json!([]));
        for input in [Value::Null, json!([]), json!(false)] {
            assert_eq!(handle(&input)["commands"], json!([]));
        }
    }

    #[test]
    fn large_unicode_metadata_stays_below_the_status_budget() {
        let mut input = request("info");
        input["document"]["name"] = json!("😀\n".repeat(4096));
        input["document"]["mode"] = json!("😀".repeat(4096));
        input["document"]["activeLayer"] = Value::Null;
        let response = handle(&input);
        let message = response["message"].as_str().unwrap();
        assert!(message.len() <= 4096);
        assert!(!message.contains('\n'));
        assert!(message.contains("active layer none"));
    }
}
