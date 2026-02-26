//! WebSocket handler — the heart of trailsd Phase 1.
//!
//! Flow per connection:
//! 1. Accept WS upgrade
//! 2. Wait for register or re_register (first message)
//! 3. Validate, store in Postgres, send Registered ack
//! 4. Enter message loop: receive data messages, send acks
//! 5. On disconnect/drop: detect crash or graceful exit

use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{State, WebSocketUpgrade};
use axum::response::IntoResponse;
use futures::stream::SplitSink;
use futures::{SinkExt, StreamExt};
use tokio::sync::Mutex;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::db;
use crate::error::TrailsError;
use crate::state::{AppState, ConnectedClient};
use crate::types::*;

/// Axum handler for GET /ws — upgrades to WebSocket.
pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

/// Per-connection state machine.
async fn handle_socket(socket: WebSocket, state: Arc<AppState>) {
    let (sender, mut receiver) = socket.split();
    let sender = Arc::new(Mutex::new(sender));

    // ── Phase 1: wait for registration ──────────────────────
    let reg_result = wait_for_registration(&mut receiver, &sender, &state).await;

    let (app_id, parent_id, namespace) = match reg_result {
        Ok(info) => info,
        Err(e) => {
            warn!("registration failed: {e}");
            let _ = send_error(&sender, "registration_failed", &e.to_string()).await;
            return;
        }
    };

    info!(app_id = %app_id, "client registered, entering message loop");

    // ── Phase 2: message loop ───────────────────────────────
    let mut graceful = false;
    while let Some(msg) = receiver.next().await {
        match msg {
            Ok(Message::Text(text)) => {
                match handle_client_message(&text, app_id, &state, &sender).await {
                    Ok(terminal) => {
                        if terminal {
                            graceful = true;
                            break;
                        }
                    }
                    Err(e) => {
                        warn!(app_id = %app_id, "message error: {e}");
                        let _ = send_error(&sender, "message_error", &e.to_string()).await;
                    }
                }
            }
            Ok(Message::Close(_)) => {
                graceful = false; // Treat WS close frame without disconnect msg as crash
                break;
            }
            Ok(Message::Ping(_)) => { /* axum auto-pongs */ }
            Ok(_) => { /* binary frames ignored */ }
            Err(e) => {
                warn!(app_id = %app_id, "ws recv error: {e}");
                break;
            }
        }
    }

    // ── Phase 3: cleanup ────────────────────────────────────
    state.connections.remove(&app_id);

    if !graceful {
        info!(app_id = %app_id, "connection dropped → crash");
        if let Err(e) = db::set_crashed(&state.db, app_id).await {
            error!(app_id = %app_id, "set_crashed error: {e}");
        }
        if let Err(e) = db::record_crash(&state.db, app_id, "connection_drop", None, None).await {
            error!(app_id = %app_id, "record_crash error: {e}");
        }
        state.publish(Event::CrashDetected {
            app_id,
            parent_id,
            crash_type: "connection_drop".into(),
        });
    }
}

// ═══════════════════════════════════════════════════════════════
// Registration
// ═══════════════════════════════════════════════════════════════

type Sender = Arc<Mutex<SplitSink<WebSocket, Message>>>;

/// Wait for the first message — must be `register` or `re_register`.
async fn wait_for_registration(
    receiver: &mut futures::stream::SplitStream<WebSocket>,
    sender: &Sender,
    state: &Arc<AppState>,
) -> Result<(Uuid, Option<Uuid>, Option<String>), TrailsError> {
    // Timeout: 30 seconds to send registration.
    let msg = tokio::time::timeout(std::time::Duration::from_secs(30), receiver.next())
        .await
        .map_err(|_| TrailsError::Protocol("registration timeout".into()))?
        .ok_or_else(|| TrailsError::Protocol("connection closed before registration".into()))?
        .map_err(|e| TrailsError::Protocol(format!("ws error: {e}")))?;

    let text = match msg {
        Message::Text(t) => t,
        _ => return Err(TrailsError::Protocol("expected text frame for registration".into())),
    };

    let client_msg: ClientMessage =
        serde_json::from_str(&text).map_err(|e| TrailsError::Protocol(format!("invalid JSON: {e}")))?;

    match client_msg {
        ClientMessage::Register(reg) => handle_register(reg, sender, state).await,
        ClientMessage::ReRegister(rereg) => handle_re_register(rereg, sender, state).await,
        _ => Err(TrailsError::Protocol(
            "first message must be register or re_register".into(),
        )),
    }
}

/// Handle fresh registration.
async fn handle_register(
    reg: RegisterMsg,
    sender: &Sender,
    state: &Arc<AppState>,
) -> Result<(Uuid, Option<Uuid>, Option<String>), TrailsError> {
    let app_id = reg.app_id;
    let parent_id = reg.parent_id;

    // Check if app already exists (Phase A pre-registration by parent).
    let existing = db::get_app(&state.db, app_id).await?;

    if let Some(row) = &existing {
        if row.status != "scheduled" {
            return Err(TrailsError::RegistrationFailed(format!(
                "app {app_id} already in state '{}'",
                row.status
            )));
        }
    } else {
        // No Phase A pre-registration — auto-create scheduled row.
        // This supports the simple case: child connects directly without
        // parent calling POST /api/v1/children first.
        db::create_scheduled_app(
            &state.db,
            app_id,
            parent_id,
            &reg.app_name,
            state.config.default_start_deadline,
            &reg.role_refs,
            None,
        )
        .await?;
    }

    let pi = &reg.process_info;
    let namespace = pi.namespace.clone();

    // Transition scheduled → connected.
    db::connect_app(
        &state.db,
        app_id,
        &reg.child_pub_key,
        &state.config.server_instance,
        pi.pid,
        pi.ppid,
        pi.uid,
        pi.gid,
        &pi.hostname,
        pi.node_name.as_deref(),
        pi.pod_ip.as_deref(),
        pi.namespace.as_deref(),
        pi.executable.as_deref(),
    )
    .await?;

    // Track connection.
    state.connections.insert(
        app_id,
        ConnectedClient {
            app_id,
            parent_id,
            namespace: namespace.clone(),
            last_seq: 0,
        },
    );

    // Send Registered ack.
    let ack = ServerMessage::Registered(RegisteredMsg {
        app_id,
        server_pub_key: state.server_pub_key_str(),
    });
    send_msg(sender, &ack).await?;

    state.publish(Event::AppConnected { app_id, parent_id });

    info!(
        app_id = %app_id,
        parent_id = ?parent_id,
        app_name = %reg.app_name,
        pid = pi.pid,
        "registration complete → connected"
    );

    Ok((app_id, parent_id, namespace))
}

/// Handle re-registration after server restart (spec §19).
async fn handle_re_register(
    rereg: ReRegisterMsg,
    sender: &Sender,
    state: &Arc<AppState>,
) -> Result<(Uuid, Option<Uuid>, Option<String>), TrailsError> {
    let app_id = rereg.app_id;

    let row = db::reconnect_app(
        &state.db,
        app_id,
        &rereg.pub_key,
        &state.config.server_instance,
    )
    .await?
    .ok_or_else(|| {
        TrailsError::RegistrationFailed(format!(
            "re_register failed for {app_id}: not found or pub_key mismatch"
        ))
    })?;

    let parent_id = row.parent_id;
    let namespace = row.namespace.clone();

    state.connections.insert(
        app_id,
        ConnectedClient {
            app_id,
            parent_id,
            namespace: namespace.clone(),
            last_seq: rereg.last_seq,
        },
    );

    let ack = ServerMessage::Registered(RegisteredMsg {
        app_id,
        server_pub_key: state.server_pub_key_str(),
    });
    send_msg(sender, &ack).await?;

    state.publish(Event::AppConnected { app_id, parent_id });

    info!(app_id = %app_id, last_seq = rereg.last_seq, "re-registered → running");

    Ok((app_id, parent_id, namespace))
}

// ═══════════════════════════════════════════════════════════════
// Message handling
// ═══════════════════════════════════════════════════════════════

/// Handle a client message after registration.
/// Returns Ok(true) if this was a terminal message (disconnect/done/error).
async fn handle_client_message(
    text: &str,
    registered_app_id: Uuid,
    state: &Arc<AppState>,
    sender: &Sender,
) -> Result<bool, TrailsError> {
    let client_msg: ClientMessage =
        serde_json::from_str(text).map_err(|e| TrailsError::Protocol(format!("invalid JSON: {e}")))?;

    match client_msg {
        ClientMessage::Message(data) => {
            // Verify app_id matches registration (or is a multiplexed identity).
            // Phase 1: simple check — must match registered app_id.
            if data.app_id != registered_app_id {
                return Err(TrailsError::Protocol(format!(
                    "app_id mismatch: registered={registered_app_id}, message={}",
                    data.app_id
                )));
            }

            handle_data_message(data, state, sender).await
        }
        ClientMessage::Disconnect(disc) => {
            handle_disconnect(disc, state).await?;
            Ok(true) // terminal
        }
        ClientMessage::Register(_) | ClientMessage::ReRegister(_) => {
            Err(TrailsError::Protocol("duplicate registration".into()))
        }
    }
}

/// Process a data message (Status, Result, Error).
async fn handle_data_message(
    data: DataMsg,
    state: &Arc<AppState>,
    sender: &Sender,
) -> Result<bool, TrailsError> {
    let app_id = data.app_id;
    let msg_type = data.header.msg_type;
    let seq = data.header.seq;

    // Get namespace for snapshot storage.
    let namespace = state
        .connections
        .get(&app_id)
        .map(|c| c.namespace.clone())
        .unwrap_or(None);

    // On first Status message: transition connected → running.
    if msg_type == MsgType::Status {
        // Attempt transition — idempotent, no error if already running.
        let _ = db::set_running(&state.db, app_id).await;
    }

    // Store the message.
    db::store_message(
        &state.db,
        app_id,
        "in",
        msg_type.as_str(),
        seq,
        data.header.correlation_id.as_deref(),
        &data.payload,
    )
    .await?;

    // Status messages also stored as snapshots (spec §13).
    if msg_type == MsgType::Status {
        db::store_snapshot(&state.db, app_id, namespace.as_deref(), seq, &data.payload).await?;
    }

    // Update last_seq.
    if let Some(mut conn) = state.connections.get_mut(&app_id) {
        conn.last_seq = seq;
    }

    let parent_id = state
        .connections
        .get(&app_id)
        .map(|c| c.parent_id)
        .unwrap_or(None);

    state.publish(Event::MessageStored {
        app_id,
        parent_id,
        msg_type,
        seq,
    });

    // Handle terminal message types.
    let terminal = match msg_type {
        MsgType::Result => {
            db::set_terminal(&state.db, app_id, "done").await?;
            state.publish(Event::AppTerminal {
                app_id,
                parent_id,
                status: "done".into(),
            });
            true
        }
        MsgType::Error => {
            db::set_terminal(&state.db, app_id, "error").await?;
            state.publish(Event::AppTerminal {
                app_id,
                parent_id,
                status: "error".into(),
            });
            true
        }
        _ => false,
    };

    // Ack the message.
    let ack = ServerMessage::Ack(AckMsg { seq });
    send_msg(sender, &ack).await?;

    Ok(terminal)
}

/// Handle graceful disconnect.
async fn handle_disconnect(disc: DisconnectMsg, state: &Arc<AppState>) -> Result<(), TrailsError> {
    let app_id = disc.app_id;
    info!(app_id = %app_id, reason = %disc.reason, "graceful disconnect");

    // If reason is "completed", transition to done (if not already terminal).
    match disc.reason.as_str() {
        "completed" | "done" => {
            let _ = db::set_terminal(&state.db, app_id, "done").await;
        }
        "error" | "failed" => {
            let _ = db::set_terminal(&state.db, app_id, "error").await;
        }
        _ => {
            // Generic disconnect — mark as done.
            let _ = db::set_terminal(&state.db, app_id, "done").await;
        }
    }

    let parent_id = state
        .connections
        .get(&app_id)
        .map(|c| c.parent_id)
        .unwrap_or(None);

    state.publish(Event::AppTerminal {
        app_id,
        parent_id,
        status: "done".into(),
    });

    Ok(())
}

// ═══════════════════════════════════════════════════════════════
// Helpers
// ═══════════════════════════════════════════════════════════════

async fn send_msg(sender: &Sender, msg: &ServerMessage) -> Result<(), TrailsError> {
    let json = serde_json::to_string(msg)
        .map_err(|e| TrailsError::Protocol(format!("serialize error: {e}")))?;
    let mut guard = sender.lock().await;
    guard
        .send(Message::Text(json.into()))
        .await
        .map_err(|e| TrailsError::Protocol(format!("send error: {e}")))?;
    Ok(())
}

async fn send_error(sender: &Sender, code: &str, message: &str) -> Result<(), TrailsError> {
    let msg = ServerMessage::Error(ServerErrorMsg {
        code: code.into(),
        message: message.into(),
    });
    send_msg(sender, &msg).await
}
