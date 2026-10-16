//! Desktop-only transport to an installed plugin's bundled engine. No shell or guest paths.
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use photocraft_doc::Document;
use serde_json::{Value, json};

use crate::{EngineError, Result, jobs::JobCtx};

pub(crate) mod worker_v2;

/// Transport metadata is validated by the shared host service before starting a worker.
pub(crate) struct ImageJob {
    pub options: Value,
    pub expected: (u32, u32),
    pub result_name: String,
    pub label: &'static str,
    pub opening: &'static str,
}

fn error(e: impl std::fmt::Display) -> EngineError {
    EngineError::Other(e.to_string())
}

const DIAGNOSTIC_LINES: usize = 16;
const DIAGNOSTIC_BYTES: usize = 8192;
const DIAGNOSTIC_LINE_BYTES: usize = 2048;
const DIAGNOSTIC_CONTEXT_LINES: usize = 3;
const WORKER_LOG_BYTES: u64 = 16 * 1024 * 1024;
const WORKER_LOG_POLL_BYTES: u64 = 64 * 1024;

/// Preserve the first actual failure even when a backend prints GPU capabilities after it.
/// Keep a little following context and a bounded tail, never an unbounded copy of stderr.
#[derive(Default)]
struct WorkerDiagnostics {
    primary: Option<String>,
    tail: VecDeque<String>,
    tail_bytes: usize,
}

impl WorkerDiagnostics {
    fn record(&mut self, text: &str) {
        let text = text.trim();
        // Progress is machine-readable status, not part of the failure explanation.
        if text.is_empty() {
            return;
        }
        if let Ok(value) = serde_json::from_str::<Value>(text) {
            if let Some(message) = value.get("error").and_then(Value::as_str) {
                self.record(&format!("error: {message}"));
            }
            return;
        }
        let mut end = text.len().min(DIAGNOSTIC_LINE_BYTES);
        while !text.is_char_boundary(end) {
            end = end.saturating_sub(1);
        }
        let line = text.get(..end).unwrap_or_default().to_owned();
        let lower = line.to_ascii_lowercase();
        if self.primary.is_none() && (lower.contains("error") || lower.contains("failed")) {
            self.primary = Some(line);
            // Older diagnostics preceded the primary error. Retain details following it.
            self.tail.clear();
            self.tail_bytes = 0;
            return;
        }
        self.tail_bytes = self.tail_bytes.saturating_add(line.len() + 1);
        self.tail.push_back(line);
        let reserved = self.primary.as_ref().map_or(0, |line| line.len() + 1);
        let max_lines = DIAGNOSTIC_LINES.saturating_sub(usize::from(self.primary.is_some()));
        while self.tail.len() > max_lines || self.tail_bytes > DIAGNOSTIC_BYTES.saturating_sub(reserved) {
            // An OS error directly after the primary line is more useful than a long GPU
            // capability dump. Keep that initial context while rotating the later tail.
            let remove = if self.primary.is_some() { DIAGNOSTIC_CONTEXT_LINES.min(self.tail.len().saturating_sub(1)) } else { 0 };
            let Some(line) = self.tail.remove(remove) else { break };
            self.tail_bytes = self.tail_bytes.saturating_sub(line.len() + 1);
        }
    }

    fn message(&self) -> String {
        if let Some(primary) = &self.primary {
            let mut message = primary.clone();
            for line in &self.tail {
                message.push('\n');
                message.push_str(line);
            }
            message
        } else {
            self.tail.back().cloned().unwrap_or_else(|| "The processing engine exited without an error message".into())
        }
    }
}

/// Read a regular file with an independent cursor instead of waiting for pipe EOF. A native
/// worker can exit while a descendant still owns its stderr handle; no reader thread or
/// unbounded join may keep the image job and installation lease alive in that case.
struct WorkerLog {
    file: std::fs::File,
    position: u64,
    line: Vec<u8>,
    diagnostics: WorkerDiagnostics,
}

impl WorkerLog {
    fn open(path: &Path) -> Result<Self> {
        Ok(Self { file: std::fs::File::open(path).map_err(error)?, position: 0, line: Vec::new(), diagnostics: WorkerDiagnostics::default() })
    }

    fn length(&self) -> Result<u64> {
        let length = self.file.metadata().map_err(error)?.len();
        if length > WORKER_LOG_BYTES {
            return Err(error("Plugin processing stopped because its diagnostic output exceeded 16 MiB"));
        }
        Ok(length)
    }

    fn record_line(&mut self, ctx: &JobCtx) {
        let text = String::from_utf8_lossy(&self.line);
        self.diagnostics.record(&text);
        if let Ok(value) = serde_json::from_str::<Value>(&text)
            && let (Some(progress), Some(message)) = (value.get("progress").and_then(Value::as_f64), value.get("message").and_then(Value::as_str))
            && progress.is_finite()
        {
            ctx.progress(0.1 + 0.85 * (progress as f32 / 100.0).clamp(0.0, 1.0), message);
        }
        self.line.clear();
    }

    fn read_until(&mut self, ctx: &JobCtx, end: u64, budget: u64) -> Result<()> {
        let end = end.min(self.position.saturating_add(budget));
        let mut chunk = [0u8; 2048];
        while self.position < end {
            ctx.check()?;
            let length = end.saturating_sub(self.position).min(chunk.len() as u64) as usize;
            let bytes = chunk.get_mut(..length).ok_or_else(|| error("Invalid plugin diagnostic read length"))?;
            let count = self.file.read(bytes).map_err(error)?;
            if count == 0 {
                break;
            }
            self.position = self.position.saturating_add(count as u64);
            for byte in chunk.iter().take(count) {
                if *byte == b'\n' {
                    self.record_line(ctx);
                } else if self.line.len() < 8192 {
                    self.line.push(*byte);
                }
            }
        }
        Ok(())
    }

    fn poll(&mut self, ctx: &JobCtx) -> Result<()> {
        self.read_until(ctx, self.length()?, WORKER_LOG_POLL_BYTES)
    }

    fn finish(&mut self, ctx: &JobCtx) -> Result<()> {
        // Capture one bounded length. A descendant still writing cannot extend this drain.
        self.read_until(ctx, self.length()?, WORKER_LOG_BYTES)?;
        if !self.line.is_empty() {
            self.record_line(ctx);
        }
        Ok(())
    }
}

/// Runs the package-owned processor using the bounded RGBA8 sRGB image protocol.
pub(crate) fn process_native(doc: &Document, native: &crate::extension_package::InstalledNative, job: &ImageJob, ctx: &JobCtx) -> Result<Document> {
    let _watch = native.watch(ctx)?;
    let lease = native.lease()?.map(|lease| Box::new(lease) as Box<dyn Send>);
    native.verify_payload(ctx)?;
    validate_storage(native)?;
    if let Some(supervisor) = &native.worker_v2 {
        process_with(doc, &native.worker, job, ctx, Some((supervisor, lease)), || validate_storage(native))
    } else {
        let _lease = lease;
        process_with(doc, &native.worker, job, ctx, None, || validate_storage(native))
    }
}

type Persistent<'a> = Option<(&'a std::sync::Arc<worker_v2::Supervisor>, Option<Box<dyn Send>>)>;

fn validate_storage(native: &crate::extension_package::InstalledNative) -> Result<()> {
    let result = native.validate_storage();
    if result.is_err() {
        native.stop();
    }
    result
}

fn process_with(
    doc: &Document,
    worker: &Path,
    job: &ImageJob,
    ctx: &JobCtx,
    persistent: Persistent<'_>,
    completed: impl FnOnce() -> Result<()>,
) -> Result<Document> {
    ctx.check()?;
    let temp = tempfile::Builder::new().prefix("photocraft-plugin-image-").tempdir().map_err(error)?;
    let input = temp.path().join("source.png");
    let output = temp.path().join("result.png");
    let request = temp.path().join("job.json");
    let exported = {
        use photocraft_codecs::{ChannelLayout, Format, Image, SampleType};
        let surface = doc.layers.first().and_then(|layer| layer.surface()).ok_or_else(|| error("The plugin input has no pixel surface"))?;
        if surface.format() != photocraft_color::PixelFormat::RGBA8 || doc.mode != photocraft_color::ColorMode::Rgb || doc.depth.bits() != 8 {
            return Err(error("The plugin input must be an 8-bit RGBA sRGB image"));
        }
        let image = Image::from_raw(doc.size.width, doc.size.height, ChannelLayout::Rgba, SampleType::U8, surface.to_interleaved(doc.bounds()))
            .map_err(error)?
            .with_icc(Some(photocraft_cms::Builtin::Srgb.profile().to_bytes().to_vec()));
        photocraft_codecs::encode(&image, Format::Png, &Default::default()).map_err(error)?
    };
    ctx.check()?;
    std::fs::write(&input, exported).map_err(error)?;
    let payload = json!({"apiVersion":1,"input":input,"output":output,"options":job.options,
        "outputSize":{"width":job.expected.0,"height":job.expected.1},"colorSpace":"srgb"});
    std::fs::write(&request, serde_json::to_vec(&payload).map_err(error)?).map_err(error)?;
    if let Some((supervisor, lease)) = persistent {
        supervisor.run(photocraft_plugin_api::WorkerOperation::Image, payload, lease, ctx)?;
    } else {
        run_worker(worker, "--job", &request, temp.path(), job.label, ctx)?;
    }
    completed()?;
    ctx.progress(0.97, job.opening);
    read_result(doc, &output, job, ctx)
}

/// A task result is data only: it cannot open documents, apply edits or dispatch commands.
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TaskResult {
    api_version: u32,
    pub message: String,
    pub data: Value,
}

const TASK_RESULT_BYTES: u64 = 128 << 10;

pub(crate) fn task_native(native: &crate::extension_package::InstalledNative, options: &Value, ctx: &JobCtx) -> Result<TaskResult> {
    let _watch = native.watch(ctx)?;
    let lease = native.lease()?.map(|lease| Box::new(lease) as Box<dyn Send>);
    native.verify_payload(ctx)?;
    validate_storage(native)?;
    if let Some(supervisor) = &native.worker_v2 {
        task_with(&native.worker, options, ctx, Some((supervisor, lease)), || validate_storage(native))
    } else {
        let _lease = lease;
        task_with(&native.worker, options, ctx, None, || validate_storage(native))
    }
}

fn task_with(worker: &Path, options: &Value, ctx: &JobCtx, persistent: Persistent<'_>, completed: impl FnOnce() -> Result<()>) -> Result<TaskResult> {
    ctx.check()?;
    let temp = tempfile::Builder::new().prefix("photocraft-plugin-task-").tempdir().map_err(error)?;
    let output = temp.path().join("result.json");
    let request = temp.path().join("task.json");
    let payload = json!({"apiVersion":1,"options":options,"output":output});
    std::fs::write(&request, serde_json::to_vec(&payload).map_err(error)?).map_err(error)?;
    if let Some((supervisor, lease)) = persistent {
        supervisor.run(photocraft_plugin_api::WorkerOperation::Task, payload, lease, ctx)?;
    } else {
        run_worker(worker, "--task", &request, temp.path(), "Plugin task", ctx)?;
    }
    completed()?;
    read_task_result(&output, ctx)
}

fn read_task_result(output: &Path, ctx: &JobCtx) -> Result<TaskResult> {
    ctx.check()?;
    let metadata = std::fs::symlink_metadata(output).map_err(error)?;
    if !metadata.file_type().is_file() || metadata.len() > TASK_RESULT_BYTES {
        return Err(error("Native plugin task result must be a regular JSON file of at most 128 KiB"));
    }
    let file = std::fs::File::open(output).map_err(error)?;
    if !file.metadata().map_err(error)?.is_file() {
        return Err(error("Native plugin task result must be a regular JSON file"));
    }
    let mut bytes = Vec::new();
    file.take(TASK_RESULT_BYTES + 1).read_to_end(&mut bytes).map_err(error)?;
    if bytes.len() as u64 > TASK_RESULT_BYTES {
        return Err(error("Native plugin task result grew beyond 128 KiB"));
    }
    let result: TaskResult = serde_json::from_slice(&bytes).map_err(error)?;
    if result.api_version != 1 || !result.data.is_object() || result.message.trim().is_empty() || result.message.len() > 8192 {
        return Err(error("Native plugin task requires apiVersion 1, object data, and a nonempty message of at most 8192 UTF-8 bytes"));
    }
    ctx.check()?;
    Ok(result)
}

fn run_worker(worker: &Path, flag: &str, request: &Path, directory: &Path, label: &str, ctx: &JobCtx) -> Result<()> {
    ctx.check()?;
    let log_path = directory.join("worker.log");
    let log_writer = std::fs::File::create(&log_path).map_err(error)?;
    // File::try_clone shares its cursor, so open a separate read handle before passing the
    // write handle to the child. A regular-file read never waits for another handle to close.
    let mut worker_log = WorkerLog::open(&log_path)?;
    let mut command = Command::new(worker);
    command.arg(flag).arg(request).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(log_writer);
    if let Some(parent) = worker.parent() {
        command.current_dir(parent);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command.spawn().map_err(|e| error(format!("Could not start the {label} plugin's processing engine: {e}")))?;
    drop(command);
    wait_for_worker(&mut child, &mut worker_log, label, ctx)
}

fn wait_for_worker(child: &mut std::process::Child, worker_log: &mut WorkerLog, label: &str, ctx: &JobCtx) -> Result<()> {
    let started = Instant::now();
    let mut stopping = None;
    let mut timed_out = false;
    let status = loop {
        // Keep log work bounded so cancellation and the worker deadline are checked promptly.
        if let Err(e) = worker_log.poll(ctx)
            && !matches!(e, EngineError::Cancelled)
        {
            let _ = child.kill();
            let _ = child.wait();
            break Err(e);
        }
        if stopping.is_none() && (ctx.cancelled() || started.elapsed() > Duration::from_secs(45 * 60)) {
            timed_out = !ctx.cancelled();
            // A cooperative worker stops inference (and any child engine) before exiting.
            // Closing this pipe also cancels it if PhotoCraft exits unexpectedly.
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(b"cancel\n");
            }
            stopping = Some(Instant::now());
        }
        if stopping.is_some_and(|t| t.elapsed() > Duration::from_secs(10)) {
            let _ = child.kill();
        }
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => std::thread::sleep(Duration::from_millis(40)),
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(error(e));
            }
        }
    };
    let log_result = worker_log.finish(ctx);
    ctx.check()?;
    if timed_out {
        return Err(error(format!("{label} worker exceeded the 45-minute time limit")));
    }
    let status = status?;
    log_result?;
    if !status.success() {
        return Err(error(format!("{label} failed ({status}): {}", worker_log.diagnostics.message())));
    }
    Ok(())
}

fn read_result(doc: &Document, output: &Path, job: &ImageJob, ctx: &JobCtx) -> Result<Document> {
    ctx.check()?;
    // Bound compressed output by the expected raw pixels plus finite metadata overhead.
    let max_bytes = (u64::from(job.expected.0) * u64::from(job.expected.1)).saturating_mul(4).saturating_add(64 << 20).min(1024 << 20);
    // Check the PNG header before decoding. A buggy worker cannot allocate an oversized
    // document by returning dimensions different from the requested bounded result.
    let mut file = std::fs::File::open(output).map_err(error)?;
    let metadata = file.metadata().map_err(error)?;
    if !metadata.is_file() || metadata.len() > max_bytes {
        return Err(error(format!("Plugin output exceeds {} MiB", max_bytes / (1024 * 1024))));
    }
    let mut header = [0u8; 26];
    file.read_exact(&mut header).map_err(error)?;
    let width = u32::from_be_bytes([header[16], header[17], header[18], header[19]]);
    let height = u32::from_be_bytes([header[20], header[21], header[22], header[23]]);
    if &header[..8] != b"\x89PNG\r\n\x1a\n" || &header[12..16] != b"IHDR" || (width, height) != job.expected || header[24] != 8 {
        return Err(error(format!("{} returned an invalid PNG or unexpected dimensions/depth", job.label)));
    }
    if !matches!(header[25], 2 | 6) {
        return Err(error("The plugin must return an 8-bit RGB or RGBA PNG"));
    }
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(metadata.len() as usize).map_err(error)?;
    bytes.extend_from_slice(&header);
    file.take(max_bytes + 1).read_to_end(&mut bytes).map_err(error)?;
    if bytes.len() as u64 > max_bytes {
        return Err(error(format!("Plugin output grew beyond {} MiB", max_bytes / (1024 * 1024))));
    }
    ctx.check()?;
    let mut result = photocraft_io::import("plugin-result.png", &bytes).map_err(error)?.document;
    if (result.size.width, result.size.height) != job.expected || result.depth.bits() != 8 {
        return Err(error(format!("{} decoded to unexpected dimensions/depth", job.label)));
    }
    result.name.clone_from(&job.result_name);
    result.resolution_dpi = doc.resolution_dpi;
    ctx.check()?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORKER_FIXTURE: &str = "plugin_runtime::tests::native_worker_diagnostic_fixture";

    #[test]
    fn native_task_result_is_bounded_typed_and_cancellable() {
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("result.json");
        let ctx = JobCtx::new();
        std::fs::write(&output, br#"{"apiVersion":1,"message":"Ready","data":{"count":2}}"#).unwrap();
        let result = read_task_result(&output, &ctx).unwrap();
        assert_eq!(result.message, "Ready");
        assert_eq!(result.data["count"], 2);
        for malformed in [
            json!({"apiVersion":2,"message":"Ready","data":{}}),
            json!({"apiVersion":1,"message":" ","data":{}}),
            json!({"apiVersion":1,"message":"Ready","data":[]}),
            json!({"apiVersion":1,"message":"x".repeat(8193),"data":{}}),
            json!({"apiVersion":1,"message":"Ready","data":{},"commands":[]}),
        ] {
            std::fs::write(&output, malformed.to_string()).unwrap();
            assert!(read_task_result(&output, &ctx).is_err());
        }
        std::fs::write(&output, b"not json").unwrap();
        assert!(read_task_result(&output, &ctx).is_err());
        std::fs::File::create(&output).unwrap().set_len(TASK_RESULT_BYTES + 1).unwrap();
        assert!(read_task_result(&output, &ctx).unwrap_err().to_string().contains("128 KiB"));
        assert!(read_task_result(directory.path(), &ctx).unwrap_err().to_string().contains("regular JSON"));
        ctx.cancel();
        assert!(matches!(read_task_result(&output, &ctx), Err(EngineError::Cancelled)));
    }

    struct FixtureChild(std::process::Child);

    impl Drop for FixtureChild {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn diagnostic_fixture(path: &Path, mode: &str) -> FixtureChild {
        let writer = std::fs::OpenOptions::new().append(true).open(path).unwrap();
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", WORKER_FIXTURE, "--ignored", "--nocapture"])
            .env("PHOTOCRAFT_DIAGNOSTIC_FIXTURE", mode)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(writer);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        FixtureChild(command.spawn().unwrap())
    }

    #[test]
    #[ignore = "subprocess fixture used by the diagnostic transport tests"]
    fn native_worker_diagnostic_fixture() {
        match std::env::var("PHOTOCRAFT_DIAGNOSTIC_FIXTURE").unwrap_or_default().as_str() {
            "hold" => {
                // This process retains an inherited stderr handle until the owner stops it.
                let mut input = Vec::new();
                std::io::stdin().read_to_end(&mut input).unwrap();
            }
            "fail" => {
                eprintln!("processing ERROR: fixture failure");
                std::process::exit(7);
            }
            _ => eprintln!(r#"{{"progress":100,"message":"Fixture complete"}}"#),
        }
    }

    #[test]
    fn native_log_drain_finishes_while_another_process_retains_the_inherited_handle() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("worker.log");
        std::fs::write(&path, b"").unwrap();
        // A descendant retaining stderr after its worker exits has this same handle-lifetime
        // behavior. Keep both process handles here so the test always reaps its fixtures.
        let mut holder = diagnostic_fixture(&path, "hold");
        let mut worker = diagnostic_fixture(&path, "success");
        let mut log = WorkerLog::open(&path).unwrap();
        let ctx = JobCtx::new();
        wait_for_worker(&mut worker.0, &mut log, "Fixture", &ctx).unwrap();
        assert!(holder.0.try_wait().unwrap().is_none());
        assert_eq!(ctx.message(), "Fixture complete");
        assert!((ctx.fraction() - 0.95).abs() < 0.001);
        // A second final drain is bounded as well; it does not require a closed writer.
        log.finish(&ctx).unwrap();
        drop(holder);
    }

    #[test]
    fn native_log_reads_are_bounded_and_bursts_retain_primary_diagnostics() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("worker.log");
        let mut writer = std::fs::File::create(&path).unwrap();
        writeln!(writer, r#"{{"progress":50,"message":"Halfway"}}"#).unwrap();
        writeln!(writer, "inference ERROR: primary fixture failure").unwrap();
        writeln!(writer, "could not open the model").unwrap();
        for index in 0..1024 {
            writeln!(writer, "GPU {index}: {}", "details ".repeat(32)).unwrap();
        }
        writeln!(writer, r#"{{"progress":90,"message":"Finishing"}}"#).unwrap();
        writer.write_all(b"last line without newline").unwrap();
        let mut log = WorkerLog::open(&path).unwrap();
        let ctx = JobCtx::new();
        log.poll(&ctx).unwrap();
        assert_eq!(log.position, WORKER_LOG_POLL_BYTES);
        assert_eq!(ctx.message(), "Halfway");
        log.finish(&ctx).unwrap();
        let message = log.diagnostics.message();
        assert!(message.starts_with("inference ERROR: primary fixture failure"));
        assert!(message.contains("could not open the model"));
        assert!(message.contains("GPU 1023"));
        assert!(message.contains("last line without newline"));
        assert!(message.len() <= DIAGNOSTIC_BYTES);
        assert!(message.lines().count() <= DIAGNOSTIC_LINES);
        assert_eq!(ctx.message(), "Finishing");
        assert!((ctx.fraction() - 0.865).abs() < 0.001);
    }

    #[test]
    fn excessive_native_diagnostic_output_fails_and_reaps_the_worker() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("worker.log");
        let writer = std::fs::File::create(&path).unwrap();
        writer.set_len(WORKER_LOG_BYTES + 1).unwrap();
        let mut worker = diagnostic_fixture(&path, "hold");
        let mut log = WorkerLog::open(&path).unwrap();
        let error = wait_for_worker(&mut worker.0, &mut log, "Fixture", &JobCtx::new()).unwrap_err().to_string();
        assert!(error.contains("diagnostic output exceeded 16 MiB"), "{error}");
        assert_eq!(log.position, 0);
        assert!(worker.0.try_wait().unwrap().is_some());
    }

    #[test]
    fn native_log_transport_preserves_failure_and_cooperative_cancellation() {
        for mode in ["fail", "hold"] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("worker.log");
            std::fs::write(&path, b"").unwrap();
            let mut worker = diagnostic_fixture(&path, mode);
            let mut log = WorkerLog::open(&path).unwrap();
            let ctx = JobCtx::new();
            if mode == "hold" {
                ctx.cancel();
            }
            let result = wait_for_worker(&mut worker.0, &mut log, "Fixture", &ctx);
            assert!(worker.0.try_wait().unwrap().is_some());
            if mode == "hold" {
                assert!(matches!(result, Err(EngineError::Cancelled)));
            } else {
                assert!(result.unwrap_err().to_string().contains("processing ERROR: fixture failure"));
            }
        }
    }

    #[test]
    fn native_failure_keeps_its_primary_error_before_gpu_capability_details() {
        let mut diagnostics = WorkerDiagnostics::default();
        diagnostics.record("[0 GPU] driver=1 compute=1");
        diagnostics.record(r#"{"progress":50,"message":"Reconstructing image"}"#);
        diagnostics.record("Neural inference FAILED (exit code: -9)");
        diagnostics.record("could not open model.param");
        for i in 0..32 {
            diagnostics.record(&format!("[{i} GPU] subgroup=64 basic=1 vote=1 ballot=1 shuffle=1"));
        }
        diagnostics.record(r#"{"progress":100,"message":"Finished"}"#);
        let message = diagnostics.message();
        assert!(message.starts_with("Neural inference FAILED (exit code: -9)"));
        assert!(message.contains("could not open model.param"));
        assert!(message.contains("[31 GPU]"));
        assert!(!message.contains("Reconstructing image"));
        assert!(!message.contains("Finished"));
        assert!(message.lines().count() <= DIAGNOSTIC_LINES);
        assert!(message.len() <= DIAGNOSTIC_BYTES);
    }

    #[test]
    fn structured_native_errors_remain_visible_and_bounded() {
        let mut diagnostics = WorkerDiagnostics::default();
        diagnostics.record(r#"{"progress":10,"message":"Preparing"}"#);
        diagnostics.record(r#"{"error":"The bundled model could not be opened"}"#);
        diagnostics.record(r#"{"progress":100,"message":"Finished"}"#);
        assert_eq!(diagnostics.message(), "error: The bundled model could not be opened");
        let mut oversized = WorkerDiagnostics::default();
        oversized.record(&json!({"error":"x".repeat(DIAGNOSTIC_BYTES * 2)}).to_string());
        assert!(oversized.message().starts_with("error: "));
        assert!(oversized.message().len() <= DIAGNOSTIC_BYTES);
    }

    #[test]
    fn native_diagnostic_bounds_preserve_utf8_and_fall_back_to_the_last_text() {
        let mut diagnostics = WorkerDiagnostics::default();
        diagnostics.record("initial backend message");
        diagnostics.record("last backend message");
        assert_eq!(diagnostics.message(), "last backend message");
        diagnostics.record(&format!("error: {}", "🦀é".repeat(4096)));
        for _ in 0..100 {
            diagnostics.record(&"模型🦀".repeat(4096));
        }
        let message = diagnostics.message();
        assert!(message.starts_with("error: "));
        assert!(message.len() <= DIAGNOSTIC_BYTES);
        assert!(message.lines().count() <= DIAGNOSTIC_LINES);
        assert_eq!(String::from_utf8(message.as_bytes().to_vec()).unwrap(), message);
        assert!(diagnostics.tail.iter().all(|line| line.len() <= DIAGNOSTIC_LINE_BYTES));
    }

    #[test]
    fn native_result_must_match_bounded_dimensions_and_eight_bit_png() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("result.png");
        let mut source = Document::new("Source", photocraft_doc::Size::new(8, 6), photocraft_color::ColorMode::Rgb, photocraft_color::SampleType::U8);
        source.resolution_dpi = 144.0;
        let job = ImageJob { options: json!({}), expected: (8, 6), result_name: "Processed copy".into(), label: "Image processing", opening: "Opening copy" };
        let valid = photocraft_io::export(&source, "png", &Default::default()).unwrap().bytes;
        std::fs::write(&path, &valid).unwrap();
        let result = read_result(&source, &path, &job, &JobCtx::new()).unwrap();
        assert_eq!(result.name, "Processed copy");
        assert_eq!(result.resolution_dpi, 144.0);
        assert_eq!(result.size, source.size);
        assert_eq!(result.depth.bits(), 8);
        for (index, replacement) in [(0, b'X'), (16, 127), (20, 127), (24, 16)] {
            let mut broken = valid.clone();
            broken[index] = replacement;
            std::fs::write(&path, broken).unwrap();
            let error = read_result(&source, &path, &job, &JobCtx::new()).err().unwrap().to_string();
            assert!(error.contains("unexpected dimensions/depth"), "{error}");
        }
        std::fs::write(&path, b"truncated").unwrap();
        assert!(read_result(&source, &path, &job, &JobCtx::new()).is_err());
        std::fs::write(&path, valid).unwrap();
        let ctx = JobCtx::new();
        ctx.cancel();
        assert!(matches!(read_result(&source, &path, &job, &ctx), Err(EngineError::Cancelled)));
    }

    #[test]
    fn generic_native_result_rejects_gray_palette_and_oversized_files_before_decoding() {
        use photocraft_codecs::{ChannelLayout, Format, Image, SampleType};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("result.png");
        let source = Document::new("Source", photocraft_doc::Size::new(8, 6), photocraft_color::ColorMode::Rgb, photocraft_color::SampleType::U8);
        let job = ImageJob { options: json!({}), expected: (8, 6), result_name: "Processed".into(), label: "Image processing", opening: "Opening copy" };
        let gray = Image::from_raw(8, 6, ChannelLayout::Gray, SampleType::U8, vec![128; 48]).unwrap();
        let bytes = photocraft_codecs::encode(&gray, Format::Png, &Default::default()).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        let message = read_result(&source, &path, &job, &JobCtx::new()).unwrap_err().to_string();
        assert!(message.contains("RGB or RGBA"), "{message}");
        let mut palette = bytes;
        palette[25] = 3;
        std::fs::write(&path, palette).unwrap();
        assert!(read_result(&source, &path, &job, &JobCtx::new()).unwrap_err().to_string().contains("RGB or RGBA"));
        std::fs::File::create(&path).unwrap().set_len((512 << 20) + 1).unwrap();
        assert!(read_result(&source, &path, &job, &JobCtx::new()).unwrap_err().to_string().contains("exceeds 64 MiB"));
    }

    #[test]
    fn persistent_frames_refuse_unbounded_lengths_truncation_unknown_shapes_and_stale_ids() {
        use std::io::Cursor;
        for bytes in [Vec::new(), vec![1, 0], 0u32.to_le_bytes().to_vec(), u32::MAX.to_le_bytes().to_vec(), vec![16, 0, 0, 0, b'{', b'}']] {
            assert!(worker_v2::read_frame(&mut Cursor::new(bytes)).is_err());
        }
        for value in [json!({"type":"result","id":"0"}), json!({"type":"result","id":"1","commands":[]}), json!({"type":"unknown","id":"1"})] {
            let body = serde_json::to_vec(&value).unwrap();
            let mut bytes = (body.len() as u32).to_le_bytes().to_vec();
            bytes.extend(body);
            assert!(worker_v2::read_frame(&mut Cursor::new(bytes)).is_err());
        }
        let body = br#"{"type":"result","id":"1"}"#;
        let mut bytes = (body.len() as u32).to_le_bytes().to_vec();
        bytes.extend(body);
        assert_eq!(worker_v2::read_frame(&mut Cursor::new(bytes)).unwrap().id(), "1");
    }

    /// A transient std-only executable exercises the production --serve pipes exactly. A
    /// libtest child would print a harness preamble and invalidate strict binary framing.
    const PERSISTENT_FIXTURE: &str = r#"
use std::io::{Read,Write};
use std::time::Duration;
fn receive()->Option<String>{let mut header=[0u8;4];std::io::stdin().read_exact(&mut header).ok()?;let length=u32::from_le_bytes(header)as usize;if length>262144{return None}let mut bytes=vec![0;length];std::io::stdin().read_exact(&mut bytes).ok()?;String::from_utf8(bytes).ok()}
fn send(text:&str){let mut output=std::io::stdout().lock();output.write_all(&(text.len()as u32).to_le_bytes()).unwrap();output.write_all(text.as_bytes()).unwrap();output.flush().unwrap();}
fn string(text:&str,key:&str)->String{let needle=format!("\"{}\":",key);let Some(start)=text.find(&needle)else{return String::new()};let mut bytes=text[start+needle.len()..].trim_start().chars();if bytes.next()!=Some('"'){return String::new()}let mut result=String::new();while let Some(c)=bytes.next(){match c{'"'=>break,'\\'=>{if let Some(c)=bytes.next(){result.push(match c{'n'=>'\n','r'=>'\r','t'=>'\t',other=>other});}},other=>result.push(other)}}result}
fn main(){
 if std::env::args().any(|arg|arg=="--hold-stdio"){std::thread::sleep(Duration::from_millis(600));return}
 assert!(std::env::args().any(|arg|arg=="--serve"));
 let executable=std::env::current_exe().unwrap();let mode=executable.file_stem().unwrap().to_string_lossy();
 let hello=receive().unwrap();let plugin=string(&hello,"pluginId");let generation=string(&hello,"generation");
 let version=if mode.contains("badversion"){3}else{2};
 let owner=if mode.contains("badowner"){format!("{}.other",plugin)}else{plugin};
 if mode.contains("huge"){std::io::stdout().write_all(&u32::MAX.to_le_bytes()).unwrap();return}
 if mode.contains("truncated"){std::io::stdout().write_all(&20u32.to_le_bytes()).unwrap();std::io::stdout().write_all(b"{}").unwrap();return}
 if mode.contains("utf8"){std::io::stdout().write_all(&1u32.to_le_bytes()).unwrap();std::io::stdout().write_all(&[255]).unwrap();return}
 if mode.contains("handshakehold"){std::thread::sleep(Duration::from_secs(20));return}
 send(&format!("{{\"type\":\"ready\",\"id\":\"0\",\"protocolVersion\":{},\"pluginId\":{:?},\"generation\":{:?},\"operations\":[\"image\",\"task\"]}}",version,owner,generation));
 let mut runs=0;
 while let Some(request)=receive(){let kind=string(&request,"type");let id=string(&request,"id");
  match kind.as_str(){
   "shutdown"=>return,
   "reset"=>{runs=0;send(&format!("{{\"type\":\"resetComplete\",\"id\":{:?}}}",id));},
   "run"=>{
    if mode.contains("crash"){std::process::exit(7)}
    if mode.contains("wrongid"){send("{\"type\":\"result\",\"id\":\"999\"}");continue}
    if mode.contains("cancel"){send(&format!("{{\"type\":\"progress\",\"id\":{:?},\"fraction\":0.1,\"message\":\"Waiting for cancellation\"}}",id));while let Some(next)=receive(){if string(&next,"type")=="cancel"{send(&format!("{{\"type\":\"cancelled\",\"id\":{:?}}}",id));break}}continue}
    if mode.contains("ignore"){std::thread::sleep(Duration::from_secs(20));return}
    if mode.contains("featureerror"){send(&format!("{{\"type\":\"error\",\"id\":{:?},\"error\":{{\"code\":\"conflict\",\"message\":\"Fixture feature failure\",\"details\":{{}}}}}}",id));continue}
    if mode.contains("flood"){for _ in 0..5000{send(&format!("{{\"type\":\"progress\",\"id\":{:?},\"fraction\":0.5,\"message\":\"flood\"}}",id));}}
    if mode.contains("retain"){std::process::Command::new(&executable).arg("--hold-stdio").stdin(std::process::Stdio::inherit()).stdout(std::process::Stdio::inherit()).stderr(std::process::Stdio::inherit()).spawn().unwrap();}
    runs+=1;let output=string(&request,"output");
    if string(&request,"operation")=="image"{std::fs::copy(string(&request,"input"),&output).unwrap();}
    else{std::fs::write(&output,format!("{{\"apiVersion\":1,\"message\":\"Ready\",\"data\":{{\"pid\":{},\"runs\":{},\"storage\":{}}}}}",std::process::id(),runs,request.contains("managedStorage"))).unwrap();}
    send(&format!("{{\"type\":\"progress\",\"id\":{:?},\"fraction\":0.5,\"message\":\"Fixture progress\"}}",id));
    send(&format!("{{\"type\":\"result\",\"id\":{:?}}}",id));
    if mode.contains("retain"){return}
   },
   _=>std::process::exit(9),
  }
 }
}
"#;

    struct ResidentCounter(std::sync::Arc<std::sync::atomic::AtomicUsize>);
    impl Drop for ResidentCounter {
        fn drop(&mut self) {
            self.0.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
        }
    }

    struct GenerationCounter(std::sync::Arc<std::sync::atomic::AtomicUsize>);
    impl Drop for GenerationCounter {
        fn drop(&mut self) {
            self.0.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        }
    }

    fn persistent_supervisor(
        path: &Path,
        owner: &str,
        resident: &std::sync::Arc<std::sync::atomic::AtomicUsize>,
        timeout: u64,
    ) -> std::sync::Arc<worker_v2::Supervisor> {
        let descriptor = photocraft_plugin_api::WorkerV2Descriptor::parse(
            &serde_json::to_vec(&json!({"protocolVersion":2,"executionProfile":"trustedNative",
            "operations":["image","task"],"idleTimeoutMs":1000,"requestTimeoutMs":timeout,
            "resources":{"cpuUnits":1,"memoryBytes":1048576,"gpuBytes":0,"tempBytes":1048576,"cacheBytes":0}}))
            .unwrap(),
        )
        .unwrap();
        let count = std::sync::Arc::clone(resident);
        std::sync::Arc::new(
            worker_v2::Supervisor::new(path.into(), owner.into(), "fixture-generation".into(), descriptor)
                .unwrap()
                .with_resident_admission(std::sync::Arc::new(move |_, _| {
                    count.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                    Ok(Box::new(ResidentCounter(std::sync::Arc::clone(&count))) as Box<dyn Send>)
                }))
                .with_managed_storage(json!({"schemaVersion":1,"data":"private-fixture-data"}))
                .unwrap(),
        )
    }

    fn wait_persistent(predicate: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(4);
        while !predicate() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(predicate(), "persistent runtime did not release its process/lease resources");
    }

    fn persistent_package(worker: &[u8], version: &str) -> Vec<u8> {
        let manifest = json!({"apiVersion":1,"id":"org.example.lifecycle","name":"Worker lifecycle","version":version,
            "capabilities":{"required":[{"id":"host.commands.invoke","minVersion":1}]},
            "permissions":{"commands":["plugin.runTask","plugin.processImage"]},"panel":{"buttons":[{"id":"run","label":"Run"}]}})
        .to_string();
        let escaped = manifest.bytes().map(|byte| format!("\\{byte:02x}")).collect::<String>();
        let module = wat::parse_str(format!(
            r#"(module (memory (export "memory") 2)
            (data (i32.const 16) "{escaped}") (data (i32.const 2048) "{{}}")
            (func (export "pce_api_version") (result i32) i32.const 1)
            (func (export "pce_manifest") (result i64) i64.const {})
            (func (export "pce_alloc") (param i32) (result i32) i32.const 4096)
            (func (export "pce_handle") (param i32 i32) (result i64) i64.const {}))"#,
            ((manifest.len() as u64) << 32) | 16,
            (2u64 << 32) | 2048
        ))
        .unwrap();
        let entry = if cfg!(windows) { "bin/worker-normal.exe" } else { "bin/worker-normal" };
        let descriptor = serde_json::to_vec(&json!({"packageVersion":2,"module":"plugin.pcext","native":{
            "service":"plugin.processImage","platform":crate::extension_package::host_platform(),"entry":entry}}))
        .unwrap();
        let transport = serde_json::to_vec(&json!({"protocolVersion":2,"executionProfile":"trustedNative","operations":["image","task"],
            "idleTimeoutMs":1000,"requestTimeoutMs":1000,"resources":{"cpuUnits":1,"memoryBytes":1048576,"gpuBytes":0,"tempBytes":1048576,"cacheBytes":1048576}})).unwrap();
        let mut archive = photocraft_format::zip::ZipWriter::new();
        for (name, bytes) in [
            ("package.json", descriptor.as_slice()),
            ("plugin.pcext", module.as_slice()),
            (entry, worker),
            ("runtime/worker-v2.json", transport.as_slice()),
            ("LICENSE", b"Neutral fixture".as_slice()),
            ("THIRD-PARTY-NOTICES.md", b"No external content".as_slice()),
        ] {
            archive.add(name, bytes).unwrap();
        }
        if version == "2" {
            archive
                .add(
                    "runtime/managed-v1.json",
                    br#"{"formatVersion":1,"fromSchema":0,"toSchema":1,"settings":[{"op":"setDefault","key":"release","value":2}]}"#,
                )
                .unwrap();
        }
        archive.finish().unwrap()
    }

    fn persistent_host_lifecycle(directory: &Path, binary: &Path) {
        use crate::Session;
        const ID: &str = "org.example.lifecycle";
        let folder = directory.join("Extensions");
        let archive = directory.join("lifecycle.pcplugin");
        let executable = std::fs::read(binary).unwrap();
        std::fs::write(&archive, persistent_package(&executable, "1")).unwrap();
        let mut session = Session::new();
        session.extensions.attach_folder(folder.clone()).unwrap();
        session.execute("extension.install", json!({"path":archive,"allowNative":true})).unwrap();
        assert_eq!(worker_v2::process_count(), 0, "installation must not launch native code");
        drop(session);
        let mut session = Session::new();
        session.extensions.attach_folder(folder.clone()).unwrap();
        assert!(session.extensions.installed[ID].deferred.is_some(), "restart must discover without activation");
        let first = session.execute("plugin.runTask", json!({"id":ID})).unwrap();
        let second = session.execute("plugin.runTask", json!({"id":ID})).unwrap();
        assert_eq!(first["data"]["storage"], true);
        assert_eq!(first["data"]["pid"], second["data"]["pid"]);
        assert_eq!(second["data"]["runs"], 2);
        let mut other_session = Session::new();
        other_session.extensions.attach_folder(folder.clone()).unwrap();
        crate::extension_cmds::activate(&mut other_session, ID).unwrap();
        assert_ne!(
            session.extensions.installed[ID].native.as_ref().unwrap().scheduler_owner(),
            other_session.extensions.installed[ID].native.as_ref().unwrap().scheduler_owner(),
            "independent supervisors in the same store cannot borrow one another's resident process credits"
        );
        drop(other_session);
        session.execute("extension.runtime", json!({"id":ID,"action":"reset"})).unwrap();
        assert_eq!(session.execute("plugin.runTask", json!({"id":ID})).unwrap()["data"]["runs"], 1);
        let original = session.extensions.installed[ID].native.as_ref().unwrap().clone();
        let hash = original.worker.parent().unwrap().parent().unwrap().file_name().unwrap();
        let managed = folder.join("managed").join(ID).join(hash);
        std::fs::write(managed.join("settings.json"), br#"{"note":"retained"}"#).unwrap();
        assert!(session.execute("extension.storage", json!({"id":ID})).unwrap()["settingsBytes"].as_u64().unwrap() > 2);
        session.execute("extension.enable", json!({"id":ID,"enabled":false})).unwrap();
        assert!(original.lease().is_err());
        assert!(session.execute("plugin.runTask", json!({"id":ID})).is_err());
        wait_persistent(|| worker_v2::process_count() == 0);
        session.execute("extension.enable", json!({"id":ID,"enabled":true})).unwrap();
        let refreshed = session.execute("plugin.runTask", json!({"id":ID})).unwrap();
        assert_ne!(first["data"]["pid"], refreshed["data"]["pid"]);
        assert_eq!(refreshed["data"]["runs"], 1);
        let native = session.extensions.installed[ID].native.as_ref().unwrap().clone();
        let cache = managed.join("cache").join("oversized.bin");
        std::fs::File::create(&cache).unwrap().set_len((1 << 20) + 1).unwrap();
        assert!(session.execute("plugin.runTask", json!({"id":ID})).is_err(), "a subsequent job must revalidate durable state before running");
        assert_eq!(native.worker_v2.as_ref().unwrap().status()["state"], "revoked");
        std::fs::remove_file(cache).unwrap();
        session.execute("extension.enable", json!({"id":ID,"enabled":false})).unwrap();
        wait_persistent(|| worker_v2::process_count() == 0);
        std::fs::write(&archive, persistent_package(&executable, "2")).unwrap();
        session.execute("extension.install", json!({"path":archive,"replace":true,"allowNative":true})).unwrap();
        let next = session.extensions.installed[ID].native.as_ref().unwrap();
        let next_hash = next.worker.parent().unwrap().parent().unwrap().file_name().unwrap();
        let next_settings: Value =
            serde_json::from_slice(&std::fs::read(folder.join("managed").join(ID).join(next_hash).join("settings.json")).unwrap()).unwrap();
        assert_eq!(next_settings, json!({"note":"retained","release":2}));
        session.execute("extension.rollback", json!({"id":ID})).unwrap();
        assert_eq!(session.extensions.installed[ID].extension.manifest.version, "1");
        assert_eq!(serde_json::from_slice::<Value>(&std::fs::read(managed.join("settings.json")).unwrap()).unwrap(), json!({"note":"retained"}));
        session.execute("extension.enable", json!({"id":ID,"enabled":true})).unwrap();
        assert_eq!(session.execute("plugin.runTask", json!({"id":ID})).unwrap()["data"]["runs"], 1);
        session.execute("extension.runtime", json!({"id":ID,"action":"retry"})).unwrap();
        wait_persistent(|| worker_v2::process_count() == 0);
        assert_eq!(session.execute("plugin.runTask", json!({"id":ID})).unwrap()["data"]["runs"], 1);
        session.execute("extension.enable", json!({"id":ID,"enabled":false})).unwrap();
        wait_persistent(|| worker_v2::process_count() == 0);
    }

    #[test]
    fn persistent_native_workers_reuse_reset_cancel_evict_isolate_and_quarantine_real_processes() {
        use photocraft_plugin_api::WorkerOperation;
        use std::sync::{
            Arc,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        };
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("worker.rs");
        std::fs::write(&source, PERSISTENT_FIXTURE).unwrap();
        let binary = directory.path().join(if cfg!(windows) { "worker-normal.exe" } else { "worker-normal" });
        let compiler = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
        let built = Command::new(compiler).args(["--edition=2024", "-C", "debuginfo=0"]).arg(&source).arg("-o").arg(&binary).output().unwrap();
        assert!(built.status.success(), "standalone fixture compiler: {}", String::from_utf8_lossy(&built.stderr));
        let executable = |mode: &str| {
            let path = directory.path().join(if cfg!(windows) { format!("worker-{mode}.exe") } else { format!("worker-{mode}") });
            if path != binary {
                std::fs::copy(&binary, &path).unwrap();
            }
            path
        };
        let resident = Arc::new(AtomicUsize::new(0));
        let released = Arc::new(AtomicUsize::new(0));
        let supervisor = persistent_supervisor(&binary, "org.example.worker", &resident, 500);
        let output = directory.path().join("task-result.json");
        let payload = json!({"apiVersion":1,"options":{},"output":output});
        supervisor.run(WorkerOperation::Task, payload.clone(), Some(Box::new(GenerationCounter(Arc::clone(&released)))), &JobCtx::new()).unwrap();
        let first = read_task_result(&output, &JobCtx::new()).unwrap();
        assert_eq!(first.data["runs"], 1);
        assert_eq!(first.data["storage"], true);
        supervisor.run(WorkerOperation::Task, payload.clone(), None, &JobCtx::new()).unwrap();
        let second = read_task_result(&output, &JobCtx::new()).unwrap();
        assert_eq!(first.data["pid"], second.data["pid"]);
        assert_eq!(second.data["runs"], 2);
        assert_eq!(resident.load(Ordering::Acquire), 1);
        assert_eq!(released.load(Ordering::Acquire), 0);
        let image = Document::with_background(
            "Original",
            photocraft_doc::Size::new(8, 6),
            photocraft_color::ColorMode::Rgb,
            photocraft_color::SampleType::U8,
            photocraft_color::Color::WHITE,
        );
        let image_job = ImageJob { options: json!({}), expected: (8, 6), result_name: "Processed".into(), label: "Image fixture", opening: "Opening fixture" };
        let processed = process_with(&image, &binary, &image_job, &JobCtx::new(), Some((&supervisor, None)), || Ok(())).unwrap();
        assert_eq!(processed.size, image.size);
        assert_eq!(processed.name, "Processed");
        assert_eq!(
            processed.layers[0].surface().unwrap().to_interleaved(processed.bounds()),
            image.layers[0].surface().unwrap().to_interleaved(image.bounds())
        );
        supervisor.reset(&JobCtx::new()).unwrap();
        supervisor.run(WorkerOperation::Task, payload.clone(), None, &JobCtx::new()).unwrap();
        assert_eq!(read_task_result(&output, &JobCtx::new()).unwrap().data["runs"], 1);
        let other = persistent_supervisor(&binary, "org.example.other", &resident, 500);
        other.run(WorkerOperation::Task, payload.clone(), None, &JobCtx::new()).unwrap();
        assert_ne!(first.data["pid"], read_task_result(&output, &JobCtx::new()).unwrap().data["pid"]);
        other.stop();
        supervisor.stop();
        wait_persistent(|| resident.load(Ordering::Acquire) == 0 && released.load(Ordering::Acquire) == 1);

        let idle = persistent_supervisor(&binary, "org.example.idle", &resident, 500);
        idle.run(WorkerOperation::Task, payload.clone(), None, &JobCtx::new()).unwrap();
        wait_persistent(|| idle.status()["state"] == "inactive" && resident.load(Ordering::Acquire) == 0);

        let cancelled = persistent_supervisor(&executable("cancel"), "org.example.cancel", &resident, 1000);
        let ctx = JobCtx::new();
        let cancel_ctx = ctx.clone();
        let cancellation = std::thread::spawn(move || {
            wait_persistent(|| cancel_ctx.message() == "Waiting for cancellation");
            cancel_ctx.cancel();
        });
        assert!(matches!(cancelled.run(WorkerOperation::Task, payload.clone(), None, &ctx), Err(EngineError::Cancelled)));
        cancellation.join().unwrap();
        assert_eq!(cancelled.status()["consecutiveFailures"], 0);
        assert_eq!(cancelled.status()["state"], "idle");
        cancelled.reset(&JobCtx::new()).unwrap();
        cancelled.stop();
        wait_persistent(|| resident.load(Ordering::Acquire) == 0);

        let retained = persistent_supervisor(&executable("retain"), "org.example.descendant", &resident, 500);
        let retained_lease = Arc::new(AtomicUsize::new(0));
        retained.run(WorkerOperation::Task, payload.clone(), Some(Box::new(GenerationCounter(Arc::clone(&retained_lease)))), &JobCtx::new()).unwrap();
        let started = Instant::now();
        retained.stop();
        assert!(started.elapsed() < Duration::from_millis(100));
        assert_eq!(retained_lease.load(Ordering::Acquire), 0, "a descendant still owns the protocol pipe");
        wait_persistent(|| resident.load(Ordering::Acquire) == 0 && retained_lease.load(Ordering::Acquire) == 1);

        let ignored = persistent_supervisor(&executable("ignore"), "org.example.timeout", &resident, 100);
        let started = Instant::now();
        let failure = ignored.run(WorkerOperation::Task, payload.clone(), None, &JobCtx::new()).unwrap_err();
        assert!(failure.to_string().contains("timed out"), "{failure}");
        assert!(started.elapsed() < Duration::from_secs(4));
        wait_persistent(|| resident.load(Ordering::Acquire) == 0);

        for mode in ["badversion", "badowner", "huge", "truncated", "utf8", "wrongid", "crash", "flood"] {
            let broken = persistent_supervisor(&executable(mode), "org.example.invalid", &resident, 500);
            assert!(broken.run(WorkerOperation::Task, payload.clone(), None, &JobCtx::new()).is_err(), "{mode}");
            broken.stop();
            wait_persistent(|| resident.load(Ordering::Acquire) == 0);
        }

        let broken = persistent_supervisor(&executable("badversion"), "org.example.quarantine", &resident, 500);
        for _ in 0..3 {
            assert!(broken.run(WorkerOperation::Task, payload.clone(), None, &JobCtx::new()).is_err());
        }
        assert_eq!(broken.status()["state"], "quarantined");
        assert!(broken.run(WorkerOperation::Task, payload.clone(), None, &JobCtx::new()).unwrap_err().to_string().contains("quarantined"));
        broken.retry().unwrap();
        assert_eq!(broken.status()["consecutiveFailures"], 0);
        broken.stop();
        wait_persistent(|| resident.load(Ordering::Acquire) == 0);

        let feature = persistent_supervisor(&executable("featureerror"), "org.example.feature", &resident, 500);
        for _ in 0..4 {
            assert!(matches!(feature.run(WorkerOperation::Task, payload.clone(), None, &JobCtx::new()), Err(EngineError::PluginApi(_))));
        }
        assert_eq!(feature.status()["consecutiveFailures"], 0);
        feature.stop();
        wait_persistent(|| resident.load(Ordering::Acquire) == 0);

        let live = Arc::new(AtomicBool::new(true));
        let health = Arc::clone(&live);
        let revoked = Arc::new(
            persistent_supervisor(&binary, "org.example.revoked", &resident, 500).fresh().with_liveness(Arc::new(move || health.load(Ordering::Acquire))),
        );
        revoked.run(WorkerOperation::Task, payload.clone(), None, &JobCtx::new()).unwrap();
        live.store(false, Ordering::Release);
        wait_persistent(|| revoked.status()["state"] == "revoked" && resident.load(Ordering::Acquire) == 0);
        assert!(matches!(revoked.run(WorkerOperation::Task, payload.clone(), None, &JobCtx::new()), Err(EngineError::Cancelled)));

        let live = Arc::new(AtomicBool::new(true));
        let health = Arc::clone(&live);
        let startup = Arc::new(
            persistent_supervisor(&executable("handshakehold"), "org.example.startup", &resident, 500)
                .fresh()
                .with_liveness(Arc::new(move || health.load(Ordering::Acquire))),
        );
        let admission = Arc::clone(&resident);
        let revocation = std::thread::spawn(move || {
            wait_persistent(|| admission.load(Ordering::Acquire) != 0);
            live.store(false, Ordering::Release);
        });
        let started = Instant::now();
        assert!(matches!(startup.run(WorkerOperation::Task, payload, None, &JobCtx::new()), Err(EngineError::Cancelled)));
        revocation.join().unwrap();
        assert!(started.elapsed() < Duration::from_secs(4), "startup revocation must not wait for the ten-second handshake deadline");
        wait_persistent(|| resident.load(Ordering::Acquire) == 0);
        wait_persistent(|| worker_v2::process_count() == 0);
        persistent_host_lifecycle(directory.path(), &binary);
    }
}
