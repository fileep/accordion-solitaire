//! Self-checks of the fast engine against the reference engine (PLAN.md §6.3, Phase 2 gates): lockstep random
//! playouts, dead-end soundness, and exhaustive position counts. The unit tests run small batches; `acc selftest`
//! runs the full 10M-playout and 1M-position checks.

use crate::card::Card;
use crate::deal::Deal;
use crate::fast::{Position, connected};
use crate::game::Game;
use crate::rng::Xoshiro256StarStar;
use std::collections::HashSet;

/// Plays one random game from `deal` with both engines in lockstep. After every move they must agree on the top
/// cards and the legal moves; the incremental masks and hash must equal a recomputation; the move picker must
/// return a legal move; the dead-end tests must match naive implementations of their definitions; and every
/// disconnection must be flagged by the link-broken trigger. Returns the number of moves played.
pub fn differential_playout(deal: &Deal, rng: &mut Xoshiro256StarStar) -> Result<usize, String> {
    let mut game = Game::new(deal);
    let mut pos = Position::from_deal(deal);
    let mut moves = 0;
    loop {
        compare(&game, &pos).map_err(|e| format!("deal {deal}, after {moves} moves: {e}"))?;
        let Some(mv) = pos.random_move(rng) else { return Ok(moves) };
        let was_connected = connected(pos.present());
        game.apply(mv).map_err(|e| format!("deal {deal}, move {}: picked illegal {mv:?}: {e}", moves + 1))?;
        pos.apply(mv.pile as usize, mv.leap);
        moves += 1;
        if was_connected && !connected(pos.present()) && !pos.link_broken() {
            return Err(format!("deal {deal}, move {moves}: disconnected without the link-broken trigger"));
        }
    }
}

fn compare(game: &Game, pos: &Position) -> Result<(), String> {
    let tops = game.tops();
    if pos.tops() != tops {
        return Err(format!("top cards differ: fast {:?}, reference {tops:?}", pos.tops()));
    }
    if pos.legal_moves() != game.legal_moves() {
        return Err(format!(
            "legal moves differ: fast {:?}, reference {:?}",
            pos.legal_moves(),
            game.legal_moves()
        ));
    }
    if (pos.slides(), pos.leaps()) != pos.full_masks() {
        return Err("incremental move masks differ from a recomputation".into());
    }
    if pos.hash() != pos.full_hash() {
        return Err("incremental hash differs from a recomputation".into());
    }
    let rebuilt = Position::from_tops(&tops);
    if rebuilt.present() != pos.present() || rebuilt.hash() != pos.hash() {
        return Err("position differs from one rebuilt from the same top cards".into());
    }
    if connected(pos.present()) != naive_connected(&tops) {
        return Err("connectivity test disagrees with its definition".into());
    }
    if tops.len() <= 24 && pos.left_leaf_pair() != naive_left_leaf_pair(&tops) {
        return Err("left-leaf test disagrees with its definition".into());
    }
    Ok(())
}

/// Connectivity straight from the definition: every top card reachable from the first through shared suits or ranks.
pub fn naive_connected(tops: &[Card]) -> bool {
    let mut reached = vec![false; tops.len()];
    let mut stack = vec![0];
    reached[0] = true;
    while let Some(i) = stack.pop() {
        for j in 0..tops.len() {
            if !reached[j] && tops[i].matches(tops[j]) {
                reached[j] = true;
                stack.push(j);
            }
        }
    }
    reached.iter().all(|&r| r)
}

/// The left-leaf pair straight from the definition: at least two cards whose only match lies to their left.
pub fn naive_left_leaf_pair(tops: &[Card]) -> bool {
    let leaves = (0..tops.len())
        .filter(|&i| {
            let matches: Vec<usize> =
                (0..tops.len()).filter(|&j| j != i && tops[i].matches(tops[j])).collect();
            matches.len() == 1 && matches[0] < i
        })
        .count();
    leaves >= 2
}

/// Plain reference-engine search: can any line from `game` reach one pile? `None` when more than `budget`
/// distinct positions would have to be examined.
pub fn reference_winnable(game: &Game, budget: usize) -> Option<bool> {
    let mut seen = HashSet::from([game.tops()]);
    let mut stack = vec![game.clone()];
    while let Some(g) = stack.pop() {
        if g.is_won() {
            return Some(true);
        }
        for mv in g.legal_moves() {
            let mut next = g.clone();
            next.apply(mv).expect("legal_moves returns legal moves");
            if seen.insert(next.tops()) {
                if seen.len() > budget {
                    return None;
                }
                stack.push(next);
            }
        }
    }
    Some(false)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Soundness {
    /// No dead-end test fired, so there is nothing to confirm.
    NotFired,
    /// A test fired and the reference search confirmed no win exists.
    Confirmed,
    /// A test fired but the position was too large to search within the budget.
    Inconclusive,
}

/// Plays a random line from the deal for `seed` down to a random 6–22 piles, then checks that if any dead-end
/// test fires there, the reference engine cannot find a win.
pub fn soundness_sample(seed: u64, budget: usize) -> Result<Soundness, String> {
    let deal = Deal::from_seed(seed);
    let mut rng = Xoshiro256StarStar::from_seed(seed ^ 0x5EED);
    let target = 6 + rng.below(17) as usize;
    let mut game = Game::new(&deal);
    let mut pos = Position::from_deal(&deal);
    while pos.len() > target {
        let Some(mv) = pos.random_move(&mut rng) else { break };
        game.apply(mv).map_err(|e| format!("seed {seed}: {e}"))?;
        pos.apply(mv.pile as usize, mv.leap);
    }
    if connected(pos.present()) && !pos.left_leaf_pair() && pos.connected_without_leftmost() {
        return Ok(Soundness::NotFired);
    }
    match reference_winnable(&game, budget) {
        Some(false) => Ok(Soundness::Confirmed),
        None => Ok(Soundness::Inconclusive),
        Some(true) => {
            Err(format!("seed {seed}: a dead-end test fired on a winnable position {:?}", game.tops()))
        }
    }
}

/// Every distinct position reachable from `start` with the fast engine (start included), and whether any is a win.
pub fn explore_fast(start: Position) -> (usize, bool) {
    let mut seen = HashSet::from([start.codes().to_vec()]);
    let mut stack = vec![start];
    let mut winnable = false;
    while let Some(pos) = stack.pop() {
        winnable |= pos.is_won();
        for mv in pos.legal_moves() {
            let next = pos.play(mv);
            if seen.insert(next.codes().to_vec()) {
                stack.push(next);
            }
        }
    }
    (seen.len(), winnable)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engines_agree_on_random_playouts() {
        let mut moves = 0;
        for seed in 0..1500 {
            let mut rng = Xoshiro256StarStar::from_seed(seed + 1_000_000);
            moves += differential_playout(&Deal::from_seed(seed), &mut rng).unwrap_or_else(|e| panic!("{e}"));
        }
        assert!(moves > 40_000, "{moves}");
    }

    #[test]
    fn dead_end_tests_never_fire_on_winnable_positions() {
        let mut fired = 0;
        for seed in 0..1500 {
            match soundness_sample(seed, 200_000).unwrap_or_else(|e| panic!("{e}")) {
                Soundness::NotFired => {}
                Soundness::Confirmed | Soundness::Inconclusive => fired += 1,
            }
        }
        assert!(fired > 100, "tests should fire on a good share of small positions ({fired})");
    }
}
