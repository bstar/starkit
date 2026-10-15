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
Input credits are released by host acknowledgements, so slow connections and
drives backpressure file streaming without an unbounded Kitty PTY write queue.

This requires installation on each client machine and the updated application
on the SSH host. It does not require enabling general Kitty remote control.
It is an experimental terminal integration, not a feature of unmodified Kitty.
Verified on Linux with Kitty 0.48.2, using a real SSH TTY and Nautilus mouse drops
for regular files, directories and symlinks. macOS is supported by the Unix
transport implementation but has not been exercised in the desktop fixture.

Run transport isolation/backpressure checks without desktop access:

```sh
PYTHONDONTWRITEBYTECODE=1 python3 integrations/kitty/test_star_kit.py
```

## REEL video and audio

KIT's generic `session-frontend` example presents an application without linking
its backend. Build it on the terminal computer through KIT's Nix shell:

```sh
./scripts/build-frontend-local.sh
mkdir -p ~/.local/bin
ln -s "$PWD/scripts/run-frontend-local.sh" ~/.local/bin/star-kit-frontend
```

The binary is `target/release/examples/session-frontend`. This repository's
`scripts/run-frontend-local.sh` also loads `target/frontend-env.sh` for Linux
ALSA plugins; the build script records this environment automatically. Run
`star-kit-frontend --ssh USER@HOST --project REMOTE_DIRECTORY`;
`--executable` selects the absolute host executable and `--ssh-config` selects
an SSH configuration. Sessions use the existing authenticated SSH transport.

The normal REEL command inside SSH remains
`starreel --restart --project ~/Videos/demo.starreel`. Applications start their
controller before attaching the terminal bridge, including fresh projects and
restarts. A FOLD terminal-client wrapper uses the same decoder after rebuilding
FOLD against the matching KIT revision. This is a one-time frontend update on
the terminal computer; it requires no new launch command or SSH authentication.

For installations without FOLD, the fixed `star-kit-terminal` wrapper may instead
execute `star-kit-frontend --terminal-frontend "$@"`. The socket is supplied by
the watcher. REEL's application lifecycle now negotiates this bridge too.
Updating the watcher file alone is insufficient: both the frontend and the
remote KIT consumer need streamed-frame support.

Updated media-enabled frontends negotiate `canvas_video` for interactive H.264
canvases. REEL encodes access units on GStreamer workers; KIT reuses FOLD's
hardware/software decoder helpers on a local worker, without a movie clock or
seek buffering. Video is bounded to 1280×720 and 60 fps, one access unit in
flight, and 2 MiB per access unit. Interdependent encoded packets are preserved;
raw source updates coalesce before encoding. Decoded frames use local Kitty
files, whose consumption gates further decoding. A failed hardware decoder
retries the initial keyframe in software. Epochs isolate reconnects and switches.
The build script enables `terminal-graphics,media`; both the host REEL build and
the frontend on the terminal computer must be updated for this path.

Older frontends negotiate `streamed_frames`, retaining one latest pending frame
and one acknowledged JPEG in flight per stream; no host paths cross SSH.
Compatibility JPEGs are bounded to 1280×720, 512 KiB and 10 fps. Ordinary SSH
terminal previews also retain a 10 fps ceiling, with an 8 MiB/s encoded-payload
budget per canvas. Terminal writes alone do not acknowledge image decoding, so
this path stays conservative. Source recording pixels are independent.

The frontend uses its existing PCM worker for 48 kHz stereo audio with eight
block credits; errors close the output rather than switching sound to the host.
REEL taps its private demo sink during capture and a separate audition sink
during editing, so background apps do not mix with timeline playback. Microphone
capture remains on the recording machine. Physical Mac live-preview performance
and listening remain unverified.
