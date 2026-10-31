#!/usr/bin/env python3
"""Prove a slow image upload leaves the complete previous Kitty frame visible."""
import argparse
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
from terminal_fixture import TerminalStartup

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--kitty', required=True)
p.add_argument('--kitten', required=True)
p.add_argument('--frames', required=True, type=Path)
p.add_argument('--output', required=True, type=Path)
p.add_argument('--display-server', choices=['wayland', 'x11'], default='x11')
a = p.parse_args()
frames = a.frames.resolve()
output = a.output.resolve()
output.mkdir(parents=True, exist_ok=True)
viewport = json.loads((frames / 'viewport.json').read_text())
with tempfile.TemporaryDirectory(prefix='kit-atomic-refresh-') as directory:
    private = Path(directory)
    env = dict(os.environ)
    for name in ('TMUX', 'STY', 'SSH_CONNECTION', 'SSH_TTY'):
        env.pop(name, None)
    writer = private / 'writer.py'
    writer.write_text('''import sys, time, tty
from pathlib import Path
root, frames = map(Path, sys.argv[1:])
tty.setraw(sys.stdin.fileno())
def wait(name):
    deadline = time.monotonic() + 30
    while not (root / name).exists():
        if time.monotonic() >= deadline: raise SystemExit('Proof driver did not release ' + name)
        time.sleep(.01)
def send(data):
    sys.stdout.buffer.write(data)
    sys.stdout.buffer.flush()
sys.stdout.buffer.write(b'\\x1b[?25l')
send((frames / 'initial.bin').read_bytes())
(root / 'initial').touch()
wait('upload')
data = (frames / 'replacement.bin').read_bytes()
split = data.index(b'\\x1b[?2026h')
half = split // 2
send(data[:half])
(root / 'half').touch()
wait('finish-upload')
send(data[half:split])
(root / 'staged').touch()
wait('commit')
send(data[split:])
(root / 'committed').touch()
wait('quit')
''')
    startup = TerminalStartup(private)
    address = 'unix:' + str(private / 'kitty.sock')
    log = (output / 'kitty.log').open('w')
    child = subprocess.Popen([a.kitty, '--config', '/dev/null', '--listen-on', address,
        '-o', 'allow_remote_control=yes', '-o', f'linux_display_server={a.display_server}',
        '-o', 'remember_window_size=no', '-o', f'initial_window_width={viewport["columns"]}c',
        '-o', f'initial_window_height={viewport["rows"]}c', '--title', 'STAR-KIT-ATOMIC-REFRESH-PROOF',
        *startup.command(sys.executable, str(writer), str(private), str(frames))],
        env=env, stdout=log, stderr=log, start_new_session=True)
    def rc(*args):
        return subprocess.check_output([a.kitten, '@', '--to', address, *args], stderr=subprocess.PIPE, timeout=10)
    def wait(name):
        deadline = time.monotonic() + 20
        while not (private / name).exists():
            assert child.poll() is None, 'Private terminal exited: inspect kitty.log'
            assert time.monotonic() < deadline, 'Missing writer phase ' + name
            time.sleep(.02)
    def screenshot(name, color, absent):
        path = output / (name + '.png')
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            rc('screenshot', '--match', 'id:1', str(path))
            rgb = subprocess.check_output(['ffmpeg', '-v', 'error', '-i', str(path),
                '-f', 'rawvideo', '-pix_fmt', 'rgb24', 'pipe:1'], stderr=subprocess.PIPE, timeout=10)
            total = len(rgb) // 3
            expected = sum(all(abs(rgb[i+j]-color[j]) <= 2 for j in range(3)) for i in range(0,len(rgb),3))
            unexpected = sum(all(abs(rgb[i+j]-absent[j]) <= 2 for j in range(3)) for i in range(0,len(rgb),3))
            if expected > total * .95 and unexpected == 0:
                return
            time.sleep(.05)
        raise AssertionError(f'{name}: partial or incorrect frame ({expected}/{total} expected, {unexpected} wrong-color pixels)')
    old, new = [20,30,40], [200,100,50]
    try:
        startup.release(child, rc, output / 'terminal-ready.png')
        wait('initial')
        screenshot('initial', old, new)
        (private / 'upload').touch()
        wait('half')
        time.sleep(.6)  # Deliberately longer than synchronized-output safety deadlines.
        screenshot('half-upload-old-frame', old, new)
        (private / 'finish-upload').touch()
        wait('staged')
        time.sleep(.6)
        screenshot('fully-uploaded-old-frame', old, new)
        (private / 'commit').touch()
        wait('committed')
        screenshot('committed-new-frame', new, old)
        (private / 'quit').touch()
        child.wait(timeout=10)
        assert child.returncode == 0
        report = dict(partial_upload_invisible=True, staged_upload_invisible=True,
                      complete_frame_swap=True, clean_exit=True)
        (output / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
        print(json.dumps(report, sort_keys=True))
    finally:
        if child.poll() is None:
            os.killpg(child.pid, signal.SIGTERM)
            try: child.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(child.pid, signal.SIGKILL)
                child.wait()
        log.close()
