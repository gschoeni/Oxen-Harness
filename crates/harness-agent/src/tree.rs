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

use std::sync::Mutex;

use serde::Serialize;

/// The tree's ceilings. Every field is a hard line for the subtree of one
/// root turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TreeLimits {
    /// Tokens every lane of the tree may spend together, counted as the
    /// provider bills them: uncached prompt + cache writes + completion (see
    /// `budget::billable_tokens`). A cached prefix re-sent on every tool
    /// round costs the wallet nothing, as it costs the bill nothing.
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
    /// Billable tokens spent so far (see [`TreeLimits::max_tokens`]).
    pub tokens: u64,
    pub requests: u32,
    pub spawns: u32,
}

/// The shared wallet (see the module docs). Admissions and resets are serialized under one lock.
#[derive(Debug)]
pub struct TreeBudget {
    limits: TreeLimits,
    state: Mutex<BudgetState>,
}

#[derive(Debug, Default)]
struct BudgetState {
    tokens: u64,
    requests: u32,
    spawns: u32,
    inflight: u32,
    pending_reset: bool,
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
            state: Mutex::default(),
        }
    }
    pub fn limits(&self) -> TreeLimits {
        self.limits
    }
    pub fn reset(&self) {
        let mut state = self.state.lock().expect("tree budget poisoned");
        if state.inflight > 0 {
            state.pending_reset = true;
        } else {
            *state = BudgetState::default();
        }
    }
    pub fn begin_fleet(&self) {
        self.state.lock().expect("tree budget poisoned").inflight += 1;
    }
    pub fn end_fleet(&self) {
        let mut state = self.state.lock().expect("tree budget poisoned");
        state.inflight = state.inflight.saturating_sub(1);
        if state.inflight == 0 && state.pending_reset {
            *state = BudgetState::default();
        }
    }
    /// Admit a model round atomically before sending it, including leaf calls.
    pub fn reserve_request(&self) -> Result<(), String> {
        let mut state = self.state.lock().expect("tree budget poisoned");
        if let Some(reason) = self.reason(&state) {
            return Err(reason);
        }
        state.requests += 1;
        Ok(())
    }
    /// Admit the one tool-free call a lane makes to write up after its
    /// wallet is spent (see `Agent::final_report`). Counted, never refused:
    /// the report is what makes a stopped lane's result usable, and the
    /// caller bounds it to one per turn.
    pub fn reserve_report(&self) {
        self.state.lock().expect("tree budget poisoned").requests += 1;
    }
    pub fn charge_tokens(&self, tokens: u64) {
        let mut state = self.state.lock().expect("tree budget poisoned");
        state.tokens = state.tokens.saturating_add(tokens);
    }
    /// Account for a caller that already made a request without a reservation.
    pub fn charge(&self, tokens: u64) {
        let mut state = self.state.lock().expect("tree budget poisoned");
        state.tokens = state.tokens.saturating_add(tokens);
        state.requests += 1;
    }
    pub fn admit_spawn(&self, count: u32) -> Result<(), String> {
        let mut state = self.state.lock().expect("tree budget poisoned");
        if let Some(reason) = self.reason(&state) {
            return Err(reason);
        }
        if state.spawns.saturating_add(count) > self.limits.max_spawns {
            return Err(format!("the tree budget allows {} agents per turn and {} have been spawned; finish with what they returned or ask for fewer", self.limits.max_spawns, state.spawns));
        }
        state.spawns += count;
        Ok(())
    }
    fn reason(&self, state: &BudgetState) -> Option<String> {
        if state.tokens >= self.limits.max_tokens {
            Some(format!(
                "the agents of this turn have spent their shared budget of {} tokens",
                self.limits.max_tokens
            ))
        } else if state.requests >= self.limits.max_requests {
            Some(format!(
                "the agents of this turn have made their shared budget of {} model calls",
                self.limits.max_requests
            ))
        } else {
            None
        }
    }
    pub fn exhausted(&self) -> Option<String> {
        self.reason(&self.state.lock().expect("tree budget poisoned"))
    }
    pub fn usage(&self) -> TreeUsage {
        let state = self.state.lock().expect("tree budget poisoned");
        TreeUsage {
            tokens: state.tokens,
            requests: state.requests,
            spawns: state.spawns,
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
