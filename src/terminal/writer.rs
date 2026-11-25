//! Ordered writer queue shared by the UI/input side and Ghostty's WRITE_PTY
//! effect callback. Both producers push whole byte buffers; one platform
//! writer thread consumes them in order. No OS I/O happens here and no lock is
//! held while a buffer crosses an FFI boundary.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex};

/// Point-in-time writer statistics, folded into `CountersSnapshot` by the
/// session so the callback path is counted exactly like UI input.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WriterStats {
    pub messages: u64,
    pub bytes: u64,
    pub max_depth: u64,
}

struct Inner {
    messages: VecDeque<Vec<u8>>,
    closed: bool,
}

/// A bounded-by-consumer, unbounded-by-producer queue. The wakeup condvar lets
/// the writer thread sleep instead of polling. `close` unblocks it for shutdown.
pub struct WriterQueue {
    inner: Mutex<Inner>,
    changed: Condvar,
    stats: WriterStatsAtomics,
}

#[derive(Default)]
struct WriterStatsAtomics {
    messages: AtomicU64,
    bytes: AtomicU64,
    max_depth: AtomicU64,
}

impl Default for WriterQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl WriterQueue {
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                messages: VecDeque::new(),
                closed: false,
            }),
            changed: Condvar::new(),
            stats: WriterStatsAtomics::default(),
        }
    }

    /// Enqueue one ordered write. Returns the new queue depth.
    pub fn push(&self, bytes: Vec<u8>) -> usize {
        let byte_count = bytes.len();
        let depth = {
            let mut inner = self.inner.lock().expect("writer mutex poisoned");
            if inner.closed {
                return 0;
            }
            inner.messages.push_back(bytes);
            inner.messages.len()
        };
        self.stats.messages.fetch_add(1, Ordering::Relaxed);
        self.stats.bytes.fetch_add(byte_count as u64, Ordering::Relaxed);
        self.stats.max_depth.fetch_max(depth as u64, Ordering::Relaxed);
        self.changed.notify_one();
        depth
    }

    /// Block until one message is available. `None` means the queue was closed
    /// and drained, which is the writer thread's shutdown signal.
    #[must_use]
    pub fn pop_blocking(&self) -> Option<Vec<u8>> {
        let mut inner = self.inner.lock().expect("writer mutex poisoned");
        loop {
            if let Some(bytes) = inner.messages.pop_front() {
                return Some(bytes);
            }
            if inner.closed {
                return None;
            }
            inner = self
                .changed
                .wait(inner)
                .expect("writer mutex poisoned while waiting");
        }
    }

    /// Non-blocking drain used by tests and headless sessions.
    #[must_use]
    pub fn pop(&self) -> Option<Vec<u8>> {
        self.inner
            .lock()
            .expect("writer mutex poisoned")
            .messages
            .pop_front()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.inner
            .lock()
            .expect("writer mutex poisoned")
            .messages
            .len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Signal shutdown and wake the writer thread. Already-queued bytes stay
    /// queued so they can be flushed before the writer exits.
    pub fn close(&self) {
        let mut inner = self.inner.lock().expect("writer mutex poisoned");
        inner.closed = true;
        drop(inner);
        self.changed.notify_all();
    }

    #[must_use]
    pub fn stats(&self) -> WriterStats {
        WriterStats {
            messages: self.stats.messages.load(Ordering::Relaxed),
            bytes: self.stats.bytes.load(Ordering::Relaxed),
            max_depth: self.stats.max_depth.load(Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn push_pop_preserves_order_and_counts() {
        let queue = WriterQueue::new();
        assert_eq!(queue.push(b"a".to_vec()), 1);
        assert_eq!(queue.push(b"b".to_vec()), 2);
        assert_eq!(queue.pop().expect("first"), b"a");
        assert_eq!(queue.pop().expect("second"), b"b");
        assert!(queue.is_empty());
        let stats = queue.stats();
        assert_eq!(stats.messages, 2);
        assert_eq!(stats.bytes, 2);
        assert_eq!(stats.max_depth, 2);
    }

    #[test]
    fn close_wakes_a_blocked_writer() {
        let queue = Arc::new(WriterQueue::new());
        let waiter = Arc::clone(&queue);
        let handle = std::thread::spawn(move || waiter.pop_blocking());
        // Give the writer a chance to block; close must still wake it.
        std::thread::sleep(std::time::Duration::from_millis(10));
        queue.close();
        assert!(handle.join().expect("writer thread").is_none());
    }

    #[test]
    fn close_flushes_queued_bytes_before_shutdown() {
        let queue = WriterQueue::new();
        queue.push(b"pending".to_vec());
        queue.close();
        assert_eq!(queue.pop_blocking().expect("flushed"), b"pending");
        assert_eq!(queue.pop_blocking(), None);
        assert_eq!(queue.push(b"ignored".to_vec()), 0);
    }
}
