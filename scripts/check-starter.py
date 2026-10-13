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
    cargo("check", "--all-targets", *feature)
    cargo("clippy", "--all-targets", *feature, "--", "-D", "warnings", "-A", "dead_code")
    output = cargo("run", "--quiet", "--", "--inspect", capture_output=True, text=True)
    report = json.loads(output.stdout)
    assert report["layout"]["fits"] and report["focus"]["active"] == "workspace"
    actions = [{"action": "resize", "columns": 35, "rows": 20}, {"action": "step", "forward": True}]
    traces = [cargo("run", "--quiet", "--", "--replay", input=json.dumps(actions), capture_output=True, text=True).stdout for _ in range(2)]
    assert traces[0] == traces[1] and len(traces[0].splitlines()) == 2
    cargo("run", "--quiet", "--features", "native", "--", "--png", str(root / "starter.png"))
    assert (root / "starter.png").stat().st_size > 1000
    print("External starter: minimal diagnostics, deterministic replay, native/media builds and PNG passed")
