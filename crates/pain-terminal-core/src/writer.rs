//! Ordered writer queue shared by the UI/input side and Ghostty's WRITE_PTY
//! effect callback. Both producers push whole byte buffers; one platform
//! writer thread consumes them in order. No OS I/O happens here and no lock is
//! held while a buffer crosses an FFI boundary.
//!
//! The queue has one source of truth for its lifecycle ([`WriterQueueState`])
//! and a fixed byte budget. A producer never waits for capacity: when a buffer
//! would exceed the budget the queue fails explicitly. This keeps the C
//! `WRITE_PTY` callback non-blocking and stops a dead platform writer from
//! growing process memory without bound.

use std::collections::VecDeque;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex};

/// Default implementation limit for pending outbound bytes. This is a resource
/// guard, not a user preference; callers can override it through
/// [`SessionConfig`](crate::session::SessionConfig) or
/// [`WriterQueue::with_byte_budget`].
pub const DEFAULT_WRITER_BYTE_BUDGET: usize = 4 * 1024 * 1024;

/// The queue's lifecycle. `Failed` is deliberately distinct from `Closed`:
/// normal shutdown and a failed writer need different diagnostics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriterQueueState {
    Open,
    Closed,
    Failed,
}

/// Why a write was rejected. The core queue does not know about platform
/// errors; it only records whether writes are still accepted and why not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriterQueueError {
    /// The queue was closed normally. Already-queued bytes can still drain.
    Closed,
    /// A platform writer failed. No further writes are accepted.
    Failed,
    /// One whole buffer would exceed the configured pending-byte budget. The
    /// queue transitions to `Failed` instead of dropping or splitting it.
    ByteBudgetExceeded { len: usize, budget: usize },
}

impl fmt::Display for WriterQueueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Closed => write!(f, "writer queue is closed"),
            Self::Failed => write!(f, "writer queue has failed"),
            Self::ByteBudgetExceeded { len, budget } => write!(
                f,
                "writer buffer of {len} bytes exceeds the {budget}-byte pending budget"
            ),
        }
    }
}

impl std::error::Error for WriterQueueError {}

/// Point-in-time writer statistics, folded into `CountersSnapshot` by the
/// session so the callback path is counted exactly like UI input.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WriterStats {
    pub messages: u64,
    pub bytes: u64,
    pub max_depth: u64,
    /// Writes rejected after close, after failure, or by the byte budget.
    pub rejected: u64,
}

struct Inner {
    messages: VecDeque<Vec<u8>>,
    pending_bytes: usize,
    byte_budget: usize,
    state: WriterQueueState,
}

/// A bounded-by-consumer, bounded-in-memory queue. The wakeup condvar lets the
/// writer thread sleep instead of polling. `close` unblocks it for normal
/// shutdown; `fail` unblocks it after a transport failure.
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
    rejected: AtomicU64,
}

impl Default for WriterQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl WriterQueue {
    #[must_use]
    pub fn new() -> Self {
        Self::with_byte_budget(DEFAULT_WRITER_BYTE_BUDGET)
    }

    /// Construct a queue with an explicit pending-byte budget. A budget of
    /// zero is nonsensical for a queue that must accept at least one buffer,
    /// so it is raised to one.
    #[must_use]
    pub fn with_byte_budget(byte_budget: usize) -> Self {
        Self {
            inner: Mutex::new(Inner {
                messages: VecDeque::new(),
                pending_bytes: 0,
                byte_budget: byte_budget.max(1),
                state: WriterQueueState::Open,
            }),
            changed: Condvar::new(),
            stats: WriterStatsAtomics::default(),
        }
    }

    /// Enqueue one whole ordered buffer. Returns the new queue depth on
    /// success, or the reason the write was rejected. This never blocks.
    pub fn push(&self, bytes: Vec<u8>) -> Result<usize, WriterQueueError> {
        let byte_count = bytes.len();
        let depth = {
            let mut inner = self.inner.lock().expect("writer mutex poisoned");
            match inner.state {
                WriterQueueState::Closed => {
                    drop(inner);
                    self.reject();
                    return Err(WriterQueueError::Closed);
                }
                WriterQueueState::Failed => {
                    drop(inner);
                    self.reject();
                    return Err(WriterQueueError::Failed);
                }
                WriterQueueState::Open => {}
            }
            if inner.pending_bytes.saturating_add(byte_count) > inner.byte_budget {
                let budget = inner.byte_budget;
                inner.state = WriterQueueState::Failed;
                drop(inner);
                self.reject();
                // Wake the writer so it observes the failed state and exits.
                self.changed.notify_all();
                return Err(WriterQueueError::ByteBudgetExceeded {
                    len: byte_count,
                    budget,
                });
            }
            inner.messages.push_back(bytes);
            inner.pending_bytes += byte_count;
            inner.messages.len()
        };
        self.stats.messages.fetch_add(1, Ordering::Relaxed);
        self.stats
            .bytes
            .fetch_add(byte_count as u64, Ordering::Relaxed);
        self.stats
            .max_depth
            .fetch_max(depth as u64, Ordering::Relaxed);
        self.changed.notify_one();
        Ok(depth)
    }

    fn reject(&self) {
        self.stats.rejected.fetch_add(1, Ordering::Relaxed);
    }

    /// Block until one message is available. `None` means the queue was closed
    /// or failed *and* every already-queued buffer has been drained, which is
    /// the writer thread's shutdown signal.
    #[must_use]
    pub fn pop_blocking(&self) -> Option<Vec<u8>> {
        let mut inner = self.inner.lock().expect("writer mutex poisoned");
        loop {
            if let Some(bytes) = inner.messages.pop_front() {
                inner.pending_bytes = inner.pending_bytes.saturating_sub(bytes.len());
                return Some(bytes);
            }
            if inner.state != WriterQueueState::Open {
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
        let mut inner = self.inner.lock().expect("writer mutex poisoned");
        let bytes = inner.messages.pop_front()?;
        inner.pending_bytes = inner.pending_bytes.saturating_sub(bytes.len());
        Some(bytes)
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

    /// Bytes currently held by queued buffers. Tracks message removals from
    /// both `pop` and `pop_blocking`.
    #[must_use]
    pub fn pending_bytes(&self) -> usize {
        self.inner
            .lock()
            .expect("writer mutex poisoned")
            .pending_bytes
    }

    #[must_use]
    pub fn state(&self) -> WriterQueueState {
        self.inner.lock().expect("writer mutex poisoned").state
    }

    /// Signal normal shutdown and wake the writer thread. Already-queued bytes
    /// stay queued so they can be flushed before the writer exits. A queue
    /// that already failed keeps its `Failed` state for diagnostics.
    pub fn close(&self) {
        let mut inner = self.inner.lock().expect("writer mutex poisoned");
        if inner.state == WriterQueueState::Open {
            inner.state = WriterQueueState::Closed;
        }
        drop(inner);
        self.changed.notify_all();
    }

    /// Record a transport failure, wake the writer, and reject new writes.
    ///
    /// Policy: already-queued complete buffers remain available to `pop` and
    /// `pop_blocking`, preserving FIFO order if a still-functioning consumer
    /// needs to flush them. The platform writer calls `fail()` and exits when
    /// the writer itself failed; any buffers it could not send are discarded
    /// with the queue. `fail()` never re-enters Ghostty or acquires a terminal
    /// lock.
    pub fn fail(&self) {
        let mut inner = self.inner.lock().expect("writer mutex poisoned");
        inner.state = WriterQueueState::Failed;
        drop(inner);
        self.changed.notify_all();
    }

    #[must_use]
    pub fn stats(&self) -> WriterStats {
        WriterStats {
            messages: self.stats.messages.load(Ordering::Relaxed),
            bytes: self.stats.bytes.load(Ordering::Relaxed),
            max_depth: self.stats.max_depth.load(Ordering::Relaxed),
            rejected: self.stats.rejected.load(Ordering::Relaxed),
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
        assert_eq!(queue.push(b"a".to_vec()), Ok(1));
        assert_eq!(queue.push(b"b".to_vec()), Ok(2));
        assert_eq!(queue.pop().expect("first"), b"a");
        assert_eq!(queue.pop().expect("second"), b"b");
        assert!(queue.is_empty());
        let stats = queue.stats();
        assert_eq!(stats.messages, 2);
        assert_eq!(stats.bytes, 2);
        assert_eq!(stats.max_depth, 2);
        assert_eq!(stats.rejected, 0);
    }

    #[test]
    fn pop_decrements_pending_bytes() {
        let queue = WriterQueue::with_byte_budget(8);
        assert_eq!(queue.push(b"abcd".to_vec()), Ok(1));
        assert_eq!(queue.push(b"ef".to_vec()), Ok(2));
        assert_eq!(queue.pending_bytes(), 6);
        assert_eq!(queue.pop_blocking().expect("first"), b"abcd");
        assert_eq!(queue.pending_bytes(), 2);
        assert_eq!(queue.pop().expect("second"), b"ef");
        assert_eq!(queue.pending_bytes(), 0);
        // Capacity is released by a pop, so a later write still fits.
        assert_eq!(queue.push(b"gh".to_vec()), Ok(1));
        assert_eq!(queue.state(), WriterQueueState::Open);
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
        assert_eq!(queue.push(b"pending".to_vec()), Ok(1));
        queue.close();
        assert_eq!(queue.pop_blocking().expect("flushed"), b"pending");
        assert_eq!(queue.pop_blocking(), None);
        assert_eq!(
            queue.push(b"ignored".to_vec()),
            Err(WriterQueueError::Closed)
        );
        assert_eq!(queue.state(), WriterQueueState::Closed);
    }

    #[test]
    fn push_after_close_is_rejected() {
        let queue = WriterQueue::new();
        queue.close();
        assert_eq!(queue.push(b"x".to_vec()), Err(WriterQueueError::Closed));
        assert_eq!(queue.stats().rejected, 1);
        assert_eq!(queue.len(), 0);
    }

    #[test]
    fn byte_budget_overflow_fails_the_queue() {
        let queue = WriterQueue::with_byte_budget(8);
        assert_eq!(queue.push(b"12345678".to_vec()), Ok(1));
        assert_eq!(
            queue.push(b"9".to_vec()),
            Err(WriterQueueError::ByteBudgetExceeded { len: 1, budget: 8 })
        );
        assert_eq!(queue.state(), WriterQueueState::Failed);
        assert_eq!(queue.stats().rejected, 1);
        // The original buffer is intact and can still be drained.
        assert_eq!(queue.pop().expect("original"), b"12345678");
        assert_eq!(queue.pop(), None);
    }

    #[test]
    fn a_single_buffer_larger_than_the_budget_fails_explicitly() {
        // Bracketed paste must fit as one coherent buffer or fail; splitting
        // it could reorder terminal replies relative to user input.
        let queue = WriterQueue::with_byte_budget(4);
        assert_eq!(
            queue.push(b"too large".to_vec()),
            Err(WriterQueueError::ByteBudgetExceeded { len: 9, budget: 4 })
        );
        assert_eq!(queue.state(), WriterQueueState::Failed);
        assert!(queue.is_empty());
    }

    #[test]
    fn failure_wakes_a_blocked_writer() {
        let queue = Arc::new(WriterQueue::new());
        let waiter = Arc::clone(&queue);
        let handle = std::thread::spawn(move || waiter.pop_blocking());
        std::thread::sleep(std::time::Duration::from_millis(10));
        queue.fail();
        assert!(handle.join().expect("writer thread").is_none());
    }

    #[test]
    fn no_write_is_accepted_after_failure() {
        let queue = WriterQueue::new();
        queue.fail();
        assert_eq!(queue.push(b"a".to_vec()), Err(WriterQueueError::Failed));
        assert_eq!(queue.push(b"b".to_vec()), Err(WriterQueueError::Failed));
        assert_eq!(queue.stats().rejected, 2);
        assert_eq!(queue.state(), WriterQueueState::Failed);
    }
}
