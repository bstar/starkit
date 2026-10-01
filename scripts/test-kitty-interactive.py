#!/usr/bin/env python3
"""Exercise real Kitty pixels and terminal input; owns only its private instance.

Requires Pillow, a built terminal-graphics example, Kitty >= 0.49, and the
configured Electron runtime. Run on a desktop or under xvfb-run on Linux.
"""
import argparse
import json
from pathlib import Path
import subprocess
import tempfile
import time

from PIL import Image


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--kitty", required=True)
    parser.add_argument("--kitten", required=True)
    parser.add_argument("--example", required=True)
    parser.add_argument("--output", required=True)
    args = parser.parse_args()
    output = Path(args.output).resolve()
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="kit-pixels-") as private:
        address = f"unix:{private}/rc.sock"
        log = (output / "kitty.log").open("w")
        child = subprocess.Popen([
            args.kitty, "--hold", "--config", "/dev/null", "--listen-on", address,
            "-o", "allow_remote_control=yes", "-o", "remember_window_size=no",
            "-o", "initial_window_width=100c", "-o", "initial_window_height=40c",
            "--title", "STAR-KIT-INTERACTIVE-PROOF", args.example, "--interactive",
        ], stdout=log, stderr=log)
        session_path = None

        def rc(*command):
            try:
                return subprocess.check_output([
                    args.kitten, "@", "--to", address, *command,
                ], stderr=subprocess.PIPE, timeout=15)
            except subprocess.CalledProcessError as error:
                print(error.stderr.decode(errors="replace"), flush=True)
                raise

        def capture(name):
            path = output / f"{name}.png"
            rc("screenshot", "--match", "id:1", str(path))
            with Image.open(path) as image:
                rgb = image.convert("RGB")
                assert len(rgb.getcolors(rgb.width * rgb.height) or []) > 32, \
                    "Terminal screenshot has no graphical content"
                return rgb.size, rgb.tobytes()

        try:
            deadline = time.monotonic() + 30
            while True:
                assert child.poll() is None, "Kitty exited during startup; inspect kitty.log"
                try:
                    windows = json.loads(rc("ls"))
                    window = windows[0]["tabs"][0]["windows"][0]
                    break
                except (subprocess.SubprocessError, IndexError):
                    assert time.monotonic() < deadline, "Kitty remote control startup timed out"
                    time.sleep(.1)
            # The demo names its session from its frontend process ID.
            session_path = Path.home() / ".local/starkit/graphical" / f"demo-{window['pid']}.sock"
            time.sleep(3)
            initial = capture("initial")
            rc("send-text", "--match", "id:1", "jjjjj ")
            time.sleep(.7)
            selected = capture("selected")
            assert initial != selected, "Keyboard navigation did not change rendered pixels"
            rc("send-text", "--match", "id:1", "c")
            time.sleep(.5)
            menu = capture("menu")
            assert menu != selected, "Menu did not change rendered pixels"
            rc("send-text", "--match", "id:1", "\x1b")
            time.sleep(.4)
            rc("action", "--match", "id:1", "change_font_size", "current", "+2")
            time.sleep(.7)
            zoomed = capture("zoomed")
            assert zoomed != selected, "Font resize did not repaint"
            rc("send-text", "--match", "id:1", "q")
            deadline = time.monotonic() + 15
            while session_path.exists() and time.monotonic() < deadline:
                time.sleep(.1)
            assert not session_path.exists(), "Controller session leaked after normal exit"
            rc("close-window", "--match", "id:1")
            child.wait(timeout=15)
            (output / "result.json").write_text(json.dumps({
                "keyboard_pixels_changed": True, "menu_pixels_changed": True,
                "font_resize_pixels_changed": True, "clean_exit": True,
                "initial_pixels": initial[0], "zoomed_pixels": zoomed[0],
            }, indent=2) + "\n")
            print("Kitty interactive pixel/input/resize/exit proof passed")
        except Exception:
            try:
                (output / "terminal-error.txt").write_bytes(rc("get-text", "--match", "id:1"))
            except subprocess.SubprocessError:
                pass
            raise
        finally:
            if child.poll() is None:
                try:
                    rc("send-text", "--match", "id:1", "q")
                    child.wait(timeout=5)
                except subprocess.SubprocessError:
                    child.terminate()
                    child.wait(timeout=5)
            log.close()


if __name__ == "__main__":
    main()
