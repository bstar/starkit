"""Transport isolation and backpressure checks; no desktop input required."""
import importlib.util
import json
import socket
import sys
import types
import unittest
from collections import deque
from pathlib import Path
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('star_kit', Path(__file__).with_name('star_kit.py'))
bridge = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bridge)


class Window:
    id = 1

    def __init__(self):
        self.replies = []

    def write_to_child(self, data):
        self.replies.append(data)

    def tabref(self):
        return None


class Tests(unittest.TestCase):
    def tearDown(self):
        bridge.sessions.clear()

    def test_nonce_is_scoped_to_the_parent_window(self):
        window = Window()
        commands = []
        current = types.SimpleNamespace(nonce='a' * 48, data=lambda *args: commands.append(args))
        bridge.sessions[window.id] = current
        for target, nonce in [(window, 'b' * 48), (types.SimpleNamespace(id=2), 'a' * 48)]:
            bridge.on_set_user_var(None, target, {'key': 'star_kit', 'value': json.dumps(
                {'v': 1, 'op': 'end', 'nonce': nonce})})
        self.assertEqual(commands, [])
        bridge.on_set_user_var(None, window, {'key': 'star_kit', 'value': json.dumps(
            {'v': 1, 'op': 'end', 'nonce': 'a' * 48})})
        self.assertEqual(commands, [('end', None)])

    def test_slow_host_backpressures_input_until_acknowledgement(self):
        state = bridge.Session.__new__(bridge.Session)
        state.peer, peer = socket.socketpair()
        try:
            state.peer.setblocking(False)
            state.window = Window()
            state.nonce = 'a' * 48
            state.closed = False
            state.authenticated = True
            state.pending_inputs = {}
            state.pending_input_bytes = 0
            messages = [json.dumps({'type': 'input', 'id': i, 'input': {'kind': 'paste', 'text': 'x' * 200000}}).encode()
                        for i in (1, 2)]
            state.incoming = bytearray(b'\n'.join(messages) + b'\n')
            state.frame = bytearray()
            state.output = deque()
            state.output_size = 0
            state.tick(None)
            self.assertEqual(len(state.window.replies), 1)
            self.assertEqual(set(state.pending_inputs), {1})
            state.frame.extend(json.dumps({'type': 'ack', 'id': 1, 'accepted': True}).encode())
            state.data('end', None)
            state.tick(None)
            self.assertEqual(len(state.window.replies), 2)
            self.assertEqual(set(state.pending_inputs), {2})
        finally:
            state.peer.close()
            peer.close()

    def test_remote_start_cannot_select_a_local_command(self):
        calls = []
        overlay = types.SimpleNamespace(id=2)
        boss = types.SimpleNamespace(window_id_map={2: overlay}, mark_window_for_close=lambda _: None)
        window = Window()
        modules = {'kitty': types.ModuleType('kitty'),
                   'kitty.fast_data_types': types.ModuleType('kitty.fast_data_types'),
                   'kitty.launch': types.ModuleType('kitty.launch')}
        modules['kitty.fast_data_types'].add_timer = lambda *args: 1
        modules['kitty.fast_data_types'].remove_timer = lambda _: None
        modules['kitty.launch'].parse_launch_args = lambda _: (None, [])
        modules['kitty.launch'].launch = lambda boss, opts, args, **kw: calls.append(args) or overlay
        with patch.dict(sys.modules, modules):
            bridge.on_set_user_var(boss, window, {'key': 'star_kit', 'value': json.dumps({
                'v': 1, 'nonce': 'a' * 48, 'op': 'start',
                'command': ['arbitrary-remote-command'], 'path': '/arbitrary/remote/path'})})
            self.assertEqual(len(calls), 1)
            self.assertEqual(calls[0][0], str(bridge.CLIENT))
            self.assertTrue(calls[0][1].startswith('/tmp/star-kit-terminal-'))
            self.assertEqual(Path(calls[0][1]).name, 'a' * 48)
            bridge.sessions[window.id].close()


if __name__ == '__main__':
    unittest.main()
