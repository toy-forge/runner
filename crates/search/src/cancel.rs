//! A tiny cancellation token shared by the long-running parts of the crate.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Cooperative cancellation: clone it, hand the clone to a worker, call
/// [`cancel`](CancelToken::cancel) from anywhere. All clones share the flag.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    /// A fresh, not cancelled token.
    pub fn new() -> Self {
        Self::default()
    }

    /// Requests cancellation (idempotent).
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    /// Whether [`cancel`](CancelToken::cancel) was called on any clone.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}
