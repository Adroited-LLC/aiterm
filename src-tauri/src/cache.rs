//! A value that is expensive to produce and cheap to reuse for a moment.
//!
//! Written for the agent roster, which costs a whole subprocess to read, and
//! kept general because every agent backend will have one of these. A CLI
//! agent has to be asked — a process spawn per question — while an API-backed
//! session knows its own liveness for free. The ones that have to be asked are
//! the ones that need this, and they should not each grow their own cache.
//!
//! The contract is deliberately small: `get` may return a value up to `ttl`
//! old, `refresh` never does. Anything that acts on the answer — stopping a
//! session, then checking whether it stopped — must use `refresh`, because a
//! cached "still running" would be indistinguishable from a stop that failed.

use std::sync::Mutex;
use std::time::{Duration, Instant};

pub struct TtlCache<T> {
    ttl: Duration,
    /// `None` until the first read. Poisoning is treated as "no value": this
    /// caches a recomputable answer, so a panicked writer costs one extra
    /// call, never a wrong one.
    slot: Mutex<Option<(Instant, T)>>,
}

impl<T: Clone> TtlCache<T> {
    pub const fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            slot: Mutex::new(None),
        }
    }

    /// The cached value if it is younger than `ttl`, otherwise a fresh one.
    pub fn get(&self, produce: impl FnOnce() -> T) -> T {
        if let Ok(slot) = self.slot.lock() {
            if let Some((at, value)) = slot.as_ref() {
                if at.elapsed() < self.ttl {
                    return value.clone();
                }
            }
        }
        self.refresh(produce)
    }

    /// Produce a new value and store it, ignoring whatever was cached.
    ///
    /// The lock is not held across `produce`. Two callers racing here both run
    /// it and the later write wins, which costs a duplicate call in a rare
    /// window — the alternative, holding the lock through a subprocess, would
    /// block every other reader for the length of it.
    pub fn refresh(&self, produce: impl FnOnce() -> T) -> T {
        let value = produce();
        if let Ok(mut slot) = self.slot.lock() {
            *slot = Some((Instant::now(), value.clone()));
        }
        value
    }

    /// Drop the cached value, so the next `get` produces afresh. For use after
    /// something is known to have changed the underlying state.
    pub fn invalidate(&self) {
        if let Ok(mut slot) = self.slot.lock() {
            *slot = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Counts how many times the producer actually ran.
    fn counting(n: &AtomicUsize) -> impl FnOnce() -> usize + '_ {
        move || n.fetch_add(1, Ordering::SeqCst) + 1
    }

    #[test]
    fn a_second_read_inside_the_ttl_does_not_produce_again() {
        let calls = AtomicUsize::new(0);
        let cache = TtlCache::new(Duration::from_secs(60));
        assert_eq!(cache.get(counting(&calls)), 1);
        assert_eq!(
            cache.get(counting(&calls)),
            1,
            "returned a newly produced value"
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "produced twice inside the ttl"
        );
    }

    #[test]
    fn an_expired_value_is_produced_again() {
        let calls = AtomicUsize::new(0);
        // Zero ttl: nothing is ever young enough, so every get produces.
        let cache = TtlCache::new(Duration::ZERO);
        cache.get(counting(&calls));
        cache.get(counting(&calls));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn ttl_is_measured_in_real_time() {
        let calls = AtomicUsize::new(0);
        let cache = TtlCache::new(Duration::from_millis(30));
        cache.get(counting(&calls));
        cache.get(counting(&calls));
        assert_eq!(calls.load(Ordering::SeqCst), 1, "expired early");
        std::thread::sleep(Duration::from_millis(45));
        cache.get(counting(&calls));
        assert_eq!(calls.load(Ordering::SeqCst), 2, "did not expire");
    }

    /// The property the stop path depends on: after acting on the world, a
    /// caller must be able to ask again and not be told what it was told
    /// before. A cached "still running" reads exactly like a failed stop.
    #[test]
    fn refresh_ignores_a_still_valid_cache_and_reseeds_it() {
        let calls = AtomicUsize::new(0);
        let cache = TtlCache::new(Duration::from_secs(60));
        assert_eq!(cache.get(counting(&calls)), 1);
        assert_eq!(
            cache.refresh(counting(&calls)),
            2,
            "refresh served a cached value"
        );
        assert_eq!(
            cache.get(counting(&calls)),
            2,
            "refresh did not reseed the cache"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn invalidate_forces_the_next_read_to_produce() {
        let calls = AtomicUsize::new(0);
        let cache = TtlCache::new(Duration::from_secs(60));
        cache.get(counting(&calls));
        cache.invalidate();
        cache.get(counting(&calls));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}
