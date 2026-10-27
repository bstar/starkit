//! Compare changing CPU/GPU scenes, including hardware readback, without a window.
use anyhow::{ensure, Context, Result};
use starkit::{
    native_surface::{PixelRect, Surface},
    terminal_graphics::{
        protocol::{Component, ImageScale, Rect, Scene, Viewport},
        renderer::{Backend, SceneRenderer},
    },
};
use std::{path::PathBuf, time::Instant};
fn scene(width: u32, height: u32, index: usize) -> Scene {
    let v = Viewport {
        width,
        height,
        columns: (width / 10) as u16,
        rows: (height / 20) as u16,
        generation: 1,
    };
    let mut scene = Scene::from_buffer(
        &starkit::ratatui::buffer::Buffer::empty(starkit::ratatui::layout::Rect::new(
            0, 0, v.columns, v.rows,
        )),
        v,
        index as u64 + 1,
    );
    scene.background = "#151a24".into();
    let mut surface = Surface::new(width as u16, height as u16, "#151a24".into()).with_ui_text();
    surface.text(
        PixelRect::new(24, 18, (width - 48).min(600) as u16, 35),
        "STAR / KIT    Hardware scene renderer",
        "#dce7f4",
        22,
        true,
    );
    surface.text(
        PixelRect::new(24, 60, (width - 48).min(700) as u16, 28),
        "Vector paths · shaped text · images · clipping · transparency",
        "#91a5be",
        14,
        false,
    );
    for row in 0..height.saturating_sub(180) / 34 {
        let y = 114 + row * 34;
        surface.fill(
            PixelRect::new(20, y as u16, width.saturating_sub(40) as u16, 29),
            if row % 2 == 0 { "#202b3a" } else { "#1a2432" },
            5,
        );
        surface.text(
            PixelRect::new(34, y as u16 + 3, 300, 22),
            format!("Session {}  ·  日本語 λ", row + 1),
            "#cbd9ea",
            13,
            false,
        );
        let span = width.saturating_sub(430).max(1);
        let progress = (row * 31 + index as u32 * 13) % span;
        surface.fill(
            PixelRect::new(390, y as u16 + 8, progress.max(1) as u16, 10),
            "#84c5a5",
            3,
        );
    }
    // Include a fractional vector path and a transparent-backed icon.
    surface
        .nodes
        .push(starkit::native_surface::Primitive::Path {
            rect: PixelRect::new(width.saturating_sub(130) as u16, 20, 100, 60),
            points: vec![[0, 50], [30, 4], [65, 39], [99, 8]],
            color: "#d8aa76".into(),
            width: 3,
        });
    scene.components.push(Component::Surface {
        rect: Rect {
            x: 0,
            y: 0,
            width: v.columns,
            height: v.rows,
        },
        surface,
    });
    let mut pixels = starkit::image::RgbaImage::new(24, 16);
    for (x, y, p) in pixels.enumerate_pixels_mut() {
        *p = starkit::image::Rgba(if x < 12 {
            [240, 80, 60, 255]
        } else if y < 8 {
            [50, 210, 90, 160]
        } else {
            [60, 130, 240, 255]
        });
    }
    let png = starkit::terminal_graphics::assets::encode_png(&pixels).unwrap();
    scene.components.push(Component::Image {
        rect: Rect {
            x: v.columns.saturating_sub(21),
            y: 1,
            width: 9,
            height: 3,
        },
        id: "gpu-image".into(),
        png: Some(png.clone()),
        scale: ImageScale::Pixels,
        zoom: 100,
    });
    scene.components.push(Component::Image {
        rect: Rect {
            x: v.columns.saturating_sub(11),
            y: 1,
            width: 9,
            height: 3,
        },
        id: "gpu-zoom".into(),
        png: Some(png),
        scale: ImageScale::Smooth,
        zoom: 200,
    });
    scene
}
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let out = PathBuf::from(
        args.first()
            .map(String::as_str)
            .unwrap_or("/tmp/starkit-gpu"),
    );
    let width = args.get(1).map(|s| s.parse()).transpose()?.unwrap_or(1920);
    let height = args.get(2).map(|s| s.parse()).transpose()?.unwrap_or(1080);
    let count = args
        .get(3)
        .map(|s| s.parse())
        .transpose()?
        .unwrap_or(30usize);
    ensure!(
        (640..=4096).contains(&width)
            && (320..=2160).contains(&height)
            && (2..=240).contains(&count),
        "Use WIDTH 640–4096, HEIGHT 320–2160 and FRAMES 2–240"
    );
    std::fs::create_dir_all(&out)?;
    let mut reports = vec![];
    let mut images = vec![];
    let requested = if std::env::var("STAR_GPU_BENCH_BACKEND").as_deref() == Ok("auto") {
        Backend::Auto
    } else {
        Backend::Gpu
    };
    for backend in [Backend::Cpu, requested] {
        let cold = Instant::now();
        let mut renderer = SceneRenderer::new(backend)?;
        let init = cold.elapsed().as_secs_f64() * 1000.;
        let warm = renderer.render(&scene(width, height, 0))?;
        let mut previous = warm;
        let mut timings = vec![];
        let mut changed = 0usize;
        for index in 1..=count {
            let s = scene(width, height, index);
            let start = Instant::now();
            let image = renderer.render(&s)?;
            timings.push(start.elapsed().as_secs_f64() * 1000.);
            ensure!(image != previous, "Benchmark frame did not change");
            changed += image
                .as_raw()
                .iter()
                .zip(previous.as_raw())
                .filter(|(a, b)| a != b)
                .count();
            previous = image;
        }
        timings.sort_by(f64::total_cmp);
        previous.save(out.join(if backend == Backend::Cpu {
            "cpu.png"
        } else if backend == Backend::Auto {
            "auto.png"
        } else {
            "gpu.png"
        }))?;
        reports.push(serde_json::json!({"backend":renderer.backend(),"initialization_ms":init,"frames":count,"width":width,"height":height,"scene_to_rgba_ms":{"median":timings[count/2],"p95":timings[((count as f64*0.95).ceil()as usize).saturating_sub(1)]},"changed_channels":changed}));
        images.push(previous);
    }
    if requested == Backend::Gpu {
        ensure!(
            reports[1]["backend"]["selected"] == "gpu",
            "Hardware backend was not selected"
        );
    }
    let a = images.first().context("CPU frame")?;
    let b = images.get(1).context("GPU frame")?;
    for image in &images {
        ensure!(
            image.get_pixel(2, 2).0 == [21, 26, 36, 255],
            "Scene background did not render"
        );
        ensure!(
            image.pixels().filter(|p| p.0 == [240, 80, 60, 255]).count() > 10,
            "Image red orientation marker missing"
        );
        ensure!(
            image
                .pixels()
                .filter(|p| p.0 == [60, 130, 240, 255])
                .count()
                > 10,
            "Image blue orientation marker missing"
        );
    }
    let differences: Vec<u8> = a
        .as_raw()
        .iter()
        .zip(b.as_raw())
        .map(|(a, b)| a.abs_diff(*b))
        .collect();
    let report = serde_json::json!({"reports":reports,"comparison":{"mean_absolute_channel_error":differences.iter().map(|v|u64::from(*v)).sum::<u64>()as f64/differences.len()as f64,"channels_over_16":differences.iter().filter(|v|**v>16).count(),"max_channel_error":differences.iter().max()},"includes":"native scene layout, text coverage, vector rasterization/composition and GPU readback; excludes Kitty PNG encoding and final terminal display"});
    let text = serde_json::to_string_pretty(&report)?;
    std::fs::write(out.join("report.json"), &text)?;
    println!("{text}");
    Ok(())
}
