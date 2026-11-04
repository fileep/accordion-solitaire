//! Exhaustive search, for proving a loss (PLAN.md §5 stage 3; §6.3 Phase 7).
//!
//! The solver's dead-position table is lossy: an eviction only costs time, but a hash collision could hide a win, so
//! its "every line searched" is not a proof. Here dead positions go in an exact set keyed by the whole row of top
//! cards, and a search that finishes without finding a win proves the position lost. A loss is only claimed when two
//! independent runs agree (`acc prove`):
//! - **plain**: fixed move order, no dead-end tests, no sleep sets, no tables;
//! - **independent**: random move order, the sound dead-end tests (RESEARCH.md §5.8), sleep sets and the endgame
//!   tables, each with its own set.
//!
//! Exhaustive search of a whole random deal is far out of reach (around 10^17 positions); this is for positions
//! and deals that are small or nearly stuck, and for the golden unwinnable cases.

use crate::dfs::{Order, asleep_after, pick};
use crate::endgame::Endgame;
use accordion_core::Move;
use accordion_core::fast::{Position, connected};
use accordion_core::rng::Xoshiro256StarStar;
use std::collections::HashSet;

#[derive(Clone, Copy, Debug)]
pub struct ProveOptions {
    /// Random move order from this seed (`None`: lowest pile first, slide before leap).
    pub order_seed: Option<u64>,
    /// Run the connectivity and left-leaf tests at every position (both proven sound).
    pub dead_end_tests: bool,
    pub sleep_sets: bool,
    /// Give up after generating this many positions.
    pub max_positions: u64,
}

impl ProveOptions {
    /// The plain run: nothing but the rules.
    pub fn plain(max_positions: u64) -> ProveOptions {
        ProveOptions { order_seed: None, dead_end_tests: false, sleep_sets: false, max_positions }
    }

    /// The independent run: everything the solver uses, in a random order.
    pub fn independent(seed: u64, max_positions: u64) -> ProveOptions {
        ProveOptions { order_seed: Some(seed), dead_end_tests: true, sleep_sets: true, max_positions }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Proof {
    /// A winning line from the root.
    Won(Vec<Move>),
    /// Every line was searched and none wins: a proof.
    Lost,
    /// The position limit came first.
    Unfinished,
}

#[derive(Clone, Debug)]
pub struct ProofReport {
    pub proof: Proof,
    /// Positions generated.
    pub positions: u64,
    /// Distinct positions proven dead (the root included when lost). For the plain run of a lost root, this is every
    /// position reachable from it.
    pub dead: usize,
    /// Fewest piles reached, and a line from the root that reaches them.
    pub best_piles: usize,
    pub best_line: Vec<Move>,
}

struct Frame {
    pos: Position,
    slides: u64,
    leaps: u64,
    done_slides: u64,
    done_leaps: u64,
    via: Move,
}

fn dead_end(pos: &Position) -> bool {
    pos.len() > 1 && (!connected(pos.present()) || pos.left_leaf_pair())
}

/// Searches every line from `root` (or until a win or the limit), with an exact set of dead positions. With
/// `endgame`, positions the tables cover are answered from them (they are exact too).
pub fn prove(root: &Position, opts: &ProveOptions, endgame: Option<&Endgame>) -> ProofReport {
    let mut rng = opts.order_seed.map(Xoshiro256StarStar::from_seed);
    let mut dead: HashSet<Box<[u8]>> = HashSet::new();
    let mut report = ProofReport {
        proof: Proof::Lost,
        positions: 0,
        dead: 0,
        best_piles: root.len(),
        best_line: Vec::new(),
    };
    let reach = endgame.map_or(0, Endgame::max_piles);
    let line_to = |stack: &[Frame], last: Move| -> Vec<Move> {
        stack.iter().skip(1).map(|f| f.via).chain(std::iter::once(last)).collect()
    };
    if root.is_won() {
        report.proof = Proof::Won(Vec::new());
        return report;
    }
    if root.len() <= reach {
        let tables = endgame.expect("reach > 0 needs tables");
        report.proof = match tables.finish(root) {
            Some(line) if tables.winnable(root) == Some(true) => Proof::Won(line),
            _ => Proof::Lost,
        };
        report.dead = usize::from(report.proof == Proof::Lost);
        return report;
    }
    if opts.dead_end_tests && dead_end(root) {
        report.dead = 1;
        return report;
    }
    let mut stack = vec![Frame {
        pos: *root,
        slides: root.slides(),
        leaps: root.leaps(),
        done_slides: 0,
        done_leaps: 0,
        via: Move::slide(0),
    }];
    while let Some(top) = stack.last_mut() {
        if top.slides | top.leaps == 0 {
            let frame = stack.pop().expect("non-empty");
            dead.insert(frame.pos.codes().into());
            continue;
        }
        if report.positions >= opts.max_positions {
            report.proof = Proof::Unfinished;
            break;
        }
        let (pile, leap) = match rng.as_mut() {
            None => {
                let pile = (top.slides | top.leaps).trailing_zeros();
                (pile, top.slides >> pile & 1 == 0)
            }
            Some(r) => pick(&top.pos, top.slides, top.leaps, Order::Uniform, r),
        };
        let bit = 1u64 << pile;
        let sleep =
            if opts.sleep_sets { asleep_after(top.done_slides, top.done_leaps, pile, leap) } else { (0, 0) };
        if leap {
            top.leaps &= !bit;
            top.done_leaps |= bit;
        } else {
            top.slides &= !bit;
            top.done_slides |= bit;
        }
        let child = top.pos.play(Move { pile: pile as u8, leap });
        let via = Move { pile: pile as u8, leap };
        report.positions += 1;
        if child.len() < report.best_piles {
            report.best_piles = child.len();
            report.best_line = line_to(&stack, via);
        }
        if child.is_won() {
            report.proof = Proof::Won(line_to(&stack, via));
            break;
        }
        if child.len() <= reach {
            let tables = endgame.expect("reach > 0 needs tables");
            if tables.winnable(&child) == Some(true) {
                let mut line = line_to(&stack, via);
                line.extend(tables.finish(&child).expect("a winnable position has a line"));
                report.proof = Proof::Won(line);
                break;
            }
            dead.insert(child.codes().into());
            continue;
        }
        if dead.contains(child.codes()) {
            continue;
        }
        if opts.dead_end_tests && dead_end(&child) {
            dead.insert(child.codes().into());
            continue;
        }
        stack.push(Frame {
            slides: child.slides() & !sleep.0,
            leaps: child.leaps() & !sleep.1,
            done_slides: sleep.0,
            done_leaps: sleep.1,
            pos: child,
            via,
        });
    }
    report.dead = dead.len();
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use accordion_core::Deal;

    #[test]
    fn small_positions_prove_the_same_either_way() {
        let tables = Endgame::build(8, 4, |_, _, _| {});
        let mut rng = Xoshiro256StarStar::from_seed(5);
        let (mut won, mut lost) = (0, 0);
        for i in 0..400 {
            let mut pos = Position::from_deal(&Deal::from_seed(1000 + i));
            for _ in 0..32 + rng.below(8) {
                match pos.random_move(&mut rng) {
                    Some(mv) => pos = pos.play(mv),
                    None => break,
                }
            }
            let plain = prove(&pos, &ProveOptions::plain(u64::MAX), None);
            let independent = prove(&pos, &ProveOptions::independent(i, u64::MAX), Some(&tables));
            assert_eq!(plain.proof == Proof::Lost, independent.proof == Proof::Lost, "deal {}", 1000 + i);
            for p in [&plain.proof, &independent.proof] {
                if let Proof::Won(line) = p {
                    assert!(line.iter().fold(pos, |q, &m| q.play(m)).is_won());
                }
            }
            if plain.proof == Proof::Lost {
                lost += 1;
            } else {
                won += 1;
            }
        }
        assert!(won > 10 && lost > 10, "{won} {lost}");
    }
}
