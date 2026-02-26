//! Postgres query layer for trailsd.
//!
//! All state mutations go through this module.
//! Uses sqlx with compile-time-unchecked queries (runtime-checked)
//! to avoid needing a live DB at compile time.

use chrono::{DateTime, Utc};
use serde_json::Value as JsonValue;
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::TrailsError;

// ═══════════════════════════════════════════════════════════════
// App lifecycle
// ═══════════════════════════════════════════════════════════════

/// Row returned from apps table queries.
#[derive(Debug, sqlx::FromRow)]
pub struct AppRow {
    pub app_id: Uuid,
    pub parent_id: Option<Uuid>,
    pub app_name: String,
    pub status: String,
    pub pub_key: Option<String>,
    pub server_instance: Option<String>,
    pub start_deadline: Option<i32>,
    pub namespace: Option<String>,
    pub connected_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// Create a "scheduled" app row — Phase A of two-phase lifecycle (spec §7).
/// Called when parent registers intent via REST (Phase 2) or when
/// a child registers directly and we auto-create the scheduled row.
pub async fn create_scheduled_app(
    pool: &PgPool,
    app_id: Uuid,
    parent_id: Option<Uuid>,
    app_name: &str,
    start_deadline: i32,
    role_refs: &[String],
    metadata: Option<&JsonValue>,
) -> Result<(), TrailsError> {
    sqlx::query(
        r#"
        INSERT INTO apps (app_id, parent_id, app_name, status, start_deadline, role_refs, metadata_json)
        VALUES ($1, $2, $3, 'scheduled', $4, $5, $6)
        ON CONFLICT (app_id) DO NOTHING
        "#,
    )
    .bind(app_id)
    .bind(parent_id)
    .bind(app_name)
    .bind(start_deadline)
    .bind(role_refs)
    .bind(metadata)
    .execute(pool)
    .await?;
    Ok(())
}

/// Transition app to 'connected' and record process info + pub_key.
/// Called on successful registration.
pub async fn connect_app(
    pool: &PgPool,
    app_id: Uuid,
    pub_key: &str,
    server_instance: &str,
    pid: i32,
    ppid: i32,
    uid: i32,
    gid: i32,
    hostname: &str,
    node_name: Option<&str>,
    pod_ip: Option<&str>,
    namespace: Option<&str>,
    executable: Option<&str>,
) -> Result<(), TrailsError> {
    let result = sqlx::query(
        r#"
        UPDATE apps SET
            status = 'connected',
            pub_key = $2,
            server_instance = $3,
            connected_at = NOW(),
            pid = $4,
            ppid = $5,
            proc_uid = $6,
            proc_gid = $7,
            pod_name = $8,
            node_name = $9,
            pod_ip = $10,
            namespace = $11,
            executable = $12
        WHERE app_id = $1
          AND status IN ('scheduled', 'reconnecting')
        "#,
    )
    .bind(app_id)
    .bind(pub_key)
    .bind(server_instance)
    .bind(pid)
    .bind(ppid)
    .bind(uid)
    .bind(gid)
    .bind(hostname)
    .bind(node_name)
    .bind(pod_ip)
    .bind(namespace)
    .bind(executable)
    .execute(pool)
    .await?;

    if result.rows_affected() == 0 {
        return Err(TrailsError::InvalidTransition {
            from: "?".into(),
            to: "connected".into(),
        });
    }
    Ok(())
}

/// Transition to 'running'. Called on first Status message.
pub async fn set_running(pool: &PgPool, app_id: Uuid) -> Result<(), TrailsError> {
    sqlx::query(
        r#"
        UPDATE apps SET status = 'running', start_time = NOW()
        WHERE app_id = $1 AND status = 'connected'
        "#,
    )
    .bind(app_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Transition to terminal state: done, error, cancelled.
pub async fn set_terminal(
    pool: &PgPool,
    app_id: Uuid,
    status: &str,
) -> Result<(), TrailsError> {
    sqlx::query(
        r#"
        UPDATE apps SET status = $2, disconnected_at = NOW()
        WHERE app_id = $1 AND status IN ('connected', 'running')
        "#,
    )
    .bind(app_id)
    .bind(status)
    .execute(pool)
    .await?;
    Ok(())
}

/// Mark app as crashed (connection drop).
pub async fn set_crashed(pool: &PgPool, app_id: Uuid) -> Result<(), TrailsError> {
    sqlx::query(
        r#"
        UPDATE apps SET status = 'crashed', disconnected_at = NOW()
        WHERE app_id = $1 AND status IN ('connected', 'running')
        "#,
    )
    .bind(app_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Mark app as start_failed (deadline expired, never connected).
pub async fn set_start_failed(pool: &PgPool, app_id: Uuid) -> Result<(), TrailsError> {
    sqlx::query(
        r#"
        UPDATE apps SET status = 'start_failed', disconnected_at = NOW()
        WHERE app_id = $1 AND status = 'scheduled'
        "#,
    )
    .bind(app_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Mark apps on this server instance as 'reconnecting' after restart (spec §19).
pub async fn mark_reconnecting(
    pool: &PgPool,
    server_instance: &str,
) -> Result<u64, TrailsError> {
    let result = sqlx::query(
        r#"
        UPDATE apps SET status = 'reconnecting'
        WHERE server_instance = $1
          AND status IN ('connected', 'running')
        "#,
    )
    .bind(server_instance)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

/// Mark apps that failed to reconnect within window as 'lost_contact'.
pub async fn mark_lost_contact(
    pool: &PgPool,
    server_instance: &str,
) -> Result<u64, TrailsError> {
    let result = sqlx::query(
        r#"
        UPDATE apps SET status = 'lost_contact', disconnected_at = NOW()
        WHERE server_instance = $1 AND status = 'reconnecting'
        "#,
    )
    .bind(server_instance)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

/// Re-connect an app after server restart. Verifies pub_key matches.
pub async fn reconnect_app(
    pool: &PgPool,
    app_id: Uuid,
    pub_key: &str,
    server_instance: &str,
) -> Result<Option<AppRow>, TrailsError> {
    let row: Option<AppRow> = sqlx::query_as(
        r#"
        UPDATE apps SET
            status = 'running',
            server_instance = $3,
            connected_at = NOW()
        WHERE app_id = $1
          AND pub_key = $2
          AND status IN ('reconnecting', 'lost_contact')
        RETURNING app_id, parent_id, app_name, status, pub_key,
                  server_instance, start_deadline, namespace,
                  connected_at, created_at
        "#,
    )
    .bind(app_id)
    .bind(pub_key)
    .bind(server_instance)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// Lookup an app by id.
pub async fn get_app(pool: &PgPool, app_id: Uuid) -> Result<Option<AppRow>, TrailsError> {
    let row: Option<AppRow> = sqlx::query_as(
        r#"
        SELECT app_id, parent_id, app_name, status, pub_key,
               server_instance, start_deadline, namespace,
               connected_at, created_at
        FROM apps WHERE app_id = $1
        "#,
    )
    .bind(app_id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// Get all 'scheduled' apps past their start deadline.
pub async fn get_expired_scheduled(pool: &PgPool) -> Result<Vec<AppRow>, TrailsError> {
    let rows: Vec<AppRow> = sqlx::query_as(
        r#"
        SELECT app_id, parent_id, app_name, status, pub_key,
               server_instance, start_deadline, namespace,
               connected_at, created_at
        FROM apps
        WHERE status = 'scheduled'
          AND created_at + (COALESCE(start_deadline, 300) || ' seconds')::INTERVAL < NOW()
        "#,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

// ═══════════════════════════════════════════════════════════════
// Messages
// ═══════════════════════════════════════════════════════════════

/// Store a data message (Status, Result, Error).
pub async fn store_message(
    pool: &PgPool,
    app_id: Uuid,
    direction: &str,
    msg_type: &str,
    seq: i64,
    correlation_id: Option<&str>,
    payload: &JsonValue,
) -> Result<(), TrailsError> {
    sqlx::query(
        r#"
        INSERT INTO messages (app_id, direction, msg_type, seq, correlation_id, payload_json)
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(app_id)
    .bind(direction)
    .bind(msg_type)
    .bind(seq)
    .bind(correlation_id)
    .bind(payload)
    .execute(pool)
    .await?;
    Ok(())
}

/// Store a snapshot (Status messages double as snapshots).
pub async fn store_snapshot(
    pool: &PgPool,
    app_id: Uuid,
    namespace: Option<&str>,
    seq: i64,
    snapshot: &JsonValue,
) -> Result<(), TrailsError> {
    sqlx::query(
        r#"
        INSERT INTO snapshots (app_id, namespace, seq, snapshot_json)
        VALUES ($1, $2, $3, $4)
        "#,
    )
    .bind(app_id)
    .bind(namespace)
    .bind(seq)
    .bind(snapshot)
    .execute(pool)
    .await?;
    Ok(())
}

// ═══════════════════════════════════════════════════════════════
// Crashes
// ═══════════════════════════════════════════════════════════════

/// Record a crash event.
pub async fn record_crash(
    pool: &PgPool,
    app_id: Uuid,
    crash_type: &str,
    gap_seconds: Option<f32>,
    metadata: Option<&JsonValue>,
) -> Result<(), TrailsError> {
    sqlx::query(
        r#"
        INSERT INTO crashes (app_id, crash_type, gap_seconds, metadata_json)
        VALUES ($1, $2, $3, $4)
        "#,
    )
    .bind(app_id)
    .bind(crash_type)
    .bind(gap_seconds)
    .bind(metadata)
    .execute(pool)
    .await?;
    Ok(())
}
