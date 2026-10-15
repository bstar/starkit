#!/usr/bin/env python3
"""Build and exercise the independent graphics-only live engine consumer."""
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile

kit = Path(__file__).resolve().parents[1]
with tempfile.TemporaryDirectory(prefix="starkit-live-consumer-") as directory:
    app = Path(directory) / "app"
    shutil.copytree(kit / "examples/live-canvas", app, ignore=shutil.ignore_patterns("target", "Cargo.lock"))
    manifest = app / "Cargo.toml"
    manifest.write_text(re.sub(r'starkit = \{[^}]+\}', 'starkit = { path = ' + json.dumps(str(kit)) + ', default-features = false, features = ["terminal-graphics"] }', manifest.read_text()))
    env = dict(os.environ)
    env.setdefault("CARGO_TARGET_DIR", str(kit / "target/external-starter"))
    def cargo(*args, **kwargs):
        return subprocess.run(["cargo", *args], cwd=app, env=env, check=True, **kwargs)
    cargo("clippy", "--all-targets", "--", "-D", "warnings", "-A", "dead_code")
    tree = cargo("tree", "--prefix", "none", capture_output=True, text=True).stdout
    assert not re.search(r"(?m)^(ffmpeg|cpal|rtrb)", tree)
    cargo("run", "--quiet", "--", "--self-test")
    scene = json.loads(cargo("run", "--quiet", "--", "--inspect", capture_output=True, text=True).stdout)
    assert sum(c["kind"] == "live_canvas" for c in scene["components"]) == 2
    cargo("run", "--quiet", "--", "--png", str(app / "live.png"))
    assert (app / "live.png").stat().st_size > 1000
    print("Independent live-canvas consumer: two streams, scene, PNG, clippy and graphics-only dependency gate passed")
