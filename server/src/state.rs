//! Shared server state — connection tracking and event bus.

use std::sync::Arc;

use dashmap::DashMap;
use ed25519_dalek::SigningKey;
use sqlx::PgPool;
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::config::Config;
use crate::types::Event;

/// Per-connection info for a connected client.
#[derive(Debug)]
pub struct ConnectedClient {
    pub app_id: Uuid,
    pub parent_id: Option<Uuid>,
    pub namespace: Option<String>,
    /// Current highest seq received from this client.
    pub last_seq: i64,
}

/// Shared state accessible from all handlers.
pub struct AppState {
    pub db: PgPool,
    /// Active WebSocket connections keyed by app_id.
    pub connections: DashMap<Uuid, ConnectedClient>,
    /// Internal event bus (spec §21). Today: parent notification.
    /// Future: observer fan-out, Kafka/NATS publishing.
    pub event_tx: broadcast::Sender<Event>,
    /// Server's Ed25519 signing key. Public key shared with clients.
    pub server_key: SigningKey,
    pub config: Config,
}

impl AppState {
    pub fn new(db: PgPool, config: Config) -> Arc<Self> {
        let (event_tx, _) = broadcast::channel(4096);

        // Generate server Ed25519 keypair.
        // In production, load from K8s Secret for persistence across restarts.
        // Phase 1: fresh keypair per startup is fine.
        let mut rng = rand::thread_rng();
        let server_key = SigningKey::generate(&mut rng);

        Arc::new(Self {
            db,
            connections: DashMap::new(),
            event_tx,
            server_key,
            config,
        })
    }

    /// Server's public key as "ed25519:<base64>" string.
    pub fn server_pub_key_str(&self) -> String {
        use base64::Engine;
        let pub_bytes = self.server_key.verifying_key().to_bytes();
        let b64 = base64::engine::general_purpose::STANDARD.encode(pub_bytes);
        format!("ed25519:{b64}")
    }

    /// Publish an event to the internal bus. Failures (no receivers) are ignored.
    pub fn publish(&self, event: Event) {
        let _ = self.event_tx.send(event);
    }
}
