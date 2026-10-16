# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
On 0.x, an API change is a minor bump and a fix is a patch.

## [Unreleased]

### Added (0.11 candidate)

- Opt-in soft workspace chrome, negotiated native theme gradients with bounded raster caching, floating panels and context menus.
- Credit-bounded export delivery to a frontend-owned folder with checksum verification, atomic no-clobber completion and disconnect cleanup.

### Fixed (0.11 candidate)

- Keep independently mounted native surfaces in their own physical placements,
  preventing duplicated, shifted desktop docks and overlapping controls.
- Composite transparent placements over existing pixels so rounded docks and focus outlines preserve the desktop beneath them.
- Route modal and floating-control input ahead of live canvases and preserve precise local hover across physical placements.

### Added (0.10 candidate)

- Additive `canvas_video_high_quality` negotiation for full-resolution H.264
  canvases up to 3840×2160, including 4:4:4 software decoding. Legacy peers keep
  their 1280×720/2 MiB contract; new clients accept bounded 8 MiB access units.


- Start persistent application controllers before attaching a terminal-owned
  frontend, so normal SSH launch and `--restart` work for fresh sessions.

- Negotiated H.264 live canvases on media-enabled local frontends, using the
  movie hardware/software decoder helpers on a worker without playback buffering.
  One access unit in flight, packet/dimension bounds, local Kitty file feedback
  and epoch cancellation bound work independently of input and recording.
  Compatibility JPEG/remote-TTY previews retain 10 fps pacing; inline PNGs have
  an 8 MiB/s per-canvas payload budget. Embedded input also routes streamed frames.

- SIMD bilinear scaling for live canvases, with a direct pixel path at 1:1,
  reducing per-frame work on the terminal input thread without changing captures.
- Shared theme-picker pointer handling in native and cell views: wheel to browse,
  click to choose, and modal bounds that prevent input reaching the underlying app.
- Shared searchable command guide with native/cell layouts, ranked and typo-tolerant search, category filters, and modal input handling.

### Added (0.9 candidate)

- Negotiated, bounded live-canvas video over SSH, including inline Kitty
  presentation in remote shells. A presentation-only frontend example reuses
  KIT's independent PCM worker for remote app audio without application backend
  dependencies on the terminal computer.

- Active local live-canvas sessions sample producers every 8 ms, independently
  of UI scene painting. Quiet apps and detached sessions keep the 33 ms timer.

- Retained-framebuffer display recovery: exact frame/layout snapshots, automatic
  focus restoration, bounded cached-tile repairs, and restoration of idle live
  canvases and paused video. Ctrl+L refreshes all layers without restarting the
  rasterizer. Real Kitty tests fault-inject missing terminal placements.

- Audio transport uses one state-dependent Play/Pause button in a stable slot,
  matching the existing movie transport behavior.

- Shared tutorial keycap surfaces and node filtering that remaps text rules,
  centered labels and hover references when a panel is collapsed.

- Shared workspace panels, full-target toolbar controls, aligned property fields,
  section rules and numeric/time-ruler presentation. Palette-derived neutral
  surfaces support dense video editors without private chrome. Additive `ui_font`
  native surfaces use bundled UI faces; legacy terminal typography stays intact,
  and cache identities distinguish the font choice.

- Shared `theme::picker::Picker`: Alt+T opener, search, navigation, cancellation,
  current-theme checks, native palette swatches and cell fallback over any theme
  registry. `Style::from_named` prepares selected chrome without app-side palette
  tables. REEL persists the selection separately from the recorded background.

- Default `native_surface::style::Style`: AMP's Classic Rack palette, original
  1x/2x panel artwork, title strips, centered compact buttons, local hover,
  selection rows, meters and quiet ledger/timeline treatments. `scene` and
  `mount` share physical presentation and pointer placement. AMP, REEL and
  the external starter consume it; primitive-only clients retain a fallback.
  Generic `style::Palette` keeps the compatible `media_controls::Palette` alias.
  Consumer checks can build/test a candidate in a disposable tree with `--cargo`.
- Native surface text can opt into measured horizontal centering. Editor buttons
  use centered labels and even horizontal padding, retaining their full hit area
  and existing vertical alignment. Legacy surface text stays left aligned.
- Frontend OSC 11 background detection reported as optional
  `Capabilities::terminal_background`; legacy capability reports remain valid.
  Live canvas masking respects physical placements of overlaid UI surfaces.
- `Application::close_matching` sends application-defined quit inputs and waits
  for orderly resource cleanup, allowing explicit restarts after local updates
  without killing retained recording or media workers on a timeout.
- Resizing hides live placements and discards obsolete hit targets until a scene
  for the new viewport arrives. Retained pixels are then placed again; stale
  scene geometry cannot trigger a fatal canvas bounds check during the wait.
- Live frame leases remain valid until acknowledgement rather than expiring
  while a frontend is paused. Frontends skip already-retired descriptors from
  older controllers and generation changes while retaining usable pixels.
- Applications can opt into resource-based controller reattachment. Launch
  directories are retained for relative argument matching, including legacy
  sessions. Controller startup failures reach the frontend with their underlying
  error instead of disappearing behind a generic handshake failure.
- Drag input coalesces both at terminal ingress and before a blocked session
  writer, preserving button/key/control ordering within the original 64-entry
  control bound. Terminal restoration drains parsed and partial kernel mouse
  reports before giving cooked input back to the shell.

- Graphics-only local live canvases with bounded latest-frame queues, private
  immutable mapped-frame leases, generation/sequence acknowledgements and
  independent Kitty layers. Raw pixels stay out of the JSON protocol and codecs.
- `ContentView` for shared aspect-fit display/input geometry; timestamped embedded
  key phases, pointer capture, paste and explicit input release. Ordinary `Key`
  and `Pointer` fields retain their existing shape. Relative pointer capture
  remains an explicitly unsupported capability.
- Local `application::Application` lifecycle, opaque domain arguments and
  frontend reattachment; cancellable `Worker` jobs with coalesced progress.
- Render-only timeline transforms/snapping, sliders, buttons and waveform peaks.
- Independent `examples/live-canvas` consumer and real Kitty live-layer proof.

The Cargo version is 0.9.0 for local candidate testing. No 0.9 tag is published
by this change; the starter and four application release pins remain v0.8.1.

## [0.8.1] - 2026-10-13

### Fixed

- ASCII adaptation preserves the columns reserved for combining text, flags and
  ZWJ emoji instead of expanding each codepoint into a replacement character.

## [0.8.0] - 2026-10-13

### Added

- Shared render-only transport controls, native movie track pickers and bounded
  cell grids, extracted from AMP without audio/player dependencies.
- Explicit ASCII/cell/native capability profiles, reduced-color adaptation,
  independent graphics/input/media evidence and inspectable fallback reasons.
- Deterministic presentation replay, bounded agent diagnostics and an independent
  public-API starter with local/SSH native presentation and headless inspection.
- Scoped terminal ownership that restores modes on early returns; restoration
  attempts to release raw keyboard mode even if terminal writes fail.
- Physical text sizing for fixed native artwork, retaining legacy scaling by default.

### Changed

- `terminal-graphics` no longer implies `media`. Enable `media` explicitly for
  video and `audio-output` for PCM frontends. Media descriptions remain codec-free.
- Anonymous PCM caches now enforce per-track/process disk and metadata budgets,
  releasing reservations on close and reporting exhaustion without host fallback.
- CI verifies graphics-only builds without native codec/audio development packages
  and compiles the starter outside the KIT workspace.

## [0.7.0] - 2026-10-13

### Added

- Backend-independent responsive panes with nested rows/columns, wrapping,
  minimum/preferred/maximum sizes, capped weighted growth, priority collapse
  and hiding, explicit fallback reasons and immutable solved hit geometry.
- Shared focus traversal, viewport restoration, exclusive modal scopes and
  origin-based divider capture with cell/pixel unit scaling.
- Renderer-free JSON layout inspection, interactive pane example, property
  tests and a release-mode solver benchmark. The legacy dock API remains available.

## [0.6.1] - 2026-10-13

### Fixed

- Unbridged SSH terminals use bounded video proxies and advertise audio output
  only when the frontend can reach the presentation machine's speakers.
- Remote preview work scales before HDR and subtitle processing, uses checked
  hardware decoding, and resets proxy timestamps across repeated seeks.
- Original-media seeks keep their generation stable during proxy-quality tuning;
  bounded video lookahead allows the shared demuxer to refill audio.
- The graphics benchmark changes rendered row colors on every frame, so its
  incremental payload and timing measurements exercise actual visual updates.

### Added

- Four-application consumer compatibility CI using isolated candidate builds.
- A public framework charter with explicit phased readiness and API boundaries.

## [0.6.0] - 2026-10-12

### Added

- Optional native surface text rules placed after the actual rendered glyph
  edge, including ellipsized text, local font selection and display density.
  Older frontends safely ignore the decoration.

## [0.5.0] - 2026-10-12

### Added

- Negotiated whole-track PCM disk caching, paused background downloads and
  received-buffer progress for SSH audio. Legacy frontends keep bounded credits.

### Fixed

- Audio refill and playback run independently of UI polling, with a larger device
  reserve to tolerate workspace changes and drawing stalls.
- Resize handling measures cells and pixels together, detects settled geometry
  after missed events and invalidates cached presentations before redrawing.

## [0.4.0] - 2026-10-12

### Added

- Negotiated `audio_pcm_v2` SSH audio alongside the existing audio relay.
  Source rate, speaker layout, valid precision and integer/float representation
  travel with bounded, epoch-scoped PCM blocks. Float64 processing provenance,
  credits, device progress and end-of-stream draining preserve audio and timing.
  Shared native output selects a preserving device format or requires explicit
  compatibility conversion, with float64 resampling and TPDF integer dither.
- Terminal bridge attachments can choose their initial cell or pixel
  presentation without starting another SSH connection.

### Fixed

- Native surface row hover can be painted by the frontend in physical pixels,
  without waiting for controller scene rebuilds or sending motion within the
  surface. Negotiated hover fills preserve selection and legacy clients.

- Graphical frontends coalesce queued hover motion before forwarding input,
  avoiding lag and session input queue overflow during fast pointer movement.
  Clicks, drags, wheel input, keyboard events and resizes retain their order.

- Terminal resize generations invalidate graphical image placements and cell
  diff buffers, including rapid shrink/grow sequences that finish at the same
  dimensions. The next frame replaces the complete presentation.

- Kitty graphical sessions use SGR pixel mouse coordinates when geometry is
  measured, avoiding character-cell steps during EQ and other native drags.
  Ordinary terminals and multiplexers retain cell input. The optional
  `pixel_mouse` capability preserves legacy reports, and shutdown/panic restore
  the terminal mode. `scripts/test-pixel-mouse.py` exercises actual SGR input
  through the frontend and relay using the isolated `pixel-mouse-probe` example.

- Native text now contributes glyph opacity on transparent canvases, keeping
  footer labels visible in Kitty without painting the terminal background.

### Added

- Experimental native media previews: bounded FFmpeg posters, H.264/AAC SSH
  proxy, local video scheduling and CPAL audio, generation-scoped seek/cancel,
  priority media transport and a shared native seek timeline.

- **A `webp` feature.** The `image` decoder for WebP, which is what news
  sites serve now, behind its own feature the way GIF and BMP are, so only
  an application that draws pictures out of web pages carries it.

- **How big a cell is, not just how tall** (`Graphics::cell_size`). The
  companion to `cell_aspect`: the measurement the ratio is taken from, in
  pixels, under the same condition -- `None` when nothing has measured one.
  `Resize::Fit` never upsizes, so an application that wants a small picture
  grown has to scale it itself, to the exact pixel size of the rectangle it
  will occupy, and the ratio alone cannot say what that is.

## [0.3.0] - 2026-09-15

### Added

- **A panel's frame, in one place** (`chrome::frame::Frame`, `frame()`,
  `body()`, `NoWords`, `NO_WORDS`, `Badge`, `Tone`). starcord and starfold
  each carried a byte-identical `panels::frame`, and staramp built nine
  frames by hand with four different treatments of the title, the corners
  and the background. The double border, the uppercased title with its
  `— detail`, an optional badge at the right end of the top border, a
  footer of key hints on the bottom border, and the header words are now
  drawn from one struct, which hands back the body a panel draws its own
  contents into. A title that does not fit drops its
  detail before it drops itself, and a panel with no header words reserves
  no header row rather than leaving a blank one. `chrome::rgb`,
  `chrome::empty` and `text::fit` came out with it, lifted from the same
  panels.

- **One scrollbar for every list** (`chrome::scrollbar`: `thumb`, `rows`,
  `virtual_list`, `track`, `track_at`, `render`, `fraction_at`, `GLYPH`,
  `THUMB_MAX_DIV`). A `█` thumb drawn on the panel's right border, in the
  accent, sized by how much of the list is showing but never longer than a
  quarter of the track and never shorter than a row -- the honest ratio
  reads as a second border at the sizes these panels actually run at, and a
  fixed one-row bead throws away the one thing a sized thumb has over a
  scroll percentage: a sense of how much more there is. `thumb` is the
  arithmetic; `rows` and `virtual_list` are the two ways a caller reaches
  it, for a flat list and for one anchored through `VirtualList`; `render`
  draws it and `fraction_at` turns a click or a drag back into a position.
  starcord's chat and staramp's playlist each drew their own version of
  this and every other list in the three applications drew nothing.

- **A scrollbar the mouse can drag** (`chrome::scrollbar`: `grab`, `drag`,
  `Grab`, `index_at`, `virtual_extent`, `Scrollbars`; `list::cursor_into_view`).
  Only starcord's chat dragged, and it did its own hit-testing against
  `fraction_at` and kept its own held-thumb state; staramp's playlist and
  every list in starfold had a bar to look at and nothing to grab. `grab`
  and `drag` are the arithmetic -- a press either takes hold of the row
  under the pointer or, off the thumb, jumps it so its middle lands there,
  and a drag afterwards is the exact inverse of the sizing in `thumb`, run
  through the same geometry so the mouse and the drawing never disagree.
  `Scrollbars` is the part that turns that into three event handlers: it
  keeps the bars drawn each frame and the one grab a button-down is holding,
  across frames and even across a frame that redraws without that bar, until
  `release` lets go. The one thing a caller cannot skip is feeding the
  `above` a drag returns back into the next frame's `record` or `draw` for
  that bar -- a list that instead re-derives its scroll from a cursor every
  frame needs `cursor_into_view` to move the cursor into the new viewport
  too, or the next frame snaps the thumb straight back to where the cursor
  still is. `index_at` and `virtual_extent` are what a variable-height list
  needs to turn a dragged `above` back into `VirtualList::scroll_to`.

- **The overlay a full-screen dialog draws through** (`chrome::overlay`:
  `rect`, `inner`, `Overlay`, `render`). Clear, then the same frame a
  docked panel draws, always focused, its title uppercased -- one shape
  for settings, help and confirmation rather than three that happen to
  agree. `SettingsView` gains a required `footer: &str` field and its
  cursor row is now drawn with `row_cursor_*` in place of
  `row_selected_*`. `HelpView` draws through the overlay too, which gives
  it the `═ ` lead, the uppercase title and `panel_bg` it did not have
  before, and moves "more below" and "the end" out of the title and onto
  the bottom-border footer; its rect is exported as `keymap::help_rect`
  and `HelpView::rect` for the mouse side to test against.

- **A yes/no dialog** (`chrome::confirm`: `Confirm`, `layout`, `render`,
  `answer`, `Answer`, `hit`). One dialog for every place an application
  asks before doing something it cannot undo: a wrapped body, and the
  question's own answer words written on the bottom border as
  `y {yes} · n {no}`, clickable there as well as bound to `y`/`n`/`esc`/
  `ctrl+c`, resolving to an `Answer` of `Yes`, `No`, `Waiting` or `Quit`.
  Enter is deliberately not bound to yes -- starfold asks this before a
  permanent delete, and the key a hand rests on is not the key that should
  answer it.

### Changed

- **Breaking:** `SettingsView` has a new required `footer: &str` field, and
  its cursor row is drawn with `row_cursor_*` rather than `row_selected_*`.
- **Breaking:** `HelpView`'s title no longer says how much more there is to
  scroll -- "more below" and "the end" moved to the bottom-border footer
  the overlay draws -- and the title itself is now uppercased with the
  overlay's `═ ` lead, matching every other panel.
- **Breaking:** The corner gradient is gone. Every panel frame is one border
  colour all the way round -- `border` or `border_focused` -- rather than
  four corners lifted to a different weight and fading back into the edges
  either side. `frame::render_corners` is removed; no consumer called it
  directly, since all three draw their frames through `chrome::frame`.

## [0.2.1] - 2026-09-13

### Fixed

- **The terminal no longer drops out of raw mode under tmux.** The picture
  probe is not asked inside a multiplexer under `auto`. tmux relays the
  query only with `allow-passthrough` on, and when nobody answers, the
  library's reader thread outlives the probe and later restores the cooked
  terminal mode it had saved -- after the application has entered raw mode,
  which left the screen frozen with keystrokes echoing. Half blocks inside
  tmux by default; `graphics = "kitty"` still asks through it on purpose.

## [0.2.0] - 2026-09-13

### Added

- **Re-anchoring a list that grew at the front** (`VirtualList::prepended`).
  A page of history loaded above the view moves every index along, and a view
  that does not move with them jumps back by a page at the moment somebody is
  reading. The anchor is an index, so the answer was always
  `scroll_to(anchor() + n)`; that is now written down as the supported way to
  do it, and `prepended` is the same call with the row *within* the top item
  kept, which matters on a message taller than the viewport.

- **Wrapping text that is already styled** (`wrap::wrap_runs`). The same
  wrapping as `wrap`, over a line that arrives as a list of runs and has to
  leave as one: which bytes of which run are on each row. STAR/CORD was
  writing the cluster loop a second time to produce `Vec<Span>`, and a second
  copy of it is a copy that disagrees with the first about a family emoji --
  which shows up as a message list whose heights and whose drawing are
  measuring different things. A run boundary is not a break opportunity, so a
  word with a bold half and a plain half wraps as a word, and the break space
  is covered by a piece but left out of the drawn ones: a styled space at the
  end of a row is a visible cell of the wrong colour.

- **Two more directories both applications had written out by hand**
  (`Paths::session_file`, `Paths::media_cache_dir`). `<base>/session.toml` for
  what was open when the application was last closed -- data rather than
  config, because nobody typed it, and 0700 rather than 0644, because it names
  everything the user reads -- and `<cache>/media` for the pictures that came
  off somebody else's server, in a directory of its own so that clearing them
  does not take the log with it. STAR/CORD kept a whole copy of `paths.rs` for
  these two; it can now be the thin wrapper the others are.

- **Which settings row is under the pointer** (`chrome::settings::hit`). The
  overlay could be scrolled and chosen from with the keyboard but not clicked,
  because the arithmetic that says where a row was drawn lived inside the
  widget's render loop. It is now a function beside `rect` and `list_rect`,
  which is where both applications keep geometry that drawing and the mouse
  have to agree on.

- **A binding on the slash key, written in the column that prints it**
  (`keymap::alternatives`). The column separates alternatives on `/` and `,`,
  which left the one key every client binds to search unwritable in it:
  STAR/CORD kept a one-entry `UNSPELLABLE` table beside its bindings and
  checked it before the keymap. A separator with nothing on one side of it is
  now the key rather than a separator, so `"ctrl+f / /"` is two spellings of
  search and the help overlay prints what is actually bound.

- **The two columns the help overlay lays out** (`keymap::KEYS_COLUMN`,
  `keymap::GESTURE_COLUMN`). STAR/CORD generates `docs/keys-and-mouse.md` from
  the same table the overlay draws, and a copy of the number in the generator
  is a document that drifts from the screen it documents.

- **Putting the cursor somewhere, and putting a string where it is**
  (`TextInput::set_cursor`, `TextInput::insert_str`). Accepting an
  autocomplete is replacing the typed query with what was chosen and leaving
  the caret after it, and there was no way to say the second half: STAR/CORD
  rebuilt the string, called `set_text`, which parks the caret at the end, and
  then walked it back over the tail one synthetic `Left` key at a time.
  `set_cursor` clamps to the end and rounds down to a character boundary, so
  arithmetic on a string the caller built itself cannot land inside one;
  `insert_str` is the insertion `paste` was already doing privately, minus the
  clipboard's carriage returns.

- **Focus reporting, turned on with everything else** (`term::init`). An
  application has things to stop doing when its window is not in front --
  acknowledging messages somebody else is reading, animating a picture nobody
  can see -- and STAR/CORD was writing the escape for it itself immediately
  after `init`, which left `restore` not knowing to undo it. A terminal that
  does not support it sends neither event, so an application that ignores both
  is where it was.

### Fixed

- **A key arrives spelled the way the table writes it** (`Keymap::resolve`).
  `KeySpec::parse` already normalises what is written -- `shift+tab` is
  `BackTab`, `shift+d` is `D` -- but the arriving event was matched as the
  terminal happened to send it, so a table was correct in one terminal and
  silently wrong in the next, and STAR/CORD had a `normalise` of its own in
  front of every lookup. `resolve` now does it, and resolves presses only: a
  terminal that reports releases sent every action twice.

- **Taking the terminal no longer asks it a question** (`term::init`). ratatui's
  `Terminal::clear` reads the cursor position back so it can put the cursor
  where it found it, which means writing `\e[6n` and blocking on an answer. A
  terminal with nobody behind it -- a tmux pane with no attached client, a bare
  pty, a CI harness -- never answers, and the application hung before its first
  frame with nothing on screen to say why: STAR/CORD would only start with its
  output piped through `cat`, which turns the query into a no-op. The screen is
  now cleared with an escape of this crate's own and the terminal built
  afterwards, over the blank screen its own blank back buffer already assumes.

## [0.1.0] - 2026-09-13

### Added

- **A layout tree for docked panels** (`dock`). One `layout` call per frame
  answers where every panel is, and both drawing and mouse handling read that
  answer; nothing else computes a rect. Splits, fixed and flexible sizes,
  minimums, seams that drag, hidden panels, focus order, and a config shape
  that round-trips through TOML. Placed rects always tile the area exactly;
  what does not fit is reported, not overlapped.

- **A text field** (`input`). One line or several, a cursor that steps by
  cluster so a family emoji is one backspace, the readline keys, bracketed
  paste, and a `render` that says where the terminal's own cursor goes. Any
  key it does not know is reported as such, so an application's `alt+` panel
  keys keep working while the field has focus.

- **Wrapping that reports where it cut** (`wrap`). Byte ranges per row,
  measured per cluster with `unicode-width`, so a message list can know how
  tall a message is before deciding what is on screen and which byte is
  under the pointer. With a bounded cache keyed by text and width.

- **A bottom-anchored virtual list** (`vlist`). Variable-height items,
  stick-to-end that lets go on the first upward scroll and returns when the
  end comes back into view, hit-testing that inverts the layout.

- **Animated GIF frames** (`anim`, feature `gif`). Decoded with caps on
  pixels and frames checked before anything is allocated, a 20 ms floor on
  frame delay, and the time until the next frame for a redraw loop to sleep
  on.

- **A theme file that dresses both applications** (`theme::schema`,
  `theme::resolve`). The format and the derivation chain: every role is
  optional and anything omitted is derived from what was given, so an
  eight-line theme is usable and a two-hundred-line one is exact without two
  code paths. The core resolves the tables every application has -- the
  palette, the chrome, the rows, the status line -- and keeps every other
  table verbatim in `ThemeFile::extra`, handed back by `ThemeFile::table` to
  whichever application asked for it. That is what lets one file carry
  STAR/AMP's `[vis]` and STAR/CORD's `[chat]` at once and neither see the
  other's as an error.

- **Finding a theme, once, for both of them** (`theme::Registry`). Built-in,
  or a file in the user's themes directory, or the desktop's own scheme, in
  that order, with a reason string for what it settled on and a fallback
  rather than a refusal when a config file has a typo in it. Generic over
  what a theme resolves *to*, via `theme::Resolve`, because the lookup is
  shared and the roles are not. The sixteen built-in themes come with it.

- **Following the desktop** (`theme::base16`, `theme::system`). A line scanner
  rather than a YAML dependency -- a scheme file is sixteen `baseXX: value`
  lines, and Stylix's `palette.json` is the same shape -- and the detection
  that turns one into a theme file. A file rather than a resolved theme,
  because each application resolves its own roles out of it.

- **Importing classic Winamp skins** (`theme::wsz`, feature `wsz`). A `.wsz`
  is a ZIP of bitmaps plus `VISCOLOR.TXT` and `PLEDIT.TXT`, which is where an
  imported skin's analyzer colours come from. Behind a feature because only
  one of the two applications imports skins, and it is the only thing here
  that wants a ZIP reader.

- **One set of HTTP defaults** (`net`, feature `net`). Blocking, because
  neither application has an async runtime and a frame never waits on a
  request. The agent names its caller -- MusicBrainz answers 503 to a client
  that does not, and a header that silently failed to apply looks exactly like
  the service being down -- reads statuses rather than having them raised,
  follows five redirects rather than ten, gives up after fifteen seconds, and
  refuses plaintext so that an intercepted 302 cannot put a request back on the
  wire in the clear. `builder` is the same without that last rule, for the one
  caller that points an agent at a socket of its own.

- **The key table, and the overlay that prints it** (`keymap`). `Binding<A>`
  and `MouseHelp` are the one place a key is described, so the dispatcher and
  the help screen cannot drift; `KeySpec::parse` reads the spellings such a
  table already writes for the reader (`ctrl+s`, `alt+1`, `space`, `?`, `F1`,
  `shift+left`), and `Keymap<A>` answers a key press from the table for an
  application whose dispatch is flat. A column may also describe a chord or a
  range, and those are dropped rather than refused: it is written for a person,
  and some of what a person reads is a summary. `HelpView` draws the two
  columns, group headings and all, and says in its title whether there is more
  below.

- **The parts of a panel that are not its contents** (`chrome`). The border
  decoration that makes four corners read as one frame, the row of action words
  at the top, and the settings overlay a panel opens to change itself. The
  header is generic over a `Word` the application owns, because which words a
  panel offers is the application's business and the width of them is all this
  needs to know. Words are dropped from the left as a panel narrows rather than
  the header vanishing whole: losing the way to close a panel because it got
  narrow is worse than losing the way to reorder it. One function places the
  words and both the renderer and the mouse read it, so a word that was never
  drawn cannot be clicked.

- **Pictures on a terminal that will have them** (`graphics`, feature `image`).
  Capability probing, the four modes (`auto`, `kitty`, `blocks`, `off`), the
  cell measurement a font zoom invalidates, and the half-block and `░`
  fallbacks for everything else. `Graphics::probe` must run before raw mode,
  since the terminal's reply arrives on stdin; `term::init` now records that it
  ran and probing afterwards asserts in a debug build and degrades to no
  pictures in a release one, rather than reading the user's keystrokes as the
  terminal's answer.

- **One cache for every encoded picture** (`graphics::ImageCache`). Keyed by
  what the picture *is* rather than by any name the application knows it by --
  keying a cover on its track served the second cover of an album as the first
  -- and by its placement in cells *and* in pixels, which are two different
  facts: four cells of seven pixels and two of fourteen are both twenty-eight,
  and a protocol placed over cells it was not built for leaves the rest showing
  whatever the terminal had there. Least-recently-used, because what is on the
  other side of it is the terminal's own memory, with `forget`, `forget_all`
  and a `forget_unused` for a view that knows what it just drew.

- **Shapes drawn as pixels rather than as glyphs** (`graphics::raster`). A
  polygon scan-converter that anti-aliases by supersampling the same shape test
  that decides the fill, so an edge cannot disagree with its interior. A
  terminal program cannot ship a font: a text icon is drawn by whatever typeface
  the terminal fell back to, and two machines set to different fonts draw two
  different rows of buttons from the same bytes.

- **Refusing a picture that lies about its size** (`graphics::decode_limited`).
  The `image` crate's defaults allow any dimensions and cap only the total
  allocation, at 512 MiB, so a few kilobytes claiming to be 10000x10000 turn
  into four hundred megabytes before anything notices. The header is checked
  before the pixels are read.

- **Colour and the perceptual maths under the theme engine** (`theme::color`).
  `Rgb`, hex parsing, WCAG contrast and luminance, Oklab conversion, mixing,
  ramps, and the two contrast repairs the derivation chain leans on. Blending
  in sRGB gives muddy midpoints; Oklab is perceptually uniform, which is what
  lets a theme omit a role and have the derived one look deliberate.

- **Editing one setting in a config file without disturbing the rest**
  (`config::edit`). A line editor rather than a TOML round-trip, because
  serialising the whole struct back throws away every comment, the section
  order, and any key the application does not know about. Property tests come
  with it: the file being edited is one a person typed.

- **Where an application keeps its files** (`paths::Paths`). One directory per
  application -- `~/.local/<app>`, overridable with `$<APP>_DIR` -- rather than
  three XDG roots, so a whole setup can be backed up or deleted by moving one
  folder. Const-constructible, and it names its own environment variables,
  because STAR/CORD honouring `STARAMP_DIR` would be a surprise.

- **File logging** (`logging::init`). To `<cache>/<app>.log`, filtered by
  `$<APP>_LOG` or by the `-v` flag, and covering this crate as well as the
  application: half of what an application does now happens in here, and a
  default filter naming only the application turns all of it off. Never
  stdout: it corrupts the alternate screen, and a TUI that scribbles on itself
  when something goes wrong is worse than one that says nothing.

- **Writes that cannot be half-done** (`fs::write_atomic`, `fs::write_private`).
  Temporary file in the same directory, then a rename. Everything either
  application saves while running is written over its own previous version, and
  a truncating write that is interrupted leaves a file that parses as far as it
  got. The private variant sets 0600 when the file is *created*, so a token is
  never briefly world-readable.

- **Terminal setup and teardown** (`term`). Raw mode, the alternate screen,
  mouse capture, bracketed paste, and the panic hook that undoes all of it
  before the panic is printed. Without the hook a crash leaves the user with no
  echo, no cursor and a scrambled screen, blind-typing `reset`.

- **Fitting text into columns** (`text::marquee`, `text::truncate`). Both
  measure display width rather than counting characters, because half the
  interesting titles are CJK or carry an emoji and `chars().take(n)` overflows
  the panel on one and cuts a character in half on the other.

- **Keeping a cursor on screen** (`list::clamp_scroll`). The arithmetic every
  fixed-height list in both applications was writing out for itself. A
  `VirtualList` for variable-height rows belongs beside it and is not here yet.

- **Pointer arithmetic and double clicks** (`mouse`). `hit` against a `Rect`,
  and a `ClickTracker` that pairs two clicks within 450 ms on the same cell.
  The same cell as well as the same moment: a second click a row away is a new
  selection, not an activation of the first. It clears on a pair, so a rapid
  run fires on every second click rather than on every click after the first.

- **A seven-segment font in three rows** (`digits`). Box-drawing characters
  rather than a bitmap, so it needs no font the terminal might not have.

- **The crate itself, and one copy of ratatui for both applications.**
  `ratatui`, `crossterm` and, under the `image` feature, `ratatui-image` and
  `image` are re-exported from the crate root. A widget implemented against a
  second copy of ratatui does not satisfy a signature expecting the first, and
  the compiler reports that as a mismatch between two versions carrying the
  same number. Taking all four from here makes that impossible rather than
  unlikely.
