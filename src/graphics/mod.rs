//! Pictures, and getting them onto a terminal.
//!
//! Half blocks put two pixels in a cell, which is enough to tell one album
//! cover from another and not enough to read the title on it. Kitty's graphics
//! protocol, and sixel and iTerm2's, put the actual image there instead.
//!
//! The awkward part is the probe. Asking a terminal what it supports means
//! writing an escape sequence and reading the reply off stdin, and that cannot
//! be done once the application owns the keyboard -- the reply would arrive as
//! keystrokes. So the probe happens before the alternate screen, at startup,
//! and its answer is carried in. [`term::init`](crate::term::init) records that
//! it ran, and [`Graphics::probe`] refuses to run afterwards.
//!
//! ## Decoding
//!
//! Every image either application decodes came from outside it: embedded in a
//! tag, sitting in an album folder, inside a downloaded skin, or fetched from
//! a CDN. The `image` crate's own defaults allow any width and height and cap
//! only the total allocation, at 512 MiB -- so a file of a few kilobytes that
//! declares itself 10000x10000 is decoded into four hundred megabytes before
//! anything notices it is absurd, on a worker a track change is waiting for.
//!
//! Dimensions are checked against the header before the pixels are read, which
//! is the difference between refusing a lie and allocating for it.

pub mod cache;
pub mod raster;

pub use cache::{ImageCache, ImageId, Key};

use std::collections::HashSet;
use std::sync::Arc;

use ratatui::buffer::Buffer;
use ratatui::layout::{Rect, Size};
use ratatui::style::{Color, Style};
use ratatui_image::picker::cap_parser::QueryStdioOptions;
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::protocol::Protocol;
use ratatui_image::{FontSize, Resize};

/// The largest picture worth decoding, on a side.
///
/// Well above any real cover -- the Cover Art Archive's largest is 1200 -- and
/// far enough above a scan that nothing legitimate is refused.
pub const MAX_DIMENSION: u32 = 8192;

/// Decode `bytes`, refusing anything larger than `max_dim` on a side.
pub fn decode_limited(bytes: &[u8], max_dim: u32) -> image::ImageResult<image::DynamicImage> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(image::ImageError::IoError)?;
    reader.limits(limits(max_dim));
    reader.decode()
}

/// Decode the file at `path`, with the same limits.
///
/// Streams rather than reading the whole file first, so a huge file on disk is
/// refused after its header rather than after its bytes.
pub fn open_limited(
    path: &std::path::Path,
    max_dim: u32,
) -> image::ImageResult<image::DynamicImage> {
    let mut reader = image::ImageReader::open(path)?
        .with_guessed_format()
        .map_err(image::ImageError::IoError)?;
    reader.limits(limits(max_dim));
    reader.decode()
}

fn limits(max_dim: u32) -> image::Limits {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(max_dim);
    limits.max_image_height = Some(max_dim);
    // Below the 512 MiB default, and still far more than any cover needs:
    // 8192 squared at four bytes a pixel is 256 MiB, so this permits the
    // largest picture the dimensions allow and nothing beyond it.
    limits.max_alloc = Some(256 * 1024 * 1024);
    limits
}

/// What the user asked for, from `[ui] graphics`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Ask the terminal and believe it.
    #[default]
    Auto,
    /// Insist on kitty. Over ssh and inside multiplexers the probe sometimes
    /// says no when the answer is yes.
    Kitty,
    /// Never use a protocol; draw pictures as half blocks.
    Blocks,
    /// Draw no picture at all.
    Off,
}

impl Mode {
    /// The name this mode has in `config.toml`, which is also the one worth
    /// showing: a setting should be listed by the word you would type.
    pub fn name(self) -> &'static str {
        match self {
            Mode::Auto => "auto",
            Mode::Kitty => "kitty",
            Mode::Blocks => "blocks",
            Mode::Off => "off",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "kitty" => Mode::Kitty,
            "blocks" | "halfblocks" => Mode::Blocks,
            "off" | "none" => Mode::Off,
            _ => Mode::Auto,
        }
    }
}

/// What the terminal can do, and what has been built for it.
pub struct Graphics {
    mode: Mode,
    /// What the terminal said it could do, kept so the setting can be changed
    /// back without asking again -- which cannot be done once the alternate
    /// screen is up, since the reply would arrive as keystrokes.
    probed: Option<Picker>,
    /// `None` when the terminal has no protocol, or the user asked for none.
    picker: Option<Picker>,
    cache: ImageCache,
}

impl Graphics {
    /// Ask the terminal what it can do.
    ///
    /// **Must be called before the alternate screen is entered**, and before
    /// raw mode: the query's reply comes back on stdin, and once the
    /// application is reading keys it would be read as one. The ordering is not
    /// expressible in the type system across a crate boundary, so
    /// [`term::init`](crate::term::init) records that it ran and this
    /// debug-asserts against it. A debug build panics; a release build degrades
    /// to [`Graphics::disabled`] rather than reading the user's keystrokes and
    /// reporting whatever they typed as the terminal's answer.
    ///
    /// Never fails otherwise. A terminal that does not answer, or answers
    /// badly, simply gets half blocks.
    pub fn probe(mode: Mode) -> Self {
        let entered = crate::term::entered();
        debug_assert!(
            !entered,
            "Graphics::probe ran after term::init: the capability reply arrives as keystrokes"
        );
        if entered {
            tracing::warn!("graphics: probed after raw mode was entered; drawing no pictures");
            return Self::disabled();
        }

        // Asked whatever the picture setting says, because covers are not the
        // only thing drawn this way: an application's own rasterised icons are
        // too, and turning covers off is not a statement about them.
        let probed = match mode {
            // Only ask something that will answer.
            //
            // Not squeamishness about the cost: a terminal that answers the
            // query does it in single-digit milliseconds, and one that answers
            // *anything* -- even "no graphics here" -- costs nothing either,
            // because the reply ends the probe. The danger is the thing that
            // answers nothing at all, which is not a terminal so much as a pty
            // with nobody behind it: a bare `script`, some CI harnesses. There
            // the probe's reader thread is still sitting on stdin when it gives
            // up, and it eats the backend's own cursor-position reply, and the
            // first frame never comes. Measured: 0.11s to draw when the far end
            // answers anything, and no frame at all when it answers nothing.
            //
            // So the question is not "can this terminal draw pixels" but "is
            // anyone listening", and the environment is read for that.
            Mode::Auto | Mode::Off | Mode::Blocks if behind_multiplexer() => {
                tracing::info!(
                    "graphics: inside tmux, drawing half blocks; set graphics = \"kitty\" to ask through it"
                );
                None
            }
            Mode::Auto | Mode::Off | Mode::Blocks if !looks_capable() => {
                tracing::debug!("graphics: nothing in the environment suggests a protocol");
                None
            }
            _ => match Picker::from_query_stdio_with_options(QueryStdioOptions {
                // A quarter of a second. The library's own default is two, and
                // a terminal that is going to answer does so in single-digit
                // milliseconds -- the wait is only ever paid by one that will
                // not answer at all, and paying two seconds of it before the
                // first frame is worse than drawing half blocks.
                timeout: std::time::Duration::from_millis(250),
                ..Default::default()
            }) {
                Ok(p) => {
                    // Said out loud, because a terminal that answers the query
                    // and then names half blocks looks exactly like one that
                    // never answered: everything comes out as text, and nothing
                    // before this said which it was.
                    tracing::debug!(
                        "graphics: terminal answered with {:?}, cell {}x{}",
                        p.protocol_type(),
                        p.font_size().width,
                        p.font_size().height
                    );
                    Some(p)
                }
                Err(e) => {
                    tracing::debug!("no graphics protocol: {e}");
                    None
                }
            },
        };
        let mut g = Self {
            mode,
            probed,
            picker: None,
            cache: ImageCache::default(),
        };
        g.apply();
        if let Some(p) = &g.picker {
            tracing::info!("graphics: {:?}", p.protocol_type());
        }
        g
    }

    /// Probe, but only if this is really a terminal.
    ///
    /// Piping an application's output somewhere is not a normal thing to do,
    /// but a query written into a pipe waits for a reply that will never come.
    /// Infallible on purpose: no capability check is worth refusing to start
    /// over.
    pub fn probe_if_tty(mode: Mode) -> Self {
        use std::io::IsTerminal;
        if !std::io::stdout().is_terminal() || !std::io::stdin().is_terminal() {
            return Self::disabled();
        }
        let g = Self::probe(mode);
        drain_stdin();
        g
    }

    /// Half blocks with no probe at all, for tests and for `graphics = "off"`.
    pub fn disabled() -> Self {
        Self {
            mode: Mode::Off,
            probed: None,
            picker: None,
            cache: ImageCache::default(),
        }
    }

    /// Change how pictures are drawn.
    ///
    /// Returns false when the choice cannot be honoured for the rest of this
    /// run: `auto` re-detects nothing, because detection can only happen
    /// before the alternate screen. Saying so is better than appearing to do
    /// nothing.
    pub fn set_mode(&mut self, mode: Mode) -> bool {
        self.mode = mode;
        self.apply();
        !(mode == Mode::Auto && self.probed.is_none())
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// Point the picker at whatever the current mode asks for.
    fn apply(&mut self) {
        // The built images belong to the old renderer.
        self.cache.clear();
        self.picker = match self.mode {
            Mode::Off | Mode::Blocks => None,
            // Half blocks are drawn by the caller, which knows the theme;
            // there is nothing for a picker to add.
            Mode::Auto => self
                .probed
                .clone()
                .filter(|p| p.protocol_type() != ProtocolType::Halfblocks),
            Mode::Kitty => {
                // Being told to use kitty is the point of this setting, so it
                // is honoured even when nothing was detected -- over ssh and
                // inside a multiplexer the outer terminal cannot be seen from
                // here. The font size comes from the probe when there was one.
                let mut p = self.probed.clone().unwrap_or_else(Picker::halfblocks);
                p.set_protocol_type(ProtocolType::Kitty);
                Some(p)
            }
        };
    }

    /// Take the cell size again, after the terminal changed shape.
    ///
    /// The probe measures a cell once, at startup, and a font zoom changes
    /// it. That matters more for a rasterised icon than for a photograph: the
    /// photograph is fitted to its area and merely goes soft, but an icon is
    /// built at the cell size in pixels, and kitty places it by its pixel size
    /// -- the library sends no cell count with the placement -- so after a zoom
    /// the old image covers fewer cells than the icon and the rest of it shows
    /// through. Measured from the window's pixel size, which the terminals with
    /// a graphics protocol all report; where it is not reported nothing
    /// changes.
    pub fn remeasure(&mut self) {
        let Ok(ws) = crossterm::terminal::window_size() else {
            return;
        };
        let Some(cell) = cell_size(ws.columns, ws.rows, ws.width, ws.height) else {
            return;
        };
        let mut changed = false;
        for p in [&mut self.probed, &mut self.picker].into_iter().flatten() {
            let had = p.font_size();
            if had.width != cell.width || had.height != cell.height {
                *p = with_cell(p, cell);
                changed = true;
            }
        }
        if changed {
            tracing::debug!("graphics: cell is now {}x{} px", cell.width, cell.height);
            self.cache.clear();
        }
    }

    /// The picker for pictures the application rasterises itself.
    ///
    /// Not the photographs': `[ui] graphics` is about those, and `off` or
    /// `blocks` there leaves an icon drawn as a picture wherever the terminal
    /// can manage one. Forcing `kitty` is honoured for both, since it exists
    /// for the terminals the probe cannot see.
    fn pixel_picker(&self) -> Option<&Picker> {
        match self.mode {
            Mode::Kitty => self.picker.as_ref(),
            _ => self
                .probed
                .as_ref()
                .filter(|p| p.protocol_type() != ProtocolType::Halfblocks),
        }
    }

    /// Could a rasterised picture be drawn if asked? Ignores any application
    /// setting, so the UI can say whether text is a choice or the only thing
    /// available.
    pub fn pictures_available(&self) -> bool {
        self.pixel_picker().is_some()
    }

    /// Log what this will actually do. Called once the picker is settled, so
    /// it reports the decision rather than the inputs.
    pub fn log_capabilities(&self) {
        tracing::debug!(
            "graphics: mode {}, protocol {}, pictures available {}",
            self.mode.name(),
            self.name(),
            self.pictures_available(),
        );
    }

    /// How tall a terminal cell is relative to its width.
    ///
    /// The usual assumption is two, and it is only ever approximately true:
    /// kitty reports its real cell size, and at a typical font that is nearer
    /// 2.1 or 2.4. It matters because it decides how many columns make a
    /// square, and getting it wrong leaves a picture letterboxed inside the
    /// space reserved for it rather than filling it.
    ///
    /// `None` when nothing has measured it, in which case two is as good a
    /// guess as any.
    pub fn cell_aspect(&self) -> Option<f32> {
        let f = self.picker.as_ref()?.font_size();
        (f.width > 0 && f.height > 0).then(|| f.height as f32 / f.width as f32)
    }

    /// What is actually in use, for the help overlay.
    pub fn name(&self) -> &'static str {
        match self.picker.as_ref().map(|p| p.protocol_type()) {
            Some(ProtocolType::Kitty) => "kitty",
            Some(ProtocolType::Sixel) => "sixel",
            Some(ProtocolType::Iterm2) => "iterm2",
            _ if self.mode == Mode::Off => "off",
            _ => "half blocks",
        }
    }

    /// How many built protocols to keep. See [`cache::DEFAULT_CAPACITY`].
    pub fn set_capacity(&mut self, entries: usize) {
        self.cache.set_capacity(entries);
    }

    /// A protocol for this image at this size, building one if need be.
    ///
    /// `id` is what the picture is, to the cache: [`ImageId::of_arc`] where the
    /// application has nothing better, since two pictures the caller cannot
    /// tell apart are still two pictures. The `Arc` is held for as long as the
    /// protocol is, so an address taken as an identity cannot be freed and
    /// handed back out to the next picture along.
    ///
    /// `None` means "draw it yourself": either there is no protocol, or
    /// encoding failed, and in both cases [`halfblocks`] is the right answer
    /// rather than an empty panel.
    pub fn protocol(
        &mut self,
        id: ImageId,
        img: &Arc<image::RgbaImage>,
        area: Rect,
    ) -> Option<&Protocol> {
        if area.width == 0 || area.height == 0 {
            return None;
        }
        let picker = self.picker.as_ref()?;
        let key = placement(id, picker, area);
        if !self.cache.contains(&key) {
            let dynamic = image::DynamicImage::ImageRgba8((**img).clone());
            match picker.new_protocol(dynamic, key.cells, Resize::Fit(None)) {
                Ok(p) => self.cache.insert(key, Some(Arc::clone(img)), p),
                Err(e) => {
                    tracing::debug!("encoding a picture failed: {e}");
                    return None;
                }
            }
        }
        self.cache.get(&key)
    }

    /// A picture the size of its cells, drawn on demand and kept afterwards.
    ///
    /// `build` is handed the rectangle's size in pixels and draws it; it is
    /// called only when nothing has been built for `id` at this placement, so
    /// it is the expensive half and runs once. `id` must name everything
    /// `build` reads -- which shape, in which colours -- or a theme change
    /// serves the old colours back; [`ImageId::of`] over those inputs is what
    /// that looks like.
    ///
    /// `None` when there is no protocol, or the terminal never said how big a
    /// cell is -- in which case there is nothing to size the image to, and the
    /// caller draws its text fallback instead.
    pub fn raster(
        &mut self,
        id: ImageId,
        area: Rect,
        build: impl FnOnce(u32, u32) -> image::RgbaImage,
    ) -> Option<&Protocol> {
        if area.width == 0 || area.height == 0 {
            return None;
        }
        let picker = self.pixel_picker()?;
        let cell = picker.font_size();
        if cell.width == 0 || cell.height == 0 {
            return None;
        }
        let key = placement(id, picker, area);
        if !self.cache.contains(&key) {
            let img = build(key.pixels.0, key.pixels.1);
            let dynamic = image::DynamicImage::ImageRgba8(img);
            // Built before the cache is touched: the picker is borrowed from
            // `self`, and the borrow has to end before the insert.
            match picker.new_protocol(dynamic, key.cells, Resize::Fit(None)) {
                Ok(p) => self.cache.insert(key, None, p),
                Err(e) => {
                    tracing::debug!("encoding a picture failed: {e}");
                    return None;
                }
            }
        }
        self.cache.get(&key)
    }

    /// Forget every built size of one picture.
    ///
    /// Kitty keeps uploaded images in the terminal's own memory keyed by id;
    /// dropping the protocol is what releases one.
    pub fn forget(&mut self, id: ImageId) {
        self.cache.forget(id);
    }

    /// Forget everything built so far.
    pub fn forget_all(&mut self) {
        self.cache.clear();
    }

    /// Forget every picture not named in `keep`.
    ///
    /// For a view that collects, as it draws, the pictures it actually put on
    /// screen: everything else has scrolled off, and it costs terminal memory
    /// until something says so.
    pub fn forget_unused(&mut self, keep: &HashSet<ImageId>) {
        self.cache.forget_unused(keep);
    }
}

/// The cache key for `area`, measured with `picker`'s cell.
fn placement(id: ImageId, picker: &Picker, area: Rect) -> Key {
    let cell = picker.font_size();
    Key {
        id,
        cells: Size::new(area.width, area.height),
        pixels: (
            area.width as u32 * cell.width as u32,
            area.height as u32 * cell.height as u32,
        ),
    }
}

/// Upper half block: two rows of pixels in one cell, the top from the
/// foreground and the bottom from the background.
pub const HALF: char = '\u{2580}';

/// Draw a picture by sampling it into half-block cells.
///
/// Every cell carries two pixels, so a six-row rect is twelve pixels tall. It
/// is coarse, and it works in every terminal without a graphics protocol, which
/// is what makes it the floor rather than the ceiling. Also the answer for a
/// rectangle a protocol cannot honour: kitty tolerates a placement clipped by
/// the edge of the screen and sixel does not, so a clipped picture is drawn
/// this way for the rows that are visible.
///
/// Alpha is ignored. A terminal cell has one background colour and no way to
/// say what is behind it, so there is nothing to composite against.
pub fn halfblocks(img: &image::RgbaImage, area: Rect, buf: &mut Buffer) {
    if area.width == 0 || area.height == 0 || img.width() == 0 || img.height() == 0 {
        return;
    }
    let (iw, ih) = (img.width(), img.height());
    let rows = area.height as u32 * 2;

    for cy in 0..area.height {
        for cx in 0..area.width {
            // Two samples per cell: the upper and lower halves.
            let sample = |half: u32| {
                let py = cy as u32 * 2 + half;
                let sx = (cx as u32 * iw / area.width as u32).min(iw - 1);
                let sy = (py * ih / rows).min(ih - 1);
                let p = img.get_pixel(sx, sy);
                Color::Rgb(p[0], p[1], p[2])
            };
            buf[(area.x + cx, area.y + cy)]
                .set_char(HALF)
                .set_style(Style::default().fg(sample(0)).bg(sample(1)));
        }
    }
}

/// A quiet stand-in where a picture will go, rather than a hole in the panel.
pub fn placeholder(area: Rect, buf: &mut Buffer, style: Style) {
    for y in 0..area.height {
        for x in 0..area.width {
            buf[(area.x + x, area.y + y)]
                .set_char('\u{2591}')
                .set_style(style);
        }
    }
}

/// Repair the cursor dance the kitty renderer leaves in a one-cell image.
///
/// The library writes a whole image row into its first cell: save the
/// cursor, the placeholders, restore, then move right by the width less one
/// and down by the height less one, so ratatui's idea of where the cursor is
/// holds. For a single cell both moves are zero, and a zero parameter to
/// those controls means one -- ECMA-48, and every terminal follows it -- so
/// the cursor ends a row too low. ratatui then prints the rest of the line
/// where it believes the cursor to be, which is one row down: the playing
/// row's text appeared twice, once on the row below. Wider images are fine
/// because the next drawn cell is never adjacent and ratatui moves to it
/// explicitly.
///
/// Fixed by rewriting the two zero moves as the one forward step ratatui
/// expects. Only that exact tail is touched, so a library that stops
/// emitting it is left alone.
pub fn mend_unit_placeholder(buf: &mut Buffer, x: u16, y: u16) {
    const BAD: &str = "\x1b[0C\x1b[0B";
    const GOOD: &str = "\x1b[1C";
    let Some(cell) = buf.cell_mut((x, y)) else {
        return;
    };
    let symbol = cell.symbol();
    if let Some(head) = symbol.strip_suffix(BAD) {
        let mended = format!("{head}{GOOD}");
        cell.set_symbol(&mended);
    }
}

/// A cell's size in pixels, from the window's size in both cells and pixels.
///
/// `None` when either is unknown: a terminal that does not report pixels
/// says zero, and dividing by it would say the same.
fn cell_size(columns: u16, rows: u16, width_px: u16, height_px: u16) -> Option<FontSize> {
    if columns == 0 || rows == 0 || width_px == 0 || height_px == 0 {
        return None;
    }
    Some(FontSize {
        width: width_px / columns,
        height: height_px / rows,
    })
}

/// The same picker at another cell size.
///
/// The library measures once and offers no way to say otherwise, so this is
/// a new picker carrying the old one's protocol. What the constructor reads
/// from the environment -- tmux, iTerm2 -- it reads the same way the probe
/// did, and the capabilities it cannot know are informational only.
fn with_cell(p: &Picker, cell: FontSize) -> Picker {
    #[allow(deprecated)]
    let mut n = Picker::from_fontsize(cell);
    n.set_protocol_type(p.protocol_type());
    n
}

/// Is there a terminal on the other end that will answer a question?
///
/// Two ways to be sure. Either the environment names a terminal that speaks a
/// graphics protocol -- which is the local case, where these variables are
/// actually set -- or this is an ssh session, where a real terminal is by
/// definition attached and only `TERM` survives the hop.
///
/// That second clause is the whole point. `TERM_PROGRAM` and
/// `WEZTERM_EXECUTABLE` are local variables that ssh does not forward, and a
/// terminal whose terminfo is not installed on the far end gets `TERM`
/// normalised to `xterm-256color` -- which is how Ghostty and WezTerm, both of
/// which speak the kitty protocol, ended up drawing half blocks over ssh
/// without ever being asked. Being on the far end of an ssh connection is
/// itself the evidence that somebody is listening, so ask.
fn looks_capable() -> bool {
    use std::env::var;
    let term = var("TERM").unwrap_or_default().to_ascii_lowercase();
    let program = var("TERM_PROGRAM").unwrap_or_default().to_ascii_lowercase();

    term.contains("kitty")
    || term.contains("ghostty")
    || term.contains("wezterm")
    || term.contains("sixel")
    || term.starts_with("foot")
    || term.starts_with("contour")
    || matches!(program.as_str(), "wezterm" | "iterm.app" | "ghostty")
    || var("KITTY_WINDOW_ID").is_ok()
    || var("GHOSTTY_RESOURCES_DIR").is_ok()
    || var("WEZTERM_EXECUTABLE").is_ok()
    || var("KONSOLE_VERSION").is_ok()
    // Over ssh the outer terminal is invisible from here and `TERM` has very
    // likely been flattened on the way, so ask: the terminal at the far end
    // answers the query itself, and it answers quickly.
    || var("SSH_TTY").is_ok()
    || var("SSH_CONNECTION").is_ok()
}

/// Whether a multiplexer sits between this process and the terminal.
///
/// Inside tmux the query is not asked at all under `auto`, and that is a
/// safety rule rather than a capability judgement. tmux only relays the
/// query when `allow-passthrough` is on, and a query nobody relays is one
/// nobody answers. The library that asks it does so on a thread that puts
/// the terminal into raw mode itself and puts it *back* when it finishes --
/// and when the answer never comes, that thread outlives the probe, sits on
/// stdin until the person types something, and then restores the cooked
/// mode it saved: after the application has taken the terminal. The screen
/// stops updating and keystrokes echo. Measured in tmux with the query
/// timing out every time; never without it. A person who has turned
/// passthrough on can still say `graphics = "kitty"` and get the query.
fn behind_multiplexer() -> bool {
    std::env::var("TMUX").is_ok()
}

/// Throw away whatever the terminal said back.
///
/// A capability query is a conversation, and terminals are not obliged to
/// answer only the questions asked -- a reply that arrives late, or one the
/// probe did not consume, is still sitting in the input buffer afterwards.
/// Left there it is read as keystrokes, and the first thing that reads it is
/// the backend's own cursor-position query, which then fails and takes startup
/// with it.
fn drain_stdin() {
    use crossterm::event::{poll, read};
    use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
    use std::time::Duration;

    if enable_raw_mode().is_err() {
        return;
    }
    // A bounded number of reads: a terminal that talks forever must not be
    // able to hold startup open.
    for _ in 0..64 {
        match poll(Duration::from_millis(20)) {
            Ok(true) => {
                if read().is_err() {
                    break;
                }
            }
            _ => break,
        }
    }
    let _ = disable_raw_mode();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A header is a claim, not a fact. Refusing it costs a few bytes of
    /// parsing; believing it costs however much memory it asked for.
    #[test]
    fn a_picture_larger_than_the_limit_is_refused_before_it_is_decoded() {
        // 64x64 is a real image; the limit here is deliberately smaller.
        let small = image::RgbImage::from_pixel(64, 64, image::Rgb([1, 2, 3]));
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(small)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let bytes = png.into_inner();

        assert!(
            decode_limited(&bytes, 32).is_err(),
            "a 64x64 picture was decoded under a 32-pixel limit"
        );
        let ok = decode_limited(&bytes, MAX_DIMENSION).expect("an ordinary picture was refused");
        assert_eq!((ok.width(), ok.height()), (64, 64));
    }

    #[test]
    fn rubbish_is_an_error_rather_than_a_panic() {
        for bytes in [&b""[..], &b"not a picture"[..], &[0xff; 64][..]] {
            assert!(decode_limited(bytes, MAX_DIMENSION).is_err());
        }
    }

    #[test]
    fn every_mode_name_parses_back_to_itself() {
        // The name is written into config.toml and read back on the next run,
        // so a mode that cannot survive the round trip is a setting that
        // silently resets.
        for m in [Mode::Auto, Mode::Kitty, Mode::Blocks, Mode::Off] {
            assert_eq!(Mode::parse(m.name()), m, "{} did not round trip", m.name());
        }
    }

    #[test]
    fn the_mode_names_are_the_ones_documented_in_the_config() {
        assert_eq!(Mode::parse("auto"), Mode::Auto);
        assert_eq!(Mode::parse("kitty"), Mode::Kitty);
        assert_eq!(Mode::parse("blocks"), Mode::Blocks);
        assert_eq!(Mode::parse("off"), Mode::Off);
        // A typo falls back to detection rather than to nothing: an unreadable
        // setting should not cost the user their pictures.
        assert_eq!(Mode::parse("KITTEN"), Mode::Auto);
        assert_eq!(Mode::parse(""), Mode::Auto);
    }

    #[test]
    fn without_a_protocol_the_caller_is_told_to_draw_it_itself() {
        let mut g = Graphics::disabled();
        let img = Arc::new(image::RgbaImage::from_pixel(
            4,
            4,
            image::Rgba([1, 2, 3, 255]),
        ));
        assert!(g
            .protocol(ImageId::of_arc(&img), &img, Rect::new(0, 0, 12, 6))
            .is_none());
        assert!(g
            .raster(ImageId::of(&"icon"), Rect::new(0, 0, 2, 1), |w, h| {
                image::RgbaImage::new(w, h)
            })
            .is_none());
        assert!(!g.pictures_available());
        assert_eq!(g.name(), "off");
    }

    #[test]
    fn a_cell_is_the_window_in_pixels_over_the_window_in_cells() {
        let c = cell_size(100, 50, 800, 850).unwrap();
        assert_eq!((c.width, c.height), (8, 17));
        assert!(cell_size(100, 50, 0, 0).is_none(), "no pixels reported");
        assert!(cell_size(0, 0, 800, 850).is_none(), "no cells reported");
    }

    #[test]
    fn a_one_cell_placeholder_ends_with_a_single_step_right() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 4, 2));
        // What the library writes for a 1x1 image, minus the placeholder
        // body, which is not what is being tested.
        buf[(1, 0)].set_symbol("\x1b[s\u{10EEEE}\x1b[u\x1b[0C\x1b[0B");
        mend_unit_placeholder(&mut buf, 1, 0);
        assert_eq!(buf[(1, 0)].symbol(), "\x1b[s\u{10EEEE}\x1b[u\x1b[1C");
        // A wider image's tail, and an ordinary cell, are left alone.
        buf[(2, 0)].set_symbol("\x1b[u\x1b[3C\x1b[2B");
        mend_unit_placeholder(&mut buf, 2, 0);
        assert_eq!(buf[(2, 0)].symbol(), "\x1b[u\x1b[3C\x1b[2B");
        mend_unit_placeholder(&mut buf, 3, 0);
        assert_eq!(buf[(3, 0)].symbol(), " ");
    }

    /// A picture is transmitted as pixels and placed over cells, and those are
    /// two different facts. Keyed on pixels alone, four cells of seven pixels
    /// and two cells of fourteen are the same entry -- so a picture spanning
    /// four cells was served one built to cover two, and the two it did not
    /// cover kept whatever the terminal already had there. On a paused player
    /// that was the lit pause icon, sitting in the right-hand half of the
    /// next-track button.
    #[test]
    fn a_picture_for_four_cells_is_not_reused_for_two() {
        #[allow(deprecated)]
        let small = Picker::from_fontsize(FontSize {
            width: 7,
            height: 16,
        });
        // Twice as wide a cell, the same height: contrived so that both
        // dimensions collide at once, which is what the key has to survive.
        #[allow(deprecated)]
        let large = Picker::from_fontsize(FontSize {
            width: 14,
            height: 16,
        });

        let mut g = Graphics::disabled();
        g.mode = Mode::Kitty;
        let id = ImageId::of(&"next");
        let build = |w: u32, h: u32| image::RgbaImage::from_pixel(w, h, image::Rgba([7; 4]));

        // Four cells at seven pixels, then two cells at fourteen. Both are
        // 28 by 48 pixels, and they must not be the same entry.
        g.picker = Some(small);
        assert!(g.raster(id, Rect::new(0, 0, 4, 3), build).is_some());
        g.picker = Some(large);
        assert!(g.raster(id, Rect::new(0, 0, 2, 3), build).is_some());

        assert_eq!(
            g.cache.len(),
            2,
            "the two-cell picture was served the four-cell picture's entry"
        );
        // And each is held against the cells it was actually built to cover.
        assert!(g.cache.contains(&Key {
            id,
            cells: Size::new(4, 3),
            pixels: (28, 48)
        }));
        assert!(g.cache.contains(&Key {
            id,
            cells: Size::new(2, 3),
            pixels: (28, 48)
        }));
    }

    #[test]
    fn a_picture_is_drawn_once_and_served_afterwards() {
        #[allow(deprecated)]
        let picker = Picker::from_fontsize(FontSize {
            width: 8,
            height: 16,
        });
        let mut g = Graphics::disabled();
        g.mode = Mode::Kitty;
        g.picker = Some(picker);

        let drawn = std::cell::Cell::new(0u32);
        let id = ImageId::of(&"play");
        for _ in 0..5 {
            assert!(g
                .raster(id, Rect::new(0, 0, 2, 1), |w, h| {
                    drawn.set(drawn.get() + 1);
                    image::RgbaImage::new(w, h)
                })
                .is_some());
        }
        assert_eq!(drawn.get(), 1, "drawn once, served four times");

        // And forgetting it means drawing it again, which is what frees the
        // terminal's own copy.
        g.forget(id);
        assert!(g
            .raster(id, Rect::new(0, 0, 2, 1), |w, h| {
                drawn.set(drawn.get() + 1);
                image::RgbaImage::new(w, h)
            })
            .is_some());
        assert_eq!(drawn.get(), 2);
    }

    #[test]
    fn a_different_picture_is_a_different_entry_even_at_the_same_size() {
        // The bug this exists for: the encoded image used to be kept against
        // the *track*, so cycling to another of an album's covers -- same
        // track, same panel, same size -- served the one before it while the
        // caption underneath named the new one.
        #[allow(deprecated)]
        let picker = Picker::from_fontsize(FontSize {
            width: 8,
            height: 16,
        });
        let mut g = Graphics::disabled();
        g.mode = Mode::Kitty;
        g.picker = Some(picker);

        let one = Arc::new(image::RgbaImage::from_pixel(
            4,
            4,
            image::Rgba([1, 2, 3, 255]),
        ));
        let two = Arc::new(image::RgbaImage::from_pixel(
            4,
            4,
            image::Rgba([9, 9, 9, 255]),
        ));
        let area = Rect::new(0, 0, 12, 6);
        assert!(g.protocol(ImageId::of_arc(&one), &one, area).is_some());
        assert!(g.protocol(ImageId::of_arc(&two), &two, area).is_some());
        assert_eq!(g.cache.len(), 2);
        // The same cover in a resized panel is a third.
        assert!(g
            .protocol(ImageId::of_arc(&one), &one, Rect::new(0, 0, 20, 10))
            .is_some());
        assert_eq!(g.cache.len(), 3);

        g.forget_all();
        assert!(g.cache.is_empty());
    }

    #[test]
    fn a_two_by_four_picture_fills_two_cells_by_two() {
        // Half blocks put the upper pixel in the foreground and the lower one
        // in the background, so four rows of pixels are two rows of cells.
        let mut img = image::RgbaImage::new(2, 4);
        for y in 0..4u32 {
            for x in 0..2u32 {
                img.put_pixel(x, y, image::Rgba([(x * 10) as u8, (y * 20) as u8, 5, 255]));
            }
        }
        let area = Rect::new(0, 0, 2, 2);
        let mut buf = Buffer::empty(area);
        halfblocks(&img, area, &mut buf);

        for (cx, cy) in [(0u16, 0u16), (1, 0), (0, 1), (1, 1)] {
            let cell = &buf[(cx, cy)];
            assert_eq!(cell.symbol(), HALF.to_string(), "cell {cx},{cy}");
            let top = *img.get_pixel(cx as u32, cy as u32 * 2);
            let bottom = *img.get_pixel(cx as u32, cy as u32 * 2 + 1);
            assert_eq!(cell.fg, Color::Rgb(top[0], top[1], top[2]));
            assert_eq!(cell.bg, Color::Rgb(bottom[0], bottom[1], bottom[2]));
        }
    }

    #[test]
    fn a_picture_larger_than_its_rectangle_is_sampled_down_to_it() {
        let img = image::RgbaImage::from_pixel(64, 64, image::Rgba([3, 4, 5, 255]));
        let area = Rect::new(1, 1, 3, 2);
        let mut buf = Buffer::empty(Rect::new(0, 0, 6, 4));
        halfblocks(&img, area, &mut buf);
        assert_eq!(buf[(1, 1)].fg, Color::Rgb(3, 4, 5));
        assert_eq!(buf[(3, 2)].bg, Color::Rgb(3, 4, 5));
        // Nothing outside the rectangle was touched.
        assert_eq!(buf[(0, 0)].symbol(), " ");
        assert_eq!(buf[(4, 1)].symbol(), " ");
    }

    #[test]
    fn an_empty_rectangle_or_an_empty_picture_draws_nothing() {
        let img = image::RgbaImage::from_pixel(2, 2, image::Rgba([1, 1, 1, 255]));
        let mut buf = Buffer::empty(Rect::new(0, 0, 4, 2));
        halfblocks(&img, Rect::new(0, 0, 0, 2), &mut buf);
        halfblocks(
            &image::RgbaImage::new(0, 0),
            Rect::new(0, 0, 4, 2),
            &mut buf,
        );
        assert!(buf.content().iter().all(|c| c.symbol() == " "));
    }

    #[test]
    fn a_placeholder_fills_its_rectangle_and_stops() {
        let area = Rect::new(1, 1, 2, 1);
        let mut buf = Buffer::empty(Rect::new(0, 0, 4, 3));
        placeholder(area, &mut buf, Style::default().fg(Color::Rgb(9, 9, 9)));
        assert_eq!(buf[(1, 1)].symbol(), "\u{2591}");
        assert_eq!(buf[(2, 1)].fg, Color::Rgb(9, 9, 9));
        assert_eq!(buf[(0, 1)].symbol(), " ");
        assert_eq!(buf[(3, 1)].symbol(), " ");
    }
}
