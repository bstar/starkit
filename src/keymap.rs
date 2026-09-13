//! One table describing every action, its keys and its help text.
//!
//! Single source of truth on purpose. The reference implementation both
//! applications took their shape from dispatches keys in hand-written handlers
//! *and* lists them in a separate registry *and* documents them in a third
//! place, which is how `r` ended up meaning four different things. Here the
//! help overlay and the dispatcher read the same table, so they cannot drift.
//!
//! What is here is the machinery: the table's shape, a key spec that parses the
//! strings the table already writes for the reader, and the overlay that draws
//! it. The table itself is the application's, and so is the decision of whether
//! to dispatch through [`Keymap`] or through a hand-written `match`. STAR/AMP
//! matches by hand, because its dispatch is modal -- the same key means
//! different things in different panels -- and a flat map cannot say that.

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Widget, Wrap};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::theme::Theme;

/// One action, the keys that reach it, and how it is described.
///
/// `keys` is written for the reader first -- it is what the help overlay
/// prints -- and parsed second. Alternatives are separated by `/` or `,`, so
/// `"space / c"` is two keys and one line of help.
pub struct Binding<A: Copy + 'static> {
    pub action: A,
    pub keys: &'static str,
    pub label: &'static str,
    pub group: &'static str,
}

/// What the mouse does.
///
/// Same single-table rule as the key bindings: the help overlay reads this, so
/// a gesture cannot be implemented and left undocumented in a second place.
/// There is no dispatcher for it, because pointer handling is geometry and
/// every application's is its own.
pub struct MouseHelp {
    pub gesture: &'static str,
    pub label: &'static str,
    pub group: &'static str,
}

/// One key, as a binding table writes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeySpec {
    pub code: KeyCode,
    pub mods: KeyModifiers,
}

/// The modifiers a binding can ask for.
///
/// Terminals set others -- `SUPER`, `HYPER`, `META`, whatever the window
/// manager passed through -- and a binding that silently failed because the
/// user is on a keyboard whose alt key also reports `META` would be impossible
/// to diagnose from the keyboard.
const INTERESTING: KeyModifiers = KeyModifiers::CONTROL
    .union(KeyModifiers::ALT)
    .union(KeyModifiers::SHIFT);

impl KeySpec {
    /// Parse one key: `"ctrl+s"`, `"alt+1"`, `"space"`, `"?"`, `"F1"`,
    /// `"shift+left"`, `"enter"`, `"esc"`, `"tab"`, `"pgup"`.
    ///
    /// One key, not a list: [`alternatives`] splits a binding's `keys` first.
    /// `None` for anything this cannot match -- a chord like `gg`, a range like
    /// `alt+1..5` -- which a table is allowed to contain, because the column is
    /// for a person to read and some of what a person reads is a summary.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        if s.is_empty() {
            return None;
        }
        let mut mods = KeyModifiers::NONE;
        let mut rest = s;
        // Walked from the left rather than split on the last `+`, because `+`
        // is itself a key: `"ctrl++"` is control and plus.
        while let Some((head, tail)) = rest.split_once('+') {
            let m = match head.trim().to_ascii_lowercase().as_str() {
                "ctrl" | "control" => KeyModifiers::CONTROL,
                "alt" | "meta" | "option" => KeyModifiers::ALT,
                "shift" => KeyModifiers::SHIFT,
                _ => break,
            };
            if tail.trim().is_empty() {
                break;
            }
            mods |= m;
            rest = tail;
        }
        let mut code = key_code(rest.trim())?;

        // Two normalisations, both about what a terminal actually sends.
        //
        // Shift and a letter arrive as the capital letter, so `shift+d` and `D`
        // are the same binding and are stored the same way -- otherwise one of
        // the two spellings never matches anything.
        if let (KeyCode::Char(c), true) = (code, mods.contains(KeyModifiers::SHIFT)) {
            code = KeyCode::Char(c.to_ascii_uppercase());
            mods -= KeyModifiers::SHIFT;
        }
        // And shift with tab arrives as its own code.
        if code == KeyCode::Tab && mods.contains(KeyModifiers::SHIFT) {
            code = KeyCode::BackTab;
            mods -= KeyModifiers::SHIFT;
        }
        Some(Self {
            code,
            mods: mods & INTERESTING,
        })
    }

    /// Is this the key the user pressed?
    pub fn matches(&self, k: KeyEvent) -> bool {
        if k.code != self.code {
            return false;
        }
        let mut want = self.mods & INTERESTING;
        let mut got = k.modifiers & INTERESTING;
        // Shift is already in the character for a printable key, and terminals
        // disagree about whether they also report it. `D` is `D` whether or not
        // the flag came with it; a binding that depended on the flag would work
        // in one terminal and not the next.
        if matches!(self.code, KeyCode::Char(_)) || self.code == KeyCode::BackTab {
            want -= KeyModifiers::SHIFT;
            got -= KeyModifiers::SHIFT;
        }
        want == got
    }
}

/// The keys a binding's `keys` column lists.
///
/// Separated by `/` or `,`, either with spaces around it or without, because
/// that is how the column reads best and the column is written for a person.
pub fn alternatives(keys: &str) -> impl Iterator<Item = &str> {
    keys.split(['/', ','])
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// A table of bindings, ready to answer a key press.
///
/// For an application whose dispatch is flat. One whose meaning of a key
/// depends on what has focus writes the `match` by hand and uses the table only
/// for the help overlay; both are ordinary uses of [`Binding`].
pub struct Keymap<A: Copy + 'static> {
    entries: Vec<(KeySpec, A)>,
}

impl<A: Copy + 'static> Keymap<A> {
    /// Read a table, keeping every key it lists that can be matched.
    ///
    /// An entry that cannot be parsed is dropped rather than refused: a table
    /// may describe a chord or a range of keys in the column a person reads,
    /// and those are dispatched elsewhere or not at all. An application that
    /// wants every one of its bindings to be reachable asserts so in a test of
    /// its own, over [`alternatives`] and [`KeySpec::parse`].
    pub fn from_table(table: &[Binding<A>]) -> Self {
        let mut entries = Vec::new();
        for b in table {
            for alt in alternatives(b.keys) {
                match KeySpec::parse(alt) {
                    Some(spec) => entries.push((spec, b.action)),
                    None => tracing::debug!("keymap: `{alt}` in `{}` is not one key", b.keys),
                }
            }
        }
        Self { entries }
    }

    /// What this key does, if anything. First match in table order wins, so a
    /// table reads top to bottom the way it is written.
    pub fn resolve(&self, k: KeyEvent) -> Option<A> {
        self.entries
            .iter()
            .find(|(spec, _)| spec.matches(k))
            .map(|(_, action)| *action)
    }

    /// How many keys are bound, counting alternatives separately.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

fn key_code(name: &str) -> Option<KeyCode> {
    let lower = name.to_ascii_lowercase();
    Some(match lower.as_str() {
        "space" => KeyCode::Char(' '),
        "enter" | "return" => KeyCode::Enter,
        "esc" | "escape" => KeyCode::Esc,
        "tab" => KeyCode::Tab,
        "backtab" => KeyCode::BackTab,
        "backspace" | "bs" => KeyCode::Backspace,
        "del" | "delete" => KeyCode::Delete,
        "ins" | "insert" => KeyCode::Insert,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pgup" | "pageup" => KeyCode::PageUp,
        "pgdn" | "pagedown" => KeyCode::PageDown,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        _ => {
            if let Some(n) = lower.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()) {
                if (1..=12).contains(&n) {
                    return Some(KeyCode::F(n));
                }
                return None;
            }
            let mut chars = name.chars();
            let c = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            KeyCode::Char(c)
        }
    })
}

/// Columns the key column is padded to before the label starts.
const KEY_COLUMN: usize = 14;

/// The same, for the gestures, which are phrases rather than keys.
const GESTURE_COLUMN: usize = 21;

/// Every binding and every gesture, in two columns over the whole screen.
///
/// Two columns because the key list alone is longer than most terminals are
/// tall, and it used to be silently clipped. The keys scroll; the gestures do
/// not, there being few enough of them to fit.
pub struct HelpView<'a, A: Copy + 'static> {
    pub theme: &'a Theme,
    pub bindings: &'a [Binding<A>],
    pub mouse: &'a [MouseHelp],
    pub scroll: u16,
    /// The word on the border. The overlay adds its own "more below" and "the
    /// end" to it.
    pub title: &'a str,
}

impl<A: Copy + 'static> Widget for HelpView<'_, A> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let t = self.theme;
        let fg = Color::Rgb(t.fg.r, t.fg.g, t.fg.b);
        let key = Color::Rgb(t.accent.r, t.accent.g, t.accent.b);
        let head = Color::Rgb(t.warn.r, t.warn.g, t.warn.b);

        let w = area.width.min(80);
        let h = area.height.min(38);
        let rect = Rect {
            x: area.x + (area.width - w) / 2,
            y: area.y + (area.height - h) / 2,
            width: w,
            height: h,
        };
        Clear.render(rect, buf);

        let heading = |g: &str| {
            Line::from(Span::styled(
                format!("  {g}"),
                Style::default().fg(head).add_modifier(Modifier::BOLD),
            ))
        };
        let entry = |k: &str, label: &str, pad: usize| {
            Line::from(vec![
                Span::styled(format!("  {k:<pad$}"), Style::default().fg(key)),
                Span::styled(label.to_string(), Style::default().fg(fg)),
            ])
        };

        let mut keys: Vec<Line> = Vec::new();
        let mut group = "";
        for b in self.bindings {
            if b.group != group {
                group = b.group;
                keys.push(heading(group));
            }
            keys.push(entry(b.keys, b.label, KEY_COLUMN));
        }

        let mut mouse: Vec<Line> = Vec::new();
        group = "";
        for m in self.mouse {
            if m.group != group {
                group = m.group;
                mouse.push(heading(group));
            }
            mouse.push(entry(m.gesture, m.label, GESTURE_COLUMN));
        }

        // Clamped here rather than where the key is handled, because only the
        // draw knows how tall the box came out and how many lines went in it.
        let inner_h = h.saturating_sub(2);
        let over = (keys.len() as u16).saturating_sub(inner_h);
        let at = self.scroll.min(over);
        let title = if over == 0 {
            format!(" {} ", self.title)
        } else if at == over {
            format!(" {} \u{2014} the end ", self.title)
        } else {
            format!(" {} \u{2014} more below ", self.title)
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Double)
            .border_style(Style::default().fg(Color::Rgb(
                t.border_focused.r,
                t.border_focused.g,
                t.border_focused.b,
            )))
            // Styled rather than inherited: an untitled `title` takes the
            // block's border colour, which is chrome and reads as chrome. The
            // other overlays all name themselves in `header_fg`, and this is
            // the one you open when you cannot find something.
            .title(Span::styled(
                title,
                Style::default()
                    .fg(Color::Rgb(t.header_fg.r, t.header_fg.g, t.header_fg.b))
                    .add_modifier(Modifier::BOLD),
            ))
            .style(Style::default().bg(Color::Rgb(t.bg.r, t.bg.g, t.bg.b)));
        let inner = block.inner(rect);
        block.render(rect, buf);

        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
            .split(inner);
        Paragraph::new(keys)
            .wrap(Wrap { trim: false })
            .scroll((at, 0))
            .render(cols[0], buf);
        Paragraph::new(mouse)
            .wrap(Wrap { trim: false })
            .render(cols[1], buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Act {
        PlayPause,
        Save,
        Focus(u8),
        Back,
        Help,
        Bigger,
    }

    const TABLE: &[Binding<Act>] = &[
        Binding {
            action: Act::PlayPause,
            keys: "space / c",
            label: "play or pause",
            group: "transport",
        },
        Binding {
            action: Act::Bigger,
            keys: "+ / -",
            label: "resize",
            group: "transport",
        },
        Binding {
            action: Act::Save,
            keys: "ctrl+s",
            label: "save",
            group: "files",
        },
        Binding {
            action: Act::Focus(1),
            keys: "alt+1",
            label: "first panel",
            group: "panels",
        },
        Binding {
            action: Act::Back,
            keys: "esc",
            label: "close",
            group: "panels",
        },
        Binding {
            action: Act::Help,
            keys: "? / F1",
            label: "help",
            group: "panels",
        },
    ];

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn the_spellings_a_binding_table_uses_all_parse() {
        let cases: &[(&str, KeyCode, KeyModifiers)] = &[
            ("ctrl+s", KeyCode::Char('s'), KeyModifiers::CONTROL),
            ("alt+1", KeyCode::Char('1'), KeyModifiers::ALT),
            ("space", KeyCode::Char(' '), KeyModifiers::NONE),
            ("?", KeyCode::Char('?'), KeyModifiers::NONE),
            ("F1", KeyCode::F(1), KeyModifiers::NONE),
            ("f12", KeyCode::F(12), KeyModifiers::NONE),
            ("shift+left", KeyCode::Left, KeyModifiers::SHIFT),
            ("enter", KeyCode::Enter, KeyModifiers::NONE),
            ("esc", KeyCode::Esc, KeyModifiers::NONE),
            ("tab", KeyCode::Tab, KeyModifiers::NONE),
            ("pgup", KeyCode::PageUp, KeyModifiers::NONE),
            ("pgdn", KeyCode::PageDown, KeyModifiers::NONE),
            ("del", KeyCode::Delete, KeyModifiers::NONE),
            ("+", KeyCode::Char('+'), KeyModifiers::NONE),
            ("ctrl++", KeyCode::Char('+'), KeyModifiers::CONTROL),
            ("ctrl+alt+x", KeyCode::Char('x'), KeyModifiers::CONTROL),
            ("f", KeyCode::Char('f'), KeyModifiers::NONE),
        ];
        for (s, code, mods) in cases {
            let spec = KeySpec::parse(s).unwrap_or_else(|| panic!("`{s}` did not parse"));
            assert_eq!(spec.code, *code, "`{s}`");
            assert!(spec.mods.contains(*mods), "`{s}` lost {mods:?}");
        }
    }

    #[test]
    fn shift_and_a_letter_is_the_capital_letter() {
        // A terminal sends the capital, with or without the flag, so the two
        // spellings have to land on the same spec or one of them never fires.
        let a = KeySpec::parse("shift+d").unwrap();
        let b = KeySpec::parse("D").unwrap();
        assert_eq!(a, b);
        assert!(a.matches(key(KeyCode::Char('D'), KeyModifiers::SHIFT)));
        assert!(a.matches(key(KeyCode::Char('D'), KeyModifiers::NONE)));
        assert!(!a.matches(key(KeyCode::Char('d'), KeyModifiers::NONE)));
    }

    #[test]
    fn shift_and_tab_is_the_code_the_terminal_actually_sends() {
        let spec = KeySpec::parse("shift+tab").unwrap();
        assert_eq!(spec.code, KeyCode::BackTab);
        assert!(spec.matches(key(KeyCode::BackTab, KeyModifiers::SHIFT)));
        assert!(spec.matches(key(KeyCode::BackTab, KeyModifiers::NONE)));
        assert!(!spec.matches(key(KeyCode::Tab, KeyModifiers::NONE)));
    }

    #[test]
    fn a_modifier_the_binding_did_not_ask_for_is_not_a_match() {
        let spec = KeySpec::parse("left").unwrap();
        assert!(spec.matches(key(KeyCode::Left, KeyModifiers::NONE)));
        assert!(!spec.matches(key(KeyCode::Left, KeyModifiers::SHIFT)));
        assert!(!spec.matches(key(KeyCode::Left, KeyModifiers::CONTROL)));
    }

    #[test]
    fn a_modifier_nobody_binds_is_ignored_rather_than_disqualifying() {
        // Terminals and window managers report flags of their own, and a
        // binding that silently failed on one of them would be undiagnosable
        // from the keyboard.
        let spec = KeySpec::parse("ctrl+s").unwrap();
        let mut k = key(KeyCode::Char('s'), KeyModifiers::CONTROL);
        k.modifiers |= KeyModifiers::META | KeyModifiers::SUPER;
        assert!(spec.matches(k));
    }

    #[test]
    fn what_cannot_be_one_key_is_not_pretended_to_be() {
        for s in ["", "  ", "gg", "alt+1..5", "ctrl+", "f13", "f0", "up/down"] {
            assert!(KeySpec::parse(s).is_none(), "`{s}` parsed as a key");
        }
    }

    #[test]
    fn a_keys_column_is_split_the_way_it_reads() {
        let split = |s| alternatives(s).collect::<Vec<_>>();
        assert_eq!(split("space / c"), vec!["space", "c"]);
        assert_eq!(split("up/down, j/k"), vec!["up", "down", "j", "k"]);
        assert_eq!(split("? / F1"), vec!["?", "F1"]);
        assert_eq!(split("ctrl+s"), vec!["ctrl+s"]);
        assert_eq!(split("+ / -"), vec!["+", "-"], "a key that is punctuation");
    }

    #[test]
    fn every_alternative_in_a_table_reaches_its_action() {
        let map = Keymap::from_table(TABLE);
        assert_eq!(
            map.resolve(key(KeyCode::Char(' '), KeyModifiers::NONE)),
            Some(Act::PlayPause)
        );
        assert_eq!(
            map.resolve(key(KeyCode::Char('c'), KeyModifiers::NONE)),
            Some(Act::PlayPause),
            "the second spelling of the same binding"
        );
        assert_eq!(
            map.resolve(key(KeyCode::Char('s'), KeyModifiers::CONTROL)),
            Some(Act::Save)
        );
        assert_eq!(
            map.resolve(key(KeyCode::Char('1'), KeyModifiers::ALT)),
            Some(Act::Focus(1))
        );
        assert_eq!(
            map.resolve(key(KeyCode::F(1), KeyModifiers::NONE)),
            Some(Act::Help)
        );
        assert_eq!(
            map.resolve(key(KeyCode::Esc, KeyModifiers::NONE)),
            Some(Act::Back)
        );
        // Bare `s` is not ctrl+s, and nothing else claims it.
        assert_eq!(
            map.resolve(key(KeyCode::Char('s'), KeyModifiers::NONE)),
            None
        );
    }

    #[test]
    fn the_first_binding_to_claim_a_key_keeps_it() {
        const CLASH: &[Binding<Act>] = &[
            Binding {
                action: Act::Save,
                keys: "x",
                label: "first",
                group: "g",
            },
            Binding {
                action: Act::Back,
                keys: "x",
                label: "second",
                group: "g",
            },
        ];
        let map = Keymap::from_table(CLASH);
        assert_eq!(
            map.resolve(key(KeyCode::Char('x'), KeyModifiers::NONE)),
            Some(Act::Save)
        );
    }

    #[test]
    fn a_key_the_table_only_describes_costs_nothing() {
        const PROSE: &[Binding<Act>] = &[Binding {
            action: Act::Focus(1),
            keys: "alt+1..5",
            label: "a panel",
            group: "g",
        }];
        let map = Keymap::from_table(PROSE);
        assert!(map.is_empty(), "a range is not a key");
    }

    #[test]
    fn the_overlay_lists_every_group_and_says_where_it_is_in_the_list() {
        let theme = crate::chrome::test_theme("cosmic");
        let draw = |w: u16, h: u16, scroll: u16| {
            let area = Rect::new(0, 0, w, h);
            let mut buf = Buffer::empty(area);
            HelpView {
                theme: &theme,
                bindings: TABLE,
                mouse: &[MouseHelp {
                    gesture: "wheel",
                    label: "scroll",
                    group: "list",
                }],
                scroll,
                title: "HELP",
            }
            .render(area, &mut buf);
            (0..h)
                .map(|y| {
                    (0..w)
                        .map(|x| buf[(x, y)].symbol().to_string())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n")
        };

        let big = draw(100, 44, 0);
        for word in ["transport", "files", "panels", "play or pause", "wheel"] {
            assert!(big.contains(word), "{word} is missing:\n{big}");
        }
        // Everything fits, so the title says nothing about scrolling.
        assert!(big.contains(" HELP "), "{big}");
        assert!(!big.contains("more below"), "{big}");

        // Too short for the key list: the title offers the rest, and says so
        // again when there is no more of it.
        let short = draw(100, 10, 0);
        assert!(short.contains("more below"), "{short}");
        let end = draw(100, 10, 999);
        assert!(end.contains("the end"), "{end}");
    }

    #[test]
    fn an_area_too_small_to_hold_the_box_still_draws_something() {
        let theme = crate::chrome::test_theme("cosmic");
        for (w, h) in [(1u16, 1u16), (3, 3), (10, 4), (0, 0)] {
            let area = Rect::new(0, 0, w, h);
            let mut buf = Buffer::empty(area);
            HelpView {
                theme: &theme,
                bindings: TABLE,
                mouse: &[],
                scroll: 0,
                title: "HELP",
            }
            .render(area, &mut buf);
        }
    }
}
