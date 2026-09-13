//! Logo component — renders the welcome-screen art logo.
//!
//! Default art is Grok's braille logo. External ACP hosts (e.g. `grok-pi`)
//! may install a process-wide override via [`set_logo_override`] so the same
//! shimmer renderer paints their brand without forking the welcome layout.
//!
//! Default braille art is hidden entirely on legacy Windows consoles: the
//! U+2800 braille block is not covered by the ConHost raster fonts and would
//! render as tofu. An installed override is shown even there — the host is
//! responsible for supplying displayable glyphs (e.g. block-character art).

use std::sync::{OnceLock, RwLock};

use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use crate::acp::ExternalLogoArt;
use crate::render::color::blend_color;
use crate::theme::Theme;

const LOGO: &str = include_str!("../../../assets/logo/logo07.txt");
const LOGO_SMALL: &str = include_str!("../../../assets/logo/logo05.txt");

/// Height at or above which the small logo is shown (below it, no logo).
const SMALL_LOGO_MIN_HEIGHT: u16 = 22;
/// Height at or above which the full logo is shown.
const FULL_LOGO_MIN_HEIGHT: u16 = 26;

/// Which logo art the stacked column shows.
/// The terminal height picks the tier; the stacked layout steps it down only while the column would not fit beside the draft.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogoTier {
    Full,
    Compact,
    Hidden,
}

impl LogoTier {
    pub fn for_height(window_height: u16) -> Self {
        Self::for_height_and_hidden(window_height, logo_hidden())
    }

    /// Takes the legacy-console flag as a parameter so tests can drive it directly.
    fn for_height_and_hidden(window_height: u16, hidden: bool) -> Self {
        if hidden || window_height < SMALL_LOGO_MIN_HEIGHT {
            Self::Hidden
        } else if window_height < FULL_LOGO_MIN_HEIGHT {
            Self::Compact
        } else {
            Self::Full
        }
    }

    /// Host overrides replace both tiers; [`logo_hidden`] is already false when one is installed.
    fn art(self) -> Option<&'static str> {
        let (full, small) = match logo_override() {
            Some(art) => (art.full, art.small),
            None => (LOGO, LOGO_SMALL),
        };
        match self {
            Self::Full => Some(full),
            Self::Compact => Some(small),
            Self::Hidden => None,
        }
    }

    pub fn rows(self) -> u16 {
        self.art().map_or(0, count_lines)
    }

    /// The next smaller tier; `None` once hidden.
    pub fn step_down(self) -> Option<Self> {
        match self {
            Self::Full => Some(Self::Compact),
            Self::Compact => Some(Self::Hidden),
            Self::Hidden => None,
        }
    }
}

fn logo_override_cell() -> &'static RwLock<Option<ExternalLogoArt>> {
    static CELL: OnceLock<RwLock<Option<ExternalLogoArt>>> = OnceLock::new();
    CELL.get_or_init(|| RwLock::new(None))
}

/// Install or clear the process-wide logo override.
///
/// Mirrors [`crate::slash::set_builtin_command_profile`]: set once before the
/// welcome screen renders so every logo call site (stacked welcome, hero box,
/// minimal compact card) picks up the same art without threading renderer
/// policy through the component tree.
pub fn set_logo_override(logo: Option<ExternalLogoArt>) {
    *logo_override_cell()
        .write()
        .expect("logo override lock poisoned") = logo;
}

fn logo_override() -> Option<ExternalLogoArt> {
    *logo_override_cell()
        .read()
        .expect("logo override lock poisoned")
}

/// Welcome-menu policy for an external host (e.g. `grok-pi`).
///
/// Kept beside the logo override so composition binaries can brand the
/// welcome card without forking the menu renderer: hide Grok product rows,
/// and redirect Changelog to a project URL.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExternalWelcomeMenu {
    pub hide_new_worktree: bool,
    pub changelog_url: Option<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExternalWelcomeBrand {
    pub title: &'static str,
    pub subtitle: &'static str,
    pub version: &'static str,
}

fn welcome_menu_cell() -> &'static RwLock<Option<ExternalWelcomeMenu>> {
    static CELL: OnceLock<RwLock<Option<ExternalWelcomeMenu>>> = OnceLock::new();
    CELL.get_or_init(|| RwLock::new(None))
}

/// Install or clear process-wide welcome menu policy for external hosts.
pub fn set_welcome_menu_override(menu: Option<ExternalWelcomeMenu>) {
    *welcome_menu_cell()
        .write()
        .expect("welcome menu override lock poisoned") = menu;
}

/// Current welcome menu override, if any.
pub fn welcome_menu_override() -> Option<ExternalWelcomeMenu> {
    *welcome_menu_cell()
        .read()
        .expect("welcome menu override lock poisoned")
}

fn welcome_brand_cell() -> &'static RwLock<Option<ExternalWelcomeBrand>> {
    static CELL: OnceLock<RwLock<Option<ExternalWelcomeBrand>>> = OnceLock::new();
    CELL.get_or_init(|| RwLock::new(None))
}

pub fn set_welcome_brand_override(brand: Option<ExternalWelcomeBrand>) {
    *welcome_brand_cell()
        .write()
        .expect("welcome brand override lock poisoned") = brand;
}

pub fn welcome_brand_override() -> Option<ExternalWelcomeBrand> {
    *welcome_brand_cell()
        .read()
        .expect("welcome brand override lock poisoned")
}

fn pick_logo(window_height: u16) -> Option<&'static str> {
    pick_logo_for(window_height, logo_hidden(), logo_override())
}

/// Pure tier selection so tests can drive the legacy-console flag and override
/// art directly without racing the process-wide cell.
fn pick_logo_for(
    window_height: u16,
    hidden: bool,
    override_art: Option<ExternalLogoArt>,
) -> Option<&'static str> {
    let (full, small) = match override_art {
        Some(art) => (art.full, art.small),
        None => {
            if hidden {
                return None;
            }
            (LOGO, LOGO_SMALL)
        }
    };
    if window_height < SMALL_LOGO_MIN_HEIGHT {
        None
    } else if window_height < FULL_LOGO_MIN_HEIGHT {
        Some(small)
    } else {
        Some(full)
    }
}

/// Default braille art has no legacy-safe stand-in; see the module doc.
/// Overrides are never auto-hidden.
fn logo_hidden() -> bool {
    logo_override().is_none() && crate::glyphs::is_legacy_windows_console()
}

fn non_empty_lines(logo: &str) -> impl Iterator<Item = &str> {
    logo.lines().filter(|l| !l.is_empty())
}

fn count_lines(logo: &str) -> u16 {
    non_empty_lines(logo).count() as u16
}

fn visual_width(logo: &str) -> u16 {
    non_empty_lines(logo)
        .map(unicode_width::UnicodeWidthStr::width)
        .max()
        .unwrap_or(24) as u16
}

/// Animation phase in seconds since the first render. Wall-clock based so the
/// shimmer speed is independent of the frame rate.
fn anim_phase_secs() -> f32 {
    use std::sync::OnceLock;
    use std::time::Instant;
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f32()
}

/// Shimmer redraw cadence in frames per second. The sweep is slow, so a few fps
/// looks smooth while sparing the long-lived welcome screen from full-rate
/// repaints.
const SHIMMER_FPS: f32 = 12.0;

/// Quantized shimmer frame for the current wall-clock phase. The welcome screen
/// redraws only when this advances, throttling the animation to ~`SHIMMER_FPS`
/// rather than the full event-loop tick rate. Pinned to 0 when the logo is
/// hidden.
pub fn shimmer_frame() -> u64 {
    if logo_hidden() {
        return 0;
    }
    (anim_phase_secs() * SHIMMER_FPS) as u64
}

/// Per-glyph shine opacity in `[0, 1]` at normalized diagonal position `diag`
/// (0 = bottom-left .. 1 = top-right) and animation time `secs`. A raised-cosine
/// band sweeps bottom-left → top-right and parks off-screen between sweeps; a
/// gentle global pulse breathes underneath it. 0 keeps the resting gray, 1 is
/// full bright.
fn shine_opacity(diag: f32, secs: f32) -> f32 {
    const BAND: f32 = 0.38; // half-width of the shine band — wider = more gradual falloff
    const CYCLE: f32 = 4.0; // seconds per sweep + rest
    const SWEEP_FRAC: f32 = 0.32; // portion of the cycle spent sweeping (~1.3s glint, rest idles)
    const SHINE: f32 = 0.33; // peak shine strength
    const PULSE: f32 = 0.06; // global breathing amount
    const PULSE_SECS: f32 = 5.0; // breathing period

    let p = (secs % CYCLE) / CYCLE;
    let q = (p / SWEEP_FRAC).min(1.0); // parks the band off-screen during the rest
    let band_pos = -BAND + q * (1.0 + 2.0 * BAND);
    let pulse = PULSE * (0.5 - 0.5 * (std::f32::consts::TAU * secs / PULSE_SECS).cos());

    let d = (diag - band_pos).abs();
    let shine = if d < BAND {
        0.5 * (1.0 + (std::f32::consts::PI * d / BAND).cos())
    } else {
        0.0
    };
    (pulse + SHINE * shine).clamp(0.0, 1.0)
}

fn render_into(area: Rect, buf: &mut Buffer, theme: &Theme, logo: &str) {
    let lines: Vec<&str> = non_empty_lines(logo).collect();
    let rows = lines.len().max(1) as f32;
    // Pad every row to the same visual width so per-line `Alignment::Center`
    // keeps the glyph columns locked together. Uneven art (e.g. Pi's block π)
    // otherwise drifts row-by-row when shorter lines are centered independently.
    let max_width = lines
        .iter()
        .map(|l| unicode_width::UnicodeWidthStr::width(*l))
        .max()
        .unwrap_or(1)
        .max(1);
    let cols = max_width as f32;
    let secs = anim_phase_secs();

    // Blend each glyph from the resting gray toward the bright text color by its
    // shine opacity, so a sheen sweeps across the braille art. Adjacent glyphs
    // that land on the same blended color share one Span to hold down the
    // per-frame allocation.
    let base = theme.gray;
    let hilite = theme.text_primary;
    let logo_lines: Vec<Line> = lines
        .iter()
        .enumerate()
        .map(|(row, line)| {
            let mut spans: Vec<Span> = Vec::new();
            let mut run = String::new();
            let mut run_color: Option<Color> = None;
            let mut col = 0usize;
            for ch in line.chars() {
                // Sweep along the bottom-left → top-right diagonal: the
                // coordinate grows as col increases and row decreases.
                let diag = (col as f32 + (rows - 1.0 - row as f32)) / (cols + rows);
                let color = blend_color(base, hilite, shine_opacity(diag, secs)).unwrap_or(base);
                if run_color != Some(color) {
                    if let Some(prev) = run_color {
                        spans.push(Span::styled(
                            std::mem::take(&mut run),
                            Style::default().fg(prev),
                        ));
                    }
                    run_color = Some(color);
                }
                run.push(ch);
                col += unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
            }
            // Trailing pad uses resting gray so shorter rows stay left-aligned
            // within the centered block without inventing extra shine samples.
            let pad = max_width.saturating_sub(unicode_width::UnicodeWidthStr::width(*line));
            if pad > 0 {
                if run_color != Some(base) {
                    if let Some(prev) = run_color {
                        spans.push(Span::styled(
                            std::mem::take(&mut run),
                            Style::default().fg(prev),
                        ));
                    }
                    run_color = Some(base);
                }
                run.push_str(&" ".repeat(pad));
            }
            if let Some(prev) = run_color {
                spans.push(Span::styled(run, Style::default().fg(prev)));
            }
            Line::from(spans).alignment(Alignment::Center)
        })
        .collect();
    Paragraph::new(logo_lines).render(area, buf);
}

pub fn logo_line_count(window_height: u16) -> u16 {
    pick_logo(window_height).map_or(0, count_lines)
}

pub fn logo_visual_width(window_height: u16) -> u16 {
    pick_logo(window_height).map_or(24, visual_width)
}

pub fn render_logo(area: Rect, buf: &mut Buffer, theme: &Theme, window_height: u16) {
    if let Some(logo) = pick_logo(window_height) {
        render_into(area, buf, theme, logo);
    }
}

/// Paint the tier the layout reserved rows for, so the art can never outgrow its slot.
pub fn render_logo_tier(area: Rect, buf: &mut Buffer, theme: &Theme, tier: LogoTier) {
    if let Some(logo) = tier.art() {
        render_into(area, buf, theme, logo);
    }
}

/// The hero box always shows the full logo: it is laid out beside the menu, so
/// it fits whenever the box does. These report and render that logo directly,
/// independent of the height-based [`pick_logo`] tiers used by the stacked
/// layout. When [`logo_hidden`], they report 0 and render nothing.
pub fn full_logo_line_count() -> u16 {
    full_logo_line_count_for(logo_hidden(), logo_override())
}

fn full_logo_line_count_for(hidden: bool, override_art: Option<ExternalLogoArt>) -> u16 {
    match override_art {
        Some(art) => count_lines(art.full),
        None if hidden => 0,
        None => count_lines(LOGO),
    }
}

pub fn full_logo_visual_width() -> u16 {
    full_logo_visual_width_for(logo_hidden(), logo_override())
}

fn full_logo_visual_width_for(hidden: bool, override_art: Option<ExternalLogoArt>) -> u16 {
    match override_art {
        Some(art) => visual_width(art.full),
        None if hidden => 0,
        None => visual_width(LOGO),
    }
}

pub fn render_full_logo(area: Rect, buf: &mut Buffer, theme: &Theme) {
    if let Some(logo) = full_logo_art(logo_hidden(), logo_override()) {
        render_into(area, buf, theme, logo);
    }
}

fn full_logo_art(hidden: bool, override_art: Option<ExternalLogoArt>) -> Option<&'static str> {
    match override_art {
        Some(art) => Some(art.full),
        None if hidden => None,
        None => Some(LOGO),
    }
}

/// Line count of the small logo used in minimal's committed welcome card
/// (0 on a legacy Windows console, where the default braille art is suppressed).
pub fn compact_logo_line_count() -> u16 {
    compact_logo_line_count_for(logo_hidden(), logo_override())
}

fn compact_logo_line_count_for(hidden: bool, override_art: Option<ExternalLogoArt>) -> u16 {
    match override_art {
        Some(art) => count_lines(art.small),
        None if hidden => 0,
        None => count_lines(LOGO_SMALL),
    }
}

/// Render the small logo (centered) into `area` for minimal's welcome card.
/// No-op when the logo is hidden.
pub fn render_compact_logo(area: Rect, buf: &mut Buffer, theme: &Theme) {
    if let Some(logo) = compact_logo_art(logo_hidden(), logo_override()) {
        render_into(area, buf, theme, logo);
    }
}

fn compact_logo_art(hidden: bool, override_art: Option<ExternalLogoArt>) -> Option<&'static str> {
    match override_art {
        Some(art) => Some(art.small),
        None if hidden => None,
        None => Some(LOGO_SMALL),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logo_sizes_by_height() {
        assert!(pick_logo_for(SMALL_LOGO_MIN_HEIGHT - 1, false, None).is_none());
        assert_eq!(
            pick_logo_for(SMALL_LOGO_MIN_HEIGHT, false, None),
            Some(LOGO_SMALL)
        );
        assert_eq!(
            pick_logo_for(FULL_LOGO_MIN_HEIGHT - 1, false, None),
            Some(LOGO_SMALL)
        );
        assert_eq!(pick_logo_for(FULL_LOGO_MIN_HEIGHT, false, None), Some(LOGO));
    }

    // The braille art has no legacy-safe stand-in, so every height tier must
    // collapse to no logo when the legacy-console flag is set.
    #[test]
    fn logo_hidden_on_legacy_console_at_every_height() {
        for h in [0, SMALL_LOGO_MIN_HEIGHT, FULL_LOGO_MIN_HEIGHT, u16::MAX] {
            assert!(pick_logo_for(h, true, None).is_none(), "height {h}");
        }
    }

    #[test]
    fn hero_box_always_uses_full_logo() {
        // The box renders the full logo regardless of height (it's laid out
        // beside the menu), and it's the large variant — never the small one.
        assert_eq!(full_logo_line_count_for(false, None), count_lines(LOGO));
        assert_eq!(full_logo_visual_width_for(false, None), visual_width(LOGO));
        assert!(full_logo_line_count_for(false, None) > count_lines(LOGO_SMALL));
        assert!(full_logo_visual_width_for(false, None) > visual_width(LOGO_SMALL));
    }

    #[test]
    fn full_logo_helpers_collapse_when_hidden() {
        assert_eq!(full_logo_line_count_for(true, None), 0);
        assert_eq!(full_logo_visual_width_for(true, None), 0);
    }

    #[test]
    fn compact_logo_line_count_matches_small_logo_when_visible() {
        // The minimal welcome card budgets exactly the small logo's rows. When
        // the logo isn't hidden, the count equals the small art's line count and
        // is strictly shorter than the full logo.
        if !logo_hidden() {
            assert_eq!(compact_logo_line_count(), count_lines(LOGO_SMALL));
            assert!(compact_logo_line_count() < count_lines(LOGO));
            assert!(compact_logo_line_count() > 0);
        } else {
            assert_eq!(compact_logo_line_count(), 0);
        }
    }

    #[test]
    fn shine_opacity_stays_in_unit_range() {
        let mut secs = 0.0;
        while secs < 10.0 {
            for i in 0..=20 {
                let diag = i as f32 / 20.0;
                let op = shine_opacity(diag, secs);
                assert!(
                    (0.0..=1.0).contains(&op),
                    "opacity {op} out of range at diag {diag}, secs {secs}"
                );
            }
            secs += 0.13;
        }
    }

    #[test]
    fn shine_band_sweeps_across() {
        // The brightest point along the diagonal advances left → right as the
        // sweep progresses through its active phase.
        let brightest = |secs: f32| -> f32 {
            (0..=100)
                .map(|i| i as f32 / 100.0)
                .max_by(|a, b| {
                    shine_opacity(*a, secs)
                        .partial_cmp(&shine_opacity(*b, secs))
                        .unwrap()
                })
                .unwrap()
        };
        let early = brightest(0.1);
        let mid = brightest(0.4);
        let late = brightest(0.7);
        assert!(early < mid, "early {early} should precede mid {mid}");
        assert!(mid < late, "mid {mid} should precede late {late}");
    }

    #[test]
    fn shine_rests_dim_between_sweeps() {
        // During the rest phase the band is parked off-screen, so an interior
        // glyph falls back to at most the gentle pulse — never full bright.
        let op = shine_opacity(0.5, 6.0); // secs % 4.0 = 2.0 → past SWEEP_FRAC, in the rest phase
        assert!(op < 0.2, "resting opacity {op} should stay dim");
    }

    #[test]
    fn logo_override_replaces_art_and_survives_legacy_hide() {
        const FULL: &str = "████\n██  ██";
        const SMALL: &str = "██";
        let art = Some(ExternalLogoArt {
            full: FULL,
            small: SMALL,
        });

        // Height tiers still apply, but the art comes from the override and the
        // legacy-console hide flag is ignored for host-supplied glyphs.
        assert_eq!(
            pick_logo_for(FULL_LOGO_MIN_HEIGHT, true, art),
            Some(FULL),
            "override must ignore the legacy-console hide flag"
        );
        assert_eq!(pick_logo_for(SMALL_LOGO_MIN_HEIGHT, true, art), Some(SMALL));
        assert!(pick_logo_for(SMALL_LOGO_MIN_HEIGHT - 1, true, art).is_none());

        assert_eq!(full_logo_line_count_for(true, art), count_lines(FULL));
        assert_eq!(full_logo_visual_width_for(true, art), visual_width(FULL));
        assert_eq!(compact_logo_line_count_for(true, art), count_lines(SMALL));
        assert_eq!(full_logo_art(true, art), Some(FULL));
        assert_eq!(compact_logo_art(true, art), Some(SMALL));
    }

    #[test]
    fn set_logo_override_round_trips() {
        // Process-wide cell; always restore so sibling tests see the default.
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                set_logo_override(None);
            }
        }
        let _reset = Reset;

        let art = ExternalLogoArt {
            full: "A",
            small: "B",
        };
        set_logo_override(Some(art));
        assert_eq!(logo_override(), Some(art));
        set_logo_override(None);
        assert_eq!(logo_override(), None);
    }

    #[test]
    fn welcome_brand_override_round_trips() {
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                set_welcome_brand_override(None);
            }
        }
        let _reset = Reset;

        let brand = ExternalWelcomeBrand {
            title: "grok-pi",
            subtitle: "Pi agent core",
            version: "0.0.5",
        };
        set_welcome_brand_override(Some(brand));
        assert_eq!(welcome_brand_override(), Some(brand));
        set_welcome_brand_override(None);
        assert_eq!(welcome_brand_override(), None);
    }
}
