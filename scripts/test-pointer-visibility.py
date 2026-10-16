#!/usr/bin/env python3
"""Exercise Kitty pointer lifetime/ownership without hiding the real cursor."""
import ctypes
from pathlib import Path
import runpy
import sys
from types import ModuleType, SimpleNamespace as NS
import unittest
from unittest.mock import patch


class Callable:
    def __init__(self, fn):
        self.fn = fn
    def __call__(self, *args):
        return self.fn(*args)


class PointerTests(unittest.TestCase):
    def setUp(self):
        self.world = NS(active=True, over=9, x=400, y=400, balance=0, dead=False)
        self.window = NS(id=3, os_window_id=7, geometry=NS(left=0, top=40, right=2400, bottom=1560))
        self.boss = NS(active_window=self.window, window_id_map={3: self.window})
        self.timers = {}
        handler = ModuleType('kittens.tui.handler')
        handler.result_handler = lambda **kwargs: lambda fn: fn
        fast = ModuleType('kitty.fast_data_types')
        fast.add_timer = self.add_timer
        fast.remove_timer = lambda timer: self.timers.pop(timer)
        fast.get_os_window_size = lambda _: dict(width=1200, height=800, framebuffer_width=2400, framebuffer_height=1600)
        utils = ModuleType('kitty.utils')
        utils.platform_window_id = lambda _: 9
        self.modules = patch.dict(sys.modules, {'kittens.tui.handler': handler, 'kitty.fast_data_types': fast, 'kitty.utils': utils})
        self.modules.start()
        self.helper = runpy.run_path(str(Path(__file__).resolve().parents[1] / 'integrations/kitty/fullscreen.py'))
        library = NS(objc_getClass=Callable(lambda x: x.decode()), sel_registerName=Callable(lambda x: x.decode()))
        self.patches = [
            patch('os.uname', return_value=NS(sysname='Darwin')),
            patch('os.kill', side_effect=self.kill),
            patch.object(ctypes, 'CDLL', return_value=library),
            patch.object(ctypes, 'CFUNCTYPE', side_effect=lambda *args: lambda binding: self.dispatch),
        ]
        for item in self.patches:
            item.start()

    def tearDown(self):
        for item in reversed(self.patches):
            item.stop()
        self.modules.stop()

    def kill(self, pid, signal):
        self.assertEqual((pid, signal), (123, 0))
        if self.world.dead:
            raise ProcessLookupError()

    def add_timer(self, callback, interval, repeat):
        self.assertTrue(repeat)
        self.assertAlmostEqual(interval, 1 / 30)
        self.timers[1] = callback
        return 1

    def dispatch(self, obj, selector, *args):
        values = {
            'sharedApplication': 'app', 'isActive': self.world.active,
            'mouseLocation': NS(x=1, y=1), 'windowNumberAtPoint:belowWindowWithWindowNumber:': self.world.over,
            'windows': 'windows', 'count': 1, 'objectAtIndex:': 'native-window',
            'windowNumber': 9, 'mouseLocationOutsideOfEventStream': NS(x=self.world.x, y=self.world.y),
        }
        if selector in ('hide', 'unhide'):
            self.world.balance += 1 if selector == 'hide' else -1
            self.assertIn(self.world.balance, (0, 1), 'Cocoa hide/unhide must stay balanced')
            return None
        return values[selector]

    def enable(self):
        self.helper['pointer_visibility'](self.boss, self.window, True, 123)

    def tick(self):
        for callback in tuple(self.timers.values()):
            callback(1)

    def test_moves_focus_and_window_edges_restore_without_unbalanced_hides(self):
        self.enable()
        self.assertEqual(self.world.balance, 1)
        self.tick()
        self.assertEqual(self.world.balance, 1)
        self.world.over = 99
        self.tick()
        self.assertEqual(self.world.balance, 0)
        self.world.over = 9
        self.tick()
        self.assertEqual(self.world.balance, 1)
        self.world.active = False
        self.tick()
        self.assertEqual(self.world.balance, 0)
        self.world.active = True
        self.world.y = 799  # Kitty title/tab area, outside the terminal pane.
        self.tick()
        self.assertEqual(self.world.balance, 0)
        self.world.y = 400
        self.tick()
        self.assertEqual(self.world.balance, 1)
        self.helper['pointer_visibility'](self.boss, self.window, False)
        self.assertEqual(self.world.balance, 0)
        self.assertFalse(self.timers)

    def test_client_crash_restores_and_removes_watchdog(self):
        self.enable()
        self.world.dead = True
        self.tick()
        self.assertEqual(self.world.balance, 0)
        self.assertFalse(self.timers)

    def test_tab_switch_and_closed_window_restore(self):
        self.enable()
        self.boss.active_window = None
        self.tick()
        self.assertEqual(self.world.balance, 0)
        self.boss.active_window = self.window
        self.tick()
        self.assertEqual(self.world.balance, 1)
        self.boss.window_id_map.clear()
        self.tick()
        self.assertEqual(self.world.balance, 0)
        self.assertFalse(self.timers)

    def test_repeat_enable_disable_does_not_leak_timers_or_hide_counts(self):
        for _ in range(5):
            self.enable()
            self.enable()
            self.assertEqual(len(self.timers), 1)
            self.assertEqual(self.world.balance, 1)
            self.helper['pointer_visibility'](self.boss, self.window, False)
            self.helper['pointer_visibility'](self.boss, self.window, False)
            self.assertFalse(self.timers)
            self.assertEqual(self.world.balance, 0)


if __name__ == '__main__':
    unittest.main()
