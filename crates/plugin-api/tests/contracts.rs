use photocraft_plugin_api::*;
use serde_json::{Value, json};

fn descriptor() -> Value {
    json!({"apiVersion":2,"id":"org.example.layers","name":"Layer Organizer","version":"0.1.0",
        "permissions":{"documentRead":true,"commands":[]},
        "panel":{"fields":[{"id":"visible","label":"Visible","type":"bool","default":true}],"buttons":[{"id":"apply","label":"Apply"}]},
        "capabilities":{"required":[{"id":"host.document.edit","minVersion":1,"maxVersion":1}],"optional":[{"id":"future.optional","minVersion":1}]},
        "contributes":{"commands":[{"id":"org.example.layers.organize","label":"Organize Layers","event":"organize","params":{},"needsDocument":true}]}})
}

#[test]
fn complete_descriptor_round_trip_is_independent_of_editor_types() {
    let bytes = serde_json::to_vec(&descriptor()).unwrap();
    let first = Descriptor::parse(&bytes).unwrap();
    let second = Descriptor::parse(&serde_json::to_vec(&first).unwrap()).unwrap();
    assert_eq!(first, second);
    assert_eq!(first.contributes.commands[0].event, "organize");
}

#[test]
fn missing_required_and_wrong_version_have_structured_errors_optional_falls_back() {
    let parsed = Descriptor::parse(&serde_json::to_vec(&descriptor()).unwrap()).unwrap();
    let absent = negotiate(&parsed.capabilities, &[]).unwrap_err();
    assert_eq!(absent.code, ErrorCode::MissingCapability);
    assert_eq!(absent.details["capability"], "host.document.edit");
    let host = [CapabilitySupport { id: KnownCapability::DocumentEdit.into(), version: 1 }];
    let accepted = negotiate(&parsed.capabilities, &host).unwrap();
    assert_eq!(accepted.available, host);
    assert_eq!(accepted.unavailable_optional[0].as_str(), "future.optional");
    let wrong = [CapabilitySupport { id: KnownCapability::DocumentEdit.into(), version: 2 }];
    assert_eq!(negotiate(&parsed.capabilities, &wrong).unwrap_err().code, ErrorCode::UnsupportedVersion);
}

#[test]
fn malformed_duplicate_and_unbounded_descriptors_fail_without_panicking() {
    for (pointer, invalid) in [
        ("/apiVersion", json!(1)),
        ("/id", json!("../escape")),
        ("/name", json!("\u{202e}Layers")),
        ("/capabilities/required/0/minVersion", json!(0)),
        ("/capabilities/required/0/maxVersion", json!(0)),
        ("/capabilities/required/0/id", json!("malformed id")),
        ("/contributes/commands/0/id", json!("org.another.plugin.command")),
        ("/contributes/commands/0/event", json!("_hidden")),
        ("/contributes/commands/0/params", json!([])),
    ] {
        let mut value = descriptor();
        *value.pointer_mut(pointer).unwrap() = invalid;
        assert!(Descriptor::parse(&serde_json::to_vec(&value).unwrap()).is_err(), "{pointer}");
    }
    let mut duplicate = descriptor();
    duplicate["capabilities"]["optional"] = duplicate["capabilities"]["required"].clone();
    assert!(Descriptor::parse(&serde_json::to_vec(&duplicate).unwrap()).is_err());
    let mut value = descriptor();
    value["unexpected"] = json!(true);
    assert!(Descriptor::parse(&serde_json::to_vec(&value).unwrap()).is_err());
    assert!(Descriptor::parse(&vec![b' '; MAX_DESCRIPTOR_BYTES + 1]).is_err());
    assert!(Descriptor::parse(&[0xff]).is_err());
    assert!(Descriptor::parse(br#"{"apiVersion":2,"apiVersion":2}"#).is_err());
    let nested = format!("{}0{}", "[".repeat(1024), "]".repeat(1024));
    assert!(Descriptor::parse(nested.as_bytes()).is_err());
}

#[test]
fn contributions_are_unique_across_categories_and_inspector_needs_no_button() {
    let mut value = descriptor();
    value["panel"]["buttons"] = json!([]);
    assert!(Descriptor::parse(&serde_json::to_vec(&value).unwrap()).is_err());
    value["contributes"]["inspectors"] = json!([{"id":"org.example.layers.inspector","label":"Document Inspector","event":"inspect"}]);
    value["contributes"]["subscriptions"] = json!([{"id":"org.example.layers.events","event":"document.changed","handler":"inspect"}]);
    assert!(Descriptor::parse(&serde_json::to_vec(&value).unwrap()).is_ok());
    value["contributes"]["inspectors"][0]["id"] = json!("org.example.layers.organize");
    assert!(Descriptor::parse(&serde_json::to_vec(&value).unwrap()).is_err());
}

#[test]
fn response_controls_are_bounded_and_do_not_grant_authority() {
    let response = json!({"requests":[{"id":"read","method":"host.document.read","params":{"document":"fabricated"}}],
        "view":{"title":"Layers","rows":[{"label":"Count","value":"3"}]}});
    let parsed = PanelResponse::parse(&serde_json::to_vec(&response).unwrap()).unwrap();
    // Parsing an opaque handle deliberately says nothing about whether the host accepts it.
    assert_eq!(parsed.requests[0].params["document"], "fabricated");
    let mut mixed = response.clone();
    mixed["commands"] = json!([{"command":"edit.fill","params":{}}]);
    assert!(PanelResponse::parse(&serde_json::to_vec(&mixed).unwrap()).is_err());
    let mut duplicate = response;
    let repeated = duplicate["requests"][0].clone();
    duplicate["requests"].as_array_mut().unwrap().push(repeated);
    assert!(PanelResponse::parse(&serde_json::to_vec(&duplicate).unwrap()).is_err());
    assert!(PanelResponse::parse(&serde_json::to_vec(&json!({"state":"x".repeat(MAX_STATE_BYTES)})).unwrap()).is_err());
}

#[test]
fn metadata_versions_and_error_codes_round_trip_independently() {
    let metadata = HostApi { version: HOST_API_VERSION, capabilities: vec![CapabilitySupport { id: KnownCapability::DocumentRead.into(), version: 1 }] };
    assert_eq!(serde_json::from_value::<HostApi>(serde_json::to_value(&metadata).unwrap()).unwrap(), metadata);
    let error = ApiError::at(ErrorCode::InvalidHandle, "document", "handle expired");
    assert_eq!(serde_json::from_value::<ApiError>(serde_json::to_value(&error).unwrap()).unwrap(), error);
    assert!(CapabilityId::new("future.optional").unwrap().known().is_none());
    assert_eq!(CapabilityId::from(KnownCapability::Events).known(), Some(KnownCapability::Events));
    assert!(metadata.validate().is_ok());
    assert_eq!(HostApi { version: 0, ..HostApi::default() }.validate().unwrap_err().code, ErrorCode::IncompatibleApi);
    let repeated = HostApi { version: HOST_API_VERSION, capabilities: vec![metadata.capabilities[0].clone(), metadata.capabilities[0].clone()] };
    assert!(repeated.validate().is_err());
}

#[cfg(feature = "schema")]
#[test]
fn checked_in_json_schemas_match_rust_contract_generation() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("schemas");
    for (name, generated) in schema::all() {
        let saved: Value = serde_json::from_slice(&std::fs::read(root.join(name)).unwrap()).unwrap();
        assert_eq!(saved, serde_json::to_value(generated).unwrap(), "regenerate {name} using export_schemas");
        assert_eq!(saved["$schema"], "https://json-schema.org/draft/2020-12/schema");
        let public = root.join("../../../schemas/plugins").join(name);
        if public.exists() {
            let exported: Value = serde_json::from_slice(&std::fs::read(public).unwrap()).unwrap();
            assert_eq!(saved, exported, "public schema {name} differs from independent crate snapshot");
        }
    }
}

#[test]
fn actual_host_envelope_and_nullable_replies_decode_with_future_event_fields() {
    let mut request = json!({"apiVersion":2,"host":{"version":1,"capabilities":[]},
        "compatibility":{"panelApi":2,"hostApi":1,"negotiated":{"available":[],"unavailableOptional":[]}},
        "event":"host_response","values":{},"state":{"saved":7},"document":null,
        "responses":[{"id":"read","result":null}],"futureOptionalField":{"version":1}});
    let event = EventRequest::parse(&serde_json::to_vec(&request).unwrap()).unwrap();
    assert_eq!(event.reply("read").unwrap().decode::<Value>().unwrap(), Value::Null);
    request["responses"][0]["error"] = json!({"code":"conflict","message":"no","details":{}});
    assert!(EventRequest::parse(&serde_json::to_vec(&request).unwrap()).is_err());
    request["responses"] = json!([{"id":"read","error":{"code":"staleRevision","message":"changed","details":{}}}]);
    let event = EventRequest::parse(&serde_json::to_vec(&request).unwrap()).unwrap();
    assert_eq!(event.reply("read").unwrap().decode::<Value>().unwrap_err().code, ErrorCode::StaleRevision);
}

#[test]
fn invalid_versions_duplicate_replies_and_oversized_state_fail_before_a_handler() {
    let event = json!({"apiVersion":2,"host":{"version":1,"capabilities":[]},"event":"info","state":null});
    for change in [
        json!({"apiVersion":1}),
        json!({"event":"_reserved"}),
        json!({"responses":[{"id":"r","result":{}},{"id":"r","result":{}}]}),
        json!({"state":"x".repeat(129*1024)}),
    ] {
        let mut request = event.clone();
        request.as_object_mut().unwrap().extend(change.as_object().unwrap().clone());
        assert!(EventRequest::parse(&serde_json::to_vec(&request).unwrap()).is_err());
    }
}

#[test]
fn typed_broker_requests_match_existing_json_and_reject_partial_or_duplicate_edits() {
    let read = LayersRead { limit: Some(128), ..Default::default() }.request("layers").unwrap();
    assert_eq!(serde_json::to_value(read).unwrap(), json!({"id":"layers","method":"documents.layers.read","params":{"limit":128}}));
    let op = LayerEdit { layer: ScopedHandle("opaque-layer".into()), name: Some("Review".into()), visible: None };
    let mut edit = LayersEdit { document: ScopedHandle("opaque-document".into()), expected_revision: 9, edits: vec![op.clone()] };
    assert_eq!(edit.request("edit").unwrap().params["expectedRevision"], 9);
    edit.edits.push(op);
    assert_eq!(edit.request("edit").unwrap_err().code, ErrorCode::InvalidHandle);
    edit.edits.clear();
    assert!(edit.request("edit").is_err());
    assert!(LayersRead { limit: Some(129), ..Default::default() }.request("layers").is_err());
}

#[test]
fn no_document_snapshot_and_future_layer_metadata_remain_readable() {
    let snapshot: LayersSnapshot =
        serde_json::from_value(json!({"document":null,"revision":null,"name":"","layers":[],"totalLayers":0,"nextCursor":null})).unwrap();
    assert!(snapshot.document.is_none());
    assert!(snapshot.width.is_none());
    let layer: LayerSnapshot =
        serde_json::from_value(json!({"handle":"opaque","name":"A","visible":true,"selected":false,"kind":"pixel","depth":0,"locked":false,"future":true}))
            .unwrap();
    assert_eq!(layer.handle.0, "opaque");
}

#[test]
fn authoring_rejects_unsafe_paths_versions_and_inconsistent_payload_grants() {
    let mut value = json!({"formatVersion":1,"descriptor":descriptor(),"package":{"packageVersion":2,"module":"plugin.pcext"}});
    assert!(AuthoringManifest::parse(&serde_json::to_vec(&value).unwrap()).is_ok());
    value["descriptor"]["permissions"]["commands"] = json!(["plugin.processImage"]);
    for name in ["../filter.wasm", "CON/filter.wasm", "filters/NUL.wasm", "plugin.pcext/filter.wasm", "LICENSE/filter.wasm", "filters\\gain.wasm"] {
        value["package"]["filters"] = json!([{"module":name}]);
        assert!(AuthoringManifest::parse(&serde_json::to_vec(&value).unwrap()).is_err(), "{name}");
    }
    value["package"]["filters"] = json!([{"module":"filters/gain.wasm"}]);
    assert!(AuthoringManifest::parse(&serde_json::to_vec(&value).unwrap()).is_ok());
    value["descriptor"]["permissions"]["commands"] = json!([]);
    assert!(AuthoringManifest::parse(&serde_json::to_vec(&value).unwrap()).is_err());
    value["package"]["filters"] = json!([]);
    value["formatVersion"] = json!(2);
    assert_eq!(AuthoringManifest::parse(&serde_json::to_vec(&value).unwrap()).unwrap_err().code, ErrorCode::UnsupportedVersion);
    value["formatVersion"] = json!(1);
    value["package"]["packageVersion"] = json!(3);
    assert!(AuthoringManifest::parse(&serde_json::to_vec(&value).unwrap()).is_err());
}
