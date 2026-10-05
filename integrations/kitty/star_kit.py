"""STAR/KIT terminal frontend transport. Install as a Kitty watcher.

Runs only ~/.local/bin/star-kit-terminal, never a command or path from the host.
Each parent terminal has an isolated nonce and private socket. All UI, input,
rendering and drop capabilities belong to the native Rust frontend.
"""
import base64
import json
import os
import re
import shutil
import socket
import tempfile
import time
from collections import deque
from pathlib import Path

MAX_MESSAGE = 16 * 1024 * 1024
CLIENT = Path.home() / '.local/bin/star-kit-terminal'
sessions = {}


class Session:
    def __init__(self, boss, window, nonce):
        from kitty.fast_data_types import add_timer
        from kitty.launch import launch, parse_launch_args
        self.boss, self.window, self.nonce = boss, window, nonce
        self.root = tempfile.mkdtemp(prefix='star-kit-terminal-')
        self.listener = socket.socket(socket.AF_UNIX)
        path = os.path.join(self.root, nonce)
        self.listener.bind(path)
        os.chmod(path, 0o600)
        self.listener.listen(1)
        self.listener.setblocking(False)
        self.peer = None
        self.authenticated = False
        self.incoming = bytearray()
        self.frame = bytearray()
        self.output = deque()
        self.output_size = 0
        self.closed = False
        self.overlay = None
        self.deadline = time.monotonic() + 20
        self.timer = add_timer(self.tick, .005, True)
        try:
            opts, args = parse_launch_args(['--type=overlay-main', '--copy-colors', '--copy-env',
                                          '--next-to', f'id:{window.id}'])
            self.overlay = launch(boss, opts, [str(CLIENT), path],
                                  target_tab=window.tabref(), force_target_tab=True,
                                  rc_from_window=window)
            if self.overlay is None:
                raise RuntimeError('Native frontend could not start')
        except Exception:
            self.close()
            raise

    def reply(self, value):
        self.window.write_to_child(f'STAR_KIT_BRIDGE {self.nonce} {value}\n'.encode())

    def close(self):
        if self.closed:
            return
        self.closed = True
        from kitty.fast_data_types import remove_timer
        remove_timer(self.timer)
        if self.peer:
            self.peer.close()
        self.listener.close()
        shutil.rmtree(self.root, ignore_errors=True)
        if self.overlay and self.overlay.id in self.boss.window_id_map:
            self.boss.mark_window_for_close(self.overlay)
        self.reply('gone')
        if sessions.get(self.window.id) is self:
            sessions.pop(self.window.id, None)

    def data(self, op, data):
        if op == 'data':
            chunk = base64.b64decode(data, validate=True)
            if len(chunk) > 3072 or len(self.frame) + len(chunk) > MAX_MESSAGE:
                raise ValueError('Terminal bridge frame exceeds limit')
            self.frame.extend(chunk)
        elif op == 'end':
            # Reject raw newlines/escape injection into the local transport.
            json.loads(self.frame)
            if b'\n' in self.frame or self.output_size + len(self.frame) + 1 > MAX_MESSAGE * 2:
                raise ValueError('Terminal bridge queue exceeds limit')
            value = bytes(self.frame) + b'\n'
            self.output.append(memoryview(value))
            self.output_size += len(value)
            self.frame.clear()

    def tick(self, _):
        try:
            if self.closed:
                return
            if not self.peer:
                try:
                    self.peer, _ = self.listener.accept()
                    self.peer.setblocking(False)
                except BlockingIOError:
                    if time.monotonic() > self.deadline:
                        self.close()
                    return
            # Bound work on Kitty's main loop, leaving time for pointer events.
            for _ in range(8):
                try:
                    chunk = self.peer.recv(65536)
                except BlockingIOError:
                    break
                if not chunk:
                    self.close()
                    return
                self.incoming.extend(chunk)
                if len(self.incoming) > MAX_MESSAGE + 128:
                    raise ValueError('Native frontend input exceeds limit')
            for _ in range(32):
                end = self.incoming.find(b'\n')
                if end < 0:
                    break
                line = bytes(self.incoming[:end])
                del self.incoming[:end + 1]
                if not self.authenticated:
                    if line != f'STAR_KIT_CLIENT {self.nonce}'.encode():
                        raise ValueError('Invalid terminal bridge capability')
                    self.authenticated = True
                else:
                    json.loads(line)
                    self.reply(line.decode())
            if self.authenticated:
                for _ in range(8):
                    if not self.output:
                        break
                    try:
                        written = self.peer.send(self.output[0])
                    except BlockingIOError:
                        break
                    if not written:
                        self.close()
                        return
                    self.output_size -= written
                    if written == len(self.output[0]):
                        self.output.popleft()
                    else:
                        self.output[0] = self.output[0][written:]
                        break
        except Exception as error:
            print(f'STAR/KIT terminal bridge: {error}', flush=True)
            self.close()


def on_set_user_var(boss, window, data):
    if data.get('key') != 'star_kit' or not data.get('value'):
        return
    try:
        if len(data['value']) > 8192:
            return
        value = json.loads(data['value'])
        nonce, op = value.get('nonce', ''), value.get('op')
        if value.get('v') != 1 or not isinstance(nonce, str) or not re.fullmatch('[0-9a-f]{48}', nonce):
            return
        current = sessions.get(window.id)
        if op == 'probe':
            if CLIENT.is_file() and os.access(CLIENT, os.X_OK):
                window.write_to_child(f'STAR_KIT_BRIDGE {nonce} ready\n'.encode())
        elif op == 'start':
            if current:
                # A different nonce cannot replace a live session.
                return
            sessions[window.id] = Session(boss, window, nonce)
        elif current and current.nonce == nonce:
            if op == 'stop':
                current.close()
            else:
                current.data(op, value.get('data'))
    except Exception as error:
        print(f'STAR/KIT terminal bridge: {error}', flush=True)
        if sessions.get(window.id):
            sessions[window.id].close()
        else:
            nonce = locals().get('nonce')
            if isinstance(nonce, str) and re.fullmatch('[0-9a-f]{48}', nonce):
                window.write_to_child(f'STAR_KIT_BRIDGE {nonce} gone\n'.encode())


def on_close(boss, window, data):
    current = sessions.get(window.id)
    if current:
        current.close()
