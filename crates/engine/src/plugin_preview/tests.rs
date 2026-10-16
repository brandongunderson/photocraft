use super::*;
use crate::extension_cmds::Installed;
use crate::extension_package::{InstalledNative, NativeInfo, PackageInfo};
use photocraft_cms::{Builtin, Intent, Transform};
use photocraft_color::{ColorMode, SampleType};
use photocraft_doc::Size;

const PLUGIN: &str = "org.example.preview";

fn extension_bytes() -> Vec<u8> {
    extension_bytes_for(PLUGIN)
}

fn extension_bytes_for(id: &str) -> Vec<u8> {
    let manifest = json!({"apiVersion":1,"id":id,"name":"Preview fixture","version":"1.0.0",
        "capabilities":{"required":[{"id":"host.commands.invoke","minVersion":1}]},
        "permissions":{"commands":["plugin.processImage"]},"panel":{"buttons":[{"id":"process","label":"Preview"}]}})
    .to_string();
    let quote = manifest.bytes().map(|b| format!("\\{b:02x}")).collect::<String>();
    let packed = ((manifest.len() as u64) << 32) | 32;
    wat::parse_str(format!(
        r#"(module
        (memory (export "memory") 4 512)
        (data (i32.const 32) "{quote}")
        (func (export "pce_api_version") (result i32) i32.const 1)
        (func (export "pce_manifest") (result i64) i64.const {packed})
        (func (export "pce_alloc") (param i32) (result i32) i32.const 196608)
        (func (export "pce_handle") (param i32 i32) (result i64) i64.const 0)
    )"#
    ))
    .unwrap()
}

fn extension() -> Arc<Extension> {
    Arc::new(Extension::load(&extension_bytes()).unwrap())
}

fn session() -> Session {
    let mut s = Session::new();
    s.execute("file.new", json!({"width":8,"height":6,"background":"white"})).unwrap();
    let native = Arc::new(InstalledNative {
        generation: None,
        worker_v2: None,
        worker: "unused-fixture-worker.exe".into(),
        package: PackageInfo {
            package_version: 2,
            module: "plugin.pcext".into(),
            native: Some(NativeInfo { service: "plugin.processImage".into(), platform: "windows-x86_64".into(), entry: "unused-fixture-worker.exe".into() }),
            filters: Vec::new(),
        },
    });
    s.extensions.installed.insert(
        PLUGIN.into(),
        Installed {
            #[cfg(not(target_arch = "wasm32"))]
            deferred: None,
            extension: extension(),
            enabled: true,
            state: json!({"runs":0}),
            storage: Default::default(),
            package: Some(native.package.clone()),
            native: Some(native),
            filters: Default::default(),
        },
    );
    s
}

fn staged(s: &mut Session, scale: u32) -> u64 {
    let source = Arc::clone(&s.active().unwrap().doc);
    let revision = s.active().unwrap().revision;
    let installed = &s.extensions.installed[PLUGIN];
    let completion = Some((PLUGIN.into(), Arc::clone(&installed.extension), json!({"runs":1})));
    let native = Arc::clone(installed.native.as_ref().unwrap());
    let binding = PreviewBinding::for_native(s, &native, completion).unwrap();
    let mut result = Document::new("Enhanced", Size::new(source.size.width * scale, source.size.height * scale), ColorMode::Rgb, SampleType::U8);
    result.icc_profile = Some(Builtin::Srgb.profile().to_bytes());
    let mut layer = photocraft_doc::Layer::raster("Processed", result.pixel_format());
    layer.surface_mut().unwrap().write_region(result.bounds(), &[0.6, 0.2, 0.3, 0.8].repeat((result.size.width * result.size.height) as usize));
    result.layers.push(layer);
    let id = reserve(s, source.id, &binding).unwrap();
    assert_eq!(stage(s, id, source, revision, result, binding).unwrap()["preview"], id);
    id
}

#[test]
fn staging_has_no_document_or_state_effect_and_apply_opens_only_once() {
    let mut s = session();
    let source = Arc::clone(&s.active().unwrap().doc);
    let revision = s.active().unwrap().revision;
    let id = staged(&mut s, 2);
    assert_eq!(s.documents().len(), 1);
    assert!(Arc::ptr_eq(&source, &s.active().unwrap().doc));
    assert_eq!(s.active().unwrap().revision, revision);
    assert_eq!(s.extensions.installed[PLUGIN].state["runs"], 0);
    assert_eq!(s.plugin_preview_for(PLUGIN).unwrap().id, id);
    let result = s.execute("image.pluginPreview.apply", json!({"preview":id})).unwrap();
    assert_eq!(result["document"], 1);
    assert_eq!(s.documents().len(), 2);
    assert!(Arc::ptr_eq(&source, &s.documents()[0].doc));
    assert_eq!(s.active().unwrap().doc.size, Size::new(16, 12));
    assert_eq!(s.extensions.installed[PLUGIN].state["runs"], 1);
    assert!(s.plugin_preview(id).is_none());
    assert!(s.execute("image.pluginPreview.apply", json!({"preview":id})).is_err());
}

#[test]
fn same_size_result_is_a_new_top_level_undoable_layer_in_source_profile_and_depth() {
    for depth in [SampleType::U8, SampleType::U16] {
        let mut s = session();
        s.execute(if depth == SampleType::U16 { "image.mode.bits16" } else { "image.mode.bits8" }, json!({})).unwrap();
        let doc = Arc::make_mut(&mut s.active_mut().unwrap().doc);
        doc.depth = depth;
        doc.icc_profile = Some(Builtin::DisplayP3.profile().to_bytes());
        let source = Arc::clone(&s.active().unwrap().doc);
        let count = source.layers.len();
        let id = staged(&mut s, 1);
        assert!(s.plugin_preview(id).unwrap().can_apply_as_layer());
        s.execute("image.pluginPreview.apply", json!({"preview":id,"target":"layer"})).unwrap();
        assert_eq!(s.documents().len(), 1);
        let doc = &s.active().unwrap().doc;
        assert_eq!(doc.layers.len(), count + 1);
        assert_eq!(doc.size, source.size);
        assert_eq!(doc.depth, depth);
        assert_eq!(doc.icc_profile, source.icc_profile);
        let mut hidden_originals = source.layers.clone();
        for original in &mut hidden_originals {
            original.visible = false;
        }
        assert_eq!(doc.layers[..count], hidden_originals[..]);
        let layer = doc.layers.last().unwrap();
        assert_eq!(layer.surface().unwrap().format().sample, depth);
        assert!(!layer.locks.all);
        let mut expected = [0.6, 0.2, 0.3];
        Transform::new(Builtin::Srgb.profile(), Builtin::DisplayP3.profile(), Intent::RelativeColorimetric, true).unwrap().apply(&mut expected, 3);
        let px = layer.surface().unwrap().read_region(photocraft_geom::Rect::new(0, 0, 1, 1));
        for (actual, expected) in px[..3].iter().zip(expected) {
            assert!((actual - expected).abs() < 0.01, "{actual} vs {expected}");
        }
        assert!((px[3] - 0.8).abs() < 0.01);
        let applied = Arc::clone(doc);
        assert!(s.undo());
        assert_eq!(s.active().unwrap().doc, source);
        assert!(s.redo());
        assert_eq!(s.active().unwrap().doc, applied);
    }
}

#[test]
fn applying_a_same_size_composite_preserves_preview_alpha_and_undo_restores_visibility() {
    let mut s = session();
    s.edit("Translucent source fixture", |doc, active| {
        doc.icc_profile = Some(Builtin::Srgb.profile().to_bytes());
        let mut visible = photocraft_doc::Layer::raster("Visible original", doc.pixel_format());
        visible.surface_mut().unwrap().write_region(doc.bounds(), &[0.2, 0.4, 0.6, 0.5].repeat(48));
        let mut hidden = photocraft_doc::Layer::raster("Already hidden", doc.pixel_format());
        hidden.surface_mut().unwrap().write_region(doc.bounds(), &[0.9, 0.1, 0.2, 0.8].repeat(48));
        hidden.visible = false;
        *active = Some(visible.id);
        doc.layers = vec![visible, hidden];
        Ok(())
    })
    .unwrap();
    let source = Arc::clone(&s.active().unwrap().doc);
    let revision = s.active().unwrap().revision;
    let before = photocraft_compose::flatten(&source);
    let installed = &s.extensions.installed[PLUGIN];
    let native = Arc::clone(installed.native.as_ref().unwrap());
    let completion = Some((PLUGIN.into(), Arc::clone(&installed.extension), json!({"runs":1})));
    let binding = PreviewBinding::for_native(&s, &native, completion).unwrap();
    // A zero-detail/same-size result is the same visible pixels, flattened once.
    let mut result = Document::new("Same-size result", source.size, source.mode, source.depth);
    result.icc_profile.clone_from(&source.icc_profile);
    result.layers.push(source.layers[0].duplicate());
    let expected = photocraft_compose::flatten(&result);
    let id = reserve(&mut s, source.id, &binding).unwrap();
    stage(&mut s, id, Arc::clone(&source), revision, result, binding).unwrap();
    s.execute("image.pluginPreview.apply", json!({"preview":id,"target":"layer"})).unwrap();
    let applied = &s.active().unwrap().doc;
    assert!(!applied.layers[0].visible);
    assert!(!applied.layers[1].visible);
    assert!(applied.layers[2].visible);
    let actual = photocraft_compose::flatten(applied);
    assert_eq!(actual.px, expected.px, "visible Apply output must match the preview, including partial alpha");
    assert!(actual.px.iter().all(|pixel| (pixel[3] - 0.5).abs() < 0.01));
    assert!(s.undo());
    assert_eq!(s.active().unwrap().doc, source);
    assert!(s.active().unwrap().doc.layers[0].visible);
    assert!(!s.active().unwrap().doc.layers[1].visible);
    assert_eq!(photocraft_compose::flatten(&s.active().unwrap().doc).px, before.px);
    assert!(s.redo());
    assert_eq!(photocraft_compose::flatten(&s.active().unwrap().doc).px, expected.px);
}

#[test]
fn layer_choice_rejects_resized_output_without_losing_preview() {
    let mut s = session();
    let id = staged(&mut s, 2);
    assert!(!s.plugin_preview(id).unwrap().can_apply_as_layer());
    assert!(s.execute("image.pluginPreview.apply", json!({"preview":id,"target":"layer"})).unwrap_err().to_string().contains("same-size"));
    assert!(s.plugin_preview(id).is_some());
    assert_eq!(s.documents().len(), 1);
    assert_eq!(s.extensions.installed[PLUGIN].state["runs"], 0);
}

#[test]
fn same_size_grayscale_layer_uses_the_parent_gray_profile_and_depth() {
    let mut s = session();
    s.execute("image.mode.grayscale", json!({})).unwrap();
    s.execute("image.mode.bits16", json!({})).unwrap();
    let source = Arc::clone(&s.active().unwrap().doc);
    let id = staged(&mut s, 1);
    s.execute("image.pluginPreview.apply", json!({"preview":id,"target":"layer"})).unwrap();
    let doc = &s.active().unwrap().doc;
    assert_eq!(doc.mode, ColorMode::Grayscale);
    assert_eq!(doc.depth, SampleType::U16);
    assert_eq!(doc.icc_profile, source.icc_profile);
    let layer = doc.layers.last().unwrap();
    let surface = layer.surface().unwrap();
    assert_eq!(surface.format().mode, ColorMode::Grayscale);
    assert_eq!(surface.format().sample, SampleType::U16);
    let mut expected = [0.0; 2];
    Transform::new(Builtin::Srgb.profile(), &crate::color_cmds::document_profile(&source), Intent::RelativeColorimetric, true).unwrap().convert_f32(
        &[0.6, 0.2, 0.3, 0.8],
        4,
        &mut expected,
        2,
        true,
    );
    let px = surface.read_region(photocraft_geom::Rect::new(0, 0, 1, 1));
    for (actual, expected) in px.iter().zip(expected) {
        assert!((actual - expected).abs() < 0.01, "{actual} vs {expected}");
    }
    assert!(s.undo());
    assert_eq!(s.active().unwrap().doc, source);
}

#[test]
fn edits_undo_close_disable_remove_and_replacement_cannot_apply_stale_results() {
    for change in ["edit", "undo", "close", "disable", "remove", "replace", "engine"] {
        let mut s = session();
        let id = staged(&mut s, 1);
        match change {
            "edit" => {
                s.execute("layer.new.layer", json!({})).unwrap();
            }
            "undo" => {
                s.execute("layer.new.layer", json!({})).unwrap();
                assert!(s.undo());
            }
            "close" => {
                s.close(0);
            }
            "disable" => s.extensions.installed.get_mut(PLUGIN).unwrap().enabled = false,
            "remove" => {
                s.execute("extension.remove", json!({"id":PLUGIN})).unwrap();
            }
            "replace" => s.extensions.installed.get_mut(PLUGIN).unwrap().extension = extension(),
            "engine" => s.extensions.installed.get_mut(PLUGIN).unwrap().native = None,
            _ => panic!("unknown preview invalidation fixture: {change}"),
        }
        let before = s.documents().iter().map(|d| Arc::clone(&d.doc)).collect::<Vec<_>>();
        assert!(s.execute("image.pluginPreview.apply", json!({"preview":id})).is_err(), "{change}");
        assert!(s.plugin_preview(id).is_none());
        assert_eq!(before, s.documents().iter().map(|d| Arc::clone(&d.doc)).collect::<Vec<_>>());
        if let Some(installed) = s.extensions.installed.get(PLUGIN) {
            assert_eq!(installed.state["runs"], 0);
        }
    }
}

#[test]
fn discard_wrong_ids_bad_shapes_and_new_previews_preserve_transaction_rules() {
    let mut s = session();
    let id = staged(&mut s, 1);
    for p in [json!(null), json!([]), json!({}), json!({"preview":-1}), json!({"preview":id,"target":"replace"}), json!({"preview":id,"path":"file.png"})] {
        assert!(s.execute("image.pluginPreview.apply", p).is_err());
        assert!(s.plugin_preview(id).is_some());
    }
    assert!(s.execute("image.pluginPreview.apply", json!({"preview":id+1})).is_err());
    assert!(s.execute("image.pluginPreview.discard", json!({"preview":id+1})).is_err());
    assert!(s.plugin_preview(id).is_some());
    let next = staged(&mut s, 1);
    assert!(next > id);
    assert!(s.plugin_preview(id).is_none());
    s.execute("image.pluginPreview.discard", json!({"preview":next})).unwrap();
    assert!(s.plugin_preview(next).is_none());
    assert_eq!(s.documents().len(), 1);
    assert_eq!(s.extensions.installed[PLUGIN].state["runs"], 0);
    assert!(!crate::extension_cmds::COMMANDS.contains(&"image.pluginPreview.apply"));
    assert!(!crate::extension_cmds::COMMANDS.contains(&"image.pluginPreview.discard"));
}

#[test]
fn discard_inflight_token_rejects_late_result_and_source_close_releases_it() {
    let mut s = session();
    let source = Arc::clone(&s.active().unwrap().doc);
    let revision = s.active().unwrap().revision;
    let native = Arc::clone(s.extensions.installed[PLUGIN].native.as_ref().unwrap());
    let binding = PreviewBinding::for_native(&s, &native, None).unwrap();
    let id = reserve(&mut s, source.id, &binding).unwrap();
    s.execute("image.pluginPreview.discard", json!({"preview":id})).unwrap();
    assert!(stage(&mut s, id, Arc::clone(&source), revision, (*source).clone(), binding).unwrap_err().to_string().contains("discarded"));
    let binding = PreviewBinding::for_native(&s, &native, None).unwrap();
    let id = reserve(&mut s, source.id, &binding).unwrap();
    s.close(0);
    assert!(s.extensions.previews.pending.is_empty());
    assert!(s.plugin_preview(id).is_none());
}

#[test]
fn disabling_and_reenabling_before_completion_cannot_resurrect_an_inflight_preview() {
    let mut s = session();
    let source = Arc::clone(&s.active().unwrap().doc);
    let revision = s.active().unwrap().revision;
    let installed = &s.extensions.installed[PLUGIN];
    let extension = Arc::clone(&installed.extension);
    let native = Arc::clone(installed.native.as_ref().unwrap());
    let completion = Some((PLUGIN.into(), Arc::clone(&extension), json!({"runs":1})));
    let binding = PreviewBinding::for_native(&s, &native, completion).unwrap();
    let id = reserve(&mut s, source.id, &binding).unwrap();
    // An unrelated plugin does not own this pending operation.
    s.forget_plugin_preview("org.example.unrelated");
    assert!(!s.extensions.previews.pending.is_empty());
    s.execute("extension.enable", json!({"id":PLUGIN,"enabled":false})).unwrap();
    assert!(s.extensions.previews.pending.is_empty());
    s.execute("extension.enable", json!({"id":PLUGIN,"enabled":true})).unwrap();
    assert!(Arc::ptr_eq(&extension, &s.extensions.installed[PLUGIN].extension));
    // The slow worker's result arrives only after the same installation was re-enabled.
    assert!(stage(&mut s, id, Arc::clone(&source), revision, (*source).clone(), binding).unwrap_err().to_string().contains("discarded"));
    assert!(s.plugin_preview(id).is_none());
    assert_eq!(s.documents().len(), 1);
    assert!(Arc::ptr_eq(&source, &s.active().unwrap().doc));
    assert_eq!(s.extensions.installed[PLUGIN].state["runs"], 0);
    assert!(s.execute("image.pluginPreview.apply", json!({"preview":id})).is_err());
    drop(native);
    let fresh = staged(&mut s, 1);
    assert!(fresh > id);
    s.execute("image.pluginPreview.apply", json!({"preview":fresh})).unwrap();
    assert_eq!(s.extensions.installed[PLUGIN].state["runs"], 1);
}

#[test]
fn plugin_management_immediately_releases_the_completed_preview() {
    for action in ["disable", "remove", "replace", "attach"] {
        let mut s = session();
        let id = staged(&mut s, 1);
        let result = Arc::downgrade(&s.plugin_preview(id).unwrap().result);
        let source = Arc::clone(&s.active().unwrap().doc);
        match action {
            "disable" => {
                s.execute("extension.enable", json!({"id":PLUGIN,"enabled":false})).unwrap();
            }
            "remove" => {
                s.execute("extension.remove", json!({"id":PLUGIN})).unwrap();
            }
            "replace" => {
                let dir = tempfile::tempdir().unwrap();
                let path = dir.path().join("replacement.pcext");
                std::fs::write(&path, extension_bytes()).unwrap();
                s.execute("extension.install", json!({"path":path,"replace":true})).unwrap();
            }
            "attach" => {
                let dir = tempfile::tempdir().unwrap();
                s.extensions.attach_folder(dir.path().join("Plugins")).unwrap();
            }
            _ => panic!("unknown plugin management fixture: {action}"),
        }
        assert!(s.plugin_preview(id).is_none(), "{action}");
        assert!(result.upgrade().is_none(), "result retained after {action}");
        assert!(Arc::ptr_eq(&source, &s.active().unwrap().doc));
    }
}

fn install_other(s: &mut Session) -> &'static str {
    let id = "org.example.preview-other";
    let mut donor = session();
    let mut installed = donor.extensions.installed.remove(PLUGIN).unwrap();
    installed.extension = Arc::new(Extension::load(&extension_bytes_for(id)).unwrap());
    s.extensions.installed.insert(id.into(), installed);
    id
}

fn stage_other(s: &mut Session, id: &str) -> u64 {
    let source = Arc::clone(&s.active().unwrap().doc);
    let revision = s.active().unwrap().revision;
    let extension = Arc::clone(&s.extensions.installed[id].extension);
    let binding = PreviewBinding::for_installed(s, id, &extension, None).unwrap();
    let token = reserve(s, source.id, &binding).unwrap();
    stage(s, token, Arc::clone(&source), revision, (*source).clone(), binding).unwrap();
    token
}

#[test]
fn independent_plugins_and_sources_keep_their_own_unapplied_comparisons() {
    let mut s = session();
    let other = install_other(&mut s);
    let first = staged(&mut s, 1);
    let second = stage_other(&mut s, other);
    assert!(s.plugin_preview(first).is_some());
    assert!(s.plugin_preview(second).is_some());
    assert_eq!(s.plugin_previews().count(), 2);
    s.execute("file.new", json!({"width":8,"height":6})).unwrap();
    let third = staged(&mut s, 1);
    assert_eq!(s.plugin_preview_for(PLUGIN).unwrap().id, third);
    s.set_active(0);
    assert_eq!(s.plugin_preview_for(PLUGIN).unwrap().id, first);
    s.execute("image.pluginPreview.discard", json!({"preview":second})).unwrap();
    assert!(s.plugin_preview(first).is_some());
    assert!(s.plugin_preview(third).is_some());
    s.forget_plugin_preview(PLUGIN);
    assert!(s.plugin_preview(first).is_none());
    assert!(s.plugin_preview(third).is_none());
}

#[test]
fn exhausted_memory_or_count_never_evicts_an_unrelated_result() {
    let mut s = session();
    let other = install_other(&mut s);
    let first = staged(&mut s, 1);
    let source = s.active().unwrap().doc.id;
    let extension = Arc::clone(&s.extensions.installed[other].extension);
    let binding = PreviewBinding::for_installed(&s, other, &extension, None).unwrap();
    assert!(reserve_with_budget(&mut s, source, &binding, MAX_PREVIEW_BYTES).is_err());
    assert!(s.plugin_preview(first).is_some());
    for _ in 1..MAX_PREVIEWS {
        s.execute("file.new", json!({"width":8,"height":6})).unwrap();
        staged(&mut s, 1);
    }
    s.execute("file.new", json!({"width":8,"height":6})).unwrap();
    let source = s.active().unwrap().doc.id;
    let extension = Arc::clone(&s.extensions.installed[PLUGIN].extension);
    let binding = PreviewBinding::for_installed(&s, PLUGIN, &extension, None).unwrap();
    assert!(reserve(&mut s, source, &binding).is_err());
    assert_eq!(s.plugin_previews().count(), MAX_PREVIEWS);
    assert!(s.plugin_preview(first).is_some());
    s.execute("image.pluginPreview.discard", json!({"preview":first})).unwrap();
    assert!(reserve(&mut s, source, &binding).is_ok());
}

#[test]
fn failed_or_cancelled_work_releases_its_pending_memory_on_next_admission() {
    let mut s = session();
    let source = s.active().unwrap().doc.id;
    let extension = Arc::clone(&s.extensions.installed[PLUGIN].extension);
    let binding = PreviewBinding::for_installed(&s, PLUGIN, &extension, None).unwrap();
    let pending = reserve_with_budget(&mut s, source, &binding, MAX_PREVIEW_BYTES).unwrap();
    let active = lifetime(&mut s, pending).unwrap();
    assert_eq!(s.plugin_preview_usage()["pending"], 1);
    drop(active);
    let other = install_other(&mut s);
    let second = stage_other(&mut s, other);
    assert!(!s.extensions.previews.pending.contains_key(&pending));
    assert!(s.plugin_preview(second).is_some());
}
