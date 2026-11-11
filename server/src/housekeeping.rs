//! What the server does by itself, once a minute: forgetting audit entries
//! past their retention, closing databases nothing used for a while,
//! scheduled backups and TWIC updates (NET-06).

use std::sync::Arc;
use std::time::Duration;

use crate::AppState;
use crate::store::now;

/// How often it looks.
const TICK: Duration = Duration::from_secs(60);

/// Runs until the server stops (the task is aborted).
pub async fn run(state: Arc<AppState>) {
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        let st = state.clone();
        let _ = tokio::task::spawn_blocking(move || once(&st)).await;
    }
}

/// One round (blocking).
pub fn once(state: &Arc<AppState>) {
    let before = now() - i64::from(state.config.audit_days) * 24 * 3600;
    match state.store.prune_audit(before) {
        Ok(0) => {}
        Ok(n) => log::info!("forgot {n} audit log entries older than {} days", state.config.audit_days),
        Err(e) => log::warn!("couldn't prune the audit log: {}", e.message),
    }
}
