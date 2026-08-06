//! Raw bindings to the cgo bridge in
//! `psiphon-core/RustBridge/bridge.go`, built into
//! `libpsiphon_bridge.so` by `build.rs`.
//!
//! Nothing here is safe to call directly outside of [`crate::psiphon`] — see
//! that module for the safe wrapper (thread ownership, string lifetimes,
//! etc).

use std::os::raw::{c_char, c_int};

extern "C" {
    /// Loads/commits the config synchronously and, on success, launches the
    /// tunnel in a background goroutine. Returns 0 on success or a negative
    /// error code.
    pub fn PsiphonStart(
        config_path: *const c_char,
        server_list_path: *const c_char,
        data_root_directory: *const c_char,
        egress_region: *const c_char,
    ) -> c_int;

    /// Cancels a running tunnel and blocks (bounded) until it has shut down.
    /// Safe to call when nothing is running.
    pub fn PsiphonStop();

    /// Waits up to `timeout_ms` milliseconds for the next queued notice.
    /// Returns NULL if none arrived in time. The returned pointer must be
    /// freed with `PsiphonFreeString`.
    pub fn PsiphonPollNotice(timeout_ms: c_int) -> *mut c_char;

    /// Frees a string returned by `PsiphonPollNotice`.
    pub fn PsiphonFreeString(s: *mut c_char);

    /// Returns 1 if a tunnel is currently starting/running, 0 otherwise.
    pub fn PsiphonIsRunning() -> c_int;
}
