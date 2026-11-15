//! Native haptic feedback when the pointer crosses a timeline tick.
//!
//! The pulse is a best-effort in-process OS call, never a subprocess. On macOS
//! it messages `NSHapticFeedbackManager` through libobjc. AppKit is `dlopen`ed
//! on the first pulse instead of being linked so headless runs never load it
//! (the same rule as `xai_grok_shared::clipboard`). Other platforms no-op.
//!
//! Strength is a user preference ([`TimelineHapticStrength`],
//! `[ui].timeline_haptics`). AppKit exposes only one knob that changes how hard
//! a pulse lands — the pattern — so the levels pick the strongest pattern the
//! system offers. `LevelChange` is the only simulated detent AppKit provides;
//! `Alignment` is a much softer guide-snapping pulse, and `Generic` is weaker
//! still.

use crate::appearance::cache::TimelineHapticStrength;
use std::time::{Duration, Instant};

/// Several ticks can pass under one pointer movement; cap the pulse rate so a
/// fast vertical drag reads as discrete steps instead of a continuous buzz.
///
/// The cap also has to keep up with the pointer: the rail packs one tick per
/// terminal row, and a normal trackpad flick crosses several rows between two
/// mouse-move events. A ceiling much above the row rate turns a flick that
/// crosses twelve ticks into a single pulse, which is most of why the feedback
/// read as "too weak" rather than "too fast". 30 ms leaves headroom over a
/// 60 Hz event stream while still preventing a continuous buzz.
const MIN_INTERVAL: Duration = Duration::from_millis(30);

/// Most ticks one event may contribute. A fast flick can cross a dozen rows;
/// beyond a handful of extra detents it only reads as noise.
const MAX_CROSSED_BURST: usize = 4;

/// Pulses delivered per allowed [`tick`]. A harder level repeats the same
/// pattern instead of switching patterns: AppKit has no amplitude control, so
/// back-to-back detents are the only way to make one crossing feel more
/// substantial than another.
fn pulses(strength: TimelineHapticStrength) -> usize {
    match strength {
        TimelineHapticStrength::Off => 0,
        TimelineHapticStrength::Light => 1,
        TimelineHapticStrength::Medium => 1,
        TimelineHapticStrength::Strong => 2,
    }
}

#[derive(Default)]
struct Throttle {
    last: Option<Instant>,
}

impl Throttle {
    fn allow(&mut self, now: Instant) -> bool {
        if self
            .last
            .is_some_and(|last| now.saturating_duration_since(last) < MIN_INTERVAL)
        {
            return false;
        }
        self.last = Some(now);
        true
    }
}

// Process-wide rather than thread-local: mouse input is driven from the
// event-loop thread today, but the rate limit should not reset if it moves.
static THROTTLE: std::sync::Mutex<Throttle> = std::sync::Mutex::new(Throttle { last: None });

/// Fire the pointer-crossing feedback at `strength`, rate-limited.
///
/// `crossed` is how many ticks one pointer event skipped over. It scales the
/// pulse count, never the rate limit: the cap exists to keep a flick from
/// turning into a buzz, and dropping the skipped steps on top of it is what
/// made the feedback feel absent rather than merely subtle.
pub(super) fn tick(strength: TimelineHapticStrength, crossed: usize) {
    if strength == TimelineHapticStrength::Off {
        return;
    }
    let allowed = {
        let mut throttle = THROTTLE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        throttle.allow(Instant::now())
    };
    if allowed {
        // Cap the burst so a twelve-row jump does not machine-gun the motor.
        let burst = pulses(strength).saturating_mul(crossed.clamp(1, MAX_CROSSED_BURST));
        for _ in 0..burst {
            platform::pulse(strength);
        }
    }
}

#[cfg(target_os = "macos")]
mod platform {
    //! `NSHapticFeedbackManager` reached through libobjc runtime messaging.
    //!
    //! Threading: AppKit's header translation does not mark
    //! `NSHapticFeedbackManager` as `MainThreadOnly`, and the pager polls mouse
    //! input on the process main thread (`block_on(LocalSet::...)`), so the call
    //! needs no main-queue hop. Deferring to the main queue would in fact be
    //! wrong here: a TUI never drains the main run loop, so the feedback would
    //! be dropped.

    use super::TimelineHapticStrength;
    use std::ffi::{c_char, c_void};
    use std::ptr::NonNull;
    use std::sync::OnceLock;

    /// `NSHapticFeedbackPattern` from `AppKit/NSHapticFeedback.h`.
    const PATTERN_GENERIC: isize = 0;
    const PATTERN_ALIGNMENT: isize = 1;
    const PATTERN_LEVEL_CHANGE: isize = 2;

    /// `NSHapticFeedbackPerformanceTimeNow` — perform immediately; the default
    /// time would wait for the next draw, which a TUI frame may never reach.
    const PERFORM_NOW: usize = 1;

    /// Load AppKit once. Linking the framework instead makes dyld initialise
    /// its GPU subsystem at process startup, even for headless runs.
    fn appkit_loaded() -> bool {
        static LOADED: OnceLock<bool> = OnceLock::new();
        *LOADED.get_or_init(|| {
            let path = c"/System/Library/Frameworks/AppKit.framework/AppKit";
            // SAFETY: `path` is a constant NUL-terminated string. The handle is
            // leaked on purpose so AppKit stays loaded for the process.
            let handle = unsafe { libc::dlopen(path.as_ptr(), libc::RTLD_LAZY) };
            !handle.is_null()
        })
    }

    /// The pattern a level asks the system for. `LevelChange` is the only
    /// simulated detent AppKit offers, so it carries the two levels that must
    /// actually be felt; `Light` deliberately stays on the softer guide-snap
    /// pulse.
    fn pattern(strength: TimelineHapticStrength) -> isize {
        match strength {
            // `tick()` never reaches the platform module with `Off`.
            TimelineHapticStrength::Off => PATTERN_GENERIC,
            TimelineHapticStrength::Light => PATTERN_ALIGNMENT,
            TimelineHapticStrength::Medium | TimelineHapticStrength::Strong => PATTERN_LEVEL_CHANGE,
        }
    }

    // libobjc is cheap to link and, unlike AppKit, pulls no GPU subsystem.
    #[link(name = "objc")]
    unsafe extern "C" {
        fn objc_getClass(name: *const c_char) -> *mut c_void;
        fn sel_registerName(name: *const c_char) -> *mut c_void;
        fn objc_autoreleasePoolPush() -> *mut c_void;
        fn objc_autoreleasePoolPop(pool: *mut c_void);
        fn objc_msgSend();
    }

    type MsgSend0 = unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void;
    /// `performFeedbackPattern:performanceTime:` takes `NSInteger` then
    /// `NSUInteger`.
    type MsgSend2 = unsafe extern "C" fn(*mut c_void, *mut c_void, isize, usize);

    pub(super) fn pulse(strength: TimelineHapticStrength) {
        if !appkit_loaded() {
            return;
        }

        // SAFETY: every message below targets AppKit's public
        // `NSHapticFeedbackManager` API. `objc_msgSend` is typed per call site
        // through the signatures above; the class resolves because AppKit is
        // loaded, and nil receivers are checked instead of assumed.
        unsafe {
            let Some(class) = NonNull::new(objc_getClass(c"NSHapticFeedbackManager".as_ptr()))
            else {
                return;
            };
            let get_performer: MsgSend0 = std::mem::transmute(objc_msgSend as *const ());
            let perform_feedback: MsgSend2 = std::mem::transmute(objc_msgSend as *const ());
            let default_performer = sel_registerName(c"defaultPerformer".as_ptr());
            let perform = sel_registerName(c"performFeedbackPattern:performanceTime:".as_ptr());

            // `+defaultPerformer` may hand back an autoreleased object, so give
            // it a pool to drain into rather than accumulating across pulses.
            let pool = objc_autoreleasePoolPush();
            let performer = get_performer(class.as_ptr(), default_performer);
            if let Some(performer) = NonNull::new(performer) {
                // The system may still coalesce the pulse or skip it when no
                // haptic device exists, so this stays best-effort by design.
                perform_feedback(performer.as_ptr(), perform, pattern(strength), PERFORM_NOW);
            }
            objc_autoreleasePoolPop(pool);
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::TimelineHapticStrength;

    /// Haptic feedback is a macOS-only surface; elsewhere the pulse is a no-op.
    pub(super) fn pulse(_strength: TimelineHapticStrength) {}
}

#[cfg(test)]
mod tests {
    use super::{MAX_CROSSED_BURST, MIN_INTERVAL, Throttle, pulses};
    use crate::appearance::cache::TimelineHapticStrength;
    use std::time::{Duration, Instant};

    /// Mirrors the burst arithmetic in [`super::tick`] so the scaling rule is
    /// covered without touching the platform layer.
    fn burst(strength: TimelineHapticStrength, crossed: usize) -> usize {
        pulses(strength).saturating_mul(crossed.clamp(1, MAX_CROSSED_BURST))
    }

    #[test]
    fn crossing_more_ticks_pulses_proportionally_up_to_the_cap() {
        assert_eq!(burst(TimelineHapticStrength::Medium, 1), 1);
        assert_eq!(burst(TimelineHapticStrength::Medium, 3), 3);
        // Beyond the cap the burst plateaus instead of growing without bound.
        assert_eq!(
            burst(TimelineHapticStrength::Medium, 12),
            burst(TimelineHapticStrength::Medium, MAX_CROSSED_BURST)
        );
        assert_eq!(burst(TimelineHapticStrength::Off, 12), 0);
    }

    #[test]
    fn a_zero_crossing_still_pulses_once() {
        assert_eq!(burst(TimelineHapticStrength::Medium, 0), 1);
    }

    #[test]
    fn allows_first_pulse_then_throttles_until_the_interval_elapses() {
        let mut throttle = Throttle::default();
        let start = Instant::now();
        assert!(throttle.allow(start));
        assert!(!throttle.allow(start));
        assert!(!throttle.allow(start + MIN_INTERVAL - Duration::from_millis(1)));
        assert!(throttle.allow(start + MIN_INTERVAL));
    }

    #[test]
    fn off_requests_no_pulses_and_strong_repeats() {
        assert_eq!(pulses(TimelineHapticStrength::Off), 0);
        assert_eq!(pulses(TimelineHapticStrength::Light), 1);
        assert_eq!(pulses(TimelineHapticStrength::Medium), 1);
        assert!(
            pulses(TimelineHapticStrength::Strong) > 1,
            "strong must out-pulse medium"
        );
    }

    #[test]
    fn the_rate_cap_lets_a_flick_through_tick_by_tick() {
        // A 120 Hz trackpad stream (8.3 ms apart) must not collapse into a
        // single pulse: the cap is what makes a flick read as discrete steps.
        let mut throttle = Throttle::default();
        let start = Instant::now();
        let allowed = (0..12)
            .filter(|step| throttle.allow(start + Duration::from_millis(8) * *step))
            .count();
        assert!(
            allowed >= 3,
            "12 crossings at 120 Hz yielded {allowed} pulses"
        );
    }
}
