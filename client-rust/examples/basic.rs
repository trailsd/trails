//! Basic TRAILS client example.
//!
//! Run with TRAILS_INFO set:
//! ```bash
//! TRAILS_INFO=<base64> cargo run --example basic
//! ```
//! Or without — the client will be no-op and silently succeed.

use serde_json::json;
use trails_client::TrailsClient;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    // Two lines to integrate — that's the TRAILS promise.
    let g = TrailsClient::init().await;

    if g.is_active() {
        println!("TRAILS active — connected: {}", g.is_connected());
    } else {
        println!("TRAILS inactive (no TRAILS_INFO) — no-op mode");
    }

    // Status updates — progress snapshots.
    g.status(json!({
        "phase": "processing",
        "progress": 0.25,
        "rows_done": 25000,
        "rows_total": 100000,
    }))
    .await
    .unwrap();

    g.status(json!({
        "phase": "processing",
        "progress": 0.75,
        "rows_done": 75000,
        "rows_total": 100000,
    }))
    .await
    .unwrap();

    // Business result — structured output.
    g.result(json!({
        "rows_scanned": 100000,
        "pii_columns_found": 4,
        "duration_sec": 342,
    }))
    .await
    .unwrap();

    // Graceful shutdown.
    g.shutdown().await.unwrap();
}
