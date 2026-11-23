//! Native trackpad haptics when the pointer lands on a new timeline tick.
//!
//! On macOS the primary backend is the private Force Touch actuator behind
//! `MultitouchSupport`: the framework is `dlopen`ed, an `AppleMultitouchDevice`
//! that advertises `ActuationSupported` is opened via IOKit, and pulses are
//! sent with `MTActuatorActuate` at type 3 (weak) / 4 (medium) / 6 (strong).
//! No Alma resource or helper process is involved.
//!
//! The waveform is a sequence of pulses with absolute actuation types and
//! relative millisecond offsets (the comma pattern reversed from Alma's
//! renderer). The default EDC-style detent is the finger-tested double tooth:
//! `strong -> 45ms -> medium`. Delayed pulses run on a dedicated thread
//! because a TUI never drains the main run loop, so a main-queue timer would
//! swallow the second tooth.
//!
//! Both frameworks are loaded lazily on the first pulse, so headless and
//! non-macOS runs stay GPU-free. When no actuator can be opened, AppKit's
//! `NSHapticFeedbackManager` remains the best-effort fallback.

use crate::appearance::cache::TimelineHapticStrength;
use crate::views::timeline::TimelineHit;

pub(super) fn landed_on_tick(
    pointer_moved: bool,
    previous: Option<TimelineHit>,
    current: Option<TimelineHit>,
) -> bool {
    pointer_moved && previous != current && matches!(current, Some(TimelineHit::Tick(_)))
}

/// How hard a dent the actuator makes. These are the raw `actuationType`
/// values used by Alma's `HapticTool` (`weak` / `medium` / `strong`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Strength(i32);

impl Strength {
    const WEAK: Self = Self(3);
    const MEDIUM: Self = Self(4);
    const STRONG: Self = Self(6);

    /// One step down, mirroring Alma's `ZL(t)` (`mgn[index - 1]`).
    fn lower(self) -> Self {
        match self {
            Self::WEAK => Self::WEAK,
            Self::MEDIUM => Self::WEAK,
            Self::STRONG => Self::MEDIUM,
            // Strength is only ever constructed from the three constants.
            other => other,
        }
    }
}

/// One pulse of a detent: an absolute actuation type and its offset (ms)
/// relative to the start of the sequence. Offset zero fires immediately.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Pulse {
    strength: Strength,
    offset_ms: u32,
}

impl Pulse {
    const fn now(strength: Strength) -> Self {
        Self {
            strength,
            offset_ms: 0,
        }
    }

    const fn after(strength: Strength, offset_ms: u32) -> Self {
        Self {
            strength,
            offset_ms,
        }
    }
}

/// Ratchet styles from Alma's renderer. The pager has no style setting yet;
/// `edc` (the finger-tested double tooth) is the shipped default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Style {
    Edc,
    Single,
    Double,
    Engage,
    Triple,
    Soft,
}

impl Style {
    /// The hover waveform. `at_edge` marks the first/last tick of the rail,
    /// where the detent keeps the heavy-then-light shape but shortens the gap.
    fn pulses(self, strength: Strength, at_edge: bool) -> Vec<Pulse> {
        let lower = strength.lower();
        match self {
            // The user's finger test confirmed the double-tooth shape as the
            // EDC feel. Speed adaptation from Alma's `Ucs` was dropped: it
            // made each hover fire either one or two pulses unpredictably.
            Self::Edc => vec![
                Pulse::now(strength),
                Pulse::after(lower, if at_edge { 40 } else { 45 }),
            ],
            Self::Single => vec![Pulse::now(strength)],
            Self::Double => vec![Pulse::now(strength), Pulse::after(lower, 45)],
            Self::Engage => vec![Pulse::now(lower), Pulse::after(strength, 30)],
            Self::Triple => vec![
                Pulse::now(lower),
                Pulse::after(lower, 35),
                Pulse::after(strength, 70),
            ],
            // Magnetic alignment is a system AppKit pulse, not an actuator
            // pattern; see `pulse`.
            Self::Soft => Vec::new(),
        }
    }
}

pub(super) fn tick(strength: TimelineHapticStrength) {
    match strength {
        TimelineHapticStrength::Off => {}
        // Keep the magnetic / guide-snap level on AppKit's soft alignment
        // pulse, as before, while the stronger levels gain the private
        // actuator and its double-tooth waveform.
        TimelineHapticStrength::Light => platform::appkit_pulse(Pattern::Alignment),
        TimelineHapticStrength::Medium | TimelineHapticStrength::Strong => {
            platform::detent(Style::Edc, actuation_strength(strength));
        }
    }
}

/// The user setting still says `medium`/`strong`, but the shipped waveform is
/// the finger-tested double tooth at full strength. Both levels use it.
fn actuation_strength(_strength: TimelineHapticStrength) -> Strength {
    Strength::STRONG
}

/// `NSHapticFeedbackPattern` from `AppKit/NSHapticFeedback.h`.
#[repr(isize)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pattern {
    Alignment = 1,
    LevelChange = 2,
}

/// Build the raw pulse schedule for a style, without touching any OS API.
fn request_detent(style: Style, strength: Strength, at_edge: bool) -> Vec<Pulse> {
    style.pulses(strength, at_edge)
}

#[cfg(target_os = "macos")]
mod platform {
    //! Private `MultitouchSupport` actuator, with AppKit as the fallback.
    //!
    //! Threading: the first pulse initialises the actuator and spawns one
    //! dedicated thread that owns the handle. Pulses are messages on a
    //! channel; a delayed tooth is slept out on that thread, so it fires even
    //! while the TUI's main thread is busy in `block_on`.

    use super::{Pattern, Pulse, Strength, Style, request_detent};
    use std::ffi::{CStr, CString, c_char, c_void};
    use std::ptr::{NonNull, null_mut};
    use std::sync::{OnceLock, mpsc};
    use std::time::Duration;

    /// `/System/Library/PrivateFrameworks/MultitouchSupport.framework/MultitouchSupport`
    const MULTITOUCH_PATH: &CStr = c"/System/Library/PrivateFrameworks/MultitouchSupport.framework/MultitouchSupport";
    const APPKIT_PATH: &CStr = c"/System/Library/Frameworks/AppKit.framework/AppKit";

    /// `NSHapticFeedbackPerformanceTimeNow` — perform immediately; the default
    /// time would wait for the next draw, which a TUI frame may never reach.
    const PERFORM_NOW: usize = 1;

    type CreateFn = unsafe extern "C" fn(u64) -> *mut c_void;
    type StateFn = unsafe extern "C" fn(*mut c_void) -> i32;
    type ActuateFn = unsafe extern "C" fn(*mut c_void, i32, u32, f32, f32) -> i32;

    /// An opened actuator handle. Only the haptic thread touches it; it stays
    /// open for the pager's lifetime so repeated hovers never pay for a reopen.
    struct Actuator {
        handle: NonNull<c_void>,
        actuate: ActuateFn,
    }

    // SAFETY: the private framework's state is only accessed from the single
    // haptic thread, which is the only owner of this value.
    unsafe impl Send for Actuator {}

    impl Actuator {
        /// dlopen the private framework, resolve the four symbols, and open
        /// the first multitouch device that supports actuation.
        ///
        /// # Safety
        /// Calls into the private MultitouchSupport and IOKit C APIs.
        unsafe fn open() -> Option<Self> {
            // SAFETY: constant NUL-terminated path; the handle is leaked on
            // purpose so the framework stays loaded for the process.
            let framework = unsafe { libc::dlopen(MULTITOUCH_PATH.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
            if framework.is_null() {
                return None;
            }
            macro_rules! symbol {
                ($name:literal) => {{
                    let ptr = unsafe { libc::dlsym(framework, c_string($name).as_ptr()) };
                    if ptr.is_null() {
                        return None;
                    }
                    ptr
                }};
            }
            let create: CreateFn = unsafe { std::mem::transmute(symbol!("MTActuatorCreateFromDeviceID")) };
            let open: StateFn = unsafe { std::mem::transmute(symbol!("MTActuatorOpen")) };
            let close: StateFn = unsafe { std::mem::transmute(symbol!("MTActuatorClose")) };
            let actuate: ActuateFn = unsafe { std::mem::transmute(symbol!("MTActuatorActuate")) };

            // SAFETY: `IOServiceMatching` consumes the name string; the
            // iterator and every service object are released below.
            let matching = c_string("AppleMultitouchDevice");
            let dict = unsafe { IOServiceMatching(matching.as_ptr()) };
            if dict.is_null() {
                return None;
            }
            let mut iterator: u32 = 0;
            let status = unsafe { IOServiceGetMatchingServices(KIOMAINPORT_DEFAULT, dict, &mut iterator) };
            if status != KERN_SUCCESS {
                return None;
            }

            let supported_key = cf_string("ActuationSupported");
            let device_id_key = cf_string("Multitouch ID");
            let mut opened = None;
            loop {
                let service = unsafe { IOIteratorNext(iterator) };
                if service == 0 {
                    break;
                }
                let supported = io_property_bool(service, supported_key);
                let device_id = io_property_u64(service, device_id_key);
                unsafe { IOObjectRelease(service) };
                if !supported {
                    continue;
                }
                if let Some(id) = device_id {
                    // SAFETY: `id` came from the IORegistry on a device that
                    // advertises actuation support.
                    let device = unsafe { create(id) };
                    if device.is_null() {
                        continue;
                    }
                    // SAFETY: `device` is the freshly created actuator.
                    if unsafe { open(device) } == KERN_SUCCESS {
                        opened = Some(Self {
                            handle: NonNull::new_unchecked(device),
                            actuate,
                        });
                        break;
                    }
                    // `open` failed; release the created object if the
                    // framework exposes the matching close operation.
                    unsafe { close(device) };
                }
            }
            unsafe { IOObjectRelease(iterator) };
            unsafe {
                CFRelease(supported_key);
                CFRelease(device_id_key);
            }
            opened
        }

        fn fire(&self, strength: Strength) -> bool {
            // SAFETY: `self.handle` is an open MTActuator handle, owned by
            // this thread. The trailing flags match Alma's call: repeat=0,
            // sampleCount=0, samplePeriod=1.
            let result = unsafe { (self.actuate)(self.handle.as_ptr(), strength.0, 0, 0.0, 1.0) };
            result == KERN_SUCCESS
        }
    }

    /// Helper for short NUL-terminated symbol and service names.
    fn c_string(value: &str) -> CString {
        CString::new(value).expect("IOKit names contain no NUL")
    }

    /// `IORegistryEntryCreateCFProperty` takes a `CFStringRef`, not a C string.
    fn cf_string(value: &str) -> *mut c_void {
        let name = c_string(value);
        // SAFETY: constant NUL-terminated UTF-8; handle released by the caller.
        let key = unsafe { CFStringCreateWithCString(null_mut(), name.as_ptr(), UTF8_ENCODING) };
        assert!(!key.is_null(), "failed to allocate IOKit property key");
        key
    }

    /// `kCFStringEncodingUTF8` from `CFStringEncoding.h`.
    const UTF8_ENCODING: u32 = 0x0800_0100;

    const KERN_SUCCESS: i32 = 0;
    const KIOMAINPORT_DEFAULT: u32 = 0;

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOServiceMatching(name: *const c_char) -> *mut c_void;
        fn IOServiceGetMatchingServices(
            mainPort: u32,
            matching: *mut c_void,
            iterator: *mut u32,
        ) -> i32;
        fn IOIteratorNext(iterator: u32) -> u32;
        fn IOObjectRelease(object: u32) -> i32;
        fn IORegistryEntryCreateCFProperty(
            entry: u32,
            key: *const c_void,
            allocator: *mut c_void,
            options: u32,
        ) -> *mut c_void;
    }

    /// Read a boolean IOKit property through the `CFBoolean` API.
    fn io_property_bool(service: u32, key: *const c_void) -> bool {
        // SAFETY: `service` is a live iterator entry and `key` is a CFString.
        let value = unsafe { IORegistryEntryCreateCFProperty(service, key, null_mut(), 0) };
        if value.is_null() {
            return false;
        }
        // SAFETY: the property is an `NSNumber`/`CFBoolean` on this IOKit key.
        let result = unsafe { CFBooleanGetValue(value) };
        // SAFETY: balanced retain from IORegistryEntryCreateCFProperty.
        unsafe { CFRelease(value) };
        result
    }

    /// Read a 64-bit integer IOKit property via the CoreFoundation number API.
    fn io_property_u64(service: u32, key: *const c_void) -> Option<u64> {
        // SAFETY: see `io_property_bool`.
        let value = unsafe { IORegistryEntryCreateCFProperty(service, key, null_mut(), 0) };
        if value.is_null() {
            return None;
        }
        // `kCFNumberSInt64Type` is 4; the value buffer must be 64-bit.
        let mut out: u64 = 0;
        // SAFETY: the property is a CFNumber on this IOKit key.
        let ok = unsafe { CFNumberGetValue(value, 4, &mut out as *mut u64 as *mut c_void) };
        // SAFETY: balanced retain from IORegistryEntryCreateCFProperty.
        unsafe { CFRelease(value) };
        ok.then_some(out)
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRelease(value: *mut c_void);
        fn CFStringCreateWithCString(
            allocator: *mut c_void,
            c_string: *const c_char,
            encoding: u32,
        ) -> *mut c_void;
        fn CFBooleanGetValue(value: *mut c_void) -> bool;
        fn CFNumberGetValue(value: *mut c_void, the_type: i32, value_ptr: *mut c_void) -> bool;
    }

    enum Message {
        Pulses(Vec<Pulse>),
    }

    struct HapticThread {
        sender: mpsc::Sender<Message>,
    }

    fn haptic_thread() -> &'static HapticThread {
        static HAPTICS: OnceLock<HapticThread> = OnceLock::new();
        HAPTICS.get_or_init(|| {
            let (sender, receiver) = mpsc::channel();
            std::thread::Builder::new()
                .name("timeline-haptics".to_string())
                .spawn(move || {
                    // Created on the dedicated thread: dlopen + IOKit probing
                    // never blocks the TUI's event loop.
                    let actuator = unsafe { Actuator::open() };
                    loop {
                        let Ok(message) = receiver.recv() else {
                            break;
                        };
                        let Message::Pulses(pulses) = message;
                        let mut due: Vec<(Strength, std::time::Instant)> = Vec::new();
                        let now = std::time::Instant::now();
                        for pulse in pulses {
                            let at = now + Duration::from_millis(pulse.offset_ms as u64);
                            if pulse.offset_ms == 0 {
                                fire(&actuator, pulse.strength);
                            } else {
                                due.push((pulse.strength, at));
                            }
                        }
                        due.sort_by_key(|(_, at)| *at);
                        while let Some((strength, at)) = due.first().copied() {
                            let wait = at.saturating_duration_since(std::time::Instant::now());
                            if wait > Duration::ZERO {
                                std::thread::sleep(wait);
                            }
                            fire(&actuator, strength);
                            due.remove(0);
                        }
                    }
                });
            HapticThread { sender }
        })
    }

    fn fire(actuator: &Option<Actuator>, strength: Strength) {
        let Some(actuator) = actuator else {
            appkit_pulse(Pattern::LevelChange);
            return;
        };
        if !actuator.fire(strength) {
            // A failed private actuation is a best-effort fallback signal.
            appkit_pulse(Pattern::LevelChange);
        }
    }

    pub(super) fn detent(style: Style, strength: Strength) {
        let pulses = request_detent(style, strength, false);
        if pulses.is_empty() {
            appkit_pulse(Pattern::Alignment);
            return;
        }
        // The dedicated thread is created before sending: even the immediate
        // tooth goes through it, so all pulses share one serial timeline.
        let _ = haptic_thread().sender.send(Message::Pulses(pulses));
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

    fn appkit_loaded() -> bool {
        static LOADED: OnceLock<bool> = OnceLock::new();
        *LOADED.get_or_init(|| {
            // SAFETY: constant NUL-terminated path; handle leaked on purpose.
            let handle = unsafe { libc::dlopen(APPKIT_PATH.as_ptr(), libc::RTLD_LAZY) };
            !handle.is_null()
        })
    }

    pub(super) fn appkit_pulse(pattern: Pattern) {
        if !appkit_loaded() {
            return;
        }

        // SAFETY: every message targets AppKit's public
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
    use super::{Pattern, Pulse, Strength, Style, request_detent};

    pub(super) fn detent(style: Style, strength: Strength) {
        // Keep the waveform constructors live on every target so their tests
        // compile and run off macOS too.
        let _ = request_detent(style, strength, false);
    }

    pub(super) fn appkit_pulse(_pattern: Pattern) {}
}

#[cfg(test)]
mod tests {
    use super::{Pulse, Strength, Style, TimelineHapticStrength, landed_on_tick, request_detent};

    #[test]
    fn rapid_hover_changes_request_one_pulse_per_landing() {
        use super::TimelineHit::{Down, Tick, Up};

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
                requests.push(current);
            }
            assert_eq!(requests.len(), expected_count);
            previous = current;
        }
        assert_eq!(requests.len(), 4);
    }

    #[test]
    fn configured_modes_are_off_light_and_two_detent_levels() {
        assert_eq!(TimelineHapticStrength::default().as_canonical(), "medium");
        for (canonical, expected) in [
            ("off", Some(TimelineHapticStrength::Off)),
            ("light", Some(TimelineHapticStrength::Light)),
            ("medium", Some(TimelineHapticStrength::Medium)),
            ("strong", Some(TimelineHapticStrength::Strong)),
        ] {
            assert_eq!(
                TimelineHapticStrength::from_canonical(canonical),
                expected,
                "{canonical}"
            );
        }
    }

    #[test]
    fn edc_is_the_finger_tested_double_tooth() {
        assert_eq!(
            request_detent(Style::Edc, Strength::STRONG, false),
            vec![Pulse::now(Strength::STRONG), Pulse::after(Strength::MEDIUM, 45)]
        );
        assert_eq!(
            request_detent(Style::Edc, Strength::STRONG, true),
            vec![Pulse::now(Strength::STRONG), Pulse::after(Strength::MEDIUM, 40)]
        );
    }

    #[test]
    fn all_six_styles_match_the_reverse_engineered_patterns() {
        let t = Strength::STRONG;
        let zl = t.lower();
        assert_eq!(
            request_detent(Style::Single, t, false),
            vec![Pulse::now(t)]
        );
        assert_eq!(
            request_detent(Style::Double, t, false),
            vec![Pulse::now(t), Pulse::after(zl, 45)]
        );
        assert_eq!(
            request_detent(Style::Engage, t, false),
            vec![Pulse::now(zl), Pulse::after(t, 30)]
        );
        assert_eq!(
            request_detent(Style::Triple, t, false),
            vec![
                Pulse::now(zl),
                Pulse::after(zl, 35),
                Pulse::after(t, 70),
            ]
        );
        assert!(request_detent(Style::Soft, t, false).is_empty());
    }

    #[test]
    fn waveforms_scale_with_the_configured_strength() {
        assert_eq!(
            request_detent(Style::Edc, Strength::MEDIUM, false),
            vec![
                Pulse::now(Strength::MEDIUM),
                Pulse::after(Strength::WEAK, 45),
            ]
        );
        assert_eq!(
            request_detent(Style::Single, Strength::WEAK, false),
            vec![Pulse::now(Strength::WEAK)]
        );
    }

    #[test]
    fn strength_levels_and_lower_step_match_actuation_types() {
        assert_eq!(Strength::WEAK.0, 3);
        assert_eq!(Strength::MEDIUM.0, 4);
        assert_eq!(Strength::STRONG.0, 6);
        assert_eq!(Strength::STRONG.lower(), Strength::MEDIUM);
        assert_eq!(Strength::MEDIUM.lower(), Strength::WEAK);
        assert_eq!(Strength::WEAK.lower(), Strength::WEAK);
    }
}
