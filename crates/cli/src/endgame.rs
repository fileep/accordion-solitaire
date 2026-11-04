//! `acc endgame build`: the complete endgame tables (PLAN.md Phase 5 track E), one bit per suit/rank pattern for
//! every pile count up to `--piles`, each built from the one below and written to `<dir>/endgame-<k>.bin`.

use crate::runner;
use accordion_solver::Endgame;
use accordion_solver::endgame::patterns;
use clap::{Args, Subcommand};
use std::path::PathBuf;
use std::time::Instant;

#[derive(Args)]
pub struct EndgameArgs {
    #[command(subcommand)]
    command: EndgameCommand,
}

#[derive(Subcommand)]
enum EndgameCommand {
    /// Build the tables for 1 to --piles piles and save them.
    Build {
        /// Largest pile count (10 is 68 MB and about a minute; 11 is 1.15 GB).
        #[arg(long, default_value_t = 10)]
        piles: usize,
        #[arg(long, default_value = "endgame")]
        dir: PathBuf,
        /// Threads (0 = every core).
        #[arg(long, default_value_t = 0)]
        threads: usize,
    },
}

pub fn run(args: &EndgameArgs) -> Result<(), String> {
    match &args.command {
        EndgameCommand::Build { piles, dir, threads } => {
            if !(1..=accordion_solver::endgame::MAX_PILES).contains(piles) {
                return Err(format!("--piles must be 1 to {}", accordion_solver::endgame::MAX_PILES));
            }
            let threads = if *threads == 0 { runner::all_threads() } else { *threads };
            let size: u64 = (1..=*piles).map(|k| patterns(k).div_ceil(8)).sum();
            eprintln!(
                "building endgame tables for 1–{piles} piles ({:.1} MB) on {threads} threads",
                size as f64 / 1e6
            );
            let start = Instant::now();
            let mut last = Instant::now();
            let tables = Endgame::build(*piles, threads, |k, n, wins| {
                println!(
                    "{k:>3} piles  {n:>16} patterns  {wins:>15} winnable ({:>5.2}%)  {:>8.1} s",
                    100.0 * wins as f64 / n as f64,
                    last.elapsed().as_secs_f64()
                );
                last = Instant::now();
            });
            let t = Instant::now();
            tables.save(dir).map_err(|e| format!("writing {}: {e}", dir.display()))?;
            eprintln!(
                "built in {:.1} s, saved to {} in {:.1} s",
                start.elapsed().as_secs_f64() - t.elapsed().as_secs_f64(),
                dir.display(),
                t.elapsed().as_secs_f64()
            );
            Ok(())
        }
    }
}
