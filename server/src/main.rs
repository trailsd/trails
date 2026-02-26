//! trailsd — TRAILS server.
//!
//! Phase 1: WebSocket handler + lifecycle state machine + Postgres.
//! See TRAILS-SPEC.md §21 for architecture overview.

mod config;
mod db;
mod error;
mod lifecycle;
mod state;
mod types;
mod ws;

use std::sync::Arc;

use axum::routing::get;
use axum::Router;
use sqlx::postgres::PgPoolOptions;
use tower_http::trace::TraceLayer;
use tracing::info;

#[tokio::main]
async fn main() {
    // Load .env if present (local dev).
    let _ = dotenvy::dotenv();

    let config = config::Config::from_env();

    // Tracing.
    tracing_subscriber::fmt()
        .with_env_filter(&config.log_level)
        .with_target(true)
        .init();

    info!("trailsd starting");
    info!(listen = %config.listen_addr, instance = %config.server_instance);

    // ── Postgres ────────────────────────────────────────────
    let pool = PgPoolOptions::new()
        .max_connections(20)
        .connect(&config.database_url)
        .await
        .expect("failed to connect to Postgres");

    // Run migration.
    info!("running migrations");
    sqlx::query(include_str!("../migrations/001_init.sql"))
        .execute(&pool)
        .await
        .unwrap_or_else(|e| {
            // Migration may fail if tables exist — that's fine on restart.
            info!("migration note (may already exist): {e}");
            Default::default()
        });

    info!("database ready");

    // ── Shared state ────────────────────────────────────────
    let state = state::AppState::new(pool, config.clone());

    // ── Background tasks ────────────────────────────────────
    // Reconnection window — mark old connections, wait, then mark lost.
    lifecycle::spawn_reconnection_window(Arc::clone(&state));
    // Start deadline checker — periodic scan.
    lifecycle::spawn_deadline_checker(Arc::clone(&state));

    // ── Routes ──────────────────────────────────────────────
    let app = Router::new()
        // WebSocket endpoint.
        .route("/ws", get(ws::ws_handler))
        // Health check (useful for K8s liveness probes).
        .route("/healthz", get(healthz))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    // ── Bind & serve ────────────────────────────────────────
    let listener = tokio::net::TcpListener::bind(&config.listen_addr)
        .await
        .expect("failed to bind");

    info!(addr = %config.listen_addr, "trailsd listening");

    axum::serve(listener, app)
        .await
        .expect("server error");
}

/// Liveness probe.
async fn healthz() -> &'static str {
    "ok"
}
