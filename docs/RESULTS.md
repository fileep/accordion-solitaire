# Results

The big runs of PLAN.md Phase 7: how often open Accordion is won, and what it costs. The rules are the 52-card open
deal; a pile moves onto the pile 1 or 3 to its left when the top cards share a suit or rank; **a win is one pile left**.

## Headline

**30,000,000 random deals (seeds 0–29,999,999): every one won, and every win replayed by the reference engine.**
The 95% Wilson interval for the win rate is **[99.999987%, 100%]**.

## Runs

All runs used `acc batch` with the default strategy (below) on the laptop in docs/BENCHMARKS.md: i7-1260P, 16 threads, at
its sustained heat-soaked clocks. Deals come from seeds via the specified generator (docs/FORMATS.md), so anyone can
re-deal and re-check them.

| Seeds | Deals | Won | 95% CI (Wilson) | Deals/s | Wall time |
|---|---|---|---|---|---|
| 0–999,999 | 1,000,000 | 1,000,000 | [99.999616%, 100%] | 19,512 | 51 s |
| 0–9,999,999 | 10,000,000 | 10,000,000 | [99.999962%, 100%] | 15,348 | 10 min 52 s |
| 10,000,000–29,999,999 | 20,000,000 | 20,000,000 | [99.999981%, 100%] | 13,422 | 24 min 50 s |
| **0–29,999,999 (both runs)** | **30,000,000** | **30,000,000** | **[99.999987%, 100%]** | | |

No deal ended `unknown`, so there were no dossiers and nothing for the §5 escalation to do. Not-won deals would have been
counted as losses for the interval's lower bound and as wins for its upper bound.

**Cost per deal** (10M run; positions are solver nodes, times are one core of 16 busy ones):

| | Median | p90 | p99 | p99.9 | Max |
|---|---|---|---|---|---|
| Positions | 2,755 | 13,797 | 47,133 | 116,609 | 1,620,265 |
| Time | 0.45 ms | 2.2 ms | 8.3 ms | 22 ms | 272 ms |

**Where the wins came from** (10M run): the strategy is a ladder of random-prefix attempts in front of a thorough search.

| Stage | Deals | Share |
|---|---|---|
| Ladder rung 0 (36 random moves, then a 4,000-position search; up to 256 tries) | 9,457,422 | 94.57% |
| Rung 1 (34 moves, 8,000 positions) | 504,824 | 5.05% |
| Rung 2 (32 moves, 16,000 positions) | 34,628 | 0.35% |
| Rungs 3–6 | 3,122 | 0.031% |
| Thorough search from the start (Luby restarts) | 4 | 0.00004% |

## Compared with earlier work

| | Deals | Won | Reported interval | Speed |
|---|---|---|---|---|
| Masten (1999 onwards, Solitaire Win Rates) | 30,000,000 | 30,000,000 | his estimate 99.9999936 ± 0.0000064% | about 12 deals/s with his newer program; his hardest 1999 deal took about 2 hours |
| Blake & Gent, Solvitaire (JAIR) | — | — | 99.99948 ± 0.00052% (95%) | general-purpose solver |
| **This solver** | **30,000,000** | **30,000,000** | **Wilson 95% [99.999987%, 100%]** | **13,400–19,500 deals/s on a laptop**; hardest deal 0.86 s |

Masten's deals come from his own generator, so the two sets of 30M deals are different deals; the agreement is in the
rate, not the deals. No random Accordion deal has yet been found unwinnable, by anyone.

## Hardest deals

The 20 deals of the 10M run that took the most positions are in [hard-deals/INDEX.md](../hard-deals/INDEX.md).
(The later 20M run had one harder deal still, at 2.46M positions and 863 ms; it wasn't added to the gallery.) Each is
a verified solve file in `hard-deals/gallery/` that the visualizer replays move by move. The hardest, seed 1,670,239, took
1.62M positions and was won on ladder rung 5. Of the 20, two were won on rung 5 and the rest on rungs 3 and 4.

"Hard" here means hard for this solver's random choices. With all 16 threads on one deal (`acc solve`), each of these is
won in a few hundredths of a second.

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

## Reproducing

```bash
cargo run --release -p accordion-cli -- endgame build --piles 11
```

```bash
cargo run --release -p accordion-cli -- batch --from 0 --count 10000000 --gallery 20
```

```bash
cargo run --release -p accordion-cli -- batch --from 10000000 --count 20000000
```

A single deal's line can be printed and checked with `acc solve --seed N` and `acc verify`.
