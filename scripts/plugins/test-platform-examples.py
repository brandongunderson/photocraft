#!/usr/bin/env python3
"""Build independent Plugin API examples and test their actual packages in the host."""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[2]
EXAMPLES = (
    ("layer-organizer", "Layer-Organizer-1.0.0.pcplugin"),
    ("document-inspector", "Document-Inspector-1.0.0.pcplugin"),
)

SDK_HANDLER = '''#![deny(unsafe_code)]
use photocraft_plugin_sdk::{api::*, serde_json::json};
pub mod bindings { include!(concat!(env!("OUT_DIR"), "/bindings.rs")); }

fn handle(event: &EventRequest) -> Result<PanelResponse> {
    match event.event.as_str() {
        "inspect" => Ok(PanelResponse {
            state: json!({"pending": true}),
            requests: vec![LayersRead { limit: Some(128), ..Default::default() }.request("layers")?],
            ..Default::default()
        }),
        "host_response" => {
            let snapshot: LayersSnapshot = event.reply("layers")?.decode()?;
            let document = snapshot.document.ok_or_else(|| ApiError::new(ErrorCode::NotFound, "No document"))?;
            let layer = snapshot.layers.first().ok_or_else(|| ApiError::new(ErrorCode::NotFound, "No layers"))?;
            Ok(PanelResponse {
                state: json!({"checked": true}),
                requests: vec![LayersEdit {
                    document, expected_revision: snapshot.revision.ok_or_else(|| ApiError::new(ErrorCode::Conflict, "No revision"))?,
                    edits: vec![LayerEdit { layer: layer.handle.clone(), name: Some("SDK checked".into()), visible: None }],
                }.request("edit")?],
                ..Default::default()
            })
        }
        _ => Ok(PanelResponse { state: event.state.clone(), ..Default::default() }),
    }
}
photocraft_plugin_sdk::export_plugin!(bindings::MANIFEST_JSON, handle);
'''


def cargo_executable():
    executable = shutil.which("cargo")
    if executable is None:
        raise FileNotFoundError("Cargo was not found on PATH")
    # Rustup dispatches by the invoked filename. Resolving a Unix cargo symlink
    # changes that filename to rustup and turns Cargo options into rustup options.
    return os.path.abspath(executable)


def sdk_check(temporary, offline):
    cargo = [cargo_executable()]
    locked = ["--locked"] + (["--offline"] if offline else [])
    subprocess.run(cargo + ["build", "-p", "photocraft-plugin"] + locked, cwd=ROOT, check=True)
    metadata = json.loads(subprocess.check_output(
        cargo + ["metadata", "--no-deps", "--format-version", "1"] + locked, cwd=ROOT, text=True))
    tool = Path(metadata["target_directory"]) / "debug" / ("photocraft-plugin.exe" if os.name == "nt" else "photocraft-plugin")
    authoring = temporary / "authoring.json"
    plugin_id = "org.example.sdk-validation"
    authoring.write_text(json.dumps({
        "formatVersion": 1,
        "descriptor": {
            "apiVersion": 1, "id": plugin_id, "name": "SDK Validation", "version": "1.0.0",
            "permissions": {"documentRead": False, "commands": []},
            "capabilities": {"required": [{"id": "host.document.read", "minVersion": 1}, {"id": "host.document.edit", "minVersion": 1}], "optional": []},
            "contributes": {"commands": [{"id": plugin_id + ".inspect", "label": "Inspect", "event": "inspect", "params": {}, "needsDocument": True}]},
            "panel": {"fields": [], "buttons": [{"id": "inspect", "label": "Inspect"}]},
        },
        "package": {"packageVersion": 2, "module": "plugin.pcext"},
    }), encoding="utf-8")
    project = temporary / "sdk-source"
    exported = temporary / "sdk-export"
    environment = os.environ.copy()
    environment["PHOTOCRAFT_PLUGIN_CARGO"] = cargo[0]
    environment["PHOTOCRAFT_PLUGIN_BUILD_PATH"] = os.environ["PATH"]
    subprocess.run([str(tool), "new", str(authoring), str(project)], cwd=temporary, env=environment, check=True)
    (project / "src/lib.rs").write_text(SDK_HANDLER, encoding="utf-8")
    subprocess.run([str(tool), "project", str(project)], cwd=temporary, env=environment, check=True)
    guest_environment = environment.copy()
    guest_environment["CARGO_TARGET_DIR"] = str(temporary / "sdk-check-target")
    subprocess.run(cargo + ["test"] + locked, cwd=project, env=guest_environment, check=True)
    subprocess.run(cargo + ["clippy", "--all-targets"] + locked + ["--", "-D", "warnings"], cwd=project, env=guest_environment, check=True)
    subprocess.run([str(tool), "build-export", str(project), str(exported)], cwd=temporary, env=environment, check=True)
    package = exported / "dist/exported.pcplugin"
    subprocess.run([str(tool), "check", str(package)], cwd=temporary, env=environment, check=True)
    environment["PHOTOCRAFT_PLUGIN_SDK_PACKAGE"] = str(package)
    subprocess.run(cargo + ["test", "-p", "photocraft-plugin"] + locked + [
        "tests::independently_built_sdk_guest_edits_through_the_real_broker_and_undoes_atomically",
        "--", "--ignored", "--exact"], cwd=ROOT, env=environment, check=True)


def test_cargo_proxy():
    """Exercise SDK creation/export orchestration without invoking Cargo or the host."""
    from unittest.mock import patch

    with tempfile.TemporaryDirectory(prefix="photocraft-cargo-proxy-") as directory:
        temporary = Path(directory)
        proxy = temporary / "bin" / "cargo"
        resolved = proxy.with_name("rustup")
        original_resolve = Path.resolve
        # Simulate the Unix proxy target on every platform, including Windows
        # machines where creating a symlink would need an extra privilege.
        def resolve(path, *args, **kwargs):
            return resolved if path == proxy else original_resolve(path, *args, **kwargs)

        def run(command, **kwargs):
            if command[1] == "new":
                (Path(command[3]) / "src").mkdir(parents=True)
            return subprocess.CompletedProcess(command, 0)

        metadata = json.dumps({"target_directory": str(temporary / "host-target")})
        with patch.object(shutil, "which", return_value=str(proxy)), \
                patch.object(Path, "resolve", resolve), \
                patch.object(subprocess, "run", side_effect=run) as invoked, \
                patch.object(subprocess, "check_output", return_value=metadata):
            sdk_check(temporary, offline=True)
        commands = [call.args[0] for call in invoked.call_args_list]
        for call in invoked.call_args_list:
            command = call.args[0]
            assert command[0] != str(resolved)
            if command[1] in ("new", "project", "build-export"):
                executable = call.kwargs["env"]["PHOTOCRAFT_PLUGIN_CARGO"]
                assert executable == str(proxy) and Path(executable).is_absolute()
        assert any(command[0] == str(proxy) and "--offline" in command for command in commands)
        with patch.object(shutil, "which", return_value="relative/bin/cargo"):
            assert Path(cargo_executable()).is_absolute()
    print("Cargo proxy regression passed.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--offline", action="store_true")
    parser.add_argument("--sdk-only", action="store_true", help="only check the generated standalone SDK guest")
    parser.add_argument("--export-parent", type=Path, help="temporary build parent outside the editor checkout")
    parser.add_argument("--host-features", default="heif", help="host Cargo features; pass an empty value to use defaults only")
    parser.add_argument("--self-test", action="store_true", help="test Cargo proxy handling without building or running plugins")
    args = parser.parse_args()
    if args.self_test:
        test_cargo_proxy()
        return
    with tempfile.TemporaryDirectory(prefix="photocraft-platform-", dir=args.export_parent) as temporary:
        if Path(temporary).resolve().is_relative_to(ROOT):
            raise ValueError("Use --export-parent outside the editor checkout for independent builds")
        sdk_check(Path(temporary), args.offline)
        if args.sdk_only:
            return
        exported = Path(temporary) / "examples"
        guest_environment = os.environ.copy()
        guest_environment["CARGO_TARGET_DIR"] = str(Path(temporary) / "target")
        for project, package in EXAMPLES:
            destination = exported / project
            shutil.copytree(
                ROOT / "examples" / "extensions" / project,
                destination,
                ignore=shutil.ignore_patterns("target", "dist", ".git", "__pycache__"),
            )
            command = [sys.executable, str(destination / "build.py")]
            if args.offline:
                command.append("--offline")
            subprocess.run(command, cwd=destination, env=guest_environment, check=True)
            artifact = destination / "dist" / package
            if not artifact.is_file() or artifact.stat().st_size == 0:
                raise ValueError(f"Missing built plugin package: {artifact}")

        host_environment = os.environ.copy()
        host_environment["PHOTOCRAFT_PLATFORM_EXAMPLES"] = str(exported)
        command = ["cargo", "test", "--workspace", "--locked"]
        if args.host_features:
            command.extend(["--features", args.host_features])
        command.extend(["--test", "plugin_platform_examples"])
        if args.offline:
            command.append("--offline")
        command.extend(["--", "--ignored"])
        subprocess.run(command, cwd=ROOT, env=host_environment, check=True)


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"Plugin integration failed: {error}", file=sys.stderr)
        raise SystemExit(1) from error
