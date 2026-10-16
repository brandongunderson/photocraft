//! Package-owned image processing. Panels can use their own native engine or sandboxed
//! filters without adding command implementations or process paths to PhotoCraft.
use std::sync::Arc;

use photocraft_cms::{Builtin, Intent, Transform};
use photocraft_color::{ColorMode, PixelFormat, SampleType};
use photocraft_doc::{Document, Layer};
use photocraft_plugins::Plugin;
use photocraft_raster::Surface;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::extension_cmds::Completion;
use crate::{EngineError, Result, Session, commands::CommandSpec, jobs::JobCtx};

const MAX_OPTIONS_BYTES: usize = 128 << 10;

fn error(message: impl std::fmt::Display) -> EngineError {
    EngineError::Other(message.to_string())
}

fn object() -> Value {
    json!({})
}

fn yes() -> bool {
    true
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Params {
    id: Option<String>,
    processor: Option<String>,
    #[serde(default = "object")]
    options: Value,
    width: Option<u32>,
    height: Option<u32>,
    #[serde(default = "yes")]
    preview: bool,
}

/// Count the JSON representation without allocating an attacker-sized serialization buffer.
struct OptionsBudget(usize);

impl std::io::Write for OptionsBudget {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self.0.checked_add(bytes.len()).filter(|n| *n <= MAX_OPTIONS_BYTES).ok_or_else(|| std::io::Error::other("options exceed 128 KiB"))?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn params(p: &Value) -> Result<Params> {
    let fields = p.as_object().ok_or_else(|| error("Plugin image processing settings must be an object"))?;
    if fields.keys().any(|key| !matches!(key.as_str(), "id" | "processor" | "options" | "width" | "height" | "preview")) {
        return Err(error("Unknown plugin image processing setting"));
    }
    for field in ["id", "processor"] {
        if let Some(value) = fields.get(field).filter(|value| !value.is_null())
            && !value.as_str().is_some_and(photocraft_plugins::manifest::valid_id)
        {
            return Err(error(format!("`{field}` must be a valid plugin or processor id")));
        }
    }
    for field in ["width", "height"] {
        if let Some(value) = fields.get(field).filter(|value| !value.is_null())
            && !value.as_u64().is_some_and(|value| u32::try_from(value).is_ok())
        {
            return Err(error(format!("`{field}` must be a nonnegative 32-bit integer")));
        }
    }
    if fields.get("preview").is_some_and(|value| !value.is_boolean()) {
        return Err(error("`preview` must be a boolean"));
    }
    if let Some(options) = p.get("options") {
        if !options.is_object() {
            return Err(error("Plugin image processing options must be an object"));
        }
        serde_json::to_writer(OptionsBudget(0), options).map_err(|_| error("Plugin image processing options exceed 128 KiB"))?;
    }
    serde_json::from_value(p.clone()).map_err(error)
}

fn dimensions(source: (u32, u32), requested: (Option<u32>, Option<u32>)) -> Result<(u32, u32)> {
    let target = (requested.0.unwrap_or(source.0), requested.1.unwrap_or(source.1));
    for ((w, h), max_pixels, label) in [(source, 100_000_000, "source"), (target, 256_000_000, "output")] {
        if w == 0 || h == 0 || w > 32768 || h > 32768 || u64::from(w) * u64::from(h) > max_pixels {
            return Err(error(format!(
                "Plugin {label} images must fit within 32,768 pixels per edge and {} megapixels, with nonzero dimensions",
                max_pixels / 1_000_000
            )));
        }
    }
    let source_pixels = u64::from(source.0) * u64::from(source.1);
    let target_pixels = u64::from(target.0) * u64::from(target.1);
    let estimate = source_pixels.saturating_add(target_pixels).saturating_mul(32).saturating_add(128 << 20);
    let memory_gib = if cfg!(target_pointer_width = "64") { 8 } else { 2 };
    if estimate > memory_gib * 1024 * 1024 * 1024 {
        return Err(error(format!(
            "Plugin image processing exceeds this build's {memory_gib} GiB working-memory budget; reduce the output dimensions or crop a copy"
        )));
    }
    Ok(target)
}

enum Processor {
    #[cfg(not(target_arch = "wasm32"))]
    Native(Arc<crate::extension_package::InstalledNative>),
    Filter(Arc<Plugin>),
}

/// Resolve exclusively within the invoking installation. Registry filters and other packages
/// are intentionally inaccessible even if their processor ids happen to match.
fn processor(s: &Session, id: &str, extension: &Arc<photocraft_plugins::extension::Extension>, requested: Option<&str>) -> Result<Processor> {
    let installed = s
        .extensions
        .installed
        .get(id)
        .filter(|p| p.enabled && Arc::ptr_eq(&p.extension, extension))
        .ok_or_else(|| error("The processing plugin was disabled, removed or replaced"))?;
    #[cfg(not(target_arch = "wasm32"))]
    let native = installed.native.as_ref().filter(|native| native.package.native.as_ref().is_some_and(|n| n.service == "plugin.processImage"));
    #[cfg(target_arch = "wasm32")]
    let native: Option<()> = None;
    let name = match requested {
        Some(name) => name,
        None if native.is_some() => "native",
        None if installed.filters.len() == 1 => {
            installed.filters.keys().next().map(String::as_str).ok_or_else(|| error("The plugin has no image processor"))?
        }
        None => return Err(error("Choose one of this plugin's packaged image processors")),
    };
    if name == "native" {
        #[cfg(not(target_arch = "wasm32"))]
        {
            if native.is_some_and(|native| Arc::strong_count(native) > 1) {
                return Err(error("Wait for this plugin's current processing or task to finish or cancel it first"));
            }
            return native.cloned().map(Processor::Native).ok_or_else(|| error("This plugin does not contain a compatible native image processor"));
        }
        #[cfg(target_arch = "wasm32")]
        return Err(error("Native image processors require the desktop application"));
    }
    installed.filters.get(name).cloned().map(Processor::Filter).ok_or_else(|| error("The requested image processor does not belong to this installed plugin"))
}

/// One flattened RGBA8 document in sRGB, rendered in bounded bands and colour-converted before
/// quantization. Selection and unrelated metadata are not part of the worker's input image.
pub(crate) fn flatten_srgb(doc: &Document, ctx: &JobCtx) -> Result<Document> {
    ctx.check()?;
    let profile = crate::color_cmds::composite_profile(doc);
    let transform = Transform::new(&profile, Builtin::Srgb.profile(), Intent::RelativeColorimetric, true).map_err(error)?;
    let mut surface = Surface::new(PixelFormat::RGBA8);
    photocraft_compose::render_bands(doc, doc.bounds(), 256, |mut band| -> Result<()> {
        ctx.check()?;
        transform.apply(band.px.as_flattened_mut(), 4);
        surface.write_region(band.rect, band.px.as_flattened());
        ctx.progress(0.15 * band.rect.y1 as f32 / doc.size.height.max(1) as f32, "Preparing visible image");
        Ok(())
    })?;
    ctx.check()?;
    surface.prune();
    let mut result = Document::new(&doc.name, doc.size, ColorMode::Rgb, SampleType::U8);
    result.icc_profile = Some(Builtin::Srgb.profile().to_bytes());
    result.resolution_dpi = doc.resolution_dpi;
    let mut layer = Layer::raster("Processed image", PixelFormat::RGBA8);
    *layer.surface_mut().ok_or_else(|| error("Could not create the processing image's pixel layer"))? = surface;
    result.layers.push(layer);
    Ok(result)
}

fn filter_image(doc: &Document, plugin: &Plugin, options: &Value, ctx: &JobCtx) -> Result<Document> {
    let mut result = flatten_srgb(doc, ctx)?;
    let canvas = result.bounds();
    let layer = result.layers.first_mut().ok_or_else(|| error("The processing image has no pixel layer"))?;
    let surface = layer.surface_mut().ok_or_else(|| error("The processing image has no pixel surface"))?;
    let filtered = ctx.stage(0.15, 0.97, "Processing image", |ctl| plugin.apply_with(surface, canvas, doc.selection.as_ref(), options, ctl));
    ctx.check()?;
    *surface = filtered.map_err(error)?;
    result.name = format!("{} — {}", doc.name, plugin.manifest().name);
    ctx.check()?;
    Ok(result)
}

pub(crate) fn run(s: &mut Session, p: &Value, completion: Option<Completion>) -> Result<Value> {
    let params = params(p)?;
    let (id, extension) = match &completion {
        Some((id, extension, _)) => {
            if params.id.as_ref().is_some_and(|requested| requested != id) {
                return Err(error("A plugin may process images only with its own installation"));
            }
            (id.clone(), Arc::clone(extension))
        }
        None => {
            let id = params.id.as_ref().ok_or_else(|| error("Choose an installed plugin with `id`"))?;
            let installed =
                s.extensions.installed.get(id).filter(|p| p.enabled).ok_or_else(|| error("The requested processing plugin is not installed or enabled"))?;
            (id.clone(), Arc::clone(&installed.extension))
        }
    };
    let processor = processor(s, &id, &extension, params.processor.as_deref())?;
    let active = s.active().ok_or(EngineError::NoDocument)?;
    let source_revision = active.revision;
    let doc = Arc::clone(&active.doc);
    if s.job_on(doc.id).is_some() {
        return Err(error("Wait for the current image job to finish or cancel it first"));
    }
    let expected = dimensions((doc.size.width, doc.size.height), (params.width, params.height))?;
    if !matches!(doc.depth, SampleType::U8 | SampleType::U16) || !matches!(doc.mode, ColorMode::Rgb | ColorMode::Grayscale) {
        return Err(error("Plugin image processing accepts 8/16-bit RGB or Grayscale images. Convert a copy first; results are flattened 8-bit sRGB."));
    }
    let options = match &processor {
        Processor::Filter(plugin) => {
            if expected != (doc.size.width, doc.size.height) {
                return Err(error("Sandboxed pixel filters keep the source image's dimensions"));
            }
            Value::Object(plugin.manifest().resolve_params(&params.options).map_err(error)?)
        }
        #[cfg(not(target_arch = "wasm32"))]
        Processor::Native(_) => params.options,
    };
    let binding = match (&processor, completion) {
        #[cfg(not(target_arch = "wasm32"))]
        (Processor::Native(native), completion) => crate::plugin_preview::PreviewBinding::for_native(s, native, completion)?,
        (_, Some(completion)) => crate::plugin_preview::PreviewBinding::for_extension(s, &completion)?,
        (_, None) => crate::plugin_preview::PreviewBinding::for_installed(s, &id, &extension, None)?,
    };
    // Even immediate results need a revocable token: disabling and re-enabling a plugin
    // during processing must never admit an operation from before that revocation.
    let token = crate::plugin_preview::reserve(s, doc.id, &binding)?;
    let preview = params.preview;
    let source = Arc::clone(&doc);
    // Both closures hold a processor lease: cancellation may finish before the worker exits,
    // and updates must not replace its files or code while either side is still using them.
    let apply_processor = match &processor {
        #[cfg(not(target_arch = "wasm32"))]
        Processor::Native(native) => Processor::Native(Arc::clone(native)),
        Processor::Filter(filter) => Processor::Filter(Arc::clone(filter)),
    };
    crate::jobs::run(
        s,
        "Process plugin image",
        true,
        move |ctx| match processor {
            Processor::Filter(filter) => filter_image(&doc, &filter, &options, ctx),
            #[cfg(not(target_arch = "wasm32"))]
            Processor::Native(native) => {
                let _store = native.lease()?;
                let input = flatten_srgb(&doc, ctx)?;
                let job = crate::plugin_runtime::ImageJob {
                    options,
                    expected,
                    result_name: format!("{} — Processed", doc.name),
                    label: "Image processing",
                    opening: "Preparing processed image",
                };
                let mut result = crate::plugin_runtime::process(&input, &native.worker, &job, ctx)?;
                if result.mode != ColorMode::Rgb || result.depth != SampleType::U8 {
                    return Err(error("The plugin must return an 8-bit RGB or RGBA PNG"));
                }
                crate::color_cmds::convert_document(&mut result, Builtin::Srgb.profile(), Intent::RelativeColorimetric, true)?;
                ctx.check()?;
                Ok(result)
            }
        },
        move |s, document| {
            let _processor = apply_processor;
            let staged = crate::plugin_preview::stage(s, token, source, source_revision, document, binding)?;
            if preview { Ok(staged) } else { crate::plugin_preview::apply(s, &json!({"preview":token,"target":"document"})) }
        },
    )
}

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: "plugin.processImage",
        label: "Process Visible Image with Plugin",
        menu: &[],
        shortcut: None,
        params: r#"{"id":text,"processor":text,"options":object={},"width":u32,"height":u32,"preview":bool=true} (uses this installed plugin's packaged processor; result: flattened 8-bit sRGB)"#,
        enabled: |s| s.active().map(|_| ()).ok_or_else(|| "no document open".into()),
        run: |s, p| run(s, p, None),
        journal: false,
    }]
}

#[cfg(test)]
mod tests;
