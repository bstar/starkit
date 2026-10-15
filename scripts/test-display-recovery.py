#!/usr/bin/env python3
"""Fault-inject missing Kitty images into a private AMP or REEL desktop.

Checks actual terminal pixels, not just rasterizer output. No user sessions,
configuration, playback queues, or projects are opened or restarted.
"""
import argparse
import json
import os
from pathlib import Path
import signal
import shutil
import struct
import subprocess
import tempfile
import time

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--kitty', required=True)
p.add_argument('--kitten', required=True)
p.add_argument('--binary', required=True)
p.add_argument('--app', required=True, choices=['amp', 'reel'])
p.add_argument('--output', required=True)
p.add_argument('--project', help='Optional REEL fixture copied into the private test directory')
p.add_argument('--edit', action='store_true', help='Test REEL paused timeline video instead of its live desktop')
a = p.parse_args()
output = Path(a.output).resolve()
output.mkdir(parents=True, exist_ok=True)
with tempfile.TemporaryDirectory(prefix='star-display-recovery-') as temporary:
    private = Path(temporary)
    runtime = private / 'runtime'
    runtime.mkdir(mode=0o700)
    original_runtime = os.environ.get('XDG_RUNTIME_DIR', f'/run/user/{os.getuid()}')
    env = {**os.environ, 'XDG_RUNTIME_DIR': str(runtime),
           'WAYLAND_DISPLAY': str(Path(original_runtime) / os.environ.get('WAYLAND_DISPLAY', 'wayland-1')),
           'PULSE_SERVER': os.environ.get('PULSE_SERVER', f'unix:{original_runtime}/pulse/native'),
           'STARAMP_DIR': str(private / 'amp-data'),
           'STARAMP_CONFIG_DIR': str(private / 'amp-config'),
           'STARREEL_DIR': str(private / 'reel-data'), 'STAR_GRAPHICS_SYSTEM_FONTS': '0',
           'DBUS_SESSION_BUS_ADDRESS': 'unix:path=/nonexistent/star-display-test-bus'}
    # This helper only runs inside the test's private Kitty process. It models
    # loss of terminal placements, including unchanged UI and live layers.
    fault = private / 'discard_images.py'
    fault.write_text('''from kittens.tui.handler import result_handler
def main(args): pass
@result_handler(no_ui=True)
def handle_result(args, result, target_window_id, boss):
    window = boss.window_id_map[target_window_id]
    window.screen.erase_in_display(2, False)
''')
    address = f'unix:{private}/kitty.sock'
    command = [str(Path(a.binary).resolve())]
    if a.app == 'amp':
        subprocess.run(command + ['scrobble', 'status'], env=env, check=True,
                       stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        command += ['graphical']
    else:
        project = private / 'demo.starreel'
        if a.project:
            shutil.copytree(a.project, project, symlinks=True)
        command += ['--project', str(project)]
    log = (output / 'kitty.log').open('w')
    child = subprocess.Popen([a.kitty, '--config', '/dev/null', '--listen-on', address,
                              '-o', 'allow_remote_control=yes', '-o', 'linux_display_server=wayland',
                              '-o', 'background=#1f1f28', '--title', 'STAR-DISPLAY-RECOVERY-PROOF',
                              *command], env=env, stdout=log, stderr=log)

    def rc(*args):
        try:
            return subprocess.check_output([a.kitten, '@', '--to', address, *args],
                                           stderr=subprocess.PIPE, timeout=10)
        except subprocess.CalledProcessError as error:
            raise RuntimeError(error.stderr.decode()) from error

    def screenshot(name):
        assert child.poll() is None, 'Private application exited; inspect kitty.log'
        path = output / f'{name}.png'
        rc('screenshot', '--match', 'id:1', str(path))
        dimensions = struct.unpack('>II', path.read_bytes()[16:24])
        raw = subprocess.check_output(['ffmpeg', '-v', 'error', '-i', str(path),
                                       '-f', 'rawvideo', '-pix_fmt', 'rgb24', 'pipe:1'],
                                      stderr=subprocess.PIPE, timeout=10)
        return dimensions, raw

    def difference(before, after):
        if before[0] != after[0]:
            return 1.0
        return sum(abs(x - y) > 8 for x, y in zip(before[1], after[1])) / len(before[1])

    def stable(name):
        deadline = time.monotonic() + 20
        previous = screenshot(name)
        while time.monotonic() < deadline:
            time.sleep(.25)
            current = screenshot(name)
            if difference(previous, current) < .001:
                return current
            previous = current
        raise AssertionError('Private desktop did not settle')

    def recover(name, reference, timeout):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            current = screenshot(name)
            if difference(reference, current) < .001:
                return
            time.sleep(.05)
        raise AssertionError(f'{name}: missing graphics did not recover within {timeout}s')

    result = {'app': a.app}
    try:
        deadline = time.monotonic() + 20
        while True:
            try:
                rc('ls')
                break
            except (subprocess.SubprocessError, RuntimeError):
                assert child.poll() is None, 'Private Kitty failed to start'
                assert time.monotonic() < deadline, 'Private Kitty socket unavailable'
                time.sleep(.1)
        time.sleep(2)
        if a.edit:
            assert a.app == 'reel' and a.project, '--edit requires a REEL fixture'
            rc('send-key', '--match', 'id:1', 'e')
            time.sleep(1)
        baseline = stable('initial')
        for name, event, timeout in [
            ('automatic', None, 6),
            ('focus', ('send-text', '--match', 'id:1', '\x1b[O\x1b[I'), 2),
            ('refresh', ('send-key', '--match', 'id:1', 'ctrl+l'), 2),
        ]:
            rc('kitten', '--match', 'id:1', str(fault))
            damaged = screenshot(f'{name}-damaged')
            changed = difference(baseline, damaged)
            assert changed > .02, f'{name}: fault did not actually remove graphics ({changed})'
            if event:
                rc(*event)
            started = time.monotonic()
            recover(name, baseline, timeout)
            result[name] = {'damaged_fraction': round(changed, 4),
                            'recovered_seconds': round(time.monotonic() - started, 3)}
        # Modal artwork must also survive terminal loss, and close cleanly.
        if a.app == 'amp':
            rc('send-text', '--match', 'id:1', '?')
        else:
            rc('send-key', '--match', 'id:1', 'alt+t')
        modal = stable('modal-before')
        assert difference(baseline, modal) > .001, 'Modal did not open'
        rc('kitten', '--match', 'id:1', str(fault))
        recover('modal-restored', modal, 6)
        rc('send-key', '--match', 'id:1', 'escape')
        closed = stable('modal-closed')
        assert difference(closed, modal) > .001, 'Modal did not close'
        rc('kitten', '--match', 'id:1', str(fault))
        recover('after-modal-restored', closed, 6)
        result['modal'] = True
        rc('send-key', '--match', 'id:1', 'q')
        child.wait(timeout=15)
        assert child.returncode == 0, 'Private frontend did not exit cleanly'
        result['clean_exit'] = True
        (output / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
        print(json.dumps(result))
    finally:
        if child.poll() is None:
            child.terminate()
            child.wait(timeout=10)
        # Only controllers whose environment contains this unique test runtime.
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
