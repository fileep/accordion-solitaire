# Results

The big runs of PLAN.md Phases 7 and 8: how often open Accordion is won, and what it costs. The rules are the 52-card open
deal; a pile moves onto the pile 1 or 3 to its left when the top cards share a suit or rank; **a win is one pile left**.

## Headline

**100,000,000 random deals (seeds 0–99,999,999): every one won, every win replayed by the reference engine, and no two
deals the same game.** The 95% Wilson interval for the win rate is **[99.999996%, 100%]**.

## Runs

All runs used `acc batch` with the default strategy (below) on the laptop in docs/BENCHMARKS.md: i7-1260P with 16 threads,
at its sustained heat-soaked clocks, and for the 100M run its NVIDIA T550 GPU as well. Deals come from seeds via the
specified generator (docs/FORMATS.md), so anyone can re-deal and re-check them.

| Seeds | Deals | Won | 95% CI (Wilson) | Solved on | Deals/s | Wall time |
|---|---|---|---|---|---|---|
| 0–999,999 | 1,000,000 | 1,000,000 | [99.999616%, 100%] | CPU | 19,512 | 51 s |
| 0–9,999,999 | 10,000,000 | 10,000,000 | [99.999962%, 100%] | CPU | 15,348 | 10 min 52 s |
| 10,000,000–29,999,999 | 20,000,000 | 20,000,000 | [99.999981%, 100%] | CPU | 13,422 | 24 min 50 s |
| **0–99,999,999** | **100,000,000** | **100,000,000** | **[99.999996%, 100%]** | **CPU + GPU** | **19,281** | **1 h 26 min 27 s** |

The first three are Phase 7; the 100M run (Phase 8) covers their 30M deals again and 70M new ones. No deal ended
`unknown`, so there were no dossiers and nothing for the §5 escalation to do. Not-won deals would have been counted as
losses for the interval's lower bound and as wins for its upper bound.

**No repeated games.** The 100M run also checked that its deals are different games, not only different orders of the
cards. Two deals that differ only in the names of the suits or ranks play exactly alike, so each deal is renamed first
(suits and ranks in order of first appearance). The renamed decks are then compared: 64-bit fingerprints are sorted, and
any two with the same fingerprint are compared card by card. All 100,000,000 differ.

**The GPU's share** (100M run): the GPU was given 82,273,155 deals and won 81,823,614 of them (99.45%), with 13.5 billion
ladder attempts at 214M positions/s. The reference engine replayed every one of its lines, and none failed. The CPU solved
the other 18,176,386:

- the 449,541 deals the GPU tried and missed, solved again from the start;
- 17,726,845 deals the CPU workers took for themselves while the GPU was busy.

**Sustained speed.** After its first two minutes, the run held 18,900–19,700 deals/s over each stretch of 10M deals. The
CPU alone held 13,422 deals/s over the 20M run, so over long runs the GPU adds about 1.4×. That is less than the 2.15×
median of the short comparison runs in docs/BENCHMARKS.md (Phase 8). The CPU slowed far more than the GPU over the long
run:

- the CPU's median time per deal was 1.15 ms, against 0.47–0.61 ms in the short runs with the GPU;
- the GPU's rate fell only from 228M to 214M positions/s;
- the GPU's share of the deals was 82%, against 58–65% in the short runs.

On this laptop the CPU and the GPU share one cooling system.

**Cost per deal** (positions are solver nodes, times are one core of 16 busy ones). The 10M run covers every deal. The
100M run covers only the deals solved on the CPU: its tail is heavier because the GPU's misses are among them, and its
times are longer because its CPU ran hotter.

| | Median | p90 | p99 | p99.9 | Max |
|---|---|---|---|---|---|
| Positions, 10M run | 2,755 | 13,797 | 47,133 | 116,609 | 1,620,265 |
| Time, 10M run | 0.45 ms | 2.2 ms | 8.3 ms | 22 ms | 272 ms |
| Positions, 100M run (18.2M deals on the CPU) | 2,858 | 15,236 | 59,406 | 170,936 | 4,683,481 |
| Time, 100M run (18.2M deals on the CPU) | 1.15 ms | 6.8 ms | 31 ms | 93 ms | 1.94 s |

**Where the wins came from:** the strategy is a ladder of random-prefix attempts in front of a thorough search. A GPU, when
there is one, runs a shorter ladder of its own first.

| Stage | 10M run (CPU) | | 100M run (CPU + GPU) | |
|---|---|---|---|---|
| GPU ladder (38, then 36 random moves, then a 2,000-position search; up to 1,280 tries) | | | 81,823,614 | 81.82% |
| Ladder rung 0 (36 random moves, then a 4,000-position search; up to 256 tries) | 9,457,422 | 94.57% | 16,918,509 | 16.92% |
| Rung 1 (34 moves, 8,000 positions) | 504,824 | 5.05% | 1,090,798 | 1.09% |
| Rung 2 (32 moves, 16,000 positions) | 34,628 | 0.35% | 143,009 | 0.14% |
| Rungs 3–6 | 3,122 | 0.031% | 24,018 | 0.024% |
| Thorough search from the start (Luby restarts) | 4 | 0.00004% | 52 | 0.00005% |

The GPU takes deals the CPU would mostly have won on rungs 0–2. The deals that defeat the whole ladder are about as rare
with it as without: 52 in 100M against 4 in 10M.

## Compared with earlier work

| | Deals | Won | Reported interval | Speed |
|---|---|---|---|---|
| Masten (1999 onwards, Solitaire Win Rates) | 30,000,000 | 30,000,000 | his estimate 99.9999936 ± 0.0000064% | about 12 deals/s with his newer program; his hardest 1999 deal took about 2 hours |
| Blake & Gent, Solvitaire (JAIR) | — | — | 99.99948 ± 0.00052% (95%) | general-purpose solver |
| **This solver** | **100,000,000** | **100,000,000** | **Wilson 95% [99.999996%, 100%]** | **19,300 deals/s on a laptop with its GPU, 13,400 without**; hardest deal under 2 s on one core |

Masten's deals come from his own generator, so his 30M deals and these 100M are different deals; the agreement is in the
rate, not the deals. No random Accordion deal has yet been found unwinnable, by anyone.

## Hardest deals

The 20 deals of the 100M run that took the most positions are in [hard-deals/INDEX.md](../hard-deals/INDEX.md). Each is a
verified solve file in `hard-deals/gallery/` that the visualizer replays move by move. Deals the GPU won aren't ranked:
the GPU counts positions differently, and gives up on a deal after 1,280 attempts.

- The hardest, seed 87,477,620, took 4.68M positions: the whole ladder failed on it, and the thorough search won it.
- Of the 20, two were won by the thorough search, one on rung 6, nine on rung 5 and eight on rung 4.
- Ninth is seed 23,861,335, the hardest deal of the 20M run. It took exactly 2,461,394 positions in both runs, since the
  CPU solver's random choices are seeded by the deal.

"Hard" here means hard for this solver's random choices. With all 16 threads on one deal (`acc solve`), the three hardest
are each won in 0.04–0.06 s.

## Losses

The solver never claims a loss from its lossy table. `acc prove` proves a loss with two independent exhaustive searches,
each with an exact set of dead positions. It proves all three constructed unwinnable golden cases lost: the deal with no
legal moves, the frozen 44-pile dead end, and a 16-pile dead end reached from seed 42. The plain run's dead sets match the
independent Python reference's reachable-position counts exactly (1, 353 and 71). No random deal needed it.

## How every win is checked

- The solver searches with the fast engine, but nothing counts until the deliberately simple **reference engine** replays
  the whole line. That covers legality of every move, the card set, and one pile after exactly 51 moves (PLAN.md §6.1).
- The reference engine's rules agree with an independent Python implementation on every golden file. The fast engine
  agrees with the reference engine over 10M lockstep random playouts.
- The GPU's lines are replayed the same way: 81.8M of them in the 100M run, none failing. The kernel's rules agree with
  the Rust engines move by move on random playouts from the golden files and 20,000 seeded deals (`crates/gpu/tests`).
- The pruning the solver relies on agreed with plain search on 1,000,000 random residuals:
  - sound dead-end tests;
  - sleep sets;
  - endgame tables, whose winnable counts also match an independent Python enumeration.
- Lossy caching can only miss wins, never invent one, and every win is replayed anyway.

## The strategy

The defaults since Phase 5 (docs/BENCHMARKS.md has the measurements):

- **Ladder:** up to 256 attempts of 36 uniform random moves, then a first-win depth-first search of the remaining 16 piles
  with a 4,000-position budget. Then 34 moves and 8,000 positions, and so on down to 24 moves.
- **Thorough search** if the ladder fails: randomized first-win DFS from the start with Luby restarts of 1,000 positions.
- **Throughout:**
  - a dead-position table kept across attempts;
  - the connectivity test at ≤19 piles, on trigger;
  - sleep sets (commuting moves are tried in one order only);
  - complete endgame tables keyed by suit/rank pattern for ≤11 piles.
- **GPU**, when there is one (since Phase 8): before any of the above, up to 256 attempts of 38 random moves and then up
  to 1,024 of 36, each followed by a 2,000-position search. A deal it misses goes to the CPU, which solves it from the
  start.

## Reproducing

```bash
cargo run --release -p accordion-cli -- endgame build --piles 11
```

```bash
cargo run --release -p accordion-cli -- batch --from 0 --count 100000000 --gallery 20
```

The Phase 7 runs were on the CPU alone; to repeat one, add `--gpu off`:

```bash
cargo run --release -p accordion-cli -- batch --from 10000000 --count 20000000 --gpu off
```

A single deal's line can be printed and checked with `acc solve --seed N` and `acc verify`.
