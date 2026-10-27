use super::*;

#[test]
fn font_measurements_come_from_the_selected_face() {
    let font = |name: &str| super::super::font::Font {
        name: Some(name.into()),
        pixels: Some(16.),
        cell: None,
    };
    let mono = terminal_typography(&font("Liberation Mono")).unwrap();
    let sans = terminal_typography(&font("Liberation Sans")).unwrap();
    assert_ne!(
        mono.cell[0], sans.cell[0],
        "M advance must come from the face, not a fixed size ratio"
    );
    for table in [mono.sizes, sans.sizes] {
        assert!(table
            .windows(2)
            .all(|s| s[1][0] >= s[0][0] && s[1][1] >= s[0][1]));
        assert!(table.iter().all(|s| s[0] > 0 && s[1] > 0));
    }
}

#[test]
fn terminal_surface_shapes_a_whole_run_and_keeps_fractional_size() {
    use crate::native_surface::{PixelRect, Surface};
    let mut painter = Painter::with_font(super::super::font::Font {
        name: Some("Liberation Mono".into()),
        pixels: Some(14.667),
        cell: Some((9, 20)),
    });
    let mut surface = Surface::new(400, 40, "#000000".into()).with_physical_text();
    surface.cell_size = Some([9, 20]);
    surface.terminal_font_millipixels = Some(14667);
    let text = "i W -> != café 世界 é";
    surface.text(PixelRect::new(0, 0, 396, 20), text, "#ffffff", 15, false);
    let mut canvas = Pixmap::new(400, 40).unwrap();
    painter
        .surface(&mut canvas, &surface, [0., 0., 400., 40.])
        .unwrap();
    let key = painter
        .text
        .keys()
        .find(|k| k.text == text)
        .expect("complete shaped run");
    assert!((f32::from_bits(key.size) - 14.667).abs() < 0.002);
    assert_eq!(f32::from_bits(key.line_height), 20.);
    assert!(canvas.data().as_chunks::<4>().0.iter().any(|p| p[0] > 0));
    // Editor grids keep their existing per-grapheme behavior.
    assert!(!painter.text.keys().any(|k| k.text == "i"));
}

#[test]
fn terminal_joining_glyphs_still_touch_cell_edges() {
    use crate::native_surface::{PixelRect, Surface};
    let mut surface = Surface::new(32, 40, "#000000".into()).with_physical_text();
    surface.cell_size = Some([8, 20]);
    surface.terminal_font_millipixels = Some(14667);
    surface.text(PixelRect::new(0, 0, 32, 20), "────", "#ffffff", 15, false);
    let mut painter = Painter::new();
    let mut canvas = Pixmap::new(32, 40).unwrap();
    painter
        .surface(&mut canvas, &surface, [0., 0., 32., 40.])
        .unwrap();
    for x in 0..32 {
        assert_eq!(canvas.pixel(x, 10).unwrap().red(), 255);
    }
}

#[test]
fn queried_postscript_font_has_usable_metrics() {
    let font = super::super::font::Font {
        name: Some("JetBrainsMonoNF-Regular".into()),
        pixels: Some(14.667),
        cell: Some((9, 20)),
    };
    let metrics = terminal_typography(&font).expect("queried font must resolve to usable metrics");
    assert!(metrics.sizes.iter().all(|s| s[0] > 0 && s[1] > 0));
}
