//! Run interactively, inspect without a terminal, or measure the solver:
//! cargo run --example responsive-panes --no-default-features -- --inspect 80x24
use starkit::{
    crossterm::event::{self, Event, KeyCode, KeyEventKind, MouseButton, MouseEventKind},
    panes::{Focus, Node, Overflow, Size, ViewportRule, Visibility, Wrap},
    ratatui::{
        layout::Rect,
        style::{Color, Style},
        widgets::{Block, Borders, Paragraph},
    },
};
use std::time::{Duration, Instant};

fn tree(hidden: bool) -> Node<&'static str> {
    Node::column(vec![
        Node::pane("workspace").height(Size::fill(5)).priority(100),
        Node::row(vec![
            Node::pane("preview")
                .width(Size::fill(30))
                .height(Size::bounded(4, 8, 12).grow(1))
                .priority(20)
                .compact(Size::fill(12), Size::fixed(3))
                .overflow(Overflow::CollapseThenHide),
            Node::pane("activity")
                .width(Size::fill(30))
                .height(Size::bounded(4, 8, 12).grow(1))
                .visibility(if hidden {
                    Visibility::Hidden
                } else {
                    Visibility::Expanded
                })
                .compact(Size::fill(12), Size::fixed(3))
                .overflow(Overflow::CollapseThenHide),
        ])
        .wrap(Wrap::AtMinimum)
        .gap(1)
        .height(Size::bounded(0, 8, 24)),
        Node::pane("details")
            .height(Size::fixed(3))
            .when(ViewportRule {
                min_height: 28,
                ..ViewportRule::default()
            }),
    ])
    .gap(1)
}
fn dimensions(s: &str) -> anyhow::Result<Rect> {
    let (width, height) = s
        .split_once('x')
        .ok_or_else(|| anyhow::anyhow!("expected WIDTHxHEIGHT"))?;
    Ok(Rect::new(0, 0, width.parse()?, height.parse()?))
}
fn main() -> anyhow::Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|s| s == "--inspect") {
        let area = dimensions(args.get(1).map(String::as_str).unwrap_or("80x24"))?;
        println!(
            "{}",
            serde_json::to_string_pretty(&tree(false).layout(area)?)?
        );
        return Ok(());
    }
    if args.first().is_some_and(|s| s == "--bench") {
        let tree = tree(false);
        let start = Instant::now();
        for i in 0u32..100_000 {
            std::hint::black_box(tree.layout(Rect::new(
                0,
                0,
                60 + (i % 160) as u16,
                20 + (i % 60) as u16,
            ))?);
        }
        println!(
            "100000 layouts: {:.2} microseconds/layout",
            start.elapsed().as_secs_f64() * 10.
        );
        return Ok(());
    }
    let mut terminal = starkit::term::init()?;
    let result = (|| -> anyhow::Result<()> {
        let mut hidden = false;
        let mut focus = Focus::new(Some("workspace"));
        loop {
            let mut solved = tree(hidden).layout(terminal.size()?.into())?;
            terminal.draw(|frame| {
                solved = tree(hidden).layout(frame.area()).expect("static valid example tree");
                focus.reconcile(&solved);
                for pane in &solved.panes {
                    if let Some(area) = pane.area {
                        let selected = focus.active() == Some(pane.id);
                        let text = format!("{:?} / {:?}\nTab: focus  h: activity  q: exit\nResize to wrap, collapse and restore panes.", pane.state.visibility, pane.state.reason);
                        frame.render_widget(Paragraph::new(text).block(Block::default().borders(Borders::ALL)
                            .title(pane.id).border_style(Style::default().fg(if selected {Color::Cyan} else {Color::DarkGray}))), area);
                    }
                }
            })?;
            if event::poll(Duration::from_millis(100))? {
                match event::read()? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => break,
                        KeyCode::Char('h') => hidden = !hidden,
                        KeyCode::Tab => {
                            focus.step(&solved, true);
                        }
                        KeyCode::BackTab => {
                            focus.step(&solved, false);
                        }
                        _ => {}
                    },
                    Event::Mouse(mouse)
                        if mouse.kind == MouseEventKind::Down(MouseButton::Left) =>
                    {
                        if let Some(id) = solved.hit(mouse.column, mouse.row) {
                            focus.request(id);
                        }
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    })();
    let restored = starkit::term::restore();
    result.and(restored)
}
