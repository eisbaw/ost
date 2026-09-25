//! OstMac patch: in-process event hub for Trouter push frames.
//!
//! The CLI prints Trouter events to stdout; embedders (Swift via ostmac-core)
//! need them as data. [`publish`] is called from the Trouter frame handler,
//! [`drain`] lets the host poll without sharing a tokio runtime, and
//! [`drain_wait`] lets the host block until events arrive (or a timeout
//! elapses) instead of waking on a fixed timer.

use std::collections::VecDeque;
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::Duration;

fn queue() -> &'static (Mutex<VecDeque<String>>, Condvar) {
    static Q: OnceLock<(Mutex<VecDeque<String>>, Condvar)> = OnceLock::new();
    Q.get_or_init(|| (Mutex::new(VecDeque::with_capacity(256)), Condvar::new()))
}

/// Publish one event payload (JSON). Never blocks, drops oldest over 1024.
pub fn publish(event_json: String) {
    let (m, c) = queue();
    let mut q = m.lock().unwrap_or_else(|e| e.into_inner());
    if q.len() >= 1024 {
        q.pop_front();
    }
    q.push_back(event_json);
    c.notify_one();
}

/// Drain up to `max` queued events, oldest first.
pub fn drain(max: usize) -> Vec<String> {
    let (m, _) = queue();
    let mut q = m.lock().unwrap_or_else(|e| e.into_inner());
    let n = max.min(q.len());
    q.drain(..n).collect()
}

/// Drain up to `max` queued events, waiting up to `timeout_ms` for the
/// first one when the queue is empty. Returns immediately once any event
/// is present; a timeout yields whatever is queued (possibly empty).
pub fn drain_wait(max: usize, timeout_ms: u64) -> Vec<String> {
    let (m, c) = queue();
    let mut q = m.lock().unwrap_or_else(|e| e.into_inner());
    if q.is_empty() && timeout_ms > 0 {
        q = c
            .wait_timeout(q, Duration::from_millis(timeout_ms))
            .map(|(g, _)| g)
            .unwrap_or_else(|e| e.into_inner().0);
    }
    let n = max.min(q.len());
    q.drain(..n).collect()
}

/// Events still queued (backlog after a drain).
pub fn len() -> usize {
    let (m, _) = queue();
    m.lock().unwrap_or_else(|e| e.into_inner()).len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wait_wakes_on_publish_and_zero_timeout_never_blocks() {
        // Single test: hub is global, parallel hub tests would race.
        drain(1024);
        let h = std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(20));
            publish(r#"{"kind":"wake"}"#.to_string());
        });
        let got = drain_wait(64, 5000); // 5s budget, no timing assert
        h.join().unwrap();
        assert_eq!(got.len(), 1);

        publish("a".to_string());
        publish("b".to_string());
        assert_eq!(drain_wait(64, 0).len(), 2);
        assert!(drain_wait(64, 0).is_empty());
        assert_eq!(len(), 0);
    }
}
