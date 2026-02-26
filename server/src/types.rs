//! Wire protocol types for TRAILS Phase 1.
//!
//! Covers: register, re_register, message (Status/Result/Error),
//! disconnect, ack, registered, server_error.
//! Control path types are defined but not routed until Phase 3.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ═══════════════════════════════════════════════════════════════
// Client → Server messages
// ═══════════════════════════════════════════════════════════════

/// Top-level envelope from client.
/// The `type` field is used to dispatch.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    Register(RegisterMsg),
    ReRegister(ReRegisterMsg),
    Message(DataMsg),
    Disconnect(DisconnectMsg),
}

/// First message after WebSocket connect (spec §8).
#[derive(Debug, Deserialize)]
pub struct RegisterMsg {
    pub app_id: Uuid,
    pub parent_id: Option<Uuid>,
    pub app_name: String,
    pub child_pub_key: String,
    pub process_info: ProcessInfo,
    #[serde(default)]
    pub role_refs: Vec<String>,
    /// Ed25519 signature — present but not verified in Phase 1 (secLevel: open).
    pub sig: Option<String>,
}

/// Process identity collected at trails_init() (spec §6).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessInfo {
    pub pid: i32,
    #[serde(default)]
    pub ppid: i32,
    #[serde(default)]
    pub uid: i32,
    #[serde(default)]
    pub gid: i32,
    #[serde(default)]
    pub hostname: String,
    #[serde(default)]
    pub node_name: Option<String>,
    #[serde(default)]
    pub pod_ip: Option<String>,
    #[serde(default)]
    pub namespace: Option<String>,
    #[serde(default)]
    pub start_time: Option<i64>,
    #[serde(default)]
    pub executable: Option<String>,
}

/// Re-registration after server restart (spec §19).
#[derive(Debug, Deserialize)]
pub struct ReRegisterMsg {
    pub app_id: Uuid,
    pub last_seq: i64,
    pub pub_key: String,
    pub sig: Option<String>,
}

/// Data message carrying Status, Result, or Error (spec §8).
#[derive(Debug, Deserialize)]
pub struct DataMsg {
    pub app_id: Uuid,
    pub header: MsgHeader,
    pub payload: serde_json::Value,
    pub sig: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MsgHeader {
    pub msg_type: MsgType,
    pub timestamp: i64,
    pub seq: i64,
    pub correlation_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MsgType {
    Status,
    Result,
    Error,
    Control,
}

impl MsgType {
    pub fn as_str(&self) -> &'static str {
        match self {
            MsgType::Status => "Status",
            MsgType::Result => "Result",
            MsgType::Error => "Error",
            MsgType::Control => "Control",
        }
    }
}

/// Graceful disconnect (spec §8).
#[derive(Debug, Deserialize)]
pub struct DisconnectMsg {
    pub app_id: Uuid,
    pub reason: String,
}

// ═══════════════════════════════════════════════════════════════
// Server → Client messages
// ═══════════════════════════════════════════════════════════════

/// Top-level envelope to client.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Registered(RegisteredMsg),
    Ack(AckMsg),
    Error(ServerErrorMsg),
    // Control — Phase 3
}

/// Sent after successful registration.
#[derive(Debug, Serialize)]
pub struct RegisteredMsg {
    pub app_id: Uuid,
    pub server_pub_key: String,
}

/// Sent after each data message.
#[derive(Debug, Serialize)]
pub struct AckMsg {
    pub seq: i64,
}

/// Sent on protocol errors.
#[derive(Debug, Serialize)]
pub struct ServerErrorMsg {
    pub code: String,
    pub message: String,
}

// ═══════════════════════════════════════════════════════════════
// Internal event bus types
// ═══════════════════════════════════════════════════════════════

/// Events published to the internal broadcast channel.
/// Phase 1: used for parent notification (future: observer fan-out).
#[derive(Debug, Clone)]
pub enum Event {
    /// A child registered / re-registered.
    AppConnected {
        app_id: Uuid,
        parent_id: Option<Uuid>,
    },
    /// A data message was stored.
    MessageStored {
        app_id: Uuid,
        parent_id: Option<Uuid>,
        msg_type: MsgType,
        seq: i64,
    },
    /// App reached terminal state.
    AppTerminal {
        app_id: Uuid,
        parent_id: Option<Uuid>,
        status: String,
    },
    /// Crash detected.
    CrashDetected {
        app_id: Uuid,
        parent_id: Option<Uuid>,
        crash_type: String,
    },
}

// ═══════════════════════════════════════════════════════════════
// App status enum (matches Postgres CHECK constraint)
// ═══════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppStatus {
    Scheduled,
    Connected,
    Running,
    Done,
    Error,
    Crashed,
    Cancelled,
    StartFailed,
    Reconnecting,
    LostContact,
}

impl AppStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Scheduled => "scheduled",
            Self::Connected => "connected",
            Self::Running => "running",
            Self::Done => "done",
            Self::Error => "error",
            Self::Crashed => "crashed",
            Self::Cancelled => "cancelled",
            Self::StartFailed => "start_failed",
            Self::Reconnecting => "reconnecting",
            Self::LostContact => "lost_contact",
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Done | Self::Error | Self::Crashed | Self::Cancelled | Self::StartFailed
        )
    }
}
