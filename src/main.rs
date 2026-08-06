use psiphon_tui::app::{App, ConnectionState};
use psiphon_tui::{cli, psiphon, ui};

use crossterm::event::{self, Event as CEvent, KeyCode, KeyEventKind};
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

fn run(args: cli::Args) -> Result<(), Box<dyn std::error::Error>> {
    let mut app = App::new(
        args.config.clone(),
        args.server_list.clone(),
        args.data_root_directory.clone(),
    );
    app.push_system("psiphon-tui starting — press 's' to (re)connect, 'q' to quit");

    let (controller, notice_rx) = psiphon::Controller::new();

    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;

    // Auto-launch immediately, mirroring the original CLI's behaviour of
    // connecting as soon as it's invoked with -config/-serverList/-dataRootDirectory.
    launch(&mut app, &controller);

    let result = event_loop(&mut terminal, &mut app, &controller, notice_rx);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    // Graceful shutdown: now that the terminal is restored, block (bounded)
    // until the bridge confirms the tunnel is fully stopped.
    if psiphon::Controller::is_running() {
        println!("Stopping tunnel…");
        controller.stop_blocking();
        println!("Stopped.");
    }

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
