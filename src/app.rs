//! Application state: everything the UI needs to render, and the logic that
//! turns incoming [`Notice`] events into that state.

use crate::notice::{Notice, Severity};
use std::collections::VecDeque;
use std::time::Instant;

pub const MAX_LOG_LINES: usize = 4000;

#[derive(Debug, Clone, PartialEq)]
pub enum ConnectionState {
    Idle,
    Starting,
    Connecting,
    Connected,
    Reconnecting,
    Stopping,
    Stopped,
    Failed(String),
}

impl ConnectionState {
    pub fn label(&self) -> String {
        match self {
            ConnectionState::Idle => "IDLE".to_string(),
            ConnectionState::Starting => "STARTING".to_string(),
            ConnectionState::Connecting => "CONNECTING".to_string(),
            ConnectionState::Connected => "CONNECTED".to_string(),
            ConnectionState::Reconnecting => "RECONNECTING".to_string(),
            ConnectionState::Stopping => "STOPPING".to_string(),
            ConnectionState::Stopped => "STOPPED".to_string(),
            ConnectionState::Failed(_) => "ERROR".to_string(),
        }
    }
}

pub struct LogLine {
    pub timestamp: String,
    pub severity: Severity,
    pub text: String,
}

pub struct App {
    pub config_path: String,
    pub server_list_path: String,
    pub data_root_directory: String,

    pub state: ConnectionState,
    pub log: VecDeque<LogLine>,
    /// Lines scrolled up from the bottom; 0 means pinned to the latest line.
    pub scroll_up: usize,

    pub socks_port: Option<u16>,
    pub http_port: Option<u16>,
    pub client_region: Option<String>,
    pub server_region: Option<String>,
    pub homepages: Vec<String>,
    pub tunnels_count: i64,
    pub session_id: Option<String>,
    pub total_sent: i64,
    pub total_received: i64,

    pub started_at: Option<Instant>,
    pub connected_at: Option<Instant>,

    /// Most recent "Error"/"BridgeError" notice text, kept for display even
    /// while otherwise Connected (see apply_notice's "Error" handling).
    pub last_error: Option<String>,
    /// The tunnel protocol actually in use (e.g. "TLS-OSSH",
    /// "INPROXY-WEBRTC-OSSH"), from the "ActiveTunnel" notice.
    pub active_protocol: Option<String>,

    /// Region codes Psiphon itself has reported as available (from the
    /// "AvailableEgressRegions" notice, which reflects the server entries
    /// the client actually has - not a static/hardcoded list).
    pub available_regions: Vec<String>,
    /// Currently applied egress region filter; None means "Any".
    pub selected_region: Option<String>,
    pub region_picker_open: bool,
    pub region_picker_index: usize,
    /// Set when a region change requires a stop+relaunch cycle; consumed
    /// once the "BridgeStopped" notice confirms the old tunnel is down.
    pub pending_relaunch: bool,

    pub should_quit: bool,
}

impl App {
    pub fn new(config_path: String, server_list_path: String, data_root_directory: String) -> Self {
        App {
            config_path,
            server_list_path,
            data_root_directory,
            state: ConnectionState::Idle,
            log: VecDeque::new(),
            scroll_up: 0,
            socks_port: None,
            http_port: None,
            client_region: None,
            server_region: None,
            homepages: Vec::new(),
            tunnels_count: 0,
            session_id: None,
            total_sent: 0,
            total_received: 0,
            started_at: None,
            connected_at: None,
            last_error: None,
            active_protocol: None,
            available_regions: Vec::new(),
            selected_region: None,
            region_picker_open: false,
            region_picker_index: 0,
            pending_relaunch: false,
            should_quit: false,
        }
    }

    /// Whether it's currently safe to call `Controller::launch` - i.e. the
    /// bridge is definitely not mid-connection or mid-teardown. Used to
    /// guard both the manual 's' keybinding and region-change relaunches;
    /// without excluding `Stopping`, pressing 's' right after 'x' could
    /// race the Go side's shutdown (see bridge.go's ResetNoticeWriter
    /// ordering notes).
    pub fn can_launch(&self) -> bool {
        matches!(
            self.state,
            ConnectionState::Idle | ConnectionState::Stopped | ConnectionState::Failed(_)
        )
    }

    /// Items shown in the region picker: "Any" followed by whatever regions
    /// Psiphon has actually reported.
    pub fn region_picker_items(&self) -> Vec<String> {
        let mut items = vec!["Any".to_string()];
        items.extend(self.available_regions.iter().cloned());
        items
    }

    pub fn open_region_picker(&mut self) {
        let items = self.region_picker_items();
        self.region_picker_index = match &self.selected_region {
            Some(region) => items.iter().position(|r| r == region).unwrap_or(0),
            None => 0,
        };
        self.region_picker_open = true;
    }

    pub fn move_region_picker(&mut self, delta: isize) {
        let len = self.region_picker_items().len();
        if len == 0 {
            return;
        }
        let current = self.region_picker_index as isize;
        let next = (current + delta).rem_euclid(len as isize);
        self.region_picker_index = next as usize;
    }

    pub fn push_log(&mut self, severity: Severity, timestamp: String, text: String) {
        self.log.push_back(LogLine {
            timestamp,
            severity,
            text,
        });
        if self.log.len() > MAX_LOG_LINES {
            self.log.pop_front();
        }
    }

    pub fn push_system(&mut self, text: impl Into<String>) {
        self.push_log(Severity::Info, String::new(), text.into());
    }

    pub fn mark_starting(&mut self) {
        self.state = ConnectionState::Starting;
        self.started_at = Some(Instant::now());
        self.connected_at = None;
        self.socks_port = None;
        self.http_port = None;
        self.tunnels_count = 0;
        self.last_error = None;
        self.active_protocol = None;
    }

    pub fn mark_stopping(&mut self) {
        self.state = ConnectionState::Stopping;
    }

    pub fn apply_notice(&mut self, notice: Notice) {
        let summary = notice.summary();
        self.push_log(notice.severity(), notice.timestamp.clone(), summary.clone());

        match notice.notice_type.as_str() {
            "BridgeStarting" => {
                if self.started_at.is_none() {
                    self.started_at = Some(Instant::now());
                }
            }
            "ConnectingServer" | "RequestingTactics" => {
                if self.state != ConnectionState::Connected {
                    self.state = ConnectionState::Connecting;
                }
            }
            "Tunnels" => {
                let count = notice.data.get("count").and_then(|v| v.as_i64()).unwrap_or(0);
                self.tunnels_count = count;
                if count > 0 {
                    if self.state != ConnectionState::Connected {
                        self.connected_at = Some(Instant::now());
                    }
                    self.state = ConnectionState::Connected;
                } else if self.state == ConnectionState::Connected {
                    self.state = ConnectionState::Reconnecting;
                    self.active_protocol = None;
                }
            }
            "ActiveTunnel" => {
                self.active_protocol = notice
                    .data
                    .get("protocol")
                    .and_then(|v| v.as_str())
                    .map(String::from);
            }
            "ListeningSocksProxyPort" => {
                self.socks_port = notice
                    .data
                    .get("port")
                    .and_then(|v| v.as_u64())
                    .map(|p| p as u16);
            }
            "ListeningHttpProxyPort" => {
                self.http_port = notice
                    .data
                    .get("port")
                    .and_then(|v| v.as_u64())
                    .map(|p| p as u16);
            }
            "ClientRegion" => {
                self.client_region = notice
                    .data
                    .get("region")
                    .and_then(|v| v.as_str())
                    .map(String::from);
            }
            "AvailableEgressRegions" => {
                if let Some(regions) = notice.data.get("regions").and_then(|v| v.as_array()) {
                    self.available_regions = regions
                        .iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect();
                    self.available_regions.sort();
                }
            }
            "ConnectedServerRegion" => {
                self.server_region = notice
                    .data
                    .get("serverRegion")
                    .and_then(|v| v.as_str())
                    .map(String::from);
            }
            "Homepage" => {
                if let Some(url) = notice.data.get("url").and_then(|v| v.as_str()) {
                    self.homepages.push(url.to_string());
                }
            }
            "SessionId" => {
                self.session_id = notice
                    .data
                    .get("sessionId")
                    .and_then(|v| v.as_str())
                    .map(String::from);
            }
            "TotalBytesTransferred" => {
                if let Some(sent) = notice.data.get("sent").and_then(|v| v.as_i64()) {
                    self.total_sent = sent;
                }
                if let Some(received) = notice.data.get("received").and_then(|v| v.as_i64()) {
                    self.total_received = received;
                }
            }
            "BridgeStopped" => {
                self.state = ConnectionState::Stopped;
                self.socks_port = None;
                self.http_port = None;
                self.tunnels_count = 0;
                self.active_protocol = None;
            }
            "BridgeError" => {
                // Always fatal to the current launch attempt - these come
                // from the bridge itself (bad config path, commit failure,
                // controller creation failure), never from a background
                // fetcher inside an already-running tunnel.
                self.last_error = Some(summary.clone());
                self.state = ConnectionState::Failed(summary);
            }
            "Error" => {
                // Generic Psiphon-internal error notices cover everything
                // from "couldn't connect at all" to "a secondary background
                // fetcher (e.g. DSL) hit a hiccup while the main tunnel is
                // fine". Only treat it as connection-fatal when there is no
                // active tunnel right now; otherwise just surface it as
                // informational context without hiding a working connection
                // behind a scary ERROR badge.
                self.last_error = Some(summary.clone());
                if self.tunnels_count == 0 {
                    self.state = ConnectionState::Failed(summary);
                }
            }
            _ => {}
        }
    }
}
