//! ratatui rendering for the app state in [`crate::app::App`].

use crate::app::{App, ConnectionState};
use crate::notice::Severity;
use crate::regions;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

fn state_color(state: &ConnectionState) -> Color {
    match state {
        ConnectionState::Idle => Color::Gray,
        ConnectionState::Starting | ConnectionState::Connecting => Color::Yellow,
        ConnectionState::Connected => Color::Green,
        ConnectionState::Reconnecting => Color::Yellow,
        ConnectionState::Stopping => Color::Yellow,
        ConnectionState::Stopped => Color::Gray,
        ConnectionState::Failed(_) => Color::Red,
    }
}

fn severity_style(sev: Severity) -> Style {
    match sev {
        Severity::Info => Style::default(),
        Severity::Warning => Style::default().fg(Color::Yellow),
        Severity::Error => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
    }
}

pub fn draw(frame: &mut Frame, app: &App) {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // header
            Constraint::Length(7), // stats
            Constraint::Min(5),    // log
            Constraint::Length(1), // footer
        ])
        .split(frame.area());

    draw_header(frame, app, root[0]);
    draw_stats(frame, app, root[1]);
    draw_log(frame, app, root[2]);
    draw_footer(frame, app, root[3]);

    if app.region_picker_open {
        draw_region_picker(frame, app);
    }
}

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let color = state_color(&app.state);
    let title = Line::from(vec![
        Span::styled(" psiphon-tui ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("── "),
        Span::styled(
            format!(" {} ", app.state.label()),
            Style::default()
                .fg(Color::Black)
                .bg(color)
                .add_modifier(Modifier::BOLD),
        ),
    ]);
    let subtitle = Line::from(vec![Span::styled(
        format!(
            "config: {}   serverList: {}   dataRootDirectory: {}",
            display_or(&app.config_path),
            display_or(&app.server_list_path),
            display_or(&app.data_root_directory)
        ),
        Style::default().fg(Color::DarkGray),
    )]);
    let block = Block::default().borders(Borders::ALL);
    frame.render_widget(
        Paragraph::new(vec![title, subtitle]).block(block),
        area,
    );
}

fn display_or(s: &str) -> &str {
    if s.is_empty() {
        "(none)"
    } else {
        s
    }
}

fn draw_stats(frame: &mut Frame, app: &App, area: Rect) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    let mut left = vec![
        line_kv("SOCKS proxy", opt_port(app.socks_port)),
        line_kv("HTTP proxy", opt_port(app.http_port)),
        line_kv("Active tunnels", app.tunnels_count.to_string()),
    ];
    if let Some(msg) = &app.last_error {
        // Red when it actually reflects the current (failed) state; a
        // dimmer color when the tunnel is otherwise fine and this is just
        // the most recent background hiccup (e.g. a secondary fetcher) for
        // context - see app.rs's "Error" notice handling.
        let style = if matches!(app.state, ConnectionState::Failed(_)) {
            Style::default().fg(Color::Red)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        left.push(Line::from(Span::styled(format!("Last error: {msg}"), style)));
    }
    frame.render_widget(
        Paragraph::new(left)
            .block(Block::default().title(" Proxy ").borders(Borders::ALL))
            .wrap(Wrap { trim: true }),
        cols[0],
    );

    let region_filter = match &app.selected_region {
        Some(code) => regions::display_region(code),
        None => "Any".to_string(),
    };
    let right = vec![
        line_kv("Client region", app.client_region.clone().unwrap_or_else(|| "?".into())),
        line_kv("Server region", app.server_region.clone().unwrap_or_else(|| "?".into())),
        line_kv("Region filter", format!("{region_filter}  (r to change)")),
        line_kv(
            "Transferred",
            format!("{} sent / {} recv", human_bytes(app.total_sent), human_bytes(app.total_received)),
        ),
        line_kv("Homepages", app.homepages.len().to_string()),
    ];
    frame.render_widget(
        Paragraph::new(right)
            .block(Block::default().title(" Session ").borders(Borders::ALL))
            .wrap(Wrap { trim: true }),
        cols[1],
    );
}

fn line_kv(key: &str, value: impl Into<String>) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{key:<15}"), Style::default().fg(Color::DarkGray)),
        Span::raw(value.into()),
    ])
}

fn opt_port(p: Option<u16>) -> String {
    match p {
        Some(p) => format!("127.0.0.1:{p}"),
        None => "-".to_string(),
    }
}

fn human_bytes(n: i64) -> String {
    let n = n as f64;
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = n;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

fn draw_log(frame: &mut Frame, app: &App, area: Rect) {
    let inner_height = area.height.saturating_sub(2) as usize; // minus borders
    let total = app.log.len();
    let max_scroll = total.saturating_sub(inner_height);
    let scroll_up = app.scroll_up.min(max_scroll);
    let end = total.saturating_sub(scroll_up);
    let start = end.saturating_sub(inner_height);

    let lines: Vec<Line> = app
        .log
        .iter()
        .skip(start)
        .take(end - start)
        .map(|l| {
            let ts = if l.timestamp.is_empty() {
                String::new()
            } else {
                format!("{} ", short_timestamp(&l.timestamp))
            };
            Line::from(vec![
                Span::styled(ts, Style::default().fg(Color::DarkGray)),
                Span::styled(l.text.clone(), severity_style(l.severity)),
            ])
        })
        .collect();

    let title = if scroll_up > 0 {
        format!(" Log (scrolled, {scroll_up} from bottom — End to jump to live) ")
    } else {
        " Log (live) ".to_string()
    };

    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::default().title(title).borders(Borders::ALL))
            .wrap(Wrap { trim: true }),
        area,
    );
}

/// Psiphon timestamps are RFC3339; show just the time-of-day portion to
/// keep log lines compact.
fn short_timestamp(ts: &str) -> &str {
    if let Some(t_pos) = ts.find('T') {
        let rest = &ts[t_pos + 1..];
        let end = rest.find(['.', 'Z', '+']).unwrap_or(rest.len());
        &rest[..end]
    } else {
        ts
    }
}

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let mut spans = vec![Span::styled(
        if app.can_launch() { " s:start " } else { " x:stop " },
        Style::default().add_modifier(Modifier::REVERSED),
    )];
    spans.push(Span::raw("  "));
    spans.push(Span::styled(" r:region ", Style::default().add_modifier(Modifier::REVERSED)));
    spans.push(Span::raw("  "));
    spans.push(Span::styled(" q:quit ", Style::default().add_modifier(Modifier::REVERSED)));
    spans.push(Span::raw("  "));
    spans.push(Span::styled(" ↑/↓ j/k PgUp/PgDn:scroll  End:jump to live ", Style::default().fg(Color::DarkGray)));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Centers a `width`x`height`-cell box within `area`.
fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    Rect::new(x, y, width, height)
}

fn draw_region_picker(frame: &mut Frame, app: &App) {
    let items = app.region_picker_items();
    let popup_area = centered_rect(46, (items.len() as u16 + 4).min(20), frame.area());

    frame.render_widget(Clear, popup_area);

    let list_items: Vec<ListItem> = items
        .iter()
        .map(|code| {
            let label = if code == "Any" {
                "Any (no preference)".to_string()
            } else {
                regions::display_region(code)
            };
            let is_current = app.selected_region.as_deref() == Some(code.as_str())
                || (code == "Any" && app.selected_region.is_none());
            let prefix = if is_current { "● " } else { "  " };
            ListItem::new(format!("{prefix}{label}"))
        })
        .collect();

    let title = if app.available_regions.is_empty() {
        " Region (only 'Any' available — connect once to discover more) "
    } else {
        " Region — Enter to apply, Esc to cancel "
    };

    let list = List::new(list_items)
        .block(Block::default().title(title).borders(Borders::ALL))
        .highlight_style(
            Style::default()
                .bg(Color::Blue)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        );

    let mut list_state = ListState::default();
    list_state.select(Some(app.region_picker_index));

    frame.render_stateful_widget(list, popup_area, &mut list_state);
}
