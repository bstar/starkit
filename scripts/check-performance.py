#!/usr/bin/env python3
"""Broad release-mode regression budgets; printed timings are the actual evidence."""
import argparse
import os
import re
import subprocess
import tempfile
from pathlib import Path
p = argparse.ArgumentParser(description=__doc__)
p.add_argument("--layout-us", type=float, default=10)
p.add_argument("--frame-ms", type=float, default=100)
a = p.parse_args()
kit = Path(__file__).resolve().parents[1]
def run(*args, env=None):
    output = subprocess.check_output(["cargo", "run", "--quiet", "--release", *args], cwd=kit, env=env, text=True)
    print(output, end="", flush=True)
    return output
layout = run("--no-default-features", "--example", "responsive-panes", "--", "--bench")
assert float(re.search(r"([\d.]+) microseconds/layout", layout)[1]) <= a.layout_us, "Pane solver budget exceeded"
with tempfile.TemporaryDirectory(prefix="starkit-performance-") as directory:
    frame = run("--features", "terminal-graphics", "--example", "terminal-graphics", "--", str(Path(directory) / "frame.png"), env={**os.environ, "STAR_GRAPHICS_BENCH_FRAMES": "120"})
assert float(re.search(r"scene-to-frame p95 ([\d.]+) ms", frame)[1]) <= a.frame_ms, "Native frame budget exceeded"
assert float(re.search(r"later mean payload ([\d.]+) bytes", frame)[1]) > 0, "Benchmark did not change rendered pixels"
print(f"Regression gates passed: layout <= {a.layout_us:g} us, native p95 <= {a.frame_ms:g} ms")
