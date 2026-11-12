//! Native haptic feedback when the pointer crosses a timeline tick.
//!
//! The pulse is a best-effort in-process OS call, never a subprocess. On macOS
//! it messages `NSHapticFeedbackManager` through libobjc. AppKit is `dlopen`ed
//! on the first pulse instead of being linked so headless runs never load it
//! (the same rule as `xai_grok_shared::clipboard`). Other platforms no-op.

use std::time::{Duration, Instant};

/// Several ticks can pass under one pointer movement; cap the pulse rate so a
/// fast vertical drag reads as discrete steps instead of a continuous buzz.
const MIN_INTERVAL: Duration = Duration::from_millis(50);

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

pub(super) fn tick() {
    let allowed = {
        let mut throttle = THROTTLE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        throttle.allow(Instant::now())
    };
    if allowed {
        platform::alignment_tick();
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

    use std::ffi::{c_char, c_void};
    use std::ptr::NonNull;
    use std::sync::OnceLock;

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

    pub(super) fn alignment_tick() {
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
                // NSHapticFeedbackPatternAlignment = 1 and
                // NSHapticFeedbackPerformanceTimeNow = 1; the system may still
                // coalesce the pulse or skip it when no haptic device exists.
                perform_feedback(performer.as_ptr(), perform, 1, 1);
            }
            objc_autoreleasePoolPop(pool);
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    /// Haptic feedback is a macOS-only surface; elsewhere the pulse is a no-op.
    pub(super) fn alignment_tick() {}
}

#[cfg(test)]
mod tests {
    use super::{MIN_INTERVAL, Throttle};
    use std::time::{Duration, Instant};

    #[test]
    fn allows_first_pulse_then_throttles_until_the_interval_elapses() {
        let mut throttle = Throttle::default();
        let start = Instant::now();
        assert!(throttle.allow(start));
        assert!(!throttle.allow(start));
        assert!(!throttle.allow(start + MIN_INTERVAL - Duration::from_millis(1)));
        assert!(throttle.allow(start + MIN_INTERVAL));
    }
}
