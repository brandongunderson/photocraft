//! Process-wide admission accounting for independent plugin work.
//!
//! These are cooperative reservations of declared/estimated resources. Trusted native
//! processes retain OS privileges; this scheduler is not an OS memory or GPU sandbox.
use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{EngineError, Result, jobs::JobCtx};

const MAX_ADMISSION_WAIT_MS: u64 = 300_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Resources {
    pub cpu_units: u32,
    pub memory_bytes: u64,
    pub gpu_bytes: u64,
    pub temp_bytes: u64,
    pub cache_bytes: u64,
}

impl Resources {
    fn fits(self, limit: Self) -> bool {
        self.cpu_units <= limit.cpu_units
            && self.memory_bytes <= limit.memory_bytes
            && self.gpu_bytes <= limit.gpu_bytes
            && self.temp_bytes <= limit.temp_bytes
            && self.cache_bytes <= limit.cache_bytes
    }

    pub(crate) fn plus(self, other: Self) -> Self {
        Self {
            cpu_units: self.cpu_units.saturating_add(other.cpu_units),
            memory_bytes: self.memory_bytes.saturating_add(other.memory_bytes),
            gpu_bytes: self.gpu_bytes.saturating_add(other.gpu_bytes),
            temp_bytes: self.temp_bytes.saturating_add(other.temp_bytes),
            cache_bytes: self.cache_bytes.saturating_add(other.cache_bytes),
        }
    }

    fn minus(self, other: Self) -> Self {
        Self {
            cpu_units: self.cpu_units.saturating_sub(other.cpu_units),
            memory_bytes: self.memory_bytes.saturating_sub(other.memory_bytes),
            gpu_bytes: self.gpu_bytes.saturating_sub(other.gpu_bytes),
            temp_bytes: self.temp_bytes.saturating_sub(other.temp_bytes),
            cache_bytes: self.cache_bytes.saturating_sub(other.cache_bytes),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Limits {
    pub running_jobs: usize,
    pub resident_workers: usize,
    pub queued_jobs: usize,
    pub queued_per_plugin: usize,
    pub resources: Resources,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            running_jobs: 4,
            resident_workers: 8,
            queued_jobs: 64,
            queued_per_plugin: 8,
            resources: Resources {
                cpu_units: 4,
                memory_bytes: if cfg!(target_pointer_width = "64") { 8 << 30 } else { 1 << 30 },
                gpu_bytes: 4 << 30,
                temp_bytes: 16 << 30,
                cache_bytes: 8 << 30,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Priority {
    Interactive,
    Background,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Job(Priority),
    Resident,
}

struct Request {
    id: u64,
    plugin: String,
    kind: Kind,
    resources: Resources,
    bypasses: u8,
    resident: Resources,
}

struct Active {
    plugin: String,
    kind: Kind,
    resources: Resources,
    cancel: JobCtx,
    resident: Resources,
    credit: Resources,
    borrowed: Option<u64>,
}

struct State {
    limits: Limits,
    next: u64,
    queued: VecDeque<Request>,
    active: BTreeMap<u64, Active>,
    used: Resources,
    rotation: VecDeque<String>,
    interactive_burst: u8,
}

impl State {
    fn prune_owners(&mut self) {
        let queued = &self.queued;
        let active = &self.active;
        self.rotation.retain(|owner| queued.iter().any(|request| &request.plugin == owner) || active.values().any(|request| &request.plugin == owner));
    }
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}

/// Cloning shares one process budget across all editor sessions and plugin identities.
#[derive(Clone)]
pub struct PluginScheduler {
    shared: Arc<Shared>,
}

impl Default for PluginScheduler {
    fn default() -> Self {
        static GLOBAL: OnceLock<PluginScheduler> = OnceLock::new();
        GLOBAL.get_or_init(|| Self::new(Limits::default())).clone()
    }
}

fn failure(message: &str) -> EngineError {
    EngineError::Other(format!("Plugin resource admission: {message}"))
}

impl PluginScheduler {
    fn new(limits: Limits) -> Self {
        Self {
            shared: Arc::new(Shared {
                state: Mutex::new(State {
                    limits,
                    next: 0,
                    queued: VecDeque::new(),
                    active: BTreeMap::new(),
                    used: Resources::default(),
                    rotation: VecDeque::new(),
                    interactive_burst: 0,
                }),
                changed: Condvar::new(),
            }),
        }
    }

    pub fn status(&self) -> Value {
        let state = self.shared.state.lock().unwrap_or_else(PoisonError::into_inner);
        json!({"limits":state.limits,"reserved":state.used,"running":state.active.values().filter(|active|matches!(active.kind,Kind::Job(_))).count(),
            "residentWorkers":state.active.values().filter(|active|active.kind==Kind::Resident).count(),"queued":state.queued.len(),
            "queuedJobs":state.queued.iter().filter(|request|matches!(request.kind,Kind::Job(_))).count(),
            "queuedResidentStarts":state.queued.iter().filter(|request|request.kind==Kind::Resident).count(),"maxAdmissionWaitMs":MAX_ADMISSION_WAIT_MS,
            "accounting":"cooperative declared estimates; trusted native code is not OS resource-contained"})
    }

    pub fn validate(&self, resources: Resources) -> Result<()> {
        if !resources.fits(self.shared.state.lock().unwrap_or_else(PoisonError::into_inner).limits.resources) {
            return Err(failure("this operation exceeds the aggregate CPU, memory, GPU, temporary-disk or cache budget"));
        }
        Ok(())
    }

    pub(crate) fn enqueue(&self, plugin: &str, priority: Priority, resources: Resources) -> Result<Ticket> {
        if resources.cpu_units == 0 {
            return Err(failure("a running job needs at least one CPU admission unit"));
        }
        self.queue(plugin, Kind::Job(priority), resources, Resources::default())
    }

    pub(crate) fn enqueue_worker(&self, plugin: &str, priority: Priority, resources: Resources, resident: Resources) -> Result<Ticket> {
        if resources.cpu_units == 0 || resident.cpu_units != 0 || resident.temp_bytes != 0 {
            return Err(failure("invalid persistent-worker admission declaration"));
        }
        self.queue(plugin, Kind::Job(priority), resources.plus(resident), resident)
    }

    fn queue(&self, plugin: &str, kind: Kind, resources: Resources, resident: Resources) -> Result<Ticket> {
        let mut state = self.shared.state.lock().unwrap_or_else(PoisonError::into_inner);
        if plugin.is_empty() || plugin.len() > 256 || !plugin.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"._-:".contains(&byte)) {
            return Err(failure("invalid host-owned plugin scheduling identity"));
        }
        if !resources.fits(state.limits.resources) {
            return Err(failure("declared resources exceed the aggregate budget; reduce this operation's requirements"));
        }
        let full = if kind == Kind::Resident {
            state.queued.iter().filter(|request| request.kind == Kind::Resident).count() >= state.limits.resident_workers
        } else {
            state.queued.iter().filter(|request| matches!(request.kind, Kind::Job(_))).count() >= state.limits.queued_jobs
                || state.queued.iter().filter(|request| request.plugin == plugin && matches!(request.kind, Kind::Job(_))).count()
                    >= state.limits.queued_per_plugin
        };
        if full {
            return Err(failure("the bounded plugin queue is full; finish or cancel queued work before retrying"));
        }
        if kind == Kind::Resident {
            let owner_jobs = state
                .active
                .values()
                .filter(|active| active.plugin == plugin && matches!(active.kind, Kind::Job(_)))
                .fold(Resources::default(), |total, active| total.plus(active.resources).plus(resources.minus(active.credit)));
            if !owner_jobs.fits(state.limits.resources) {
                return Err(failure("this worker and its active job cannot fit together in the aggregate budget"));
            }
        }
        state.next = state.next.checked_add(1).ok_or_else(|| failure("admission identifiers exhausted; restart PhotoCraft"))?;
        let id = state.next;
        if !state.rotation.iter().any(|owner| owner == plugin) {
            state.rotation.push_back(plugin.into());
        }
        state.queued.push_back(Request { id, plugin: plugin.into(), kind, resources, bypasses: 0, resident });
        drop(state);
        self.shared.changed.notify_all();
        Ok(Ticket { scheduler: self.clone(), id: Some(id) })
    }

    /// Called only for a newly launched persistent process. The guard remains with that
    /// process, including its idle lifetime; reuse never reserves the same model twice.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn resident(&self, plugin: &str, resources: &photocraft_plugin_api::WorkerResources, ctx: &JobCtx) -> Result<ResidentPermit> {
        let resources =
            Resources { cpu_units: 0, memory_bytes: resources.memory_bytes, gpu_bytes: resources.gpu_bytes, temp_bytes: 0, cache_bytes: resources.cache_bytes };
        self.queue(plugin, Kind::Resident, resources, Resources::default())?.wait(ctx).map(|permit| ResidentPermit { _permit: permit })
    }

    /// Revocation cancels waiting and active work. Reservations remain held until workers
    /// actually stop; reporting cancellation does not pretend native memory was released.
    pub(crate) fn cancel_plugin(&self, plugin: &str) {
        let mut state = self.shared.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.queued.retain(|request| request.plugin != plugin);
        for active in state.active.values().filter(|active| active.plugin == plugin) {
            active.cancel.cancel();
        }
        state.prune_owners();
        drop(state);
        self.shared.changed.notify_all();
    }
}

fn eligible(state: &State, request: &Request) -> bool {
    if !state.used.plus(charged(state, request)).fits(state.limits.resources) {
        return false;
    }
    match request.kind {
        Kind::Resident => state.active.values().filter(|active| active.kind == Kind::Resident).count() < state.limits.resident_workers,
        Kind::Job(_) => {
            state.active.values().filter(|active| matches!(active.kind, Kind::Job(_))).count() < state.limits.running_jobs
                && !state.active.values().any(|active| active.plugin == request.plugin && matches!(active.kind, Kind::Job(_)))
        }
    }
}

fn existing_resident(state: &State, request: &Request) -> Option<u64> {
    if request.resident == Resources::default() {
        return None;
    }
    state
        .active
        .iter()
        .find(|(_, active)| active.kind == Kind::Resident && active.plugin == request.plugin && active.resources == request.resident)
        .map(|(id, _)| *id)
}

fn charged(state: &State, request: &Request) -> Resources {
    match request.kind {
        Kind::Job(_) if existing_resident(state, request).is_some() => request.resources.minus(request.resident),
        Kind::Resident => {
            let credit = state
                .active
                .values()
                .find(|active| active.plugin == request.plugin && matches!(active.kind, Kind::Job(_)))
                .map_or(Resources::default(), |active| active.credit);
            request.resources.minus(credit)
        }
        _ => request.resources,
    }
}

/// Round robin among runnable owners. After four interactive admissions a waiting
/// background operation gets the next available turn. After eight bypasses the oldest
/// blocked request drains competing work, so a large reservation cannot starve forever.
fn selected(state: &State) -> Option<u64> {
    let protected = state.queued.iter().find(|request| request.bypasses >= 8);
    if let Some(request) = protected {
        if eligible(state, request) {
            return Some(request.id);
        }
        // An active job may need to start its resident worker before it can finish.
        // Always let such dependencies proceed while ordinary job admissions drain.
        return state.queued.iter().find(|request| request.kind == Kind::Resident && eligible(state, request)).map(|request| request.id);
    }
    let background =
        state.interactive_burst >= 4 && state.queued.iter().any(|request| request.kind == Kind::Job(Priority::Background) && eligible(state, request));
    let priority = if background { Kind::Job(Priority::Background) } else { Kind::Job(Priority::Interactive) };
    let preferred = |request: &&Request| eligible(state, request) && (request.kind == priority || (!background && request.kind == Kind::Resident));
    let candidates = state.queued.iter().filter(preferred).collect::<Vec<_>>();
    let candidates = if candidates.is_empty() { state.queued.iter().filter(|request| eligible(state, request)).collect::<Vec<_>>() } else { candidates };
    state.rotation.iter().find_map(|owner| candidates.iter().find(|request| &request.plugin == owner).map(|request| request.id))
}

#[must_use]
pub(crate) struct Ticket {
    scheduler: PluginScheduler,
    id: Option<u64>,
}

impl Ticket {
    pub(crate) fn wait(mut self, ctx: &JobCtx) -> Result<Permit> {
        let id = self.id.ok_or_else(|| failure("admission ticket was already consumed"))?;
        #[cfg(not(target_arch = "wasm32"))]
        let started = std::time::Instant::now();
        let mut state = self.scheduler.shared.state.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            ctx.check()?;
            #[cfg(not(target_arch = "wasm32"))]
            if started.elapsed() >= std::time::Duration::from_millis(MAX_ADMISSION_WAIT_MS) {
                return Err(failure("timed out waiting for plugin resources; finish or cancel existing work and retry"));
            }
            if !state.queued.iter().any(|request| request.id == id) {
                return Err(EngineError::Cancelled);
            }
            if selected(&state) == Some(id) {
                let index = state.queued.iter().position(|request| request.id == id).ok_or_else(|| failure("queued admission was revoked"))?;
                let request = state.queued.remove(index).ok_or_else(|| failure("queued admission was revoked"))?;
                let charge = charged(&state, &request);
                state.used = state.used.plus(charge);
                let borrowed = existing_resident(&state, &request);
                let mut credit = if borrowed.is_none() { request.resident } else { Resources::default() };
                if request.kind == Kind::Resident {
                    if let Some(active) = state.active.values_mut().find(|active| active.plugin == request.plugin && matches!(active.kind, Kind::Job(_))) {
                        let transfer = request.resources.minus(charge);
                        active.resources = active.resources.minus(transfer);
                        active.credit = active.credit.minus(transfer);
                        if active.resident == request.resources {
                            active.borrowed = Some(id);
                        }
                    }
                    credit = Resources::default();
                }
                state.rotation.retain(|owner| owner != &request.plugin);
                state.rotation.push_back(request.plugin.clone());
                if request.kind == Kind::Job(Priority::Background) {
                    state.interactive_burst = 0;
                } else if request.kind == Kind::Job(Priority::Interactive) {
                    state.interactive_burst = state.interactive_burst.saturating_add(1);
                }
                for pending in &mut state.queued {
                    pending.bypasses = pending.bypasses.saturating_add(1);
                }
                let resources = if matches!(request.kind, Kind::Job(_)) { charge } else { request.resources };
                state.active.insert(
                    id,
                    Active { plugin: request.plugin, kind: request.kind, resources, cancel: ctx.clone(), resident: request.resident, credit, borrowed },
                );
                self.id = None;
                drop(state);
                self.scheduler.shared.changed.notify_all();
                return Ok(Permit { scheduler: self.scheduler.clone(), id });
            }
            ctx.progress(0.0, "Waiting for plugin resource admission");
            #[cfg(not(target_arch = "wasm32"))]
            {
                state = self.scheduler.shared.changed.wait_timeout(state, std::time::Duration::from_millis(20)).unwrap_or_else(PoisonError::into_inner).0;
            }
            #[cfg(target_arch = "wasm32")]
            {
                return Err(failure("plugin resources are busy; finish the current operation before retrying"));
            }
        }
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        if let Some(id) = self.id {
            let mut state = self.scheduler.shared.state.lock().unwrap_or_else(PoisonError::into_inner);
            state.queued.retain(|request| request.id != id);
            state.prune_owners();
            drop(state);
            self.scheduler.shared.changed.notify_all();
        }
    }
}

#[must_use]
pub(crate) struct Permit {
    scheduler: PluginScheduler,
    id: u64,
}

impl Drop for Permit {
    fn drop(&mut self) {
        let mut state = self.scheduler.shared.state.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(active) = state.active.remove(&self.id) {
            if active.kind == Kind::Resident {
                // If an idle runtime expires after its next job was admitted, transfer the
                // already-counted model reservation back to that job. Its replacement can
                // launch without a hold-and-wait gap or a double reservation.
                if let Some(job) = state.active.values_mut().find(|job| job.borrowed == Some(self.id)) {
                    job.resources = job.resources.plus(active.resources);
                    job.credit = job.credit.plus(active.resources);
                    job.borrowed = None;
                } else {
                    state.used = state.used.minus(active.resources);
                }
            } else {
                state.used = state.used.minus(active.resources);
            }
        }
        state.prune_owners();
        drop(state);
        self.scheduler.shared.changed.notify_all();
    }
}

/// A process-owned reservation, intentionally distinct from a job/CPU admission.
#[must_use]
#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct ResidentPermit {
    _permit: Permit,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scheduler() -> PluginScheduler {
        PluginScheduler::new(Limits {
            running_jobs: 1,
            resources: Resources { cpu_units: 1, memory_bytes: 100, gpu_bytes: 100, temp_bytes: 100, cache_bytes: 100 },
            ..Limits::default()
        })
    }

    fn resource(memory: u64) -> Resources {
        Resources { cpu_units: 1, memory_bytes: memory, ..Resources::default() }
    }

    #[test]
    fn all_resource_dimensions_are_bounded_and_raii_releases_after_real_completion() {
        let scheduler = scheduler();
        for oversized in [
            Resources { cpu_units: 2, ..resource(1) },
            resource(101),
            Resources { gpu_bytes: 101, ..resource(1) },
            Resources { temp_bytes: 101, ..resource(1) },
            Resources { cache_bytes: 101, ..resource(1) },
        ] {
            assert!(scheduler.enqueue("org.example.one", Priority::Interactive, oversized).is_err());
        }
        let context = JobCtx::new();
        let permit = scheduler.enqueue("org.example.one", Priority::Interactive, resource(80)).unwrap().wait(&context).unwrap();
        scheduler.cancel_plugin("org.example.one");
        assert!(context.cancelled());
        assert_eq!(scheduler.status()["reserved"]["memoryBytes"], 80);
        drop(permit);
        assert_eq!(scheduler.status()["reserved"]["memoryBytes"], 0);
    }

    #[test]
    fn queue_is_finite_and_cancelled_waiters_do_not_block_other_plugins() {
        let scheduler = scheduler();
        let mut tickets = Vec::new();
        for _ in 0..8 {
            tickets.push(scheduler.enqueue("org.example.one", Priority::Background, resource(1)).unwrap());
        }
        assert!(scheduler.enqueue("org.example.one", Priority::Interactive, resource(1)).is_err());
        scheduler.cancel_plugin("org.example.one");
        assert!(matches!(tickets.pop().unwrap().wait(&JobCtx::new()), Err(EngineError::Cancelled)));
        let permit = scheduler.enqueue("org.example.two", Priority::Interactive, resource(1)).unwrap().wait(&JobCtx::new()).unwrap();
        drop(permit);
        assert_eq!(scheduler.status()["queued"], 0);
    }

    #[test]
    fn interactive_precedes_background_but_bounded_burst_prevents_background_starvation() {
        let scheduler = scheduler();
        let background = scheduler.enqueue("org.example.background", Priority::Background, resource(1)).unwrap();
        for number in 0..4 {
            let ticket = scheduler.enqueue(&format!("org.example.interactive{number}"), Priority::Interactive, resource(1)).unwrap();
            let permit = ticket.wait(&JobCtx::new()).unwrap();
            drop(permit);
        }
        let next = scheduler.enqueue("org.example.later", Priority::Interactive, resource(1)).unwrap();
        let state = scheduler.shared.state.lock().unwrap();
        assert_eq!(selected(&state), background.id);
        drop(state);
        drop(background.wait(&JobCtx::new()).unwrap());
        drop(next.wait(&JobCtx::new()).unwrap());
    }

    #[test]
    fn another_owner_gets_a_turn_before_the_same_owner_and_pending_cancel_is_prompt() {
        let scheduler = scheduler();
        let first = scheduler.enqueue("org.example.one", Priority::Interactive, resource(1)).unwrap();
        let again = scheduler.enqueue("org.example.one", Priority::Interactive, resource(1)).unwrap();
        let other = scheduler.enqueue("org.example.two", Priority::Interactive, resource(1)).unwrap();
        drop(first.wait(&JobCtx::new()).unwrap());
        assert_eq!(selected(&scheduler.shared.state.lock().unwrap()), other.id);
        let running = other.wait(&JobCtx::new()).unwrap();
        let context = JobCtx::new();
        context.cancel();
        assert!(matches!(again.wait(&context), Err(EngineError::Cancelled)));
        drop(running);
        assert_eq!(scheduler.status()["queued"], 0);
    }

    #[test]
    fn persistent_models_transfer_without_double_reservation_and_remain_accounted_while_idle() {
        let scheduler = scheduler();
        let declaration = photocraft_plugin_api::WorkerResources { cpu_units: 1, memory_bytes: 80, gpu_bytes: 40, temp_bytes: 0, cache_bytes: 20 };
        let resident = Resources { memory_bytes: 80, gpu_bytes: 40, cache_bytes: 20, ..Default::default() };
        let job = scheduler.enqueue_worker("org.example.model", Priority::Interactive, resource(20), resident).unwrap().wait(&JobCtx::new()).unwrap();
        assert_eq!(scheduler.status()["reserved"]["memoryBytes"], 100);
        let model = scheduler.resident("org.example.model", &declaration, &JobCtx::new()).unwrap();
        assert_eq!(scheduler.status()["reserved"]["memoryBytes"], 100, "model launch transfers the reserved bytes");
        drop(job);
        assert_eq!(scheduler.status()["reserved"]["memoryBytes"], 80, "idle runtime retains its model reservation");
        let reused = scheduler.enqueue_worker("org.example.model", Priority::Interactive, resource(20), resident).unwrap().wait(&JobCtx::new()).unwrap();
        assert_eq!(scheduler.status()["reserved"]["memoryBytes"], 100, "reusing an idle model must not wait for or reserve another 80 bytes");
        drop(model);
        assert_eq!(scheduler.status()["reserved"]["memoryBytes"], 100, "idle expiry transfers its reserved allocation back to the admitted job");
        let restarted = scheduler.resident("org.example.model", &declaration, &JobCtx::new()).unwrap();
        assert_eq!(scheduler.status()["reserved"]["memoryBytes"], 100);
        drop(reused);
        assert_eq!(scheduler.status()["reserved"]["memoryBytes"], 80);
        drop(restarted);
        assert_eq!(scheduler.status()["reserved"], serde_json::to_value(Resources::default()).unwrap());
    }

    #[test]
    fn full_peak_admission_prevents_jobs_holding_host_memory_while_waiting_for_models() {
        let scheduler = PluginScheduler::new(Limits {
            running_jobs: 4,
            resources: Resources { cpu_units: 4, memory_bytes: 100, gpu_bytes: 100, temp_bytes: 100, cache_bytes: 100 },
            ..Limits::default()
        });
        let resident = Resources { memory_bytes: 60, ..Default::default() };
        let first = scheduler.enqueue_worker("org.example.one", Priority::Interactive, resource(30), resident).unwrap().wait(&JobCtx::new()).unwrap();
        let second = scheduler.enqueue_worker("org.example.two", Priority::Interactive, resource(30), resident).unwrap();
        assert_eq!(scheduler.status()["reserved"]["memoryBytes"], 90);
        assert_eq!(selected(&scheduler.shared.state.lock().unwrap()), None, "a second partial admission could deadlock both model launches");
        scheduler.shared.state.lock().unwrap().queued.front_mut().unwrap().bypasses = 8;
        let declaration = photocraft_plugin_api::WorkerResources { cpu_units: 1, memory_bytes: 60, gpu_bytes: 0, temp_bytes: 0, cache_bytes: 0 };
        let model = scheduler.resident("org.example.one", &declaration, &JobCtx::new()).unwrap();
        drop(first);
        drop(model);
        drop(second.wait(&JobCtx::new()).unwrap());
        assert_eq!(scheduler.status()["reserved"]["memoryBytes"], 0);
    }

    #[test]
    fn private_store_owner_keys_never_share_resident_credit_or_cancellation() {
        let scheduler = scheduler();
        let declaration = photocraft_plugin_api::WorkerResources { cpu_units: 1, memory_bytes: 40, gpu_bytes: 0, temp_bytes: 0, cache_bytes: 0 };
        let resident = Resources { memory_bytes: 40, ..Default::default() };
        let first_context = JobCtx::new();
        let first = scheduler.enqueue_worker("org.example.same:store-a", Priority::Interactive, resource(20), resident).unwrap().wait(&first_context).unwrap();
        let first_model = scheduler.resident("org.example.same:store-a", &declaration, &first_context).unwrap();
        drop(first);
        let second_context = JobCtx::new();
        let second =
            scheduler.enqueue_worker("org.example.same:store-b", Priority::Interactive, resource(20), resident).unwrap().wait(&second_context).unwrap();
        assert_eq!(scheduler.status()["reserved"]["memoryBytes"], 100, "a different private store needs another model allocation");
        let second_model = scheduler.resident("org.example.same:store-b", &declaration, &second_context).unwrap();
        drop(second);
        assert_eq!(scheduler.status()["reserved"]["memoryBytes"], 80);
        scheduler.cancel_plugin("org.example.same:store-a");
        assert!(first_context.cancelled());
        assert!(!second_context.cancelled());
        drop(first_model);
        assert_eq!(scheduler.status()["reserved"]["memoryBytes"], 40);
        drop(second_model);
    }

    #[test]
    fn round_robin_reaches_a_third_owner_before_repeating_the_first() {
        let scheduler = scheduler();
        let first = scheduler.enqueue("org.example.one", Priority::Interactive, resource(1)).unwrap();
        let repeated = scheduler.enqueue("org.example.one", Priority::Interactive, resource(1)).unwrap();
        let second = scheduler.enqueue("org.example.two", Priority::Interactive, resource(1)).unwrap();
        let third = scheduler.enqueue("org.example.three", Priority::Interactive, resource(1)).unwrap();
        drop(first.wait(&JobCtx::new()).unwrap());
        drop(second.wait(&JobCtx::new()).unwrap());
        assert_eq!(selected(&scheduler.shared.state.lock().unwrap()), third.id);
        drop(third.wait(&JobCtx::new()).unwrap());
        drop(repeated.wait(&JobCtx::new()).unwrap());
    }

    #[test]
    fn cancellation_wakes_a_waiting_thread_without_releasing_the_active_workers_resources() {
        let scheduler = scheduler();
        let running = scheduler.enqueue("org.example.running", Priority::Interactive, resource(80)).unwrap().wait(&JobCtx::new()).unwrap();
        let waiting = scheduler.enqueue("org.example.waiting", Priority::Interactive, resource(80)).unwrap();
        let context = JobCtx::new();
        let worker_context = context.clone();
        let (sent, received) = std::sync::mpsc::sync_channel(1);
        let thread = std::thread::spawn(move || {
            let result = waiting.wait(&worker_context);
            sent.send(matches!(result, Err(EngineError::Cancelled))).unwrap();
        });
        context.cancel();
        assert!(received.recv_timeout(std::time::Duration::from_millis(250)).unwrap());
        thread.join().unwrap();
        assert_eq!(scheduler.status()["reserved"]["memoryBytes"], 80);
        drop(running);
    }
}
