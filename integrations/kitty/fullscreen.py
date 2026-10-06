"""Fixed STAR/KIT window operation, invoked locally through Kitty remote control.
No host-provided command, identifier, or filesystem path is executed.
"""
import json
import os
import subprocess


def main(args):
    pass


def platform_fullscreen(boss, window):
    from kitty.utils import platform_window_id
    native_id = platform_window_id(window.os_window_id)
    if os.uname().sysname == 'Darwin':
        import ctypes
        objc = ctypes.CDLL('/usr/lib/libobjc.A.dylib')
        objc.objc_getClass.argtypes = [ctypes.c_char_p]
        objc.objc_getClass.restype = ctypes.c_void_p
        objc.sel_registerName.argtypes = [ctypes.c_char_p]
        objc.sel_registerName.restype = ctypes.c_void_p
        send = ctypes.CFUNCTYPE(ctypes.c_void_p, ctypes.c_void_p, ctypes.c_void_p)(('objc_msgSend', objc))
        integer = ctypes.CFUNCTYPE(ctypes.c_ulonglong, ctypes.c_void_p, ctypes.c_void_p)(('objc_msgSend', objc))
        item = ctypes.CFUNCTYPE(ctypes.c_void_p, ctypes.c_void_p, ctypes.c_void_p, ctypes.c_ulonglong)(('objc_msgSend', objc))
        sel = lambda name: objc.sel_registerName(name.encode())
        app = send(objc.objc_getClass(b'NSApplication'), sel('sharedApplication'))
        windows = send(app, sel('windows'))
        for i in range(integer(windows, sel('count'))):
            w = item(windows, sel('objectAtIndex:'), i)
            if integer(w, sel('windowNumber')) == int(native_id):
                return bool(integer(w, sel('styleMask')) & (1 << 14))
        raise RuntimeError('Cannot identify Kitty Cocoa window')
    if os.environ.get('HYPRLAND_INSTANCE_SIGNATURE'):
        clients = json.loads(subprocess.check_output(['hyprctl', '-j', 'clients'], timeout=1))
        candidates = [c for c in clients if c.get('pid') == os.getpid()]
        exact = [c for c in candidates if str(c.get('address', '')) == str(native_id)]
        if not exact and len(candidates) > 1:
            titles = {w.title for w in boss.window_id_map.values() if w.os_window_id == window.os_window_id}
            exact = [c for c in candidates if c.get('title') in titles]
        candidates = exact or candidates
        if len(candidates) != 1:
            raise RuntimeError('Cannot identify Kitty desktop window unambiguously')
        return bool(candidates[0].get('fullscreen', 0))
    if os.environ.get('DISPLAY'):
        state = subprocess.check_output(['xprop', '-id', str(native_id), '_NET_WM_STATE'], timeout=1).decode()
        return '_NET_WM_STATE_FULLSCREEN' in state
    raise RuntimeError('Desktop window state unavailable; use Kitty fullscreen shortcut')


def apply(boss, window, enter):
    from kitty.fast_data_types import toggle_fullscreen
    states = getattr(boss, '_star_video_windows', None)
    if states is None:
        states = boss._star_video_windows = {}
    key = window.id
    if enter:
        if key in states:
            return
        previous = platform_fullscreen(boss, window)
        tab = window.tabref()
        layout = tab.current_layout.name if tab else None
        manager = tab.tab_manager_ref() if tab else None
        edges = ('left', 'top', 'right', 'bottom')
        spacing = {kind: {edge: getattr(getattr(window, kind), edge) for edge in edges}
                   for kind in ('padding', 'margin')}
        states[key] = (window.os_window_id, previous, layout, manager,
                       manager.tab_bar_hidden if manager else None,
                       manager.tab_bar.blank_rects if manager else (), spacing,
                       window.screen.color_profile.default_bg)
        for kind in spacing:
            for edge in edges:
                window.patch_edge_width(kind, edge, 0)
        window.set_dynamic_color(11, '#000000')
        if manager:
            manager.tab_bar_hidden = True
            manager.tab_bar.blank_rects = ()
            manager.mark_tab_bar_dirty()
        if tab:
            tab.goto_layout('stack')
        if manager:
            manager.resize()
        if not previous:
            toggle_fullscreen(window.os_window_id)
    else:
        state = states.pop(key, None)
        if state:
            os_id, previous, layout, manager, hidden, blanks, spacing, background = state
            for kind, widths in spacing.items():
                for edge, value in widths.items():
                    window.patch_edge_width(kind, edge, value)
            window.screen.color_profile.default_bg = background
            window.screen.mark_as_dirty()
            boss.default_bg_changed_for(window.id, via_escape_code=True)
            if manager:
                manager.tab_bar_hidden = hidden
                manager.tab_bar.blank_rects = blanks
                manager.mark_tab_bar_dirty()
            # Toggle only if playback changed the current desktop state.
            if platform_fullscreen(boss, window) != previous:
                toggle_fullscreen(os_id)
            tab = window.tabref()
            if tab and layout:
                tab.goto_layout(layout)
            if manager:
                manager.resize()


from kittens.tui.handler import result_handler

@result_handler(no_ui=True)
def handle_result(args, result, target_window_id, boss):
    window = boss.window_id_map.get(target_window_id)
    if window is None:
        raise RuntimeError('Kitty window no longer exists')
    if len(args) != 2 or args[1] not in ('enter', 'leave', 'leave-cleanup'):
        raise ValueError('Invalid STAR/KIT fullscreen action')
    apply(boss, window, args[1] == 'enter')
    if args[1] == 'leave-cleanup':
        os.unlink(args[0])
        os.rmdir(os.path.dirname(args[0]))
