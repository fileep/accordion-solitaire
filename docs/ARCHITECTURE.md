# Architecture

How the program is put together, for someone reading or changing the code. Using it is in
[GETTING_STARTED.md](GETTING_STARTED.md); the measurements behind each design choice are in [BENCHMARKS.md](BENCHMARKS.md),
and the reasoning in [RESEARCH.md](RESEARCH.md) and [PLAN.md](PLAN.md).

## The crates

```
accordion-core ──► accordion-solver ──► accordion-cli (acc)
      │                   │                  ▲
      │                   └──► accordion-wasm ──► viz (the web page)
      └──────────────────────► accordion-gpu ─┘  (and accgpu)
```

| Crate | What is in it |
|---|---|
| `crates/core` (`accordion-core`) | Cards and decks (`card.rs`, `deck_input.rs`), the deal of a seed (`rng.rs`, `deal.rs`), two engines for the rules, the move notation, solve files and their verification (`verify.rs`). The **reference engine** (`game.rs`) is deliberately plain: a vector of piles, every rule spelled out. The **fast engine** (`fast.rs`) keeps a position as packed top cards and bit masks of legal moves, updated move by move, and has the dead-end tests; `quick.rs` is a third, allocation-free replay of the rules for checking lines. |
| `crates/solver` (`accordion-solver`) | The search (`dfs.rs`: depth-first, with a table of lost positions, sleep sets and the dead-end tests), the strategy around it (`solver.rs`: the ladder and the thorough search), the endgame tables (`endgame.rs`), and the exhaustive prover (`prove.rs`). |
| `crates/gpu` (`accordion-gpu`) | The CUDA kernel (`kernel.cu`) that makes ladder attempts on an NVIDIA GPU, and its host side (`lib.rs`): finding the driver and NVRTC, compiling the kernel when a run starts, preparing batches of deals and reading back their results. `accgpu` (`main.rs`) runs the GPU alone, for measuring. |
| `crates/cli` (`accordion-cli`) | `acc`. `batch.rs` is a run of deals with its checkpoint, report, status file and log of events; `runner.rs` the worker threads and the GPU's feeder, `games.rs` the check for repeated games, `prove.rs` and `dossier.rs` proofs and dossiers, `pause.rs` Ctrl+C and the pause file, `progress.rs` the progress line and speed log. `front/` is the front end (`acc run`, `resume`, `status`, `pause`, `results`, `doctor`, `show`, `settings`), which only ever starts `acc batch` as a process of its own and reads the files it writes. |
| `crates/wasm` | The core and the solver for the browser, through `wasm-bindgen`, for the visualizer. |
| `viz/` | The visualizer, in TypeScript with Vite ([VISUALIZER.md](VISUALIZER.md)). |

## The life of a deal in a run

1. **The deal.** Seed `s` becomes 52 cards by SplitMix64, xoshiro256\*\* and a Fisher-Yates shuffle
   ([FORMATS.md](FORMATS.md#deals-from-seeds)), so anyone can make the same deal from the same seed. A build with the
   `second-generator` feature (`crates/core/src/rng.rs`) uses ChaCha20 instead, on the CPU and the GPU alike: other deals
   for the same seeds, to check that the win rate doesn't depend on the generator.
2. **The GPU, if there is one.** A feeder thread takes batches of 262,144 deals and launches the kernel on them. For each
   deal the kernel makes up to a few hundred **attempts**: play a prefix of 38 to 40 moves chosen at random (weighted
   towards moves three to the left, and scored by patterns that winning prefixes share), then search what is left, at most
   16 piles, exhaustively within 700 positions. Attempts go in pairs that share their first 20 moves; a leftover with too
   many different ranks isn't searched. The first winning line of each deal comes back.
3. **Checking the GPU's lines.** The processor's worker threads replay each line before the deal counts as won, with
   `quick::line_wins`: a replay of the rules on the GPU's own cards that says yes only to a legal win of a full deck. A line
   it doesn't pass goes to the reference rules. 1 deal in 256 is replayed by the reference engine from the seed's deal
   instead, which also checks that the GPU had the right deal.
4. **The processor's solver**, for every deal the GPU missed (or every deal, without a GPU), from the start, on one thread:
   - **The ladder:** up to 256 attempts of 36 random moves and a search of the rest within 4,000 positions; then 34 moves
     and 8,000 positions, and so on down to 24 moves. Most deals are won on the first rung.
   - **The thorough search**, if the ladder fails: a randomized depth-first search from the start, restarted with a fresh
     random order after budgets of 1,000 positions times the Luby sequence (1 1 2 1 1 2 4 ...), until the deal's limit
     (`--max-nodes`, 50 million positions).
   - Throughout, a table of positions known to be lost (1 MB per thread, kept across attempts; it may forget, which can
     only cost time), the connectivity test for dead ends at 19 piles or fewer, sleep sets (moves that don't affect each
     other are tried in one order only), and the endgame tables up to 11 piles.

   The deal's seed seeds the solver's random choices, so the same deal is solved the same way every time.
5. **Checking the processor's lines:** every winning line is replayed by the reference rules.
6. **In seed order.** Results are handed on in seed order, so a run's progress is one number: every deal before it is done.
   That is what the checkpoint saves.
7. **A deal not won** (every line searched, or the limit reached) gets a dossier, and goes to the **prover**, on a thread of
   its own while the run goes on. The prover makes two exhaustive searches, each keeping an exact set of the positions
   proven lost: a *plain* one, with the rules alone in a fixed order, and an *independent* one, with the solver's dead-end
   tests, sleep sets, endgame tables and positions stored by their pattern of suits and ranks, in a random order. A loss
   counts only if both go through every reachable position without a win, and then the deal gets a loss record. If either
   finds a win, its line is checked like any other. If either reaches its limit (10 billion positions), the deal is undecided.
8. **At the end**, the check for repeated games (below), the last proofs, and the report.

## How results are checked

Nothing the search finds counts until something separate from it has checked it:

- **Wins** are replayed under the rules by the reference engine, or by `quick::line_wins` (step 3). Both are separate from
  the fast engine and the kernel that found the line. `acc verify` uses the reference engine.
- **Losses** need the two exhaustive searches to agree (step 7). Both use the solver's move generator, so they guard against
  the solver's shortcuts but not against that; the independent Python search in `testdata/tools/check_loss.py` and the Lean
  proofs in [research/lean](../research/lean/README.md) share no code with it.
- **The engines** are checked against an independent implementation of the rules and formats in Python
  (`testdata/tools/make_golden.py`), which writes the golden files in `testdata/` that the Rust and TypeScript tests must
  reproduce: seeded deals, deck input, legal and illegal lines, and unwinnable positions with their exact counts of
  reachable positions. The fast engine is checked against the reference engine over random games, and the kernel against
  both, move by move (`crates/gpu/tests`).
- **The shortcuts** (dead-end tests, sleep sets, endgame tables, the prover's optimisations) are checked against plain
  search on random positions by `acc selftest` and the solver's tests: a shortcut may make the search faster, never change
  an answer.

## Checkpoints

A run's checkpoint (`batch.rs`, `Checkpoint`) holds its arguments and full settings, where it stands, its tallies, the
proofs in hand, and how much of each file it writes as it goes counts. Every minute the run first makes those files safe
on disk (the `--out` file, the sample's log, the positions and times of the deals solved on the processor), then writes the
checkpoint to a new file and renames it into place, so a checkpoint is always whole and never counts anything a crash could
lose. Carrying on cuts each file back to the length the checkpoint counts, and the deals after that are done again: their
results are the same, as everything is seeded.

Beside it, a `.lock` file held for as long as the run goes keeps a second process from taking up the same run; the
system lets go of it however the process ends. The `.status.json` and `.events.log` files are for reading from outside
([FORMATS.md](FORMATS.md#the-status-file)); the front end reads only those, never the checkpoint of a run that is going.

A checkpoint must carry on with a newer build: its `VERSION` (3) changes only when an old one can't be read correctly, a
field of the saved arguments is never renamed or removed, and a new field gets `#[serde(default)]`.
`crates/cli/tests/data/v3-checkpoint.json` is a real checkpoint from before the last such change, and a test resumes it.

## The check for repeated games

Two deals that differ only in the names of the suits or ranks play alike, so each deal is first renamed (suits and ranks in
order of first appearance) and fingerprinted (64 bits). With room for 8 bytes a deal in `--games-memory`, every fingerprint
is kept and sorted at the end, and equal fingerprints are compared card by card. With room for 5 bytes a deal on disk
(`--games-disk`), the same is done on disk in two more passes. Otherwise a hash of the renamed deal's first cards picks a
sample as the run goes: each picked deal costs 11 bytes (a 5-byte key and a 6-byte index, so up to 2^48 deals) in an
append-only log, read back in parts at the end. Any two deals that are the same game are picked together, so a sample
still sees a fair share of repeats, and the report says how many it would have seen. The count of chance matches of 40
bits, against the formula, tests the generator.

## The GPU

`kernel.cu` is compiled when a run starts, by NVRTC, for the compute capability of the card found, with the GPU settings as
defines ([COMMANDS.md](COMMANDS.md#the-gpus-settings)). A block holds one warp that plays prefixes and one that searches,
passing leftovers of at most 16 piles through shared memory, with eight blocks on each multiprocessor. Searches are
first-win depth-first searches of the leftover with the connectivity test and a budget, lane by lane. The design, and what
was tried and why it was or wasn't kept, is the second half of [BENCHMARKS.md](BENCHMARKS.md), from "Phase 8" on.

The feeder thread keeps launches in flight and hands results to the workers, which check the lines before they take new
deals. If the GPU fails, or no deal finishes for 15 minutes, a run with a checkpoint pauses with exit code 75 rather than go
on at the processor's speed.

## Tests

```
cargo test --workspace --release
```

The tests run optimised: unoptimised, the solver is many times slower, and some tests run real batches. Without an NVIDIA
card and NVRTC the kernel's tests pass without running anything. What they cover:

- `crates/core`: the golden files (`tests/golden.rs`), the engines against each other, notation and solve files.
- `crates/solver`: unit tests of the search, the endgame tables and the prover, and `tests/gates.rs`: the golden positions
  get the right verdicts, the unwinnable golden deals are never won and are proved lost, and the first hundred deals are
  won and verify.
- `crates/gpu/tests`: the kernel against the Rust engines (needs a GPU).
- `crates/cli`: unit tests in the modules; `tests/pause_resume.rs` (pausing, crashing and carrying on, the lock, Ctrl+C),
  `tests/batch_limits.rs` (argument limits, the capped costs, old checkpoints, settings files), `tests/front.rs` (the front
  end, with a pretend battery), and a test that this documentation names every command and flag
  (`src/docs_check.rs`). Some are timing-sensitive and can fail on a heavily loaded machine: run them again alone before
  calling them failures.
- `viz/tests`: the visualizer's engine against the golden files (`npm test`).

The golden files are made by `python testdata/tools/make_golden.py`; change them only with the Python, never by hand.

To add a speed measurement, follow [BENCHMARKS.md](BENCHMARKS.md): say the machine, the commit and the date, time the change
against the code before it in alternated runs, and record what was tried and not kept, and why.
