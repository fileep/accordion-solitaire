# Results

The big runs of PLAN.md Phases 7 and 8 and after: how often open Accordion is won, and what it costs. The rules are the
52-card open deal; a pile moves onto the pile 1 or 3 to its left when the top cards share a suit or rank; **a win is one
pile left**.

## Headline

**1,000,000,000,000 random deals (seeds 0–999,999,999,999): 999,999,999,279 won, every win checked by replaying its line, and
the other 721 proven unwinnable, in 55 h 54 min of running at 4,969,178 deals/s on the same laptop.** Their win rate is exactly
**99.9999999279%**: 0.721 lost per billion, or 1 deal in 1.39 billion. That is a count, with no uncertainty: every deal was won
with a checked line or proven lost, and none was left undecided. For the loss rate of all random deals, the exact 95% interval
(Clopper–Pearson) is **0.669 to 0.776 per billion**, 1 in 1.29 to 1.49 billion, or a win rate of
**[99.9999999224%, 99.9999999331%]**; it assumes the seeds give uniformly random deals. The run covers the seeds of all the runs
below again, and found exactly their 23 losses among them. Nothing failed verification.

The runs before it:

**1,000,000,000 random deals (seeds 0–999,999,999): 999,999,998 won, every win replayed by the reference engine, and the
other 2 proven unwinnable. No two deals are the same game.** 1 deal in 500 million can't be won; the exact 95% interval is
1 in 138 million to 1 in 4.1 billion, or a win rate of **[99.99999928%, 99.99999998%]**. They are the first random deals shown unwinnable that docs/RESEARCH.md knows of (see Losses).

**A second billion (seeds 1,000,000,000–1,999,999,999): 999,999,999 won and 1 proven lost, in 45 min 28 s at 366,514
deals/s,** 8.4 times the first billion's speed on the same laptop. No two of its deals are the same game. Over both,
**1,999,999,997 of 2,000,000,000 deals won and the other 3 proven lost**: 1 deal in 670 million can't be won (exact 95%:
1 in 228 million to 1 in 3.2 billion; win rate [99.99999956%, 99.99999997%]).

**Then 20 billion (seeds 2,000,000,000–21,999,999,999): 19,999,999,980 won and 20 proven lost, in 5 h 56 min 52 s at 934,074
deals/s,** 2.5 times the second billion's speed on the same laptop. No two of its deals are the same game. Over all three,
**21,999,999,977 of 22,000,000,000 deals won and the other 23 proven lost**: 1 deal in 960 million can't be won (exact 95%:
1 in 637 million to 1 in 1.51 billion; win rate [99.99999984%, 99.99999993%]).

The intervals here are exact (Clopper–Pearson; at these rates Garwood's for a Poisson count is the same to four decimals of a
count). Wilson's, which Blake and Gent use, is nearly the same at the trillion's 721 losses but narrower at a few; it is beside the
trillion's in Compared with earlier work, and `research/paper/paper_numbers.py` prints all three.

## Runs

All runs used `acc batch` with the default strategy (below) on the laptop in docs/BENCHMARKS.md: i7-1260P with 16 threads,
at its sustained heat-soaked clocks, and for the last five runs its NVIDIA T550 GPU as well. Deals come from seeds via the
specified generator (docs/FORMATS.md), so anyone can re-deal and re-check them.

| Seeds | Deals | Won | 95% interval (exact) | Solved on | Deals/s | Wall time |
|---|---|---|---|---|---|---|
| 0–999,999 | 1,000,000 | 1,000,000 | [99.999631%, 100%] | CPU | 19,512 | 51 s |
| 0–9,999,999 | 10,000,000 | 10,000,000 | [99.999963%, 100%] | CPU | 15,348 | 10 min 52 s |
| 10,000,000–29,999,999 | 20,000,000 | 20,000,000 | [99.999982%, 100%] | CPU | 13,422 | 24 min 50 s |
| 0–99,999,999 | 100,000,000 | 100,000,000 | [99.999996%, 100%] | CPU + GPU | 19,281 | 1 h 26 min 27 s |
| 0–999,999,999 | 1,000,000,000 | 999,999,998 | [99.99999928%, 99.99999998%] | CPU + GPU | 43,855 | 6 h 20 min 3 s |
| 1,000,000,000–1,999,999,999 | 1,000,000,000 | 999,999,999 | [99.99999944%, 99.999999997%] | CPU + GPU | 366,514 | 45 min 28 s |
| 2,000,000,000–21,999,999,999 | 20,000,000,000 | 19,999,999,980 | [99.99999985%, 99.99999994%] | CPU + GPU | 934,074 | 5 h 56 min 52 s |
| **0–999,999,999,999** | **1,000,000,000,000** | **999,999,999,279** | **[99.9999999224%, 99.9999999331%]** | **CPU + GPU** | **4,969,178** | **55 h 54 min, in 6 sessions** |

The first three are Phase 7 and the 100M run is Phase 8. They won every deal: none ended `unknown`, so they left no
dossiers. The billion run came after Phase 8, with the faster code of docs/BENCHMARKS.md's "After Phase 8" sections
(commit a761123; its speed log says f91bd67+dirty, as the run started before that commit was made). It covers all of their
deals again and 900M new ones. Two of its deals were not won, one at the 50M-position limit and one with every line
searched; both went through the §5 escalation and were proven lost (Losses, below), so its interval counts them as
losses. A deal that is not won but not proven lost counts as a loss for the interval's lower bound and as a win for its
upper bound.

The second billion ran commit d167ccf (docs/BENCHMARKS.md, from "split warps" to "cheaper verification, and the retune"), on
seeds no earlier run used. Its one deal not won stopped at the 50M-position limit, and `acc batch` itself proved it lost
while the run went on (Losses).

The 20 billion ran commit 926e105 (docs/BENCHMARKS.md, "what winning prefixes have in common" and "pausing, and tens of billions
of deals"), again on seeds no earlier run used, in one session: it didn't need to pause. 19 of its 20 deals not won had every line
searched and one stopped at the limit, and `acc batch` proved them all lost as it went (Losses). The check for repeated games and
the last proofs took about 27 minutes more.

The trillion ran commit fbbff00 from seed 0, so it covers every earlier run's seeds again. It was started as 2 trillion deals
and cut to a trillion on 2026-11-11, at 833.7 billion, by lowering the count in its checkpoint; it then finished normally. It ran
in 6 sessions, from 2026-11-09 to 2026-11-12, between pauses to charge the battery (docs/LONG_RUNS.md), a Windows Update restart
and that change. The solver didn't win 726 deals: 630 with every line searched and 96 at the 50M-position limit. `acc batch` won 5
of them with its exhaustive search and proved the other 721 lost as it went, with none left undecided. Below 22 billion its losses
are exactly the 23 of the earlier runs. Its speed logs, report and events log are in `research/runs/`.

**No repeated games.** The 100M and billion runs also checked that their deals are different games, not only different
orders of the cards. Two deals that differ only in the names of the suits or ranks play exactly alike, so each deal is
renamed first (suits and ranks in order of first appearance). The renamed decks are then compared: 64-bit fingerprints
are sorted, and any two with the same fingerprint are compared card by card. All 100,000,000, and then all
1,000,000,000, differ. The second billion passed the same check among its own deals, and the 20 billion among its own, from
disk, as 160 GB of fingerprints wouldn't fit in memory; the three were not compared with each other. The trillion checked a
sample: a hash of the renamed deal picked 2,272,765,214 deals (0.227%), and none of them repeat. Two deals that are the same game
are always picked together, so 1,317 or more such pairs among the trillion would have shown with 95% chance. 2,349,633 other pairs
share 40 bits of their fingerprints, against 2,348,980 expected by chance.

**The GPU's share.** Every GPU line was replayed, and none failed: by the reference engine up to the first billion, by the
reference rules on the top cards (`replay_tops`) in the second billion and the 20 billion, and in the trillion mostly by
`quick::line_wins`, with 1 deal in 256 through the reference engine as well (How every win is checked, below). The CPU solved the rest: the deals
the GPU tried and missed, solved again from the start, and the deals the CPU workers took for themselves while the GPU
was busy.

| | 100M run | Billion run | Second billion | 20 billion | Trillion |
|---|---|---|---|---|---|
| Deals given to the GPU | 82,273,155 | 790,102,016 | 996,646,794 | 20,000,000,000 | 1,000,000,000,000 |
| Won on the GPU | 81,823,614 (99.45%) | 786,211,377 (99.51%) | 994,056,063 (99.74%) | 19,997,847,246 (99.989%) | 999,945,361,644 (99.9945%) |
| GPU ladder attempts | 13.5 billion | 127.5 billion | 132.7 billion | 553.6 billion | 5,021 billion |
| GPU speed | 214M positions/s | 331M positions/s | 3,206M positions/s | 2,632M positions/s | 2,721M positions/s |
| GPU misses, solved on the CPU | 449,541 | 3,890,639 | 2,590,731 | 2,152,754 | 54,638,356 |
| Deals the CPU workers took for themselves | 17,726,845 | 209,897,984 | 3,353,206 | 0 | 0 |

In the billion run the CPU workers spent 79.6% of their time on their own deals, 10.9% on the GPU's misses and 7.8%
verifying GPU lines (34.1 µs a line). Handing results on took 1.1% and the rest 0.6%; they never waited for the GPU. In
the second billion they spent 71.3% on the GPU's misses, 11.8% verifying its lines (4.9 µs a line), 9.8% on their own
deals, 2.2% handing results on and 5.0% on the rest. The GPU did nearly everything, and held back only briefly when the
CPU fell behind (at most 170,848 results waiting).

In the 20 billion the GPU had every deal, and missed 1 in 9,290. The CPU workers spent 77.2% of their time waiting for it, 16.3%
verifying its lines (2.6 µs a line), 6.0% on its misses and 0.5% on the rest: the GPU sets the speed now. A deal took it 27.7
attempts and about 2,800 positions, against 133 and about 8,800 in the second billion, so it wins more deals on fewer positions a
second.

In the trillion the GPU missed 1 deal in 18,300, and a deal took it 5.0 attempts. The CPU workers spent 36.8% of their time waiting
for it, 31.5% on its misses, 26.2% verifying its lines (0.8 µs a line), 5.3% handing results on and 0.3% on the rest.

**Sustained speed.** After its first two minutes, the 100M run held 18,900–19,700 deals/s over each stretch of 10M deals.
The CPU alone held 13,422 deals/s over the 20M run, so over that long run the GPU added about 1.4×. That was less than the
2.15× median of the short comparison runs in docs/BENCHMARKS.md (Phase 8). (Since then the GPU has come to do nearly all
the work: the 20 billion ran at 934,074 deals/s, 48 to 70 times the CPU alone of Phase 7, and the trillion at 4,969,178 on the
same laptop; the CPU alone hasn't been timed again.) The CPU slowed far more than the GPU over
the 100M run:

- the CPU's median time per deal was 1.15 ms, against 0.47–0.61 ms in the short runs with the GPU;
- the GPU's rate fell only from 228M to 214M positions/s;
- the GPU's share of the deals was 82%, against 58–65% in the short runs.

On this laptop the CPU and the GPU share one cooling system.

The billion run held **42,169–45,026 deals/s in each of its six full hours** (43,737 over the last 20 minutes), and the
GPU 330–332M positions/s throughout. Its speed log (`acc batch` writes a row a second to `logs/`) has one second out of
22,802 below 20,000 deals/s after the first half minute. That is 2.3× the 100M run, from the code since Phase 8: better
GPU occupancy and register stacks, the leftmost-card dead-end test, and leaner verification (docs/BENCHMARKS.md, "After
Phase 8").

The second billion held **363,504–377,216 deals/s over every stretch of 100M deals** (the first was the fastest; after it
363,504–367,647), and the GPU 3,180–3,243M positions/s over every five minutes. Its speed log has no second below 200,000
deals/s after the first half minute.

The 20 billion held **927,678–937,553 deals/s in each of its six hours**, the first the slowest, and its speed log has no second
below 850,114 deals/s from start to finish.

The trillion averaged **4,969,178 deals/s** over its 6 sessions. Its last session, 7.4 hours, held **6.44–6.63M deals/s in each
full hour** after a slow first one, and the first session's first seven hours 6.41–6.75M. The average is lower for two reasons.
Two sessions (12.5 hours) ran at 3.5–3.7M: Windows' power throttling had put the process, which had no window of its own, on
the efficiency cores alone. A small script turned that off for acc's process, by calling
`SetProcessInformation(ProcessPowerThrottling)` with the execution-speed bit in the mask and cleared in the state
(docs/TROUBLESHOOTING.md, "The run is half as fast as it was"). And in the first and third sessions other work on the laptop, even at idle
priority, took some of the shared power budget, down to 1.4M deals/s for two hours of the first.

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
| Positions, second billion (5.9M deals on the CPU, mostly the GPU's misses) | 4,884 | 39,364 | 131,917 | 313,943 | 50,000,001 |
| Time, second billion (5.9M deals on the CPU) | 1.36 ms | 14.6 ms | 58 ms | 143 ms | 4.85 s |
| Positions, 20 billion (2.15M deals on the CPU, all the GPU's misses) | 39,262 | 138,582 | 418,504 | 970,789 | 50,000,001 |
| Time, 20 billion (2.15M deals on the CPU) | 6.65 ms | 19.1 ms | 38 ms | 81 ms | 3.16 s |
| Positions, trillion (54.6M deals on the CPU, all the GPU's misses) | 35,516 | 124,525 | 398,587 | 997,454 | 50,000,001 |
| Time, trillion (54.6M deals on the CPU) | 6.53 ms | 39.4 ms | 168 ms | 488 ms | 49.3 s |

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

In the second billion the GPU ran up to 1,280 tries with a 700-position search and won **994,056,063 deals (99.41%)**. The
CPU won the rest: rung 0 4,264,658, rung 1 1,148,628, rung 2 419,421, rungs 3–6 110,801, and the thorough search 428.

In the 20 billion the GPU's up to 1,280 tries won **19,997,847,246 deals (99.989%)**, choosing its moves by what winning prefixes do.
The CPU won the deals it missed: rung 0 244,759, rung 1 562,320, rung 2 678,360, rungs 3–6 658,965, and the thorough search 8,330.

In the trillion the GPU's up to 384 tries won **999,945,361,644 deals (99.9945%)**. The CPU won the deals it missed: rung 0
6,529,094, rung 1 12,753,974, rung 2 15,471,866, rungs 3–6 19,494,352, the thorough search 388,344, and the exhaustive search 5.

## Compared with earlier work

| | Deals | Won | Reported interval | Speed |
|---|---|---|---|---|
| Masten (1999 onwards, Solitaire Win Rates) | 30,000,000 | 30,000,000 | none published; Blake & Gent's Wilson 95% on this count is 99.9999936 ± 0.0000064% | about 12 deals/s with the newer program; the hardest 1999 deal took about 2 hours |
| Blake & Gent, Solvitaire (JAIR 85, article 21, 2026) | 1,000,000 | 999,996; 4 undecided, none proven lost | 99.99948 ± 0.00052% (95%) | general-purpose solver: mean 9.8 CPU-seconds a deal, 110 CPU-days in all |
| **This solver** | **1,000,000,000,000** | **999,999,999,279; the other 721 proven lost** | **99.9999999279% measured; 95% interval [99.9999999224%, 99.9999999331%]** | **about 5 million deals/s on a laptop with its GPU** |

Masten's deals come from another generator, so those 30M deals and these are different deals; the agreement is in the
rate, not the deals. Both earlier intervals contain this one, and both still include 100%. Written as Blake & Gent write
theirs, Wilson's interval for the trillion is 99.9999999277 ± 0.0000000053%. Its centre is not the measured rate, as Wilson's
centre adds about 2 losses to the 721, and its bounds, [99.9999999224%, 99.9999999330%], differ from the exact ones only in the
last digit. Before the first billion no random
Accordion deal had been shown unwinnable, as far as docs/RESEARCH.md found. At 0.72 in a billion that is no surprise: Masten's
30M deals would hold about 0.02 of one.

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

The solver never claims a loss from its lossy table. `acc prove` proves a loss with two exhaustive searches, each
with an exact set of dead positions: a plain one (the rules only, in a fixed order) and one with everything the solver
uses (random order, dead-end tests, sleep sets, endgame tables). Both use the solver's move generator, so they guard
against its pruning, not against that: the Python search below and the certificates and Lean proofs in `research/lean` do. It proves all three constructed unwinnable golden
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

**A third, in the second billion.** Seed 1,005,616,798 stopped at the limit, 50M positions (4.85 s), and `acc batch` proved
it lost as the run went on: the independent run after 6,702,116 positions (3,389,020 dead, 1.9 s), the plain run after
16,141,582 (3,405,471 dead, 2.7 s). The Python search agrees: 3,405,471 reachable positions, no win (39 s), and it gets
down to 2 piles six ways, none of them able to finish. Its record and dossier are in `hard-deals/`.

**Twenty more, in the 20 billion.** `acc batch` proved all 20 lost as the run went on, each in under a second. The Python search
agrees on every one: no win, and exactly as many reachable positions as the plain run has dead ones, 4,081,460 in all. Their records
are in `hard-deals/proofs/`, and `acc verify` passes them all.

| Seed | In the run | `acc prove`, plain run | Independent run | Python: reachable, no win | Fewest piles |
|---|---|---|---|---|---|
| 3,417,058,778 | every line searched after 12,512 positions (0.002 s) | 416 positions, 128 dead (0.0 s) | 155, 128 dead (0.0 s) | 128 | 45 |
| 4,959,093,869 | every line searched after 108,125 positions (0.016 s) | 331,164 positions, 63,720 dead (0.02 s) | 71,857, 63,720 dead (0.013 s) | 63,720 | 32 |
| 5,895,817,790 | every line searched after 24,450,876 positions (1.284 s) | 4,359,712 positions, 957,502 dead (0.504 s) | 1,640,371, 955,955 dead (0.285 s) | 957,502 | 3 |
| 6,885,997,408 | every line searched after 494,044 positions (0.038 s) | 1,377,392 positions, 281,440 dead (0.151 s) | 397,391, 281,440 dead (0.089 s) | 281,440 | 27 |
| 7,311,711,652 | every line searched after 37,395 positions (0.013 s) | 51,436 positions, 12,696 dead (0.004 s) | 16,307, 12,696 dead (0.004 s) | 12,696 | 35 |
| 7,548,976,414 | every line searched after 232,340 positions (0.019 s) | 665,760 positions, 130,240 dead (0.054 s) | 179,721, 130,240 dead (0.029 s) | 130,240 | 28 |
| 7,615,968,364 | every line searched after 23,684,407 positions (1.647 s) | 2,553,071 positions, 726,019 dead (0.376 s) | 1,143,145, 650,005 dead (0.306 s) | 726,019 | 2 |
| 7,812,820,073 | every line searched after 73,321 positions (0.012 s) | 141,456 positions, 30,608 dead (0.009 s) | 47,146, 30,608 dead (0.006 s) | 30,608 | 31 |
| 10,345,199,754 | every line searched after 228,380 positions (0.018 s) | 549,000 positions, 107,325 dead (0.046 s) | 186,485, 107,325 dead (0.025 s) | 107,325 | 29 |
| 12,929,752,010 | every line searched after 118,108 positions (0.011 s) | 223,841 positions, 79,426 dead (0.025 s) | 88,129, 59,498 dead (0.016 s) | 79,426 | 2 |
| 13,060,247,781 | every line searched after 49,521 positions (0.003 s) | 73,650 positions, 15,750 dead (0.005 s) | 18,725, 15,750 dead (0.003 s) | 15,750 | 32 |
| 13,313,622,150 | every line searched after 25,702 positions (0.004 s) | 17,502 positions, 5,148 dead (0.002 s) | 7,242, 5,148 dead (0.003 s) | 5,148 | 35 |
| 14,204,616,047 | stopped at the limit after 50,000,001 positions (3.156 s) | 4,642,496 positions, 923,520 dead (0.561 s) | 1,570,194, 923,520 dead (0.35 s) | 923,520 | 31 |
| 17,436,063,346 | every line searched after 34,084 positions (0.005 s) | 39,432 positions, 7,344 dead (0.003 s) | 11,542, 7,344 dead (0.002 s) | 7,344 | 39 |
| 17,480,567,423 | every line searched after 397,876 positions (0.028 s) | 929,402 positions, 219,072 dead (0.078 s) | 326,366, 219,072 dead (0.054 s) | 219,072 | 27 |
| 18,238,151,902 | every line searched after 313,264 positions (0.022 s) | 568,832 positions, 168,098 dead (0.063 s) | 274,798, 167,386 dead (0.052 s) | 168,098 | 3 |
| 19,010,792,488 | every line searched after 444,487 positions (0.038 s) | 1,452,708 positions, 271,728 dead (0.13 s) | 341,039, 271,728 dead (0.079 s) | 271,728 | 29 |
| 20,456,660,150 | every line searched after 77,584 positions (0.008 s) | 187,640 positions, 33,600 dead (0.014 s) | 47,295, 33,600 dead (0.009 s) | 33,600 | 36 |
| 20,677,695,248 | every line searched after 43,050 positions (0.01 s) | 72,648 positions, 15,840 dead (0.005 s) | 21,051, 15,840 dead (0.005 s) | 15,840 | 36 |
| 21,018,945,683 | every line searched after 73,043 positions (0.01 s) | 195,456 positions, 32,256 dead (0.015 s) | 38,171, 32,256 dead (0.007 s) | 32,256 | 35 |

Sixteen of the 20 get no lower than 27 piles, and only two get down to 2. The one that stopped at the limit is no larger than the
others: seed 14,204,616,047 has 923,520 positions, which the solver, with its lossy table and its restarts, went over until it had
generated 50M.

**721 in the trillion.** `acc batch` proved them all lost as the run went on; below 22 billion they are the same 23 seeds as above.
The Python search of `classify_losses.py` (below) agrees on every one: no win, and exactly as many reachable positions as the plain
run has dead ones, 424,002,536 in all, the most 22,066,800 (seed 268,817,008,782). Their records are in `hard-deals/proofs/`, and
each has an automaton certificate checked in Python and proved in Lean (`research/lean/`).

**How often.** 721 losses in a trillion deals, 0.72 per billion. The first 22 billion had 23, 1.05 per billion, and the other 978
billion 698, 0.71 per billion: high, though not far outside chance (exact two-sided p = 0.07). The loss seeds fit uniform
placement over the trillion (Kolmogorov–Smirnov p = 0.96).

Since the first billion, `acc batch` proves the deals it doesn't win itself, while the run goes on, and the prover is 2.4× faster
(both runs on seed 112,249,515 now take 16 s; docs/BENCHMARKS.md, "After Phase 8 — proving losses"). A future run's
undecided deals are only those a proof of 10 billion positions a run can't settle.

**Two kinds of loss.** `testdata/tools/classify_losses.py` searches every reachable position of every loss again with the
independent Python rules, and records which cards ever interact: a move joins the card that moves and the card it covers. All 721
have no win and exactly the plain run's count of positions (424,002,536 in all, for the 721 of the trillion). Row by row in
`research/runs/losses.csv`:

| Kind | Deals | Groups of cards that ever meet | Fewest piles reached | Cards that never move |
|---|---|---|---|---|
| **Split** | 595 | 2–51 | 2–51 | 13–51 |
| **Endgame** | 126 | 1 | 2–5 | 1–28 |

A split deal has a short explanation, though the groups come from the search: knowing that two cards never meet takes every reachable
position. A pile only ever holds cards of one group, so at least one pile per group is always left, and two or more groups can't be
won. An endgame deal has no such block: all 52 cards link up through
moves that some line can play, but no single line plays enough of them, and play gets down to 2 to 5 piles and stops there.

**Why they're lost, two in detail.** Seed 202,346,720 is split and seed 112,249,515 an endgame loss. The Python search of
`check_loss.py` also prints what it saw:

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

- **By run.** Up to the first billion, the reference engine replayed every line. The second billion and the 20 billion replayed
  every line by the reference rules on the top cards alone (`replay_tops`, below), and sent 1 deal in 256 through the reference
  engine as well. The runs keep no lines (51 TB for the trillion at 51 bytes each), so to check a win independently, solve
  that deal again and replay its line; a deal once won stays won, whatever path the search took. The trillion run (fbbff00)
  checks 255 of 256 GPU lines with `quick::line_wins` instead, a third implementation of the rules on the card codes the GPU
  had, which says yes only to a legal win of a full deck (tests compare it with the reference engine on legal and illegal
  moves) and sends anything else to the reference rules. 1 deal in 256 goes through the reference engine, which also checks
  that the GPU had that seed's deal. Every line won on the CPU is replayed by the reference rules, and 1 in 256 of them by
  the reference engine as well.
- The solver searches with the fast engine, but nothing counts until a check separate from the search replays the
  whole line: the deliberately simple **reference rules**, or for most GPU lines of the trillion run `quick::line_wins`. That covers legality of every move, the card set, and one pile after exactly 51 moves (PLAN.md §6.1).
- The reference engine's rules agree with an independent Python implementation on every golden file. The fast engine
  agrees with the reference engine over 10M lockstep random playouts.
- **At scale, by independent rules.** A rule too loose in both the search and its replay would count a lost deal as won,
  and no Lean proof would notice. So on 2026-11-14 the trillion run's program (fbbff00) re-solved 100 million deals, in
  100 blocks of a million spread evenly over its seeds, and wrote every checked line; `testdata/tools/replay_lines.py`
  replayed all 100,000,000 with the Python rules and generator of `make_golden.py`, which share no code with the Rust
  ones. Every deal was its seed's, every move legal, and every line ended in one pile after 51 moves; none failed. A
  damaged move, deal or line is refused. The log is `research/runs/win-replay.log`.
- The GPU's lines are replayed the same way: 81.8M of them in the 100M run and 786.2M in the billion run, none failing.
  Since the second billion, `acc batch` without `--out` replays each line by the reference rules on the top cards alone,
  which is all they look at, and allocates nothing (`replay_tops`: a test checks it against the reference engine on random
  legal and illegal moves); 1 deal in 256 goes through the reference engine as well, which has to agree. Of its 994.1M
  lines none failed. The kernel's rules agree with
  the Rust engines move by move on random playouts from the golden files and 20,000 seeded deals (`crates/gpu/tests`).
- The pruning the solver relies on agreed with plain search on 1,000,000 random residuals:
  - sound dead-end tests;
  - sleep sets;
  - endgame tables, whose winnable counts also match an independent Python enumeration.
- Lossy caching can only miss wins, never invent one, and every win is replayed anyway.

## Are the deals uniformly random?

The win rate is a statement about uniformly random deals, and the runs take theirs from consecutive seeds. So
`crates/core/examples/deal_stats.rs` compares the deals of a range of seeds with exact expectations, in 25 tests: which card lands in
each position; the cards at six pairs of positions a slide or a leap apart; the first three cards together; fixed points, parity and
inversions of the shuffle; opening slides and leaps; and how many positions hold the same card in the deals of seeds s and s + d, for
twelve differences d. A set of tests fails if any p is under 0.002, 5% shared among the 25. The output is in
`research/runs/generator-tests.txt`. As a control, the textbook wrong shuffle fails the first 13 tests outright on a million deals
(every p under 0.000001).

| Deals | Seeds | Smallest p of the 25 | |
|---|---|---|---|
| 22,000,000,000 | all of 0–21,999,999,999 | 0.017 | pass |
| 10,082,474,226 | every 97th from 22,000,000,000, up to the trillion | 0.00011 (same cards as seed + 2^32) | **one test under the line** |
| 2,000,000,000 | every 97th from 200,000,000,000, fresh seeds | 0.013 | pass; seed + 2^32 has p = 0.79 |

Each set tested the seed differences on its first billion deals, the last on all two billion. The one failure was chance, as a real
effect grows with the deals: at twice as many pairs its chi-square of 31.6 would have been about 55, and on fresh seeds it was 4.7. Earlier,
a timing run of 200M deals showed a low p for seed + 8 (0.0013), which went back to 0.60 at a billion.

These tests look at the deals, and at their low-order structure; the raw generator has not been through a general battery such as
TestU01 or PractRand. A 64-bit seed can reach at most 2^64 of the 52! orders of the cards.

A bias confined to deals as rare as lost ones would not show in these tests, so the measurement was repeated with a second
generator: ChaCha20 (RFC 8439) keyed by the seed, each index drawn by masking a byte and redrawing instead of Lemire's method, and
the same shuffle, solver and checks (docs/FORMATS.md, "The second generator"). Seeds 0–99,999,999,999 gave 99,999,999,919 wins,
every line replayed, and 81 proven losses, none undecided: **0.810 lost per billion, exact 95% interval 0.643 to 1.007**, against
0.721 in the trillion run. The ratio of the two rates is 1.12 (exact conditional 95% interval 0.88 to 1.42; two-sided p = 0.33).
With about 72 losses expected, the comparison would catch a difference of about 40% four times in five; it cannot rule out smaller
ones. The run was planned as 200 billion deals and cut to 100 billion for time: 15.8 hours of running in 7 sessions, at 1,756,634
deals/s, slower than the main runs because making a ChaCha20 deal on the processor takes about 5 times the work. Its records are in
`research/runs/` and its losses in `hard-deals-chacha/`. All 81 are proved in Lean as theorems about `ChaCha.deal seed`, and
nanoda accepts the proofs; the independent Python rules searched each exhaustively, found no win and counted the same positions
as its record, 103,643,234 in all (`research/runs/losses-chacha.csv`).

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
- **GPU**, when there is one (since Phase 8; these are the defaults of the build the trillion run used, `acc settings --gpu`):
  before any of the above, a first rung of attempts of 40 random moves and a second of 38 (`rungs` `[[40, 24], [38, 376]]`, in
  the kernel's numbering of tries, which comes to at most 384 played), each followed by a search of the rest within 700
  positions. After a deal's first 16 tries, tries go in pairs that share their first 20 random moves; a rest with more
  than 8 ranks (10 on the second rung) isn't searched; and the random moves
  favour leaps 8 to 1 and are scored by what winning prefixes have in common (docs/BENCHMARKS.md, from "what winning prefixes
  have in common" to the fourth step). A deal it misses goes to the CPU, which solves it from the start. Earlier runs used
  earlier settings: the 100M and billion runs up to 256 attempts of 38 random moves and up to 1,024 of 36, each with a
  2,000-position search, as in the table above.

## Reproducing

The commands below run the current code, which reaches the same counts faster. To repeat a run of record exactly, check out its
build first; its speed log and report are in [research/runs/](../research/runs/README.md).

| Run | Build |
|---|---|
| 100M | af14316 plus uncommitted changes (`af14316+dirty`) |
| First billion | a761123; its speed log says `f91bd67+dirty`, as the run started before that commit was made |
| Second billion | d167ccf |
| 20 billion | 926e105 |
| Trillion | fbbff00 |

```bash
cargo run --release -p accordion-cli -- endgame build --piles 11
```

```bash
cargo run --release -p accordion-cli -- batch --from 0 --count 1000000000
```

```bash
cargo run --release -p accordion-cli -- batch --from 1000000000 --count 1000000000
```

```bash
cargo run --release -p accordion-cli -- batch --from 2000000000 --count 20000000000
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
