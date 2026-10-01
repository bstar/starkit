# Terminal graphics foundation

Status: design and reference investigation. Implementation has not started.
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
