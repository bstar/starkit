# Searchable command guides

`help::Guide` provides the standard STAR command guide in native pixels and
terminal cells. It owns search, categories, keyboard and pointer navigation,
selection, scrolling, and themed modal presentation. Apps own a catalogue of
`help::Command` values with stable IDs, readable titles, categories, shortcuts,
summaries, instructions and search aliases. Include buttons and mouse gestures,
not just keyboard bindings. Context-specific keys must explain their context.

Open with `?` or F1 outside text entry. F1 can also open contextual help while
another view owns typing. Route keyboard, paste and pointer input to the guide
before dispatching application commands. Browsing never executes a command.
Close on `Outcome::Close`; restore the previous application view and focus.
Both pointer entry points consume events outside the guide. In the cell view,
the footer's `Esc close` label can also be clicked. Results and details accept
wheel input independently, using their painted geometry for routing.

```rust
use starkit::help::{Command, Guide};
let mut help = Guide::new("MY APP", vec![
    Command::new("microphone", "Choose microphone", "Audio")
        .shortcut("m")
        .summary("Record narration on a separate track.")
        .instructions("Open Microphone, search for a device, and press Enter.")
        .keywords("voice narration input device"),
]);
help.search("voice input");
assert_eq!(help.highlighted().unwrap().id, "microphone");
```

Search is case-insensitive, combines words in any order across all fields, and
ranks titles and shortcuts ahead of incidental instruction matches. A single
insertion, deletion, substitution or adjacent transposition is tolerated in
words of at least four characters. Short keys require direct matches. Quoted
phrases require an exact normalized phrase; `-word` excludes direct matches.
Control/Ctrl, Option/Alt, Escape/Esc and arrow glyphs share normalizations.
Use `category:audio` to restrict categories, `key:ctrl+s` to find a shortcut,
or `title:"play or pause"` to restrict a title phrase. Combine these with
ordinary words and exclusions such as `-category:editing`. Field filters use
direct matches so an exact constraint is never widened by typo tolerance.
Pasted input is bounded to 256 characters. Matching and category counts update
on edits; painting does not repeat fuzzy search.

- Type or paste to search; use standard text-editing and word-deletion keys.
- Up/Down browse; PageUp/PageDown move eight results.
- Tab/Shift+Tab change category; native category controls are clickable.
- Ctrl+F searches all categories without discarding the query.
- Ctrl+U or Clear clears the query.
- Ctrl+Home/Ctrl+End select the first/last result.
- Wheel over results browses; wheel over instructions scrolls that pane.
- Ctrl+PageUp/PageDown scroll instructions; native arrow buttons do the same.
- Esc or F1 closes; `?` closes when the search is empty.

For native presentation, get `Guide::native_rect` from the physical viewport,
render `native`, mount after a `Component::Menu`, and route `pointer_native`
with those exact bounds. Use KIT's UI font and palette. For cell fallback, call
`render_cell` and `pointer_cell` with the same available area. Instructions wrap
and scroll in both modes; small windows stack results above details.

This is an additive API in the unreleased KIT 0.9 candidate. The existing
`keymap::HelpView` remains compatible for consumers that have not migrated.
AMP derives its shortcut/gesture entries from the existing dispatch tables;
REEL supplies a feature catalogue including precise command syntax. Neither
application puts playback, recording or project-editing logic into KIT.
