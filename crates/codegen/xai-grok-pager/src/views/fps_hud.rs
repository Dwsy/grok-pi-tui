//! Release-safe FPS readout: `/debug fps`, and `GROK_FPS` on release builds.
//!
//! The frame profiler (`render::frame_metrics`, `GROK_FPS`) threads per-phase timings through `draw_frame`, so it exists only in debug/dev builds.
//! This HUD measures the one thing that needs no pipeline change: the wall-clock cost of the whole `draw_frame` call (render, flush, writer handoff).
//! It therefore compiles into release builds behind a runtime toggle (the scroll-debug HUD precedent).
//! It measures the production render path itself, with no debug-only approximation.
//!
//! `GROK_FPS` ownership: in debug/dev builds the env feeds `FrameMetrics` and this HUD stays toggle-only (no double overlay).
//! On release binaries, where that overlay does not exist, the same env enables this HUD from startup.
//! `GROK_FPS=1` is therefore never a silent no-op ([`HONORS_GROK_FPS_ENV`]).
//!
//! "fps" here is render throughput (1 / mean frame cost), not paint frequency: the pager draws on demand.
//! The second stats line reports measured paints and ticks per second over a sliding window; it refreshes on paint, so an idle screen keeps its last pre-idle rates.
//!
//! Invariant (shared with the scroll HUD): pure observation. Disabled cost is one bool check per frame; rendering only paints buffer cells.
use super::debug_style;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use std::collections::VecDeque;
use std::time::{Duration, Instant};
/// Frame-duration ring buffer capacity (~4s at 30fps).
const SAMPLE_CAP: usize = 120;
/// Overlay text refresh cadence; avoids re-sorting/formatting every frame.
const REFRESH: Duration = Duration::from_millis(250);
/// Sliding window for the measured paint/tick rates.
const RATE_WINDOW: Duration = Duration::from_secs(2);
/// Panel width in cells; each line is padded/truncated to this.
const PANEL_WIDTH: u16 = 32;
/// Whether this HUD owns the `GROK_FPS` env gate: only where the dev `FrameMetrics` overlay is compiled out.
/// In debug/dev builds the env keeps feeding that overlay alone.
const HONORS_GROK_FPS_ENV: bool = true;
/// Runtime state for the FPS HUD.
/// `GROK_FPS` enables it at startup on release binaries ([`HONORS_GROK_FPS_ENV`]); `/debug fps` toggles it live everywhere.
/// Deliberately NOT a settings-registry entry: it is a diagnostic, not a preference to persist.
pub struct FpsHud {
    samples: VecDeque<Duration>,
    /// Instants of painted frames from the last [`RATE_WINDOW`].
    paints: VecDeque<Instant>,
    /// Instants of animation ticks from the last [`RATE_WINDOW`].
    ticks: VecDeque<Instant>,
    /// `Some` while the HUD is enabled, holding the enable instant; the rate window never reaches back past it.
    enabled_at: Option<Instant>,
    /// Cached stats line, rewritten at most every [`REFRESH`].
    body: String,
    /// Cached measured-rates line, rewritten together with `body`.
    rates: String,
    last_refresh: Option<Instant>,
}
impl Default for FpsHud {
    fn default() -> Self {
        Self::new()
    }
}
impl FpsHud {
    pub fn new() -> Self {
        Self::with_env(std::env::var("GROK_FPS").ok())
    }
    /// `env` is the raw `GROK_FPS` value; the truthiness rule (nonempty and not `"0"`) matches `FrameMetrics` and `GROK_SCROLL_DEBUG`.
    fn with_env(env: Option<String>) -> Self {
        let env_on = HONORS_GROK_FPS_ENV && env.is_some_and(|v| !v.is_empty() && v != "0");
        Self {
            samples: VecDeque::with_capacity(SAMPLE_CAP),
            paints: VecDeque::new(),
            ticks: VecDeque::new(),
            enabled_at: env_on.then(Instant::now),
            body: String::new(),
            rates: String::new(),
            last_refresh: None,
        }
    }
    /// Whether the HUD is currently enabled.
    pub fn enabled(&self) -> bool {
        self.enabled_at.is_some()
    }
    /// `/debug fps` runtime toggle.
    pub fn toggle(&mut self) {
        self.enabled_at = if self.enabled_at.is_some() {
            None
        } else {
            Some(Instant::now())
        };
        self.samples.clear();
        self.paints.clear();
        self.ticks.clear();
        self.body.clear();
        self.rates.clear();
        self.last_refresh = None;
    }
    /// Rows the overlay occupies when enabled (for stacking overlays).
    pub fn overlay_height(&self) -> u16 {
        if self.enabled() { 3 } else { 0 }
    }
    /// Record one frame's `draw_frame` wall duration.
    pub fn record(&mut self, frame: Duration) {
        if !self.enabled() {
            return;
        }
        if self.samples.len() >= SAMPLE_CAP {
            self.samples.pop_front();
        }
        self.samples.push_back(frame);
        push_event(&mut self.paints, Instant::now());
    }
    /// Record one animation tick (`AppView::tick`), for the measured tick rate.
    /// Ticks are demand-driven, so a low rate with nothing animating is normal.
    pub fn note_tick(&mut self) {
        if !self.enabled() {
            return;
        }
        push_event(&mut self.ticks, Instant::now());
    }
    /// Owned per-frame render params (`None` unless enabled), assembled by `AppView::draw` BEFORE the frame closure (the `ScrollDebugPanel` pattern).
    /// `top_offset` leaves rows for overlays stacked above.
    pub fn overlay(&mut self, top_offset: u16) -> Option<FpsOverlay> {
        let enabled_at = self.enabled_at?;
        if self.last_refresh.is_none_or(|at| at.elapsed() >= REFRESH) {
            let now = Instant::now();
            self.body = format_stats(&self.samples);
            self.rates = format_rates(&mut self.paints, &mut self.ticks, enabled_at, now);
            self.last_refresh = Some(now);
        }
        Some(FpsOverlay {
            body: self.body.clone(),
            rates: self.rates.clone(),
            top_offset,
        })
    }
}
/// Appends `now` and drops entries older than [`RATE_WINDOW`], so the deque needs no cap.
fn push_event(events: &mut VecDeque<Instant>, now: Instant) {
    events.push_back(now);
    prune(events, now);
}
fn prune(events: &mut VecDeque<Instant>, now: Instant) {
    while events
        .front()
        .is_some_and(|&at| now.duration_since(at) > RATE_WINDOW)
    {
        events.pop_front();
    }
}
/// The measured-rates line: painted frames and animation ticks per second.
fn format_rates(
    paints: &mut VecDeque<Instant>,
    ticks: &mut VecDeque<Instant>,
    enabled_at: Instant,
    now: Instant,
) -> String {
    format!(
        "paint:{:.1}/s tick:{:.1}/s",
        rate(paints, enabled_at, now),
        rate(ticks, enabled_at, now),
    )
}
/// Events per second over the sliding window, shrunk to the time since enabling.
/// A full window would understate a freshly enabled HUD's rates.
fn rate(events: &mut VecDeque<Instant>, enabled_at: Instant, now: Instant) -> f64 {
    prune(events, now);
    let window = now
        .duration_since(enabled_at)
        .min(RATE_WINDOW)
        .max(Duration::from_millis(100));
    events.len() as f64 / window.as_secs_f64()
}
/// Mean/percentile line from the ring buffer; placeholder before samples.
fn format_stats(samples: &VecDeque<Duration>) -> String {
    if samples.is_empty() {
        return "fps:- p50:- p95:-".to_string();
    }
    let mut ms: Vec<f64> = samples.iter().map(|d| d.as_secs_f64() * 1000.0).collect();
    ms.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mean_ms = ms.iter().sum::<f64>() / ms.len() as f64;
    let fps = if mean_ms > 1e-6 {
        1000.0 / mean_ms
    } else {
        0.0
    };
    let p50 = percentile(&ms, 50.0);
    let p95 = percentile(&ms, 95.0);
    format!("fps:{fps:.0} p50:{p50:.1}ms p95:{p95:.1}ms")
}
/// Linear-interpolation percentile from a sorted slice.
fn percentile(sorted: &[f64], pct: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = (pct / 100.0) * (sorted.len() - 1) as f64;
    let lower = rank.floor() as usize;
    let Some(&lo) = sorted.get(lower) else {
        return 0.0;
    };
    let Some(&hi) = sorted.get(lower + 1) else {
        return lo;
    };
    let frac = rank - lower as f64;
    lo + (hi - lo) * frac
}
/// Owned render params for one frame (title, throughput line, measured-rates line, top-right).
pub struct FpsOverlay {
    body: String,
    rates: String,
    /// Rows left free for overlays above (the dev `GROK_FPS` line).
    pub top_offset: u16,
}
impl FpsOverlay {
    /// Paint the three-line panel in the top-right corner of `area`, in the shared theme-agnostic debug chrome (every cell, padding included).
    pub fn render(&self, area: Rect, buf: &mut Buffer) {
        debug_style::render_panel(
            area,
            buf,
            self.top_offset,
            PANEL_WIDTH,
            &[
                "fps debug  (/debug fps)",
                self.body.as_str(),
                self.rates.as_str(),
            ],
        );
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::{Color, Modifier, Style};
    #[test]
    fn disabled_by_default_and_toggle_round_trips() {
        let mut hud = FpsHud::with_env(None);
        assert!(!hud.enabled());
        assert_eq!(hud.overlay_height(), 0);
        assert!(hud.overlay(0).is_none());
        hud.toggle();
        assert!(hud.enabled());
        assert_eq!(hud.overlay_height(), 3);
        assert!(hud.overlay(0).is_some());
        hud.toggle();
        assert!(!hud.enabled());
        assert!(hud.overlay(0).is_none());
    }
    /// Default test builds compile without dev instrumentation (release-shaped for this gate), so a
    /// truthy env must construct enabled.
    #[test]
    fn grok_fps_env_enables_hud_where_dev_overlay_absent() {
        for truthy in ["1", "full", " "] {
            assert_eq!(
                FpsHud::with_env(Some(truthy.into())).enabled(),
                HONORS_GROK_FPS_ENV,
                "GROK_FPS={truthy:?} must track the env-gate owner"
            );
        }
        for falsy in [None, Some(String::new()), Some("0".into())] {
            assert!(!FpsHud::with_env(falsy).enabled());
        }
    }
    #[test]
    fn record_caps_ring_buffer_and_toggle_clears_stale_samples() {
        let mut hud = FpsHud::with_env(None);
        hud.toggle();
        for _ in 0..SAMPLE_CAP + 30 {
            hud.record(Duration::from_millis(10));
        }
        assert_eq!(hud.samples.len(), SAMPLE_CAP);
        hud.toggle();
        hud.toggle();
        assert!(hud.samples.is_empty());
        assert!(hud.paints.is_empty());
        assert!(hud.ticks.is_empty());
        assert!(
            hud.overlay(0).unwrap().body.contains("fps:-"),
            "fresh enablement must show placeholders"
        );
    }
    #[test]
    fn measured_rates_count_paints_and_ticks() {
        let mut hud = FpsHud::with_env(None);
        hud.toggle();
        for _ in 0..10 {
            hud.record(Duration::from_millis(5));
        }
        for _ in 0..4 {
            hud.note_tick();
        }
        assert_eq!(hud.paints.len(), 10);
        assert_eq!(hud.ticks.len(), 4);
        let overlay = hud.overlay(0).expect("enabled");
        assert!(
            overlay.rates.starts_with("paint:") && overlay.rates.contains(" tick:"),
            "rates line must carry both measured rates, got {:?}",
            overlay.rates
        );
    }
    #[test]
    fn rate_prunes_clamps_and_floors() {
        let assert_rate = |got: f64, want: f64| {
            assert!((got - want).abs() < 1e-9, "rate {got} != {want}");
        };
        let now = Instant::now();
        let mut events: VecDeque<Instant> = [
            now - Duration::from_secs(3),
            now - Duration::from_millis(1500),
            now - Duration::from_millis(500),
            now,
        ]
        .into();
        assert_rate(
            rate(&mut events, now - Duration::from_secs(10), now),
            3.0 / 2.0,
        );
        assert_eq!(events.len(), 3, "the 3s-old event must be pruned");
        let mut events: VecDeque<Instant> = [now - Duration::from_millis(50), now].into();
        assert_rate(
            rate(&mut events, now - Duration::from_millis(500), now),
            2.0 / 0.5,
        );
        let mut events: VecDeque<Instant> = [now].into();
        assert_rate(
            rate(&mut events, now - Duration::from_millis(10), now),
            1.0 / 0.1,
        );
    }
    #[test]
    fn stats_line_reports_mean_fps_and_percentiles() {
        let mut hud = FpsHud::with_env(None);
        hud.toggle();
        for _ in 0..100 {
            hud.record(Duration::from_millis(10));
        }
        let overlay = hud.overlay(0).expect("enabled");
        assert_eq!(overlay.body, "fps:100 p50:10.0ms p95:10.0ms");
    }
    #[test]
    fn record_and_note_tick_are_noops_while_disabled() {
        let mut hud = FpsHud::with_env(None);
        hud.record(Duration::from_millis(10));
        hud.note_tick();
        assert!(hud.samples.is_empty());
        assert!(hud.paints.is_empty());
        assert!(hud.ticks.is_empty());
    }
    /// Every cell of the panel rect (trailing padding included) must carry the explicit debug chrome, not the theme style underneath.
    #[test]
    fn render_paints_theme_agnostic_style_over_every_panel_cell() {
        let area = Rect::new(0, 0, 60, 6);
        let mut buf = Buffer::empty(area);
        let theme = Style::default()
            .fg(Color::Rgb(228, 228, 228))
            .bg(Color::Rgb(3, 3, 4))
            .add_modifier(Modifier::ITALIC);
        buf.set_style(area, theme);
        let overlay = FpsOverlay {
            body: "fps:100 p50:10.0ms p95:10.0ms".to_string(),
            rates: "paint:12.0/s tick:29.5/s".to_string(),
            top_offset: 1,
        };
        overlay.render(area, &mut buf);
        let x0 = area.width - PANEL_WIDTH;
        for y in 1..4u16 {
            for x in x0..area.width {
                let Some(cell) = buf.cell((x, y)) else {
                    panic!("cell ({x},{y})");
                };
                assert_eq!(cell.bg, Color::Black, "cell ({x},{y}) bg");
                assert!(
                    cell.fg == Color::White || cell.fg == Color::Yellow,
                    "cell ({x},{y}) fg must be debug chrome, got {:?}",
                    cell.fg
                );
                assert_eq!(
                    cell.modifier,
                    Modifier::empty(),
                    "cell ({x},{y}) must shed themed modifiers"
                );
            }
        }
        assert_eq!(buf.cell((0, 1)).map(|c| c.bg), Some(Color::Rgb(3, 3, 4)));
        assert_eq!(
            buf.cell((area.width - 1, 0)).map(|c| c.bg),
            Some(Color::Rgb(3, 3, 4))
        );
    }
}
