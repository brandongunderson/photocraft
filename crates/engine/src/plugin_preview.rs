//! A bounded, session-only image comparison transaction. Native processing produces one
//! immutable result; Apply admits that result as a document or as one undoable layer.
use std::{collections::BTreeMap, sync::Arc};

use photocraft_doc::{DocId, Document};
use photocraft_plugins::extension::Extension;
use serde::Deserialize;
use serde_json::{Value, json};

#[cfg(not(target_arch = "wasm32"))]
use crate::extension_cmds::Completion;
use crate::{EngineError, Result, Session, commands::CommandSpec};

fn error(message: impl Into<String>) -> EngineError {
    EngineError::Other(message.into())
}

/// Read-only comparison data. The source shares the document's copy-on-write tiles and the
/// result is decoded once. Each invocation retains its own validated source and result.
pub struct PluginImagePreview {
    pub id: u64,
    pub plugin_id: String,
    pub source: Arc<Document>,
    pub result: Arc<Document>,
    source_revision: u64,
    binding: PreviewBinding,
    retained_bytes: u64,
}

impl PluginImagePreview {
    /// Enlarged results need a new document: a layer cannot change its parent's canvas.
    pub fn can_apply_as_layer(&self) -> bool {
        self.source.size == self.result.size
            && self.source.depth.bits() <= 16
            && matches!(self.source.mode, photocraft_color::ColorMode::Rgb | photocraft_color::ColorMode::Grayscale)
    }
}

#[derive(Default)]
pub(crate) struct PreviewStore {
    next: u64,
    pending: BTreeMap<u64, PendingPreview>,
    previews: BTreeMap<u64, PluginImagePreview>,
}

pub const MAX_PREVIEWS: usize = 16;
pub const MAX_PREVIEW_BYTES: u64 = 512 << 20;

struct PendingPreview {
    id: u64,
    source: DocId,
    plugin_id: String,
    reserved_bytes: u64,
    lifetime: Option<std::sync::Weak<()>>,
}

impl PreviewStore {
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn clear(&mut self) {
        self.previews.clear();
        self.pending.clear();
    }

    pub(crate) fn forget_source(&mut self, source: DocId) {
        self.previews.retain(|_, preview| preview.source.id != source);
        self.pending.retain(|_, pending| pending.source != source);
    }

    fn prune_finished(&mut self) {
        self.pending.retain(|_, pending| pending.lifetime.as_ref().is_none_or(|lifetime| lifetime.strong_count() > 0));
    }

    fn bytes(&self) -> u64 {
        self.previews
            .values()
            .fold(0_u64, |total, preview| total.saturating_add(preview.retained_bytes))
            .saturating_add(self.pending.values().fold(0_u64, |total, pending| total.saturating_add(pending.reserved_bytes)))
    }
}

pub(crate) struct PreviewBinding {
    id: String,
    extension: Arc<Extension>,
    state: Option<Value>,
    /// Proposed state must not overwrite a newer successful panel action or task.
    source_state: Value,
    // A finished preview must not lease native files: the user may uninstall the plugin.
    // Apply nevertheless rejects an installation replaced after processing completed.
    #[cfg(not(target_arch = "wasm32"))]
    native: Option<std::sync::Weak<crate::extension_package::InstalledNative>>,
}

impl PreviewBinding {
    #[cfg(not(target_arch = "wasm32"))]
    fn lease(&self, session: &Session) -> Result<Option<crate::extension_package::AdmissionGuard>> {
        if let Some(native) = &self.native {
            return native.upgrade().ok_or_else(|| error("The processing plugin is no longer installed"))?.admission();
        }
        session.extensions.folder.as_deref().map(crate::extension_package::AdmissionGuard::store).transpose()
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn for_native(s: &Session, native: &Arc<crate::extension_package::InstalledNative>, completion: Option<Completion>) -> Result<Self> {
        let (id, extension, state) = match completion {
            Some((id, extension, state)) => (id, extension, Some(state)),
            None => {
                let (id, installed) = s
                    .extensions
                    .installed
                    .iter()
                    .find(|(_, p)| p.native.as_ref().is_some_and(|n| Arc::ptr_eq(n, native)))
                    .ok_or_else(|| error("The processing plugin is no longer installed"))?;
                (id.clone(), Arc::clone(&installed.extension), None)
            }
        };
        let mut binding = Self::for_installed(s, &id, &extension, state)?;
        binding.native = Some(Arc::downgrade(native));
        binding.validate(s)?;
        Ok(binding)
    }

    /// Bind a result to a package-owned WASM processor, without leasing its files after work.
    pub(crate) fn for_extension(s: &Session, completion: &crate::extension_cmds::Completion) -> Result<Self> {
        let (id, extension, state) = completion;
        Self::for_installed(s, id, extension, Some(state.clone()))
    }

    pub(crate) fn for_installed(s: &Session, id: &str, extension: &Arc<Extension>, state: Option<Value>) -> Result<Self> {
        let source_state = s.extensions.installed.get(id).ok_or_else(|| error("The processing plugin is no longer installed"))?.state.clone();
        let binding = Self {
            id: id.into(),
            extension: Arc::clone(extension),
            state,
            source_state,
            #[cfg(not(target_arch = "wasm32"))]
            native: None,
        };
        binding.validate(s)?;
        Ok(binding)
    }

    pub(crate) fn validate(&self, s: &Session) -> Result<()> {
        #[cfg(not(target_arch = "wasm32"))]
        let _store = self.lease(s)?;
        self.extension.check_current().map_err(|cause| error(cause.to_string()))?;
        let installed = s
            .extensions
            .installed
            .get(&self.id)
            .filter(|p| p.enabled && Arc::ptr_eq(&p.extension, &self.extension))
            .ok_or_else(|| error("This plugin was disabled, removed or replaced. Generate a new preview before applying it."))?;
        if installed.state != self.source_state {
            return Err(error("This plugin's state changed after the preview was requested. Generate a new preview before applying it."));
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(native) = &self.native
            && !installed.native.as_ref().is_some_and(|n| std::sync::Weak::ptr_eq(native, &Arc::downgrade(n)))
        {
            return Err(error("This plugin's processing engine was replaced. Generate a new preview before applying it."));
        }
        #[cfg(target_arch = "wasm32")]
        let _ = installed;
        Ok(())
    }

    pub(crate) fn commit(self, s: &mut Session) {
        if let Some(state) = self.state
            && let Some(installed) = s.extensions.installed.get_mut(&self.id)
            && installed.enabled
            && Arc::ptr_eq(&installed.extension, &self.extension)
        {
            installed.state = state;
        }
    }
}

impl Session {
    /// Invalidate both in-flight and completed comparisons when their plugin changes.
    pub(crate) fn forget_plugin_preview(&mut self, plugin_id: &str) {
        self.extensions.previews.previews.retain(|_, preview| preview.plugin_id != plugin_id);
        // Re-enabling the same installation must not resurrect an earlier operation.
        self.extensions.previews.pending.retain(|_, pending| pending.plugin_id != plugin_id);
    }

    pub fn plugin_preview(&self, id: u64) -> Option<&PluginImagePreview> {
        self.extensions.previews.previews.get(&id)
    }

    pub fn plugin_preview_for(&self, plugin_id: &str) -> Option<&PluginImagePreview> {
        // Legacy panels display one comparison: prefer this plugin's newest result for the
        // active image, then its newest result anywhere. Other comparisons remain addressable.
        let active = self.active().map(|document| document.doc.id);
        self.extensions
            .previews
            .previews
            .values()
            .rev()
            .find(|preview| preview.plugin_id == plugin_id && Some(preview.source.id) == active)
            .or_else(|| self.extensions.previews.previews.values().rev().find(|preview| preview.plugin_id == plugin_id))
    }

    pub fn plugin_previews(&self) -> impl Iterator<Item = &PluginImagePreview> {
        self.extensions.previews.previews.values()
    }

    pub fn plugin_preview_usage(&self) -> Value {
        json!({"count":self.extensions.previews.previews.len(),"pending":self.extensions.previews.pending.len(),
            "reservedBytes":self.extensions.previews.bytes(),"maxCount":MAX_PREVIEWS,"maxBytes":MAX_PREVIEW_BYTES,
            "policy":"same plugin and source replace their previous comparison; unrelated results require explicit Apply or Discard"})
    }

    /// A UI may show this error before the user clicks Apply. Apply checks it again.
    pub fn validate_plugin_preview(&self, id: u64) -> Result<()> {
        let preview = self.plugin_preview(id).ok_or_else(|| error("This image preview is no longer available. Generate a new preview."))?;
        let source = self
            .documents()
            .iter()
            .find(|d| d.doc.id == preview.source.id)
            .ok_or_else(|| error("The preview's source document was closed. Generate a new preview."))?;
        if !Arc::ptr_eq(&source.doc, &preview.source) || source.revision != preview.source_revision {
            return Err(error("The source image changed after this preview was created. Generate a new preview before applying it."));
        }
        if self.job_on(source.doc.id).is_some() {
            return Err(error("Wait for the source image's current job to finish before applying this preview."));
        }
        preview.binding.validate(self)
    }
}

#[cfg(test)]
pub(crate) fn reserve(s: &mut Session, source: DocId, binding: &PreviewBinding) -> Result<u64> {
    let document = &s.documents().iter().find(|document| document.doc.id == source).ok_or(EngineError::NoDocument)?.doc;
    let estimate = retained_document_bytes(document).saturating_add(result_bytes(document.size.width, document.size.height));
    reserve_with_budget(s, source, binding, estimate)
}

pub(crate) fn result_bytes(width: u32, height: u32) -> u64 {
    // RGBA8 tiles include edge padding. Reserve the actual tiled allocation, not just
    // width*height*4, which undercounts very small or narrow decoded results.
    u64::from(width).div_ceil(256).saturating_mul(u64::from(height).div_ceil(256)).saturating_mul(256 * 256 * 4)
}

pub(crate) fn retained_document_bytes(document: &Document) -> u64 {
    let pixels = photocraft_ops::History::default().pixel_bytes(document) as u64;
    let mut bytes = pixels.saturating_add(std::mem::size_of::<Document>() as u64);
    bytes = bytes.saturating_add(document.name.len() as u64);
    bytes = bytes.saturating_add(document.icc_profile.as_ref().map_or(0, |profile| profile.len() as u64));
    bytes = bytes.saturating_add(document.metadata.exif.as_ref().map_or(0, |exif| exif.len() as u64));
    bytes = bytes.saturating_add(document.metadata.xmp.as_ref().map_or(0, |xmp| xmp.len() as u64));
    for (key, value) in &document.metadata.text {
        bytes = bytes.saturating_add(key.len() as u64).saturating_add(value.len() as u64);
    }
    for (_, name, payload) in &document.metadata.psd_resources {
        bytes = bytes.saturating_add(name.len() as u64).saturating_add(payload.len() as u64);
    }
    for (_, _, payload) in &document.metadata.psd_global_blocks {
        bytes = bytes.saturating_add(payload.len() as u64);
    }
    for (_, _, layer) in document.walk() {
        bytes = bytes.saturating_add(std::mem::size_of::<photocraft_doc::Layer>() as u64).saturating_add(layer.name.len() as u64);
        for (_, payload) in &layer.psd_blocks {
            bytes = bytes.saturating_add(payload.len() as u64);
        }
        if let photocraft_doc::LayerContent::Smart(smart) = &layer.content {
            if let photocraft_doc::SmartSource::Embedded { bytes: payload, .. } = &smart.source {
                bytes = bytes.saturating_add(payload.len() as u64);
            }
            bytes = bytes.saturating_add(smart.psd_raw.as_ref().map_or(0, |raw| raw.len() as u64));
        }
        if let photocraft_doc::LayerContent::Text(text) = &layer.content {
            bytes = bytes.saturating_add(text.text.len() as u64);
        }
    }
    bytes
}

pub(crate) fn reserve_with_budget(s: &mut Session, source: DocId, binding: &PreviewBinding, reserved_bytes: u64) -> Result<u64> {
    if s.extensions.previews.pending.values().any(|pending| pending.source == source && pending.plugin_id == binding.id && s.job_on(source).is_some()) {
        return Err(error("Wait for this plugin's current preview of this image to finish or cancel it first"));
    }
    let store = &mut s.extensions.previews;
    store.prune_finished();
    let replace = store
        .previews
        .values()
        .filter(|preview| preview.source.id == source && preview.plugin_id == binding.id)
        .map(|preview| preview.retained_bytes)
        .fold(0_u64, u64::saturating_add);
    let replace_pending = store
        .pending
        .values()
        .filter(|pending| pending.source == source && pending.plugin_id == binding.id)
        .map(|pending| pending.reserved_bytes)
        .fold(0_u64, u64::saturating_add);
    let replaced_count = store.previews.values().filter(|preview| preview.source.id == source && preview.plugin_id == binding.id).count()
        + store.pending.values().filter(|pending| pending.source == source && pending.plugin_id == binding.id).count();
    if store.previews.len().saturating_add(store.pending.len()).saturating_sub(replaced_count) >= MAX_PREVIEWS
        || store.bytes().saturating_sub(replace).saturating_sub(replace_pending).saturating_add(reserved_bytes) > MAX_PREVIEW_BYTES
    {
        return Err(error(
            "Plugin comparison memory is full. Apply or Discard an existing result, or request a smaller image; other plugins' results were preserved.",
        ));
    }
    let id = s.extensions.previews.next.checked_add(1).ok_or_else(|| error("The preview identifier limit was reached; restart PhotoCraft"))?;
    s.extensions.previews.next = id;
    s.extensions.previews.previews.retain(|_, preview| preview.source.id != source || preview.plugin_id != binding.id);
    s.extensions.previews.pending.retain(|_, pending| pending.source != source || pending.plugin_id != binding.id);
    s.extensions.previews.pending.insert(id, PendingPreview { id, source, plugin_id: binding.id.clone(), reserved_bytes, lifetime: None });
    Ok(id)
}

/// The work and completion closures own this liveness marker. Failed/cancelled jobs drop
/// it without touching Session, and their expired reservations are pruned on next admission.
pub(crate) fn lifetime(s: &mut Session, id: u64) -> Result<Arc<()>> {
    let pending = s.extensions.previews.pending.get_mut(&id).ok_or_else(|| error("The preview reservation was revoked"))?;
    let lifetime = Arc::new(());
    pending.lifetime = Some(Arc::downgrade(&lifetime));
    Ok(lifetime)
}

pub(crate) fn stage(s: &mut Session, id: u64, source: Arc<Document>, source_revision: u64, result: Document, binding: PreviewBinding) -> Result<Value> {
    #[cfg(not(target_arch = "wasm32"))]
    let _store = binding.lease(s)?;
    if !s.extensions.previews.pending.get(&id).is_some_and(|pending| pending.id == id && pending.source == source.id && pending.plugin_id == binding.id) {
        return Err(error("This preview was discarded or replaced before processing finished"));
    }
    s.extensions.previews.pending.remove(&id);
    binding.validate(s)?;
    let current = s.documents().iter().find(|d| d.doc.id == source.id);
    if !current.is_some_and(|d| Arc::ptr_eq(&d.doc, &source) && d.revision == source_revision) {
        return Err(error("The source image changed while the preview was being prepared"));
    }
    let size = result.size;
    let retained_bytes = retained_document_bytes(&source).saturating_add(retained_document_bytes(&result));
    if s.extensions.previews.bytes().saturating_add(retained_bytes) > MAX_PREVIEW_BYTES {
        return Err(error("The completed comparison exceeds the retained-memory budget. Other plugins' results were preserved; request a smaller image."));
    }
    let preview = PluginImagePreview { id, plugin_id: binding.id.clone(), source, result: Arc::new(result), source_revision, binding, retained_bytes };
    s.extensions.previews.previews.insert(id, preview);
    Ok(json!({"preview":id,"message":format!("Preview ready: {} × {}. Compare the image, then Apply or Discard.", size.width, size.height)}))
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Target {
    #[default]
    Document,
    Layer,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Apply {
    preview: u64,
    #[serde(default)]
    target: Target,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Discard {
    preview: u64,
}

pub(crate) fn apply(s: &mut Session, p: &Value) -> Result<Value> {
    if !p.is_object() {
        return Err(error("Preview settings must be an object"));
    }
    let params: Apply = serde_json::from_value(p.clone()).map_err(|e| error(e.to_string()))?;
    if let Err(e) = s.validate_plugin_preview(params.preview) {
        // Do not let a stale result pin a large image indefinitely. A wrong token must
        // nevertheless leave the user's current valid comparison available.
        if s.plugin_preview(params.preview).is_some() {
            s.extensions.previews.previews.remove(&params.preview);
        }
        return Err(e);
    }
    #[cfg(not(target_arch = "wasm32"))]
    let _store = s.plugin_preview(params.preview).map_or(Ok(None), |preview| preview.binding.lease(s))?;
    // Recheck under the retained publication guard: another process may have revoked the
    // installation between the UI's initial validation and acquisition of this guard.
    s.validate_plugin_preview(params.preview)?;
    match params.target {
        Target::Document => {
            let preview = s.extensions.previews.previews.remove(&params.preview).ok_or_else(|| error("The image preview is no longer available"))?;
            let size = preview.result.size;
            let document = Arc::try_unwrap(preview.result).unwrap_or_else(|doc| (*doc).clone());
            let (index, color) = s.open_document(document, None);
            preview.binding.commit(s);
            Ok(json!({"document":index,"color":color,"message":format!("Opened {} × {} processed copy. The original is unchanged.", size.width,size.height)}))
        }
        Target::Layer => {
            let preview = s.plugin_preview(params.preview).ok_or_else(|| error("The image preview is no longer available"))?;
            if !preview.can_apply_as_layer() {
                return Err(error(
                    "A new layer requires a same-size result in an 8/16-bit RGB or Grayscale document. Choose New document for an enlarged result.",
                ));
            }
            let source = Arc::clone(&preview.source);
            let mut copies = (*preview.result).clone();
            let original = copies.layers.first().ok_or_else(|| error("The processed image has no pixel layer"))?;
            let mut layer = original.duplicate();
            layer.name = source.next_layer_name("Processed image");
            layer.locks = Default::default();
            copies.layers = vec![layer];
            crate::layer_copy_cmds::adapt_copies(s, &mut copies, &source)?;
            let layer = copies.layers.pop().ok_or_else(|| error("The processed image has no pixel layer"))?;
            let index = s.documents().iter().position(|d| d.doc.id == source.id).ok_or(EngineError::NoDocument)?;
            let previous = s.active_index();
            s.set_active(index);
            let applied = s.edit("Apply Plugin Result", move |doc, active| {
                // The result already contains the visible composite. Leaving the source
                // visible underneath would double-composite partial alpha (0.5 over 0.5
                // becomes 0.75), so Apply would differ from the comparison. Keep every
                // original layer intact but hide the original top-level stack in this
                // same undo step; Undo restores its exact previous visibility.
                for original in &mut doc.layers {
                    original.visible = false;
                }
                // Land at the document's top level so a hidden group cannot hide the result.
                let top = doc.layers.last().map(|l| l.id);
                let id = doc.insert_above(top, layer);
                *active = Some(id);
                Ok(id)
            });
            let layer = match applied {
                Ok(layer) => layer,
                Err(e) => {
                    if let Some(previous) = previous {
                        s.set_active(previous);
                    }
                    return Err(e);
                }
            };
            let preview = s.extensions.previews.previews.remove(&params.preview).ok_or_else(|| error("The image preview is no longer available"))?;
            preview.binding.commit(s);
            Ok(
                json!({"document":index,"layer":layer.0,"message":"Applied the processed image as a new layer. Original layers are preserved underneath and hidden; Undo restores their visibility."}),
            )
        }
    }
}

fn discard(s: &mut Session, p: &Value) -> Result<Value> {
    if !p.is_object() {
        return Err(error("Preview settings must be an object"));
    }
    let params: Discard = serde_json::from_value(p.clone()).map_err(|e| error(e.to_string()))?;
    if s.extensions.previews.previews.remove(&params.preview).is_none() && s.extensions.previews.pending.remove(&params.preview).is_none() {
        return Err(error("This image preview is no longer available"));
    }
    Ok(json!({"discarded":params.preview,"message":"Preview discarded. The original image is unchanged."}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "image.pluginPreview.apply",
            label: "Apply Plugin Image Preview",
            menu: &[],
            shortcut: None,
            params: r#"{"preview":u64,"target":"document"|"layer"="document"}"#,
            enabled: |_| Ok(()),
            run: apply,
            journal: false,
        },
        CommandSpec {
            id: "image.pluginPreview.discard",
            label: "Discard Plugin Image Preview",
            menu: &[],
            shortcut: None,
            params: r#"{"preview":u64}"#,
            enabled: |_| Ok(()),
            run: discard,
            journal: false,
        },
    ]
}

#[cfg(test)]
#[cfg(not(target_arch = "wasm32"))]
mod tests;
