//! Shim; see `xai_grok_telemetry::instrumentation` for the implementation.
//!
//! `instrumentation_timer!` lives in telemetry and is re-exported from the shell crate root
//! so existing `crate::instrumentation_timer!` and `xai_grok_shell::instrumentation_timer!` call sites stay.
//! `$crate` inside the macro is telemetry.
//!
//! [`finalize_and_exit`] logs a terminal exit event and shuts down the shared OTel pipeline before the process exits.
//! The telemetry crate exposes the shutdown helper; this thin wrapper combines it with `process::exit`.

pub use xai_grok_telemetry::instrumentation::{
    ChromeTraceOptions, InstrumentationFinalizer, InstrumentationMode, InstrumentationTimer,
    TARGET, current_mode, finalize, finalizer, generate_chrome_trace, install_panic_hook, layer,
    timer,
};

/// Logs an exit event, flushes instrumentation guards, shuts down the OpenTelemetry pipeline, and exits with `code`.
///
/// Stays in shell so callers can keep calling `xai_grok_shell::instrumentation::finalize_and_exit`.
pub fn finalize_and_exit(code: i32) -> ! {
    let signal_name = match code {
        130 => "SIGINT",
        143 => "SIGTERM",
        _ => "other",
    };
    tracing::info!(
        event_type = "process_exit",
        signal = signal_name,
        exit_code = code,
        "Exiting process"
    );
    let _ = finalize();
    if let Some(path) = xai_grok_telemetry::span_profile::finalize() {
        eprintln!("span profile written to {}", path.display());
    }
    xai_grok_telemetry::otel_layer::shutdown_otel();
    // Flush the --debug log stream; exiting via process::exit bypasses main's flush
    xai_grok_telemetry::debug_log::flush();
    std::process::exit(code);
}
