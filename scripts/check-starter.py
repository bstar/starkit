#!/usr/bin/env python3
"""Build the standalone starter outside KIT; optionally use the candidate source."""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

p = argparse.ArgumentParser(description=__doc__)
p.add_argument("--candidate", action="store_true")
p.add_argument("--release", action="store_true", help="Build the interactive preview in release mode")
p.add_argument("--artifacts", type=Path, help="Retain gallery PNG and cell text for visual review")
a = p.parse_args()
kit = Path(__file__).resolve().parents[1]
with tempfile.TemporaryDirectory(prefix="starkit-external-") as directory:
    root = Path(directory)
    shutil.copytree(kit / "examples/starter", root / "app", ignore=shutil.ignore_patterns("target", "Cargo.lock"))
    root = root / "app"
    if a.candidate:
        manifest = root / "Cargo.toml"
        text = manifest.read_text()
        import re
        text = re.sub(r'starkit = \{[^}]+\}', 'starkit = { path = ' + json.dumps(str(kit)) + ', default-features = false }', text)
        manifest.write_text(text)
    env = dict(os.environ)
    env.setdefault("CARGO_TARGET_DIR", str(kit / "target" / "external-starter"))
    def cargo(*args, **kwargs):
        return subprocess.run(["cargo", *args], cwd=root, env=env, check=True, **kwargs)
    feature = ("--features", "native") if env.get("STAR_KIT_STARTER_NO_MEDIA") == "1" else ("--all-features",)
    # Dependency patches are not inherited from KIT. Guard the standalone
    # manifest so copying it cannot silently restore the crashing 0.29 parser.
    parser_source = cargo("tree", "-i", "crossterm", capture_output=True, text=True).stdout
    assert "d4e9929f" in parser_source, "Starter lost the zero-coordinate parser fix"
    cargo("check", "--all-targets", *feature)
    cargo("clippy", "--all-targets", *feature, "--", "-D", "warnings", "-A", "dead_code")
    output = cargo("run", "--quiet", "--", "--inspect", capture_output=True, text=True)
    report = json.loads(output.stdout)
    assert report["layout"]["fits"] and report["focus"]["active"] == "workspace"
    actions = [{"action": "resize", "columns": 35, "rows": 20}, {"action": "step", "forward": True}]
    traces = [cargo("run", "--quiet", "--", "--replay", input=json.dumps(actions), capture_output=True, text=True).stdout for _ in range(2)]
    assert traces[0] == traces[1] and len(traces[0].splitlines()) == 2
    cells = cargo("run", "--quiet", "--", "--gallery", capture_output=True, text=True).stdout
    assert "LIBRARY" in cells and "Filter" in cells and "[ Play ]" in cells
    # Leave both advertised preview paths graphical, even after minimal runs.
    cargo("build", "--quiet", "--features", "native")
    cargo("run", "--quiet", *(["--release"] if a.release else []), "--features", "native", "--", "--png", str(root / "starter.png"))
    assert (root / "starter.png").stat().st_size > 1000
    if a.artifacts:
        a.artifacts.mkdir(parents=True, exist_ok=True)
        shutil.copy2(root / "starter.png", a.artifacts / "gallery.png")
        (a.artifacts / "gallery.txt").write_text(cells)
    print("External starter: minimal diagnostics, deterministic replay, native/media builds and PNG passed")
