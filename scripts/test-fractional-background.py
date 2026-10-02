#!/usr/bin/env python3
"""Check actual native pixels for fractional-cell background seams."""
import argparse
import json
from pathlib import Path
import subprocess

from PIL import Image

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--example", required=True)
parser.add_argument("--output", required=True)
args = parser.parse_args()
output = Path(args.output).resolve()
output.mkdir(parents=True, exist_ok=True)
scene = {"revision": 1, "interaction": 1,
         "viewport": {"columns": 97, "rows": 47, "width": 1850, "height": 1998, "generation": 1},
         "background": "#1e1e2e", "foreground": "#ffffff", "accent": "#89b4fa", "border": "#45475a",
         "spans": [{"x": 0, "y": 3, "text": " " * 97, "foreground": "#ffffff",
                    "background": "#667788", "bold": False}], "components": []}
fixture = output / "scene.json"
fixture.write_text(json.dumps(scene))
png = output / "fractional-background.png"
subprocess.run([args.example, "--scene", str(fixture), str(png)], check=True, timeout=15)
with Image.open(png) as source:
    image = source.convert("RGB")
    assert image.size == (1850, 1998)
    # Check every pixel throughout the row, including all cell boundaries.
    # Blank glyphs isolate background coverage from text antialiasing.
    top, bottom = int(3 * 1998 / 47), int(4 * 1998 / 47)
    mismatches = sum(image.getpixel((x, y)) != (102, 119, 136)
                     for y in range(top, bottom) for x in range(1850))
    assert mismatches == 0, f"{mismatches} fractional background pixels have seams"
print("Fractional-cell native background pixels are solid")
