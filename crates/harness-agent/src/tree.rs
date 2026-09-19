//! The tree budget: one wallet per root turn, carved into a wallet per lane.
//!
//! Depth caps alone don't bound cost — a lane that may spawn lanes multiplies
//! it — so billable tokens, model calls, and spawns are counted across the
//! whole tree of one root turn. Within that ceiling every lane opens with an
//! allowance of its own: an equal share of what its parent has left, so one
//! greedy lane cannot drain its siblings, and a lane that spawns a fleet
//! funds it from its own share. When a lane closes, what it spent folds into
//! its parent and the rest of its allowance goes back. Exhaustion — of the
//! tree or of one allowance — ends that lane's turn with a final report
//! (see `Agent::final_report`), never with an error the root can't use.
//!
//! The root's own spend is not counted: the session budget bounds that. The
//! budget resets when a root turn starts, so a tree is one turn's worth of
//! delegated work.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

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
    /// Lane model calls in flight at once, across every fleet of the tree
    /// at every depth. A per-fleet limit bounds one fleet; three fleets of
    /// three with lanes spawning fleets of their own is what a provider's
    /// rate limit counts, and this is the line it sees.
    pub max_parallel: u32,
}

impl Default for TreeLimits {
    fn default() -> Self {
        Self {
            max_tokens: 1_500_000,
            max_requests: 200,
            max_spawns: 24,
            max_parallel: 4,
        }
    }
}

/// The least a lane is worth opening with. A fleet its parent's remaining
/// budget can't fund at this rate is refused at admission, with the number,
/// rather than started to stop after a call or two.
pub const MIN_LANE_TOKENS: u64 = 8_000;

/// Where a tree stands against its limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct TreeUsage {
    /// Billable tokens spent so far (see [`TreeLimits::max_tokens`]).
    pub tokens: u64,
    pub requests: u32,
    pub spawns: u32,
}

/// One lane's standing against its own allowance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Allowance {
    pub cap: u64,
    pub spent: u64,
}

impl Allowance {
    pub fn left(&self) -> u64 {
        self.cap.saturating_sub(self.spent)
    }
}

/// The root's wallet, keyed by the empty id no lane can have.
const ROOT: &str = "";

#[derive(Debug)]
struct Wallet {
    parent: String,
    cap: u64,
    /// This lane's own spend, plus everything its closed children spent.
    spent: u64,
    /// The allowances of its open children, promised and not yet folded.
    reserved: u64,
    /// Lanes about to open under it; each takes an equal share of what is
    /// left, so a fleet's lanes get the same allowance whatever order they
    /// open in.
    expected: u32,
}

impl Wallet {
    fn left(&self) -> u64 {
        self.cap
            .saturating_sub(self.spent)
            .saturating_sub(self.reserved)
    }
}

/// The shared wallet (see the module docs). Admissions, opens, and resets
/// are serialized under one lock.
#[derive(Debug)]
pub struct TreeBudget {
    limits: TreeLimits,
    state: Mutex<BudgetState>,
    /// The [`TreeLimits::max_parallel`] call slots; never closed.
    slots: Arc<Semaphore>,
}

#[derive(Debug, Default)]
struct BudgetState {
    tokens: u64,
    requests: u32,
    spawns: u32,
    inflight: u32,
    pending_reset: bool,
    wallets: HashMap<String, Wallet>,
}

impl BudgetState {
    /// The wallet `key` spends from: its own, or the root's for a parent
    /// that never opened as a lane (a side agent, a test).
    fn resolve<'a>(&self, key: &'a str) -> &'a str {
        if key != ROOT && !self.wallets.contains_key(key) {
            ROOT
        } else {
            key
        }
    }

    /// The wallet `key` spends from, creating the root's on first use.
    fn wallet_of(&mut self, key: &str, max_tokens: u64) -> &mut Wallet {
        let key = self.resolve(key);
        self.wallets
            .entry(key.to_string())
            .or_insert_with(|| Wallet {
                parent: ROOT.to_string(),
                cap: max_tokens,
                spent: 0,
                reserved: 0,
                expected: 0,
            })
    }

    /// Close a lane's wallet: its children first, then its spend into its
    /// parent and its allowance back to the parent's pool.
    fn fold(&mut self, lane: &str) {
        let children: Vec<String> = self
            .wallets
            .iter()
            .filter(|(id, w)| w.parent == lane && id.as_str() != lane)
            .map(|(id, _)| id.clone())
            .collect();
        for child in children {
            self.fold(&child);
        }
        if let Some(wallet) = self.wallets.remove(lane) {
            if let Some(parent) = self.wallets.get_mut(&wallet.parent) {
                parent.spent = parent.spent.saturating_add(wallet.spent);
                parent.reserved = parent.reserved.saturating_sub(wallet.cap);
            }
        }
    }
}

impl Default for TreeBudget {
    fn default() -> Self {
        Self::new(TreeLimits::default())
    }
}

fn key(parent: Option<&str>) -> &str {
    parent.unwrap_or(ROOT)
}

impl TreeBudget {
    pub fn new(limits: TreeLimits) -> Self {
        Self {
            limits,
            state: Mutex::default(),
            slots: Arc::new(Semaphore::new(limits.max_parallel.max(1) as usize)),
        }
    }

    /// Wait for one of the tree's call slots (see
    /// [`TreeLimits::max_parallel`]); the permit is the slot, released on
    /// drop. Held around a model call only, never across a tool call or a
    /// wait on a child fleet, so a parent lane never starves its children.
    pub async fn call_slot(&self) -> Result<OwnedSemaphorePermit, String> {
        self.slots
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| "the tree's call slots were closed".to_string())
    }

    /// Call slots free right now, for readouts and tests.
    pub fn free_slots(&self) -> usize {
        self.slots.available_permits()
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

    /// Admit `count` lanes under `parent` (`None`: the root turn itself):
    /// refused when the tree is spent, past its spawn cap, or when the
    /// parent's remaining allowance can't give each lane
    /// [`MIN_LANE_TOKENS`]. Nothing is reserved here; each lane takes its
    /// share when it opens.
    pub fn admit_spawn(&self, parent: Option<&str>, count: u32) -> Result<(), String> {
        let mut state = self.state.lock().expect("tree budget poisoned");
        if let Some(reason) = self.reason(&state, key(parent)) {
            return Err(reason);
        }
        if state.spawns.saturating_add(count) > self.limits.max_spawns {
            return Err(format!(
                "the tree budget allows {} agents per turn and {} have been spawned; finish \
                 with what they returned or ask for fewer",
                self.limits.max_spawns, state.spawns
            ));
        }
        let left = state.wallet_of(key(parent), self.limits.max_tokens).left();
        if count > 0 && left / u64::from(count) < MIN_LANE_TOKENS {
            return Err(format!(
                "the {left} tokens left in this agent's budget can't fund {count} more \
                 agents (each opens with at least {MIN_LANE_TOKENS}); finish with what you \
                 have, or ask for fewer"
            ));
        }
        state.spawns += count;
        Ok(())
    }

    /// Announce `count` lanes about to open under `parent`, so they split
    /// what it has left evenly rather than the first taking everything.
    pub fn expect_lanes(&self, parent: Option<&str>, count: u32) {
        let mut state = self.state.lock().expect("tree budget poisoned");
        let wallet = state.wallet_of(key(parent), self.limits.max_tokens);
        wallet.expected = wallet.expected.saturating_add(count);
    }

    /// Withdraw an announcement whose lanes never opened.
    pub fn forget_lanes(&self, parent: Option<&str>, count: u32) {
        let mut state = self.state.lock().expect("tree budget poisoned");
        let wallet = state.wallet_of(key(parent), self.limits.max_tokens);
        wallet.expected = wallet.expected.saturating_sub(count);
    }

    /// Open `lane`'s wallet under `parent` with an equal share of what the
    /// parent has left for the lanes it expects.
    pub fn open_lane(&self, parent: Option<&str>, lane: &str) {
        let mut state = self.state.lock().expect("tree budget poisoned");
        let parent_key = state.resolve(key(parent)).to_string();
        let parent_wallet = state.wallet_of(&parent_key, self.limits.max_tokens);
        let share = u64::from(parent_wallet.expected.max(1));
        let cap = (parent_wallet.left() / share).max(1);
        parent_wallet.expected = parent_wallet.expected.saturating_sub(1);
        parent_wallet.reserved = parent_wallet.reserved.saturating_add(cap);
        state.wallets.insert(
            lane.to_string(),
            Wallet {
                parent: parent_key,
                cap,
                spent: 0,
                reserved: 0,
                expected: 0,
            },
        );
    }

    /// Close `lane`'s wallet (and any child it left open): its spend folds
    /// into its parent, the rest of its allowance goes back.
    pub fn release_lane(&self, lane: &str) {
        let mut state = self.state.lock().expect("tree budget poisoned");
        if lane != ROOT {
            state.fold(lane);
        }
    }

    /// Admit a model round for `lane` atomically before sending it.
    pub fn reserve_request(&self, lane: &str) -> Result<(), String> {
        let mut state = self.state.lock().expect("tree budget poisoned");
        if let Some(reason) = self.reason(&state, lane) {
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
    /// Charge a call's billable tokens to `lane` and to the tree.
    pub fn charge_tokens(&self, lane: &str, tokens: u64) {
        let mut state = self.state.lock().expect("tree budget poisoned");
        state.tokens = state.tokens.saturating_add(tokens);
        if let Some(wallet) = state.wallets.get_mut(lane) {
            wallet.spent = wallet.spent.saturating_add(tokens);
        }
    }
    /// Account for a caller that already made a request without a
    /// reservation, against the tree alone.
    pub fn charge(&self, tokens: u64) {
        let mut state = self.state.lock().expect("tree budget poisoned");
        state.tokens = state.tokens.saturating_add(tokens);
        state.requests += 1;
    }

    /// Why `lane` may not call the model again, if it may not: the tree's
    /// ceilings first, then the lane's own allowance.
    fn reason(&self, state: &BudgetState, lane: &str) -> Option<String> {
        if state.tokens >= self.limits.max_tokens {
            return Some(format!(
                "the agents of this turn have spent their shared budget of {} tokens",
                self.limits.max_tokens
            ));
        }
        if state.requests >= self.limits.max_requests {
            return Some(format!(
                "the agents of this turn have made their shared budget of {} model calls",
                self.limits.max_requests
            ));
        }
        let wallet = state.wallets.get(lane)?;
        (wallet.spent >= wallet.cap).then(|| {
            format!(
                "this agent has spent its allowance of {} tokens ({} of the {} tokens this \
                 turn's agents share are used)",
                wallet.cap, state.tokens, self.limits.max_tokens
            )
        })
    }
    pub fn exhausted(&self, lane: &str) -> Option<String> {
        self.reason(&self.state.lock().expect("tree budget poisoned"), lane)
    }
    /// What `parent` (`None`: the root turn) has left to fund more lanes.
    pub fn remaining_for(&self, parent: Option<&str>) -> u64 {
        let mut state = self.state.lock().expect("tree budget poisoned");
        state.wallet_of(key(parent), self.limits.max_tokens).left()
    }
    /// Where `lane` stands against its own allowance, once it has opened.
    pub fn allowance(&self, lane: &str) -> Option<Allowance> {
        let state = self.state.lock().expect("tree budget poisoned");
        state.wallets.get(lane).map(|w| Allowance {
            cap: w.cap,
            spent: w.spent,
        })
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
            max_tokens: 100_000,
            max_requests: 3,
            max_spawns: 4,
            max_parallel: 4,
        });
        assert!(budget.admit_spawn(None, 3).is_ok());
        let err = budget.admit_spawn(None, 2).unwrap_err();
        assert!(err.contains("allows 4 agents"), "{err}");
        assert!(
            budget.admit_spawn(None, 1).is_ok(),
            "exactly the cap is fine"
        );

        budget.charge(60_000);
        assert!(budget.exhausted("any").is_none());
        budget.charge(60_000);
        assert!(budget.exhausted("any").unwrap().contains("100000 tokens"));
        assert!(
            budget.admit_spawn(None, 0).is_err(),
            "no new lanes once spent"
        );

        budget.reset();
        assert!(budget.exhausted("any").is_none());

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
        assert!(budget.exhausted("any").unwrap().contains("3 model calls"));
        assert_eq!(
            budget.usage(),
            TreeUsage {
                tokens: 3,
                requests: 3,
                spawns: 0
            }
        );
    }

    #[test]
    fn lanes_open_with_equal_shares_that_fold_back_into_their_parent() {
        let budget = TreeBudget::new(TreeLimits {
            max_tokens: 120_000,
            ..Default::default()
        });
        // Three lanes announced: each opens with a third of the root's pool,
        // whatever order they open in.
        budget.expect_lanes(None, 3);
        for lane in ["a", "b", "c"] {
            budget.open_lane(None, lane);
            assert_eq!(budget.allowance(lane).unwrap().cap, 40_000, "{lane}");
        }
        // A lane funds its own fleet from what it has left.
        budget.charge_tokens("a", 10_000);
        assert_eq!(budget.allowance("a").unwrap().left(), 30_000);
        budget.expect_lanes(Some("a"), 2);
        budget.open_lane(Some("a"), "a1");
        budget.open_lane(Some("a"), "a2");
        assert_eq!(budget.allowance("a1").unwrap().cap, 15_000);
        assert_eq!(budget.allowance("a2").unwrap().cap, 15_000);
        // One child overspends (its report call, say): it alone is stopped.
        budget.charge_tokens("a1", 20_000);
        assert!(budget
            .exhausted("a1")
            .unwrap()
            .contains("allowance of 15000"));
        assert!(budget.exhausted("a2").is_none());
        assert!(budget.exhausted("b").is_none());
        // Closing folds spend into the parent and returns the allowance.
        budget.release_lane("a1");
        assert_eq!(budget.allowance("a").unwrap().spent, 30_000);
        budget.release_lane("a2");
        budget.release_lane("a");
        assert!(budget.allowance("a").is_none());
        // The root now holds a's 30k spend and b + c's 80k promise: 10k left,
        // enough for one more lane but not two.
        let err = budget.admit_spawn(None, 2).unwrap_err();
        assert!(
            err.contains("10000 tokens left") && err.contains("can't fund 2"),
            "{err}"
        );
        assert!(budget.admit_spawn(None, 1).is_ok());
        assert_eq!(
            budget.usage().tokens,
            30_000,
            "the tree counts every charge"
        );
    }

    #[test]
    fn a_lane_that_closes_with_open_children_folds_them_first() {
        let budget = TreeBudget::new(TreeLimits {
            max_tokens: 100_000,
            ..Default::default()
        });
        budget.expect_lanes(None, 1);
        budget.open_lane(None, "p");
        budget.expect_lanes(Some("p"), 1);
        budget.open_lane(Some("p"), "child");
        budget.charge_tokens("child", 5_000);
        budget.release_lane("p");
        assert!(budget.allowance("child").is_none());
        assert!(budget.allowance("p").is_none());
        // Everything the subtree spent is back in the root's ledger.
        budget.expect_lanes(None, 1);
        budget.open_lane(None, "next");
        assert_eq!(budget.allowance("next").unwrap().cap, 95_000);
    }

    #[tokio::test]
    async fn call_slots_bound_lane_calls_across_the_whole_tree() {
        let budget = TreeBudget::new(TreeLimits {
            max_parallel: 2,
            ..Default::default()
        });
        let first = budget.call_slot().await.unwrap();
        let second = budget.call_slot().await.unwrap();
        assert_eq!(budget.free_slots(), 0);
        // A third call waits until one of the two finishes.
        let third =
            tokio::time::timeout(std::time::Duration::from_millis(30), budget.call_slot()).await;
        assert!(third.is_err(), "no slot until a call ends");
        drop(first);
        let third = budget.call_slot().await.unwrap();
        drop((second, third));
        assert_eq!(budget.free_slots(), 2);
        // A zero cap still lets one call through: a tree that can never
        // call is a misconfiguration, not a feature.
        assert_eq!(
            TreeBudget::new(TreeLimits {
                max_parallel: 0,
                ..Default::default()
            })
            .free_slots(),
            1
        );
    }

    #[test]
    fn a_lane_never_opened_through_a_fleet_spends_from_the_tree_alone() {
        let budget = TreeBudget::new(TreeLimits {
            max_tokens: 100,
            ..Default::default()
        });
        budget.charge_tokens("stray", 60);
        assert!(budget.exhausted("stray").is_none());
        assert!(budget.allowance("stray").is_none());
        budget.charge_tokens("stray", 60);
        assert!(budget.exhausted("stray").unwrap().contains("shared budget"));
    }
}
