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

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::theme::Theme;

/// One action, the keys that reach it, and how it is described.
///
/// `keys` is written for the reader first -- it is what the help overlay
/// prints -- and parsed second. Alternatives are separated by `/` or `,`, so
/// `"space / c"` is two keys and one line of help. A binding *on* one of those
/// two characters is written as a token of its own -- `"ctrl+f / /"` -- which
/// [`alternatives`] explains.
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
/// that is how the column reads best and the column is written for a person:
/// `"up/down, j/k"` is four keys.
///
/// A separator that has nothing on one side of it is not a separator but the
/// key itself, which is how a binding on the slash is written:
///
/// ```
/// use starkit::keymap::alternatives;
/// let keys: Vec<&str> = alternatives("ctrl+f / /").collect();
/// assert_eq!(keys, ["ctrl+f", "/"]);
/// ```
///
/// There is one such key in either application and it is `/`, the search key
/// every other client has, which until this rule existed could not be put in
/// the column the help overlay prints and had to be handled beside the table
/// as an exception. Put it last, where the empty side is the end of the
/// string; the same reading applies to `,`.
pub fn alternatives(keys: &str) -> impl Iterator<Item = &str> {
    let mut out: Vec<&str> = Vec::new();
    let mut start = 0usize;
    for (i, c) in keys.char_indices() {
        if c != '/' && c != ',' {
            continue;
        }
        let end = i + c.len_utf8();
        // Nothing before it, and nothing after it before the next separator:
        // there is no alternative here for it to separate, so it is one.
        let rest = keys[end..].trim_start();
        let alone =
            keys[start..i].trim().is_empty() && (rest.is_empty() || rest.starts_with(['/', ',']));
        if alone {
            out.push(&keys[i..end]);
        } else if !keys[start..i].trim().is_empty() {
            out.push(keys[start..i].trim());
        }
        start = end;
    }
    if !keys[start..].trim().is_empty() {
        out.push(keys[start..].trim());
    }
    out.into_iter()
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
    ///
    /// The arriving key is spelled the way [`KeySpec::parse`] spells a written
    /// one before it is looked up -- see [`normalise`] -- so a table does not
    /// have to list both of the spellings a terminal might send.
    pub fn resolve(&self, k: KeyEvent) -> Option<A> {
        let k = normalise(k)?;
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

/// Spell an arriving key the way [`KeySpec::parse`] spells a written one.
///
/// The two normalisations are the ones `parse` performs on the string, from
/// the other end: `parse` turns `shift+tab` into `BackTab` and `shift+d` into
/// `D` because that is what most terminals send, and the terminals that send
/// the other spelling have to arrive at the same key or a binding works in one
/// terminal and not the next.
///
/// `None` for anything that is not a press. A terminal that reports releases
/// sends the same key twice, and running the action on both is the sort of bug
/// that only appears on somebody else's machine. Repeats are not resolved
/// either: they only arrive when an application has turned on the keyboard
/// protocol that reports them, which [`term::init`](crate::term::init) does
/// not, and an application that turns it on has decided for itself what
/// holding a key means.
fn normalise(mut k: KeyEvent) -> Option<KeyEvent> {
    if k.kind != KeyEventKind::Press {
        return None;
    }
    if k.code == KeyCode::Tab && k.modifiers.contains(KeyModifiers::SHIFT) {
        k.code = KeyCode::BackTab;
        k.modifiers -= KeyModifiers::SHIFT;
    }
    if let KeyCode::Char(c) = k.code {
        if k.modifiers.contains(KeyModifiers::SHIFT) {
            k.code = KeyCode::Char(c.to_ascii_uppercase());
        }
    }
    Some(k)
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
///
/// Public because an application that generates its own key documentation --
/// a manual page, a `docs/keys-and-mouse.md` with a test that greps it --
/// lays the same two columns out somewhere this widget is not, and a second
/// copy of the number is a document that drifts from the overlay.
pub const KEYS_COLUMN: usize = 14;

/// The same, for the gestures, which are phrases rather than keys.
pub const GESTURE_COLUMN: usize = 21;

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
            keys.push(entry(b.keys, b.label, KEYS_COLUMN));
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

    /// The one key the column's own syntax could not write. Both applications
    /// bind `/` to search, as every other client does, and it used to be
    /// handled beside the table as an exception with a comment explaining
    /// itself.
    #[test]
    fn a_binding_on_the_separator_is_spelled_in_the_column_like_any_other() {
        let split = |s| alternatives(s).collect::<Vec<_>>();
        assert_eq!(split("ctrl+f / /"), vec!["ctrl+f", "/"]);
        assert_eq!(split("/"), vec!["/"], "on its own");
        assert_eq!(split("a , ,"), vec!["a", ","], "and the other separator");
        assert_eq!(split("/ / a"), vec!["/", "a"], "first rather than last");

        const SEARCH: &[Binding<Act>] = &[Binding {
            action: Act::Help,
            keys: "ctrl+f / /",
            label: "search",
            group: "g",
        }];
        let map = Keymap::from_table(SEARCH);
        assert_eq!(
            map.resolve(key(KeyCode::Char('/'), KeyModifiers::NONE)),
            Some(Act::Help)
        );
        assert_eq!(
            map.resolve(key(KeyCode::Char('f'), KeyModifiers::CONTROL)),
            Some(Act::Help)
        );
    }

    /// The same key, spelled the two ways terminals spell it.
    #[test]
    fn an_arriving_key_is_normalised_the_way_a_written_one_is() {
        const TABS: &[Binding<Act>] = &[
            Binding {
                action: Act::Back,
                keys: "shift+tab",
                label: "back",
                group: "g",
            },
            Binding {
                action: Act::Bigger,
                keys: "D",
                label: "delete",
                group: "g",
            },
        ];
        let map = Keymap::from_table(TABS);
        assert_eq!(
            map.resolve(key(KeyCode::BackTab, KeyModifiers::NONE)),
            Some(Act::Back)
        );
        assert_eq!(
            map.resolve(key(KeyCode::Tab, KeyModifiers::SHIFT)),
            Some(Act::Back),
            "the terminals that send tab with the flag instead"
        );
        assert_eq!(
            map.resolve(key(KeyCode::Char('D'), KeyModifiers::NONE)),
            Some(Act::Bigger)
        );
        assert_eq!(
            map.resolve(key(KeyCode::Char('d'), KeyModifiers::SHIFT)),
            Some(Act::Bigger),
            "and the ones that send the letter unshifted with the flag"
        );
        assert_eq!(
            map.resolve(key(KeyCode::Char('d'), KeyModifiers::NONE)),
            None,
            "which is not the same as the letter on its own"
        );
    }

    /// A terminal that reports releases sends every key twice, and an action
    /// that ran on both would delete two messages for one press.
    #[test]
    fn only_a_press_does_anything() {
        let map = Keymap::from_table(TABLE);
        let mut k = key(KeyCode::Char('s'), KeyModifiers::CONTROL);
        assert_eq!(map.resolve(k), Some(Act::Save));
        for kind in [KeyEventKind::Release, KeyEventKind::Repeat] {
            k.kind = kind;
            assert_eq!(map.resolve(k), None, "{kind:?}");
        }
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

    /// The columns the overlay lays out are the ones it publishes, so a
    /// generated key reference lines up with the screen it documents.
    #[test]
    fn the_published_column_widths_are_the_ones_the_overlay_uses() {
        let theme = crate::chrome::test_theme("cosmic");
        let area = Rect::new(0, 0, 100, 44);
        let mut buf = Buffer::empty(area);
        HelpView {
            theme: &theme,
            bindings: TABLE,
            mouse: &[MouseHelp {
                gesture: "wheel",
                label: "scroll",
                group: "list",
            }],
            scroll: 0,
            title: "HELP",
        }
        .render(area, &mut buf);

        let row = |needle: &str| {
            (0..area.height)
                .map(|y| {
                    (0..area.width)
                        .map(|x| buf[(x, y)].symbol().to_string())
                        .collect::<String>()
                })
                .find(|line| line.contains(needle))
                .unwrap_or_else(|| panic!("{needle} was not drawn"))
        };

        let keys = row("play or pause");
        assert_eq!(
            keys.find("play or pause").unwrap() - keys.find("space / c").unwrap(),
            KEYS_COLUMN
        );
        let mouse = row("scroll");
        assert_eq!(
            mouse.find("scroll").unwrap() - mouse.find("wheel").unwrap(),
            GESTURE_COLUMN
        );
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
