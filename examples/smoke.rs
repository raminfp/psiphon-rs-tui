//! Headless diagnostic tool: exercises config -> bridge -> live notice
//! stream -> stop, without a terminal UI. Useful for sanity-checking a real
//! psiphon.config / server list before running the full TUI, or when
//! stdout piping/redirection is more convenient than a TTY.
//!
//! Usage:
//!   cargo run --example smoke -- <config> [serverList] <dataRootDirectory> [runSeconds]

use psiphon_tui::psiphon::{self, Event};
use std::time::Duration;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!(
            "usage: {} <config> [serverList] <dataRootDirectory> [runSeconds]",
            args.first().map(String::as_str).unwrap_or("smoke")
        );
        std::process::exit(2);
    }

    let config = args[1].clone();
    let (server_list, data_root, run_secs) = if args.len() >= 4 {
        (
            args[2].clone(),
            args[3].clone(),
            args.get(4).and_then(|s| s.parse().ok()).unwrap_or(15),
        )
    } else {
        (
            String::new(),
            args[2].clone(),
            args.get(3).and_then(|s| s.parse().ok()).unwrap_or(15),
        )
    };

    let (controller, rx) = psiphon::Controller::new();
    println!(">> launching: config={config:?} serverList={server_list:?} dataRoot={data_root:?}");
    if let Err(e) = controller.launch(&config, &server_list, &data_root, "") {
        println!(">> launch error: {e}");
    }

    let deadline = std::time::Instant::now() + Duration::from_secs(run_secs);
    while std::time::Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(Event::Notice(n)) => println!("[{}] {}: {}", n.timestamp, n.notice_type, n.summary()),
            Ok(Event::Unparsed(s)) => println!("[unparsed] {s}"),
            Err(_) => {}
        }
    }

    println!(">> stopping…");
    controller.stop_blocking();
    println!(">> done, is_running={}", psiphon::Controller::is_running());
}
