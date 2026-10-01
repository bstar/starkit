#!/usr/bin/env python3
"""Exercise real Kitty pixels and terminal input; owns only its private instance.

Requires Pillow, a built terminal-graphics example, Kitty >= 0.49, and the
configured Electron runtime. Run on a desktop or under xvfb-run on Linux.
"""
import argparse
import json
import os
from pathlib import Path
import signal
import socket
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
    parser.add_argument("--renderer-failure", action="store_true")
    parser.add_argument("--pointer-xdotool", help="Verify actual X11 pointer input")
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
            *(["-o", "linux_display_server=x11"] if args.pointer_xdotool else []),
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
            time.sleep(3)
            window = json.loads(rc("ls"))[0]["tabs"][0]["windows"][0]
            # --hold introduces a Kitty shell parent. Use the actual demo,
            # not window.pid, to prove creation and removal of its socket.
            frontend = next(p for p in window["foreground_processes"]
                            if p["cmdline"] == [args.example, "--interactive"])
            session_path = Path.home() / ".local/starkit/graphical" / f"demo-{frontend['pid']}.sock"
            assert session_path.exists(), "Demo controller never created its session"
            assert any("/main.cjs" in " ".join(p["cmdline"])
                       and "electron" in " ".join(p["cmdline"]).lower()
                       for p in window["foreground_processes"]), \
                "No Electron pixel renderer; the cell fallback does not prove this gate"
            initial = capture("initial")
            # These colors occur only in the generated image, not panel chrome.
            colors = Image.frombytes("RGB", initial[0], initial[1]).getcolors(
                initial[0][0] * initial[0][1])
            counts = {color: count for count, color in colors}
            assert counts.get((166, 227, 161), 0) > 100, "Preview ridge did not render"
            assert counts.get((137, 180, 250), 0) > 100, "Preview sky did not render"
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
            if args.pointer_xdotool:
                tool = args.pointer_xdotool
                window_id = subprocess.check_output([
                    tool, "search", "--onlyvisible", "--pid", str(child.pid),
                    "--name", "STAR-KIT-INTERACTIVE-PROOF",
                ], timeout=5).decode().splitlines()[0]
                before_pointer = capture("before-pointer")
                subprocess.run([
                    tool, "windowfocus", "--sync", window_id,
                    "mousemove", "--window", window_id,
                    str(initial[0][0] // 8), str(initial[0][1] // 2),
                    "click", "1",
                ], check=True, timeout=5)
                time.sleep(.5)
                assert capture("pointer") != before_pointer, \
                    "Pointer selection did not change rendered pixels"
            rc("action", "--match", "id:1", "change_font_size", "current", "+2")
            time.sleep(.7)
            zoomed = capture("zoomed")
            assert zoomed != selected, "Font resize did not repaint"
            if args.renderer_failure:
                # Select only the Electron main process owned by this window.
                renderer = next(p for p in window["foreground_processes"]
                                if "/main.cjs" in " ".join(p["cmdline"])
                                and "--type=" not in " ".join(p["cmdline"]))
                os.kill(renderer["pid"], signal.SIGTERM)
                deadline = time.monotonic() + 15
                while json.loads(rc("ls"))[0]["tabs"][0]["windows"][0]["in_alternate_screen"]:
                    assert time.monotonic() < deadline, "Renderer loss did not restore the terminal"
                    time.sleep(.1)
                assert session_path.exists(), "Renderer loss destroyed the controller session"
                with socket.socket(socket.AF_UNIX) as connection:
                    connection.settimeout(5)
                    connection.connect(str(session_path))
                    with connection.makefile("rwb", buffering=0) as stream:
                        def send(message):
                            stream.write((json.dumps(message) + "\n").encode())
                        send({"type": "hello", "version": 1, "client": "owned-renderer-loss-proof",
                              "viewport": {"columns": 100, "rows": 40, "width": 1200,
                                           "height": 800, "generation": 1}})
                        while True:
                            raw = stream.readline(16 * 1024 * 1024 + 1)
                            assert raw and len(raw) <= 16 * 1024 * 1024, "Invalid controller response"
                            message = json.loads(raw)
                            if message["type"] == "scene":
                                scene = message["scene"]
                                assert scene["interaction"] == 5, "Navigation state was lost"
                                assert any(c.get("marked") for c in scene["components"]), "Marks were lost"
                                send({"type": "input", "id": 1, "revision": scene["revision"],
                                      "generation": 1, "input": {"kind": "key", "code": "char:q",
                                                                "modifiers": 0}})
                                break
                (output / "renderer-loss.txt").write_bytes(rc("get-text", "--match", "id:1"))
            else:
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
                "renderer_loss_preserved_session": args.renderer_failure,
                "shared_image_pixels": True,
                "pointer_pixels_changed": bool(args.pointer_xdotool),
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
