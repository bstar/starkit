#!/usr/bin/env python3
"""Exercise SGR pixel motion through the real frontend and relay on a private PTY.

Build the isolated controller fixture first:
    nix develop -c cargo build --example pixel-mouse-probe --features terminal-graphics
"""
import os, pty, fcntl, termios, struct, subprocess, tempfile, time, select, json, re
from pathlib import Path
binary = str(Path(__file__).resolve().parents[1] / 'target/debug/examples/pixel-mouse-probe')
with tempfile.TemporaryDirectory(prefix='staramp-pixel-mouse-') as temp:
    root = Path(temp)
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 40, 100, 1000, 800))
    env = os.environ.copy()
    env.update(TERM='xterm-kitty', PIXEL_MOUSE_ROOT=str(root / 'session'), PIXEL_MOUSE_RECORD=str(root / 'inputs.jsonl'), STAR_GRAPHICS_SYSTEM_FONTS='0', XDG_RUNTIME_DIR=str(root / 'runtime'), HOME=str(root))
    for key in ['TMUX', 'STY', 'SSH_CONNECTION', 'SSH_TTY', 'KITTY_WINDOW_ID', 'KITTY_LISTEN_ON']:
        env.pop(key, None)

    def session():
        os.setsid()
        fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
    child = subprocess.Popen([binary], stdin=slave, stdout=slave, stderr=slave, env=env, preexec_fn=session)
    os.close(slave)
    output = bytearray()
    pending = bytearray()

    def pump(duration):
        until = time.monotonic() + duration
        while time.monotonic() < until:
            if not select.select([master], [], [], min(0.05, max(0, until - time.monotonic())))[0]:
                continue
            try:
                chunk = os.read(master, 65536)
            except OSError:
                return
            if not chunk:
                return
            output.extend(chunk)
            pending.extend(chunk)
            for match in list(re.finditer(b'\\x1b_G([^\\x1b]*)\\x1b\\\\', pending)):
                data = match.group(1)
                if b'a=q' in data:
                    ident = re.search(b'(?:^|,)i=(\\d+)', data)
                    if ident:
                        os.write(master, b'\x1b_Gi=' + ident.group(1) + b';OK\x1b\\')
            for query, reply in [(b'\x1b[16t', b'\x1b[6;20;10t'), (b'\x1b[14t', b'\x1b[4;800;1000t'), (b'\x1b[c', b'\x1b[?62;4;22c'), (b'\x1b[5n', b'\x1b[0n'), (b'\x1b[?u', b'\x1b[?0u')]:
                if query in pending:
                    os.write(master, reply)
                    pending[:] = pending.replace(query, b'')
            if len(pending) > 4096:
                del pending[:-512]
    try:
        deadline = time.monotonic() + 12
        while b'\x1b[?1016h' not in output:
            pump(0.1)
            assert child.poll() is None, bytes(output[-2000:])
            assert time.monotonic() < deadline, bytes(output[:3000])
        pump(1.5)

        def send(cb, x, y, end='M'):
            os.write(master, f'\x1b[<{cb};{x + 1};{y + 1}{end}'.encode())
            pump(0.12)
        send(0, 203, 405)
        for x, y in [(204, 406), (205, 407), (206, 408), (207, 409)]:
            send(32, x, y)
        send(0, 207, 409, 'm')
        pump(0.4)
        rows = [json.loads(line) for line in (root / 'inputs.jsonl').read_text().splitlines()]
        points = [row['pixel'] for row in rows if row['action'] == 'drag']
        assert points == [[204, 406], [205, 407], [206, 408], [207, 409]], rows
        assert all((row['x'] == 20 and row['y'] == 20 for row in rows)), rows
        os.write(master, b'q')
        pump(1)
        child.wait(timeout=5)
        assert child.returncode == 0, bytes(output[-2000:])
        assert b'\x1b[?1016l' in output, 'Pixel mode not restored'
        print(json.dumps({'real_frontend_pty': True, 'drag_pixels': points, 'same_logical_cell': [20, 20], 'mode_restored': True}))
    finally:
        if child.poll() is None:
            child.kill()
            child.wait()
        os.close(master)
