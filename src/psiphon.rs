//! Safe wrapper around the raw FFI in [`crate::ffi`]: owns the background
//! poller thread that drains notices out of the Go bridge and forwards them
//! to the UI thread over a channel.

use crate::ffi;
use crate::notice::Notice;
use std::ffi::CString;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread;

/// Events delivered from the poller thread to the UI thread.
pub enum Event {
    Notice(Notice),
    /// A line came back from the bridge that wasn't valid JSON. Surfaced
    /// so it isn't silently dropped, but shouldn't normally happen.
    Unparsed(String),
}

#[derive(Debug)]
pub enum StartError {
    InvalidPath(&'static str),
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StartError::InvalidPath(which) => {
                write!(f, "{which} contains a NUL byte and cannot be passed to the bridge")
            }
        }
    }
}

/// Handle to the running (or idle) tunnel. Owns the notice-poller thread
/// for the lifetime of the process.
pub struct Controller {
    stop_poller: Arc<AtomicBool>,
    poller_handle: Option<thread::JoinHandle<()>>,
}

impl Controller {
    /// Spawns the poller thread. Does NOT start a tunnel yet — call
    /// [`Controller::launch`] for that. Splitting these lets the UI come up
    /// and start rendering before any config is touched.
    pub fn new() -> (Self, Receiver<Event>) {
        let (tx, rx): (Sender<Event>, Receiver<Event>) = channel();
        let stop_poller = Arc::new(AtomicBool::new(false));
        let stop_poller_clone = stop_poller.clone();

        let poller_handle = thread::spawn(move || {
            while !stop_poller_clone.load(Ordering::Relaxed) {
                // Block up to 250ms per poll; long enough to be cheap on
                // CPU, short enough that the UI feels live and shutdown is
                // responsive.
                let ptr = unsafe { ffi::PsiphonPollNotice(250) };
                if ptr.is_null() {
                    continue;
                }
                let raw = unsafe {
                    let s = std::ffi::CStr::from_ptr(ptr).to_string_lossy().into_owned();
                    ffi::PsiphonFreeString(ptr);
                    s
                };
                let event = match Notice::parse(&raw) {
                    Some(n) => Event::Notice(n),
                    None => Event::Unparsed(raw),
                };
                if tx.send(event).is_err() {
                    break;
                }
            }
        });

        (
            Controller {
                stop_poller,
                poller_handle: Some(poller_handle),
            },
            rx,
        )
    }

    /// Loads `config_path` / `server_list_path` / `data_root_directory` and
    /// starts the tunnel in the background. Returns quickly (config load +
    /// commit only); connection progress arrives as `Event`s. `Ok(())`
    /// merely means the request was accepted, not that the tunnel is
    /// connected yet — watch for `Tunnels{count>0}` / `BridgeError` notices.
    /// `egress_region` is an optional ISO 3166-1 alpha-2 country code (e.g.
    /// "US") to prefer for egress; pass "" for no preference ("Any").
    pub fn launch(
        &self,
        config_path: &str,
        server_list_path: &str,
        data_root_directory: &str,
        egress_region: &str,
    ) -> Result<(), StartError> {
        let c_config =
            CString::new(config_path).map_err(|_| StartError::InvalidPath("config path"))?;
        let c_data_root = CString::new(data_root_directory)
            .map_err(|_| StartError::InvalidPath("data root directory"))?;
        let c_server_list = if server_list_path.is_empty() {
            None
        } else {
            Some(
                CString::new(server_list_path)
                    .map_err(|_| StartError::InvalidPath("server list path"))?,
            )
        };
        let c_egress_region = if egress_region.is_empty() {
            None
        } else {
            Some(
                CString::new(egress_region)
                    .map_err(|_| StartError::InvalidPath("egress region"))?,
            )
        };

        let server_list_ptr = c_server_list
            .as_ref()
            .map(|c| c.as_ptr())
            .unwrap_or(std::ptr::null());
        let egress_region_ptr = c_egress_region
            .as_ref()
            .map(|c| c.as_ptr())
            .unwrap_or(std::ptr::null());

        // Return code is intentionally ignored here beyond logging purposes
        // - on failure the bridge also pushes a "BridgeError" notice, which
        // is what the UI actually renders.
        let _rc = unsafe {
            ffi::PsiphonStart(
                c_config.as_ptr(),
                server_list_ptr,
                c_data_root.as_ptr(),
                egress_region_ptr,
            )
        };

        Ok(())
    }

    /// Requests shutdown on a detached thread so the caller (typically the
    /// UI thread) doesn't block. Watch for the "BridgeStopped" notice to
    /// know when it's actually done.
    pub fn stop_async(&self) {
        thread::spawn(|| unsafe {
            ffi::PsiphonStop();
        });
    }

    /// Blocking shutdown, for use during final process exit after the
    /// terminal has already been restored.
    pub fn stop_blocking(&self) {
        unsafe {
            ffi::PsiphonStop();
        }
    }

    pub fn is_running() -> bool {
        unsafe { ffi::PsiphonIsRunning() != 0 }
    }
}

impl Drop for Controller {
    fn drop(&mut self) {
        self.stop_poller.store(true, Ordering::Relaxed);
        if let Some(handle) = self.poller_handle.take() {
            let _ = handle.join();
        }
    }
}
