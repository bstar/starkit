//! Breaking text into rows of a fixed number of columns, and the rules about
//! where text may be cut at all.
//!
//! ratatui can already wrap a `Paragraph`, and both applications used to let
//! it. The reason this exists is that `Paragraph` wraps while it draws and
//! reports nothing: a message list that has to know how tall a message is
//! *before* it decides which messages are on screen, and which byte of the
//! original string is under the pointer, cannot ask it. So wrapping happens
//! once, up front, and what comes back is a list of byte ranges.
//!
//! ## What a row is
//!
//! Every byte of the input lands in exactly one row's `range`, including the
//! `\n` that ended a row and the space a break was taken at. `width` is the
//! width of what is actually *drawn*, which is the range with its trailing
//! newline and trailing spaces removed: a break space is carried so the ranges
//! still tile, and drawing it would put a stray cell of selection colour past
//! the last word. [`Row::drawn`] hands back that slice.
//!
//! ## Clusters, and the segmenter that is not here
//!
//! Text is measured and cut in *clusters*: a character plus the marks that
//! attach to it -- combining accents, variation selectors, skin tones, and the
//! whole of a zero-width-joiner emoji sequence. That is deliberately an
//! approximation of a Unicode grapheme cluster, built from the handful of
//! rules that actually come up in a chat message, rather than a dependency on
//! `unicode-segmentation` and the table that comes with it. Regional-indicator
//! flag pairs and the scripts with prepended marks are not covered; if either
//! turns up in a bug report, this is the one place to fix it.
//!
//! A cluster's width is `unicode-width`'s, measured on the cluster, so a
//! family emoji is the two columns a terminal gives it rather than the six its
//! characters add up to. The width of a row is the sum of its clusters, which
//! is what a cell grid does; it can differ from `str::width()` of the same
//! slice where Unicode defines a ligature spanning two base characters, and
//! the cell grid does not draw those as one cell either.

use std::collections::HashMap;
use std::hash::Hash;
use std::ops::Range;

use unicode_width::UnicodeWidthStr;

const ZWJ: char = '\u{200d}';

/// Does this character belong to whatever came before it?
fn is_continuation(c: char) -> bool {
    matches!(c,
        ZWJ
        | '\u{0300}'..='\u{036f}'   // combining diacritical marks
        | '\u{1ab0}'..='\u{1aff}'   // .. extended
        | '\u{1dc0}'..='\u{1dff}'   // .. supplement
        | '\u{20d0}'..='\u{20f0}'   // .. for symbols, keycap included
        | '\u{fe00}'..='\u{fe0f}'   // variation selectors
        | '\u{fe20}'..='\u{fe2f}'   // combining half marks
        | '\u{1f3fb}'..='\u{1f3ff}' // emoji skin tone modifiers
        | '\u{e0100}'..='\u{e01ef}' // variation selectors supplement
    )
}

/// The end of the cluster that starts at `at`.
///
/// `at` must be a character boundary; the result always is one.
pub fn next_boundary(text: &str, at: usize) -> usize {
    if at >= text.len() {
        return text.len();
    }
    let mut i = at;
    loop {
        let c = text[i..].chars().next().expect("in bounds");
        i += c.len_utf8();
        if i >= text.len() || c == '\n' {
            break;
        }
        let n = text[i..].chars().next().expect("in bounds");
        // A joiner joins to something. Left dangling before a space or the end
        // of a line it is its own cluster, which is also how a terminal draws
        // it -- as nothing, or as the replacement box, but on its own.
        if is_continuation(n) || (c == ZWJ && n != ' ' && n != '\n') {
            continue;
        }
        break;
    }
    i
}

/// The start of the cluster that ends at `at`.
pub fn prev_boundary(text: &str, at: usize) -> usize {
    if at == 0 {
        return 0;
    }
    let mut i = at.min(text.len());
    loop {
        let c = text[..i].chars().next_back().expect("in bounds");
        i -= c.len_utf8();
        if i == 0 || c == '\n' {
            break;
        }
        let p = text[..i].chars().next_back().expect("in bounds");
        if p == '\n' {
            break;
        }
        if is_continuation(c) || (p == ZWJ && c != ' ') {
            continue;
        }
        break;
    }
    i
}

/// Walk a string cluster by cluster, yielding `(byte offset, cluster)`.
pub fn clusters(text: &str) -> Clusters<'_> {
    Clusters { text, at: 0 }
}

/// Iterator over the clusters of a string. See [`clusters`].
#[derive(Debug, Clone)]
pub struct Clusters<'a> {
    text: &'a str,
    at: usize,
}

impl<'a> Iterator for Clusters<'a> {
    type Item = (usize, &'a str);

    fn next(&mut self) -> Option<Self::Item> {
        if self.at >= self.text.len() {
            return None;
        }
        let start = self.at;
        self.at = next_boundary(self.text, start);
        Some((start, &self.text[start..self.at]))
    }
}

/// Display columns of a string, measured cluster by cluster.
pub fn width_of(text: &str) -> u16 {
    let w: usize = clusters(text).map(|(_, c)| c.width()).sum();
    w.min(u16::MAX as usize) as u16
}

/// One row of wrapped text: a slice of the input and the columns it occupies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// Bytes of the input on this row, break space and newline included.
    pub range: Range<usize>,
    /// Display columns of the drawn part of the row.
    pub width: u16,
}

impl Row {
    /// The whole slice, trailing break space and newline included.
    pub fn slice<'a>(&self, text: &'a str) -> &'a str {
        &text[self.range.clone()]
    }

    /// The part to draw: the slice without the newline, or the trailing spaces
    /// that were only there to say where a break was allowed.
    pub fn drawn<'a>(&self, text: &'a str) -> &'a str {
        self.slice(text)
            .trim_end_matches('\n')
            .trim_end_matches(' ')
    }
}

/// Wrap `text` to `width` columns.
///
/// Greedy: a row takes as many words as fit, breaks at the last space it saw,
/// and hard-breaks a word wider than the whole row. `\n` always ends a row,
/// and the row after the last `\n` is emitted even when it is empty, so a
/// trailing newline shows as the blank line it is and there are never fewer
/// rows than lines.
///
/// Two edges worth knowing:
///
/// - `width == 0` returns a single empty row. There is nowhere to put the text
///   and no useful answer; the caller has a zero-column rect and should not be
///   drawing at all.
/// - a cluster wider than the whole row -- a CJK glyph at `width == 1` -- gets
///   a row to itself, and that row's `width` exceeds `width`. Splitting it
///   would cut a character in half, which corrupts the line rather than merely
///   overflowing it.
pub fn wrap(text: &str, width: u16) -> Vec<Row> {
    if width == 0 {
        return vec![Row {
            range: 0..0,
            width: 0,
        }];
    }
    let limit = u32::from(width);
    let mut rows = Vec::new();

    // `total` counts everything on the row including a trailing run of spaces;
    // `content` stops at the last non-space. The difference is what makes a
    // break space free: inside the range, outside the width.
    let mut start = 0usize;
    let mut total = 0u32;
    let mut content = 0u32;
    // Where the row may be cut, and the width it would have if it were: the
    // byte just past the most recent run of spaces.
    let mut brk: Option<(usize, u32)> = None;
    // Width of the run of non-space clusters since `brk`, so that after a
    // break the new row's width is known without measuring it again.
    let mut word = 0u32;

    for (i, cl) in clusters(text) {
        if cl == "\n" {
            rows.push(Row {
                range: start..i + 1,
                width: content as u16,
            });
            start = i + 1;
            total = 0;
            content = 0;
            word = 0;
            brk = None;
            continue;
        }

        let cw = cl.width() as u32;

        if cl == " " {
            // Spaces hang past the right edge rather than forcing a break:
            // they are not drawn if the row ends here.
            total += cw;
            brk = Some((i + 1, content));
            word = 0;
            continue;
        }

        if total + cw > limit {
            if let Some((at, w)) = brk {
                if at > start {
                    rows.push(Row {
                        range: start..at,
                        width: w as u16,
                    });
                    start = at;
                    total = word;
                    content = word;
                    brk = None;
                }
            }
            // Either there was no break to take, or the word after it is
            // itself wider than a row. Cut between clusters.
            if total + cw > limit && i > start {
                rows.push(Row {
                    range: start..i,
                    width: content as u16,
                });
                start = i;
                total = 0;
                // `content` and `word` are set from `total` below, on the way
                // out of the loop body; this cluster is the new row's first.
                word = 0;
                brk = None;
            }
        }

        total += cw;
        content = total;
        word += cw;
    }

    rows.push(Row {
        range: start..text.len(),
        width: content as u16,
    });
    rows
}

/// How many rows `text` occupies at `width`.
pub fn height(text: &str, width: u16) -> u16 {
    wrap(text, width).len().min(u16::MAX as usize) as u16
}

/// A wrapped layout, remembered.
///
/// Wrapping one message is cheap and wrapping eight hundred of them on every
/// frame of a scroll is not, and the answer only changes when the text or the
/// width does. The key is the caller's own -- a message id and whatever else
/// it renders from -- paired with the width, because the same message at two
/// widths is two layouts and both are worth keeping while a seam is dragged.
///
/// Eviction is least-recently-used, in batches: over either cap it drops the
/// oldest eighth rather than one entry, so a full cache costs one scan per
/// hundreds of inserts instead of one per insert.
pub struct WrapCache<K> {
    entries: HashMap<(K, u16), Entry>,
    clock: u64,
    bytes: usize,
    max_entries: usize,
    max_bytes: usize,
}

struct Entry {
    rows: Vec<Row>,
    used: u64,
    bytes: usize,
}

impl<K: Hash + Eq + Clone> WrapCache<K> {
    /// `max_bytes` counts the rows, not the text they point into: the strings
    /// belong to the caller and the cache never copies them.
    pub fn new(max_entries: usize, max_bytes: usize) -> Self {
        Self {
            entries: HashMap::new(),
            clock: 0,
            bytes: 0,
            max_entries: max_entries.max(1),
            max_bytes,
        }
    }

    /// The rows for `key` at `width`, calling `build` on a miss.
    pub fn rows(&mut self, key: K, width: u16, build: impl FnOnce() -> Vec<Row>) -> &[Row] {
        self.clock += 1;
        let now = self.clock;
        let k = (key, width);
        if !self.entries.contains_key(&k) {
            let rows = build();
            let bytes = rows.capacity() * std::mem::size_of::<Row>();
            // Evict first: the entry being inserted is the one that must
            // survive, and it is not in the map yet.
            self.trim(bytes);
            self.bytes += bytes;
            self.entries.insert(
                k.clone(),
                Entry {
                    rows,
                    used: now,
                    bytes,
                },
            );
        }
        let e = self.entries.get_mut(&k).expect("just inserted");
        e.used = now;
        &e.rows
    }

    /// Forget every width of one key, for when what it describes has changed.
    pub fn invalidate(&mut self, key: &K) {
        let bytes = &mut self.bytes;
        self.entries.retain(|(k, _), e| {
            let keep = k != key;
            if !keep {
                *bytes -= e.bytes;
            }
            keep
        });
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Heap held by the cached rows, as counted against `max_bytes`.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// Make room for one more entry of `incoming` bytes.
    fn trim(&mut self, incoming: usize) {
        let over_entries = self.entries.len() + 1 > self.max_entries;
        let over_bytes = self.bytes + incoming > self.max_bytes;
        if !over_entries && !over_bytes {
            return;
        }
        let mut by_age: Vec<(u64, usize)> =
            self.entries.values().map(|e| (e.used, e.bytes)).collect();
        by_age.sort_unstable();

        // An eighth, or as many as it takes to get back under the cap that
        // was hit, whichever is more.
        let mut drop = (by_age.len() / 8).max(1);
        if over_entries {
            drop = drop.max(self.entries.len() + 1 - self.max_entries);
        }
        if over_bytes {
            let mut freed = 0usize;
            let mut n = 0usize;
            for &(_, b) in &by_age {
                if self.bytes + incoming - freed <= self.max_bytes {
                    break;
                }
                freed += b;
                n += 1;
            }
            drop = drop.max(n);
        }
        let drop = drop.min(by_age.len());
        if drop == 0 {
            return;
        }
        let cutoff = by_age[drop - 1].0;
        let bytes = &mut self.bytes;
        self.entries.retain(|_, e| {
            let keep = e.used > cutoff;
            if !keep {
                *bytes -= e.bytes;
            }
            keep
        });
    }
}

impl<K: Hash + Eq + Clone> Default for WrapCache<K> {
    /// Two thousand entries and eight mebibytes, which is roughly a channel of
    /// scrollback at a couple of widths.
    fn default() -> Self {
        Self::new(2000, 8 << 20)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// The drawn text of every row, which is what a test usually asserts.
    fn drawn(text: &str, rows: &[Row]) -> Vec<String> {
        rows.iter().map(|r| r.drawn(text).to_string()).collect()
    }

    #[test]
    fn a_short_line_is_one_row() {
        let t = "hello";
        assert_eq!(drawn(t, &wrap(t, 20)), vec!["hello"]);
        assert_eq!(wrap(t, 20)[0].width, 5);
    }

    #[test]
    fn empty_text_is_one_empty_row() {
        assert_eq!(
            wrap("", 10),
            vec![Row {
                range: 0..0,
                width: 0
            }]
        );
    }

    #[test]
    fn zero_width_gives_up_rather_than_looping() {
        assert_eq!(
            wrap("anything at all", 0),
            vec![Row {
                range: 0..0,
                width: 0
            }]
        );
    }

    #[test]
    fn it_breaks_at_spaces_and_does_not_draw_the_break() {
        let t = "the quick brown fox";
        let rows = wrap(t, 10);
        assert_eq!(drawn(t, &rows), vec!["the quick", "brown fox"]);
        // The break space is still inside a range, so the bytes tile.
        assert_eq!(rows[0].slice(t), "the quick ");
        assert_eq!(rows[0].width, 9);
    }

    #[test]
    fn newlines_always_break() {
        let t = "a\nb\nc";
        assert_eq!(drawn(t, &wrap(t, 40)), vec!["a", "b", "c"]);
    }

    #[test]
    fn a_trailing_newline_leaves_the_blank_line_it_made() {
        let t = "a\n";
        let rows = wrap(t, 10);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].range, 2..2);
    }

    #[test]
    fn a_word_wider_than_the_row_is_cut_between_clusters() {
        let t = "supercalifragilistic";
        assert_eq!(
            drawn(t, &wrap(t, 6)),
            vec!["superc", "alifra", "gilist", "ic"]
        );
    }

    #[test]
    fn a_long_word_after_a_short_one_starts_on_its_own_row() {
        let t = "hi supercalifragilistic";
        assert_eq!(
            drawn(t, &wrap(t, 6)),
            vec!["hi", "superc", "alifra", "gilist", "ic"]
        );
    }

    #[test]
    fn wide_characters_never_straddle_a_row() {
        // Five columns, characters two wide: two per row and one column
        // wasted, which is the only correct answer.
        let t = "君の名は。";
        let rows = wrap(t, 5);
        assert_eq!(drawn(t, &rows), vec!["君の", "名は", "。"]);
        for r in &rows {
            assert!(r.width <= 5);
        }
    }

    #[test]
    fn a_cluster_wider_than_the_row_gets_the_row_to_itself() {
        let t = "君の";
        let rows = wrap(t, 1);
        assert_eq!(drawn(t, &rows), vec!["君", "の"]);
        assert_eq!(rows[0].width, 2, "it overflows, and says so");
    }

    #[test]
    fn combining_marks_cost_nothing_and_stay_with_their_base() {
        // A combining acute over each letter: five columns, ten characters.
        let t = "a\u{301}e\u{301}i\u{301}o\u{301}u\u{301}";
        let rows = wrap(t, 5);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].width, 5);

        // And at four columns the break lands before a base, never between a
        // base and its accent.
        let rows = wrap(t, 4);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].slice(t), "a\u{301}e\u{301}i\u{301}o\u{301}");
    }

    #[test]
    fn a_zwj_sequence_is_one_cluster_two_columns_wide() {
        let t = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}";
        assert_eq!(clusters(t).count(), 1);
        let rows = wrap(t, 40);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].width, 2, "not the six its characters add up to");
    }

    #[test]
    fn an_emoji_with_a_skin_tone_is_one_cluster() {
        let t = "\u{1f44b}\u{1f3fd}";
        assert_eq!(clusters(t).count(), 1);
        assert_eq!(width_of(t), 2);
    }

    #[test]
    fn runs_of_spaces_hang_off_the_edge() {
        let t = "ab    cd";
        let rows = wrap(t, 4);
        assert_eq!(drawn(t, &rows), vec!["ab", "cd"]);
        assert_eq!(rows[0].slice(t), "ab    ");
    }

    #[test]
    fn leading_spaces_are_kept_because_they_are_indentation() {
        let t = "    indented";
        let rows = wrap(t, 20);
        assert_eq!(rows[0].width, 12);
    }

    #[test]
    fn boundaries_step_over_whole_clusters_in_both_directions() {
        let t = "a\u{301}\u{1f468}\u{200d}\u{1f469}z";
        let mut fwd = vec![0];
        let mut at = 0;
        while at < t.len() {
            at = next_boundary(t, at);
            fwd.push(at);
        }
        let mut back = vec![t.len()];
        let mut at = t.len();
        while at > 0 {
            at = prev_boundary(t, at);
            back.push(at);
        }
        back.reverse();
        assert_eq!(fwd, back);
        assert_eq!(fwd.len(), 4, "three clusters");
    }

    fn check_invariants(text: &str, width: u16) {
        let rows = wrap(text, width);
        assert!(!rows.is_empty());

        // Every byte exactly once, in order.
        let mut at = 0usize;
        for r in &rows {
            assert_eq!(r.range.start, at, "rows do not tile {text:?}");
            assert!(r.range.end >= r.range.start);
            at = r.range.end;
        }
        assert_eq!(at, text.len(), "rows do not cover {text:?}");

        for r in &rows {
            // Never inside a character.
            assert!(text.is_char_boundary(r.range.start));
            assert!(text.is_char_boundary(r.range.end));
            let d = r.drawn(text);
            // The recorded width is the width of what is drawn.
            assert_eq!(r.width, width_of(d), "wrong width for {d:?}");
            // And it fits, unless one cluster alone does not.
            assert!(
                r.width <= width || clusters(d).count() == 1,
                "row {d:?} is {} wide at width {width}",
                r.width
            );
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(400))]

        #[test]
        fn rows_tile_the_input_and_fit_the_width(text in "(?s).{0,200}", width in 1u16..40) {
            check_invariants(&text, width);
        }

        #[test]
        fn the_same_holds_for_the_characters_that_break_wrapping(
            text in proptest::collection::vec(
                proptest::sample::select(vec![
                    'a', ' ', ' ', '\n', '字', '\u{301}', '\u{200d}',
                    '\u{1f600}', '\u{1f3fd}', '\t', '\u{200b}', '\u{fe0f}', 'é',
                ]),
                0..60,
            ),
            width in 1u16..20,
        ) {
            let text: String = text.into_iter().collect();
            check_invariants(&text, width);
        }
    }

    #[test]
    fn the_cache_answers_the_second_time_without_building() {
        let mut c: WrapCache<u32> = WrapCache::new(8, 1 << 20);
        let rows = c.rows(1, 10, || wrap("hello there", 10)).to_vec();
        let again = c.rows(1, 10, || panic!("should not rebuild")).to_vec();
        assert_eq!(rows, again);
    }

    #[test]
    fn one_key_at_two_widths_is_two_entries() {
        let mut c: WrapCache<u32> = WrapCache::new(8, 1 << 20);
        c.rows(1, 10, || wrap("hello there", 10));
        c.rows(1, 20, || wrap("hello there", 20));
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn invalidating_a_key_drops_every_width_of_it() {
        let mut c: WrapCache<u32> = WrapCache::new(8, 1 << 20);
        c.rows(1, 10, || wrap("a", 10));
        c.rows(1, 20, || wrap("a", 20));
        c.rows(2, 10, || wrap("b", 10));
        c.invalidate(&1);
        assert_eq!(c.len(), 1);
    }

    #[test]
    fn the_entry_cap_is_never_exceeded() {
        let mut c: WrapCache<u32> = WrapCache::new(4, 1 << 20);
        for i in 0..40u32 {
            c.rows(i, 10, || wrap("some text to wrap here", 10));
            assert!(c.len() <= 4, "{} entries", c.len());
        }
    }

    #[test]
    fn the_byte_cap_is_never_exceeded() {
        let mut c: WrapCache<u32> = WrapCache::new(1000, 512);
        for i in 0..40u32 {
            c.rows(i, 4, || {
                wrap("a rather long string that makes many rows", 4)
            });
            assert!(c.bytes() <= 512 || c.len() == 1, "{} bytes", c.bytes());
        }
    }

    #[test]
    fn the_least_recently_used_entry_is_the_one_that_goes() {
        let mut c: WrapCache<u32> = WrapCache::new(2, 1 << 20);
        c.rows(1, 10, || wrap("one", 10));
        c.rows(2, 10, || wrap("two", 10));
        c.rows(1, 10, || wrap("one", 10)); // touch 1, so 2 is the old one
        c.rows(3, 10, || wrap("three", 10));
        let mut rebuilt = false;
        c.rows(1, 10, || {
            rebuilt = true;
            wrap("one", 10)
        });
        assert!(!rebuilt, "the recently used entry was evicted");
    }
}
