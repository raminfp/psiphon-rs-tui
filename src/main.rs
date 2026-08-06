use psiphon_tui::app::{App, ConnectionState};
use psiphon_tui::{cli, psiphon, ui};

use crossterm::event::{self, Event as CEvent, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{execute, ExecutableCommand};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::stdout;
use std::sync::mpsc::TryRecvError;
use std::time::Duration;

fn main() {
    let mut argv = std::env::args();
    argv.next(); // program name

    let parsed = match cli::parse(argv) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: {e}\n\n{}", cli::usage());
            std::process::exit(2);
        }
    };

    let args = match parsed {
        cli::ParseResult::Help => {
            print!("{}", cli::usage());
            return;
        }
        cli::ParseResult::Args(a) => a,
    };

    if let Err(e) = std::fs::create_dir_all(&args.data_root_directory) {
        eprintln!(
            "error: could not create dataRootDirectory '{}': {e}",
            args.data_root_directory
        );
        std::process::exit(1);
    }

    if let Err(e) = run(args) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

/// Spawns a dedicated thread that blocks waiting for SIGINT/SIGTERM/SIGHUP
/// and, when one arrives, stops the tunnel and force-exits the whole
/// process - independently of the render loop.
///
/// An earlier version of this used `signal_hook::flag` (just an AtomicBool
/// checked once per render-loop iteration). That is NOT enough: the render
/// loop spends most of its time inside `crossterm::event::poll`, which
/// reads from the controlling tty. When the tty itself is gone - the
/// terminal window was killed, the SSH session dropped, the tmux pane was
/// torn down - that poll can end up never returning in a timely way, so the
/// flag is never actually checked and the tunnel (plus its exclusive
/// datastore lock) is orphaned indefinitely. Verified live: `kill -TERM` on
/// a running instance, and killing its tmux pane out from under it, both
/// left the process running with the flag-only approach.
///
/// This version sidesteps that: `Signals::forever()` blocks on the
/// self-pipe signal-hook maintains internally, not on tty I/O, so it is
/// unaffected by whatever state the terminal is in.
fn spawn_shutdown_watcher() -> Result<(), std::io::Error> {
    let mut signals = signal_hook::iterator::Signals::new([
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGHUP,
    ])?;

    std::thread::spawn(move || {
        // Block until any one of the registered signals arrives.
        let _ = signals.forever().next();

        // Everything from here is best-effort: none of it may be allowed
        // to skip the tunnel shutdown call below (see the writeln!/`?`
        // reasoning in `run`).
        let _ = disable_raw_mode();
        let _ = execute!(std::io::stdout(), LeaveAlternateScreen);

        use std::io::Write as _;
        let _ = writeln!(std::io::stdout(), "\nreceived shutdown signal, stopping tunnel…");
        unsafe {
            psiphon_tui::ffi::PsiphonStop();
        }
        let _ = writeln!(std::io::stdout(), "stopped.");

        // The render loop's own thread may be stuck (see above) - don't
        // wait for it, just end the process now that the tunnel is down.
        std::process::exit(0);
    });

    Ok(())
}

fn run(args: cli::Args) -> Result<(), Box<dyn std::error::Error>> {
    let mut app = App::new(
        args.config.clone(),
        args.server_list.clone(),
        args.data_root_directory.clone(),
    );
    app.push_system("psiphon-tui starting — press 's' to (re)connect, 'q' to quit");

    // Ensure a killed/dropped terminal (closed window, lost SSH session,
    // `kill`, tmux pane torn down, ...) still stops the tunnel and releases
    // the datastore lock, instead of leaving an orphaned background process.
    // Note: this does NOT cover Ctrl+C while the TUI is running - raw mode
    // disables the tty's ISIG, so Ctrl+C arrives as a normal keypress
    // instead of SIGINT; that's handled explicitly in event_loop.
    spawn_shutdown_watcher()?;

    let (controller, notice_rx) = psiphon::Controller::new();

    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;

    // Auto-launch immediately, mirroring the original CLI's behaviour of
    // connecting as soon as it's invoked with -config/-serverList/-dataRootDirectory.
    launch(&mut app, &controller);

    let result = event_loop(&mut terminal, &mut app, &controller, notice_rx);

    // From here on, everything is best-effort: none of it may be allowed to
    // skip the tunnel shutdown below via `?`. If the terminal/tty is
    // already gone (killed window, dropped SSH session, tmux pane torn
    // down, ...) these restore calls can themselves fail - that must not
    // orphan the tunnel and its exclusive datastore lock forever. Same
    // reasoning for using writeln!+ignore instead of println! (which
    // panics on a write error) for the status messages below.
    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    let _ = terminal.show_cursor();

    // stop_blocking() is documented safe to call even if nothing is
    // running, so it's always called unconditionally here rather than
    // guarded by an is_running() check that could itself be stale.
    use std::io::Write as _;
    let _ = writeln!(std::io::stdout(), "Stopping tunnel…");
    controller.stop_blocking();
    let _ = writeln!(std::io::stdout(), "Stopped.");

    result
}

/// Starts (or restarts) the tunnel using `app`'s current config/server list/
/// data root and `app.selected_region` as the egress region filter. Shared
/// by the initial auto-connect, the 's' keybinding, and post-region-change
/// relaunches so they can't drift apart.
fn launch(app: &mut App, controller: &psiphon::Controller) {
    app.mark_starting();
    let region = app.selected_region.clone().unwrap_or_default();
    if let Err(e) = controller.launch(
        &app.config_path,
        &app.server_list_path,
        &app.data_root_directory,
        &region,
    ) {
        app.push_system(format!("failed to launch: {e}"));
        app.state = ConnectionState::Failed(e.to_string());
    }
}

fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    app: &mut App,
    controller: &psiphon::Controller,
    notice_rx: std::sync::mpsc::Receiver<psiphon::Event>,
) -> Result<(), Box<dyn std::error::Error>> {
    loop {
        // Drain any pending notices without blocking the render loop.
        loop {
            match notice_rx.try_recv() {
                Ok(psiphon::Event::Notice(n)) => app.apply_notice(n),
                Ok(psiphon::Event::Unparsed(s)) => app.push_system(format!("(unparsed) {s}")),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }

        // A region change on a running tunnel stops it first; once the
        // bridge confirms shutdown ("BridgeStopped" -> ConnectionState::Stopped),
        // relaunch with the newly selected region.
        if app.pending_relaunch && app.state == ConnectionState::Stopped {
            app.pending_relaunch = false;
            launch(app, controller);
        }

        terminal.draw(|f| ui::draw(f, app))?;

        if app.should_quit {
            break;
        }

        if event::poll(Duration::from_millis(100))? {
            if let CEvent::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }

                // Raw mode disables the tty's ISIG, so Ctrl+C arrives here
                // as a normal keypress instead of a SIGINT - handle it
                // explicitly, and let it override the region picker too
                // (Ctrl+C should always mean "get me out", full stop).
                if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    app.should_quit = true;
                    continue;
                }

                if app.region_picker_open {
                    handle_region_picker_key(app, controller, key.code);
                    continue;
                }

                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => app.should_quit = true,
                    KeyCode::Char('s') => {
                        if app.can_launch() {
                            launch(app, controller);
                        }
                    }
                    KeyCode::Char('x') => {
                        if psiphon::Controller::is_running() && app.state != ConnectionState::Stopping {
                            app.mark_stopping();
                            controller.stop_async();
                        }
                    }
                    KeyCode::Char('r') => app.open_region_picker(),
                    KeyCode::Up | KeyCode::Char('k') => app.scroll_up = app.scroll_up.saturating_add(1),
                    KeyCode::Down | KeyCode::Char('j') => app.scroll_up = app.scroll_up.saturating_sub(1),
                    KeyCode::PageUp => app.scroll_up = app.scroll_up.saturating_add(10),
                    KeyCode::PageDown => app.scroll_up = app.scroll_up.saturating_sub(10),
                    KeyCode::End => app.scroll_up = 0,
                    KeyCode::Home => app.scroll_up = usize::MAX / 2,
                    _ => {}
                }
            }
        }
    }

    Ok(())
}

fn handle_region_picker_key(app: &mut App, controller: &psiphon::Controller, code: KeyCode) {
    match code {
        KeyCode::Esc | KeyCode::Char('q') => app.region_picker_open = false,
        KeyCode::Up | KeyCode::Char('k') => app.move_region_picker(-1),
        KeyCode::Down | KeyCode::Char('j') => app.move_region_picker(1),
        KeyCode::Enter => {
            let items = app.region_picker_items();
            let choice = items
                .get(app.region_picker_index)
                .cloned()
                .unwrap_or_else(|| "Any".to_string());
            app.region_picker_open = false;

            let new_region = if choice == "Any" { None } else { Some(choice) };
            if new_region == app.selected_region {
                // No actual change - don't interrupt an active tunnel for
                // nothing.
                return;
            }
            app.selected_region = new_region;
            app.push_system(format!(
                "region filter changed to {} — reconnecting…",
                app.selected_region.as_deref().unwrap_or("Any")
            ));

            if app.can_launch() {
                launch(app, controller);
            } else {
                app.pending_relaunch = true;
                app.mark_stopping();
                controller.stop_async();
            }
        }
        _ => {}
    }
}
