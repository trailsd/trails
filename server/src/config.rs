//! Server configuration — all from environment variables.

use std::env;

#[derive(Debug, Clone)]
pub struct Config {
    /// Postgres connection string.
    pub database_url: String,
    /// Listen address for WebSocket + REST.
    pub listen_addr: String,
    /// Server instance name (node identity for reconnection tracking).
    pub server_instance: String,
    /// Default start deadline in seconds (spec §7).
    pub default_start_deadline: i32,
    /// Reconnection window in seconds after server restart (spec §19).
    pub reconnect_window: u64,
    /// Log level filter.
    pub log_level: String,
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            database_url: env::var("DATABASE_URL")
                .unwrap_or_else(|_| "postgres://trails:trails@localhost:5432/trails".into()),
            listen_addr: env::var("LISTEN_ADDR").unwrap_or_else(|_| "0.0.0.0:8443".into()),
            server_instance: env::var("SERVER_INSTANCE")
                .unwrap_or_else(|_| hostname()),
            default_start_deadline: env::var("DEFAULT_START_DEADLINE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(300),
            reconnect_window: env::var("RECONNECT_WINDOW")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(60),
            log_level: env::var("RUST_LOG")
                .unwrap_or_else(|_| "trailsd=info,tower_http=info".into()),
        }
    }
}

fn hostname() -> String {
    hostname::get()
        .map(|h| h.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "unknown".into())
}
