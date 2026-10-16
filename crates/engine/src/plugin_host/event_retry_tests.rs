use super::*;
use crate::plugin_host::tests::{deny_read, inspector_manifest, install, session};
use photocraft_plugin_api::BrokerRequest;

#[test]
fn busy_launch_retries_keep_original_event_authority_after_caller_policy_is_cleared() {
    let id = "org.example.platform";
    let mut s = session();
    install(&mut s, &inspector_manifest(), &json!({}), false).unwrap();
    // Open directly so the production automatic event pump cannot run ahead of this fixture.
    open(&mut s, &json!({"id":id})).unwrap();
    s.authorize = Some(deny_read);
    s.plugin_document_changed();
    s.authorize = None;
    let busy = start_with(&mut s, id, |_, _| Err(failure(ErrorCode::Conflict, "All event workers are occupied")));
    assert!(matches!(busy, Err(EngineError::PluginApi(error)) if error.code == ErrorCode::Conflict));
    assert_eq!(s.extensions.platform.opened[id].authorities.len(), 1);
    // A deterministic successful admission exercises the same principal/queue handoff without
    // reserving global worker slots or depending on timing of other parallel tests.
    let (_sender, receiver) = mpsc::sync_channel(1);
    start_with(&mut s, id, |_, _| Ok(receiver)).unwrap();
    assert!(s.extensions.platform.opened[id].authorities.is_empty());
    let principal = s.extensions.platform.pending[id].principal.clone();
    let request = BrokerRequest { id: "read".into(), method: "documents.layers.read".into(), params: serde_json::Map::new() };
    let (replies, edited) = broker::apply_requests(&mut s, &principal, &[request], true).unwrap();
    assert!(!edited);
    assert!(replies[0]["error"]["message"].as_str().unwrap().contains("read denied by current caller"));
    assert!(replies[0].get("result").is_none(), "the retried observer must not receive a layer snapshot");
}

#[test]
fn concurrent_guest_state_change_preserves_pending_event_authority_on_retry() {
    let id = "org.example.platform";
    let mut s = session();
    install(&mut s, &inspector_manifest(), &json!({}), false).unwrap();
    open(&mut s, &json!({"id":id})).unwrap();
    s.authorize = Some(deny_read);
    s.plugin_document_changed();
    s.authorize = None;
    let (_sender, receiver) = mpsc::sync_channel(1);
    start_with(&mut s, id, |_, _| Ok(receiver)).unwrap();
    let work = s.extensions.platform.pending.remove(id).unwrap();
    s.extensions.installed.get_mut(id).unwrap().state = json!({"local":"newer state"});
    assert!(!apply(&mut s, id, work, Response::default()).unwrap());
    let (_sender, receiver) = mpsc::sync_channel(1);
    start_with(&mut s, id, |_, _| Ok(receiver)).unwrap();
    let principal = s.extensions.platform.pending[id].principal.clone();
    let request = BrokerRequest { id: "read".into(), method: "documents.layers.read".into(), params: serde_json::Map::new() };
    let (replies, _) = broker::apply_requests(&mut s, &principal, &[request], true).unwrap();
    assert!(replies[0]["error"]["message"].as_str().unwrap().contains("read denied by current caller"));
    assert_eq!(s.extensions.installed[id].state, json!({"local":"newer state"}));
}

#[test]
fn busy_query_continuations_retain_prepared_replies_and_original_authority() {
    let id = "org.example.platform";
    let mut s = session();
    install(&mut s, &inspector_manifest(), &json!({}), false).unwrap();
    open(&mut s, &json!({"id":id})).unwrap();
    s.authorize = Some(deny_read);
    s.plugin_document_changed();
    s.authorize = None;
    let (_sender, receiver) = mpsc::sync_channel(1);
    start_with(&mut s, id, |_, _| Ok(receiver)).unwrap();
    let work = s.extensions.platform.pending.remove(id).unwrap();
    let response = Response::parse(br#"{"requests":[{"id":"read","method":"documents.layers.read","params":{}}]}"#).unwrap();
    assert!(!apply_with(&mut s, id, work, response, |_, _| Err(failure(ErrorCode::Conflict, "Workers occupied"))).unwrap());
    let mut work = s.extensions.platform.pending.remove(id).unwrap();
    let prepared = work.continuation.clone().unwrap();
    assert_eq!(prepared["event"], "host_response");
    assert!(prepared["responses"][0]["error"]["message"].as_str().unwrap().contains("read denied by current caller"));
    assert_eq!(work.rounds, 2);
    assert!(!resume_with(&mut work, |_, _| Err(failure(ErrorCode::Conflict, "Still occupied"))).unwrap());
    assert_eq!(work.continuation.as_ref(), Some(&prepared));
    assert_eq!(work.rounds, 2, "waiting for admission does not spend another guest query round");
    let (_sender, receiver) = mpsc::sync_channel(1);
    let mut launched = None;
    assert!(
        resume_with(&mut work, |_, request| {
            launched = Some(request);
            Ok(receiver)
        })
        .unwrap()
    );
    assert_eq!(launched, Some(prepared));
    assert!(work.continuation.is_none());
    let request = BrokerRequest { id: "another_read".into(), method: "documents.layers.read".into(), params: serde_json::Map::new() };
    let (replies, _) = broker::apply_requests(&mut s, &work.principal, &[request], true).unwrap();
    assert!(replies[0].get("result").is_none());
    assert!(replies[0]["error"]["message"].as_str().unwrap().contains("read denied by current caller"));
}
