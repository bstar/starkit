//! Colour, and the perceptual maths the derivation chain needs.
//!
//! Blending in sRGB gives muddy midpoints — a mix of a dark background and a
//! bright accent comes out darker than it looks like it should. Oklab is
//! perceptually uniform, so a 30% mix actually reads as 30% of the way there,
//! which is what makes the "omit a role and let it be derived" design produce
//! usable colours rather than approximate ones.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    pub fn parse_hex(s: &str) -> Result<Self, String> {
        let h = s.trim().trim_start_matches('#');
        // ASCII first, and not as a formality: the length checks below count
        // bytes and the slices index them, so six bytes of multi-byte
        // characters pass for `#RRGGBB` and then split one down the middle.
        // A colour arrives from a theme file or from a downloaded skin's
        // PLEDIT.TXT, so that is a panic somebody else's file can cause.
        if !h.is_ascii() {
            return Err(format!("expected #RRGGBB or #RGB, got {s:?}"));
        }
        let val = |i: usize| -> Result<u8, String> {
            u8::from_str_radix(&h[i..i + 2], 16).map_err(|_| format!("bad hex: {s}"))
        };
        match h.len() {
            6 => Ok(Rgb::new(val(0)?, val(2)?, val(4)?)),
            // `#abc` shorthand, expanded the CSS way.
            3 => {
                let d = |i: usize| -> Result<u8, String> {
                    let v = u8::from_str_radix(&h[i..i + 1], 16)
                        .map_err(|_| format!("bad hex: {s}"))?;
                    Ok(v * 17)
                };
                Ok(Rgb::new(d(0)?, d(1)?, d(2)?))
            }
            _ => Err(format!("expected #RRGGBB or #RGB, got {s:?}")),
        }
    }

    pub fn to_hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }

    /// WCAG relative luminance.
    pub fn luminance(self) -> f64 {
        fn lin(c: u8) -> f64 {
            let c = c as f64 / 255.0;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        }
        0.2126 * lin(self.r) + 0.7152 * lin(self.g) + 0.0722 * lin(self.b)
    }

    /// WCAG contrast ratio, 1.0 (identical) to 21.0 (black on white).
    pub fn contrast(self, other: Rgb) -> f64 {
        let (a, b) = (self.luminance(), other.luminance());
        let (hi, lo) = if a > b { (a, b) } else { (b, a) };
        (hi + 0.05) / (lo + 0.05)
    }

    /// Whichever candidate reads best against this background.
    pub fn best_contrast_against(self, candidates: &[Rgb]) -> Rgb {
        candidates
            .iter()
            .copied()
            .max_by(|a, b| {
                self.contrast(*a)
                    .partial_cmp(&self.contrast(*b))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap_or(Rgb::new(255, 255, 255))
    }

    pub fn to_oklab(self) -> Oklab {
        fn lin(c: u8) -> f64 {
            let c = c as f64 / 255.0;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        }
        let (r, g, b) = (lin(self.r), lin(self.g), lin(self.b));

        let l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b).cbrt();
        let m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b).cbrt();
        let s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b).cbrt();

        Oklab {
            l: 0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s,
            a: 1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s,
            b: 0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s,
        }
    }

    /// Perceptual blend. `t = 0` is `self`, `t = 1` is `other`.
    pub fn mix(self, other: Rgb, t: f64) -> Rgb {
        let (a, b) = (self.to_oklab(), other.to_oklab());
        let t = t.clamp(0.0, 1.0);
        Oklab {
            l: a.l + (b.l - a.l) * t,
            a: a.a + (b.a - a.a) * t,
            b: a.b + (b.b - a.b) * t,
        }
        .to_rgb()
    }

    /// Move toward white perceptually.
    pub fn lighten(self, amount: f64) -> Rgb {
        self.mix(Rgb::new(255, 255, 255), amount)
    }

    pub fn darken(self, amount: f64) -> Rgb {
        self.mix(Rgb::new(0, 0, 0), amount)
    }

    pub fn is_dark(self) -> bool {
        self.luminance() < 0.18
    }

    /// Pull this colour back toward `bg` until it is no louder than `max`.
    ///
    /// The counterpart to `ensure_contrast`, for things that must stay quiet.
    /// Chrome is the case: a focused border derived straight from the accent
    /// reads as a highlight rather than a frame, and on a scheme whose accent
    /// is a saturated green on black it is glaring.
    pub fn limit_contrast(self, bg: Rgb, max: f64) -> Rgb {
        if bg.contrast(self) <= max {
            return self;
        }
        let mut best = self;
        for i in 1..=16 {
            let candidate = self.mix(bg, i as f64 / 16.0);
            best = candidate;
            if bg.contrast(candidate) <= max {
                break;
            }
        }
        best
    }

    /// Nudge this colour away from `bg` until it clears `target` contrast.
    ///
    /// base16's base03 is specified as a comment/border colour, and in plenty
    /// of real schemes it is far too dark to read as text -- COSMIC's #5A5A5A
    /// manages only 2.50:1 on its own background. `dim` carries hints, indices
    /// and timestamps, so it is text and has to be legible. Mixing toward
    /// whichever of black or white is further from the background preserves the
    /// hue rather than replacing the colour outright.
    pub fn ensure_contrast(self, bg: Rgb, target: f64) -> Rgb {
        if bg.contrast(self) >= target {
            return self;
        }
        let toward = if bg.is_dark() {
            Rgb::new(255, 255, 255)
        } else {
            Rgb::new(0, 0, 0)
        };
        let mut best = self;
        // Sixteen steps is finer than the eye resolves and always terminates.
        for i in 1..=16 {
            let candidate = self.mix(toward, i as f64 / 16.0);
            best = candidate;
            if bg.contrast(candidate) >= target {
                break;
            }
        }
        best
    }
}

impl std::fmt::Display for Rgb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl TryFrom<String> for Rgb {
    type Error = String;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Rgb::parse_hex(&s)
    }
}

impl From<Rgb> for String {
    fn from(c: Rgb) -> String {
        c.to_hex()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Oklab {
    pub l: f64,
    pub a: f64,
    pub b: f64,
}

impl Oklab {
    pub fn to_rgb(self) -> Rgb {
        let l_ = self.l + 0.3963377774 * self.a + 0.2158037573 * self.b;
        let m_ = self.l - 0.1055613458 * self.a - 0.0638541728 * self.b;
        let s_ = self.l - 0.0894841775 * self.a - 1.2914855480 * self.b;

        let (l, m, s) = (l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_);

        let r = 4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s;
        let g = -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s;
        let b = -0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s;

        fn enc(c: f64) -> u8 {
            let c = if c <= 0.0031308 {
                12.92 * c
            } else {
                1.055 * c.powf(1.0 / 2.4) - 0.055
            };
            (c.clamp(0.0, 1.0) * 255.0).round() as u8
        }
        Rgb::new(enc(r), enc(g), enc(b))
    }
}

/// Interpolate a ramp of `n` stops through a set of control colours.
///
/// This is how a 16-step spectrum ramp is generated from four theme colours when
/// a theme does not supply its own.
pub fn ramp(stops: &[Rgb], n: usize) -> Vec<Rgb> {
    if stops.is_empty() || n == 0 {
        return Vec::new();
    }
    if stops.len() == 1 {
        return vec![stops[0]; n];
    }
    (0..n)
        .map(|i| {
            let t = i as f64 / (n - 1).max(1) as f64;
            let scaled = t * (stops.len() - 1) as f64;
            let idx = (scaled.floor() as usize).min(stops.len() - 2);
            stops[idx].mix(stops[idx + 1], scaled - idx as f64)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_in_both_lengths() {
        assert_eq!(Rgb::parse_hex("#00FF00").unwrap(), Rgb::new(0, 255, 0));
        assert_eq!(Rgb::parse_hex("00ff00").unwrap(), Rgb::new(0, 255, 0));
        assert_eq!(Rgb::parse_hex("#0f0").unwrap(), Rgb::new(0, 255, 0));
        assert!(Rgb::parse_hex("#xyz").is_err());
        assert!(Rgb::parse_hex("#12345").is_err());
    }

    #[test]
    fn hex_round_trips() {
        for c in ["#000000", "#ffffff", "#29ce10", "#d6b521"] {
            assert_eq!(Rgb::parse_hex(c).unwrap().to_hex(), c);
        }
    }

    #[test]
    fn contrast_matches_the_wcag_extremes() {
        let black = Rgb::new(0, 0, 0);
        let white = Rgb::new(255, 255, 255);
        assert!((black.contrast(white) - 21.0).abs() < 0.01);
        assert!((white.contrast(white) - 1.0).abs() < 0.001);
    }

    #[test]
    fn oklab_round_trips_within_rounding() {
        for c in [
            Rgb::new(0, 0, 0),
            Rgb::new(255, 255, 255),
            Rgb::new(41, 206, 16),
            Rgb::new(30, 30, 46),
        ] {
            let back = c.to_oklab().to_rgb();
            for (a, b) in [(c.r, back.r), (c.g, back.g), (c.b, back.b)] {
                assert!((a as i32 - b as i32).abs() <= 1, "{c} -> {back}");
            }
        }
    }

    #[test]
    fn mixing_hits_the_endpoints_exactly() {
        let a = Rgb::new(0, 0, 0);
        let b = Rgb::new(255, 255, 255);
        assert_eq!(a.mix(b, 0.0), a);
        assert_eq!(a.mix(b, 1.0), b);
    }

    #[test]
    fn a_perceptual_midpoint_is_not_the_naive_average() {
        // The reason for Oklab at all. sRGB's "50% grey" #808080 sits at Oklab
        // L = 0.60, i.e. it is perceptually brighter than the true middle, so
        // channel-averaging overshoots. The perceptual midpoint of black and
        // white is around #636363.
        let mid = Rgb::new(0, 0, 0).mix(Rgb::new(255, 255, 255), 0.5);
        assert!(
            (90..=108).contains(&mid.r),
            "expected a perceptual mid-grey near #636363, got {mid}"
        );
        assert!(
            mid.r < 128,
            "channel-averaging would give 128; Oklab should not"
        );
        // Midpoint lightness really is halfway in Oklab terms.
        assert!((mid.to_oklab().l - 0.5).abs() < 0.02);
    }

    #[test]
    fn a_thirty_percent_mix_is_perceptually_thirty_percent() {
        // This is what makes "omit a role and derive it" produce usable colours:
        // a 30% mix toward the accent actually reads as 30% of the way there.
        let a = Rgb::new(0, 0, 0);
        let b = Rgb::new(255, 255, 255);
        let m = a.mix(b, 0.3);
        assert!(
            (m.to_oklab().l - 0.3).abs() < 0.02,
            "got L {}",
            m.to_oklab().l
        );
    }

    #[test]
    fn best_contrast_picks_the_readable_option() {
        let dark = Rgb::new(0, 0, 0);
        let choices = [Rgb::new(0, 0, 0), Rgb::new(255, 255, 255)];
        assert_eq!(
            dark.best_contrast_against(&choices),
            Rgb::new(255, 255, 255)
        );

        let light = Rgb::new(255, 255, 255);
        assert_eq!(light.best_contrast_against(&choices), Rgb::new(0, 0, 0));
    }

    #[test]
    fn ensure_contrast_lifts_a_too_dark_colour() {
        // COSMIC's base03 on its base00: 2.50:1, not readable as text.
        let bg = Rgb::parse_hex("#1b1b1b").unwrap();
        let base03 = Rgb::parse_hex("#5a5a5a").unwrap();
        assert!(bg.contrast(base03) < 4.5);

        let lifted = base03.ensure_contrast(bg, 4.5);
        assert!(
            bg.contrast(lifted) >= 4.5,
            "still only {:.2}:1",
            bg.contrast(lifted)
        );
        assert!(
            lifted.luminance() > base03.luminance(),
            "should get lighter"
        );
    }

    #[test]
    fn ensure_contrast_leaves_an_already_readable_colour_alone() {
        let bg = Rgb::parse_hex("#1b1b1b").unwrap();
        let fg = Rgb::parse_hex("#c4c4c4").unwrap();
        assert_eq!(fg.ensure_contrast(bg, 4.5), fg);
    }

    #[test]
    fn ensure_contrast_darkens_against_a_light_background() {
        let bg = Rgb::parse_hex("#eff1f5").unwrap();
        let pale = Rgb::parse_hex("#d0d0d0").unwrap();
        let fixed = pale.ensure_contrast(bg, 4.5);
        assert!(bg.contrast(fixed) >= 4.5);
        assert!(fixed.luminance() < pale.luminance(), "should get darker");
    }

    #[test]
    fn ensure_contrast_terminates_even_when_the_target_is_impossible() {
        let bg = Rgb::new(128, 128, 128);
        // 21:1 cannot be reached against mid grey; it must still return.
        let out = Rgb::new(130, 130, 130).ensure_contrast(bg, 21.0);
        assert!(out.luminance().is_finite());
    }

    #[test]
    fn limit_contrast_quietens_a_loud_colour() {
        let bg = Rgb::parse_hex("#000000").unwrap();
        let loud = Rgb::parse_hex("#00ff00").unwrap();
        assert!(bg.contrast(loud) > 10.0);
        let quiet = loud.limit_contrast(bg, 3.2);
        assert!(
            bg.contrast(quiet) <= 3.2,
            "still {:.2}:1",
            bg.contrast(quiet)
        );
    }

    #[test]
    fn limit_contrast_leaves_an_already_quiet_colour_alone() {
        let bg = Rgb::parse_hex("#1b1b1b").unwrap();
        let quiet = Rgb::parse_hex("#3b3b3b").unwrap();
        assert_eq!(quiet.limit_contrast(bg, 3.2), quiet);
    }

    #[test]
    fn ramps_have_the_requested_length_and_endpoints() {
        let stops = [
            Rgb::new(0x21, 0x8c, 0x00),
            Rgb::new(0xd6, 0xb5, 0x21),
            Rgb::new(0xef, 0x31, 0x10),
        ];
        let r = ramp(&stops, 16);
        assert_eq!(r.len(), 16);
        assert_eq!(r[0], stops[0]);
        assert_eq!(r[15], stops[2]);
    }

    #[test]
    fn ramps_progress_monotonically_in_lightness_for_a_dark_to_light_pair() {
        let r = ramp(&[Rgb::new(0, 0, 0), Rgb::new(255, 255, 255)], 16);
        for w in r.windows(2) {
            assert!(w[1].luminance() >= w[0].luminance());
        }
    }

    /// Found by the skin parser's property test on its first CI run, which is
    /// the entire reason those exist. The length check counts bytes and the
    /// slices index them, so six bytes of multi-byte characters looked like
    /// `#RRGGBB` and then split a character in half.
    #[test]
    fn a_colour_that_is_not_ascii_is_refused_rather_than_panicking() {
        for s in [
            "#\u{25141}\u{e9}", // four bytes plus two: len() == 6
            "\u{25141}\u{e9}",
            "#\u{128f1}\u{e9}",
            "#\u{e9}\u{e9}\u{e9}", // two bytes each: len() == 6
            "#\u{e9}\u{e9}",       // len() == 4
            "#\u{1f600}",          // len() == 4
            "#ff\u{e9}\u{e9}",     // mixed: len() == 6
        ] {
            assert!(Rgb::parse_hex(s).is_err(), "{s:?} was accepted as a colour");
        }
        // And ordinary colours still parse, in both lengths.
        assert_eq!(
            Rgb::parse_hex("#1a2b3c").unwrap(),
            Rgb::new(0x1a, 0x2b, 0x3c)
        );
        assert_eq!(
            Rgb::parse_hex("1a2b3c").unwrap(),
            Rgb::new(0x1a, 0x2b, 0x3c)
        );
        assert_eq!(Rgb::parse_hex("#abc").unwrap(), Rgb::new(0xaa, 0xbb, 0xcc));
        assert_eq!(
            Rgb::parse_hex("  #ABC  ").unwrap(),
            Rgb::new(0xaa, 0xbb, 0xcc)
        );
    }
}
