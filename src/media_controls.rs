//! Shared render-only media controls. No player, device, process or IO is started.
//! Native surfaces and bounded cell grids use the same hit geometry. Applications
//! interpret action names and own playback, track selection and persistence.
use crate::native_surface::{HitRegion, Metrics, PixelRect as R, Primitive, Surface};
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};

pub use crate::native_surface::style::Palette;
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Cell {
    pub symbol: String,
    pub fg: [u8; 3],
    pub bg: [u8; 3],
    pub modifiers: u16,
}
fn hex([r, g, b]: [u8; 3]) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Request {
    #[serde(default)]
    pub cells: Option<[u16; 2]>,
    pub width: u16,
    pub height: u16,
    pub theme: Palette,
    pub playing: bool,
    pub paused: bool,
    pub volume: f32,
    #[serde(default)]
    pub pointer: Option<[u16; 2]>,
    #[serde(default)]
    pub movie: bool,
    #[serde(default)]
    pub picker: bool,
    #[serde(default)]
    pub entries: Vec<String>,
    #[serde(default)]
    pub selected: usize,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub font: u16,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct CellTransport {
    pub columns: u16,
    pub rows: u16,
    pub cells: Vec<Cell>,
}
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Response {
    pub surface: Option<crate::native_surface::Surface>,
    pub cells: Option<CellTransport>,
    pub action: Option<String>,
    pub value: Option<f32>,
}

pub fn render(request: &Request) -> Result<Response> {
    ensure!(
        (1..=8192).contains(&request.width)
            && (1..=8192).contains(&request.height)
            && u64::from(request.width) * u64::from(request.height) <= 32_000_000,
        "Invalid transport dimensions"
    );
    ensure!(
        request.volume.is_finite() && (0.0..=1.0).contains(&request.volume),
        "Invalid volume"
    );
    ensure!(
        request.entries.len() <= 2048
            && request.title.len() <= 8192
            && request.entries.iter().all(|entry| entry.len() <= 8192)
            && request.entries.iter().map(String::len).sum::<usize>() <= 65536,
        "Track picker exceeds limits"
    );
    let surface = if request.picker {
        track_surface(
            request.width,
            request.height,
            &request.theme,
            &request.title,
            &request.entries,
            request.selected,
            request.font,
        )
    } else if request.movie {
        movie_transport_surface(
            request.width,
            request.height,
            &request.theme,
            request.playing,
            request.paused,
            request.volume,
            request.font,
        )
    } else {
        transport_surface(
            request.width,
            request.height,
            &request.theme,
            request.playing,
            request.paused,
            request.volume,
        )
    };
    let surface = surface.with_physical_text();
    surface.validate()?;
    let mut response = Response {
        cells: None,
        surface: None,
        action: None,
        value: None,
    };
    if let Some([x, y]) = request.pointer {
        if let Some(hit) = surface.hit(x, y).filter(|h| {
            h.action.starts_with("track:")
                || matches!(
                    h.action.as_str(),
                    "audio_tracks" | "subtitle_tracks" | "fullscreen" | "picker_close"
                )
        }) {
            response.action = Some(hit.action.clone());
            return Ok(response);
        }
        if let Some(hit) = surface.hit(x, y) {
            response.action = Some(hit.action.clone());
            if hit.action == "volume" {
                response.value = Some(
                    (f32::from(x.saturating_sub(hit.rect.x)) / f32::from(hit.rect.width.max(1)))
                        .clamp(0., 1.),
                );
            }
        }
    } else {
        if let Some([columns, rows]) = request.cells {
            response.cells = Some(cell_transport(&surface, columns, rows, &request.theme)?);
        }
        response.surface = Some(surface);
    }
    Ok(response)
}

/// Cell artwork uses the same transport surface and hit regions as native buttons.
fn cell_transport(
    surface: &crate::native_surface::Surface,
    columns: u16,
    rows: u16,
    palette: &Palette,
) -> Result<CellTransport> {
    use crate::{
        native_surface::Primitive,
        ratatui::{
            buffer::Buffer,
            layout::Rect,
            style::{Color, Modifier, Style},
        },
    };
    ensure!(
        columns > 0 && rows > 0 && usize::from(columns) * usize::from(rows) <= 16_384,
        "Transport cell grid exceeds limits"
    );
    let area = Rect::new(0, 0, columns, rows);
    let mut buffer = Buffer::empty(area);
    buffer.set_style(
        area,
        Style::default()
            .fg(Color::Rgb(palette.fg[0], palette.fg[1], palette.fg[2]))
            .bg(Color::Rgb(palette.bg[0], palette.bg[1], palette.bg[2])),
    );
    let color = |s: &str| {
        let n = u32::from_str_radix(s.trim_start_matches('#'), 16).unwrap_or(0);
        Color::Rgb((n >> 16) as u8, (n >> 8) as u8, n as u8)
    };
    let project = |r: R| {
        let x = (u32::from(r.x) * u32::from(columns) / u32::from(surface.width)) as u16;
        let y = (u32::from(r.y) * u32::from(rows) / u32::from(surface.height)) as u16;
        let right = ((u32::from(r.x) + u32::from(r.width)) * u32::from(columns)
            / u32::from(surface.width)) as u16;
        let bottom = ((u32::from(r.y) + u32::from(r.height)) * u32::from(rows)
            / u32::from(surface.height)) as u16;
        Rect::new(x, y, right - x, bottom - y).intersection(area)
    };
    for node in &surface.nodes {
        if let Primitive::Fill {
            rect,
            color: background,
            ..
        } = node
        {
            buffer.set_style(project(*rect), Style::default().bg(color(background)));
            continue;
        }
        let (rect, text, foreground, bold) = match node {
            Primitive::Text {
                rect,
                text,
                color,
                bold,
                ..
            } => (*rect, text.as_str(), color.as_str(), *bold),
            Primitive::Icon { rect, name, color } => (
                *rect,
                match name.as_str() {
                    "media-rewind" | "previous" => "<<",
                    "media-forward" | "next" => ">>",
                    "media-play" => ">",
                    "media-pause" => "||",
                    "media-stop" => "[]",
                    "speaker" | "media-speaker" => "vol",
                    "fullscreen" | "media-fullscreen" => "full",
                    "audio_tracks" | "media-headphones" => "audio",
                    "subtitle_tracks" | "media-subtitles" => "subs",
                    "close" => "x",
                    _ => "",
                },
                color.as_str(),
                false,
            ),
            _ => continue,
        };
        let x = (u32::from(rect.x) * u32::from(columns) / u32::from(surface.width)) as u16;
        let y = ((u32::from(rect.y) + u32::from(rect.height) / 2) * u32::from(rows)
            / u32::from(surface.height)) as u16;
        if x < columns && y < rows {
            let safe: String = text
                .chars()
                .filter(|c| {
                    !c.is_control() && !matches!(c,'\u{202a}'..='\u{202e}'|'\u{2066}'..='\u{2069}')
                })
                .collect();
            let right = project(rect).right().max(x.saturating_add(1)).min(columns);
            buffer.set_stringn(
                x,
                y,
                safe,
                usize::from(right - x),
                Style::default()
                    .fg(color(foreground))
                    .add_modifier(if bold {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            );
        }
    }
    Ok(CellTransport {
        columns,
        rows,
        cells: buffer
            .content
            .iter()
            .map(|c| {
                let rgb = |c: Color, default| {
                    if let Color::Rgb(r, g, b) = c {
                        [r, g, b]
                    } else {
                        default
                    }
                };
                Cell {
                    symbol: c.symbol().into(),
                    fg: rgb(c.fg, palette.fg),
                    bg: rgb(c.bg, palette.bg),
                    modifiers: c.modifier.bits(),
                }
            })
            .collect(),
    })
}

fn control_glyph(s: &mut Surface, rect: R, name: &str, color: &str) {
    if rect.width.min(rect.height) < 8 {
        return;
    }
    s.nodes.push(Primitive::Icon {
        rect,
        name: format!("media-{name}"),
        color: color.into(),
    });
}

#[allow(clippy::too_many_arguments)]
pub fn paint_transport(
    s: &mut Surface,
    pad: u16,
    control: u16,
    controls_y: u16,
    inner: u16,
    m: Metrics,
    palette: &Palette,
    playing: bool,
    paused: bool,
    volume_level: f32,
    movie: bool,
) {
    let w = s.width;
    let border = hex(palette.border);
    let selected = hex(palette.selected);
    let accent = hex(palette.accent);
    let fg = hex(palette.fg);
    let bg = hex(palette.bg);
    let button = control.min(44).min(inner / 6).max(1);
    let toggle = if playing && !paused { "pause" } else { "play" };
    let movie_actions = ["previous", toggle, "next", "stop"];
    let audio_actions = ["previous", toggle, "stop", "next"];
    let actions: &[&str] = if movie {
        &movie_actions
    } else {
        &audio_actions
    };
    for (i, name) in actions.iter().enumerate() {
        let x = pad + i as u16 * (button + m.small);
        if x + button > w - pad {
            break;
        }
        let rect = R::new(x, controls_y, button, control);
        let active = i == 1 && (movie || playing || paused);
        s.fill(
            rect,
            if active { &accent } else { &selected },
            (control / 4).min(10),
        );
        // Use the full 24-unit face: AMP's artwork already includes optical padding.
        let size = button.min(control).min(40);
        s.nodes.push(Primitive::Icon {
            rect: R::new(
                x + (button - size) / 2,
                controls_y + (control - size) / 2,
                size,
                size,
            ),
            name: match *name {
                "previous" if movie => "media-rewind".into(),
                "next" if movie => "media-forward".into(),
                "play" | "pause" | "stop" => format!("media-{name}"),
                other => other.into(),
            },
            color: if active { bg.clone() } else { fg.clone() },
        });
        s.hits.push(HitRegion {
            rect,
            action: (*name).into(),
        });
    }
    let volume_width = 120.min(inner / 4);
    let volume = R::new(w - pad - volume_width, controls_y, volume_width, control);
    if volume.x > pad + actions.len() as u16 * (button + m.small) + 32 {
        let size = 22.min(control);
        control_glyph(
            s,
            R::new(volume.x - 30, controls_y + (control - size) / 2, size, size),
            "speaker",
            &fg,
        );
        let track_height = 4.min(control);
        let track_y = controls_y + (control - track_height) / 2;
        s.fill(
            R::new(volume.x, track_y, volume_width, track_height),
            &border,
            2,
        );
        s.fill(
            R::new(
                volume.x,
                track_y,
                (f32::from(volume_width) * volume_level.clamp(0., 1.)) as u16,
                track_height,
            ),
            &accent,
            2,
        );
        s.hits.push(HitRegion {
            rect: volume,
            action: "volume".into(),
        });
    }
}

pub fn transport_surface(
    width: u16,
    height: u16,
    palette: &Palette,
    playing: bool,
    paused: bool,
    volume: f32,
) -> Surface {
    let mut s = Surface::new(width, height, hex(palette.bg));
    let m = Metrics::from_cell(12, 24);
    let pad = 4.min(height / 4).min(width / 12);
    let control = height.saturating_sub(pad * 2).max(1);
    let inner = width.saturating_sub(pad * 2);
    paint_transport(
        &mut s, pad, control, pad, inner, m, palette, playing, paused, volume, false,
    );
    s
}

/// Movie controls add track pickers to the shared transport artwork.
#[allow(clippy::too_many_arguments)]
pub fn movie_transport_surface(
    width: u16,
    height: u16,
    palette: &Palette,
    playing: bool,
    paused: bool,
    volume: f32,
    font: u16,
) -> Surface {
    let reserve = (width / 2).min(408);
    let separation = 24.min(width / 24);
    let transport_width = width.saturating_sub(reserve + separation).max(1);
    let mut surface = Surface::new(transport_width, height, hex(palette.bg));
    let m = Metrics::from_cell(12, 24);
    let pad = 4.min(height / 4).min(transport_width / 12);
    let control = height.saturating_sub(pad * 2).max(1);
    paint_transport(
        &mut surface,
        pad,
        control,
        pad,
        transport_width.saturating_sub(pad * 2),
        m,
        palette,
        playing,
        paused,
        volume,
        true,
    );
    surface.width = width;
    let button = reserve / 3;
    let pad = 4.min(height / 4);
    for (i, (label, action, glyph)) in [
        ("Audio", "audio_tracks", "headphones"),
        ("Subtitles", "subtitle_tracks", "subtitles"),
        ("Full screen", "fullscreen", "fullscreen"),
    ]
    .iter()
    .enumerate()
    {
        let rect = R::new(
            width - reserve + i as u16 * button,
            pad,
            button.saturating_sub(4),
            height.saturating_sub(pad * 2),
        );
        surface.fill(rect, &hex(palette.selected), (rect.height / 4).min(10));
        let icon = 22
            .min(rect.height.saturating_sub(8))
            .min(rect.width.saturating_sub(12));
        if rect.width >= 16 {
            control_glyph(
                &mut surface,
                R::new(rect.x + 6, rect.y + (rect.height - icon) / 2, icon, icon),
                glyph,
                &hex(palette.fg),
            );
        }
        if rect.width >= 88 {
            let label_rect = R::new(
                rect.x + icon + 12,
                rect.y,
                rect.width.saturating_sub(icon + 18),
                rect.height,
            );
            surface.text(
                label_rect,
                *label,
                &hex(palette.fg),
                font.clamp(10, 16),
                false,
            );
            // All three shortcuts start the label (A/S/F), so the highlighted
            // glyph uses the label's own shaping origin, including in cells.
            surface.text(
                label_rect,
                &label[..1],
                &hex(palette.accent),
                font.clamp(10, 16),
                true,
            );
        }
        surface.hits.push(HitRegion {
            rect,
            action: (*action).into(),
        });
    }
    surface
}

pub fn track_surface(
    width: u16,
    height: u16,
    palette: &Palette,
    title: &str,
    entries: &[String],
    selected: usize,
    font: u16,
) -> Surface {
    let mut surface = Surface::new(width, height, hex(palette.bg));
    let font = font.clamp(10, 128);
    let row = (font + 12).min(height);
    let pad = 12.min(width / 4);
    surface.text(
        R::new(pad, 0, width.saturating_sub(pad * 2 + row), row),
        title,
        &hex(palette.accent),
        font,
        true,
    );
    let close_width = row.max(30).min(width);
    let rect = R::new(width.saturating_sub(close_width), 0, close_width, row);
    let size = rect.width.min(rect.height).min(28).saturating_sub(2);
    surface.nodes.push(Primitive::Icon {
        rect: R::new(
            rect.x + (rect.width - size) / 2,
            rect.y + (rect.height - size) / 2,
            size,
            size,
        ),
        name: "close".into(),
        color: hex(palette.fg),
    });
    surface.hits.push(HitRegion {
        rect,
        action: "picker_close".into(),
    });
    let visible = (height.saturating_sub(row + 8) / row).min(255);
    let offset = selected.saturating_sub(visible.saturating_sub(1) as usize);
    for (index, label) in entries
        .iter()
        .enumerate()
        .skip(offset)
        .take(visible as usize)
    {
        let rect = R::new(
            pad,
            row + 4 + (index - offset) as u16 * row,
            width.saturating_sub(pad * 2),
            row,
        );
        if index == selected {
            surface.fill(rect, &hex(palette.selected), 4);
        }
        surface.text(
            R::new(
                rect.x + 8.min(rect.width),
                rect.y,
                rect.width.saturating_sub(16),
                rect.height,
            ),
            label,
            &hex(palette.fg),
            font,
            index == selected,
        );
        surface.hits.push(HitRegion {
            rect,
            action: format!("track:{index}"),
        });
    }
    surface
}

#[cfg(test)]
mod tests {
    use super::*;
    fn render_owned(r: Request) -> Result<Response> {
        render(&r)
    }
    fn request() -> Request {
        Request {
            cells: None,
            width: 600,
            height: 48,
            theme: Palette {
                bg: [24; 3],
                inset_bg: None,
                fg: [230; 3],
                muted: [100; 3],
                accent: [140; 3],
                selected: [40; 3],
                border: [60; 3],
                error: [200; 3],
            },
            playing: true,
            paused: false,
            volume: 0.8,
            pointer: None,
            movie: false,
            picker: false,
            entries: vec![],
            selected: 0,
            title: String::new(),
            font: 20,
        }
    }
    #[test]
    fn cell_controls_reuse_the_native_buttons_and_pointer_actions() {
        let mut r = request();
        r.movie = true;
        r.cells = Some([80, 2]);
        let response = render_owned(r).unwrap();
        let cells = response.cells.unwrap();
        assert_eq!(cells.cells.len(), 160);
        assert!(cells.cells.iter().any(|c| c.symbol == "|"));
        let surface = response.surface.unwrap();
        for hit in &surface.hits {
            if hit.action == "volume" {
                continue;
            }
            let mut r = request();
            r.movie = true;
            r.cells = Some([80, 2]);
            r.pointer = Some([
                hit.rect.x + hit.rect.width / 2,
                hit.rect.y + hit.rect.height / 2,
            ]);
            assert_eq!(
                render_owned(r).unwrap().action.as_deref(),
                Some(hit.action.as_str())
            );
        }
    }
    #[test]
    fn controls_use_player_artwork_and_hit_testing() {
        let surface = render_owned(request()).unwrap().surface.unwrap();
        for hit in surface.hits.iter().filter(|h| h.action != "volume") {
            let mut request = request();
            request.pointer = Some([
                hit.rect.x + hit.rect.width / 2,
                hit.rect.y + hit.rect.height / 2,
            ]);
            assert_eq!(
                render_owned(request).unwrap().action.as_deref(),
                Some(hit.action.as_str())
            );
        }
        assert_eq!(surface.hits.len(), 5);
    }
    #[test]
    fn audio_transport_has_one_play_pause_button_in_a_stable_slot() {
        let mut previous_rect = None;
        for (playing, paused, action) in [
            (false, false, "play"),
            (true, false, "pause"),
            (true, true, "play"),
        ] {
            let mut r = request();
            r.playing = playing;
            r.paused = paused;
            let surface = render_owned(r).unwrap().surface.unwrap();
            let toggles = surface
                .hits
                .iter()
                .filter(|hit| matches!(hit.action.as_str(), "play" | "pause"))
                .collect::<Vec<_>>();
            assert_eq!(toggles.len(), 1);
            assert_eq!(toggles[0].action, action);
            if let Some(rect) = previous_rect {
                assert_eq!(toggles[0].rect, rect);
            }
            previous_rect = Some(toggles[0].rect);
        }
    }
    #[test]
    fn movie_buttons_and_track_rows_accept_mouse_selection() {
        for picker in [false, true] {
            let mut r = request();
            r.movie = !picker;
            r.picker = picker;
            r.height = if picker { 300 } else { 48 };
            r.entries = vec!["Auto".into(), "Off".into(), "English".into()];
            let surface = render_owned(r).unwrap().surface.unwrap();
            for hit in &surface.hits {
                let mut r = request();
                r.movie = !picker;
                r.picker = picker;
                r.height = if picker { 300 } else { 48 };
                r.entries = vec!["Auto".into(), "Off".into(), "English".into()];
                r.pointer = Some([
                    hit.rect.x + hit.rect.width / 2,
                    hit.rect.y + hit.rect.height / 2,
                ]);
                if hit.action != "volume" {
                    assert_eq!(
                        render_owned(r).unwrap().action.as_deref(),
                        Some(hit.action.as_str())
                    );
                }
            }
        }
    }
    #[test]
    fn movie_transport_has_one_play_pause_toggle_and_seek_controls() {
        for paused in [false, true] {
            let mut r = request();
            r.movie = true;
            r.width = 1200;
            r.paused = paused;
            let surface = render_owned(r).unwrap().surface.unwrap();
            let actions: Vec<_> = surface.hits.iter().map(|h| h.action.as_str()).collect();
            assert!(actions.contains(&if paused { "play" } else { "pause" }));
            assert!(!actions.contains(&if paused { "pause" } else { "play" }));
            assert!(
                actions.contains(&"previous")
                    && actions.contains(&"next")
                    && actions.contains(&"stop")
            );
        }
    }
    #[test]
    fn volume_slider_has_space_before_audio_picker() {
        let mut r = request();
        r.movie = true;
        r.width = 1200;
        let surface = render_owned(r).unwrap().surface.unwrap();
        let volume = surface
            .hits
            .iter()
            .find(|h| h.action == "volume")
            .unwrap()
            .rect;
        let audio = surface
            .hits
            .iter()
            .find(|h| h.action == "audio_tracks")
            .unwrap()
            .rect;
        assert!(audio.x >= volume.x + volume.width + 24);
    }
    proptest::proptest! {
        #[test]
        fn bounded_geometry_never_panics(width in 0u16..9000, height in 0u16..160, volume in 0u8..=100) {
            let mut r = request(); r.width = width; r.height = height; r.volume = f32::from(volume) / 100.0;
            let result = render_owned(r);
            proptest::prop_assert_eq!(result.is_ok(), (1..=8192).contains(&width) && (1..=8192).contains(&height) && u64::from(width)*u64::from(height)<=32_000_000);
        }
    }
    proptest::proptest! {
        #[test]
        fn movie_controls_stay_inside_compact_and_fullscreen_surfaces(width in 1u16..8193, height in 1u16..160) {
            let mut r = request(); r.width = width; r.height = height; r.movie = true;
            proptest::prop_assert!(render_owned(r).is_ok());
        }
    }
    #[test]
    fn picker_supports_real_terminal_grids_and_rejects_oversized_content() {
        let mut r = request();
        r.picker = true;
        r.height = 400;
        r.entries = vec!["English".into(), "Off".into()];
        r.cells = Some([72, 20]);
        let response = render(&r).unwrap();
        assert_eq!(response.cells.unwrap().cells.len(), 1440);
        r.cells = Some([4096, 4096]);
        assert!(render(&r).is_err());
        r.cells = None;
        r.entries = vec!["x".repeat(8193)];
        assert!(render(&r).is_err());
    }
    proptest::proptest! {
        #[test]
        fn picker_geometry_is_bounded_even_when_no_row_fits(width in 1u16..8193,height in 1u16..300,font in 0u16..=65535,selected in 0usize..4000) {
            let mut r=request();r.picker=true;r.width=width;r.height=height;r.font=font;r.selected=selected;
            r.entries=vec!["English".into();20];
            proptest::prop_assert!(render(&r).is_ok());
        }
    }
    #[test]
    fn rejects_invalid_dimensions_and_volume() {
        let mut r = request();
        r.width = 0;
        assert!(render_owned(r).is_err());
        let mut r = request();
        r.volume = f32::NAN;
        assert!(render_owned(r).is_err());
    }
}
