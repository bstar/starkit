#!/usr/bin/env python3
"""Exercise a STAR command guide through a private real Kitty frontend.

No real projects, listening history, configuration or playback are opened.
"""
import argparse
import json
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import time

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--kitty', required=True)
p.add_argument('--kitten', required=True)
p.add_argument('--binary', required=True)
p.add_argument('--app', choices=['amp', 'reel'], required=True)
p.add_argument('--output', required=True)
a = p.parse_args()
output = Path(a.output).resolve()
output.mkdir(parents=True, exist_ok=True)
with tempfile.TemporaryDirectory(prefix='star-help-frontend-') as directory:
    private = Path(directory)
    runtime = private / 'runtime'
    runtime.mkdir(mode=0o700)
    original = os.environ.get('XDG_RUNTIME_DIR', f'/run/user/{os.getuid()}')
    env = {**os.environ, 'XDG_RUNTIME_DIR': str(runtime),
           'WAYLAND_DISPLAY': str(Path(original) / os.environ.get('WAYLAND_DISPLAY', 'wayland-1')),
           'PULSE_SERVER': os.environ.get('PULSE_SERVER', f'unix:{original}/pulse/native'),
           'STARAMP_DIR': str(private / 'amp-data'), 'STARAMP_CONFIG_DIR': str(private / 'amp-config'),
           'STARREEL_DIR': str(private / 'reel-data'), 'STAR_GRAPHICS_SYSTEM_FONTS': '0',
           'DBUS_SESSION_BUS_ADDRESS': 'unix:path=/nonexistent/star-help-test-bus'}
    command = [str(Path(a.binary).resolve())]
    if a.app == 'amp':
        subprocess.run(command + ['scrobble', 'status'], env=env, check=True,
                       stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        command += ['graphical']
    else:
        command += ['--project', str(private / 'demo.starreel')]
    address = f'unix:{private}/kitty.sock'
    log = (output / 'kitty.log').open('w')
    child = subprocess.Popen([a.kitty, '--config', '/dev/null', '--listen-on', address,
                              '-o', 'allow_remote_control=yes', '-o', 'linux_display_server=wayland',
                              '-o', 'background=#1f1f28', '--title', 'STAR-HELP-PROOF', *command],
                             env=env, stdout=log, stderr=log)

    def rc(*args):
        try:
            return subprocess.check_output([a.kitten, '@', '--to', address, *args],
                                           stderr=subprocess.PIPE, timeout=10)
        except subprocess.CalledProcessError as error:
            raise RuntimeError(error.stderr.decode()) from error

    def screenshot(name):
        assert child.poll() is None, 'Private frontend exited; inspect kitty.log'
        time.sleep(.35)
        path = output / f'{name}.png'
        rc('screenshot', '--match', 'id:1', str(path))
        return subprocess.check_output(['ffmpeg', '-v', 'error', '-i', str(path), '-f', 'rawvideo',
                                        '-pix_fmt', 'rgb24', 'pipe:1'], stderr=subprocess.PIPE, timeout=10)

    def difference(before, after):
        return sum(abs(x-y) > 8 for x,y in zip(before, after)) / max(len(before), 1)

    def key(name):
        rc('send-key', '--match', 'id:1', name)

    def text(value):
        rc('send-text', '--match', 'id:1', '--', value)

    result = {'app': a.app}
    try:
        deadline = time.monotonic() + 20
        while True:
            try:
                rc('ls')
                break
            except (subprocess.SubprocessError, RuntimeError):
                assert child.poll() is None and time.monotonic() < deadline
                time.sleep(.1)
        time.sleep(2)
        baseline = screenshot('initial')
        text('?')
        opened = screenshot('help')
        assert difference(baseline, opened) > .02, 'Question mark did not open help'
        query = 'history' if a.app == 'amp' else 'microhpone'
        text(query)
        filtered = screenshot('filtered')
        assert difference(opened, filtered) > .005, 'Typing did not filter commands'
        key('ctrl+u')
        text('category:playback pause')
        scoped = screenshot('scoped-search')
        assert difference(filtered, scoped) > .003, 'Field filter did not update results'
        key('ctrl+u')
        text('zzzzzzzz')
        empty = screenshot('empty')
        assert difference(filtered, empty) > .003, 'Empty result view did not update'
        key('ctrl+u')
        key('tab')
        category = screenshot('category')
        assert difference(opened, category) > .003, 'Category navigation did not update'
        key('ctrl+f')
        key('ctrl+u')
        # Q and Space must become query text, and Delete must edit the field.
        text('q ')
        key('delete')
        screenshot('modal-input')
        key('escape')
        closed = screenshot('closed')
        assert difference(opened, closed) > .02, 'Escape did not close help'
        key('f1')
        reopened = screenshot('f1')
        assert difference(opened, reopened) < .003, 'F1 did not reopen the same guide'
        key('escape')
        key('q')
        child.wait(timeout=15)
        assert child.returncode == 0, 'Private frontend did not exit cleanly'
        result.update(search=True, scoped_search=True, empty_results=True, categories=True, modal_input=True,
                      escape=True, f1=True, clean_exit=True)
        (output / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
        print(json.dumps(result), flush=True)
    finally:
        if child.poll() is None:
            child.terminate()
            try:
                child.wait(timeout=5)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait(timeout=5)
        # Match the unique private runtime, never a user's controller.
        for proc in Path('/proc').iterdir():
            if not proc.name.isdigit():
                continue
            try:
                cmd = (proc / 'cmdline').read_bytes()
                environ = (proc / 'environ').read_bytes()
                if b'--serve' in cmd and f'XDG_RUNTIME_DIR={runtime}'.encode() + b'\0' in environ:
                    os.kill(int(proc.name), signal.SIGTERM)
            except (OSError, PermissionError):
                pass
        log.close()
