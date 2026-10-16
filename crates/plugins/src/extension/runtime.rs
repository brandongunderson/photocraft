use wasmi::{Config, EnforcedLimits, Engine, Instance, Linker, Memory, Module, Store, StoreLimits, StoreLimitsBuilder, TypedFunc, WasmParams, WasmResults};

use super::{MAX_JSON_BYTES, MAX_MODULE_BYTES, Manifest, Response};
use crate::{Error, Result};
use std::sync::{
    Mutex, OnceLock,
    atomic::{AtomicU32, Ordering},
};

const FUEL: u64 = 5_000_000;
const MEMORY: usize = 32 << 20;

/// No imports, filesystem, network, GPU or native code. Only JSON crosses the boundary.
pub struct Extension {
    pub manifest: Manifest,
    runtime: Runtime,
    authority: Option<Box<dyn Fn() -> Result<()> + Send + Sync>>,
}

enum Runtime {
    Ready { engine: Engine, module: Module },
    Deferred { loader: Box<dyn Fn() -> Result<Vec<u8>> + Send + Sync>, loaded: OnceLock<Box<Extension>>, gate: Mutex<()>, failures: AtomicU32 },
}

struct Live {
    store: Store<StoreLimits>,
    instance: Instance,
    memory: Memory,
}

fn trap(e: wasmi::Error) -> Error {
    if e.as_trap_code() == Some(wasmi::TrapCode::OutOfFuel) { Error::Limit("extension instruction budget".into()) } else { Error::Trap(e.to_string()) }
}

impl Live {
    fn func<P: WasmParams, R: WasmResults>(&self, name: &str) -> Result<TypedFunc<P, R>> {
        self.instance.get_typed_func(&self.store, name).map_err(|e| Error::Abi(format!("export {name}: {e}")))
    }

    fn read(&self, packed: i64, limit: usize) -> Result<&[u8]> {
        let packed = packed as u64;
        let ptr = (packed & 0xffff_ffff) as usize;
        let len = (packed >> 32) as usize;
        if len > limit {
            return Err(Error::Limit("extension JSON size".into()));
        }
        let end = ptr.checked_add(len).ok_or_else(|| Error::Abi("extension pointer overflow".into()))?;
        self.memory.data(&self.store).get(ptr..end).ok_or_else(|| Error::Abi("extension JSON points outside memory".into()))
    }
}

impl Extension {
    pub fn load(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_MODULE_BYTES || !bytes.starts_with(b"\0asm") {
            return Err(Error::Module("expected a WebAssembly extension of at most 8 MiB".into()));
        }
        let mut config = Config::default();
        config.consume_fuel(true).enforced_limits(EnforcedLimits::strict()).set_max_recursion_depth(256);
        let engine = Engine::new(&config);
        let module = Module::new(&engine, bytes).map_err(|e| Error::Module(e.to_string()))?;
        if module.imports().next().is_some() {
            return Err(Error::Abi("extensions may not import host functions or WASI".into()));
        }
        let mut live = Self::instantiate(&engine, &module)?;
        let version = live.func::<(), i32>("pce_api_version")?.call(&mut live.store, ()).map_err(trap)?;
        if version != photocraft_plugin_api::PANEL_API_VERSION as i32 {
            return Err(Error::Abi(format!(
                "unsupported marker {version} for the current PhotoCraft Plugin API; expected {}",
                photocraft_plugin_api::PANEL_API_VERSION
            )));
        }
        let packed = live.func::<(), i64>("pce_manifest")?.call(&mut live.store, ()).map_err(trap)?;
        let manifest = Manifest::parse(live.read(packed, crate::manifest::MAX_MANIFEST_BYTES)?)?;
        live.func::<i32, i32>("pce_alloc")?;
        live.func::<(i32, i32), i64>("pce_handle")?;
        Ok(Self { manifest, runtime: Runtime::Ready { engine, module }, authority: None })
    }

    /// A host-created, validated descriptor may be discovered without executing guest code.
    /// Activation always loads the verified bytes and compares the embedded descriptor.
    pub fn deferred(manifest: Manifest, loader: impl Fn() -> Result<Vec<u8>> + Send + Sync + 'static) -> Self {
        Self {
            manifest,
            runtime: Runtime::Deferred { loader: Box::new(loader), loaded: OnceLock::new(), gate: Mutex::new(()), failures: AtomicU32::new(0) },
            authority: None,
        }
    }

    pub fn with_authority(mut self, authority: impl Fn() -> Result<()> + Send + Sync + 'static) -> Self {
        self.authority = Some(Box::new(authority));
        self
    }

    pub fn check_current(&self) -> Result<()> {
        if let Some(authority) = &self.authority {
            authority()?;
        }
        Ok(())
    }

    pub fn lifecycle(&self) -> &'static str {
        match &self.runtime {
            Runtime::Ready { .. } => "active",
            Runtime::Deferred { loaded, .. } if loaded.get().is_some() => "active",
            Runtime::Deferred { failures, .. } if failures.load(Ordering::Relaxed) >= 3 => "failed",
            Runtime::Deferred { .. } => "idle",
        }
    }

    pub fn retry_activation(&self) {
        if let Runtime::Deferred { failures, .. } = &self.runtime {
            failures.store(0, Ordering::Relaxed);
        }
    }

    pub fn activate(&self) -> Result<()> {
        self.compiled().map(|_| ())
    }

    fn compiled(&self) -> Result<(&Engine, &Module)> {
        match &self.runtime {
            Runtime::Ready { engine, module } => Ok((engine, module)),
            Runtime::Deferred { loader, loaded, gate, failures } => {
                if let Some(extension) = loaded.get() {
                    return extension.compiled();
                }
                let _guard = gate.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                if loaded.get().is_none() {
                    if failures.load(Ordering::Relaxed) >= 3 {
                        return Err(Error::Failed("Plugin activation is quarantined after three failures. Enable the plugin again to retry.".into()));
                    }
                    let candidate = loader().and_then(|bytes| Self::load(&bytes)).and_then(|extension| {
                        if extension.manifest != self.manifest {
                            return Err(Error::Manifest("Installed descriptor differs from verified guest code".into()));
                        }
                        Ok(extension)
                    });
                    match candidate {
                        Ok(extension) => {
                            let _ = loaded.set(Box::new(extension));
                        }
                        Err(cause) => {
                            failures.fetch_add(1, Ordering::Relaxed);
                            return Err(cause);
                        }
                    }
                }
                loaded.get().ok_or_else(|| Error::Failed("Plugin activation did not complete".into()))?.compiled()
            }
        }
    }

    fn instantiate(engine: &Engine, module: &Module) -> Result<Live> {
        let limits = StoreLimitsBuilder::new().memory_size(MEMORY).memories(1).tables(2).table_elements(4096).instances(1).build();
        let mut store = Store::new(engine, limits);
        store.limiter(|l| l);
        // One shared fuel budget covers start, allocation and the handler.
        store.set_fuel(FUEL).map_err(trap)?;
        let instance = Linker::<StoreLimits>::new(engine).instantiate_and_start(&mut store, module).map_err(trap)?;
        let memory = instance.get_memory(&store, "memory").ok_or_else(|| Error::Abi("extension must export memory".into()))?;
        Ok(Live { store, instance, memory })
    }

    pub fn handle(&self, request: &serde_json::Value) -> Result<Response> {
        self.check_current()?;
        let bytes = serde_json::to_vec(request).map_err(|e| Error::Params(e.to_string()))?;
        if bytes.len() > MAX_JSON_BYTES {
            return Err(Error::Limit("extension request size".into()));
        }
        let (engine, module) = self.compiled()?;
        let mut live = Self::instantiate(engine, module)?;
        let ptr = live.func::<i32, i32>("pce_alloc")?.call(&mut live.store, bytes.len() as i32).map_err(trap)? as u32 as usize;
        if ptr == 0 {
            return Err(Error::Failed("pce_alloc returned zero".into()));
        }
        let end = ptr.checked_add(bytes.len()).ok_or_else(|| Error::Abi("extension allocation overflow".into()))?;
        let dst = live.memory.data_mut(&mut live.store).get_mut(ptr..end).ok_or_else(|| Error::Abi("extension allocation outside memory".into()))?;
        dst.copy_from_slice(&bytes);
        let packed = live.func::<(i32, i32), i64>("pce_handle")?.call(&mut live.store, (ptr as i32, bytes.len() as i32)).map_err(trap)?;
        Response::parse(live.read(packed, MAX_JSON_BYTES)?)
    }
}
