#!/usr/bin/env python3
"""Build exported API-2 examples and test their actual packages in the host."""
import argparse
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


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--offline", action="store_true")
    parser.add_argument("--host-features", default="heif", help="host Cargo features; pass an empty value to use defaults only")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="photocraft-platform-") as temporary:
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
