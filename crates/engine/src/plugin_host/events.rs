//! Coalesced document invalidations. Mutation hooks never call guest code.
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{
    Arc,
    mpsc::{self, Receiver, TryRecvError},
};

use photocraft_plugin_api::{ErrorCode, KnownCapability};
use photocraft_plugins::extension::{Extension, Response};
use serde_json::{Value, json};

use super::{Authority, Platform, Principal, authorize, broker, event_request, failure, principal, require, validate_principal};
use crate::{EngineError, Result, Session};

static EVENT_WORKERS: AtomicUsize = AtomicUsize::new(0);
struct WorkerSlot;
impl Drop for WorkerSlot {
    fn drop(&mut self) {
        EVENT_WORKERS.fetch_sub(1, Ordering::AcqRel);
    }
}

pub(crate) struct OpenView {
    principal: Principal,
    event: String,
    subscribed: bool,
    sequence: u64,
    dirty: bool,
    authorities: Vec<Authority>,
    overflow: bool,
}

impl OpenView {
    fn capture(&mut self, authorities: impl IntoIterator<Item = Authority>) {
        for authority in authorities {
            if !self.authorities.iter().any(|other| std::ptr::fn_addr_eq(*other, authority)) {
                if self.authorities.len() < 8 {
                    self.authorities.push(authority);
                } else {
                    self.overflow = true;
                }
            }
        }
    }
}

pub(crate) struct EventWork {
    principal: Principal,
    extension: Arc<Extension>,
    sequence: u64,
    rounds: u8,
    values: Value,
    source_state: Value,
    receiver: Receiver<std::result::Result<Response, String>>,
    continuation: Option<Value>,
}

impl Platform {
    pub fn invalidate(&mut self, authority: Option<Authority>) {
        for open in self.opened.values_mut().filter(|open| open.subscribed) {
            open.sequence = open.sequence.saturating_add(1);
            open.dirty = true;
            open.capture(authority);
        }
    }
}

fn launch(extension: Arc<Extension>, request: Value) -> Result<Receiver<std::result::Result<Response, String>>> {
    let mut count = EVENT_WORKERS.load(Ordering::Acquire);
    loop {
        if count >= 4 {
            return Err(failure(ErrorCode::Conflict, "Plugin event workers are busy"));
        }
        match EVENT_WORKERS.compare_exchange_weak(count, count.saturating_add(1), Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => break,
            Err(observed) => count = observed,
        }
    }
    let slot = WorkerSlot;
    let (sender, receiver) = mpsc::sync_channel(1);
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::thread::Builder::new()
            .name("photocraft-plugin-view".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| extension.handle(&request)))
                    .map_err(|_| "Plugin event failed with an internal error".to_string())
                    .and_then(|result| result.map_err(|error| error.to_string()));
                drop(slot);
                let _ = sender.send(result);
            })
            .map_err(|error| EngineError::Other(format!("Could not start plugin view event: {error}")))?;
    }
    #[cfg(target_arch = "wasm32")]
    {
        let result = extension.handle(&request).map_err(|error| error.to_string());
        drop(slot);
        let _ = sender.send(result);
    }
    Ok(receiver)
}

pub(crate) fn open(s: &mut Session, params: &Value) -> Result<Value> {
    let id = params.get("id").and_then(Value::as_str).ok_or_else(|| failure(ErrorCode::InvalidJson, "Missing plugin id"))?;
    let owner = principal(s, id)?;
    require(s, &owner, KnownCapability::UiViews)?;
    authorize(s, &owner, "extension.view.open", params)?;
    let extension = validate_principal(s, &owner)?;
    let inspector =
        extension.manifest.contributes.inspectors.first().ok_or_else(|| failure(ErrorCode::NotFound, "The plugin has no inspector contribution"))?;
    let subscribed = extension.manifest.contributes.subscriptions.iter().any(|subscription| subscription.handler == inspector.event);
    if subscribed {
        require(s, &owner, KnownCapability::Events)?;
    }
    s.extensions.platform.pending.remove(id);
    s.extensions.platform.errors.remove(id);
    s.extensions.platform.opened.insert(
        id.into(),
        OpenView { principal: owner, event: inspector.event.clone(), subscribed, sequence: 1, dirty: true, authorities: Vec::new(), overflow: false },
    );
    Ok(json!({"id":id,"queued":true}))
}

pub(crate) fn close(s: &mut Session, params: &Value) -> Result<Value> {
    let id = params.get("id").and_then(Value::as_str).ok_or_else(|| failure(ErrorCode::InvalidJson, "Missing plugin id"))?;
    s.extensions.platform.opened.remove(id);
    s.extensions.platform.pending.remove(id);
    s.extensions.platform.views.remove(id);
    s.extensions.platform.errors.remove(id);
    Ok(json!({"id":id,"closed":true}))
}

fn start(s: &mut Session, id: &str) -> Result<()> {
    start_with(s, id, launch)
}

fn start_with(
    s: &mut Session,
    id: &str,
    launcher: impl FnOnce(Arc<Extension>, Value) -> Result<Receiver<std::result::Result<Response, String>>>,
) -> Result<()> {
    let open = s.extensions.platform.opened.get_mut(id).ok_or_else(|| failure(ErrorCode::NotFound, "The inspector is closed"))?;
    if open.overflow {
        open.dirty = false;
        return Err(failure(ErrorCode::LimitExceeded, "Inspector event authority overflow; reopen the view to resynchronize"));
    }
    let mut owner = open.principal.clone();
    owner.authorities.extend(open.authorities.iter().copied());
    open.dirty = false;
    let event = open.event.clone();
    let sequence = open.sequence;
    let extension = validate_principal(s, &owner)?;
    let state = s.extensions.installed.get(id).map(|installed| installed.state.clone()).unwrap_or(Value::Null);
    let values = Value::Object(extension.manifest.values(&Value::Null).map_err(|error| EngineError::Other(error.to_string()))?);
    let mut request = event_request(s, &extension, &event, values.clone(), state.clone(), json!([]));
    request["notification"] = json!({"event":"document.changed","sequence":sequence,"resyncRequired":false});
    let receiver = launcher(Arc::clone(&extension), request)?;
    if let Some(open) = s.extensions.platform.opened.get_mut(id) {
        open.authorities.clear();
    }
    s.extensions
        .platform
        .pending
        .insert(id.into(), EventWork { principal: owner, extension, sequence, rounds: 1, values, source_state: state, receiver, continuation: None });
    Ok(())
}

fn apply(s: &mut Session, id: &str, work: EventWork, response: Response) -> Result<bool> {
    apply_with(s, id, work, response, launch)
}

fn apply_with(
    s: &mut Session,
    id: &str,
    mut work: EventWork,
    response: Response,
    launcher: impl FnOnce(Arc<Extension>, Value) -> Result<Receiver<std::result::Result<Response, String>>>,
) -> Result<bool> {
    #[cfg(not(target_arch = "wasm32"))]
    let _admission = s.extensions.folder.as_deref().map(crate::extension_package::AdmissionGuard::store).transpose()?;
    validate_principal(s, &work.principal)?;
    if !s.extensions.platform.opened.get(id).is_some_and(|open| open.sequence == work.sequence) {
        if let Some(open) = s.extensions.platform.opened.get_mut(id) {
            open.capture(work.principal.authorities.iter().copied());
            open.dirty = true;
        }
        return Ok(false);
    }
    if s.extensions.installed.get(id).is_none_or(|installed| installed.state != work.source_state) {
        if let Some(open) = s.extensions.platform.opened.get_mut(id) {
            open.capture(work.principal.authorities.iter().copied());
            open.dirty = true;
            open.sequence = open.sequence.saturating_add(1);
        }
        return Ok(false);
    }
    if !response.commands.is_empty() {
        return Err(failure(ErrorCode::PermissionDenied, "Inspector events cannot execute editing commands"));
    }
    if response.view.is_some() {
        require(s, &work.principal, KnownCapability::UiViews)?;
    }
    let (replies, _) = broker::apply_requests(s, &work.principal, &response.requests, true)?;
    if !response.requests.is_empty() {
        if work.rounds >= 4 {
            return Err(failure(ErrorCode::LimitExceeded, "The inspector exceeded four host-query rounds"));
        }
        let request = event_request(s, &work.extension, "host_response", work.values.clone(), response.state, Value::Array(replies));
        match launcher(Arc::clone(&work.extension), request.clone()) {
            Ok(receiver) => work.receiver = receiver,
            Err(EngineError::PluginApi(error)) if error.code == ErrorCode::Conflict => work.continuation = Some(request),
            Err(error) => return Err(error),
        }
        work.rounds = work.rounds.saturating_add(1);
        s.extensions.platform.pending.insert(id.into(), work);
        return Ok(false);
    }
    if let Some(view) = response.view {
        s.extensions.platform.views.insert(id.into(), view);
    }
    if let Some(installed) = s.extensions.installed.get_mut(id) {
        installed.state = response.state;
    }
    s.extensions.platform.errors.remove(id);
    Ok(true)
}

fn resume_with(work: &mut EventWork, launcher: impl FnOnce(Arc<Extension>, Value) -> Result<Receiver<std::result::Result<Response, String>>>) -> Result<bool> {
    let Some(request) = work.continuation.take() else { return Ok(true) };
    match launcher(Arc::clone(&work.extension), request.clone()) {
        Ok(receiver) => {
            work.receiver = receiver;
            Ok(true)
        }
        Err(EngineError::PluginApi(error)) if error.code == ErrorCode::Conflict => {
            work.continuation = Some(request);
            Ok(false)
        }
        Err(error) => Err(error),
    }
}

impl Session {
    pub fn plugin_view_is_open(&self, id: &str) -> bool {
        self.extensions.platform.opened.contains_key(id)
    }

    pub fn plugin_events_pending(&self) -> bool {
        !self.extensions.platform.pending.is_empty() || self.extensions.platform.opened.values().any(|open| open.dirty)
    }

    /// Poll bounded work only. Idle frames perform no document scans or guest execution.
    /// Desktop WASM handlers run off-thread; the browser retains bounded inline execution.
    pub fn pump_plugin_events(&mut self) -> usize {
        if self.extensions.platform.pumping || self.extensions.platform.dispatch_depth > 0 || self.plugins_disabled {
            return 0;
        }
        self.extensions.platform.pumping = true;
        let mut delivered = 0;
        let pending = rotated(self.extensions.platform.pending.keys().cloned().collect(), self.extensions.platform.poll_cursor.as_deref(), 4);
        for id in pending {
            self.extensions.platform.poll_cursor = Some(id.clone());
            let Some(mut work) = self.extensions.platform.pending.remove(&id) else { continue };
            match resume_with(&mut work, launch) {
                Ok(true) => {}
                Ok(false) => {
                    self.extensions.platform.pending.insert(id, work);
                    continue;
                }
                Err(error) => {
                    self.extensions.platform.errors.insert(id, error.to_string());
                    continue;
                }
            }
            match work.receiver.try_recv() {
                Err(TryRecvError::Empty) => {
                    self.extensions.platform.pending.insert(id, work);
                }
                Err(TryRecvError::Disconnected) => {
                    self.extensions.platform.errors.insert(id, "Plugin event stopped without a result".into());
                }
                Ok(Err(error)) => {
                    self.extensions.platform.errors.insert(id, error);
                }
                Ok(Ok(response)) => match apply(self, &id, work, response) {
                    Ok(true) => delivered += 1,
                    Ok(false) => {}
                    Err(error) => {
                        self.extensions.platform.errors.insert(id, error.to_string());
                    }
                },
            }
        }
        let available = 4usize.saturating_sub(self.extensions.platform.pending.len());
        let dirty = self
            .extensions
            .platform
            .opened
            .iter()
            .filter(|(id, open)| open.dirty && !self.extensions.platform.pending.contains_key(*id))
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        let dirty = rotated(dirty, self.extensions.platform.event_cursor.as_deref(), available.min(2));
        for id in dirty {
            self.extensions.platform.event_cursor = Some(id.clone());
            if let Err(error) = start(self, &id) {
                if matches!(&error,EngineError::PluginApi(error) if error.code==ErrorCode::Conflict) {
                    if let Some(open) = self.extensions.platform.opened.get_mut(&id) {
                        open.dirty = true;
                    }
                } else {
                    self.extensions.platform.errors.insert(id, error.to_string());
                }
            }
        }
        self.extensions.platform.pumping = false;
        delivered
    }
}

// Rotate a sorted registry so a continuously dirty early plugin cannot starve later ones.
pub(super) fn rotated(mut ids: Vec<String>, after: Option<&str>, limit: usize) -> Vec<String> {
    if let Some(after) = after {
        let offset = ids.partition_point(|id| id.as_str() <= after);
        ids.rotate_left(offset);
    }
    ids.truncate(limit);
    ids
}

#[cfg(test)]
#[path = "event_retry_tests.rs"]
mod retry_tests;
