//! Request one native haptic pulse when the pointer lands on a new timeline tick.
//!
//! The pulse is a best-effort in-process OS call. AppKit is loaded lazily on
//! macOS so headless runs never load it. Other platforms no-op.

use crate::appearance::cache::TimelineHapticStrength;
use crate::views::timeline::TimelineHit;

pub(super) fn landed_on_tick(
    pointer_moved: bool,
    previous: Option<TimelineHit>,
    current: Option<TimelineHit>,
) -> bool {
    pointer_moved && previous != current && matches!(current, Some(TimelineHit::Tick(_)))
}

/// `NSHapticFeedbackPattern` from `AppKit/NSHapticFeedback.h`.
#[repr(isize)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pattern {
    Alignment = 1,
    LevelChange = 2,
}

pub(super) fn tick(strength: TimelineHapticStrength) {
    request_tick(strength, platform::pulse);
}

fn request_tick(strength: TimelineHapticStrength, request: impl FnOnce(Pattern)) {
    let pattern = match strength {
        TimelineHapticStrength::Off => return,
        TimelineHapticStrength::Light => Pattern::Alignment,
        // Keep the historical key as an alias for the default single detent.
        TimelineHapticStrength::Medium | TimelineHapticStrength::Strong => Pattern::LevelChange,
    };
    request(pattern);
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

    use super::Pattern;
    use std::ffi::{c_char, c_void};
    use std::ptr::NonNull;
    use std::sync::OnceLock;

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

    pub(super) fn pulse(pattern: Pattern) {
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
                perform_feedback(performer.as_ptr(), perform, pattern as isize, PERFORM_NOW);
            }
            objc_autoreleasePoolPop(pool);
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::Pattern;

    /// Haptic feedback is a macOS-only surface; elsewhere the pulse is a no-op.
    pub(super) fn pulse(_pattern: Pattern) {}
}

#[cfg(test)]
mod tests {
    use super::{Pattern, TimelineHapticStrength, TimelineHit, landed_on_tick, request_tick};

    #[test]
    fn rapid_hover_changes_request_one_pulse_per_landing() {
        use TimelineHit::{Down, Tick, Up};

        let mut previous = None;
        let mut requests = Vec::new();
        for (moved, current, expected_count) in [
            (true, Some(Up), 0),
            (true, Some(Tick(2)), 1),
            (true, Some(Tick(2)), 1),
            (true, Some(Tick(14)), 2), // Skipped ticks do not request feedback.
            (true, Some(Tick(13)), 3),
            (false, Some(Tick(7)), 3), // Layout changed under a stationary pointer.
            (true, Some(Down), 3),
            (true, None, 3),
            (true, Some(Tick(7)), 4), // Entering again is a new landing.
        ] {
            if landed_on_tick(moved, previous, current) {
                request_tick(TimelineHapticStrength::default(), |pattern| {
                    requests.push(pattern);
                });
            }
            assert_eq!(requests.len(), expected_count);
            previous = current;
        }
        assert_eq!(requests, [Pattern::LevelChange; 4]);
    }

    #[test]
    fn configured_modes_request_at_most_one_appkit_pattern() {
        assert_eq!(TimelineHapticStrength::default().as_canonical(), "medium");
        for (canonical, expected) in [
            ("off", None),
            ("light", Some(Pattern::Alignment)),
            ("medium", Some(Pattern::LevelChange)),
            ("strong", Some(Pattern::LevelChange)),
        ] {
            let strength = TimelineHapticStrength::from_canonical(canonical).unwrap();
            let mut requested = None;
            request_tick(strength, |pattern| requested = Some(pattern));
            assert_eq!(requested, expected, "{canonical}");
        }
    }
}
