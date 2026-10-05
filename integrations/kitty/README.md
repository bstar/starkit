# Native STAR/KIT frontend for existing SSH shells

This experimental integration lets an application launch normally after `ssh`
while its native Rust frontend runs inside the same local Kitty window. The
application controller and filesystem workers stay on the SSH host. All traffic
uses the existing SSH TTY; it does not infer a hostname, request authentication
again, or create a network listener.

Install the watcher on the machine running Kitty, once:

```conf
watcher /absolute/path/to/starkit/integrations/kitty/star_kit.py
```

Provide an executable `~/.local/bin/star-kit-terminal` that invokes the installed
application's terminal client entrypoint. For STAR/FOLD:

```sh
#!/bin/sh
exec /absolute/path/to/starfold --graphical-terminal-client "$@"
```

New Kitty windows load the watcher. Remote application launchers use
`terminal_bridge::probe` and, when present, `Attachment::relay` against their
persistent session socket. Unsupported terminals retain the ordinary frontend.
The client entrypoint calls `client::run_terminal_socket_with_events`; all UI
rendering and drag capability handling remain native Rust in STAR/KIT.

Kitty's Python watcher is limited to launch and message framing. It starts only
the fixed local executable above. Remote messages cannot choose a command,
executable, local path, or target window. Each session has a random capability,
private directory and Unix socket. Payloads and queues are bounded, and socket
IO on Kitty's main loop is nonblocking. Genuine desktop drops are handled by the
local Rust client, preserving source capability checks and deferred Move cleanup.

This requires installation on each client machine and the updated application
on the SSH host. It does not require enabling general Kitty remote control.
It is an experimental terminal integration, not a feature of unmodified Kitty.
