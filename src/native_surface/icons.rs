//! Small line icons shared by fleet controls, emitted as portable native paths.
use super::{PixelRect, Primitive, Surface};
pub fn draw(surface: &mut Surface, rect: PixelRect, name: &str, color: &str) {
    let paths: Vec<Vec<[u16; 2]>> = match name {
        "workspace" => vec![
            vec![[1, 1], [6, 1], [6, 6], [1, 6], [1, 1]],
            vec![[10, 1], [15, 1], [15, 6], [10, 6], [10, 1]],
            vec![[1, 10], [6, 10], [6, 15], [1, 15], [1, 10]],
            vec![[10, 10], [15, 10], [15, 15], [10, 15], [10, 10]],
        ],
        "folder" => vec![vec![
            [1, 3],
            [6, 3],
            [8, 5],
            [15, 5],
            [15, 14],
            [1, 14],
            [1, 3],
        ]],
        "archive" => vec![
            vec![[1, 3], [15, 3], [15, 6], [1, 6], [1, 3]],
            vec![[2, 6], [2, 14], [14, 14], [14, 6]],
            vec![[6, 9], [10, 9]],
        ],
        "history" => vec![
            vec![
                [2, 7],
                [3, 3],
                [7, 1],
                [12, 2],
                [15, 6],
                [14, 11],
                [10, 15],
                [5, 14],
                [2, 11],
            ],
            vec![[1, 3], [2, 7], [6, 6]],
            vec![[8, 4], [8, 8], [11, 10]],
        ],
        "activity" => vec![vec![
            [1, 9],
            [4, 9],
            [6, 3],
            [9, 14],
            [12, 6],
            [14, 9],
            [16, 9],
        ]],
        "flame" => vec![
            vec![
                [8, 0],
                [11, 4],
                [10, 7],
                [12, 5],
                [14, 8],
                [15, 11],
                [13, 14],
                [9, 16],
                [5, 15],
                [2, 12],
                [2, 9],
                [4, 6],
                [4, 10],
                [6, 9],
                [7, 6],
                [8, 3],
                [8, 0],
            ],
            vec![
                [8, 9],
                [10, 11],
                [10, 13],
                [8, 15],
                [6, 13],
                [6, 11],
                [8, 9],
            ],
        ],
        "disc" => vec![
            (0..=32)
                .map(|n| {
                    let a = n as f64 * std::f64::consts::TAU / 32.;
                    [
                        (8. + 7. * a.cos()).round() as u16,
                        (8. + 7. * a.sin()).round() as u16,
                    ]
                })
                .collect(),
            vec![[6, 6], [10, 6], [10, 10], [6, 10], [6, 6]],
        ],
        "home" => vec![
            vec![[1, 7], [8, 1], [15, 7]],
            vec![
                [3, 6],
                [3, 15],
                [6, 15],
                [6, 10],
                [10, 10],
                [10, 15],
                [13, 15],
                [13, 6],
            ],
        ],
        "download" => vec![
            vec![[8, 1], [8, 11]],
            vec![[4, 7], [8, 11], [12, 7]],
            vec![[2, 11], [2, 15], [14, 15], [14, 11]],
        ],
        "drive" => vec![
            vec![[3, 1], [13, 1], [15, 9], [15, 14], [1, 14], [1, 9], [3, 1]],
            vec![[1, 9], [15, 9]],
            vec![[11, 12], [13, 12]],
        ],
        "system" => vec![
            vec![[1, 2], [15, 2], [15, 11], [1, 11], [1, 2]],
            vec![[8, 11], [8, 15]],
            vec![[4, 15], [12, 15]],
        ],
        "settings" => vec![
            vec![[1, 4], [15, 4]],
            vec![[1, 12], [15, 12]],
            vec![[5, 1], [5, 7]],
            vec![[11, 9], [11, 15]],
        ],
        "check" => vec![vec![[2, 8], [6, 12], [14, 3]]],
        "remove" => vec![vec![[4, 4], [12, 12]], vec![[12, 4], [4, 12]]],
        _ => vec![
            vec![[3, 1], [11, 1], [14, 4], [14, 15], [3, 15], [3, 1]],
            vec![[5, 8], [12, 8]],
            vec![[5, 11], [12, 11]],
        ],
    };
    for path in paths {
        let points = path
            .into_iter()
            .map(|[x, y]| {
                [
                    ((u32::from(x) * u32::from(rect.width) / 16) as u16)
                        .min(rect.width.saturating_sub(1)),
                    ((u32::from(y) * u32::from(rect.height) / 16) as u16)
                        .min(rect.height.saturating_sub(1)),
                ]
            })
            .collect();
        surface.nodes.push(Primitive::Path {
            rect,
            points,
            color: color.into(),
            width: 1,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn icon_paths_stay_local_to_their_rect_on_existing_frontends() {
        for name in [
            "workspace",
            "folder",
            "archive",
            "history",
            "activity",
            "disc",
            "flame",
            "home",
            "download",
            "drive",
            "system",
            "settings",
            "check",
            "remove",
        ] {
            let mut surface = Surface::new(400, 300, "#282828".into());
            draw(
                &mut surface,
                PixelRect::new(150, 220, 16, 16),
                name,
                "#ebdbb2",
            );
            assert!(!surface.nodes.is_empty());
            surface.validate().unwrap();
            for node in &surface.nodes {
                if let Primitive::Path { rect, points, .. } = node {
                    assert!(points
                        .iter()
                        .all(|p| p[0] < rect.width && p[1] < rect.height));
                } else {
                    panic!("icons must use portable vector paths");
                }
            }
        }
    }
}
