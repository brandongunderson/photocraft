"""Generate the panel command reference from an actual PhotoCraft CLI build."""

import argparse
import json
from pathlib import Path
import subprocess


def read_cli(cli, args):
    result = subprocess.run(
        [str(cli), *args], capture_output=True, text=True, encoding="utf-8", check=True
    )
    return json.loads(result.stdout)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cli", type=Path, required=True)
    parser.add_argument("--out", type=Path)
    args = parser.parse_args()
    cli = args.cli.resolve()
    repo = Path(__file__).resolve().parents[2]
    output = args.out or repo / "docs/plugin-command-reference.md"
    commands = read_cli(cli, ["commands", "--json"])
    reply = read_cli(cli, ["run", "--new", '{"width":8,"height":8}', "--cmd", "extension.commands"])
    allowed = reply["result"]
    descriptions = {item["id"]: item for item in commands}
    lines = [
        "# Panel plugin command reference", "",
        "Generated from this PhotoCraft build's `extension.commands` and CLI command descriptions.",
        f"The current PhotoCraft Plugin API; **{len(allowed['commands'])}** permitted command ids.",
        "The running host remains authoritative. Start with the [handbook](plugin-development.md).", "",
        "## How to use this reference", "",
        "Declare each command id in `permissions.commands`, then return it with an object-valued",
        "`params`. Installation rejects unsupported grants. Runtime authorization and each command's",
        "target, lock, selection, mode and parameter checks still apply. A listed id is not a promise",
        "that it is currently enabled. Parameter descriptions below use the engine's human-readable",
        "notation; they are not literal JSON examples or JSON Schemas. `?` means optional, `=` marks",
        "a documented default, and alternatives/ranges describe accepted values. JSON uses ordinary",
        "objects/arrays/strings/numbers/booleans, without those notation operators.", "",
        "The guest cannot call discovery methods, inspect arbitrary documents, retrieve command",
        "results, interpolate earlier results into later commands, or invoke global `plugin.run`.",
        "Use `plugin.processImage` for its own version-2 packaged native worker or WASM filters;",
        "the host binds the invoking installation, so panel parameters do not supply `id`.",
        "Each returned command keeps normal history behavior; batches are not atomic.", "",
        "Use [`plugin.runTask`](plugin-native-tasks.md) for explicit maintenance in the same installation's",
        "native worker without a document. Like image processing, it must be the sole command in an action.", "",
        "Image processing requires the invoking plugin's compatible installed backend; a grant",
        "alone does not supply one. See [independent image processing](plugin-image-processing.md)",
        "and [package formats](plugin-native-packages.md). The direct CLI description may list",
        "an explicit owner `id`; a panel's generic image request is scoped automatically.", "",
        "## Command descriptions", "",
    ]
    for command_id in allowed["commands"]:
        item = descriptions[command_id]
        lines += [f"### `{command_id}`", "", item["label"], "", "```text", item["params"], "```", ""]
    lines += [
        "## Regenerate", "",
        "From the PhotoCraft repository root, using a CLI built from the same host source:", "",
        "```sh", "python scripts/plugins/generate-command-reference.py --cli target/debug/photocraft-cli", "```", "",
        "On Windows append `.exe` to the CLI path. This script reads the full command catalog and",
        "creates a disposable in-memory 8 × 8 document to ask for the extension allowlist. It does",
        "not install plugins, edit user documents or attach a persistent settings store.", "",
        "After regenerating, review changes against `crates/engine/src/extension_cmds.rs` and the",
        "individual command implementations. New grants require a host API/security review.", "",
    ]
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text("\n".join(lines), encoding="utf-8")
    print(f"Wrote {len(allowed['commands'])} command descriptions to {output}")


if __name__ == "__main__":
    main()
