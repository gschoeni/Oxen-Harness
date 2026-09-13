//! The tree budget: one wallet shared by every subagent a root turn spawns,
//! however deep. Depth caps alone don't bound cost — a lane that may spawn
//! lanes multiplies it — so tokens, model calls, and spawns are counted
//! across the whole tree, and exhaustion returns best-so-far (a lane is
//! told to stop and keep what it has) rather than an error the root can't
//! use.
//!
//! The root's own spend is not counted: the session budget bounds that. The
//! budget resets when a root turn starts, so a tree is one turn's worth of
//! delegated work.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use serde::Serialize;

/// The tree's ceilings. Every field is a hard line for the subtree of one
/// root turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TreeLimits {
    /// Tokens (prompt + completion) every lane of the tree may spend together.
    pub max_tokens: u64,
    /// Model calls every lane may make together.
    pub max_requests: u32,
    /// Lanes that may be spawned in the tree, all depths counted.
    pub max_spawns: u32,
}

impl Default for TreeLimits {
    fn default() -> Self {
        Self {
            max_tokens: 1_500_000,
            max_requests: 200,
            max_spawns: 24,
        }
    }
}

/// Where a tree stands against its limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct TreeUsage {
    pub tokens: u64,
    pub requests: u32,
    pub spawns: u32,
}

/// The shared wallet (see the module docs). Cheap to share: three atomics.
#[derive(Debug)]
pub struct TreeBudget {
    limits: TreeLimits,
    tokens: AtomicU64,
    requests: AtomicU32,
    spawns: AtomicU32,
    /// Fleets running right now. A `wait: false` fleet outlives the turn
    /// that spawned it, and a reset landing mid-flight would hand it a fresh
    /// wallet; the reset waits until the last fleet ends.
    inflight: AtomicU32,
    pending_reset: AtomicBool,
}

impl Default for TreeBudget {
    fn default() -> Self {
        Self::new(TreeLimits::default())
    }
}

impl TreeBudget {
    pub fn new(limits: TreeLimits) -> Self {
        Self {
            limits,
            tokens: AtomicU64::new(0),
            requests: AtomicU32::new(0),
            spawns: AtomicU32::new(0),
            inflight: AtomicU32::new(0),
            pending_reset: AtomicBool::new(false),
        }
    }

    pub fn limits(&self) -> TreeLimits {
        self.limits
    }

    /// A new root turn: the tree starts over — once no fleet is in flight.
    pub fn reset(&self) {
        if self.inflight.load(Ordering::SeqCst) > 0 {
            self.pending_reset.store(true, Ordering::SeqCst);
            return;
        }
        self.clear();
    }

    fn clear(&self) {
        self.tokens.store(0, Ordering::Relaxed);
        self.requests.store(0, Ordering::Relaxed);
        self.spawns.store(0, Ordering::Relaxed);
        self.pending_reset.store(false, Ordering::SeqCst);
    }

    /// A fleet is starting; the wallet stays as it is until it ends.
    pub fn begin_fleet(&self) {
        self.inflight.fetch_add(1, Ordering::SeqCst);
    }

    /// A fleet ended; a reset a root turn asked for meanwhile lands now.
    pub fn end_fleet(&self) {
        let remaining = self
            .inflight
            .fetch_sub(1, Ordering::SeqCst)
            .saturating_sub(1);
        if remaining == 0 && self.pending_reset.load(Ordering::SeqCst) {
            self.clear();
        }
    }

    /// Count one model call of `tokens` by a lane.
    pub fn charge(&self, tokens: u64) {
        self.tokens.fetch_add(tokens, Ordering::Relaxed);
        self.requests.fetch_add(1, Ordering::Relaxed);
    }

    /// Reserve `count` lanes, or say why not.
    pub fn admit_spawn(&self, count: u32) -> Result<(), String> {
        if let Some(reason) = self.exhausted() {
            return Err(reason);
        }
        let mut current = self.spawns.load(Ordering::Relaxed);
        loop {
            let next = current.saturating_add(count);
            if next > self.limits.max_spawns {
                return Err(format!(
                    "the tree budget allows {} agents per turn and {} have been spawned; \
                     finish with what they returned or ask for fewer",
                    self.limits.max_spawns, current
                ));
            }
            match self.spawns.compare_exchange_weak(
                current,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return Ok(()),
                Err(actual) => current = actual,
            }
        }
    }

    /// Why a lane should stop now, if the tree has spent its tokens or calls.
    pub fn exhausted(&self) -> Option<String> {
        let usage = self.usage();
        if usage.tokens >= self.limits.max_tokens {
            return Some(format!(
                "the agents of this turn have spent their shared budget of {} tokens",
                self.limits.max_tokens
            ));
        }
        if usage.requests >= self.limits.max_requests {
            return Some(format!(
                "the agents of this turn have made their shared budget of {} model calls",
                self.limits.max_requests
            ));
        }
        None
    }

    pub fn usage(&self) -> TreeUsage {
        TreeUsage {
            tokens: self.tokens.load(Ordering::Relaxed),
            requests: self.requests.load(Ordering::Relaxed),
            spawns: self.spawns.load(Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawns_tokens_and_calls_share_one_wallet_that_a_new_turn_resets() {
        let budget = TreeBudget::new(TreeLimits {
            max_tokens: 100,
            max_requests: 3,
            max_spawns: 4,
        });
        assert!(budget.admit_spawn(3).is_ok());
        let err = budget.admit_spawn(2).unwrap_err();
        assert!(err.contains("allows 4 agents"), "{err}");
        assert!(budget.admit_spawn(1).is_ok(), "exactly the cap is fine");

        budget.charge(60);
        assert!(budget.exhausted().is_none());
        budget.charge(60);
        assert!(budget.exhausted().unwrap().contains("100 tokens"));
        assert!(budget.admit_spawn(0).is_err(), "no new lanes once spent");

        budget.reset();
        assert!(budget.exhausted().is_none());

        // A reset while a fleet is in flight waits for the fleet to end.
        budget.begin_fleet();
        budget.charge(50);
        budget.reset();
        assert_eq!(budget.usage().tokens, 50, "not yet");
        budget.end_fleet();
        assert_eq!(budget.usage().tokens, 0, "landed once the fleet ended");

        for _ in 0..3 {
            budget.charge(1);
        }
        assert!(budget.exhausted().unwrap().contains("3 model calls"));
        assert_eq!(
            budget.usage(),
            TreeUsage {
                tokens: 3,
                requests: 3,
                spawns: 0
            }
        );
    }
}
