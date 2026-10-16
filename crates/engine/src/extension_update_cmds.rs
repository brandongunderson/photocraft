//! Explicit, bounded release checks and reviewed downloads. Updating never executes downloaded
//! native code or publishes a generation: the regular installer remains the only apply path.
use serde_json::Value;

use crate::{EngineError, Result, Session, commands::CommandSpec};

#[cfg(not(target_arch = "wasm32"))]
pub(crate) use desktop::UpdateState;

fn error(message: impl std::fmt::Display) -> EngineError {
    EngineError::Other(message.to_string())
}

pub fn specs() -> Vec<CommandSpec> {
    let spec = |id, label, params, run| CommandSpec { id, label, params, run, menu: &[], shortcut: None, enabled: crate::commands::always, journal: false };
    vec![
        spec("extension.updates.check", "Check Plugin Updates", r#"{"id":text?}"#, check),
        spec("extension.updates.prepare", "Download Plugin Update", r#"{"id":text,"candidate":text}"#, prepare),
    ]
}

fn check(s: &mut Session, p: &Value) -> Result<Value> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        desktop::check(s, p)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (s, p);
        Err(error("Plugin update checks require the desktop application"))
    }
}

fn prepare(s: &mut Session, p: &Value) -> Result<Value> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        desktop::prepare(s, p)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (s, p);
        Err(error("Plugin update downloads require the desktop application"))
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod desktop {
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    };
    use std::time::{Duration, Instant};

    use photocraft_plugins::extension::Extension;
    use semver::Version;
    use serde::Deserialize;
    use serde_json::json;
    use tempfile::TempPath;

    use super::*;

    const MAX_CANDIDATES: usize = 16;
    const MAX_SOURCES: usize = 64;
    const TTL: Duration = Duration::from_secs(15 * 60);
    const CHECK_BUDGET: Duration = Duration::from_secs(120);
    static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);
    static ACTIVE_REQUESTS: AtomicUsize = AtomicUsize::new(0);

    struct RequestSlot;

    impl RequestSlot {
        fn reserve() -> Result<Self> {
            let mut count = ACTIVE_REQUESTS.load(Ordering::Acquire);
            loop {
                if count >= 4 {
                    return Err(error("Plugin update requests are busy; wait for an existing check or download to finish"));
                }
                match ACTIVE_REQUESTS.compare_exchange_weak(count, count + 1, Ordering::AcqRel, Ordering::Acquire) {
                    Ok(_) => return Ok(Self),
                    Err(actual) => count = actual,
                }
            }
        }
    }

    impl Drop for RequestSlot {
        fn drop(&mut self) {
            ACTIVE_REQUESTS.fetch_sub(1, Ordering::AcqRel);
        }
    }

    fn token() -> Result<String> {
        let mut value = NEXT_TOKEN.load(Ordering::Relaxed);
        loop {
            let next = value.checked_add(1).ok_or_else(|| error("Plugin update request limit reached"))?;
            match NEXT_TOKEN.compare_exchange_weak(value, next, Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => return Ok(format!("plugin-update-{value:x}")),
                Err(actual) => value = actual,
            }
        }
    }

    #[derive(Default)]
    pub(crate) struct UpdateState {
        checks: BTreeMap<String, String>,
        preparations: BTreeMap<String, String>,
        candidates: BTreeMap<String, Candidate>,
        prepared: BTreeMap<String, Prepared>,
    }

    struct Prepared {
        _path: TempPath,
        created: Instant,
    }

    impl UpdateState {
        pub(crate) fn expire(&mut self) {
            self.candidates.retain(|_, candidate| candidate.created.elapsed() < TTL);
            self.prepared.retain(|_, prepared| prepared.created.elapsed() < TTL);
        }

        /// Drop downloaded files only after the installer has consumed them, or on removal.
        pub(crate) fn invalidate(&mut self, id: &str) {
            self.checks.remove(id);
            self.preparations.remove(id);
            self.candidates.remove(id);
            self.prepared.remove(id);
        }

        fn retain_candidate(&mut self, candidate: Candidate) {
            let id = candidate.target.id.clone();
            self.candidates.remove(&id);
            if self.candidates.len() >= MAX_CANDIDATES
                && let Some(oldest) = self.candidates.iter().min_by_key(|(_, value)| value.created).map(|(id, _)| id.clone())
            {
                self.candidates.remove(&oldest);
            }
            self.candidates.insert(id, candidate);
        }
    }

    #[derive(Clone)]
    struct Target {
        id: String,
        installed_version: String,
        repository: String,
        source: Value,
        asset: String,
        extension: Arc<Extension>,
        folder: PathBuf,
        guard: Value,
    }

    impl Target {
        fn capture(s: &Session, id: &str) -> Result<Self> {
            let installed = s.extensions.installed.get(id).ok_or_else(|| error("Plugin is no longer installed"))?;
            let manifest = &installed.extension.manifest;
            let repository = manifest.repository.clone().ok_or_else(|| error("This plugin does not declare a GitHub repository"))?;
            photocraft_plugin_api::github_repository_parts(&repository).map_err(error)?;
            let source = manifest.updates.as_ref().ok_or_else(|| error("This plugin does not publish automatic updates"))?;
            let asset = source.asset_for_platform(platform()?).map_err(error)?;
            version(&manifest.version)?;
            Ok(Self {
                id: id.into(),
                installed_version: manifest.version.clone(),
                repository,
                source: serde_json::to_value(source).map_err(error)?,
                asset,
                extension: installed.extension.clone(),
                folder: s.extensions.folder.clone().ok_or_else(|| error("Plugin updates require a persistent installation folder"))?,
                guard: crate::extension_cmds::installation_guard(s, id)?,
            })
        }

        fn current(&self, s: &Session) -> Result<()> {
            if s.extensions.folder.as_ref() != Some(&self.folder)
                || !s.extensions.installed.get(&self.id).is_some_and(|installed| Arc::ptr_eq(&installed.extension, &self.extension))
                || crate::extension_cmds::installation_guard(s, &self.id)? != self.guard
            {
                return Err(error("Plugin installation changed; check for updates again"));
            }
            Ok(())
        }

        fn endpoint(&self, suffix: &str) -> Result<String> {
            let (owner, repo) = photocraft_plugin_api::github_repository_parts(&self.repository).map_err(error)?;
            Ok(format!("https://api.github.com/repos/{owner}/{repo}/releases/{suffix}"))
        }
    }

    fn platform() -> Result<&'static str> {
        match (std::env::consts::OS, std::env::consts::ARCH) {
            ("windows", "x86_64") => Ok("windows-x86_64"),
            ("windows", "aarch64") => Ok("windows-aarch64"),
            ("linux", "x86_64") => Ok("linux-x86_64"),
            ("linux", "aarch64") => Ok("linux-aarch64"),
            ("macos", "x86_64") => Ok("macos-x86_64"),
            ("macos", "aarch64") => Ok("macos-aarch64"),
            _ => Err(error("Plugin updates are not available for this platform")),
        }
    }

    fn version(value: &str) -> Result<Version> {
        if value.len() > 128 {
            return Err(error("Plugin update version is too long"));
        }
        Version::parse(value.strip_prefix('v').unwrap_or(value))
            .map_err(|_| error("This version cannot be compared automatically; use the plugin repository to update manually"))
    }

    #[derive(Clone)]
    struct Candidate {
        token: String,
        target: Target,
        version: String,
        asset_id: u64,
        size: u64,
        digest: String,
        created: Instant,
    }

    #[derive(Clone, Deserialize)]
    struct Release {
        id: u64,
        tag_name: String,
        draft: bool,
        prerelease: bool,
        assets: Vec<Asset>,
    }

    #[derive(Clone, Deserialize)]
    struct Asset {
        id: u64,
        name: String,
        size: u64,
        state: String,
        digest: Option<String>,
    }

    fn sha256(value: &str) -> Result<String> {
        let hex = value.strip_prefix("sha256:").ok_or_else(|| error("The release asset has no SHA-256 digest; update manually from its repository"))?;
        if hex.len() != 64 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(error("The release asset SHA-256 digest is invalid"));
        }
        Ok(hex.to_ascii_lowercase())
    }

    fn select(target: &Target, release: &Release) -> Result<Option<Candidate>> {
        if release.id == 0 || release.draft || release.prerelease {
            return Err(error("The repository did not return a published stable release"));
        }
        let latest = version(&release.tag_name)?;
        if !latest.pre.is_empty() {
            return Err(error("The latest release is a prerelease; update manually from its repository"));
        }
        if latest.cmp_precedence(&version(&target.installed_version)?) != std::cmp::Ordering::Greater {
            return Ok(None);
        }
        if release.assets.len() > 256 {
            return Err(error("The release contains too many assets"));
        }
        let mut matching = release.assets.iter().filter(|asset| asset.name == target.asset);
        let asset = matching.next().ok_or_else(|| error(format!("This release has no compatible {} asset", target.asset)))?;
        if matching.next().is_some() {
            return Err(error("The release contains ambiguous plugin assets"));
        }
        let limit =
            if target.asset.ends_with(".pcext") { photocraft_plugins::extension::MAX_MODULE_BYTES } else { crate::extension_package::MAX_PACKAGE_BYTES };
        if asset.id == 0 || asset.state != "uploaded" || asset.size == 0 || asset.size > limit as u64 {
            return Err(error("The plugin release asset is unavailable or exceeds its size limit (8 MiB panel / 512 MiB package)"));
        }
        let digest = sha256(asset.digest.as_deref().unwrap_or(""))?;
        Ok(Some(Candidate {
            token: token()?,
            target: target.clone(),
            version: latest.to_string(),
            asset_id: asset.id,
            size: asset.size,
            digest,
            created: Instant::now(),
        }))
    }

    fn row(id: &str, installed: &str, status: &str, message: Option<String>) -> Value {
        let mut value = json!({"id":id,"installedVersion":installed,"status":status});
        if let Some(message) = message {
            value["message"] = json!(message);
        }
        value
    }

    pub(super) fn check(s: &mut Session, p: &Value) -> Result<Value> {
        let object = p.as_object().ok_or_else(|| error("Update check parameters must be an object"))?;
        if object.keys().any(|key| key != "id") {
            return Err(error("Unknown update check parameter"));
        }
        let ids = if let Some(id) = p.get("id") {
            let id = id.as_str().filter(|id| !id.is_empty() && id.len() <= 64).ok_or_else(|| error("Invalid plugin id"))?;
            if !s.extensions.installed.contains_key(id) {
                return Err(error("Plugin is not installed"));
            }
            vec![id.to_owned()]
        } else {
            s.extensions.installed.keys().cloned().collect()
        };
        let slot = RequestSlot::reserve()?;
        let request = token()?;
        s.extensions.updates.expire();
        let mut targets = Vec::new();
        let mut rows = Vec::new();
        let mut configured = 0;
        for id in ids {
            s.extensions.updates.checks.insert(id.clone(), request.clone());
            s.extensions.updates.candidates.remove(&id);
            if let Some(installed) = s.extensions.installed.get(&id)
                && installed.extension.manifest.repository.is_some()
                && installed.extension.manifest.updates.is_some()
            {
                if configured >= MAX_SOURCES {
                    rows.push(row(
                        &id,
                        &installed.extension.manifest.version,
                        "error",
                        Some("This check reached the 64 plugin limit; check this plugin individually".into()),
                    ));
                    continue;
                }
                configured += 1;
            }
            match Target::capture(s, &id) {
                Ok(target) => targets.push(target),
                Err(cause) => rows.push(row(
                    &id,
                    s.extensions.installed.get(&id).map_or("", |value| value.extension.manifest.version.as_str()),
                    "unsupported",
                    Some(cause.to_string()),
                )),
            }
        }
        crate::jobs::run(
            s,
            "Check plugin updates",
            false,
            move |ctx| {
                let _slot = slot;
                ctx.check()?;
                let started = Instant::now();
                let mut releases: BTreeMap<String, std::result::Result<Release, String>> = BTreeMap::new();
                let mut checked = Vec::new();
                let client = if targets.is_empty() { None } else { Some(http::Client::new()?) };
                let total = targets.len().max(1);
                for (index, target) in targets.into_iter().enumerate() {
                    ctx.check()?;
                    ctx.progress(index as f32 / total as f32, "Checking plugin repositories");
                    let result = if started.elapsed() >= CHECK_BUDGET {
                        Err(error("The update check reached its time limit; check this plugin individually"))
                    } else {
                        let endpoint = target.endpoint("latest")?;
                        if !releases.contains_key(&endpoint) && releases.len() >= MAX_SOURCES {
                            checked.push((target, Err(error("This check reached the 64 repository limit; check this plugin individually"))));
                            continue;
                        }
                        let release = releases.entry(endpoint.clone()).or_insert_with(|| {
                            let client = client.as_ref().ok_or_else(|| "Update transport is unavailable".to_owned())?;
                            let bytes = client.metadata(&endpoint, ctx).map_err(|cause| cause.to_string())?;
                            serde_json::from_slice(&bytes).map_err(|_| "GitHub returned invalid release metadata".to_owned())
                        });
                        match release {
                            Ok(release) => select(&target, release),
                            Err(cause) => Err(error(cause)),
                        }
                    };
                    ctx.check()?;
                    checked.push((target, result));
                }
                Ok((rows, checked))
            },
            move |s, (rows, checked)| finish_check(s, &request, rows, checked),
        )
    }

    fn finish_check(s: &mut Session, request: &str, mut rows: Vec<Value>, checked: Vec<(Target, Result<Option<Candidate>>)>) -> Result<Value> {
        s.extensions.updates.expire();
        for (target, result) in checked {
            let result = if s.extensions.updates.checks.get(&target.id).map(String::as_str) != Some(request) {
                Err(error("A newer update check superseded this result"))
            } else {
                target.current(s).and(result)
            };
            match result {
                Ok(Some(candidate)) => {
                    let mut value = row(&target.id, &target.installed_version, "available", None);
                    value["version"] = json!(candidate.version);
                    value["candidate"] = json!(candidate.token);
                    s.extensions.updates.retain_candidate(candidate);
                    rows.push(value);
                }
                Ok(None) => rows.push(row(&target.id, &target.installed_version, "upToDate", None)),
                Err(cause) => rows.push(row(&target.id, &target.installed_version, "error", Some(cause.to_string()))),
            }
        }
        // The cache has a hard bound. Never return an apparently usable token evicted by
        // another result in this same batch; individual checks can prepare any plugin.
        for value in &mut rows {
            if let Some(candidate) = value.get("candidate").and_then(Value::as_str)
                && !s.extensions.updates.candidates.values().any(|entry| entry.token == candidate)
            {
                if let Some(object) = value.as_object_mut() {
                    object.remove("candidate");
                }
                value["message"] = json!("Check this plugin individually to download its update");
            }
        }
        rows.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
        Ok(json!({"updates":rows}))
    }

    fn validate_download(candidate: &Candidate, inspected: &Value) -> Result<()> {
        let manifest = inspected.get("manifest").ok_or_else(|| error("Downloaded plugin has no descriptor"))?;
        if manifest.get("id").and_then(Value::as_str) != Some(candidate.target.id.as_str())
            || manifest.get("version").and_then(Value::as_str).map(version).transpose()? != Some(version(&candidate.version)?)
            || manifest.get("repository").and_then(Value::as_str) != Some(candidate.target.repository.as_str())
            || manifest.get("updates") != Some(&candidate.target.source)
            || manifest.get("apiVersion").and_then(Value::as_u64) != Some(u64::from(photocraft_plugin_api::PANEL_API_VERSION))
        {
            return Err(error("Downloaded plugin identity, version, API or update source does not match the reviewed release"));
        }
        let package = inspected.get("package").unwrap_or(&Value::Null);
        if candidate.target.asset.ends_with(".pcext") {
            if !package.is_null() {
                return Err(error("A .pcext release asset must contain only a portable panel"));
            }
        } else if !package.is_object() {
            return Err(error("A .pcplugin release asset must contain a plugin package"));
        }
        if let Some(native) = package.get("native").filter(|value| !value.is_null())
            && native.get("platform").and_then(Value::as_str) != Some(platform()?)
        {
            return Err(error("Downloaded plugin is for another platform"));
        }
        Ok(())
    }

    pub(super) fn prepare(s: &mut Session, p: &Value) -> Result<Value> {
        let object = p.as_object().ok_or_else(|| error("Update download parameters must be an object"))?;
        if object.keys().any(|key| !matches!(key.as_str(), "id" | "candidate")) {
            return Err(error("Unknown update download parameter"));
        }
        let id = p.get("id").and_then(Value::as_str).filter(|id| !id.is_empty() && id.len() <= 64).ok_or_else(|| error("Invalid plugin id"))?.to_owned();
        let requested = p.get("candidate").and_then(Value::as_str).filter(|value| value.len() <= 128).ok_or_else(|| error("Missing update candidate"))?;
        s.extensions.updates.expire();
        let candidate = s
            .extensions
            .updates
            .candidates
            .get(&id)
            .filter(|candidate| candidate.token == requested)
            .cloned()
            .ok_or_else(|| error("This update check expired; check for updates again"))?;
        candidate.target.current(s)?;
        if s.extensions.updates.prepared.len() >= MAX_CANDIDATES && !s.extensions.updates.prepared.contains_key(&id) {
            return Err(error("At most 16 plugin downloads can await review; install one or wait for its review to expire"));
        }
        let slot = RequestSlot::reserve()?;
        let request = token()?;
        s.extensions.updates.preparations.insert(id.clone(), request.clone());
        let work_candidate = candidate.clone();
        crate::jobs::run(
            s,
            "Download plugin update",
            false,
            move |ctx| {
                let _slot = slot;
                ctx.check()?;
                let endpoint = work_candidate.target.endpoint(&format!("assets/{}", work_candidate.asset_id))?;
                let suffix = if work_candidate.target.asset.ends_with(".pcext") { ".pcext" } else { ".pcplugin" };
                let mut file = tempfile::Builder::new().prefix("photocraft-plugin-update-").suffix(suffix).tempfile().map_err(error)?;
                let downloaded_hash = http::Client::new()?.download(&endpoint, &mut file, work_candidate.size, &work_candidate.digest, ctx)?;
                file.as_file().sync_all().map_err(error)?;
                ctx.check()?;
                let path = file.into_temp_path();
                let input = json!({"path":path.to_str().ok_or_else(|| error("The temporary plugin path cannot be represented as text"))?});
                ctx.progress(0.9, "Inspecting downloaded plugin permissions");
                let inspected = crate::extension_cmds::inspect_payload(&input)?;
                if inspected.get("hash").and_then(Value::as_str) != Some(downloaded_hash.as_str()) {
                    return Err(error("The downloaded plugin changed before inspection; download it again"));
                }
                validate_download(&work_candidate, &inspected)?;
                ctx.check()?;
                Ok((path, inspected, input))
            },
            move |s, (path, inspected, mut input)| {
                s.extensions.updates.expire();
                candidate.target.current(s)?;
                if s.extensions.updates.preparations.get(&id) != Some(&request)
                    || !s.extensions.updates.candidates.get(&id).is_some_and(|current| current.token == candidate.token)
                {
                    return Err(error("This plugin download was superseded; prepare its current update again"));
                }
                if s.extensions.updates.prepared.len() >= MAX_CANDIDATES && !s.extensions.updates.prepared.contains_key(&id) {
                    return Err(error("Too many downloaded plugins are awaiting review"));
                }
                let mut inspected = crate::extension_cmds::finish_inspection(s, inspected)?;
                input["expectedHash"] = inspected.get("hash").cloned().ok_or_else(|| error("Downloaded plugin has no review hash"))?;
                input["intent"] = json!("update");
                if let Some(guards) = candidate.target.guard.as_object() {
                    for (key, value) in guards {
                        input[key] = value.clone();
                    }
                }
                inspected["input"] = input;
                s.extensions.updates.prepared.insert(id.clone(), Prepared { _path: path, created: Instant::now() });
                s.extensions.updates.preparations.remove(&id);
                Ok(inspected)
            },
        )
    }

    mod http {
        use super::*;
        use crate::jobs::JobCtx;
        use reqwest::{
            Url,
            blocking::{Client as HttpClient, Response},
            header,
        };
        use rustls_platform_verifier::BuilderVerifierExt;
        use sha2::{Digest, Sha256};
        use std::io::{Read, Write};

        const MAX_METADATA: u64 = 2 << 20;
        pub(super) struct Client(HttpClient);

        fn allowed(url: &Url, asset: bool) -> bool {
            url.scheme() == "https"
                && url.username().is_empty()
                && url.password().is_none()
                && url.port_or_known_default() == Some(443)
                && url.fragment().is_none()
                && match url.host_str() {
                    Some("api.github.com") => true,
                    Some("release-assets.githubusercontent.com" | "objects.githubusercontent.com") => asset,
                    _ => false,
                }
        }

        impl Client {
            pub(super) fn new() -> Result<Self> {
                // Select this client's provider explicitly; other editor services may choose
                // a different provider. Certificate validation still uses the OS trust store.
                let tls = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                    .with_safe_default_protocol_versions()
                    .map_err(|_| error("Could not configure secure plugin downloads"))?
                    .with_platform_verifier()
                    .map_err(|_| error("Could not initialize platform certificate verification"))?
                    .with_no_client_auth();
                let client = HttpClient::builder()
                    .tls_backend_preconfigured(tls)
                    .https_only(true)
                    .no_proxy()
                    .redirect(reqwest::redirect::Policy::none())
                    .connect_timeout(Duration::from_secs(10))
                    .timeout(Duration::from_secs(20))
                    .user_agent("PhotoCraft-plugin-updater")
                    .build()
                    .map_err(|_| error("Could not initialize secure plugin downloads"))?;
                Ok(Self(client))
            }

            fn get(&self, endpoint: &str, asset: bool, ctx: &JobCtx) -> Result<Response> {
                let mut url = Url::parse(endpoint).map_err(|_| error("Invalid plugin release URL"))?;
                let started = Instant::now();
                let timeout = if asset { Duration::from_secs(300) } else { Duration::from_secs(20) };
                for redirects in 0..=3 {
                    ctx.check()?;
                    if !allowed(&url, asset) {
                        return Err(error("Plugin download redirected outside the trusted GitHub hosts"));
                    }
                    let remaining = timeout.checked_sub(started.elapsed()).ok_or_else(|| error("Plugin request timed out"))?;
                    let response = self
                        .0
                        .get(url.clone())
                        .timeout(remaining)
                        .header(header::ACCEPT, if asset { "application/octet-stream" } else { "application/vnd.github+json" })
                        .header(header::ACCEPT_ENCODING, "identity")
                        .header("X-GitHub-Api-Version", "2022-11-28")
                        .send()
                        .map_err(|_| error("Could not reach GitHub; check the connection and try again"))?;
                    ctx.check()?;
                    let status = response.status();
                    if status.is_redirection() {
                        // Repository transfers must be reviewed locally instead of silently
                        // changing the source. Only binary asset delivery may follow redirects.
                        if !asset || redirects == 3 {
                            return Err(error("GitHub redirected this source; open the repository and review it manually"));
                        }
                        let location = response
                            .headers()
                            .get(header::LOCATION)
                            .and_then(|value| value.to_str().ok())
                            .ok_or_else(|| error("GitHub returned an invalid download redirect"))?;
                        if location.len() > 8192 {
                            return Err(error("GitHub download redirect is too long"));
                        }
                        url = url.join(location).map_err(|_| error("GitHub returned an invalid download redirect"))?;
                        continue;
                    }
                    if !status.is_success() {
                        return Err(error(match status.as_u16() {
                            403 | 429 => "GitHub limited this request; try checking for updates later",
                            404 => "No public release or matching asset is available in this repository",
                            _ => "GitHub could not complete the plugin request; try again later",
                        }));
                    }
                    if status.as_u16() != 200 {
                        return Err(error("GitHub returned an unexpected partial response"));
                    }
                    if response.headers().get(header::CONTENT_ENCODING).is_some_and(|value| value != "identity") {
                        return Err(error("Compressed plugin HTTP responses are not supported"));
                    }
                    return Ok(response);
                }
                Err(error("GitHub download exceeded its redirect limit"))
            }

            pub(super) fn metadata(&self, endpoint: &str, ctx: &JobCtx) -> Result<Vec<u8>> {
                let mut response = self.get(endpoint, false, ctx)?;
                if response.content_length().is_some_and(|size| size > MAX_METADATA) {
                    return Err(error("GitHub release metadata exceeds 2 MiB"));
                }
                let mut bytes = Vec::new();
                transfer(&mut response, &mut bytes, MAX_METADATA, None, ctx)?;
                Ok(bytes)
            }

            pub(super) fn download(&self, endpoint: &str, output: &mut impl Write, size: u64, digest: &str, ctx: &JobCtx) -> Result<String> {
                let mut response = self.get(endpoint, true, ctx)?;
                if response.content_length().is_some_and(|length| length != size) {
                    return Err(error("The release asset size changed; check for updates again"));
                }
                let (actual, hash, review_hash) = transfer(&mut response, output, size, Some(size), ctx)?;
                if actual != size || hash != digest {
                    return Err(error("Downloaded plugin does not match the release size or SHA-256 digest"));
                }
                Ok(review_hash)
            }
        }

        fn transfer(input: &mut impl Read, output: &mut impl Write, limit: u64, progress_size: Option<u64>, ctx: &JobCtx) -> Result<(u64, String, String)> {
            let mut bytes = 0_u64;
            let mut digest = Sha256::new();
            let mut review_hash = blake3::Hasher::new();
            let mut buffer = [0_u8; 64 << 10];
            loop {
                ctx.check()?;
                let count = input.read(&mut buffer).map_err(|_| error("Plugin download was interrupted; try again"))?;
                ctx.check()?;
                if count == 0 {
                    break;
                }
                bytes =
                    bytes.checked_add(count as u64).filter(|bytes| *bytes <= limit).ok_or_else(|| error("Plugin response exceeds its declared size limit"))?;
                let chunk = buffer.get(..count).ok_or_else(|| error("Invalid plugin response chunk"))?;
                digest.update(chunk);
                review_hash.update(chunk);
                output.write_all(chunk).map_err(|_| error("Could not store the downloaded plugin; check available disk space"))?;
                if let Some(size) = progress_size.filter(|size| *size > 0) {
                    ctx.progress(bytes as f32 / size as f32 * 0.85, "Downloading plugin update");
                }
            }
            Ok((bytes, format!("{:x}", digest.finalize()), review_hash.finalize().to_hex().to_string()))
        }

        #[cfg(test)]
        mod tests {
            use super::*;

            #[test]
            fn secure_transport_initializes_with_an_explicit_provider() {
                assert!(Client::new().is_ok());
            }

            #[test]
            fn redirects_reject_credentials_plaintext_ports_and_host_lookalikes() {
                for url in [
                    "http://api.github.com/",
                    "https://api.github.com.evil.test/",
                    "https://user@api.github.com/",
                    "https://api.github.com:444/",
                    "https://127.0.0.1/",
                    "https://github.com/owner/repo",
                    "https://objects.githubusercontent.com/#fragment",
                ] {
                    assert!(!allowed(&Url::parse(url).unwrap(), true), "{url}");
                }
                assert!(allowed(&Url::parse("https://api.github.com/repos/example/example/releases/latest").unwrap(), false));
                assert!(allowed(&Url::parse("https://release-assets.githubusercontent.com/asset?signature=public").unwrap(), true));
                assert!(!allowed(&Url::parse("https://release-assets.githubusercontent.com/asset").unwrap(), false));
            }

            #[test]
            fn stream_bounds_hash_and_cancellation_are_enforced_before_write() {
                let ctx = JobCtx::new();
                let mut output = Vec::new();
                let (size, hash, review_hash) = transfer(&mut &b"abc"[..], &mut output, 3, Some(3), &ctx).unwrap();
                assert_eq!(size, 3);
                assert_eq!(hash, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
                assert_eq!(review_hash, blake3::hash(b"abc").to_hex().as_str());
                let mut output = Vec::new();
                assert!(transfer(&mut &b"abcd"[..], &mut output, 3, None, &ctx).is_err());
                assert!(output.is_empty());
                ctx.cancel();
                assert!(transfer(&mut &b"abc"[..], &mut output, 3, None, &ctx).is_err());
                assert!(output.is_empty());
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn target() -> Target {
            let manifest = photocraft_plugins::extension::Manifest::parse(br#"{"apiVersion":1,"id":"org.example.updater","name":"Updater fixture","version":"1.9.0","permissions":{},"panel":{"buttons":[{"id":"open","label":"Open"}]},"repository":"https://github.com/example/updater","updates":{"provider":"github","asset":"plugin.pcplugin"}}"#).unwrap();
            Target {
                id: manifest.id.clone(),
                installed_version: manifest.version.clone(),
                repository: manifest.repository.clone().unwrap(),
                source: serde_json::to_value(&manifest.updates).unwrap(),
                asset: "plugin.pcplugin".into(),
                extension: Arc::new(Extension::deferred(manifest, || Err(photocraft_plugins::Error::Manifest("not executed".into())))),
                folder: PathBuf::from("fixture"),
                guard: json!({"expectedInstalledHash":"old","expectedActivation":1,"expectedSessionRevision":1}),
            }
        }

        fn release() -> Release {
            Release {
                id: 1,
                tag_name: "v1.10.0".into(),
                draft: false,
                prerelease: false,
                assets: vec![Asset {
                    id: 7,
                    name: "plugin.pcplugin".into(),
                    size: 100,
                    state: "uploaded".into(),
                    digest: Some(format!("sha256:{}", "a".repeat(64))),
                }],
            }
        }

        #[test]
        fn newer_release_requires_exact_unambiguous_asset_and_sha256() {
            let target = target();
            let mut release = release();
            assert_eq!(select(&target, &release).unwrap().unwrap().version, "1.10.0");
            release.tag_name = "1.8.0".into();
            assert!(select(&target, &release).unwrap().is_none());
            release.tag_name = "1.10.0".into();
            release.assets[0].name = "other.pcplugin".into();
            assert!(select(&target, &release).is_err());
            release.assets[0].name = target.asset.clone();
            release.assets.push(release.assets[0].clone());
            assert!(select(&target, &release).is_err());
            release.assets.pop();
            release.assets[0].digest = None;
            assert!(select(&target, &release).is_err());
        }

        #[test]
        fn invalid_versions_and_prereleases_never_become_available() {
            assert!(version("1.10").is_err());
            assert!(version("latest").is_err());
            assert!(version(&"1".repeat(129)).is_err());
            let target = target();
            let mut release = release();
            release.tag_name = "v2.0.0-rc.1".into();
            assert!(select(&target, &release).is_err());
            release.tag_name = "2.0.0".into();
            release.draft = true;
            assert!(select(&target, &release).is_err());
        }

        #[test]
        fn build_metadata_does_not_turn_equal_precedence_into_an_update() {
            let mut target = target();
            target.installed_version = "1.10.0+first".into();
            let mut release = release();
            release.tag_name = "v1.10.0+second".into();
            assert!(select(&target, &release).unwrap().is_none());
        }

        #[test]
        fn portable_panel_updates_use_the_panel_byte_limit_and_require_no_package() {
            let mut target = target();
            target.asset = "plugin.pcext".into();
            let mut release = release();
            release.assets[0].name = target.asset.clone();
            let candidate = select(&target, &release).unwrap().unwrap();
            let mut inspected = json!({"manifest":candidate.target.extension.manifest,"package":null});
            inspected["manifest"]["version"] = json!("1.10.0");
            assert!(validate_download(&candidate, &inspected).is_ok());
            inspected["package"] = json!({"packageVersion":2});
            assert!(validate_download(&candidate, &inspected).is_err());
            release.assets[0].size = photocraft_plugins::extension::MAX_MODULE_BYTES as u64 + 1;
            assert!(select(&target, &release).is_err());
        }

        #[test]
        fn downloaded_identity_version_source_and_platform_must_match() {
            let candidate = select(&target(), &release()).unwrap().unwrap();
            let mut inspected = json!({"manifest": candidate.target.extension.manifest,"package":{"packageVersion":2,"module":"plugin.pcext"}});
            inspected["manifest"]["version"] = json!("1.10.0");
            assert!(validate_download(&candidate, &inspected).is_ok());
            for (key, value) in [
                ("id", json!("org.other.plugin")),
                ("version", json!("1.11.0")),
                ("repository", json!("https://github.com/other/source")),
                ("apiVersion", json!(0)),
                ("updates", json!({"provider":"github","asset":"other.pcplugin"})),
            ] {
                let mut changed = inspected.clone();
                changed["manifest"][key] = value;
                assert!(validate_download(&candidate, &changed).is_err(), "{key}");
            }
            inspected["package"]["native"] = json!({"platform":"other-platform"});
            assert!(validate_download(&candidate, &inspected).is_err());
        }

        #[test]
        fn cache_expiration_and_invalidation_drop_owned_downloads() {
            let mut state = UpdateState::default();
            let file = tempfile::NamedTempFile::new().unwrap().into_temp_path();
            let path = file.to_path_buf();
            state.prepared.insert("fixture".into(), Prepared { _path: file, created: Instant::now() - TTL });
            state.expire();
            assert!(!path.exists());
            let file = tempfile::NamedTempFile::new().unwrap().into_temp_path();
            let path = file.to_path_buf();
            state.prepared.insert("fixture".into(), Prepared { _path: file, created: Instant::now() });
            state.invalidate("fixture");
            assert!(!path.exists());
            assert!(target().current(&Session::new()).is_err());
        }

        #[test]
        fn disabled_plugin_target_rejects_changed_instance_and_disk_pointer_without_invalidating_unrelated_management() {
            let folder = tempfile::tempdir().unwrap();
            let fixture = target();
            let mut session = Session::new();
            session.extensions.folder = Some(folder.path().to_path_buf());
            session.extensions.installed.insert(
                fixture.id.clone(),
                crate::extension_cmds::Installed {
                    extension: fixture.extension.clone(),
                    enabled: false,
                    state: Value::Null,
                    storage: Default::default(),
                    filters: BTreeMap::new(),
                    package: None,
                    native: None,
                    deferred: None,
                },
            );
            let captured = Target::capture(&session, &fixture.id).unwrap();
            assert!(captured.current(&session).is_ok(), "disabled plugins still support updates");
            session.extensions.revision += 1;
            assert!(captured.current(&session).is_ok(), "unrelated plugin management must not invalidate a persistent target");
            session.extensions.revision -= 1;
            session.extensions.installed.get_mut(&fixture.id).unwrap().extension = target().extension;
            assert!(captured.current(&session).is_err(), "same descriptor in a new runtime is stale");
            session.extensions.installed.get_mut(&fixture.id).unwrap().extension = fixture.extension;
            assert!(captured.current(&session).is_ok());
            std::fs::write(folder.path().join(format!("{}.pcext", fixture.id)), b"another active pointer").unwrap();
            assert!(captured.current(&session).is_err(), "a cross-process disk change is stale");
            let refreshed = Target::capture(&session, &fixture.id).unwrap();
            crate::extension_package::revoke_activation(folder.path(), &fixture.id).unwrap();
            assert!(refreshed.current(&session).is_err(), "enable/disable or same-byte reinstall changes activation");
            let refreshed = Target::capture(&session, &fixture.id).unwrap();
            session.extensions.installed.remove(&fixture.id);
            assert!(refreshed.current(&session).is_err(), "uninstalled targets cannot complete");
        }

        #[test]
        fn candidates_are_bounded_and_cannot_be_prepared_in_another_session_or_for_another_id() {
            let mut session = Session::new();
            let first = select(&target(), &release()).unwrap().unwrap();
            let first_token = first.token.clone();
            let first_id = first.target.id.clone();
            session.extensions.updates.retain_candidate(first);
            assert!(prepare(&mut Session::new(), &json!({"id":first_id,"candidate":first_token})).is_err());
            assert!(prepare(&mut session, &json!({"id":"org.example.other","candidate":first_token})).is_err());
            assert!(prepare(&mut session, &json!({"id":first_id,"candidate":"invented-token"})).is_err());
            for index in 0..MAX_CANDIDATES {
                let mut candidate = select(&target(), &release()).unwrap().unwrap();
                candidate.target.id = format!("org.example.fixture{index}");
                session.extensions.updates.retain_candidate(candidate);
            }
            assert_eq!(session.extensions.updates.candidates.len(), MAX_CANDIDATES);
            assert!(!session.extensions.updates.candidates.contains_key(&first_id));
        }

        #[test]
        fn delayed_check_completion_cannot_replace_a_newer_candidate() {
            let mut session = Session::new();
            let old = select(&target(), &release()).unwrap().unwrap();
            let current = select(&target(), &release()).unwrap().unwrap();
            let current_token = current.token.clone();
            let id = current.target.id.clone();
            session.extensions.updates.checks.insert(id.clone(), "new-request".into());
            session.extensions.updates.retain_candidate(current);
            let result = finish_check(&mut session, "old-request", Vec::new(), vec![(old.target.clone(), Ok(Some(old)))]).unwrap();
            assert_eq!(result["updates"][0]["status"], "error");
            assert!(result["updates"][0].get("candidate").is_none());
            assert_eq!(session.extensions.updates.candidates[&id].token, current_token);
        }
    }
}
