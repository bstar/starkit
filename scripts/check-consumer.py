#!/usr/bin/env python3
"""Check an isolated tracked consumer tree against a candidate KIT checkout.

Version requirements are explicitly updated in the disposable copy: this is a
migration check, not proof that a breaking candidate satisfies an older release.
The consumer's original manifests, lockfile and local patches are never written.
Run in the consumer's devshell so its native build dependencies are present.
"""
import argparse
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import tomllib


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("consumer", type=Path)
    parser.add_argument("--kit", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--cargo", nargs=argparse.REMAINDER,
                        help="Run another Cargo command in the disposable candidate tree, e.g. build --release --offline")
    # Preserve Cargo's own `--` separator (e.g. strict Clippy flags). argparse
    # otherwise treats it as its global option terminator, even for REMAINDER.
    argv = sys.argv[1:]
    cargo = None
    if "--cargo" in argv:
        index = argv.index("--cargo")
        cargo, argv = argv[index + 1:], argv[:index]
    args = parser.parse_args(argv)
    if cargo is not None:
        args.cargo = cargo
    consumer, kit = args.consumer.resolve(), args.kit.resolve()
    version = tomllib.loads((kit / "Cargo.toml").read_text())["package"]["version"]
    for label, repo in (("consumer", consumer), ("KIT", kit)):
        revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=repo, text=True).strip()
        print(f"{label}: {repo} @ {revision}", flush=True)
    tracked = subprocess.check_output(["git", "ls-files", "-z"], cwd=consumer).split(b"\0")
    with tempfile.TemporaryDirectory(prefix="starkit-consumer-") as directory:
        tree = Path(directory)
        manifests = []
        for raw in tracked:
            if not raw:
                continue
            relative = Path(os.fsdecode(raw))
            source, destination = consumer / relative, tree / relative
            if not source.is_file():
                continue
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source, destination, follow_symlinks=False)
            if relative.name == "Cargo.toml":
                manifests.append(destination)
        replacements = 0
        for manifest in manifests:
            text = manifest.read_text()
            # STAR consumers declare KIT as an inline dependency. Fail rather
            # than silently skipping a future manifest layout we do not know.
            def candidate(match):
                nonlocal replacements
                replacements += 1
                return re.sub(r'version\s*=\s*"[^"]*"', f'version = "{version}"', match[0], count=1)
            text = re.sub(r'(?m)^starkit\s*=\s*\{[^}]*\}', candidate, text)
            manifest.write_text(text)
        if not replacements:
            raise SystemExit("No inline starkit dependency found in consumer")
        # JSON quoting is valid for the TOML string inside Cargo's structured
        # --config argument; no shell evaluates these arguments.
        import json
        patch = f'patch."https://github.com/bstar/starkit".starkit.path={json.dumps(str(kit))}'
        environment = dict(os.environ)
        environment.setdefault("CARGO_TARGET_DIR", str(consumer / "target" / "kit-compat"))
        command = args.cargo or ["check", "--workspace", "--all-targets", "--all-features"]
        if command[0] == "test":
            # A shared target may reuse a binary from a removed disposable
            # tree, including env!("CARGO_MANIFEST_DIR") fixture paths. Force
            # consumer units to rebuild for this tree; dependency caches stay.
            for source in tree.rglob("*.rs"):
                source.touch()
        subprocess.run(["cargo", command[0], "--config", patch, *command[1:]],
                       cwd=tree, env=environment, check=True)


if __name__ == "__main__":
    main()
