# Results

The big runs of PLAN.md Phases 7 and 8 and after: how often open Accordion is won, and what it costs. The rules are the
52-card open deal; a pile moves onto the pile 1 or 3 to its left when the top cards share a suit or rank; **a win is one
pile left**.

## Headline

**1,000,000,000 random deals (seeds 0–999,999,999): 999,999,998 won, every win replayed by the reference engine, and the
other 2 proven unwinnable. No two deals are the same game.** The 95% Wilson interval for the win rate is
**[99.99999927%, 99.99999995%]**: about 1 deal in 500 million can't be won (95% interval: 1 in 137 million to 1 in 1.8
billion). They are the first random deals shown unwinnable that docs/RESEARCH.md knows of (see Losses).

## Runs

All runs used `acc batch` with the default strategy (below) on the laptop in docs/BENCHMARKS.md: i7-1260P with 16 threads,
at its sustained heat-soaked clocks, and for the last two runs its NVIDIA T550 GPU as well. Deals come from seeds via the
specified generator (docs/FORMATS.md), so anyone can re-deal and re-check them.

| Seeds | Deals | Won | 95% CI (Wilson) | Solved on | Deals/s | Wall time |
|---|---|---|---|---|---|---|
| 0–999,999 | 1,000,000 | 1,000,000 | [99.999616%, 100%] | CPU | 19,512 | 51 s |
| 0–9,999,999 | 10,000,000 | 10,000,000 | [99.999962%, 100%] | CPU | 15,348 | 10 min 52 s |
| 10,000,000–29,999,999 | 20,000,000 | 20,000,000 | [99.999981%, 100%] | CPU | 13,422 | 24 min 50 s |
| 0–99,999,999 | 100,000,000 | 100,000,000 | [99.999996%, 100%] | CPU + GPU | 19,281 | 1 h 26 min 27 s |
| **0–999,999,999** | **1,000,000,000** | **999,999,998** | **[99.99999927%, 99.99999995%]** | **CPU + GPU** | **43,855** | **6 h 20 min 3 s** |

The first three are Phase 7 and the 100M run is Phase 8. They won every deal: none ended `unknown`, so they left no
dossiers. The billion run came after Phase 8, with the faster code of docs/BENCHMARKS.md's "After Phase 8" sections
(commit a761123; its speed log says f91bd67+dirty, as the run started before that commit was made). It covers all of their
deals again and 900M new ones. Two of its deals were not won, one at the 50M-position limit and one with every line
searched; both went through the §5 escalation and were proven lost (Losses, below), so its interval counts them as
losses. A deal that is not won but not proven lost counts as a loss for the interval's lower bound and as a win for its
upper bound.

**No repeated games.** The 100M and billion runs also checked that their deals are different games, not only different
orders of the cards. Two deals that differ only in the names of the suits or ranks play exactly alike, so each deal is
renamed first (suits and ranks in order of first appearance). The renamed decks are then compared: 64-bit fingerprints
are sorted, and any two with the same fingerprint are compared card by card. All 100,000,000, and then all
1,000,000,000, differ.

**The GPU's share.** The reference engine replayed every GPU line, and none failed. The CPU solved the rest: the deals
the GPU tried and missed, solved again from the start, and the deals the CPU workers took for themselves while the GPU
was busy.

| | 100M run | Billion run |
|---|---|---|
| Deals given to the GPU | 82,273,155 | 790,102,016 |
| Won on the GPU | 81,823,614 (99.45%) | 786,211,377 (99.51%) |
| GPU ladder attempts | 13.5 billion | 127.5 billion |
| GPU speed | 214M positions/s | 331M positions/s |
| GPU misses, solved on the CPU | 449,541 | 3,890,639 |
| Deals the CPU workers took for themselves | 17,726,845 | 209,897,984 |

In the billion run the CPU workers spent 79.6% of their time on their own deals, 10.9% on the GPU's misses and 7.8%
verifying GPU lines (34.1 µs a line). Handing results on took 1.1% and the rest 0.6%; they never waited for the GPU.

**Sustained speed.** After its first two minutes, the run held 18,900–19,700 deals/s over each stretch of 10M deals. The
CPU alone held 13,422 deals/s over the 20M run, so over long runs the GPU adds about 1.4×. That is less than the 2.15×
median of the short comparison runs in docs/BENCHMARKS.md (Phase 8). The CPU slowed far more than the GPU over the long
run:

- the CPU's median time per deal was 1.15 ms, against 0.47–0.61 ms in the short runs with the GPU;
- the GPU's rate fell only from 228M to 214M positions/s;
- the GPU's share of the deals was 82%, against 58–65% in the short runs.

On this laptop the CPU and the GPU share one cooling system.

The billion run held **42,169–45,026 deals/s in each of its six full hours** (43,737 over the last 20 minutes), and the
GPU 330–332M positions/s throughout. Its speed log (`acc batch` writes a row a second to `logs/`) has one second out of
22,802 below 20,000 deals/s after the first half minute. That is 2.3× the 100M run, from the code since Phase 8: better
GPU occupancy and register stacks, the leftmost-card dead-end test, and leaner verification (docs/BENCHMARKS.md, "After
Phase 8").

**Cost per deal** (positions are solver nodes, times are one core of 16 busy ones). The 10M run covers every deal. The
100M and billion runs cover only the deals solved on the CPU: the 100M run's tail is heavier because the GPU's misses are
among them, and its times are longer because its CPU ran hotter. The billion run's maximums are seed 112,249,515, stopped
at the limit (Losses); its hardest win wasn't recorded.

| | Median | p90 | p99 | p99.9 | Max |
|---|---|---|---|---|---|
| Positions, 10M run | 2,755 | 13,797 | 47,133 | 116,609 | 1,620,265 |
| Time, 10M run | 0.45 ms | 2.2 ms | 8.3 ms | 22 ms | 272 ms |
| Positions, 100M run (18.2M deals on the CPU) | 2,858 | 15,236 | 59,406 | 170,936 | 4,683,481 |
| Time, 100M run (18.2M deals on the CPU) | 1.15 ms | 6.8 ms | 31 ms | 93 ms | 1.94 s |
| Positions, billion run (213.8M deals on the CPU) | 2,359 | 12,363 | 43,523 | 116,937 | 50,000,001 |
| Time, billion run (213.8M deals on the CPU) | 0.58 ms | 3.1 ms | 13 ms | 41 ms | 19.0 s |

**Where the wins came from:** the strategy is a ladder of random-prefix attempts in front of a thorough search. A GPU, when
there is one, runs a shorter ladder of its own first.

| Stage | 10M run (CPU) | | 100M run (CPU + GPU) | | Billion run (CPU + GPU) | |
|---|---|---|---|---|---|---|
| GPU ladder (38, then 36 random moves, then a 2,000-position search; up to 1,280 tries) | | | 81,823,614 | 81.82% | 786,211,377 | 78.62% |
| Ladder rung 0 (36 random moves, then a 4,000-position search; up to 256 tries) | 9,457,422 | 94.57% | 16,918,509 | 16.92% | 199,786,473 | 19.98% |
| Rung 1 (34 moves, 8,000 positions) | 504,824 | 5.05% | 1,090,798 | 1.09% | 12,285,005 | 1.23% |
| Rung 2 (32 moves, 16,000 positions) | 34,628 | 0.35% | 143,009 | 0.14% | 1,475,522 | 0.15% |
| Rungs 3–6 | 3,122 | 0.031% | 24,018 | 0.024% | 241,091 | 0.024% |
| Thorough search from the start (Luby restarts) | 4 | 0.00004% | 52 | 0.00005% | 530 | 0.00005% |

The GPU takes deals the CPU would mostly have won on rungs 0–2. The deals that defeat the whole ladder are about as rare
with it as without: 4 in 10M, 52 in 100M and 530 in the billion.

## Compared with earlier work

| | Deals | Won | Reported interval | Speed |
|---|---|---|---|---|
| Masten (1999 onwards, Solitaire Win Rates) | 30,000,000 | 30,000,000 | his estimate 99.9999936 ± 0.0000064% | about 12 deals/s with his newer program; his hardest 1999 deal took about 2 hours |
| Blake & Gent, Solvitaire (JAIR) | — | — | 99.99948 ± 0.00052% (95%) | general-purpose solver |
| **This solver** | **1,000,000,000** | **999,999,998; the other 2 proven lost** | **Wilson 95% [99.99999927%, 99.99999995%]** | **43,900 deals/s sustained on a laptop with its GPU** |

Masten's deals come from his own generator, so his 30M deals and these are different deals; the agreement is in the
rate, not the deals. Both earlier intervals contain this one. Before this run no random Accordion deal had been shown
unwinnable, as far as docs/RESEARCH.md found. At 2 in a billion that is no surprise: Masten's 30M deals would hold
about 0.06 of one.

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
each with an exact set of dead positions: a plain one (the rules only, in a fixed order) and one with everything the
solver uses (random order, dead-end tests, sleep sets, endgame tables). It proves all three constructed unwinnable golden
cases lost: the deal with no legal moves, the frozen 44-pile dead end, and a 16-pile dead end reached from seed 42. The
plain run's dead sets match the independent Python reference's reachable-position counts exactly (1, 353 and 71).

**Two random deals proven lost.** The billion run left two deals not won. Both went through the §5 escalation and were
proven lost. An exhaustive search with the independent Python rules (`testdata/tools/check_loss.py`) agrees on both: it
finds no win, and exactly as many reachable positions as the plain run has dead ones (in a lost deal every reachable
position is dead).

| | Seed 202,346,720 | Seed 112,249,515 |
|---|---|---|
| In the billion run | every line searched after 132,549 positions (0.02 s) | stopped at the limit, 50M positions (19 s) |
| `acc solve --escalate` (16 threads) | | not won within 200M or 2B positions; the 20B stage searched every line after 17.2B (3.5 min) |
| `acc prove`, plain run | 379,939 positions, 74,970 dead (0.05 s) | 110,959,471 positions, 19,386,305 dead (29 s) |
| `acc prove`, independent run | 100,283 positions, 74,970 dead (0.02 s) | 31,800,248 positions, 19,383,970 dead (13 s) |
| Python: every reachable position | 74,970, no win (1 s) | 19,386,305, no win (4 min) |
| Fewest piles reachable | 32 | 2 |

The independent run stores slightly fewer dead positions where its endgame tables and dead-end tests settle some without
a search. The `loss` records are in `hard-deals/proofs/` (`acc verify` checks their moves; a loss can't be confirmed by
replay), and the dossiers in `hard-deals/` say `loss (proven)`.

Since this run, `acc batch` proves the deals it doesn't win itself, while the run goes on, and the prover is 2.4× faster
(both runs on seed 112,249,515 now take 16 s; docs/BENCHMARKS.md, "After Phase 8 — proving losses"). A future run's
undecided deals are only those a proof of 10 billion positions a run can't settle.

**Why they're lost.** The Python search also prints what it saw:

- **Seed 202,346,720 locks up at its left end.** It opens 3H 5S QH, and none of the three ever moves: 5S doesn't match
  3H, QH doesn't match 5S, and nothing ever lands on them. A card could land on them only from the 4th, 5th or 6th slot,
  and only a heart, a spade, a 3, a 5 or a queen. In all 74,970 positions those slots only ever hold 2C or 5C (which could
  move onto 3H or QH), AD (onto 5S), and 2C, 7C or 9C (onto QH): never a match. Every line stops after at most 20 moves,
  with 32 to 40 piles left.
- **Seed 112,249,515 misses by one card.** It has no block like that, and lines get down to 2 piles. But of its 19.4M
  positions only one has 2 piles, 8D 9C, which share neither suit nor rank. Of the 11 with 3 piles, two lead into it (8D
  AC 9C and 8D 7C 9C) and the other nine have no move. On the way, the leftmost pile can take cards only from the 2nd and
  4th slots, and only nine cards are ever on top of it (2H 5H 7S 8D 8S JD JH JS QH).

## How every win is checked

- The solver searches with the fast engine, but nothing counts until the deliberately simple **reference engine** replays
  the whole line. That covers legality of every move, the card set, and one pile after exactly 51 moves (PLAN.md §6.1).
- The reference engine's rules agree with an independent Python implementation on every golden file. The fast engine
  agrees with the reference engine over 10M lockstep random playouts.
- The GPU's lines are replayed the same way: 81.8M of them in the 100M run and 786.2M in the billion run, none failing. The kernel's rules agree with
  the Rust engines move by move on random playouts from the golden files and 20,000 seeded deals (`crates/gpu/tests`).
- The pruning the solver relies on agreed with plain search on 1,000,000 random residuals:
  - sound dead-end tests;
  - sleep sets;
  - endgame tables, whose winnable counts also match an independent Python enumeration.
- Lossy caching can only miss wins, never invent one, and every win is replayed anyway.

## The strategy

The defaults since Phase 5 (docs/BENCHMARKS.md has the measurements):

- **Ladder:** up to 256 attempts of 36 random moves, then a first-win depth-first search of the remaining 16 piles
  with a 4,000-position budget. Then 34 moves and 8,000 positions, and so on down to 24 moves. Since after Phase 8 the
  random moves favour leaps, 8 to 1 against a slide.
- **Thorough search** if the ladder fails: randomized first-win DFS from the start with Luby restarts of 1,000 positions.
- **Throughout:**
  - a dead-position table kept across attempts;
  - the connectivity test at ≤19 piles, on trigger;
  - the leftmost-card test (since after Phase 8);
  - sleep sets (commuting moves are tried in one order only);
  - complete endgame tables keyed by suit/rank pattern for ≤11 piles.
- **GPU**, when there is one (since Phase 8): before any of the above, up to 256 attempts of 38 random moves and then up
  to 1,024 of 36, each followed by a 2,000-position search. Its random moves also favour leaps 8 to 1, and never
  play a move that leaves no move. A deal it misses goes to the CPU, which solves it from the start.

## Reproducing

```bash
cargo run --release -p accordion-cli -- endgame build --piles 11
```

```bash
cargo run --release -p accordion-cli -- batch --from 0 --count 1000000000
```

The 100M run added `--gallery 20`. The Phase 7 runs were on the CPU alone; to repeat one, add `--gpu off`:

```bash
cargo run --release -p accordion-cli -- batch --from 10000000 --count 20000000 --gpu off
```

A single deal's line can be printed and checked with `acc solve --seed N` and `acc verify`. The two losses:

```bash
cargo run --release -p accordion-cli -- solve --seed 112249515 --escalate
```

```bash
cargo run --release -p accordion-cli -- prove --seed 112249515
```

```bash
python testdata/tools/check_loss.py 202346720 112249515
```
