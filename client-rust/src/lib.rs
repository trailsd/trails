//! TRAILS Rust client library.
//!
//! Two lines to integrate:
//! ```ignore
//! let g = TrailsClient::init().await;
//! g.status(json!({"phase": "processing", "progress": 0.5})).await;
//! ```
//!
//! If `TRAILS_INFO` is absent, `init()` returns a no-op client where all
//! methods silently succeed. Zero overhead.
//!
//! See TRAILS-SPEC.md §24 for the full API surface.

use std::env;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

// ═══════════════════════════════════════════════════════════════
// Public types
// ═══════════════════════════════════════════════════════════════

/// Decoded TRAILS_INFO envelope (spec §5).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrailsConfig {
    pub v: i32,
    pub app_id: Uuid,
    pub parent_id: Option<Uuid>,
    pub app_name: String,
    pub server_ep: String,
    #[serde(default)]
    pub server_pub_key: Option<String>,
    #[serde(default = "default_sec_level")]
    pub sec_level: String,
    #[serde(default)]
    pub scheduled_at: Option<i64>,
    #[serde(default)]
    pub start_deadline: Option<i32>,
    #[serde(default)]
    pub originator: Option<Originator>,
    #[serde(default)]
    pub role_refs: Vec<String>,
    #[serde(default)]
    pub tags: Option<JsonValue>,
}

fn default_sec_level() -> String {
    "open".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Originator {
    pub sub: Option<String>,
    pub groups: Option<Vec<String>>,
}

#[derive(Debug)]
pub enum TrailsError {
    /// TRAILS_INFO missing or invalid.
    NoConfig,
    /// WebSocket connection failed.
    ConnectionFailed(String),
    /// Channel closed (background task died).
    ChannelClosed,
    /// Server returned error.
    ServerError(String),
    /// Serialization error.
    Serialize(String),
}

impl std::fmt::Display for TrailsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoConfig => write!(f, "TRAILS_INFO not set"),
            Self::ConnectionFailed(e) => write!(f, "connection failed: {e}"),
            Self::ChannelClosed => write!(f, "background task stopped"),
            Self::ServerError(e) => write!(f, "server error: {e}"),
            Self::Serialize(e) => write!(f, "serialize error: {e}"),
        }
    }
}

impl std::error::Error for TrailsError {}

// ═══════════════════════════════════════════════════════════════
// Client
// ═══════════════════════════════════════════════════════════════

/// TRAILS client. Send status, results, and errors to the TRAILS server.
///
/// Internally spawns a background tokio task that manages the WebSocket
/// connection, including reconnection with exponential backoff + jitter.
/// Methods send messages through a channel — they never block on I/O.
///
/// If TRAILS_INFO was absent, this is a no-op client: all methods return
/// Ok(()) immediately with zero overhead.
pub struct TrailsClient {
    inner: Option<ClientInner>,
}

struct ClientInner {
    config: TrailsConfig,
    tx: mpsc::Sender<Outbound>,
    seq: AtomicI64,
    connected: Arc<AtomicBool>,
    signing_key: SigningKey,
}

/// Message sent from API methods to the background task.
enum Outbound {
    Data {
        msg_type: &'static str,
        seq: i64,
        payload: JsonValue,
        correlation_id: Option<String>,
    },
    Disconnect {
        reason: String,
    },
}

impl TrailsClient {
    /// Read TRAILS_INFO from environment, connect to server.
    /// Returns no-op client if TRAILS_INFO is absent.
    pub async fn init() -> Self {
        match env::var("TRAILS_INFO") {
            Ok(b64) => match Self::decode_config(&b64) {
                Ok(config) => Self::init_with(config).await,
                Err(e) => {
                    warn!("TRAILS_INFO decode failed: {e}, using no-op client");
                    Self { inner: None }
                }
            },
            Err(_) => {
                debug!("TRAILS_INFO not set, using no-op client");
                Self { inner: None }
            }
        }
    }

    /// Initialize with explicit config (for non-env-var delivery, spec §5).
    pub async fn init_with(config: TrailsConfig) -> Self {
        let mut rng = rand::thread_rng();
        let signing_key = SigningKey::generate(&mut rng);
        let connected = Arc::new(AtomicBool::new(false));

        let (tx, rx) = mpsc::channel::<Outbound>(256);

        // Spawn background WebSocket task.
        let bg_config = config.clone();
        let bg_key = SigningKey::from_bytes(&signing_key.to_bytes());
        let bg_connected = Arc::clone(&connected);
        tokio::spawn(async move {
            ws_task(bg_config, bg_key, rx, bg_connected).await;
        });

        Self {
            inner: Some(ClientInner {
                config,
                tx,
                seq: AtomicI64::new(0),
                connected,
                signing_key,
            }),
        }
    }

    /// Whether this is a real client (not no-op).
    pub fn is_active(&self) -> bool {
        self.inner.is_some()
    }

    /// Whether the WebSocket is currently connected.
    pub fn is_connected(&self) -> bool {
        self.inner
            .as_ref()
            .map(|i| i.connected.load(Ordering::Relaxed))
            .unwrap_or(false)
    }

    /// Send a status update (spec §9).
    pub async fn status(&self, payload: JsonValue) -> Result<(), TrailsError> {
        self.send_data("Status", payload, None).await
    }

    /// Send a business result (spec §9). Transitions app to 'done'.
    pub async fn result(&self, payload: JsonValue) -> Result<(), TrailsError> {
        self.send_data("Result", payload, None).await
    }

    /// Send a structured error (spec §9). Transitions app to 'error'.
    pub async fn error(&self, msg: &str, detail: Option<JsonValue>) -> Result<(), TrailsError> {
        let payload = serde_json::json!({
            "message": msg,
            "detail": detail,
        });
        self.send_data("Error", payload, None).await
    }

    /// Generate TRAILS_INFO config for a child (spec §7, Phase A light).
    /// Note: In Phase 1, this only creates the config. Phase 2 adds
    /// POST /api/v1/children server-side pre-registration.
    pub fn create_child(&self, name: &str) -> Result<TrailsConfig, TrailsError> {
        let inner = self.inner.as_ref().ok_or(TrailsError::NoConfig)?;
        let child_id = Uuid::new_v4();
        Ok(TrailsConfig {
            v: 1,
            app_id: child_id,
            parent_id: Some(inner.config.app_id),
            app_name: name.into(),
            server_ep: inner.config.server_ep.clone(),
            server_pub_key: inner.config.server_pub_key.clone(),
            sec_level: inner.config.sec_level.clone(),
            scheduled_at: Some(chrono::Utc::now().timestamp_millis()),
            start_deadline: inner.config.start_deadline,
            originator: inner.config.originator.clone(),
            role_refs: inner.config.role_refs.clone(),
            tags: None,
        })
    }

    /// Encode a TrailsConfig as base64 TRAILS_INFO string.
    pub fn encode_config(config: &TrailsConfig) -> Result<String, TrailsError> {
        let json = serde_json::to_string(config).map_err(|e| TrailsError::Serialize(e.to_string()))?;
        Ok(base64::engine::general_purpose::STANDARD.encode(json.as_bytes()))
    }

    /// Graceful shutdown. Sends disconnect message, closes connection.
    pub async fn shutdown(self) -> Result<(), TrailsError> {
        if let Some(inner) = &self.inner {
            let _ = inner
                .tx
                .send(Outbound::Disconnect {
                    reason: "completed".into(),
                })
                .await;
            // Give the background task a moment to send the disconnect.
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Ok(())
    }

    // ── Internal ────────────────────────────────────────────

    fn decode_config(b64: &str) -> Result<TrailsConfig, TrailsError> {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64.trim())
            .map_err(|e| TrailsError::Serialize(format!("base64 decode: {e}")))?;
        let config: TrailsConfig =
            serde_json::from_slice(&bytes).map_err(|e| TrailsError::Serialize(format!("JSON: {e}")))?;
        Ok(config)
    }

    async fn send_data(
        &self,
        msg_type: &'static str,
        payload: JsonValue,
        correlation_id: Option<String>,
    ) -> Result<(), TrailsError> {
        let inner = match &self.inner {
            Some(i) => i,
            None => return Ok(()), // no-op client
        };

        let seq = inner.seq.fetch_add(1, Ordering::Relaxed) + 1;

        // Spec §19: fail silently during disconnection.
        let _ = inner
            .tx
            .try_send(Outbound::Data {
                msg_type,
                seq,
                payload,
                correlation_id,
            })
            .map_err(|_| {
                debug!("message dropped (disconnected or channel full)");
            });

        Ok(())
    }
}

// ═══════════════════════════════════════════════════════════════
// Background WebSocket task
// ═══════════════════════════════════════════════════════════════

/// Wire protocol: client → server messages.
#[derive(Serialize)]
struct WireRegister {
    r#type: &'static str,
    app_id: Uuid,
    parent_id: Option<Uuid>,
    app_name: String,
    child_pub_key: String,
    process_info: WireProcessInfo,
    role_refs: Vec<String>,
    sig: Option<String>,
}

#[derive(Serialize)]
struct WireReRegister {
    r#type: &'static str,
    app_id: Uuid,
    last_seq: i64,
    pub_key: String,
    sig: Option<String>,
}

#[derive(Serialize)]
struct WireDataMsg {
    r#type: &'static str,
    app_id: Uuid,
    header: WireHeader,
    payload: JsonValue,
    sig: Option<String>,
}

#[derive(Serialize)]
struct WireHeader {
    msg_type: String,
    timestamp: i64,
    seq: i64,
    correlation_id: Option<String>,
}

#[derive(Serialize)]
struct WireDisconnect {
    r#type: &'static str,
    app_id: Uuid,
    reason: String,
}

#[derive(Serialize)]
struct WireProcessInfo {
    pid: i32,
    ppid: i32,
    uid: i32,
    gid: i32,
    hostname: String,
    node_name: Option<String>,
    pod_ip: Option<String>,
    namespace: Option<String>,
    start_time: Option<i64>,
    executable: Option<String>,
}

/// Collect process info from the OS (spec §6).
fn collect_process_info() -> WireProcessInfo {
    WireProcessInfo {
        pid: std::process::id() as i32,
        ppid: 0, // platform-specific; 0 is safe default
        uid: 0,  // platform-specific
        gid: 0,
        hostname: hostname::get()
            .map(|h| h.to_string_lossy().into_owned())
            .unwrap_or_default(),
        node_name: env::var("NODE_NAME").ok(),
        pod_ip: env::var("POD_IP").ok(),
        namespace: env::var("POD_NAMESPACE")
            .ok()
            .or_else(|| read_k8s_namespace()),
        start_time: Some(chrono::Utc::now().timestamp_millis()),
        executable: env::current_exe()
            .ok()
            .map(|p| p.to_string_lossy().into_owned()),
    }
}

fn read_k8s_namespace() -> Option<String> {
    std::fs::read_to_string("/var/run/secrets/kubernetes.io/serviceaccount/namespace")
        .ok()
        .map(|s| s.trim().to_string())
}

fn pub_key_string(key: &SigningKey) -> String {
    let pub_bytes = key.verifying_key().to_bytes();
    let b64 = base64::engine::general_purpose::STANDARD.encode(pub_bytes);
    format!("ed25519:{b64}")
}

/// Convert server_ep URL to a ws:// URL suitable for tungstenite.
/// Handles: ws://, wss://, http://, https://
fn normalize_ws_url(ep: &str) -> String {
    let url = ep
        .replace("https://", "wss://")
        .replace("http://", "ws://");
    // Ensure /ws path if not present.
    if !url.contains("/ws") {
        format!("{url}/ws")
    } else {
        url
    }
}

/// Background task: owns the WebSocket, handles send/recv, reconnects.
async fn ws_task(
    config: TrailsConfig,
    signing_key: SigningKey,
    mut rx: mpsc::Receiver<Outbound>,
    connected: Arc<AtomicBool>,
) {
    let ws_url = normalize_ws_url(&config.server_ep);
    let pub_key = pub_key_string(&signing_key);
    let mut attempt: u32 = 0;
    let mut last_seq: i64 = 0;
    let mut first_connect = true;

    loop {
        // ── Connect ─────────────────────────────────────────
        let ws_stream = match tokio_tungstenite::connect_async(&ws_url).await {
            Ok((stream, _)) => {
                info!(url = %ws_url, "WebSocket connected");
                attempt = 0;
                stream
            }
            Err(e) => {
                warn!(url = %ws_url, attempt, "WebSocket connect failed: {e}");
                connected.store(false, Ordering::Relaxed);
                backoff_sleep(attempt).await;
                attempt = attempt.saturating_add(1);
                continue;
            }
        };

        let (mut ws_tx, mut ws_rx) = futures::StreamExt::split(ws_stream);

        // ── Register / Re-register ──────────────────────────
        let reg_msg = if first_connect {
            let reg = WireRegister {
                r#type: "register",
                app_id: config.app_id,
                parent_id: config.parent_id,
                app_name: config.app_name.clone(),
                child_pub_key: pub_key.clone(),
                process_info: collect_process_info(),
                role_refs: config.role_refs.clone(),
                sig: None,
            };
            serde_json::to_string(&reg).unwrap()
        } else {
            let rereg = WireReRegister {
                r#type: "re_register",
                app_id: config.app_id,
                last_seq,
                pub_key: pub_key.clone(),
                sig: None,
            };
            serde_json::to_string(&rereg).unwrap()
        };

        use futures::SinkExt;
        if let Err(e) = ws_tx
            .send(tokio_tungstenite::tungstenite::Message::Text(reg_msg.into()))
            .await
        {
            warn!("failed to send registration: {e}");
            connected.store(false, Ordering::Relaxed);
            backoff_sleep(attempt).await;
            attempt = attempt.saturating_add(1);
            continue;
        }

        // Wait for Registered ack.
        match tokio::time::timeout(Duration::from_secs(10), ws_rx.next()).await {
            Ok(Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text)))) => {
                debug!("server response: {text}");
                // Could parse and validate; for Phase 1, just check it's not an error.
                if text.contains("\"error\"") {
                    error!("registration rejected: {text}");
                    connected.store(false, Ordering::Relaxed);
                    backoff_sleep(attempt).await;
                    attempt = attempt.saturating_add(1);
                    continue;
                }
            }
            Ok(Some(Ok(_))) => { /* non-text, ignore */ }
            Ok(Some(Err(e))) => {
                warn!("ws error during registration: {e}");
                connected.store(false, Ordering::Relaxed);
                backoff_sleep(attempt).await;
                attempt = attempt.saturating_add(1);
                continue;
            }
            Ok(None) | Err(_) => {
                warn!("no registration response (timeout or closed)");
                connected.store(false, Ordering::Relaxed);
                backoff_sleep(attempt).await;
                attempt = attempt.saturating_add(1);
                continue;
            }
        }

        connected.store(true, Ordering::Relaxed);
        first_connect = false;

        // ── Message loop ────────────────────────────────────
        use futures::StreamExt;
        loop {
            tokio::select! {
                // Outbound messages from API methods.
                msg = rx.recv() => {
                    match msg {
                        Some(Outbound::Data { msg_type, seq, payload, correlation_id }) => {
                            last_seq = seq;
                            let wire = WireDataMsg {
                                r#type: "message",
                                app_id: config.app_id,
                                header: WireHeader {
                                    msg_type: msg_type.into(),
                                    timestamp: chrono::Utc::now().timestamp_millis(),
                                    seq,
                                    correlation_id,
                                },
                                payload,
                                sig: None,
                            };
                            let json = serde_json::to_string(&wire).unwrap();
                            if let Err(e) = ws_tx.send(
                                tokio_tungstenite::tungstenite::Message::Text(json.into())
                            ).await {
                                warn!("send error: {e}");
                                break; // reconnect
                            }
                        }
                        Some(Outbound::Disconnect { reason }) => {
                            let disc = WireDisconnect {
                                r#type: "disconnect",
                                app_id: config.app_id,
                                reason,
                            };
                            let json = serde_json::to_string(&disc).unwrap();
                            let _ = ws_tx.send(
                                tokio_tungstenite::tungstenite::Message::Text(json.into())
                            ).await;
                            let _ = ws_tx.send(
                                tokio_tungstenite::tungstenite::Message::Close(None)
                            ).await;
                            connected.store(false, Ordering::Relaxed);
                            return; // shutdown
                        }
                        None => {
                            // Channel closed — client dropped.
                            connected.store(false, Ordering::Relaxed);
                            return;
                        }
                    }
                }
                // Inbound messages from server (acks, future: control).
                frame = ws_rx.next() => {
                    match frame {
                        Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text))) => {
                            debug!("server: {text}");
                            // Phase 1: just consume acks. Phase 3: route control messages.
                        }
                        Some(Ok(tokio_tungstenite::tungstenite::Message::Close(_))) => {
                            info!("server closed connection");
                            break; // reconnect
                        }
                        Some(Ok(_)) => {} // ping/pong/binary
                        Some(Err(e)) => {
                            warn!("ws recv error: {e}");
                            break; // reconnect
                        }
                        None => {
                            info!("ws stream ended");
                            break; // reconnect
                        }
                    }
                }
            }
        }

        // Connection lost — loop back to reconnect.
        connected.store(false, Ordering::Relaxed);
        backoff_sleep(attempt).await;
        attempt = attempt.saturating_add(1);
    }
}

/// Exponential backoff with jitter (spec §19).
/// delay = min(100ms × 2^attempt, 30s) + random(0, delay × 0.5)
async fn backoff_sleep(attempt: u32) {
    let base_ms = 100u64.saturating_mul(1u64.checked_shl(attempt).unwrap_or(u64::MAX));
    let capped_ms = base_ms.min(30_000);
    let jitter_ms = (rand::random::<f64>() * capped_ms as f64 * 0.5) as u64;
    let total = Duration::from_millis(capped_ms + jitter_ms);
    debug!(ms = total.as_millis(), attempt, "backoff sleep");
    tokio::time::sleep(total).await;
}

// ═══════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decode_config() {
        let config = TrailsConfig {
            v: 1,
            app_id: Uuid::new_v4(),
            parent_id: Some(Uuid::new_v4()),
            app_name: "test".into(),
            server_ep: "ws://localhost:8443/ws".into(),
            server_pub_key: None,
            sec_level: "open".into(),
            scheduled_at: Some(1740000000000),
            start_deadline: Some(300),
            originator: None,
            role_refs: vec![],
            tags: None,
        };

        let encoded = TrailsClient::encode_config(&config).unwrap();
        let decoded = TrailsClient::decode_config(&encoded).unwrap();
        assert_eq!(decoded.app_id, config.app_id);
        assert_eq!(decoded.app_name, config.app_name);
    }

    #[tokio::test]
    async fn test_noop_client() {
        // No TRAILS_INFO set → no-op client.
        std::env::remove_var("TRAILS_INFO");
        let g = TrailsClient::init().await;
        assert!(!g.is_active());
        assert!(!g.is_connected());

        // All methods succeed silently.
        g.status(serde_json::json!({"progress": 0.5})).await.unwrap();
        g.result(serde_json::json!({"done": true})).await.unwrap();
        g.error("test error", None).await.unwrap();
        g.shutdown().await.unwrap();
    }

    #[test]
    fn test_normalize_ws_url() {
        assert_eq!(
            normalize_ws_url("ws://localhost:8443/ws"),
            "ws://localhost:8443/ws"
        );
        assert_eq!(
            normalize_ws_url("http://localhost:8443"),
            "ws://localhost:8443/ws"
        );
        assert_eq!(
            normalize_ws_url("https://trails.svc:8443/ws"),
            "wss://trails.svc:8443/ws"
        );
    }
}
