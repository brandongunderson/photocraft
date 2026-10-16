//! A bounded, session-only image comparison transaction. Native processing produces one
//! immutable result; Apply admits that result as a document or as one undoable layer.
use std::sync::Arc;

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
/// result is decoded once. At most one completed comparison is retained per session.
pub struct PluginImagePreview {
    pub id: u64,
    pub plugin_id: String,
    pub source: Arc<Document>,
    pub result: Arc<Document>,
    source_revision: u64,
    binding: PreviewBinding,
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
    pending: Option<PendingPreview>,
    preview: Option<PluginImagePreview>,
}

struct PendingPreview {
    id: u64,
    source: DocId,
    plugin_id: String,
}

impl PreviewStore {
    pub(crate) fn clear(&mut self) {
        self.preview = None;
        self.pending = None;
    }

    pub(crate) fn forget_source(&mut self, source: DocId) {
        if self.preview.as_ref().is_some_and(|p| p.source.id == source) {
            self.preview = None;
        }
        if self.pending.as_ref().is_some_and(|pending| pending.source == source) {
            self.pending = None;
        }
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
    fn lease(&self) -> Result<Option<crate::extension_package::StoreGuard>> {
        self.native.as_ref().map_or(Ok(None), |native| native.upgrade().ok_or_else(|| error("The processing plugin is no longer installed"))?.lease())
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
        let _store = self.lease()?;
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
        if self.extensions.previews.preview.as_ref().is_some_and(|p| p.plugin_id == plugin_id) {
            self.extensions.previews.preview = None;
        }
        if self.extensions.previews.pending.as_ref().is_some_and(|pending| pending.plugin_id == plugin_id) {
            // Re-enabling the same installation must not resurrect an earlier operation.
            // The worker may finish stopping, but its revoked token can never be staged.
            self.extensions.previews.pending = None;
        }
    }

    pub fn plugin_preview(&self, id: u64) -> Option<&PluginImagePreview> {
        self.extensions.previews.preview.as_ref().filter(|p| p.id == id)
    }

    pub fn plugin_preview_for(&self, plugin_id: &str) -> Option<&PluginImagePreview> {
        self.extensions.previews.preview.as_ref().filter(|p| p.plugin_id == plugin_id)
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

pub(crate) fn reserve(s: &mut Session, source: DocId, binding: &PreviewBinding) -> Result<u64> {
    if s.extensions.previews.pending.as_ref().is_some_and(|pending| s.job_on(pending.source).is_some()) {
        return Err(error("Wait for the current plugin preview to finish or cancel it first"));
    }
    let id = s.extensions.previews.next.checked_add(1).ok_or_else(|| error("The preview identifier limit was reached; restart PhotoCraft"))?;
    s.extensions.previews.next = id;
    s.extensions.previews.preview = None;
    s.extensions.previews.pending = Some(PendingPreview { id, source, plugin_id: binding.id.clone() });
    Ok(id)
}

pub(crate) fn stage(s: &mut Session, id: u64, source: Arc<Document>, source_revision: u64, result: Document, binding: PreviewBinding) -> Result<Value> {
    #[cfg(not(target_arch = "wasm32"))]
    let _store = binding.lease()?;
    if !s.extensions.previews.pending.as_ref().is_some_and(|pending| pending.id == id && pending.source == source.id && pending.plugin_id == binding.id) {
        return Err(error("This preview was discarded or replaced before processing finished"));
    }
    s.extensions.previews.pending = None;
    binding.validate(s)?;
    let current = s.documents().iter().find(|d| d.doc.id == source.id);
    if !current.is_some_and(|d| Arc::ptr_eq(&d.doc, &source) && d.revision == source_revision) {
        return Err(error("The source image changed while the preview was being prepared"));
    }
    let size = result.size;
    let preview = PluginImagePreview { id, plugin_id: binding.id.clone(), source, result: Arc::new(result), source_revision, binding };
    s.extensions.previews.preview = Some(preview);
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
            s.extensions.previews.preview = None;
        }
        return Err(e);
    }
    #[cfg(not(target_arch = "wasm32"))]
    let _store = s.plugin_preview(params.preview).map_or(Ok(None), |preview| preview.binding.lease())?;
    match params.target {
        Target::Document => {
            let preview = s.extensions.previews.preview.take().ok_or_else(|| error("The image preview is no longer available"))?;
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
            let preview = s.extensions.previews.preview.take().ok_or_else(|| error("The image preview is no longer available"))?;
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
    if s.plugin_preview(params.preview).is_some() {
        s.extensions.previews.preview = None;
    } else if s.extensions.previews.pending.as_ref().is_some_and(|pending| pending.id == params.preview) {
        s.extensions.previews.pending = None;
    } else {
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
