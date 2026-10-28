# Application UI contract

The `ui` module is the 0.13 renovation candidate. It establishes a shared
application structure and lifecycle; migration of the reference applications
and physical platform acceptance are still required before a release.

An application describes `View`, `Node` and `Heading` values. Stable `Id`s name
controls independently of their position, text or list index. The runtime owns
focus, text editing, selection, pointer capture, layout and invalidation. The
application handles `Event`s and updates its own domain state.

```rust
use starkit::ui::{command::{Command, Registry}, Heading, Node, Runtime, View};

let commands = Registry::new(vec![Command::new("save", "Save").shortcut("ctrl+s")])?;
let view = View::new(Node::pane("document", Heading::new("Document"),
    Node::column("content", vec![
        Node::field("name", "Untitled"),
        Node::button("save-button", "save"),
    ])));
let mut ui = Runtime::new(view, commands, theme)?;
```

`Runtime::new` returns a `Result`; supply a resolved `theme::Theme`. The
independent starter demonstrates the complete construction and event loop.
Run `scripts/check-starter.py --candidate --artifacts target/ui-gallery` through
the flake to compile it outside the workspace and retain its pixel/cell gallery.
Add `--release` for a preview at `target/external-starter/release/starkit-starter`;
launch it with `--native` and use Alt+T for themes and F9 for cells/graphics.
The starter also optimizes dependencies in development so its debug preview
remains interactive.

Crossterm 0.29.0 has a mouse/cursor coordinate underflow. Until an upstream
release includes the fix, application roots must retain the starter's pinned
`[patch.crates-io]` entry. Cargo does not inherit dependency patches. This keeps
KIT and Ratatui using the same corrected parser.

## Components and geometry

Rows, columns and panes compose the shell. Standard buttons, checkboxes, fields, sliders, meters,
lists, table rows and tree indentation share interaction. Toolbar, tab-strip
and modal menu constructors compose the same command-backed controls. A command
contains its label, shortcuts, scope, enabled and checked state. The registry
validates duplicate identities, invalid keys and overlapping shortcuts; the UI
also validates mounted scopes and control references. `Runtime::update` installs
view and commands atomically.

One immutable `Frame` supplies drawing, clipping, focus order and hit geometry.
`Viewport::cells` uses columns/rows; `Viewport::graphics` uses physical pixels.
Explicit `Size`s are logical UI units, mapped to 8 pixels horizontally and 24
pixels vertically in graphics. Pane geometry preserves compact cell headings
and native title strips. Graphical workspace margins, pane body padding and
gaps are 8 physical pixels; text inside controls adds a 4 pixel inset. Explicit
dialog sizes are honored and clipped to the workspace. These are presentation
metrics, not terminal font
scaling. Existing editor-grid and legacy surface text contracts remain intact.

The pane engine retains manual sizes and user visibility. Automatic collapse
never rewrites the supplied view. IDs must be unique across both root and modal
content. The tree is bounded to 2,048 nodes and 32 levels; rendering retains the
existing terminal scene/surface limits. Use virtualized custom content for
larger documents rather than creating a control per document row.

## Input and lifecycle

The active pane and modal shell receive the focus border in both presentations,
including read-only dialogs with no focusable controls.

Tab/Shift+Tab traverse enabled controls. Text fields handle editing before
application shortcuts; modal content traps focus and background commands.
Enter and pointer release invoke the same registered control command. Lists,
tables and indented trees support Up/Down, Home/End and Page Up/Page Down;
page movement uses visible data rows, excluding a table heading. Disabled rows
are skipped. Enter emits `Event::Activated` for the selected enabled item,
separately from `Event::Selected`. Modified navigation keys remain available
to registered shortcuts. External selection and reordering reveal the selected
item; wheel scrolling can inspect other rows without changing selection.

A slider retains pointer capture outside its bounds and emits a final bounded
`Adjusted` value on release. Resize, disabled/removed targets and presentation
changes cancel capture. Wheel scrolling cancels a pending list click so release
cannot select a different row than the one pressed. Pointer
release must match its press target and viewport generation. Changed geometry,
removed targets, disconnect and presentation switching cancel capture. Visual
updates that retain targets preserve custom drags. Escape requests modal close.

`command::standard` supplies Help, Theme, presentation switching, Repaint and
Quit defaults. Applications handle their effects; the existing terminal frontend
continues to own negotiated presentation switching and display restoration.
Bindings are registry data, so user configuration can build a replacement
registry and receive explicit conflict errors.

Implement `ui::application::Application` and `ui::content::Content`, then wrap
the application in `Host`. `Host::run_cells` owns the cell terminal lifecycle
without optional graphics/media dependencies. On Unix with `terminal-graphics`,
pass that controller
to `terminal_graphics::application::Application`. This retains the existing
persistent local/SSH lifecycle, capability negotiation, bounded media, live
frames, audio and exports. Domain workers remain in their application.

Return true from `Application::tick` only when visible state changes. Quiet
hosts reuse their scene; value/focus updates retain layout, and geometry changes
solve it again. The renderer retains unchanged surface rasters under a combined
16 MB / 64-entry budget. Live frames travel independently of UI rebuilding.

## Specialized content

A `Widget::Custom` reserves a bounded slot and declares its fallback and content
revision. `Content::cells` receives a local buffer that KIT copies into that
slot. `Content::native` returns an independently mounted surface with exactly
the slot's pixel dimensions; its physical font or editor-grid hints are
preserved. Neither callback may replace the shell or own terminal modes.

`Content::canvas` names an existing bounded live stream. KIT constructs the
`ContentView` used by rendering and embedded input. Source pixels never enter
scene JSON. Native content appears below modal chrome. Missing cell content
uses the declared fallback; applications disable actions that require absent
capabilities. A cell frontend does not make a desktop capture backend available.

Keep stream generations, credits, cancellation and audio-device errors in the
existing media protocols. A failed frontend device must never switch playback
to the host. Graphics-only consumers must continue to exclude FFmpeg and CPAL.

## Verification

Linux native-graphics CI builds the independent starter and exercises it in a
private Kitty instance under Xvfb. `scripts/test-application-ui.py` verifies
commands, page navigation and item activation, modal isolation, theme delivery,
cell/native switching, display recovery and clean exit. It retains screenshots
and a JSON report in the `application-ui-proof` artifact. Run it locally with
`--kitty PATH --kitten PATH --binary PATH --output DIRECTORY`, selecting
`--display-server wayland` for a Wayland desktop or `x11` for Xvfb.

These checks cover the shared contract and gallery. Reference application
migrations and physical macOS/Sixel/iTerm2 acceptance remain separate gates.
