//! Parsing and presentation of Psiphon "notices" — the JSON event stream
//! every Psiphon client (Android/iOS/Windows/ConsoleClient) uses to report
//! progress. See psiphon/notice.go in the vendored source for the full,
//! authoritative list of notice types and their data fields.
//!
//! A handful of `Bridge*` notice types are synthesized by our own Go bridge
//! (RustBridge/bridge.go) and are handled the same way as upstream ones.

use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, Deserialize)]
pub struct Notice {
    #[serde(rename = "noticeType")]
    pub notice_type: String,
    #[serde(default)]
    pub timestamp: String,
    #[serde(default)]
    pub data: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Info,
    Warning,
    Error,
}

impl Notice {
    pub fn parse(raw: &str) -> Option<Notice> {
        serde_json::from_str(raw).ok()
    }

    pub fn severity(&self) -> Severity {
        match self.notice_type.as_str() {
            "Error" | "BridgeError" | "UpstreamProxyError" | "LocalProxyError"
            | "InproxyMustUpgrade" => Severity::Error,
            "Warning" | "SocksProxyPortInUse" | "HttpProxyPortInUse"
            | "EstablishTunnelTimeout" | "PruneServerEntry" | "SkipServerEntry" => {
                Severity::Warning
            }
            _ => Severity::Info,
        }
    }

    fn str_field(&self, key: &str) -> Option<&str> {
        self.data.get(key).and_then(Value::as_str)
    }

    fn i64_field(&self, key: &str) -> Option<i64> {
        self.data.get(key).and_then(Value::as_i64)
    }

    /// A single human-readable line for the live log pane. Falls back to a
    /// generic "<Type>: <data>" rendering for notice types we don't
    /// special-case.
    pub fn summary(&self) -> String {
        match self.notice_type.as_str() {
            "Info" | "Warning" | "Error" | "UserLog" => self
                .str_field("message")
                .unwrap_or_default()
                .to_string(),
            "ConnectingServer" => "connecting to a candidate server…".to_string(),
            "ConnectedServer" => "connected to a candidate server".to_string(),
            "ActiveTunnel" => format!(
                "active tunnel established (protocol: {})",
                self.str_field("protocol").unwrap_or("?")
            ),
            "ConnectedServerRegion" => format!(
                "connected server region: {}",
                self.str_field("serverRegion").unwrap_or("?")
            ),
            "ClientRegion" => format!("client region: {}", self.str_field("region").unwrap_or("?")),
            "ClientAddress" => "client public address determined".to_string(),
            "Tunnels" => format!(
                "active tunnel count: {}",
                self.i64_field("count").unwrap_or(0)
            ),
            "ListeningSocksProxyPort" => {
                format!("local SOCKS proxy listening on 127.0.0.1:{}", self.i64_field("port").unwrap_or(0))
            }
            "ListeningHttpProxyPort" => {
                format!("local HTTP proxy listening on 127.0.0.1:{}", self.i64_field("port").unwrap_or(0))
            }
            "SocksProxyPortInUse" => format!(
                "requested SOCKS proxy port {} is already in use",
                self.i64_field("port").unwrap_or(0)
            ),
            "HttpProxyPortInUse" => format!(
                "requested HTTP proxy port {} is already in use",
                self.i64_field("port").unwrap_or(0)
            ),
            "Homepage" => format!("sponsor homepage: {}", self.str_field("url").unwrap_or("?")),
            "ClientUpgradeAvailable" => format!(
                "client upgrade available: {}",
                self.str_field("version").unwrap_or("?")
            ),
            "EstablishTunnelTimeout" => "timed out trying to establish a tunnel".to_string(),
            "UpstreamProxyError" => format!(
                "upstream proxy error: {}",
                self.str_field("message").unwrap_or("?")
            ),
            "LocalProxyError" => format!(
                "local proxy error: {}",
                self.str_field("message").unwrap_or("?")
            ),
            "Untunneled" => "an address was accessed directly (untunneled)".to_string(),
            "BytesTransferred" => format!(
                "transferred: +{} sent / +{} received bytes",
                self.i64_field("sent").unwrap_or(0),
                self.i64_field("received").unwrap_or(0)
            ),
            "TotalBytesTransferred" => format!(
                "total transferred: {} sent / {} received bytes",
                self.i64_field("sent").unwrap_or(0),
                self.i64_field("received").unwrap_or(0)
            ),
            "Exiting" => "shutting down".to_string(),
            "BridgeStarting" => "starting tunnel…".to_string(),
            "BridgeStopped" => "tunnel stopped".to_string(),
            "BridgeError" => self
                .str_field("message")
                .unwrap_or("unknown bridge error")
                .to_string(),
            other => {
                if self.data.is_null() || self.data == Value::Object(Default::default()) {
                    other.to_string()
                } else {
                    format!("{other}: {}", self.data)
                }
            }
        }
    }
}
