"""Fixed STAR/KIT window operation, invoked locally through Kitty remote control.
No host-provided command, identifier, or filesystem path is executed.
"""
import json
import os
import subprocess


def main(args):
    pass


def cursor_hidden(hidden):
    if os.uname().sysname != 'Darwin':
        return
    import ctypes
    objc = ctypes.CDLL('/usr/lib/libobjc.A.dylib')
    objc.objc_getClass.argtypes = [ctypes.c_char_p]
    objc.objc_getClass.restype = ctypes.c_void_p
    objc.sel_registerName.argtypes = [ctypes.c_char_p]
    objc.sel_registerName.restype = ctypes.c_void_p
    send = ctypes.CFUNCTYPE(None, ctypes.c_void_p, ctypes.c_void_p, ctypes.c_bool)(('objc_msgSend', objc))
    send(objc.objc_getClass(b'NSCursor'), objc.sel_registerName(b'setHiddenUntilMouseMoves:'), hidden)


def pointer_visibility(boss, window, enabled, client_pid=None):
    """Balanced Cocoa hiding, scoped to this live client and Kitty window.

    The timer restores after SIGKILL, moving outside Kitty, tab switches and
    application focus loss, independently of the terminal client's event loop.
    """
    if os.uname().sysname != 'Darwin':
        return
    from kitty.fast_data_types import add_timer, remove_timer, get_os_window_size
    states = getattr(boss, '_star_pointer_visibility', None)
    if states is None:
        states = boss._star_pointer_visibility = {}
    existing = states.get(window.id)
    if existing:
        existing['enabled'] = enabled
        if not enabled:
            existing['refresh']()
        return
    if not enabled:
        return
    if not isinstance(client_pid, int) or client_pid <= 0:
        raise ValueError('Invalid local cursor owner')
    import ctypes
    from kitty.utils import platform_window_id
    class Point(ctypes.Structure):
        _fields_ = [('x', ctypes.c_double), ('y', ctypes.c_double)]
    objc = ctypes.CDLL('/usr/lib/libobjc.A.dylib')
    objc.objc_getClass.argtypes = [ctypes.c_char_p]
    objc.objc_getClass.restype = ctypes.c_void_p
    objc.sel_registerName.argtypes = [ctypes.c_char_p]
    objc.sel_registerName.restype = ctypes.c_void_p
    sel = lambda name: objc.sel_registerName(name.encode())
    cls = lambda name: objc.objc_getClass(name.encode())
    send = ctypes.CFUNCTYPE(ctypes.c_void_p, ctypes.c_void_p, ctypes.c_void_p)(('objc_msgSend', objc))
    perform = ctypes.CFUNCTYPE(None, ctypes.c_void_p, ctypes.c_void_p)(('objc_msgSend', objc))
    flag = ctypes.CFUNCTYPE(ctypes.c_bool, ctypes.c_void_p, ctypes.c_void_p)(('objc_msgSend', objc))
    point = ctypes.CFUNCTYPE(Point, ctypes.c_void_p, ctypes.c_void_p)(('objc_msgSend', objc))
    number = ctypes.CFUNCTYPE(ctypes.c_long, ctypes.c_void_p, ctypes.c_void_p, Point, ctypes.c_long)(('objc_msgSend', objc))
    integer = ctypes.CFUNCTYPE(ctypes.c_ulonglong, ctypes.c_void_p, ctypes.c_void_p)(('objc_msgSend', objc))
    item = ctypes.CFUNCTYPE(ctypes.c_void_p, ctypes.c_void_p, ctypes.c_void_p, ctypes.c_ulonglong)(('objc_msgSend', objc))
    state = states[window.id] = {'enabled': True, 'hidden': False, 'timer': None}
    native_id = int(platform_window_id(window.os_window_id))
    def refresh(timer_id=None):
        alive = True
        try:
            os.kill(client_pid, 0)
        except ProcessLookupError:
            alive = False
        except PermissionError:
            pass
        active = boss.active_window
        owned = state['enabled'] and alive and boss.window_id_map.get(window.id) is window
        desired = owned and active is window and flag(send(cls('NSApplication'), sel('sharedApplication')), sel('isActive'))
        if desired:
            location = point(cls('NSEvent'), sel('mouseLocation'))
            desired = number(cls('NSWindow'), sel('windowNumberAtPoint:belowWindowWithWindowNumber:'), location, 0) == native_id
            if desired:
                windows = send(send(cls('NSApplication'), sel('sharedApplication')), sel('windows'))
                native_window = next((w for w in (item(windows, sel('objectAtIndex:'), i) for i in range(integer(windows, sel('count'))))
                                      if integer(w, sel('windowNumber')) == native_id), None)
                size = get_os_window_size(window.os_window_id)
                desired = bool(native_window and size and size['width'] and size['height'])
                if desired:
                    local = point(native_window, sel('mouseLocationOutsideOfEventStream'))
                    x = local.x * size['framebuffer_width'] / size['width']
                    y = (size['height'] - local.y) * size['framebuffer_height'] / size['height']
                    g = window.geometry
                    desired = g.left <= x < g.right and g.top <= y < g.bottom
        if desired != state['hidden']:
            perform(cls('NSCursor'), sel('hide' if desired else 'unhide'))
            state['hidden'] = desired
        if not owned:
            if state['timer'] is not None:
                remove_timer(state['timer'])
            states.pop(window.id, None)
    state['refresh'] = refresh
    state['timer'] = add_timer(refresh, 1 / 30, True)
    refresh()


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
    pointer_hide = len(args) == 2 and args[1].startswith('pointer-hide:') and args[1].split(':', 1)[1].isdigit()
    if len(args) != 2 or (not pointer_hide and args[1] not in ('enter', 'leave', 'leave-cleanup', 'cursor-hide', 'cursor-show', 'pointer-show')):
        raise ValueError('Invalid STAR/KIT fullscreen action')
    if pointer_hide or args[1] == 'pointer-show':
        pointer_visibility(boss, window, pointer_hide, int(args[1].split(':', 1)[1]) if pointer_hide else None)
        return
    if args[1].startswith('cursor-'):
        cursor_hidden(args[1] == 'cursor-hide')
        return
    if args[1] != 'enter':
        cursor_hidden(False)
        pointer_visibility(boss, window, False)
    apply(boss, window, args[1] == 'enter')
    if args[1] == 'leave-cleanup':
        os.unlink(args[0])
        os.rmdir(os.path.dirname(args[0]))
