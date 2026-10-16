//! Private per-generation persistent processes. Native code is trusted, not OS-contained.
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{
    Arc, Mutex, OnceLock, TryLockError, Weak,
    atomic::{AtomicBool, AtomicUsize, Ordering},
    mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError},
};
use std::time::{Duration, Instant};

use photocraft_plugin_api::{ApiError, HostWorkerFrame, MAX_WORKER_FRAME_BYTES, WorkerHostFrame, WorkerOperation, WorkerResources, WorkerV2Descriptor};
use serde_json::{Value, json};

use super::{WorkerLog, error};
use crate::{EngineError, Result, jobs::JobCtx};

pub(crate) type ResidentAdmission = Arc<dyn Fn(&WorkerResources, &JobCtx) -> Result<Box<dyn Send>> + Send + Sync>;
const HANDSHAKE: Duration = Duration::from_secs(10);
const CANCEL_GRACE: Duration = Duration::from_secs(2);
const POLL: Duration = Duration::from_millis(20);
const MAX_PROCESSES: usize = 16;
const MAX_PROGRESS_FRAMES: usize = 4096;
static PROCESSES: AtomicUsize = AtomicUsize::new(0);

#[cfg(test)]
pub(super) fn process_count() -> usize {
    PROCESSES.load(Ordering::Acquire)
}

struct ProcessSlot;
impl ProcessSlot {
    fn acquire() -> Result<Self> {
        let mut count = PROCESSES.load(Ordering::Acquire);
        loop {
            if count >= MAX_PROCESSES {
                return Err(error("Persistent plugin transport slots are occupied; close idle plugins or retained native descendants and retry"));
            }
            match PROCESSES.compare_exchange_weak(count, count + 1, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => return Ok(Self),
                Err(current) => count = current,
            }
        }
    }
}
impl Drop for ProcessSlot {
    fn drop(&mut self) {
        PROCESSES.fetch_sub(1, Ordering::AcqRel);
    }
}

struct ResourceGuards {
    _lease: Option<Box<dyn Send>>,
    _resident: Box<dyn Send>,
}

/// A mutex makes Send-only admission/OS lease guards safe to retain across the two I/O threads.
struct Resources {
    _guards: Mutex<ResourceGuards>,
    _directory: Mutex<tempfile::TempDir>,
    _slot: ProcessSlot,
}

struct Reap {
    child: Child,
    _resources: Arc<Resources>,
    deadline: Instant,
}

fn reaper() -> Result<&'static SyncSender<Reap>> {
    static REAPER: OnceLock<std::result::Result<SyncSender<Reap>, String>> = OnceLock::new();
    REAPER
        .get_or_init(|| {
            let (sender, receiver) = mpsc::sync_channel::<Reap>(MAX_PROCESSES);
            std::thread::Builder::new()
                .name("photocraft-plugin-reaper".into())
                .spawn(move || {
                    let mut children: Vec<Reap> = Vec::new();
                    loop {
                        match receiver.recv_timeout(POLL) {
                            Ok(child) => children.push(child),
                            Err(RecvTimeoutError::Timeout) => {}
                            Err(RecvTimeoutError::Disconnected) if children.is_empty() => return,
                            Err(RecvTimeoutError::Disconnected) => {}
                        }
                        children.retain_mut(|item| {
                            if matches!(item.child.try_wait(), Ok(Some(_))) {
                                return false;
                            }
                            if Instant::now() >= item.deadline {
                                let _ = item.child.kill();
                                item.deadline = Instant::now() + Duration::from_millis(250);
                            }
                            true
                        });
                    }
                })
                .map_err(|error| error.to_string())?;
            Ok(sender)
        })
        .as_ref()
        .map_err(error)
}

fn janitor() -> Result<&'static SyncSender<Weak<Supervisor>>> {
    static JANITOR: OnceLock<std::result::Result<SyncSender<Weak<Supervisor>>, String>> = OnceLock::new();
    JANITOR
        .get_or_init(|| {
            let (sender, receiver) = mpsc::sync_channel::<Weak<Supervisor>>(64);
            std::thread::Builder::new()
                .name("photocraft-plugin-idle".into())
                .spawn(move || {
                    let mut owners: Vec<Weak<Supervisor>> = Vec::new();
                    loop {
                        match receiver.recv_timeout(POLL) {
                            Ok(owner) => {
                                if !owners.iter().any(|other| Weak::ptr_eq(other, &owner)) {
                                    owners.push(owner);
                                }
                            }
                            Err(RecvTimeoutError::Timeout) => {}
                            Err(RecvTimeoutError::Disconnected) => return,
                        }
                        owners.retain(|owner| owner.upgrade().is_some_and(|owner| owner.maintain()));
                    }
                })
                .map_err(|error| error.to_string())?;
            Ok(sender)
        })
        .as_ref()
        .map_err(error)
}

struct State {
    worker: Option<Worker>,
    failures: u8,
    last_error: Option<String>,
    next_id: u64,
}

/// Construction is inert. No thread, executable, models or files are touched before a job.
pub(crate) struct Supervisor {
    worker: PathBuf,
    plugin_id: String,
    generation: String,
    descriptor: WorkerV2Descriptor,
    resident: Option<ResidentAdmission>,
    storage: Option<Value>,
    liveness: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
    stopped: AtomicBool,
    active: AtomicBool,
    state: Mutex<State>,
}

impl Supervisor {
    pub(crate) fn new(worker: PathBuf, plugin_id: String, generation: String, descriptor: WorkerV2Descriptor) -> Result<Self> {
        descriptor.validate()?;
        HostWorkerFrame::Hello {
            id: "0".into(),
            protocol_version: 2,
            plugin_id: plugin_id.clone(),
            generation: generation.clone(),
            operations: descriptor.operations.clone(),
            max_frame_bytes: MAX_WORKER_FRAME_BYTES as u32,
            managed_storage: None,
        }
        .validate()?;
        Ok(Self {
            worker,
            plugin_id,
            generation,
            descriptor,
            resident: None,
            storage: None,
            liveness: None,
            stopped: AtomicBool::new(false),
            active: AtomicBool::new(false),
            state: Mutex::new(State { worker: None, failures: 0, last_error: None, next_id: 0 }),
        })
    }

    pub(crate) fn with_resident_admission(mut self, admission: ResidentAdmission) -> Self {
        self.resident = Some(admission);
        self
    }

    pub(crate) fn with_liveness(mut self, liveness: Arc<dyn Fn() -> bool + Send + Sync>) -> Self {
        self.liveness = Some(liveness);
        self
    }

    pub(crate) fn with_managed_storage(mut self, storage: Value) -> Result<Self> {
        let hello = HostWorkerFrame::Hello {
            id: "0".into(),
            protocol_version: 2,
            plugin_id: self.plugin_id.clone(),
            generation: self.generation.clone(),
            operations: self.descriptor.operations.clone(),
            max_frame_bytes: MAX_WORKER_FRAME_BYTES as u32,
            managed_storage: Some(storage.clone()),
        };
        hello.validate()?;
        self.storage = Some(storage);
        Ok(self)
    }

    pub(crate) fn resources(&self) -> &WorkerResources {
        &self.descriptor.resources
    }

    /// A new activation has no live process or previous failure/quarantine state.
    pub(crate) fn fresh(&self) -> Self {
        Self {
            worker: self.worker.clone(),
            plugin_id: self.plugin_id.clone(),
            generation: self.generation.clone(),
            descriptor: self.descriptor.clone(),
            resident: self.resident.clone(),
            storage: self.storage.clone(),
            liveness: self.liveness.clone(),
            stopped: AtomicBool::new(false),
            active: AtomicBool::new(false),
            state: Mutex::new(State { worker: None, failures: 0, last_error: None, next_id: 0 }),
        }
    }

    /// Revocation never waits on a job/UI thread lock. Active execution and the idle janitor stop it.
    pub(crate) fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
    }

    pub(crate) fn retry(&self) -> Result<()> {
        if self.stopped.load(Ordering::Acquire) {
            return Err(error("This worker was revoked; re-enable the plugin to create a new activation"));
        }
        let mut state = match self.state.try_lock() {
            Ok(state) => state,
            Err(TryLockError::Poisoned(error)) => error.into_inner(),
            Err(TryLockError::WouldBlock) => return Err(error("Cancel the current plugin job before retrying its runtime")),
        };
        state.worker.take();
        state.failures = 0;
        state.last_error = None;
        Ok(())
    }

    pub(crate) fn status(&self) -> Value {
        let state = self.state.try_lock().ok();
        let phase = if self.stopped.load(Ordering::Acquire) {
            "revoked"
        } else if self.active.load(Ordering::Acquire) {
            "active"
        } else if state.as_ref().is_some_and(|state| state.failures >= 3) {
            "quarantined"
        } else if state.as_ref().is_some_and(|state| state.worker.is_some()) {
            "idle"
        } else {
            "inactive"
        };
        json!({"state":phase,"pluginId":self.plugin_id,"generation":self.generation,
            "consecutiveFailures":state.as_ref().map(|state|state.failures),"lastError":state.as_ref().and_then(|state|state.last_error.as_deref())})
    }

    fn maintain(&self) -> bool {
        if self.liveness.as_ref().is_some_and(|liveness| !liveness()) {
            self.stop();
        }
        match self.state.try_lock() {
            Ok(mut state) => {
                if self.stopped.load(Ordering::Acquire)
                    || state.worker.as_ref().is_some_and(|worker| worker.last_used.elapsed() >= Duration::from_millis(self.descriptor.idle_timeout_ms))
                {
                    state.worker.take();
                }
                state.worker.is_some()
            }
            Err(TryLockError::Poisoned(error)) => {
                let mut state = error.into_inner();
                state.worker.take();
                false
            }
            Err(TryLockError::WouldBlock) => true,
        }
    }

    fn lock(&self, ctx: &JobCtx) -> Result<std::sync::MutexGuard<'_, State>> {
        let start = Instant::now();
        loop {
            ctx.check()?;
            if self.stopped.load(Ordering::Acquire) {
                return Err(EngineError::Cancelled);
            }
            match self.state.try_lock() {
                Ok(state) => return Ok(state),
                Err(TryLockError::Poisoned(error)) => return Ok(error.into_inner()),
                Err(TryLockError::WouldBlock) => {}
            }
            if start.elapsed() >= Duration::from_millis(self.descriptor.request_timeout_ms) {
                return Err(error("Timed out waiting for this plugin's persistent worker"));
            }
            std::thread::sleep(POLL);
        }
    }

    pub(crate) fn run(self: &Arc<Self>, operation: WorkerOperation, request: Value, lease: Option<Box<dyn Send>>, ctx: &JobCtx) -> Result<()> {
        if self.liveness.as_ref().is_some_and(|liveness| !liveness()) {
            self.stop();
        }
        if !self.descriptor.operations.contains(&operation) {
            return Err(error("This worker did not declare the requested image/task operation"));
        }
        let mut state = self.lock(ctx)?;
        if state.failures >= 3 {
            return Err(error(format!(
                "Plugin {} generation {} is quarantined after repeated worker failures. Retry or re-enable the plugin. Last failure: {}",
                self.plugin_id,
                self.generation,
                state.last_error.as_deref().unwrap_or("unknown failure")
            )));
        }
        self.active.store(true, Ordering::Release);
        let result = self.run_locked(&mut state, operation, request, lease, ctx);
        self.active.store(false, Ordering::Release);
        match result {
            Ok(Outcome::Success) => {
                state.failures = 0;
                state.last_error = None;
                Ok(())
            }
            Ok(Outcome::Feature(failure)) => {
                state.failures = 0;
                state.last_error = None;
                Err(EngineError::PluginApi(failure))
            }
            Ok(Outcome::Cancelled(clean)) => {
                if !clean {
                    state.worker.take();
                }
                Err(EngineError::Cancelled)
            }
            Err(failure) => {
                if let Some(worker) = state.worker.as_mut() {
                    worker.abort();
                }
                state.worker.take();
                if !matches!(&failure, EngineError::Cancelled) {
                    state.failures = state.failures.saturating_add(1);
                    state.last_error = Some(failure.to_string());
                }
                Err(failure)
            }
        }
    }

    fn run_locked(
        self: &Arc<Self>,
        state: &mut State,
        operation: WorkerOperation,
        request: Value,
        lease: Option<Box<dyn Send>>,
        ctx: &JobCtx,
    ) -> Result<Outcome> {
        if state.worker.as_mut().is_some_and(|worker| worker.exited()) {
            state.worker.take();
        }
        if state.worker.is_none() {
            // Register before admission/handshake so a revoked activation is observed even
            // while its first process is still starting and the state lock is occupied.
            janitor()?.send(Arc::downgrade(self)).map_err(error)?;
            let admission = self.resident.as_ref().ok_or_else(|| error("Persistent worker resident admission is not configured"))?;
            let resident = admission(self.resources(), ctx)?;
            let mut worker = Worker::spawn(&self.worker, lease, resident)?;
            let hello = HostWorkerFrame::Hello {
                id: "0".into(),
                protocol_version: 2,
                plugin_id: self.plugin_id.clone(),
                generation: self.generation.clone(),
                operations: self.descriptor.operations.clone(),
                max_frame_bytes: MAX_WORKER_FRAME_BYTES as u32,
                managed_storage: self.storage.clone(),
            };
            let deadline = Instant::now() + HANDSHAKE;
            worker.send(&hello, ctx, &self.stopped, deadline)?;
            match worker.receive(ctx, &self.stopped, deadline)? {
                WorkerHostFrame::Ready { id, protocol_version, plugin_id, generation, operations }
                    if id == "0"
                        && protocol_version == 2
                        && plugin_id == self.plugin_id
                        && generation == self.generation
                        && self.descriptor.operations.iter().all(|operation| operations.contains(operation)) => {}
                _ => {
                    worker.abort();
                    return Err(error("Persistent worker handshake does not match its declared protocol/owner/generation/operations"));
                }
            }
            state.worker = Some(worker);
        }
        state.next_id = state.next_id.checked_add(1).ok_or_else(|| error("Persistent worker correlation IDs exhausted"))?;
        let id = state.next_id.to_string();
        let frame = HostWorkerFrame::Run { id: id.clone(), operation, request, managed_storage: self.storage.clone() };
        let worker = state.worker.as_mut().ok_or_else(|| error("Persistent worker did not activate"))?;
        let deadline = Instant::now() + Duration::from_millis(self.descriptor.request_timeout_ms);
        worker.send(&frame, ctx, &self.stopped, deadline)?;
        let result = worker.exchange(&id, ctx, &self.stopped, deadline);
        worker.last_used = Instant::now();
        result
    }

    pub(crate) fn reset(self: &Arc<Self>, ctx: &JobCtx) -> Result<()> {
        let mut state = self.lock(ctx)?;
        state.next_id = state.next_id.checked_add(1).ok_or_else(|| error("Persistent worker correlation IDs exhausted"))?;
        let id = state.next_id.to_string();
        let Some(worker) = state.worker.as_mut() else { return Ok(()) };
        let deadline = Instant::now() + HANDSHAKE;
        let result =
            worker.send(&HostWorkerFrame::Reset { id: id.clone() }, ctx, &self.stopped, deadline).and_then(|()| worker.receive(ctx, &self.stopped, deadline));
        match result {
            Ok(WorkerHostFrame::ResetComplete { id: reply }) if reply == id => {
                worker.last_used = Instant::now();
                Ok(())
            }
            other => {
                worker.abort();
                state.worker.take();
                Err(error(format!("Persistent worker reset failed: {other:?}")))
            }
        }
    }
}

enum Outcome {
    Success,
    Feature(ApiError),
    Cancelled(bool),
}
struct Packet {
    bytes: Vec<u8>,
    acknowledgement: SyncSender<std::result::Result<(), String>>,
}

struct Worker {
    child: Option<Child>,
    outgoing: Option<SyncSender<Packet>>,
    incoming: Receiver<WorkerHostFrame>,
    fault: Arc<Mutex<Option<String>>>,
    log: WorkerLog,
    resources: Arc<Resources>,
    reap: SyncSender<Reap>,
    last_used: Instant,
}

impl Worker {
    fn spawn(path: &Path, lease: Option<Box<dyn Send>>, resident: Box<dyn Send>) -> Result<Self> {
        let reap = reaper()?.clone();
        janitor()?;
        let directory = tempfile::Builder::new().prefix("photocraft-plugin-serve-").tempdir().map_err(error)?;
        let log_path = directory.path().join("worker.log");
        let writer = std::fs::File::create(&log_path).map_err(error)?;
        let log = WorkerLog::open(&log_path)?;
        let resources = Arc::new(Resources {
            _guards: Mutex::new(ResourceGuards { _lease: lease, _resident: resident }),
            _directory: Mutex::new(directory),
            _slot: ProcessSlot::acquire()?,
        });
        let mut command = Command::new(path);
        command.arg("--serve").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(writer);
        if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
            command.current_dir(parent);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let mut child = command.spawn().map_err(|failure| error(format!("Could not start persistent plugin worker: {failure}")))?;
        let Some(stdin) = child.stdin.take() else {
            let _ = child.kill();
            let _ = reap.send(Reap { child, _resources: resources, deadline: Instant::now() });
            return Err(error("Persistent worker has no input pipe"));
        };
        let Some(stdout) = child.stdout.take() else {
            let _ = child.kill();
            let _ = reap.send(Reap { child, _resources: resources, deadline: Instant::now() });
            return Err(error("Persistent worker has no output pipe"));
        };
        let (outgoing, writer_rx) = mpsc::sync_channel::<Packet>(1);
        let (reader_tx, incoming) = mpsc::sync_channel(16);
        let fault = Arc::new(Mutex::new(None));
        let writer_resources = Arc::clone(&resources);
        let writer_fault = Arc::clone(&fault);
        if let Err(failure) = std::thread::Builder::new().name("photocraft-plugin-send".into()).spawn(move || {
            let _resources = writer_resources;
            let mut stdin = stdin;
            while let Ok(packet) = writer_rx.recv() {
                let result = stdin.write_all(&packet.bytes).and_then(|()| stdin.flush()).map_err(|failure| failure.to_string());
                if let Err(failure) = &result {
                    *writer_fault.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(failure.clone());
                }
                let failed = result.is_err();
                let _ = packet.acknowledgement.try_send(result);
                if failed {
                    break;
                }
            }
        }) {
            let _ = child.kill();
            let _ = reap.send(Reap { child, _resources: resources, deadline: Instant::now() });
            return Err(error(failure));
        }
        let reader_resources = Arc::clone(&resources);
        let reader_fault = Arc::clone(&fault);
        if let Err(failure) = std::thread::Builder::new().name("photocraft-plugin-receive".into()).spawn(move || {
            let _resources = reader_resources;
            let mut stdout = stdout;
            loop {
                let result = read_frame(&mut stdout);
                match result {
                    Ok(frame) => {
                        if reader_tx.try_send(frame).is_err() {
                            *reader_fault.lock().unwrap_or_else(std::sync::PoisonError::into_inner) =
                                Some("Persistent worker exceeded its bounded reply queue".into());
                            break;
                        }
                    }
                    Err(failure) => {
                        *reader_fault.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(failure);
                        break;
                    }
                }
            }
        }) {
            let _ = child.kill();
            drop(outgoing);
            let _ = reap.send(Reap { child, _resources: resources, deadline: Instant::now() });
            return Err(error(failure));
        }
        Ok(Self { child: Some(child), outgoing: Some(outgoing), incoming, fault, log, resources, reap, last_used: Instant::now() })
    }

    fn exited(&mut self) -> bool {
        self.child.as_mut().is_none_or(|child| matches!(child.try_wait(), Ok(Some(_))))
    }
    fn abort(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
        }
    }

    fn check(&mut self, ctx: &JobCtx, stopped: &AtomicBool, deadline: Instant) -> Result<()> {
        ctx.check()?;
        if stopped.load(Ordering::Acquire) {
            return Err(EngineError::Cancelled);
        }
        if Instant::now() >= deadline {
            return Err(error("Persistent worker exceeded its response deadline"));
        }
        self.log.poll(ctx)?;
        Ok(())
    }

    fn send(&mut self, frame: &HostWorkerFrame, ctx: &JobCtx, stopped: &AtomicBool, deadline: Instant) -> Result<()> {
        frame.validate()?;
        let payload = serde_json::to_vec(frame).map_err(error)?;
        let mut bytes = Vec::with_capacity(payload.len().saturating_add(4));
        bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&payload);
        let (acknowledgement, receiver) = mpsc::sync_channel(1);
        let mut packet = Packet { bytes, acknowledgement };
        loop {
            self.check(ctx, stopped, deadline)?;
            match self.outgoing.as_ref().ok_or_else(|| error("Persistent worker input is closed"))?.try_send(packet) {
                Ok(()) => break,
                Err(TrySendError::Full(returned)) => {
                    packet = returned;
                    std::thread::sleep(POLL);
                }
                Err(TrySendError::Disconnected(_)) => return Err(error("Persistent worker input closed unexpectedly")),
            }
        }
        loop {
            self.check(ctx, stopped, deadline)?;
            match receiver.recv_timeout(POLL) {
                Ok(result) => return result.map_err(error),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return Err(error("Persistent worker input writer stopped")),
            }
        }
    }

    fn receive(&mut self, ctx: &JobCtx, stopped: &AtomicBool, deadline: Instant) -> Result<WorkerHostFrame> {
        loop {
            self.check(ctx, stopped, deadline)?;
            if let Ok(frame) = self.incoming.try_recv() {
                return Ok(frame);
            }
            if let Some(failure) = self.fault.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone() {
                return Err(error(format!("Persistent worker transport failed: {failure}; {}", self.log.diagnostics.message())));
            }
            if self.exited() {
                return Err(error(format!("Persistent worker exited unexpectedly: {}", self.log.diagnostics.message())));
            }
            std::thread::sleep(POLL);
        }
    }

    fn exchange(&mut self, id: &str, ctx: &JobCtx, stopped: &AtomicBool, deadline: Instant) -> Result<Outcome> {
        let mut cancelling = None;
        let mut timed_out = false;
        let mut progress = 0usize;
        let drain_ctx = JobCtx::new();
        let never_stop = AtomicBool::new(false);
        loop {
            if cancelling.is_none() && (ctx.cancelled() || stopped.load(Ordering::Acquire) || Instant::now() >= deadline) {
                timed_out = !ctx.cancelled() && !stopped.load(Ordering::Acquire);
                let cancel_deadline = Instant::now() + CANCEL_GRACE;
                if self.send(&HostWorkerFrame::Cancel { id: id.into() }, &drain_ctx, &never_stop, cancel_deadline).is_err() {
                    self.abort();
                    return if timed_out { Err(error("Persistent worker request timed out")) } else { Ok(Outcome::Cancelled(false)) };
                }
                cancelling = Some(cancel_deadline);
            }
            let frame = self.receive(
                if cancelling.is_some() { &drain_ctx } else { ctx },
                if cancelling.is_some() { &never_stop } else { stopped },
                cancelling.unwrap_or(deadline),
            );
            let frame = match frame {
                Ok(frame) => frame,
                Err(EngineError::Cancelled) if cancelling.is_none() => continue,
                Err(_) if cancelling.is_some() => {
                    self.abort();
                    return if timed_out { Err(error("Persistent worker request timed out")) } else { Ok(Outcome::Cancelled(false)) };
                }
                Err(_) if ctx.cancelled() || stopped.load(Ordering::Acquire) || Instant::now() >= deadline => continue,
                Err(failure) => return Err(failure),
            };
            if frame.id() != id {
                return Err(error("Persistent worker returned an unexpected correlation ID"));
            }
            match frame {
                WorkerHostFrame::Progress { fraction, message, .. } => {
                    progress = progress.saturating_add(1);
                    if progress > MAX_PROGRESS_FRAMES {
                        return Err(error("Persistent worker exceeded its progress frame budget"));
                    }
                    if cancelling.is_none() {
                        ctx.progress(0.1 + 0.85 * fraction as f32, &message);
                    }
                }
                WorkerHostFrame::Result { .. } if cancelling.is_none() => return Ok(Outcome::Success),
                WorkerHostFrame::Error { error, .. } if cancelling.is_none() => return Ok(Outcome::Feature(error)),
                WorkerHostFrame::Cancelled { .. } if cancelling.is_some() => {
                    return if timed_out { Err(error("Persistent worker request timed out")) } else { Ok(Outcome::Cancelled(true)) };
                }
                WorkerHostFrame::Result { .. } | WorkerHostFrame::Error { .. } if cancelling.is_some() => {
                    return if timed_out { Err(error("Persistent worker request timed out")) } else { Ok(Outcome::Cancelled(false)) };
                }
                _ => return Err(error("Persistent worker returned an out-of-order message")),
            }
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        if let Some(outgoing) = self.outgoing.take() {
            let (acknowledgement, _) = mpsc::sync_channel(1);
            let payload = serde_json::to_vec(&HostWorkerFrame::Shutdown).unwrap_or_default();
            let mut bytes = (payload.len() as u32).to_le_bytes().to_vec();
            bytes.extend(payload);
            let _ = outgoing.try_send(Packet { bytes, acknowledgement });
        }
        if let Some(child) = self.child.take() {
            let reap = Reap { child, _resources: Arc::clone(&self.resources), deadline: Instant::now() + Duration::from_millis(250) };
            if let Err(failure) = self.reap.try_send(reap) {
                // Admission bounds this rare last-resort leak. Retaining the OS lease is safer
                // than collecting a generation whose unreaped process may still execute it.
                let item = match failure {
                    TrySendError::Full(item) | TrySendError::Disconnected(item) => item,
                };
                std::mem::forget(item);
            }
        }
    }
}

pub(super) fn read_frame(reader: &mut impl Read) -> std::result::Result<WorkerHostFrame, String> {
    let mut header = [0u8; 4];
    reader.read_exact(&mut header).map_err(|failure| format!("worker frame header/EOF: {failure}"))?;
    let length = u32::from_le_bytes(header) as usize;
    if length == 0 || length > MAX_WORKER_FRAME_BYTES {
        return Err("worker frame length is outside 1..=256 KiB".into());
    }
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(length).map_err(|failure| failure.to_string())?;
    bytes.resize(length, 0);
    reader.read_exact(&mut bytes).map_err(|failure| format!("worker frame body/EOF: {failure}"))?;
    WorkerHostFrame::parse(&bytes).map_err(|failure| failure.to_string())
}
