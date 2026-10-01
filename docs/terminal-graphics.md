# Terminal graphics foundation

Status: implemented experimental backend, with Linux Kitty and headless SSH
verification. macOS offscreen rendering and controller tests are CI gates.
This experiment starts at STAR/KIT main `ff7ae1a`; its first consumer starts at
STAR/FOLD main `6cbee38` (v0.0.2). Both branches are named
`experiment/terminal-graphics`. The existing GPUI experiment is preserved on
`experiment/graphical-presentation`.

## Product goal

Render an interactive graphical STAR interface inside the user's existing
terminal. Keep STAR/FOLD's stack navigation, Commander browser panes, Preview and
Operations placement, established actions, shortcuts, themes and session behavior.
The graphics foundation belongs to STAR/KIT and must support other STAR apps.

The output must include the graphical interface itself, with readable text,
menus, controls and pointer interactions. Decorating an ordinary TUI with icons
does not meet this experiment's acceptance criteria.

## References inspected

### Awrit: rendering and input

[Awrit](https://github.com/chase/awrit) renders Chromium content using the Kitty
Graphics Protocol. Its Electron implementation creates hidden offscreen browser
windows, consumes paint events, transfers bitmap buffers and forwards terminal
input to browser web contents. Its implementation includes shared-memory frame
composition and a fallback image replacement path.

Inspected snapshot: `dae228e69451132958585f48a570d4463609f989`.
Relevant source:

- [Window setup](https://github.com/chase/awrit/blob/electron/src/windows.ts)
- [Paint handling](https://github.com/chase/awrit/blob/electron/src/paint.ts)
- [Keyboard/pointer routing](https://github.com/chase/awrit/blob/electron/src/inputHandler.ts)
- [Graphics transport](https://github.com/chase/awrit/blob/electron/src/tty/kittyGraphics.ts)

Awrit's upstream is archived. Use its architecture as a reference and use a
maintained browser runtime for the new implementation. Start with a local static
STAR UI document and an application message bridge rather than general browsing.
The Chromium runtime version must be recorded in the reproducible package setup.

### Cmux: application and surface lifecycle

[Cmux](https://github.com/manaflow-ai/cmux) includes a native Ghostty-based desktop
app, a Rust terminal multiplexer and browser integration. Those are distinct
parts of the project, with different capabilities and maturity.

Inspected snapshot: `15aa32cfc4bcaedee2f99f0c03b68f9814eaba79`.
Relevant sources:

- [TUI architecture](https://github.com/manaflow-ai/cmux/blob/main/cmux-tui/README.md)
- [Transport/capability protocol](https://github.com/manaflow-ai/cmux/blob/main/cmux-tui/docs/protocol.md)
- [Browser protocol import status](https://github.com/manaflow-ai/cmux/blob/main/cmux-browser/README.md)

Lessons to apply independently:

| Lesson | STAR/KIT requirement |
| --- | --- |
| Stable workspace/pane/surface identities | Rendering and input use explicit IDs; changing focus must not redirect an already captured action |
| Capability negotiation | Report image transport, pixel geometry, pointer precision and supported input separately |
| Pointer frame guards | Bind pointer input to a presented frame and geometry generation; reject clicks against a replaced listing or stale size |
| Input backpressure and resize coalescing | Bound queues; preserve key/press/release ordering; keep the latest pending resize and obsolete repaint only |
| Versioned protocol boundaries | Keep renderer messages independent of file-manager commands and runtime-specific APIs |
| Stdio relay across SSH | Keep the application/renderer boundary transportable; evaluate local rendering of remote application state |
| Separate surface and session lifetimes | Closing a tab releases its surfaces; renderer loss leaves application errors understandable |

The Browser source import has explicit release-readiness gates. Its desktop
source also has different licensing from STAR/KIT. This work will implement its
own protocol and components; cmux is an architectural reference, not a dependency
or source to copy.

## Foundation boundary

```text
STAR application engine and controller
    │ application state → shared component model
    │ semantic user intents ← input events
    ▼
STAR/KIT graphical components and scene/layout model
    ▼
STAR/KIT offscreen browser adapter
    ▼
STAR/KIT frame scheduling / image transport
    ▼
Existing terminal emulator
```

STAR/KIT owns:

- Backend capability discovery and measured terminal pixel geometry.
- Versioned scene, surface, frame and input types.
- Reusable panels, tabs, virtual lists/trees, menus, dialogs, text fields,
  progress/storage indicators and image/embedded-terminal surfaces.
- Shared theme tokens, typography, clipping, focus and hit testing.
- Optional offscreen renderer process, startup, error reporting and shutdown.
- Frame pacing, bounded queues, dirty region updates, image cache and protocol
  cleanup. Use in-band transfers for remote paths; local shared memory is an
  optimization only after the portable path works.
- Pointer/keyboard/paste routing and guards for frame/geometry changes.
- Plain TUI fallback when graphical capabilities are unavailable.

Applications own their data, navigation, sessions, commands and operation workers.
STAR/FOLD supplies captured paths and established actions. Neither JavaScript nor
STAR/KIT performs file copy/delete/rename operations. Renderer-specific types stay
out of application engines and default consumers do not acquire Chromium.

The component model must support application-specific content slots. A reusable
backend should not force the file-manager layout on STAR/AMP or STAR/CORD.

## Implementation sequence and acceptance gates

### 1. STAR/KIT viability probe

Create a standalone shared example before integrating the file manager. Render a
panel, virtualized list, tabs, menu, text field and image using an offscreen browser.
Display the resulting graphical pixels in Kitty and route keyboard/pointer input
back to the scene. Prove that no visible native application window opens.

Measure cold/warm startup, idle CPU/RSS, input-to-presentation latency, encoded
bytes per update, scrolling, resize/font zoom, clean exit and renderer failure.
Determine whether the maintained runtime works without a display server; include
a real headless SSH-host test. Offscreen rendering alone is not proof of headless
compatibility. Record Linux and macOS runtime dependencies and compatibility.

### 2. Shared scene/components and transport

Make the probe's reusable components and lifecycle API the optional STAR/KIT
backend. Add protocol and input/frame-guard tests, bounded resource tests and
repaint/resize stress checks. Preserve the existing terminal API. Check default
STAR/AMP, STAR/CORD, STAR/WIRE and STAR/FOLD consumers before changing shared APIs.

Start with correct complete frames, then introduce dirty regions/cached surfaces
based on measured costs. Replace frames atomically and delete retired placements
without a blank interval. Throttle remote updates using actual bandwidth costs.

### 3. STAR/FOLD integration

Extract a renderer-independent view/controller boundary from main's existing
application behavior. Translate semantic intents into established actions; retain
captured paths instead of reconstructing them from displayed names.

The final integration gate includes stack/Commander navigation, marks, independent
sort/filter, tabs, Places, preview, operations, copy planning/locks/conflicts,
rename/create/delete, archive actions, errors/admin retries, clipboard, drag/drop,
embedded editor/player, themes and session restore. Preview and Operations keep
STAR/FOLD's established placement. Filenames remain single-line with full-name
access available. Preserve scrollbar and drag/drop ownership and source-scroll
freeze. Features beyond the selected main baseline require separate changes.

### 4. Compatibility and promotion

Verify Kitty first, then other actual implementations of the image protocol,
ordinary SSH, slower links and multiplexer fallback. Run small/light/dark layout
checks, large directories and active operations. Document text selection and
accessibility behavior explicitly. Enable the backend only through an isolated
experimental launcher until these gates pass; keep ordinary TUI use available.

The browser renderer's remote placement remains an experiment: running it on the
remote host requires a working headless runtime; running it locally requires the
scene/input relay. Choose from measured behavior rather than assuming either
works automatically.


## Implemented backend

Enable the optional `terminal-graphics` feature. Default consumers retain their
existing terminal API and do not install Electron. The shared model contains
panels, virtual list rows, tabs, menus, dialogs, text fields, meters, images and
embedded terminal slots, plus styled compatibility spans for established app
content. Layout and hit regions retain the application's authoritative coordinate
grid; this is not a separate browser file manager.

The maintained renderer is Electron **43.6.0**, pinned by the optional runtime's
npm lockfile (and supplied by the Nix `graphical-runtime` package). It creates one
hidden, sandboxed offscreen BrowserWindow, loads only packaged local content,
and sends PNG frames through Kitty's in-band graphics protocol. JavaScript has no
filesystem operation API. Runtime profiles and logs live in a private temporary
folder and are removed on exit. A missing/failed runtime reports an error after
restoring the terminal.

Frames use a bounded grid of cell-aligned regions, normally 32 columns × 8 rows
and at most 256 placements. The frontend compares each self-contained browser
frame with the pixels actually presented, then uploads only changed regions.
It places every replacement before retiring old regions inside synchronized
terminal updates. This remains correct when browser frames are dropped; no
remote delta base is assumed. Resize replaces the grid and cleanup removes all
owned placements. Identical frames send zero image payload bytes but still
advance scene acknowledgements and input guards. Retained decoded pixels are
bounded to 128 MB and encoded frame data to 15 MB; an oversized region payload
uses the existing complete-frame path. Decoder limits apply before allocation.
The local renderer keeps only its newest pending scene. Remote scenes include visible rows
and cached, downsampled PNG assets rather than complete directory listings.

### SSH and session boundary

```text
Local Kitty ← PNG frames ← local Electron ← local STAR/KIT frontend
                                               ↕ bounded JSON lines
                                            SSH stdio relay
                                               ↕ private Unix socket
                                    persistent remote Rust controller
                                               ↕ existing workers
```

The remote host needs neither Electron nor a display server. SSH authentication
finishes before raw mode, using a private connection multiplexing socket. Remote
commands shell-quote executable and directory arguments. Standard SSH host/key
configuration applies. Sessions use mode 0700 directories and mode 0600 sockets;
there is no public listener. A bare socket probe cannot replace an attachment.
One authenticated frontend owns an attachment at a time.

Protocol version 1 limits messages to 16 MiB, scenes to 120,000 cells and viewports
to 8192 pixels per dimension / 32 million pixels. Input queues and frame/asset
queues are bounded. Filesystem mutations remain on the application's controller.
Frontends advertising presentation acknowledgements allow only one scene in
flight until the terminal write completes. Pending application state continues
to coalesce, and resize can supersede an obsolete geometry immediately. Actual
serialized scene bytes and delivery-to-presentation time are logged. This
feedback paces transport according to observed delivery and rendering costs;
it is not a measurement of physical network bandwidth. Legacy version-one
frontends without this capability retain the existing bounded frame queue.
Per-client input IDs survive reattachment; acknowledgements reject duplicates,
and reconnect never resends a mutation. Acknowledged frame target signatures and
geometry generations protect pointer input. Progress repaint does not invalidate
a click; a replaced listing does. Stale releases cancel captured drag actions.
OSC 72 transfers are acknowledged and apply backpressure independently of scene
coalescing. Local clipboard effects are applied by the local frontend: Electron in pixel
mode, OSC 52 in cell mode, or an explicit tmux buffer write inside tmux.

### Running the shared example

```sh
nix develop .#graphical
cargo run --example terminal-graphics --features terminal-graphics -- --interactive
# Without taking over the terminal, capture the component sample:
cargo run --example terminal-graphics --features terminal-graphics -- sample.png
# Warm frame benchmark:
STAR_GRAPHICS_BENCH_FRAMES=100 cargo run --example terminal-graphics --features terminal-graphics -- sample.png
```

The interactive example includes 100,000 virtual entries, tabs, marks, a text field,
menus and a persistent controller. Arrow keys/j/k navigate; space marks, typing
edits the text field, `c` opens a menu, Escape closes it, q ends the controller and
Ctrl+Q detaches. It does not perform file operations.

### Evidence and remaining promotion gates

Measured on the Linux development machine; these are observations, not guarantees:

| Check | Result |
| --- | --- |
| Hidden renderer cold startup | 209–435 ms |
| 100 warm scene→PNG updates | p95 68.28 ms; sample PNG 120,457 bytes |
| Kitty API input→observed pixel change (20 navigation inputs) | p95 219.92 ms; includes API and screen-capture overhead; 1859×2099 terminal, 100,000-row example |
| FOLD Kitty native screenshot API, 30 navigation/preview/menu inputs | median 531.60 ms, p95 539.14 ms; 1820×2048 window; includes API/capture costs and settles preceding preview updates |
| 100 selected-row updates with dirty regions | browser p95 63.17 ms; region comparison/encoding p95 5.73 ms; mean delta payload 32,985 bytes vs 160,612 complete-frame bytes |
| FOLD 100,000-entry controller+scene updates | p95 1.848 ms; visible rows only; <100 KiB JSON |
| Headless real SSH, 50 ms modeled RTT / 10 Mbps | first listing 254.2 ms; input→ack p95 109.0 ms |
| Remote copy through disconnect | checksums verified; same process reattached; repeated paste rejected |
| Quiet full-size local sample (3 seconds) | 1.67% aggregate CPU; 451 MiB aggregate PSS; controller 19 MiB RSS |
| Full-size local idle sample (5 minutes, 61 samples) | 1.14% mean aggregate CPU; 469.02–469.07 MiB aggregate PSS; no measured growth |
| FOLD mixed workload (10 minutes, 949 samples) | 256 MiB copy checksum verified, then continuous navigation/preview/menu/theme/font changes; 96.68% aggregate CPU of one core; PSS 618.70–1379.81 MiB, median 954.36 MiB |
| Renderer loss in actual Kitty | restored terminal; controller survived and retained cursor/marks; controller then closed cleanly |
| Repaint/resize burst | 96 independently reconstructed frames match all source pixels; bounded placements and complete cleanup |
| Linux Xvfb software capture / macOS offscreen CI | PNG capture passes with the maintained npm runtime |
| Real Kitty local presentation | graphical file view rendered in the existing terminal; no visible Electron window |
| Ghostty 1.3.1 Linux | graphical view, actions menu, keyboard input, session reattachment and font zoom; X11/software Mesa test |
| Cell fallback | real tmux displays the same controller; PTY test copies a file with Electron and display servers unavailable |

The screenshot latency probe compares actual captured pixels, while including
the cost of remote-control input injection and repeated screen captures. It is
an observation of this large terminal, not a guarantee or a pure display latency.
The latency figures measure different boundaries: SSH acknowledgements are not
pixel presentation latency. Full frames travel only from local Electron to local
Kitty, so the remote link carries scene data and occasional image assets.

Linux/macOS controller builds and the offscreen sample have CI jobs.

The interactive smoke test (`scripts/test-kitty-interactive.py`) captures the
actual Kitty window, compares pixels after navigation/menu/font zoom, and checks
normal session cleanup. It passed on the local Linux desktop with Kitty 0.49.2.
The test requires a live Electron process, so a passing cell fallback is not
accepted as graphical proof. Linux Xvfb passed on shared-code commit `6534ffd`
([CI run](https://github.com/bstar/starkit/actions/runs/36934302120)); its screenshots
show the browser-rendered menus. Ordinary clients no longer send the OSC 72
extension query; STAR/FOLD registers an extension-aware input handler and retains
its terminal drag/drop negotiation. The hosted macOS VM rejects
Kitty's OpenGL surface with `NSGL: Failed to find a suitable pixel format`, so
`.github/workflows/kitty-macos.yml` provides the outstanding interactive gate for
a self-hosted Mac labelled `graphical-desktop`; its offscreen CI remains separate.

A first mixed STAR/FOLD workload verified a 256 MiB copy checksum, then exposed
Chromium `UnknownVizError` during repeated navigation/menu/theme/font changes.
That incomplete run does not establish sustained resource stability. The runtime
now retries only that compositor error up to five captures, with waits of 25,
50, 75 and 100 ms. Persistent compositor errors and unrelated errors still end
the graphical attachment; controller sessions survive attachment loss. Recovery
has deterministic runtime tests. The fresh workload completed 600.58 seconds
with 949 samples, 3,781 presented frames and 255,406,512 uploaded image bytes.
All sampled process memory was readable. The measured family includes the held
terminal's launcher, frontend, relay/controller and Electron descendants; it
excludes Kitty itself and the input driver. CPU counters can miss short-lived
preview subprocesses. The copy completed before the sustained interactive phase,
so this is not ten minutes of continuous copying. PSS fluctuated substantially;
this bounded observation does not establish long-term memory stability. The
frontend, controller and renderer exited after `q`, and the owned held terminal
was closed separately.

Promotion still requires macOS Kitty interaction and broader terminal
compatibility testing. The
experimental launcher selects pixel presentation when the Kitty image transport
is detected. Otherwise it presents the same persistent controller session as
terminal cells, without launching Electron. This includes tmux fallback; its
graphical support is not claimed. Image transport, measured/estimated pixel
geometry, cell pointer precision, keyboard and paste support are negotiated
separately. No terminal is claimed to provide pixel-precise pointer coordinates. PNG text has no native terminal selection or screen-reader text
stream; application clipboard actions remain available. There is no remote audio
or video forwarding: player/editor processes run on the application host.


For a non-Nix local runtime (Node >=22.12.0), download Electron before opening
its protocol pipe. Electron 43's npm launcher downloads lazily and prints a
status line on first use; that output is not a renderer message.

```sh
npm ci --prefix runtime/terminal-graphics
npm run install-runtime --prefix runtime/terminal-graphics
export STAR_GRAPHICS_ELECTRON="$PWD/runtime/terminal-graphics/node_modules/.bin/electron"
```


For software rendering, set `STAR_GRAPHICS_SOFTWARE=1`. On Linux,
`STAR_GRAPHICS_PLATFORM=x11` or `wayland` explicitly chooses the display backend;
CI uses Xvfb with X11 and software rendering. A local display server is still
required; the SSH application host does not run this renderer. Chromium's memory
cost remains material: the short local PSS sample includes renderer, frontend and
controller processes. Sustained measurements and a renderer sharing strategy
across multiple STAR apps remain promotion work.
