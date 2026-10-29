"""Start a terminal before allowing its child to probe terminal capabilities."""
import subprocess
import sys
import time


class TerminalStartup:
    def __init__(self, private):
        self.ready = private / 'terminal-ready'
        self.launcher = private / 'wait-for-terminal.py'
        self.launcher.write_text('''import os
from pathlib import Path
import sys
import time
ready = Path(sys.argv[1])
deadline = time.monotonic() + 30
while not ready.exists():
    if time.monotonic() >= deadline:
        raise SystemExit("Terminal proof did not release its startup gate")
    time.sleep(.01)
os.execv(sys.argv[2], sys.argv[2:])
''')

    def command(self, binary, *arguments):
        self.ready.unlink(missing_ok=True)
        return [sys.executable, str(self.launcher), str(self.ready), binary, *arguments]

    def release(self, child, remote_control, screenshot):
        # A cold software OpenGL context can take seconds to initialize in CI.
        # Complete a real screenshot before the app sends bounded queries;
        # remote-control socket existence alone does not establish readiness.
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            if child.poll() is not None:
                raise AssertionError('Private terminal exited before startup; inspect kitty.log')
            try:
                remote_control('screenshot', '--match', 'id:1', str(screenshot))
                self.ready.touch()
                return
            except (subprocess.SubprocessError, FileNotFoundError):
                time.sleep(.1)
        raise AssertionError('Private terminal did not finish initializing its renderer')
