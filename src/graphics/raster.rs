//! Shapes, drawn into pixels.
//!
//! A terminal program cannot ship a font. A text face -- a Nerd Font icon, a
//! geometric shape, a letter -- is drawn by whatever typeface the terminal was
//! configured with, at that face's size and to that font's metrics, and two
//! machines set to different fonts draw two different rows of buttons from the
//! same bytes. So the shapes an application cares about are rasterised here, at
//! the terminal's real cell size, and put on the screen over the same graphics
//! protocol its pictures use: exact, and identical on every terminal that
//! speaks kitty, sixel or iTerm2.
//!
//! Nothing here knows what it is drawing. It takes polygons on whatever grid
//! the caller keeps them on and a mapping from pixels onto that grid; which
//! polygons make a play triangle is the application's business.

use image::{Rgba, RgbaImage};

/// Samples per pixel edge.
///
/// Sixteen samples a pixel is enough that a slanted edge shows no steps at cell
/// sizes up to a few dozen pixels, which is every cell size there is.
pub const SUPERSAMPLES: u32 = 4;

/// Even-odd point-in-polygon.
///
/// The polygon is a closed ring of vertices; the last joins the first.
pub fn inside(poly: &[(f32, f32)], x: f32, y: f32) -> bool {
    let mut hit = false;
    let n = poly.len();
    if n == 0 {
        return false;
    }
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            hit = !hit;
        }
        j = i;
    }
    hit
}

/// Is `(x, y)` inside the `side`-by-`side` square at the origin whose corners
/// are rounded to `radius`?
pub fn in_rounded_square(x: f32, y: f32, side: f32, radius: f32) -> bool {
    if x < 0.0 || y < 0.0 || x >= side || y >= side {
        return false;
    }
    // Distance from the nearest corner's centre of curvature, when in a
    // corner's square at all.
    let dx = (radius - x).max(x - (side - radius)).max(0.0);
    let dy = (radius - y).max(y - (side - radius)).max(0.0);
    dx * dx + dy * dy <= radius * radius
}

/// Blend `colour` over `img` wherever `shape` says, anti-aliased.
///
/// `shape` is asked about points in the image's own pixel coordinates, several
/// times per pixel, and the share that answer yes is how much of `colour` that
/// pixel gets. That share is the whole of the anti-aliasing: there is no
/// outline pass and no coverage arithmetic beyond counting samples, which is
/// what keeps a shape and its edge exactly consistent.
///
/// Opaque, deliberately: what a graphics protocol shows behind a transparent
/// pixel is the cell's background, which is whatever style the placeholder cell
/// happened to keep, and painting the colour in ourselves is the only way to be
/// sure of it.
pub fn fill(img: &mut RgbaImage, colour: Rgba<u8>, shape: impl Fn(f32, f32) -> bool) {
    let total = (SUPERSAMPLES * SUPERSAMPLES) as f32;
    for (px, py, p) in img.enumerate_pixels_mut() {
        let mut hits = 0u32;
        for sy in 0..SUPERSAMPLES {
            for sx in 0..SUPERSAMPLES {
                let x = px as f32 + (sx as f32 + 0.5) / SUPERSAMPLES as f32;
                let y = py as f32 + (sy as f32 + 0.5) / SUPERSAMPLES as f32;
                if shape(x, y) {
                    hits += 1;
                }
            }
        }
        if hits > 0 {
            *p = mix(*p, colour, hits as f32 / total);
        }
    }
}

/// Blend `colour` over `img` wherever `polys` cover it.
///
/// `place` maps a point in the image's pixel coordinates onto the grid the
/// polygons are kept on, and returns `None` for a pixel that is outside the
/// region being drawn at all -- the plate a button's icon sits on, say. A point
/// inside any polygon is inside the shape, so two polygons that overlap do not
/// cancel each other the way the even-odd rule would within one.
pub fn fill_polygons(
    img: &mut RgbaImage,
    colour: Rgba<u8>,
    polys: &[&[(f32, f32)]],
    place: impl Fn(f32, f32) -> Option<(f32, f32)>,
) {
    fill(img, colour, |x, y| {
        let Some((ux, uy)) = place(x, y) else {
            return false;
        };
        polys.iter().any(|poly| inside(poly, ux, uy))
    });
}

/// `a` moved `t` of the way toward `b`, per channel, alpha left alone.
fn mix(a: Rgba<u8>, b: Rgba<u8>, t: f32) -> Rgba<u8> {
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Rgba([
        f(a.0[0], b.0[0]),
        f(a.0[1], b.0[1]),
        f(a.0[2], b.0[2]),
        a.0[3],
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    const INK: Rgba<u8> = Rgba([255, 255, 255, 255]);
    const BG: Rgba<u8> = Rgba([0, 0, 0, 255]);

    #[test]
    fn the_polygon_test_agrees_with_geometry() {
        let square: &[(f32, f32)] = &[(0.0, 0.0), (2.0, 0.0), (2.0, 2.0), (0.0, 2.0)];
        assert!(inside(square, 1.0, 1.0));
        assert!(!inside(square, 3.0, 1.0));
        let tri: &[(f32, f32)] = &[(0.0, 0.0), (0.0, 2.0), (2.0, 1.0)];
        assert!(inside(tri, 0.5, 1.0));
        assert!(!inside(tri, 1.5, 0.2));
        assert!(!inside(&[], 0.0, 0.0), "nothing has no inside");
    }

    #[test]
    fn rounded_corners_are_cut_and_edges_are_kept() {
        assert!(!in_rounded_square(0.1, 0.1, 10.0, 2.0), "the corner");
        assert!(
            in_rounded_square(5.0, 0.1, 10.0, 2.0),
            "the top edge's middle"
        );
        assert!(in_rounded_square(5.0, 5.0, 10.0, 2.0), "the centre");
        assert!(
            !in_rounded_square(10.0, 5.0, 10.0, 2.0),
            "just past the right"
        );
    }

    #[test]
    fn a_fill_covers_what_the_shape_covers_and_nothing_else() {
        let mut img = RgbaImage::from_pixel(8, 8, BG);
        fill(&mut img, INK, |x, y| x < 4.0 && y < 4.0);
        assert_eq!(img.get_pixel(1, 1).0, [255, 255, 255, 255], "inside");
        assert_eq!(img.get_pixel(5, 1).0, [0, 0, 0, 255], "outside");
        assert_eq!(img.get_pixel(1, 5).0, [0, 0, 0, 255], "below");
    }

    #[test]
    fn slanted_edges_are_blended_rather_than_stepped() {
        // A triangle's hypotenuse, which no grid of whole pixels can follow.
        let tri: &[(f32, f32)] = &[(0.0, 0.0), (0.0, 24.0), (24.0, 12.0)];
        let mut img = RgbaImage::from_pixel(24, 24, BG);
        fill_polygons(&mut img, INK, &[tri], |x, y| Some((x, y)));
        let partial = img.pixels().any(|p| p.0[0] > 0 && p.0[0] < 255);
        assert!(partial, "no anti-aliased pixel anywhere");
    }

    #[test]
    fn a_pixel_the_placement_masks_out_is_left_alone() {
        // What a button's plate does to the icon drawn on it: outside the
        // plate there is nothing to draw on, whatever the polygons say.
        let all: &[(f32, f32)] = &[(0.0, 0.0), (8.0, 0.0), (8.0, 8.0), (0.0, 8.0)];
        let mut img = RgbaImage::from_pixel(8, 8, BG);
        fill_polygons(&mut img, INK, &[all], |x, y| (x < 4.0).then_some((x, y)));
        assert_eq!(img.get_pixel(1, 1).0, [255, 255, 255, 255]);
        assert_eq!(img.get_pixel(6, 1).0, [0, 0, 0, 255]);
    }

    #[test]
    fn two_polygons_of_one_shape_do_not_cancel_where_they_overlap() {
        // Even-odd is the rule *within* a polygon, not between them: a pause
        // icon is two bars, and two bars that overlapped would not be a hole.
        let a: &[(f32, f32)] = &[(0.0, 0.0), (6.0, 0.0), (6.0, 6.0), (0.0, 6.0)];
        let b: &[(f32, f32)] = &[(2.0, 0.0), (8.0, 0.0), (8.0, 6.0), (2.0, 6.0)];
        let mut img = RgbaImage::from_pixel(8, 8, BG);
        fill_polygons(&mut img, INK, &[a, b], |x, y| Some((x, y)));
        assert_eq!(img.get_pixel(4, 3).0, [255, 255, 255, 255], "the overlap");
    }
}
