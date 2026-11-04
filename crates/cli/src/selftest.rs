//! `acc selftest`: the Phase 2 gates at full size, on all cores (PLAN.md §6.3).

use accordion_core::Deal;
use accordion_core::rng::Xoshiro256StarStar;
use accordion_core::selfcheck::{Soundness, differential_playout, soundness_sample};
use rayon::prelude::*;
use std::time::Instant;

/// Seeds for the soundness samples start here, so they don't reuse the playout deals.
const SOUNDNESS_SEED_BASE: u64 = 1 << 40;
const REFERENCE_BUDGET: usize = 200_000;

pub fn run(playouts: u64, positions: u64) -> Result<bool, String> {
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
    Ok(true)
}
