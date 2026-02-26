//! Background lifecycle tasks.
//!
//! 1. **Start deadline checker** — periodically scans for 'scheduled' apps
//!    past their start_deadline and transitions them to 'start_failed'.
//!    Also records a crash with crash_type = 'never_started' (spec §7).
//!
//! 2. **Reconnection window** — after server startup, waits for clients
//!    to re-register, then marks stragglers as 'lost_contact' (spec §19).

use std::sync::Arc;
use std::time::Duration;

use tracing::{info, warn};

use crate::db;
use crate::state::AppState;
use crate::types::Event;

/// Spawn the start-deadline checker. Runs every 30 seconds.
pub fn spawn_deadline_checker(state: Arc<AppState>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(30));
        loop {
            interval.tick().await;
            if let Err(e) = check_deadlines(&state).await {
                warn!("deadline checker error: {e}");
            }
        }
    });
}

async fn check_deadlines(state: &Arc<AppState>) -> Result<(), crate::error::TrailsError> {
    let expired = db::get_expired_scheduled(&state.db).await?;
    for app in &expired {
        info!(
            app_id = %app.app_id,
            app_name = %app.app_name,
            "start deadline expired → start_failed (never_started)"
        );
        db::set_start_failed(&state.db, app.app_id).await?;
        db::record_crash(&state.db, app.app_id, "never_started", None, None).await?;

        state.publish(Event::CrashDetected {
            app_id: app.app_id,
            parent_id: app.parent_id,
            crash_type: "never_started".into(),
        });
    }
    if !expired.is_empty() {
        info!(count = expired.len(), "expired scheduled apps → start_failed");
    }
    Ok(())
}

/// On server startup: mark previous connections as 'reconnecting',
/// then after the window expires, mark stragglers as 'lost_contact' (spec §19).
pub fn spawn_reconnection_window(state: Arc<AppState>) {
    let window = state.config.reconnect_window;
    let instance = state.config.server_instance.clone();

    tokio::spawn(async move {
        // Step 1: mark all apps that were connected to us as 'reconnecting'.
        match db::mark_reconnecting(&state.db, &instance).await {
            Ok(count) => {
                if count > 0 {
                    info!(
                        count,
                        window_secs = window,
                        "marked apps as 'reconnecting', waiting for re-registration"
                    );
                }
            }
            Err(e) => warn!("mark_reconnecting error: {e}"),
        }

        // Step 2: wait for reconnection window.
        tokio::time::sleep(Duration::from_secs(window)).await;

        // Step 3: mark stragglers as 'lost_contact'.
        match db::mark_lost_contact(&state.db, &instance).await {
            Ok(count) => {
                if count > 0 {
                    warn!(count, "apps failed to reconnect → lost_contact");
                }
            }
            Err(e) => warn!("mark_lost_contact error: {e}"),
        }
    });
}
