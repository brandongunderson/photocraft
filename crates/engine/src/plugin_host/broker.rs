use std::collections::{BTreeMap, BTreeSet};
use std::sync::{
    Arc, Weak,
    atomic::{AtomicU64, Ordering},
};

use photocraft_doc::{DocId, Document, LayerId};
use photocraft_plugin_api::{ApiError, BrokerRequest, ErrorCode, InspectorView, KnownCapability};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Principal, authorize, failure, principal, require, validate_principal};
use crate::{EngineError, Result, Session};

static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);
const MAX_HANDLES: usize = 4096;

fn reserve_session() -> Option<u64> {
    let mut current = NEXT_SESSION.load(Ordering::Relaxed);
    loop {
        let next = current.checked_add(1)?;
        match NEXT_SESSION.compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return Some(current),
            Err(observed) => current = observed,
        }
    }
}

pub(crate) struct Platform {
    pub session: Option<u64>,
    pub epochs: BTreeMap<String, Arc<()>>,
    handles: BTreeMap<String, Handle>,
    next_handle: u64,
    pub views: BTreeMap<String, InspectorView>,
    pub errors: BTreeMap<String, String>,
    pub opened: BTreeMap<String, super::events::OpenView>,
    pub pending: BTreeMap<String, super::events::EventWork>,
    pub dispatch_depth: usize,
    pub pumping: bool,
    pub event_cursor: Option<String>,
    pub poll_cursor: Option<String>,
}

impl Default for Platform {
    fn default() -> Self {
        Self {
            session: reserve_session(),
            epochs: BTreeMap::new(),
            handles: BTreeMap::new(),
            next_handle: 0,
            views: BTreeMap::new(),
            errors: BTreeMap::new(),
            opened: BTreeMap::new(),
            pending: BTreeMap::new(),
            dispatch_depth: 0,
            pumping: false,
            event_cursor: None,
            poll_cursor: None,
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
enum Kind {
    Document,
    Layer(LayerId),
    Cursor(usize),
}

#[derive(Clone)]
struct Handle {
    owner: Principal,
    source: Weak<Document>,
    document: DocId,
    revision: u64,
    kind: Kind,
}

impl Platform {
    pub fn forget_document(&mut self, id: DocId) {
        self.handles.retain(|_, handle| handle.document != id);
    }
    pub fn revoke(&mut self, id: &str) {
        self.epochs.remove(id);
        self.handles.retain(|_, handle| handle.owner.id != id);
        self.opened.remove(id);
        self.pending.remove(id);
        self.views.remove(id);
        self.errors.remove(id);
    }

    fn mint(&mut self, owner: &Principal, source: &Arc<Document>, revision: u64, kind: Kind) -> Result<String> {
        self.handles.retain(|_, handle| {
            handle.source.strong_count() > 0
                && handle.owner.extension.strong_count() > 0
                && !(handle.owner.id == owner.id
                    && handle.document == source.id
                    && (handle.revision != revision || !Weak::ptr_eq(&handle.source, &Arc::downgrade(source))))
        });
        if let Some((id, _)) = self.handles.iter().find(|(_, handle)| {
            handle.owner.id == owner.id
                && handle.owner.session == owner.session
                && Weak::ptr_eq(&handle.owner.epoch, &owner.epoch)
                && Weak::ptr_eq(&handle.owner.extension, &owner.extension)
                && handle.revision == revision
                && Weak::ptr_eq(&handle.source, &Arc::downgrade(source))
                && handle.kind == kind
                && handle.owner.authorities.len() == owner.authorities.len()
                && handle.owner.authorities.iter().zip(&owner.authorities).all(|(a, b)| std::ptr::fn_addr_eq(*a, *b))
        }) {
            return Ok(id.clone());
        }
        if self.handles.len() >= MAX_HANDLES {
            return Err(failure(ErrorCode::LimitExceeded, "Plugin handle budget exhausted; request a fresh view"));
        }
        self.next_handle = self.next_handle.checked_add(1).ok_or_else(|| failure(ErrorCode::LimitExceeded, "Plugin handle identifiers exhausted"))?;
        let id = format!("pc-h-{}-{}", owner.session, self.next_handle);
        self.handles.insert(id.clone(), Handle { owner: owner.clone(), source: Arc::downgrade(source), document: source.id, revision, kind });
        Ok(id)
    }

    fn get(&self, owner: &Principal, id: &str, command: &str, params: &Value) -> Result<Handle> {
        let handle = self
            .handles
            .get(id)
            .filter(|handle| {
                handle.owner.session == owner.session
                    && handle.owner.id == owner.id
                    && Weak::ptr_eq(&handle.owner.extension, &owner.extension)
                    && Weak::ptr_eq(&handle.owner.epoch, &owner.epoch)
            })
            .ok_or_else(|| failure(ErrorCode::InvalidHandle, "Unknown, revoked or foreign plugin handle"))?;
        for policy in &handle.owner.authorities {
            policy(command, params)?;
        }
        Ok(handle.clone())
    }
}

fn source(s: &Session, handle: &Handle) -> Result<(Arc<Document>, u64)> {
    let current =
        s.documents().iter().find(|doc| doc.doc.id == handle.document).ok_or_else(|| failure(ErrorCode::InvalidHandle, "The source document was closed"))?;
    if current.revision != handle.revision || !Weak::ptr_eq(&handle.source, &Arc::downgrade(&current.doc)) {
        return Err(failure(ErrorCode::StaleRevision, "The document changed; request a new snapshot"));
    }
    Ok((Arc::clone(&current.doc), current.revision))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Read {
    document: Option<String>,
    cursor: Option<String>,
    limit: Option<usize>,
}

fn read(s: &mut Session, owner: &Principal, params: &Value) -> Result<Value> {
    require(s, owner, KnownCapability::DocumentRead)?;
    authorize(s, owner, "plugin.document.read", params)?;
    let request = params.clone();
    let params: Read = serde_json::from_value(params.clone()).map_err(|e| failure(ErrorCode::InvalidJson, e.to_string()))?;
    let limit = params.limit.unwrap_or(128);
    if !(1..=128).contains(&limit) {
        return Err(failure(ErrorCode::LimitExceeded, "Layer pages contain 1–128 rows"));
    }
    let (document, revision, offset) = if let Some(cursor) = params.cursor {
        let handle = s.extensions.platform.get(owner, &cursor, "plugin.document.read", &request)?;
        let Kind::Cursor(offset) = handle.kind else {
            return Err(failure(ErrorCode::InvalidHandle, "Expected a layer cursor"));
        };
        let (doc, revision) = source(s, &handle)?;
        if let Some(id) = params.document {
            let doc_handle = s.extensions.platform.get(owner, &id, "plugin.document.read", &request)?;
            if !matches!(doc_handle.kind, Kind::Document) || !Weak::ptr_eq(&doc_handle.source, &handle.source) {
                return Err(failure(ErrorCode::InvalidHandle, "Cursor belongs to another snapshot"));
            }
        }
        (doc, revision, offset)
    } else if let Some(document) = params.document {
        let handle = s.extensions.platform.get(owner, &document, "plugin.document.read", &request)?;
        if !matches!(handle.kind, Kind::Document) {
            return Err(failure(ErrorCode::InvalidHandle, "Expected a document handle"));
        }
        let (doc, revision) = source(s, &handle)?;
        (doc, revision, 0)
    } else if let Some(doc) = s.active() {
        (Arc::clone(&doc.doc), doc.revision, 0)
    } else {
        return Ok(json!({"document":null,"revision":null,"name":"","layers":[],"totalLayers":0,"nextCursor":null}));
    };
    let selected = s.documents().iter().find(|doc| doc.doc.id == document.id).map(|doc| doc.selected_layers.clone()).unwrap_or_default();
    let handle = s.extensions.platform.mint(owner, &document, revision, Kind::Document)?;
    let rows = document.walk();
    let total = rows.len();
    let mut layers = Vec::new();
    for (_, depth, layer) in rows.into_iter().skip(offset).take(limit) {
        let key = s.extensions.platform.mint(owner, &document, revision, Kind::Layer(layer.id))?;
        layers.push(json!({"handle":key,"name":layer.name,"visible":layer.visible,"selected":selected.contains(&layer.id),
            "kind":layer.content.kind_name(),"depth":depth,"locked":document.effective_locks(layer.id).all}));
    }
    let next = offset.saturating_add(layers.len());
    let cursor = if next < total { Some(s.extensions.platform.mint(owner, &document, revision, Kind::Cursor(next))?) } else { None };
    let result = json!({"document":handle,"revision":revision,"name":document.name,"width":document.size.width,"height":document.size.height,
        "mode":format!("{:?}",document.mode),"depth":document.depth.bits(),"layers":layers,"totalLayers":total,"nextCursor":cursor});
    if serde_json::to_vec(&result).map_err(|error| failure(ErrorCode::InvalidJson, error.to_string()))?.len() > 96 * 1024 {
        return Err(failure(ErrorCode::LimitExceeded, "Layer metadata exceeds 96 KiB; request a smaller page"));
    }
    Ok(result)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Edit {
    document: String,
    expected_revision: u64,
    edits: Vec<LayerEdit>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LayerEdit {
    layer: String,
    name: Option<String>,
    visible: Option<bool>,
}

fn edit(s: &mut Session, owner: &Principal, params: &Value) -> Result<Value> {
    // Serialize final authority validation and the atomic edit with store revocation.
    #[cfg(not(target_arch = "wasm32"))]
    let _admission = s.extensions.folder.as_deref().map(crate::extension_package::AdmissionGuard::store).transpose()?;
    require(s, owner, KnownCapability::DocumentEdit)?;
    authorize(s, owner, "plugin.document.edit", params)?;
    let request = params.clone();
    let params: Edit = serde_json::from_value(params.clone()).map_err(|e| failure(ErrorCode::InvalidJson, e.to_string()))?;
    if params.edits.is_empty() || params.edits.len() > 128 {
        return Err(failure(ErrorCode::LimitExceeded, "An edit batch needs 1–128 operations"));
    }
    let handle = s.extensions.platform.get(owner, &params.document, "plugin.document.edit", &request)?;
    if !matches!(handle.kind, Kind::Document) {
        return Err(failure(ErrorCode::InvalidHandle, "Expected a document handle"));
    }
    let (document, revision) = source(s, &handle)?;
    if revision != params.expected_revision {
        return Err(failure(ErrorCode::StaleRevision, "The edit revision does not match the snapshot"));
    }
    if !s.active().is_some_and(|doc| Arc::ptr_eq(&doc.doc, &document)) {
        return Err(failure(ErrorCode::Conflict, "The source document is no longer active"));
    }
    if s.active_job().is_some() || s.active().is_some_and(|doc| doc.floating.is_some()) {
        return Err(failure(ErrorCode::Conflict, "Finish the current image operation before applying this edit"));
    }
    let mut edits = Vec::new();
    let mut seen = BTreeSet::new();
    for op in params.edits {
        let layer = s.extensions.platform.get(owner, &op.layer, "plugin.document.edit", &request)?;
        let Kind::Layer(id) = layer.kind else {
            return Err(failure(ErrorCode::InvalidHandle, "Expected a layer handle"));
        };
        source(s, &layer)?;
        if !Weak::ptr_eq(&handle.source, &layer.source) || !seen.insert(id.0) {
            return Err(failure(ErrorCode::InvalidHandle, "Layer handles must be distinct and belong to this document"));
        }
        if op.name.is_none() && op.visible.is_none() {
            return Err(failure(ErrorCode::InvalidJson, "A layer edit needs a name or visibility"));
        }
        if op.name.as_ref().is_some_and(|name| name.trim().is_empty() || name.chars().count() > 256 || name.chars().any(char::is_control)) {
            return Err(failure(ErrorCode::InvalidJson, "Layer names need 1–256 printable characters"));
        }
        edits.push((id, op.name, op.visible));
    }
    let count = edits.len();
    // Never coalesce this atomic action with an earlier edit, even when an outer caller asked.
    let previous = s.coalesce_request.take();
    let result = s.edit("Plugin Layer Edit", move |doc, _| {
        for (id, name, visible) in edits {
            if doc.effective_locks(id).all {
                return Err(failure(ErrorCode::PermissionDenied, "A layer in the batch is locked"));
            }
            let layer = doc.layer_mut(id).ok_or_else(|| failure(ErrorCode::NotFound, "A layer in the batch was removed"))?;
            if let Some(name) = name {
                layer.name = name;
            }
            if let Some(visible) = visible {
                layer.visible = visible;
            }
        }
        Ok(())
    });
    s.coalesce_request = previous;
    result?;
    Ok(json!({"edited":count,"revision":s.active().map(|doc|doc.revision)}))
}

pub(crate) fn direct(s: &mut Session, params: &Value, editing: bool) -> Result<Value> {
    let mut fields = params.as_object().cloned().ok_or_else(|| failure(ErrorCode::InvalidJson, "Broker parameters must be an object"))?;
    let id = fields.remove("id").and_then(|value| value.as_str().map(str::to_owned)).ok_or_else(|| failure(ErrorCode::InvalidJson, "Missing plugin id"))?;
    fields.remove("coalesce");
    let owner = principal(s, &id)?;
    if editing { edit(s, &owner, &Value::Object(fields)) } else { read(s, &owner, &Value::Object(fields)) }
}

pub(crate) fn apply_requests(s: &mut Session, owner: &Principal, requests: &[BrokerRequest], readonly: bool) -> Result<(Vec<Value>, bool)> {
    validate_principal(s, owner)?;
    let editing = requests.iter().any(|request| request.method == "documents.layers.edit");
    if editing && (readonly || requests.len() != 1) {
        return Err(failure(ErrorCode::PermissionDenied, "An atomic edit must be the sole request of a user action"));
    }
    // Validate every method/capability before the first possible mutation.
    for request in requests {
        match request.method.as_str() {
            "documents.layers.read" => require(s, owner, KnownCapability::DocumentRead)?,
            "documents.layers.edit" => require(s, owner, KnownCapability::DocumentEdit)?,
            _ => return Err(failure(ErrorCode::UnsupportedOperation, format!("Unsupported broker method {}", request.method))),
        }
    }
    let mut replies = Vec::new();
    for request in requests {
        let params = Value::Object(request.params.clone());
        let result = match request.method.as_str() {
            "documents.layers.read" => read(s, owner, &params),
            "documents.layers.edit" => edit(s, owner, &params),
            _ => Err(failure(ErrorCode::UnsupportedOperation, "Unsupported broker method")),
        };
        match result {
            Ok(value) => replies.push(json!({"id":request.id,"result":value})),
            Err(error) if !editing => {
                let error = match error {
                    EngineError::PluginApi(error) => error,
                    other => ApiError::new(ErrorCode::PermissionDenied, other.to_string()),
                };
                replies.push(json!({"id":request.id,"error":error}));
            }
            Err(error) => return Err(error),
        }
    }
    Ok((replies, editing))
}
