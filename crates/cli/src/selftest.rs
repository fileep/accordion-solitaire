//! `acc selftest`: the Phase 2 gates at full size, on all cores, plus the Phase 5 pruning check (PLAN.md §6.3):
//! sleep sets and endgame tables must give the same verdict as plain search; and the exhaustive prover checked
//! against a plain recursive search.

use accordion_core::fast::Position;
use accordion_core::rng::Xoshiro256StarStar;
use accordion_core::selfcheck::{Soundness, differential_playout, soundness_sample};
use accordion_core::{Deal, Game, Move};
use accordion_solver::{Endgame, Gates, Proof, ProveOptions, Search, SearchOptions, SearchResult, prove};
use rayon::prelude::*;
use std::cell::RefCell;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

/// Seeds for the soundness samples start here, so they don't reuse the playout deals.
const SOUNDNESS_SEED_BASE: u64 = 1 << 40;
const REFERENCE_BUDGET: usize = 200_000;

pub fn run(
    playouts: u64,
    positions: u64,
    residuals: u64,
    proofs: u64,
    endgame_dir: &Path,
) -> Result<bool, String> {
    let threads = rayon::current_num_threads();
    let start = Instant::now();
    let moves = (0..playouts)
        .into_par_iter()
        .map(|seed| {
            let mut rng = Xoshiro256StarStar::from_seed(seed ^ 0xD1FF);
            differential_playout(&Deal::from_seed(seed), &mut rng).map(|m| m as u64)
        })
        .try_reduce(|| 0, |a, b| Ok(a + b));
    let moves = match moves {
        Ok(m) => m,
        Err(e) => {
            println!("FAILED differential playouts: {e}");
            return Ok(false);
        }
    };
    println!(
        "differential playouts: {playouts} games, {moves} moves, fast and reference engines agree on every one \
         ({:.1}s on {threads} threads)",
        start.elapsed().as_secs_f64()
    );

    let start = Instant::now();
    let counts = (0..positions)
        .into_par_iter()
        .map(|i| soundness_sample(SOUNDNESS_SEED_BASE + i, REFERENCE_BUDGET))
        .try_fold(
            || [0u64; 3],
            |mut c, r| {
                c[r? as usize] += 1;
                Ok::<_, String>(c)
            },
        )
        .try_reduce(|| [0u64; 3], |a, b| Ok([a[0] + b[0], a[1] + b[1], a[2] + b[2]]));
    let [not_fired, confirmed, inconclusive] = match counts {
        Ok(c) => c,
        Err(e) => {
            println!("FAILED dead-end soundness: {e}");
            return Ok(false);
        }
    };
    debug_assert_eq!(Soundness::NotFired as usize, 0);
    println!(
        "dead-end soundness: {positions} positions; a test fired on {} of them, every one confirmed unwinnable by \
         the reference search ({inconclusive} too large to search within {REFERENCE_BUDGET} positions); \
         {not_fired} had no test fire ({:.1}s)",
        confirmed + inconclusive,
        start.elapsed().as_secs_f64()
    );
    if residuals == 0 && proofs == 0 {
        return Ok(true);
    }
    let piles = crate::runner::available_endgame(endgame_dir);
    let tables = Arc::new(if piles > 0 {
        Endgame::load(endgame_dir, piles)?
    } else {
        Endgame::build(8, crate::runner::all_threads(), |_, _, _| {})
    });
    Ok(pruning(residuals, &tables)? & proof_check(proofs, &tables)?)
}

/// Seeds for the pruning check's residuals.
const PRUNING_SEED_BASE: u64 = 2 << 40;
const PRUNING_BUDGET: u64 = 5_000_000;

thread_local! {
    static SEARCHES: RefCell<Option<(Search, Search)>> = const { RefCell::new(None) };
}

/// Random residuals of 10–24 piles, each searched to the end twice: plain (no dead-end tests, no sleep sets, no
/// tables, fixed order) and with everything the solver uses (sleep sets, gated tests, endgame tables, random
/// order). The verdicts must agree, and every win must replay on the reference engine.
fn pruning(residuals: u64, tables: &Arc<Endgame>) -> Result<bool, String> {
    if residuals == 0 {
        return Ok(true);
    }
    let start = Instant::now();
    let reach = tables.max_piles();
    let counts = (0..residuals)
        .into_par_iter()
        .map(|i| {
            SEARCHES.with_borrow_mut(|slot| {
                let (plain, tuned) = slot.get_or_insert_with(|| {
                    let plain = Search::with_options(
                        14,
                        SearchOptions { gates: Gates::OFF, sleep_sets: false, ..SearchOptions::default() },
                    );
                    let mut tuned = Search::with_options(
                        14,
                        SearchOptions { endgame_piles: reach, ..SearchOptions::default() },
                    );
                    tuned.set_endgame(Some(tables.clone()));
                    (plain, tuned)
                });
                let seed = PRUNING_SEED_BASE + i;
                let mut rng = Xoshiro256StarStar::from_seed(seed);
                let mut pos = Position::from_deal(&Deal::from_seed(seed));
                let k = 28 + rng.below(15) as usize;
                for _ in 0..k {
                    match pos.random_move(&mut rng) {
                        Some(mv) => pos = pos.play(mv),
                        None => break,
                    }
                }
                plain.clear_table();
                tuned.clear_table();
                let a = plain.run(&pos, PRUNING_BUDGET);
                let b = tuned.run_with(&pos, PRUNING_BUDGET, Some(&mut rng), None);
                match (&a, &b) {
                    (SearchResult::OutOfBudget, _) | (_, SearchResult::OutOfBudget) => Ok([0, 0, 1]),
                    (SearchResult::Win(_), SearchResult::Win(line)) => {
                        let mut game = Game::from_piles(pos.tops().into_iter().map(|c| vec![c]).collect());
                        for &mv in line {
                            game.apply(mv).map_err(|e| format!("residual {seed}: move rejected: {e}"))?;
                        }
                        if game.is_won() {
                            Ok([1, 0, 0])
                        } else {
                            Err(format!("residual {seed}: line not won"))
                        }
                    }
                    (SearchResult::Dead, SearchResult::Dead) => Ok([0, 1, 0]),
                    _ => Err(format!("residual {seed} ({} piles): plain {a:?}, tuned {b:?}", pos.len())),
                }
            })
        })
        .try_reduce(|| [0u64; 3], |x, y| Ok([x[0] + y[0], x[1] + y[1], x[2] + y[2]]));
    match counts {
        Ok([wins, dead, inconclusive]) => {
            println!(
                "pruning: {residuals} residuals of 10-24 piles; sleep sets, gated tests and endgame tables (to {reach} piles) agree with plain search on every one: {wins} won (every line replayed), {dead} lost, {inconclusive} too large for {PRUNING_BUDGET} positions ({:.1}s)",
                start.elapsed().as_secs_f64()
            );
            Ok(true)
        }
        Err(e) => {
            println!("FAILED pruning check: {e}");
            Ok(false)
        }
    }
}

/// Seeds for the proof check's positions.
const PROOF_SEED_BASE: u64 = 3 << 40;
/// Distinct positions the reference search may store before a position counts as too large.
const PROOF_REFERENCE_BUDGET: usize = 1_000_000;
const PROOF_BUDGET: u64 = 50_000_000;

/// Whether a win is reachable from `pos`, by the plainest search there is: recursion and a standard hash set of
/// the positions found dead. `None` past `budget` of them.
fn reference(pos: &Position, dead: &mut HashSet<Vec<u8>>, budget: usize) -> Option<bool> {
    if pos.is_won() {
        return Some(true);
    }
    if dead.contains(pos.codes()) {
        return Some(false);
    }
    for mv in pos.legal_moves() {
        if reference(&pos.play(mv), dead, budget)? {
            return Some(true);
        }
    }
    dead.insert(pos.codes().to_vec());
    (dead.len() <= budget).then_some(false)
}

fn replay(pos: &Position, line: &[Move]) -> Result<(), String> {
    let mut game = Game::from_piles(pos.tops().into_iter().map(|c| vec![c]).collect());
    for &mv in line {
        game.apply(mv).map_err(|e| format!("move rejected: {e}"))?;
    }
    if game.is_won() { Ok(()) } else { Err("the line doesn't end in one pile".into()) }
}

/// Random positions of 14-44 piles, each proved four ways: by [`reference`], and by the prover's plain run, its
/// independent run (with the endgame tables), and an independent run with a set far too small for it, so that
/// forgetting positions is exercised (it must only cost time). The verdicts must agree, every win must replay on
/// the reference engine, and in a lost position the plain run must store exactly the positions the reference did.
fn proof_check(n: u64, tables: &Endgame) -> Result<bool, String> {
    if n == 0 {
        return Ok(true);
    }
    let start = Instant::now();
    let counts = (0..n)
        .into_par_iter()
        .map(|i| {
            let seed = PROOF_SEED_BASE + i;
            let mut rng = Xoshiro256StarStar::from_seed(seed);
            let mut pos = Position::from_deal(&Deal::from_seed(seed));
            for _ in 0..8 + rng.below(31) {
                match pos.random_move(&mut rng) {
                    Some(mv) => pos = pos.play(mv),
                    None => break,
                }
            }
            let mut reached = HashSet::new();
            let Some(winnable) = reference(&pos, &mut reached, PROOF_REFERENCE_BUDGET) else {
                return Ok([0, 0, 1, 0]);
            };
            let small = ProveOptions { max_bytes: 0, ..ProveOptions::independent(!seed, PROOF_BUDGET) };
            let runs = [
                ("plain", prove(&pos, &ProveOptions::plain(PROOF_BUDGET), None)),
                ("independent", prove(&pos, &ProveOptions::independent(seed, PROOF_BUDGET), Some(tables))),
                ("small-memory", prove(&pos, &small, None)),
            ];
            let fail = |name: &str, what: String| {
                format!("position {seed} ({} piles), {name} run: {what}", pos.len())
            };
            let mut evicted = 0;
            for (name, r) in &runs {
                match (&r.proof, winnable) {
                    (Proof::Won(line), true) => replay(&pos, line).map_err(|e| fail(name, e))?,
                    (Proof::Lost, false) => {}
                    (Proof::Unfinished, _) => return Ok([0, 0, 1, 0]),
                    (p, _) => {
                        return Err(fail(
                            name,
                            format!("{p:?}, but the reference found winnable = {winnable}"),
                        ));
                    }
                }
                evicted += r.evicted;
            }
            if !winnable && runs[0].1.dead != reached.len() {
                let what = format!("stored {} positions, the reference {}", runs[0].1.dead, reached.len());
                return Err(fail("plain", what));
            }
            Ok(if winnable { [1, 0, 0, evicted] } else { [0, 1, 0, evicted] })
        })
        .try_reduce(|| [0u64; 4], |x, y| Ok([x[0] + y[0], x[1] + y[1], x[2] + y[2], x[3] + y[3]]));
    match counts {
        Ok([wins, lost, inconclusive, evicted]) => {
            println!(
                "proofs: {n} positions of 14-44 piles; the prover's plain, independent and small-memory runs agree with a plain recursive search on every one: {wins} won (every line replayed), {lost} lost (the plain run storing exactly the positions the reference reached), {inconclusive} too large; the small-memory runs forgot {evicted} positions ({:.1}s)",
                start.elapsed().as_secs_f64()
            );
            Ok(true)
        }
        Err(e) => {
            println!("FAILED proof check: {e}");
            Ok(false)
        }
    }
}
