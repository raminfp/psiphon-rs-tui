//! Library half of psiphon-tui: FFI bindings to the Go bridge, the safe
//! controller wrapper, notice parsing, and app state. Split out from the
//! binary so both `src/main.rs` (the TUI) and `examples/smoke.rs` (a
//! headless diagnostic tool) can share the same code.

pub mod app;
pub mod cli;
pub mod ffi;
pub mod notice;
pub mod psiphon;
pub mod regions;
pub mod ui;
