#!/usr/bin/env python3
"""Private Kitty proof: live engine pixels, resize, frontend loss and reattachment."""
import argparse
import json
import os
from pathlib import Path
import signal
import struct
import subprocess
import tempfile
import time

p=argparse.ArgumentParser(description=__doc__)
p.add_argument("--kitty",required=True)
p.add_argument("--kitten",required=True)
p.add_argument("--example",required=True)
p.add_argument("--output",required=True)
p.add_argument("--display-server",default="x11",choices=["x11","wayland"])
a=p.parse_args()
output=Path(a.output).resolve();output.mkdir(parents=True,exist_ok=True)
with tempfile.TemporaryDirectory(prefix="kit-live-proof-") as private:
    runtime=Path(private)/"runtime";runtime.mkdir(mode=0o700)
    address=f"unix:{private}/kitty.sock"
    fault=Path(private)/"discard_images.py"
    fault.write_text('''from kittens.tui.handler import result_handler
def main(args): pass
@result_handler(no_ui=True)
def handle_result(args, result, target_window_id, boss):
    boss.window_id_map[target_window_id].screen.erase_in_display(2, False)
''')
    env={**os.environ,"XDG_RUNTIME_DIR":str(runtime)}
    if a.display_server=="wayland" and os.environ.get("WAYLAND_DISPLAY"):
        env["WAYLAND_DISPLAY"]=str(Path(os.environ["XDG_RUNTIME_DIR"])/os.environ["WAYLAND_DISPLAY"])
    log=(output/"kitty.log").open("w")
    child=None
    def launch(*args):
        return subprocess.Popen([a.kitty,"--config","/dev/null","--listen-on",address,"-o","allow_remote_control=yes","-o",f"linux_display_server={a.display_server}","-o","initial_window_width=100c","-o","initial_window_height=40c","--title","STAR-KIT-LIVE-PROOF",a.example,*args],env=env,stdout=log,stderr=log)
    def rc(*args):
        return subprocess.check_output([a.kitten,"@","--to",address,*args],stderr=subprocess.PIPE,timeout=10)
    def pixels(name):
        path=output/f"{name}.png";deadline=time.monotonic()+30
        while time.monotonic()<deadline:
            assert child.poll() is None,"Private Kitty exited; inspect kitty.log"
            try:
                rc("screenshot","--match","id:1",str(path))
                width,height=struct.unpack(">II",path.read_bytes()[16:24])
                raw=subprocess.check_output(["ffmpeg","-v","error","-i",str(path),"-f","rawvideo","-pix_fmt","rgb24","pipe:1"],stderr=subprocess.PIPE)
                # Both independent producers use blue=110 and changing red/green.
                left=sum(raw[i+2]==110 and raw[i]!=raw[i+1] for y in range(height//5,3*height//5) for i in range((y*width+width//10)*3,(y*width+4*width//10)*3,3))
                right=sum(raw[i+2]==110 and raw[i]!=raw[i+1] for y in range(height//5,3*height//5) for i in range((y*width+6*width//10)*3,(y*width+9*width//10)*3,3))
                if left>10000 and right>10000:return width,height,raw
            except (subprocess.SubprocessError,FileNotFoundError):pass
            time.sleep(.1)
        raise AssertionError("Both live canvases did not become visible")
    try:
        child=launch();first=pixels("initial");time.sleep(.2);second=pixels("animated")
        assert first[2]!=second[2],"Live engine pixels did not advance"
        # Discard terminal placements while both engines continue animating.
        # The static footer must recover too: merely updating active canvases
        # can otherwise leave quiet controls missing indefinitely.
        footer=second[2][-second[0]*24*3:]
        rc("kitten","--match","id:1",str(fault))
        deadline=time.monotonic()+6
        while True:
            recovered=pixels("display-recovered")
            if recovered[2][-recovered[0]*24*3:]==footer:break
            assert time.monotonic()<deadline,"Static artwork did not recover alongside live canvases"
            time.sleep(.1)
        rc("send-key","--match","id:1","ctrl+l")
        pixels("all-layers-refreshed")
        if a.display_server=="x11":
            rc("resize-os-window","--action","resize","--unit","pixels","--width","900","--height","600")
            resized=pixels("resized");assert resized[:2]!=first[:2],"Resize did not change output geometry"
        else:
            # A tiled Wayland compositor owns outer window size. Font zoom changes
            # terminal rows/columns and emits the same viewport resize contract.
            before=json.loads(rc("ls"))[0]["tabs"][0]["windows"][0]
            rc("set-font-size","18");time.sleep(.3);pixels("resized")
            after=json.loads(rc("ls"))[0]["tabs"][0]["windows"][0]
            assert (before["columns"],before["lines"])!=(after["columns"],after["lines"]),"Terminal grid did not resize"
        sessions=list((runtime/"starkit-live-canvas").glob("*.args.json"));assert len(sessions)==1
        session=sessions[0].name.removesuffix(".args.json")
        child.terminate();child.wait(timeout=10);time.sleep(.3)
        assert sessions[0].exists(),"Frontend loss destroyed controller state"
        child=launch("--attach",session);pixels("reattached")
        rc("send-key","--match","id:1","q");child.wait(timeout=15)
        deadline=time.monotonic()+10
        while sessions[0].exists() and time.monotonic()<deadline:time.sleep(.05)
        assert not sessions[0].exists(),"Controller did not clean up on exit"
        print("Kitty live canvases: independent animation, terminal-loss recovery, refresh, resize, frontend loss, reattach and cleanup passed")
    finally:
        if child is not None and child.poll() is None:child.terminate();child.wait(timeout=10)
        # Terminate only controllers using this test's private runtime directory on failure.
        for proc in Path("/proc").iterdir():
            if not proc.name.isdigit():continue
            try:
                cmd=(proc/"cmdline").read_bytes()
                environ=(proc/"environ").read_bytes()
                if b"--serve" in cmd and str(runtime).encode() in environ:os.kill(int(proc.name),signal.SIGTERM)
            except (OSError,PermissionError):pass
