//! OstMac patch: in-process event hub for Trouter push frames.
//!
//! The CLI prints Trouter events to stdout; embedders (Swift via ostmac-core)
//! need them as data. [`publish`] is called from the Trouter frame handler,
//! [`drain`] lets the host poll without sharing a tokio runtime.

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

fn queue() -> &'static Mutex<VecDeque<String>> {
    static Q: OnceLock<Mutex<VecDeque<String>>> = OnceLock::new();
    Q.get_or_init(|| Mutex::new(VecDeque::with_capacity(256)))
}

/// Publish one event payload (JSON). Never blocks, drops oldest over 1024.
pub fn publish(event_json: String) {
    let mut q = queue().lock().unwrap_or_else(|e| e.into_inner());
    if q.len() >= 1024 {
        q.pop_front();
    }
    q.push_back(event_json);
}

/// Drain up to `max` queued events, oldest first.
pub fn drain(max: usize) -> Vec<String> {
    let mut q = queue().lock().unwrap_or_else(|e| e.into_inner());
    let n = max.min(q.len());
    q.drain(..n).collect()
}
