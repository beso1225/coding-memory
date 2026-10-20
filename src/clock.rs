//! Wall-clock helpers shared by capture and durable state.
//!
//! Capture timestamps, deferrals, and registration boundaries must all read the
//! same clock: a second-resolution epoch and a millisecond instant that disagree
//! would admit a run recorded before the project was enabled.

use std::time::{SystemTime, UNIX_EPOCH};

/// The current wall clock in milliseconds.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or_default()
}

/// The current wall clock in whole seconds.
pub fn now() -> i64 {
    now_ms() / 1000
}
