#!/usr/bin/env python3
"""Exercise the public UI gallery in an isolated real Kitty window.

Checks native pixels, commands, modal isolation, cell/graphical switching and
restoration after image loss. Leaves screenshots and a machine-readable report.
"""
import argparse
import json
import os
import re
from pathlib import Path
import socket
import struct
import subprocess
import tempfile
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--kitty', required=True)
parser.add_argument('--kitten', required=True)
parser.add_argument('--binary', required=True)
parser.add_argument('--output', required=True)
parser.add_argument('--window-width', default='100c')
parser.add_argument('--window-height', default='40c')
parser.add_argument('--display-server', choices=['wayland', 'x11'], default='wayland')
args = parser.parse_args()
output = Path(args.output).resolve()
output.mkdir(parents=True, exist_ok=True)
with tempfile.TemporaryDirectory(prefix='kit-ui-proof-') as temporary:
    private = Path(temporary)
    runtime = private / 'runtime'
    runtime.mkdir(mode=0o700)
    env = {**os.environ, 'XDG_RUNTIME_DIR': str(runtime),
           'XDG_CONFIG_HOME': str(private / 'config'), 'STAR_GRAPHICS_SYSTEM_FONTS': '0',
           'STAR_KIT_STARTER_DIR': str(output / 'profile')}
    env.pop('NO_COLOR', None)
    env.pop('STAR_KIT_ASCII', None)
    if args.display_server == 'wayland' and os.environ.get('WAYLAND_DISPLAY'):
        env['WAYLAND_DISPLAY'] = str(Path(os.environ['XDG_RUNTIME_DIR']) / os.environ['WAYLAND_DISPLAY'])
    address = f'unix:{private}/kitty.sock'
    fault = private / 'discard_images.py'
    fault.write_text('''from kittens.tui.handler import result_handler
def main(args): pass
@result_handler(no_ui=True)
def handle_result(args, result, target_window_id, boss):
    boss.window_id_map[target_window_id].screen.erase_in_display(2, False)
''')
    log = (output / 'kitty.log').open('w')
    child = subprocess.Popen([args.kitty, '--config', '/dev/null', '--listen-on', address,
                              '-o', 'allow_remote_control=yes', '-o', f'linux_display_server={args.display_server}',
                              '-o', f'initial_window_width={args.window_width}', '-o', f'initial_window_height={args.window_height}',
                              '--title', 'STAR-KIT-UI-PROOF', str(Path(args.binary).resolve()), '--native'],
                             env=env, stdout=log, stderr=log)

    def rc(*command):
        return subprocess.check_output([args.kitten, '@', '--to', address, *command],
                                       stderr=subprocess.PIPE, timeout=10)

    def pixels(name):
        path = output / f'{name}.png'
        rc('screenshot', '--match', 'id:1', str(path))
        dimensions = struct.unpack('>II', path.read_bytes()[16:24])
        raw = subprocess.check_output(['ffmpeg', '-v', 'error', '-i', str(path),
                                       '-vf', 'scale=480:-1:flags=neighbor',
                                       '-f', 'rawvideo', '-pix_fmt', 'rgb24', 'pipe:1'],
                                      stderr=subprocess.PIPE, timeout=10)
        return dimensions, raw

    def difference(a, b):
        if a[0] != b[0]:
            return 1.0
        return sum(abs(x - y) > 8 for x, y in zip(a[1], b[1])) / len(a[1])

    def wait_for(check, message, timeout=20):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            assert child.poll() is None, 'Private gallery exited; inspect kitty.log'
            try:
                result = check()
                if result:
                    return result
            except (subprocess.SubprocessError, FileNotFoundError):
                pass
            time.sleep(.1)
        raise AssertionError(message)

    def stable(name):
        previous = pixels(name)
        for _ in range(30):
            time.sleep(.1)
            current = pixels(name)
            if difference(previous, current) < .0001:
                return current
            previous = current
        raise AssertionError(f'{name} did not settle')

    report = {}
    try:
        def native_ready():
            current = pixels('initial')
            # Classic panel faces occupy most of both panes. This distinguishes
            # an actual native frame from the terminal's connecting message.
            return current if current[1].count(bytes([48, 51, 64])) > 10000 else None
        initial = wait_for(native_ready, 'Native application frame did not appear')
        # Crossterm 0.29.0 panics on zero SGR coordinates in debug builds and
        # wraps to 65535 in release. Inject both axes, then prove later input
        # still works. Include cursor reports, which have the same upstream bug.
        rc('send-text', '--match', 'id:1', '\x1b[<35;0;1M\x1b[<35;1;0M\x1b[0;0R')
        rc('send-key', '--match', 'id:1', 'ctrl+p')
        played = wait_for(lambda: (p if difference(initial, p := pixels('playing')) > .0001 else None),
                          'Registered play command did not change pixels')
        played = stable('playing')
        report['command_pixels'] = True
        report['zero_coordinate_input'] = True
        rc('send-key', '--match', 'id:1', 'f1')
        modal = wait_for(lambda: (p if difference(played, p := pixels('help')) > .0001 else None),
                         'Shared help dialog did not appear')
        modal = stable('help')
        rc('send-key', '--match', 'id:1', 'ctrl+p')
        time.sleep(.25)
        assert difference(modal, stable('modal-isolation')) < .0001, 'Background command escaped the modal'
        report['modal_isolation'] = True
        rc('send-key', '--match', 'id:1', 'escape')
        wait_for(lambda: difference(played, pixels('modal-closed')) < .0001,
                 'Closing modal did not restore the same application state')
        rc('send-key', '--match', 'id:1', 'alt+t')
        wait_for(lambda: difference(played, pixels('theme-dialog')) > .0001,
                 'Theme picker did not appear')
        previous_theme = stable('theme-dialog')
        latencies = []
        deliveries = []
        application_log = output / 'profile/cache/starkit-starter.log'
        for index in range(4):
            log_offset = len(application_log.read_text())
            started = time.monotonic()
            rc('send-key', '--match', 'id:1', 'down')
            changed = wait_for(lambda: (p if difference(previous_theme, p := pixels(f'theme-{index}')) > .0001 else None),
                               'Theme selection did not redraw', timeout=3)
            latencies.append(time.monotonic() - started)
            # PNG screenshots and Python comparisons are expensive at large
            # window sizes. Gate the app's measured scene-to-display time,
            # independently of screenshot acquisition/decoding.
            acknowledgements = re.findall(r'presentation acknowledged[^\n]*delivery_ms=(\d+)',
                                          application_log.read_text()[log_offset:])
            assert acknowledgements, 'Theme changed no acknowledged scene'
            deliveries.append(max(map(int, acknowledgements)))
            previous_theme = stable(f'theme-{index}')
        assert max(deliveries) < 250, f'Theme presentation is too slow: {deliveries} ms'
        report['theme_delivery_ms'] = deliveries
        report['theme_observed_seconds'] = latencies
        rc('send-key', '--match', 'id:1', 'page_down')
        wait_for(lambda: difference(previous_theme, pixels('theme-page')) > .0001,
                 'Page Down did not move the selected theme')
        paged_theme = stable('theme-page')
        report['list_page_navigation'] = True
        rc('send-key', '--match', 'id:1', 'enter')
        wait_for(lambda: difference(paged_theme, pixels('theme-closed')) > .0001,
                 'Enter did not activate the selected theme and close its dialog')
        stable('theme-closed')
        report['list_activation'] = True
        rc('send-key', '--match', 'id:1', 'f9')
        wait_for(lambda: b'LIBRARY' in rc('get-text', '--match', 'id:1'),
                 'Cell presentation did not show compact border headings')
        pixels('cells')
        report['cell_switch'] = True
        rc('send-key', '--match', 'id:1', 'f9')
        wait_for(lambda: b'LIBRARY' not in rc('get-text', '--match', 'id:1'),
                 'Graphical presentation did not return')
        before = stable('before-image-loss')
        rc('kitten', '--match', 'id:1', str(fault))
        wait_for(lambda: difference(before, pixels('recovered')) < .001,
                 'Quiet UI did not recover erased terminal placements', timeout=8)
        report['display_recovery'] = True
        rc('send-key', '--match', 'id:1', 'ctrl+l')
        wait_for(lambda: difference(before, pixels('repainted')) < .001,
                 'Shared repaint changed or lost the application frame')
        report['repaint'] = True
        rc('send-key', '--match', 'id:1', 'ctrl+q')
        child.wait(timeout=10)
        assert child.returncode == 0, f'Private terminal exited with {child.returncode}'
        report['clean_exit'] = True
        (output / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
        print(json.dumps(report, sort_keys=True))
    finally:
        # A failed proof must not leave its persistent private controller alive.
        for path in runtime.glob('starkit-kit-gallery/*.sock'):
            try:
                with socket.socket(socket.AF_UNIX) as connection:
                    connection.settimeout(1)
                    connection.connect(str(path))
                    viewport = {'columns': 100, 'rows': 40, 'width': 1200, 'height': 800, 'generation': 1}
                    messages = [{'type': 'Hello', 'version': 1, 'viewport': viewport, 'client': 'ui-proof-cleanup'}]
                    for index, code in enumerate(['escape', 'char:q'], 1):
                        messages.append({'type': 'Input', 'id': index, 'revision': 0, 'generation': 1,
                                         'input': {'kind': 'key', 'code': code, 'modifiers': 2 if index == 2 else 0}})
                    # Wire enum names use snake_case.
                    for message in messages:
                        message['type'] = message['type'].lower()
                        connection.sendall((json.dumps(message) + '\n').encode())
                    time.sleep(.1)
            except (OSError, subprocess.SubprocessError):
                pass
        if child.poll() is None:
            child.terminate()
            try:
                child.wait(timeout=5)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait()
        log.close()
