//! Shared runtime smoke example; --png captures without taking over the terminal.
use anyhow::Context as _;
use starkit::terminal_graphics::{
    protocol::*,
    renderer::{KittyPresenter, RenderMessage, Renderer},
};
use std::io::Write;
use std::time::Duration;
#[path = "terminal-graphics/interactive.rs"]
mod interactive;
fn main() -> anyhow::Result<()> {
    if interactive::handles() {
        return interactive::run();
    }
    let viewport = Viewport::default();
    let mut scene = Scene {
        revision: 1,
        interaction: 1,
        scroll_interaction: None,
        viewport,
        background: "#1e1e2e".into(),
        foreground: "#cdd6f4".into(),
        accent: "#89b4fa".into(),
        border: "#45475a".into(),
        spans: vec![],
        placements: vec![],
        resize_handles: vec![],
        components: vec![
            Component::Panel {
                rect: Rect {
                    x: 1,
                    y: 1,
                    width: 98,
                    height: 38,
                },
                active: true,
            },
            Component::Tab {
                number: None,
                close: None,
                rect: Rect {
                    x: 3,
                    y: 2,
                    width: 20,
                    height: 3,
                },
                label: "STAR / KIT  ×".into(),
                active: true,
            },
            Component::Meter {
                rect: Rect {
                    x: 4,
                    y: 33,
                    width: 30,
                    height: 1,
                },
                value: 650,
                foreground: "#89b4fa".into(),
                background: "#45475a".into(),
            },
        ],
    };
    scene.components.push(interactive::preview(Rect {
        x: 77,
        y: 7,
        width: 18,
        height: 8,
    }));
    for i in 0..20 {
        scene.components.push(Component::ListRow {
            rect: Rect {
                x: 4,
                y: 7 + i,
                width: 70,
                height: 1,
            },
            label: format!("Shared graphical component {i} · Unicode 日本語"),
            icon: if i % 3 == 0 { "folder" } else { "file" }.into(),
            foreground: "#cdd6f4".into(),
            background: if i == 3 { "#45475a" } else { "#1e1e2e" }.into(),
            selected: i == 3,
            marked: i == 7,
            marking: true,
        });
    }
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let output = if args.first().is_some_and(|s| s == "--scene") {
        scene = serde_json::from_slice(&std::fs::read(args.get(1).expect("--scene PATH"))?)?;
        args.get(2)
            .cloned()
            .unwrap_or_else(|| "/tmp/starkit-graphical.png".into())
    } else {
        args.first()
            .cloned()
            .unwrap_or_else(|| "/tmp/starkit-graphical.png".into())
    };
    let iterations = std::env::var("STAR_GRAPHICS_BENCH_FRAMES")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(1)
        .clamp(1, 1000);
    let mut timings = vec![];
    let mut presentation_times = vec![];
    let mut payloads = vec![];
    let mut presenter = KittyPresenter::default();
    let mut wire = Vec::new();
    let started = std::time::Instant::now();
    let mut renderer = Renderer::spawn()?;
    renderer.scene(&scene)?;
    for index in 0..iterations {
        scene.revision = index as u64 + 1;
        if let Some(Component::ListRow { selected, .. }) = scene
            .components
            .iter_mut()
            .find(|c| matches!(c, Component::ListRow { .. }))
        {
            *selected = index % 2 == 0;
        }
        let sent = std::time::Instant::now();
        if index > 0 {
            renderer.scene(&scene)?;
        }
        loop {
            match renderer.output.recv_timeout(Duration::from_secs(10))? {
                RenderMessage::Frame {
                    pixels,
                    width,
                    height,
                    ..
                } => {
                    use base64::Engine;
                    let pixels = pixels.context("Native frame has no pixels")?;
                    let png = starkit::terminal_graphics::renderer::encode_pixels(&pixels)?;
                    let bytes = base64::engine::general_purpose::STANDARD.decode(&png)?;
                    if index == iterations - 1 {
                        std::fs::File::create(&output)?.write_all(&bytes)?;
                    }
                    timings.push(sent.elapsed().as_secs_f64() * 1000.0);
                    let before = std::time::Instant::now();
                    wire.clear();
                    let payload = presenter.present_pixels(
                        pixels,
                        Viewport {
                            width,
                            height,
                            ..scene.viewport
                        },
                        &mut wire,
                    )?;
                    presentation_times.push(before.elapsed().as_secs_f64() * 1000.0);
                    payloads.push(payload);
                    if index == 0 {
                        println!(
                            "{output}: {width}×{height}, startup {:?}, PNG {} bytes",
                            started.elapsed(),
                            bytes.len()
                        );
                    }
                    break;
                }
                RenderMessage::Error { message } => anyhow::bail!("{message}"),
                _ => {}
            }
        }
    }
    timings.sort_by(f64::total_cmp);
    presentation_times.sort_by(f64::total_cmp);
    println!(
        "{iterations} frames; scene-to-frame p95 {:.2} ms",
        timings[(timings.len() - 1) * 95 / 100]
    );
    println!("region comparison/encoding p95 {:.2} ms; initial payload {} bytes; later mean payload {:.0} bytes",
        presentation_times[(presentation_times.len() - 1) * 95 / 100],
        payloads[0],
        payloads.iter().skip(1).sum::<usize>() as f64 / payloads.len().saturating_sub(1).max(1) as f64);
    Ok(())
}
