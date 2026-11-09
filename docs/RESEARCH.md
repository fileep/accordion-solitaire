# Accordion Solitaire — Research Findings

Research behind [PLAN.md](../PLAN.md). The probes cited here are in
[research/probes/](../research/probes/) and can be re-run with Python 3 (`python probe.py ...`).
All probe numbers come from a deliberately naive Python engine (~220k nodes/s). They show
**shape and orders of magnitude**; Rust timings will be about 100× faster.

---

## 1. Key findings

1. **Open Accordion is almost certainly ~100% winnable.** Mark Masten's solver won **30,000,000 of
   30,000,000** random deals (no losses, no inconclusives). Blake & Gent's Solvitaire estimates
   99.99948 ± 0.00052%. No random deal had been shown unwinnable (this project's two billion-deal runs later proved
   three lost: docs/RESULTS.md). The "1 in 100" figure
   repeated online (Wikipedia, game sites) applies to playing without full look-ahead, not to the open game.
2. **Masten's winning method is random early play plus brute force late, with restarts.** He gave up proving
   losses and only hunted wins. Each attempt plays "a lot of random moves in the first half"
   and then brute-forces the remainder, repeating until a win or a limit.
3. **Full brute force is infeasible; brute-forcing the back half is cheap.** Reachable states from
   a 24-pile row are about 1.5M, growing roughly 2–3× per extra pile. A full 52-card deal extrapolates to
   around 10^17 states.
4. **The prefix-length trade-off is real, and escalation wins** (§5.5–5.6).
   - The best *fixed* switch point is 28 random moves, then brute-force the last 24 piles. It needs about 49k nodes to the
     first win, averaged per deal.
   - Deeper fixed switch points leave some deals that never win.
   - An **escalation ladder** averages about 27k nodes, 1.9× cheaper, and won every deal. It makes up to 16 attempts at
     38 random moves, then 36, 34 and so on down to 24.
   - So: make many cheap attempts deep in the game first, then grow the brute-force part.
5. **Randomized restarts help.** For the same node budget, plain DFS with fixed ordering won 43/50 deals; with
   randomized ordering and Luby restarts it won 49/50 and used fewer total nodes. That is the classic heavy-tailed
   search behaviour.
6. **Structure we can exploit:**
   - Only top cards matter.
   - Every win is exactly 51 moves, so the game graph is a DAG with no cycles.
   - Moves are local and commute.
   - Solvability depends only on the suit/rank equality pattern, so endgame knowledge is **shared across all deals**.
   - Match-graph connectivity is a cheap, exact necessary condition for a win.
   - Every win is replay-verifiable, so lossy caches are safe.
7. **Cheap dead-end tests are proven sound and pay off late in the search** (§5.8, §5.10).
   - **The tests:** orphan (no card left shares its suit or rank), connectivity, and a new "left-leaf pair" test.
   - **Effect:** together they cut brute-force nodes 3.3–7×.
   - **Soundness:** zero false positives on 628 labelled positions.
   - **They can't fire early:** none can fire before move 15, and left-leaf not before move 26. In random play none fired before move 26.
     Inside the brute force they fire ≤0.1% of the time above 20 piles.
   - **So they run only where they pay:**
     - not at all during random moves (filtering those cost about 450–500 tests per attempt for no win-rate gain);
     - once at the end of the random moves;
     - then gated by piles left and triggered by cheap events.
   - **Connectivity reduces to the 4 suits** staying linked through shared ranks, so it costs at most 3 ANDs per move.
8. **Search work concentrates at 10–14 piles** (§5.9).
   - **Suits and ranks are arbitrary labels** (§5.11). A cache keyed by relabelled suits and ranks saves 16% of nodes and 28% of memory, while exact keys save nothing.
   - **Little cross-deal reuse** (2–3%). A per-thread cache plus the precomputed ≤10-pile table is enough.
9. **GPU: worth it later, for one specific job** (§9).
   - **The job:** the deep-rung "lottery" of many random prefixes plus tiny brute forces on ≤20 piles.
     Each needs a small cache that fits in GPU shared memory (§9.4).
   - **Coverage:** with 64 tries per deep rung, that alone won 99.1% of deals.
   - **What stays on the CPU:** big-cache DFS.
   - **This machine's GPU:** an entry-level NVIDIA T550, so expect a modest gain. The plan gates GPU work on a measured prototype.
10. **Language:** Rust for the engine and solver, compiled to WASM for a TypeScript visualizer. C++ is the
    equivalent-speed alternative.
11. **Measured in Rust (Phase 5, §10):** the solver needs **6.4× fewer positions per deal** than in Phase 4 (5,943 against
    38,086 on the training deals) and runs **5.0× faster**: 14,805 against 2,952 deals/s on the 100,000 held-out test deals,
    all won. Four things did it:
    - a random-prefix ladder with many cheap tries at 36 random moves;
    - short Luby restarts;
    - sleep sets that skip move orders differing only by commuting moves;
    - endgame tables up to 11 piles.

    A cache keyed by suit/rank pattern and carried across deals did not pay. The GPU works, and a CPU+GPU hybrid is about
    1.7× the CPU alone on this laptop, short of the 2× gate. Built into `acc batch` anyway (Phase 8), it measured 2.15× on
    short runs and about 1.4× sustained, and won all of 100,000,000 deals (docs/RESULTS.md).

---

## 2. The game

**Names:** Accordion, Idle Year, Tower of Babel, Methuselah, Leapfrog. It is called Accordion because the row
expands as cards are dealt and contracts as piles combine.

### 2.1 Rules (our target: open / "thoughtful" Accordion)

| Rule | Detail |
|---|---|
| Deal | Shuffle a 52-card deck and lay **all 52 cards face up in one row**, left to right |
| Move | A pile may move onto the pile **immediately to its left** ("slide", distance 1) or **three piles to its left** ("leap", distance 3) |
| Legality | Allowed when the **top cards share a suit or a rank**. Ranks have no order; only equality matters |
| Piles | A moved pile goes on top of the target; the whole pile moves as one unit; cards underneath never matter again |
| Gaps | Close immediately, so the row shifts left |
| Choice | Moves are optional and can be made in any order; with two legal targets, the player chooses |
| Win | **One pile** remains. Each move removes exactly one pile, so every win is 51 moves |

### 2.2 Win conditions ("win conditions" in the brief)

**Project target: a single pile only.** Nothing else counts as a win.

- **Standard:** reduce the deck to a single pile.
- **Sheinwold's lenient scoring:** counts it as a win if five or fewer piles remain. This is for human play and is
  **out of scope** for this project.
- **Variant goals** (possible future work; Accordion's Revenge is the likely first):
  - *Accordion's Revenge* (Masten): a chosen card must end on top. Impossible only if it is the first or second card dealt.
  - *Royal Marriage:* reduce to the K and Q of one suit at the ends.
  - *23 Skidoo:* leaps of 2 and 3 only, no slides; reduce to 2 piles.

### 2.3 Deal styles and historical rules

| Style | Description | Win rate |
|---|---|---|
| **Open** (target) | All 52 dealt first, then planned with full information | ~100% (30M/30M, Masten) |
| Traditional | Deal one card at a time; play when possible; a new card only when no move exists | Very low. Keller's automatic-play solver over 86,400 deals: 1 in 2,335 (leaps first) and 1 in 1,543 (slides first) |
| Dick 1883 ("Idle Year") | Moves obligatory; nearest neighbour preferred | Self-working, very low |
| Gibson 1964 | Self-working: no deal while a play exists; leaps only when no slide | Very low |
| Culbertson / Goren | May deal one more card before deciding | Between the two |

The widely quoted "odds about 1 in 100" (Wikipedia infobox) predates computer analysis and fits the
traditional style. CodeAbbey users who simulated automatic versions got between 1/200 and 1/5000.

### 2.4 Variants worth parameterizing

- **Leap distances:** slide 1 with leap 2–10. Masten found leaps 4–6 easier than 3, and leap 2 hard but ≥40% winnable.
- **No-slide pairs:** 2/3 (23 Skidoo), 3/4, 4/5.
- *Concertina:* adds under-slides and under-leaps.
- **Deck size:** e.g. *Late Binding Solitaire*, Knuth's 18-card accordion. Gent's estimate is 47.021 ± 0.032%.

The engine takes leap distances as const generics, so the default {1,3} hot loop stays specialized.

---

## 3. Prior work

### 3.1 Masten (Solitaire Win Rates)

- **1999:** wrote an open-Accordion solver. It "got bogged down fairly quickly", so he abandoned proving losses
  and only searched for wins.
- **Method:** for each deal, repeat {play many random moves in roughly the first half, brute-force the
  rest} until a win, a time limit, or an attempt limit.
- **Results:**
  - 25,000 deals, all won.
  - About 800,000 over 16 months, all won.
  - With a newer program, more than 1M deals per day (≈12 deals/s). The total reached 30M deals, all won (page last modified March 2022).
- **Hard deals in the 1999 era (Keller's article):**
  - Deal 1991636 took about 2 hours, 11,919 attempts, and 1.6M positions.
  - Deal 2844920 took about 5 minutes, 451 attempts, and 226k positions.
  - Hard deals have few initially playable cards: 8–9, against about 26 on average by our estimate (a given card matches a given other card with probability 15/51).

### 3.2 Blake & Gent, "The Winnability of Klondike Solitaire and Many Other Patience Games" (JAIR 85, article 21, February 2026, doi:10.1613/jair.1.17167)

- **Solvitaire** is a general solver. Accordion is a special-cased rule set: no foundations, moves L1/L3, build by same suit or same rank.
- **Accordion result:** 1,000,000 deals, 999,996 won, 4 undecided and none proven lost, at a mean of 9.8 CPU-seconds a deal:
  99.99948 ± 0.00052% (95% CI). As the best earlier result they list 99.9999936 ± 0.0000064%, which is the Wilson interval on
  Masten's 30,000,000 wins out of 30,000,000 (Masten's page gives only the count); theirs is about 80× wider.
- **Transferable techniques:**
  - DFS with *trailing* (make/unmake) rather than copying;
  - a transposition table keyed by the full compressed state, LRU-bounded, never evicting ancestors;
  - symmetry reduction (sorted canonical forms, dropping suits when no rule needs them);
  - dominance / safe-move pruning;
  - **streamliners** (incomplete fast searches that are accepted only when they find a win, with a complete search
    as fallback). This is the same "find wins fast" philosophy.
- **Statistics:** Wilson 95% CI; unknown (timed-out) deals are counted both as wins and as losses, and the reported interval spans both.

### 3.3 Keller, "How I learned to play (the) Accordion" (Solitaire Laboratory)

The page (last edited June 2024) knows of no random deal shown impossible, and credits Jason Crupper with showing that
impossible deals can be constructed, namely deals with no move at all.

Strategy that can become heuristic features:

- **Sweepers:** keep the four cards of one rank together and uncovered, then use them to sweep each suit.
- **Landing strips:** runs of the same suit that later leaps can land on.
- **Suit reduction:** eliminate a problem suit down to one card, then cover it with its rank.
- **Flotsam clean-up:** condense the leftover cards.
- **Sweeper rank choice:** score each rank by how many cards of its suits lie behind it (lower is better).
- Keller's own automatic-play numbers for traditional Accordion are in §2.3.

### 3.4 Complexity

- **Accordion:** we found **no** published complexity result.
- **Related games proven NP-complete in generalized form:** Klondike (Longpré & McKenzie), FreeCell, Black Hole,
  Forty Thieves, Golf, Calculation, peg solitaire.
- **Implication:** expect worst cases to be exponential and rely on the empirical density of wins.

### 3.5 Similar problems and what to borrow

| Problem | Similarity | Borrow |
|---|---|---|
| **Peg solitaire** | Each move removes one piece; fixed solution length; DAG | Transposition tables; invariant-based dead-state pruning (pagoda functions are the analogue of our connectivity test); bidirectional search (works there because moves are reversible, which Accordion's are not, §5.4) |
| **Birds of a Feather** (Neller et al.) | Merge piles on suit/rank-style matches | DFS solvers and an MCTS player (AAAI) |
| **Black Hole / Golf** (black-hole-solver) | Match-based single-row play | Compact state, fast DFS, solver-file conventions |
| **FreeCell / Klondike solvers** | Huge state spaces, nearly all winnable | Lossy TT, safe moves, streamlining, heuristic ordering |
| **SameGame, Morpion Solitaire** | Single-player combinatorial puzzles | **Nested Monte Carlo Search** (Cazenave 2009); **Nested Rollout Policy Adaptation** (Rosin 2011), which learns a per-instance policy for the random phase |
| **SAT/CSP randomized search** | Heavy-tailed runtime distributions | **Randomized restarts** (Gomes, Selman & Kautz 1998); **Luby universal restart schedule** (Luby, Sinclair & Zuckerman 1993), within O(log) of the best fixed cut-off for an unknown distribution |
| **Klondike rollouts** (Yan, Diaconis et al. 2004) | Policy improvement by simulation | Rollout-based move choice for the prefix |
| **Chess engines** | Transposition tables at extreme rates | Zobrist hashing, bucketed tagged entries, prefetching, epoch-based clearing |

---

## 4. Structural properties (the analytic basis for the solver)

1. **Only top cards matter.** A state is the sequence of top cards: *n* distinct cards out of 52. What lies under
   a top card never affects legality again (it matters only to the visualizer).
2. **Fixed depth, no cycles.** Each move removes one pile, so a state with *n* piles is at depth 52−*n*,
   every win is 51 moves, and the state graph is a layered DAG. DFS needs no cycle handling, and "dead" results are
   permanent facts about a state.
3. **Locality and commutation.** A move at position *i* touches only positions *i−3…i*. Moves far apart commute,
   which produces enormous numbers of transpositions (same state, different move order). This is why a
   transposition table is essential. Partial-order reduction (sleep sets) can skip many of them before the table is even probed.
4. **Relabeling symmetry.** Legality uses only *equality* of suit and *equality* of rank. Solvability is therefore
   invariant under any permutation of the 4 suits and any permutation of the 13 ranks. A state's fate depends
   only on its **equality pattern**: relabel suits and ranks in order of first appearance. That knowledge
   is **independent of the deal**, so all games can share one endgame database.
   - **Coarser still:** the fate depends only on the ordered **match matrix**, meaning which piles match which. This also merges suit↔rank swaps.
   - **Label-free winning moves:** a winning move written as (position, distance) is label-free, so one cached answer serves every equivalent position.
   - Measured value: §5.11.
5. **Connectivity is a necessary condition.** Build the match graph on the current top cards (an edge means same suit or rank).
   - If it has two or more components, the deal is lost.
   - A card can only cover, or be covered by, a card in its own component. Components never merge, and the last card of any component can never be covered or moved.
   - So a winnable state has a connected match graph.
   - Equivalently, the bipartite suit–rank graph whose edges are the present cards must be connected. That is a few bit operations on a 4×13 mask (§6.5).
   - Orphans (a card with no remaining suit-mate and no remaining rank-mate) are the cheapest special case.
6. **Wins are cheap to verify.** Replaying 51 moves takes microseconds. Any approximate machinery (lossy
   hashing, heuristic pruning, parallel races) can only *miss* wins, never fabricate one. That frees the design.

---

## 5. Brute-force feasibility — measurements

### 5.1 Breadth from a full deal (probe `probe.py width`)

Distinct states after *d* moves from one random deal:

| *d* | 1 | 2 | 3 | 4 | 5 |
|---|---|---|---|---|---|
| states | 23 | 270 | 2,176 | 13,592 | 70,250 |

Growth is about 5× per move early on. With 51 levels, enumerating the whole game is hopeless.

### 5.2 Everything reachable from a *k*-pile row (probe `probe.py reach`, `reach2.py`)

The rows here are the first *k* cards of a shuffled deck.

| piles *k* | 10 | 12 | 14 | 16 | 18 | 20 | 22 | 24 |
|---|---|---|---|---|---|---|---|---|
| reachable states (samples) | 30–337 | 59–4.2k | 1.6k–12.9k | 0.3k–10.7k | 3.1k–14.7k | 87k–116k | 954k | 1.53M |

**Interpretation:**

- Exhaustively searching a residual of about 18–24 piles costs 10^4–10^6 states. At Rust speeds that is microseconds to tens of milliseconds.
- Growth is roughly 2–3× per extra pile in this range. Extrapolated to 52 piles that is around 10^17 states, so
  proving a loss on a full deal is generally out of reach. This agrees with Masten's experience.
- **Caveat:** residuals that arise in real play are not uniform random rows. Phase 5 measures the real residual distribution.

### 5.3 Endgame patterns (probe `patterns.py`)

These are exact counts of canonical equality patterns (§4.4) of length *k*, computed with a DP over
*(length, suits used, ranks used)*. Each next card is one of: an unused (old suit, old rank) pair, an old suit
with a new rank, a new suit with an old rank, or both new.

| *k* | 6 | 7 | 8 | 9 | 10 | 11 | 12 | 13 |
|---|---|---|---|---|---|---|---|---|
| patterns | 12,657 | 162,863 | 2.28M | 34.3M | 547M | 9.21B | 163B | 3.0T |
| 1-bit table | — | — | 0.3 MB | 4.3 MB | 68 MB | 1.15 GB | 20 GB | 378 GB |

A complete tablebase is practical through *k*=10 (68 MB), or 11 (1.15 GB) if it pays off.
Beyond that, use a lazily filled shared cache. Raw sequences at *k*=10 number 5.7×10^16, so the
symmetry reduction is about 10^8×.

### 5.4 Is a two-sided (bidirectional) search possible?

- **Literal backward search from the goal does not work.** An un-move must re-insert a covered card, and any of the
  52−*n* off-row cards that matches is a candidate, at any position. The backward frontier must also land on the
  *exact* dealt permutation, which nothing constrains locally. Backward branching is roughly *n*·(52−*n*) per level with
  almost no pruning.
- **The tablebase is the backward half.** Built retrograde from 1-pile patterns, it is
  deal-independent and amortized over every game. Forward search meets it at residual length *K*.
- **Middle layers:** a symmetry-keyed endgame cache for 11–16 piles extends the meeting zone as the solver runs.
  It is per thread, because cross-deal reuse measured only 2–3% (§5.11).

### 5.5 Random prefix vs brute force (probe `tradeoff.py`)

**Setup:** 24 deals × 20 attempts per *k*. Each attempt plays *k* uniformly random moves, skipping children that
fail the connectivity test, then runs randomized first-win DFS with connectivity pruning on the residual.
The budget is 200k nodes.

| prefix *k* | residual piles | P(win)/attempt | P(refuted) | P(budget-out) | nodes/attempt | **nodes per win** | deals won (of 24) |
|---|---|---|---|---|---|---|---|
| 0 | 52 | 0.85 | 0.00 | 0.15 | 60,454 | 71,123 | 24 |
| 10 | 42 | 0.86 | 0.00 | 0.14 | 59,805 | 69,340 | 24 |
| 20 | 32 | 0.83 | 0.01 | 0.16 | 63,555 | 76,266 | 24 |
| 26 | 26 | 0.73 | 0.17 | 0.09 | 43,451 | 59,252 | 24 |
| 30 | 22 | 0.40 | 0.60 | 0.00 | 13,031 | 32,577 | 24 |
| **34** | **18** | 0.10 | 0.90 | 0.00 | 1,251 | **12,256** | 17 |
| 38 | 14 | 0.02 | 0.98 | 0.00 | 91 | 5,458 | 7 |
| 42 | 10 | 0.00 | 1.00 | 0.00 | 44 | ∞ | 0 |

**What it shows:**

- Up to about 20 random moves cost almost nothing in winnability. Uniformly random early play rarely ruins a deal, which
  is why Masten's method works.
- **Cost model:** for independent attempts, expected nodes to the first win are `T(k) = E[nodes/attempt] / P(win/attempt)`.
  Pooled over all deals, nodes per win is lowest around *k*≈34–38, because refuting an 18-pile residual takes only about 1.2k nodes.
  That pooled figure is misleading, though: it hides deals that can never win at that depth. §5.6 does the per-deal analysis.
- **Per-deal variance is large.** At *k*=34, 7 of 24 deals had no win in 20 tries, and at *k*=38, 17 of 24 had none.
  A fixed deep *k* would stall on those deals. Hence the escalation ladder:
  - start deep and cheap;
  - after *m* failures, move the switch point earlier (bigger brute force) and raise the budget.
- **Failure types carry information.** "Refuted" means the residual was too small or the prefix bad, so move *k* earlier.
  "Budget-out" means the residual is too large for *B*, so raise *B* or move *k* later. An adaptive schedule can use both.
- Smarter prefix policies raise P(win) at a given *k*, which shifts the optimum deeper and cheaper.
  Keeping dead-state caches across attempts makes repeated attempts cheaper still.

### 5.6 Restart schedules: how deep to start, how many tries before escalating (probes `schedule.py`, `schedule_eval.py`)

**Setup:**

- 40 fresh deals × 8 prefix lengths (*k* = 24…38) × 30 attempts each, with a 150k-node budget.
- From each deal's win rate *p* and mean attempt cost *c* at each *k*, we compute the **exact expected nodes to first win** of a
  schedule.
- A schedule is a list of rungs (*k*, *m*): up to *m* attempts at prefix length *k*, then move to the next rung. The last rung repeats until a win.
- Rung expected cost is `c·(1−(1−p)^m)/p`, and the chance of falling through to the next rung is `(1−p)^m`.

**Per-deal P(win per attempt):**

| *k* | residual piles | min | median | max | deals with 0/30 wins |
|---|---|---|---|---|---|
| 24 | 28 | 0.47 | 0.80 | 0.97 | 0 |
| 26 | 26 | 0.17 | 0.73 | 0.97 | 0 |
| 28 | 24 | 0.17 | 0.63 | 0.97 | 0 |
| 30 | 22 | 0.03 | 0.43 | 0.67 | 0 |
| 32 | 20 | 0.00 | 0.17 | 0.50 | 1 |
| 34 | 18 | 0.00 | 0.10 | 0.23 | 7 |
| 36 | 16 | 0.00 | 0.00 | 0.17 | 24 |
| 38 | 14 | 0.00 | 0.00 | 0.13 | 29 |

**Expected nodes to first win:**

| Schedule | mean per deal | worst deal | deals never won |
|---|---|---|---|
| fixed *k*=24 | 67,779 | 146,747 | 0 |
| fixed *k*=26 | 67,892 | 150,879 | 0 |
| **fixed *k*=28 (best fixed)** | **49,444** | 127,810 | 0 |
| fixed *k*=30 | 59,771 | 279,912 | 0 |
| fixed *k*≥32 | ∞ | ∞ | 1–29 |
| **ladder 38→24, step 2, 16 tries/rung** | **26,633** | 175,282 | 0 |
| ladder 36→24, step 4, 16 tries/rung | 26,785 | 116,395 | 0 |
| ladder 36→24, step 4, 8 tries/rung | 30,774 | **98,026** | 0 |
| ladder 32→24, step 4, 8 tries/rung | 34,203 | 96,983 | 0 |
| ladder 38→24, step 2, 4 tries/rung | 34,330 | 138,920 | 0 |

**Conclusions:**

- **An escalation ladder beats every fixed switch point.** It is about 1.9× cheaper on average than the best fixed *k* and never gets
  stuck. Start deep, with several cheap attempts, then grow the brute-force portion step by step.
- **Deep rungs are cheap lottery tickets.** An attempt at *k*=36–38 costs about 100–1,000 nodes, so 16 of them cost less than one
  attempt at *k*=24. They pay off on the many deals whose deep residuals are winnable.
- **Tries per rung and step size trade mean against worst case.** More tries per rung give a better mean; bigger steps with fewer tries give a
  better worst case. Tuning this is a Phase 5 job.
- **Use per-deal expected cost as the objective.** Pooled nodes-per-win (§5.5) overstates deep *k*.
- **Caveats:** 40 deals and 30 attempts each mean noisy *p* (±0.1–0.18). Node counts come from Python. Prefixes are uniform random, and the dead-state cache was
  not reused across attempts. Each of these will shift the optimum, so the real schedule is tuned in Phase 5 on thousands of
  deals with the Rust engine.

### 5.7 Move ordering and restarts (probe `order.py`)

50 deals, 200k-node budget per deal:

| Strategy | Wins | Budget-outs | Median nodes (wins) | Total nodes |
|---|---|---|---|---|
| DFS, moves left→right | 43 | 7 | 6,800 | 2.57M |
| DFS, moves right→left | 35 | 15 | 36,464 | 4.91M |
| **DFS, random order + Luby restarts** (unit 500 nodes) | **49** | **1** | 16,307 | **1.45M** |

Ordering matters a lot (35 vs 43 wins), and **randomization plus restarts beats any fixed order**. This is the
heavy-tailed signature: most deals are easy, and a deterministic search occasionally commits to a huge dead subtree.

### 5.8 Cheap dead-end tests (probe `deadends.py`)

A dead-end test is a cheap check that proves a position can no longer be won. It must be **sound**: it may miss
dead positions, but it must never flag a winnable one, or the solver would throw wins away.

**Tests and why each is sound** (*n* = piles; the set of top cards only ever shrinks, so match counts only fall):

| Test | Condition (*n* ≥ 2) | Why it proves a loss | Cost in the Rust engine |
|---|---|---|---|
| **Orphan** | Some top card shares neither suit nor rank with any other top card | It can never move and can never be covered, and nothing can merge into it | O(1) incremental: per-suit and per-rank counts; after a card is covered, re-check only that suit's and rank's survivors (~10 ops) |
| **Disconnected** | The match graph of top cards has ≥2 components | A card only covers or is covered within its own component. Components never merge, and each keeps at least one card forever (§4.5) | Fixed-point closure on the 4×13 `present` mask, ≤4 rounds (~30 ops) |
| **Left-leaf pair** | Two cards each have exactly one match, and that match lies to their left | See below | Degree-1 cards from the same counts (`suit_count + rank_count == 3`), then partner lookup and a position compare (~20–60 ops) |

**Why the left-leaf pair is a loss.** Let card *x* have exactly one match *y*, with *y* to its left.

- *x* can only move onto *y*, which covers *y*. *x* can never jump past *y*: a leap needs a match 3 to the left, and *y* is its only match.
- So *y* stays left of *x* and can never cover it, and no other card matches *x*. *x* is therefore never covered, which makes it the final card.
- Two such cards cannot both be final, so the position is dead.
- A corollary: *x* covering *y* leaves *x* orphaned, so that must be the game's very last move.

Each of these tests is a special case of the rule that a card must eventually be covered by a match approaching from its right.
A related fact: **the leftmost card can never be the final card**, because the final card made the last move and the leftmost
card can never move.

**Connected without the leftmost card** (added after Phase 8, now on by default). Pile 0's top card never moves, so it never
covers anything, and every other top card can only be covered by a matching card that moves. So the other top cards must
be connected among themselves, and pile 0's card must match one of them; plain connectivity lets pile 0's card link groups
that can never meet (`AH 2H AS`: 2H can only slide onto AH, stranding AS). At the root of a 16-pile ladder residual it cuts
another 6.9% of residuals (plain connectivity cuts 7.6%), and inside the searches it removes 17% of positions; the solver
is about 30% faster on the CPU and 50% on the GPU (docs/BENCHMARKS.md, after Phase 8).

**Soundness check.** 628 residual positions were labelled by DFS with no pruning, at random-prefix depths *k* = 26, 30, 34.

- No test fired on any winnable position, including every position along every winning line found.
- Every pruning set returned exactly the same answers as plain DFS.

**Value:**

| *k* (residual piles) | Root detection, all tests | Refutation nodes vs no pruning: orphan / +connectivity / +left-leaf | First-win nodes vs no pruning: orphan / +connectivity / +left-leaf |
|---|---|---|---|
| 26 (26) | 0% | 0.73× / 0.30× / 0.30× | 0.66× / 0.28× / 0.27× |
| 30 (22) | 0% | 0.67× / 0.24× / 0.23× | 0.63× / 0.22× / 0.21× |
| 34 (18) | 0% | 0.57× / 0.16× / 0.14× | 0.54× / 0.18× / 0.17× |

With all three tests on, the share of refutation nodes each one cuts off is: connectivity 17–26%, orphan 2–5%, left-leaf 1–2%.

**What this means:**

- **Inside the brute force the tests are a big win:** 3.3–7× fewer nodes. Connectivity does most of the work, and left-leaf adds 5–10% more
  at deep *k*.
- **Where to run them matters** (§5.10). They almost never fire until few piles are left, so they are gated by piles left and triggered by
  events, not run at every node.
- **The 0% root detection here is an artifact.** These residuals came from random moves that were already filtered by the tests.
  §5.10 shows that filtering is not worth its cost, so the end-of-random-moves check happens at the brute force's first node instead.
- **Losing positions that pass all three tests are connected but positionally stuck.** Only search, or an exact table, finds those.
- **Further candidate:** positional tests built on "the leftmost card must be covered".

### 5.9 Where brute-force nodes sit (probe `depth_profile.py`)

These are the shares of search nodes (cheap pruning on) at positions with fewer than *K* piles. An exact lookup at *K* piles would remove them all.

| *k* | Search type | below 10 piles | below 12 | below 14 |
|---|---|---|---|---|
| 26 | refutations | 4.7% | 21.6% | 51.8% |
| 26 | first wins | 8.8% | 32.6% | 65.5% |
| 30 | refutations | 8.9% | 36.2% | 70.7% |
| 30 | first wins | 15.0% | 44.6% | 77.8% |
| 34 | refutations | 15.1% | 51.1% | 84.4% |
| 34 | first wins | 43.7% | 78.5% | 95.4% |

- The work concentrates at **10–14 piles**.
- A complete table for ≤10 piles (68 MB) removes 5–44% of nodes. ≤11 piles (1.15 GB) removes more.
- The rest of the work at 11–14 piles is where a **symmetry-keyed endgame cache** helps: 16% fewer nodes (§5.11). Reuse between
  different deals turned out small, so the cache stays per thread.

### 5.10 When to run the dead-end tests (probes `deadend_timing.py`, `event_rates.py`, `prefix_check.py`)

The tests are cheap, but running them where they almost never fire still costs time. Three measurements decide where they run.

**Hard lower bounds.** Each move covers exactly one card, so a test can't fire until enough specific cards are gone.

| Test | What must be covered first | Earliest move | Piles left at most |
|---|---|---|---|
| Orphan | The card's 12 suit-mates and 3 rank-mates | 15 | 37 |
| Disconnected | Every card in the cross blocks of a suit/rank split: min over splits of *a*(13−*b*)+(4−*a*)*b* | 15 | 37 |
| Disconnected, both parts ≥ 2 cards | Same, with each part holding at least 2 cards | 17 | 35 |
| Left-leaf pair | Two cards that cannot match each other, each down to 1 of its 15 matches (28 − 2) | 26 | 26 |

**In random play.** These are 4,000 uniform random playouts from full deals, with no tests applied. Each cell is the share of playouts where it has happened by that move:

| By move | 25 | 30 | 35 | 40 | 45 |
|---|---|---|---|---|---|
| Any test has fired | 0.0% | 0.1% | 3.8% | 43.9% | 65.2% |
| Stuck (no legal moves) | 0.7% | 3.5% | 13.5% | 35.6% | 69.3% |

The earliest firing in all 4,000 playouts was **move 26** (median 39). No random playout ever won.

**Inside the brute force.** This uses *k*=30, so brute force starts at 22 piles; *k*=26 and 34 look the same. Tests run in the order orphan, connectivity, left-leaf, and each
column counts the nodes where that test fired first.

| Piles left | Share of nodes | Orphan fires | Connectivity fires | Left-leaf pair fires |
|---|---|---|---|---|
| 20–22 | 0.1% | 0% | 0.09% | 0% |
| 17–19 | 3.1% | 0% | 1.3% | 0% |
| 14–16 | 23.0% | 0.25% | 7.4% | 0.07% |
| 11–13 | 51.9% | 2.1% | 21.7% | 1.5% |
| 8–10 | 20.0% | 8.4% | 37.7% | 4.3% |
| 2–7 | 1.8% | 20.0% | 40.6% | 4.3% |

**Event triggers.**

- **Connectivity reduces to the four suits.** Cards are all connected exactly when the 4 suits stay linked through ranks they share.
  - Covering card (*s*, *r*) can only weaken the links between suit *s* and the other suits that still hold rank *r*.
  - So a move can disconnect the position only if it removes the **last rank some pair of suits shared**. Checking that takes at most 3 ANDs on the suit rows.
  - That event covers every orphan too. Along any line of play it happens at most 6 times, once per suit pair.
- **Left-leaf pair** needs two or more cards with exactly one match. That can only appear once a suit is down to 2 or fewer cards, or a rank is.
- **Measured event rates (k=30):**

  | Event | 20–22 piles | 14–16 | 11–13 | 8–10 |
  |---|---|---|---|---|
  | "Last shared rank removed" | 10% of nodes | 17% | 30% | 50% |
  | "Two or more single-match cards" | ≤0.3% above 17 piles | 2% | 12% | — |

- The connectivity event alone is not selective at high pile counts: 10% of nodes trigger against 0.09% that fire at 20–22 piles. So it is
  combined with a pile gate.

**Filter every random move, or check once at the end?** 40 deals × 30 attempts per *k*, with the same brute force afterwards:

| *k* | P(win): filter each move | P(win): check at end | Dead at end (end mode) | Test evaluations per attempt (filter / end) |
|---|---|---|---|---|
| 26 | 0.71 | 0.71 | 0.0% | 443 / 1 |
| 30 | 0.41 | 0.38 | 0.2% | 468 / 1 |
| 34 | 0.08 | 0.10 | 1.4% | 486 / 1 |
| 36 | 0.03 | 0.03 | 5.9% | 488 / 1 |
| 38 | 0.02 | 0.01 | 20.1% | 497 / 1 |

Expected nodes per deal for each ladder:

| Ladder | Filter each move (mean / worst) | Check at end (mean / worst) |
|---|---|---|
| 38→26 step 2, 16 tries | 31.8k / 204k | **25.3k** / 227k |
| 38→26 step 2, 8 tries | 33.5k / 213k | **26.4k** / 161k |
| 36→26 step 4, 16 tries | 32.7k / 159k | 34.5k / 151k |
| 36→26 step 4, 8 tries | 35.6k / 123k | 39.1k / 142k |

The step-4 differences are within the noise of 40 deals.

**Filtering random moves doesn't pay.** It does not raise the win rate measurably and costs about 450–500 test evaluations per attempt. Checking
once at the end is as good. That check is simply the brute force's first node, and a dead attempt is abandoned at once (up to 20% of attempts at *k*=38).

**Check schedule adopted:**

| Where | Piles left | What runs |
|---|---|---|
| Random prefix | any | **Nothing** but movegen and a random pick. "No legal moves" ends the attempt |
| Brute force | ≥ 20 | No dead-end tests (they fire ≤0.1% of the time); TT only |
| Brute force | ≤ 19 | Connectivity, on entering this zone and after any "last shared rank removed" move |
| Brute force | any | Left-leaf off: measured in Rust, it saves fewer nodes than its cost |
| Brute force | ≤ 10 | One exact endgame-table lookup replaces all tests (§5.3) |

The brute force's first node follows the same rule, so it is the end-of-random-moves check.

**Rust measurements (Phase 2, docs/BENCHMARKS.md).** The schedule was confirmed and tuned with `acc bench --sweep`:

- the triggered connectivity test at ≤ 19 piles is fastest;
- checking at every node is slower;
- gating as low as 13 piles misses cuts;
- the left-leaf test cuts 5–9% of nodes but costs more time than it saves, at every depth.

### 5.11 Using the symmetry of suits and ranks (probe `symmetry.py`)

Legality only ever asks "same suit?" or "same rank?". Suits and ranks are just labels, which gives three levels of equivalence:

1. **Relabelling.** Any permutation of the 4 suits and the 13 ranks gives an equivalent position. That is 4!·13! ≈ 1.5×10^11 relabellings of every position.
2. **Coarser: the ordered match matrix.** A position's future depends only on which piles match which. This also merges suit↔rank swaps
   and other coincidences. For example, "two cards of the same suit" and "two cards of the same rank" are the same game.
3. **Label-free answers.** A winning move written as (position, distance) works for every equivalent position. One cached answer therefore
   serves them all, and the cache becomes an endgame policy rather than just a dead-state list.

**Measured.** 240 residuals of 22 piles from 60 deals. A cache for positions with ≤14 piles was shared by all of them and stored DEAD or WIN plus the move:

| Cache key | Total nodes | vs no shared cache | Entries | Hit rate | Hits from other deals |
|---|---|---|---|---|---|
| None (per-residual TT only) | 3.64M | 1.00 | — | — | — |
| Exact cards | 3.64M | 1.00 | 1.65M | 46.8% | 0% |
| Suits/ranks relabelled | 3.06M | **0.84** | 1.19M | 52.9% | 2.3% |
| Match matrix | 3.06M | **0.84** | 1.18M | 53.0% | 2.6% |

**Findings:**

- **Symmetry keys pay; exact keys don't.** Symmetry-keyed caching saves **16% of nodes** and **28% of cache memory**, while exact-card keys save nothing beyond the
  ordinary TT. The gain comes from symmetric repeats inside the same deal, where different cards play identical roles.
- **Relabel and match-matrix keys tie.** Relabelling takes one pass to compute, so it is the default.
- **Little reuse between deals.** Only 2–3% of hits came from other deals after 60 deals. The space of 11–14-pile patterns (9×10^9 to 6×10^13) is
  too large for deals to collide often.
  - So a **per-thread** symmetry-keyed cache is enough for 11–16 piles. A global shared cache is not worth its synchronization cost unless
    it proves otherwise at scale.
  - Cross-deal knowledge comes from the precomputed ≤10-pile table, where symmetry gives a 10^8× reduction (§5.3).
- **Other uses:**
  - **Canonical deal IDs:** relabel the whole deck. A user-entered deck that is a relabelled copy of a known deal is recognized and answered
    by relabelling the stored solution. It also dedupes the hard-deal registry.
  - **Symmetry-invariant heuristics:** features that depend only on the match structure behave identically for every labelling.
- **Not symmetries:** left↔right (moves only go left), and reordering positions.
- **Key cost by size:** raw Zobrist keys stay in use above about 16 piles, where symmetric repeats are rare and computing a canonical key isn't worth it.

---

## 6. Engine design for speed

**Target machine:** Intel i7-1260P (Alder Lake): 4 P-cores (8 threads) and 8 E-cores = 16 threads, 18 MB L3, 48 GB RAM.
**ISA:** AVX2 and fast BMI2 (`pext`/`pdep`, `tzcnt`); **no AVX-512**. The design must also compile to WASM (scalar or SIMD128).

### 6.1 What a card, a position and a solution are

- **Card = 1 byte:** `suit << 4 | rank`, with rank 0–12 (A…K) in the low nibble and suit 0–3 in bits 4–5.
  - The text form (`TH`) exists only at the edges: parsing and printing.
  - Padding uses the sentinel **`0x7F`**. Its rank nibble (F) and its 3-bit suit field (7) never equal a real card's, so it never matches anything.
- **Position = the top cards only, left to right.** Cards under a pile never affect play again, so the solver doesn't store them. The
  visualizer rebuilds full piles by replaying the moves from the deal.
- **Solution = the deal plus the move list.** That is 52 bytes for the deal and 51 bytes for the moves, one byte each: `pile index << 1 | leap flag`.

```rust
#[repr(C, align(64))]
struct State {
    buf: [u8; 64],   // buf[3..3+len] = top cards left→right; buf[0..3] and the tail hold the 0x7F sentinel
    len: u8,
    present: u64,    // bit (card code) set if that card is a top card: one 16-bit row per suit (13 bits used)
    l1: u64,         // bit i: pile i may slide onto pile i-1
    l3: u64,         // bit i: pile i may leap onto pile i-3
    hash: u64,       // adjacency-Zobrist (6.4)
}                    // 128 bytes = two cache lines
```

Why this layout:

- **Offset loads.** The 3 leading sentinel bytes let SIMD fetch "the card 1 to the left" and "the card 3 to the left" of every pile as plain unaligned
  loads at offsets −1 and −3, with no shuffles.
- **Card code as bit index.** `present` uses the card code itself as the bit index:
  - suit *s* is `(present >> 16s) & 0x1FFF`;
  - rank *r* is `(present >> r) & 0x0001_0001_0001_0001`.

  The connectivity test (6.5) works directly on these.
- **Match table.** `MATCH[code]` (64 × u64 = 512 B) gives each card's 15 matches as a bitmask in the same layout.
- **DFS stack.** DFS uses **copy-make** on a fixed 52-deep stack (52 × 128 B ≈ 6.6 KB, resident in L1). Each frame also keeps its not-yet-tried moves as two
  `u64` masks, so there is no recursion and no allocation. Benchmark against make/unmake.

### 6.2 Match test

Compute `x = a ^ b`. The cards match iff `(x & 0x0F) == 0` (same rank) or `(x & 0x70) == 0` (same suit). The test is branch-free, and the same
two byte compares work on many cards at once:

- AVX2: 32 cards per instruction;
- WASM SIMD128: 16 cards;
- CUDA: 4 per 32-bit word, via `__vcmpeq4`.

### 6.3 Move generation (verified by `research/probes/encoding_check.py`)

- **Legal moves are two bitmasks, `l1` and `l3`,** with no move list.
  - DFS walks the set bits with `tzcnt` and clear-lowest-bit, in randomized order.
  - The random prefix picks a uniformly random legal move in O(1): `popcnt` both masks, draw *r*, and take the *r*-th set bit with
    `pdep(1 << r, mask)` + `tzcnt` (BMI2, fast on Alder Lake).
- **Full computation** (at the deal, and in tests): load the cards at offsets 0, −1 and −3, XOR them, test both nibble masks, then `movemask`.
  The whole row is two 32-byte halves, about 25 instructions.
- **Incremental update after pile *i* moves *d* left** (the hot path):
  1. **Shift.** Bits above *i* move down one, via `m = (m & low(i)) | ((m >> 1) & !low(i))`. Every relation further right is unchanged.
  2. **Recompute only the relations the move changed**, at most 7 match tests:
     - slide (*d*=1): `l1` bit *i−1*; `l3` bits *i−1*, *i*, *i+1*;
     - leap (*d*=3): `l1` bits *i−3*, *i−2*, *i*; `l3` bits *i−3*, *i*, *i+1*, *i+2*.
  3. **Clear the impossible bits:** `l1 &= !1` and `l3 &= !0b111`. The shift can carry a stale bit into pile 2's leap slot, and the check caught exactly
     this bug.
- **Applying the move to the cards:**
  - Write the mover onto the target slot: `buf[3+i−d] = buf[3+i]`.
  - Delete slot *i* by blending the row with a copy of itself loaded one byte later, for every byte at or past *i*. Compare a constant 0..63 index
    vector against *i* to build the blend mask.
  - This is branch-free, about 6 instructions per half; `copy_within` is the scalar fallback.
- **Verified:** 20,000 random games and 817,743 moves. The incremental masks and hash matched full recomputation every time.

### 6.4 Hashing: Zobrist over adjacency pairs (O(1) incremental)

Position-based Zobrist breaks because every move shifts the indices of everything to its right. Instead:

- A sequence of distinct cards with sentinels `^` and `$` is **uniquely determined by its set of adjacent
  pairs** {(^,c₀),(c₀,c₁),…,(cₙ₋₁,$)}.
- Hash it as `XOR Z[left][right]` over that set, using a 53×53 table of random `u64` (22 KB, L1-resident). Card codes map to indices 0–51
  through a 64-byte lookup, and index 52 is the sentinel.
- **Slide** of `c` onto `t` in `… a t c d …` → `… a c d …`: XOR out `(a,t),(t,c)`, XOR in `(a,c)`. That is 3 XORs.
- **Leap** of `c` onto `t` in `… a t x y c d …` → `… a c x y d …`: XOR out `(a,t),(t,x),(y,c),(c,d)`, XOR in `(a,c),(c,x),(y,d)`.
  That is 7 XORs.
- The child's hash is known **before** the child is built, so the TT bucket can be prefetched while the
  child state is constructed, hiding DRAM latency, which is the real bottleneck.

### 6.5 Dead-end tests: event-driven and gated (§5.10)

`present` is four 13-bit suit rows.

- **Connectivity = the 4 suits stay linked through shared ranks.**
  - **Event check after covering card (*s*, *r*):** for each other suit *s′* whose row has bit *r*, test `row[s] & row[s′] == 0`. That is at most 3 ANDs.
  - **Full check, only when the event fires:** a fixed-point closure over at most 4 suits, a dozen operations.
  - Orphans are a special case and need no separate test.
- **Left-leaf pair:**
  - Track degree-1 cards from suit and rank counts: a card has degree 1 when `suit_count + rank_count == 3`.
  - Only when two or more exist, find each partner (one bit of `MATCH[c] & present`) and compare positions.
- **Gates by piles left:**
  - none at ≥20 piles;
  - connectivity on event at 17–19 piles;
  - plus left-leaf at ≤16 piles;
  - the endgame table at ≤10 piles.
- **Random-prefix moves run no tests.**

### 6.6 Transposition table (dead states only)

- Only "this state is dead" needs storing. Wins end the search, and in first-win mode, "winnable but
  unexplored" is not cached.
- **Layout:** open addressing, 64-byte buckets of 4–8 entries. Each entry holds a 16–32-bit tag and an epoch.
  - Index with the low bits of the hash and verify with the high bits.
  - Replacement: prefer the oldest epoch, then the shallowest state, since states with more piles represent bigger refuted subtrees.
- **Lossy is fine** (§4.6). A tag collision may prune a winning line, but never produces a false win.
- **Per thread:** sized to stay largely cache-resident (e.g. 4–32 MB; tune). Clear per deal by bumping the epoch,
  not `memset`. At 1,000+ deals/s, `memset` of a 16 MB table would cost 16 GB/s of bandwidth.
- **Shared across threads:** only the read-only, precomputed ≤10-pile endgame table. The symmetry-keyed cache for 11–16 piles stays per
  thread, because cross-deal reuse was only 2–3% (§5.11).

### 6.7 Parallelism: thousands of games

- Games are independent, so run **one game per task** on a rayon work-stealing pool across all 16 threads.
  That gives linear scaling, with no locks in the hot path, and adapts automatically to fast P-cores and slower E-cores.
- Within one stubborn deal, run a **portfolio**: several threads run independent randomized attempts (different
  seeds and policies). The first win wins. This combines naturally with restarts.
- **Later options, only if profiling says so:**
  - SIMD-across-games for prefix sampling (32 games in lockstep);
  - the GPU deep-rung lottery (§9). Random rollouts and small searches suit GPUs; DFS with a large TT does not.

### 6.8 Expected throughput (to be confirmed in Phase 2)

- **Per-node work** is movegen + apply + hash + connectivity: tens of instructions, about 5 ns.
- **Bottleneck:** TT probes that miss cache (~80 ns DRAM), mitigated by prefetching and cache-sized tables.
- **Target:** ≥20M nodes/s per thread and 200M+ aggregate.
- **What that implies:** the probe ladder averages about 27k nodes per deal (§5.6), which is about 1.5 ms per deal per thread at 20M nodes/s.
  That is a few thousand deals per second on this laptop before any tuning. Masten's reported rate was about 12 deals/s.

---

## 7. Language choice

| Language | Hot-loop speed | SIMD / bit intrinsics | Parallelism | Path to visualizer | Windows tooling | Verdict |
|---|---|---|---|---|---|---|
| **Rust** | C-level (LLVM) | `std::arch` AVX2/BMI2, `#[target_feature]`, const generics | rayon; data races ruled out at compile time | **First-class WASM** (wasm-bindgen / wasm-pack) | rustup + cargo | **Recommended** |
| C++20 | C-level | Best intrinsic ecosystem | std::thread / TBB / OpenMP | Emscripten (heavier) | MSVC or clang + CMake | Equal-speed alternative |
| C | C-level | Intrinsics | pthreads / OpenMP | Emscripten | As C++ | Workable, more manual |
| Zig | C-level | `@Vector`, builtins | Manual | Good WASM | Single toolchain | Promising, less mature |
| C# / .NET 8 | ~1.5–3× slower | `Vector256`, intrinsics | Good | Blazor (heavy) | Easy | Viable but GC and bounds checks |
| Go / Java | ~2–4× slower | Go: none; Java: incubator vector API | Good | Weak | Easy | Not for the hot loop |
| Python / JS | ~50–200× slower | — | — | — | — | Analysis and UI only (our probe: 220k nodes/s) |

**Why Rust over C++ here:**

- The same `accordion-core` compiles natively for the solver and to WASM for the visualizer. One rules
  implementation and one verifier everywhere.
- Fearless threading for the batch runner.
- cargo makes benchmarks (criterion), property tests (proptest) and fuzzing trivial to add.
- Raw speed is a wash. If the team is much stronger in C++, the design transfers one-to-one.

**Visualizer stack:** TypeScript + Vite in the browser, with the engine via WASM and the solver in a Web Worker.
Python with numpy/pandas/matplotlib handles experiment analysis.

---

## 8. Solver design (summary; full plan in PLAN.md Phases 4–7)

1. **Core:** first-win DFS with
   - a dead-state TT kept for the whole deal;
   - dead-end tests only where they pay: none during random moves, then gated by piles left and event-triggered inside the brute force (§5.10);
   - randomized, heuristic-biased move ordering;
   - tablebase lookups at ≤10 piles and a per-thread, symmetry-keyed endgame cache for 11–16 piles (§5.11).
2. **Outer loop (Masten-style, tuned):**
   - Each attempt is a policy-sampled prefix of *k* moves, then budgeted DFS on the residual.
   - Attempts run under a tuned **escalation ladder** or Luby schedule, possibly adaptive per deal (bandit over rungs).
   - Expected cost is `E[cost per attempt] / P(win per attempt)`. The switch point and budgets are chosen
     from measured per-deal statistics on a training set and validated on held-out deals.
3. **Prefix policy:**
   - Start with uniform random moves and no tests (§5.10).
   - Then a softmax over cheap features: mobility, connectivity margin, Keller's sweeper clustering, leap vs slide.
   - Then NRPA-style adaptation within a deal, learning from refuted attempts.
4. **Hard deals:** never dropped. Each gets a dossier and goes through re-run, portfolio, exhaustive search and a human attempt (PLAN §5).
   It is marked `loss` only after a completed exhaustive search that a second, independent run agrees with.
5. **Correctness:** every win is replayed by an independent reference engine before it counts. Results use a Wilson CI.

---

## 9. Does GPU acceleration make sense?

### 9.1 Hardware on this machine

| Device | Details | Use |
|---|---|---|
| **NVIDIA T550 Laptop GPU** | Turing TU117, compute capability 7.5, 16 SMs × 64 = **1,024 CUDA cores**, ≤2.1 GHz, **4 GB** GDDR6 on a 64-bit bus (~96 GB/s) | CUDA target |
| Intel Iris Xe (integrated) | Shares system RAM | Ignore |
| CPU for comparison | i7-1260P, 16 threads (4 P + 8 E cores), AVX2 | — |

### 9.2 What the literature says about GPU fit

| Workload type | GPU result | Source |
|---|---|---|
| Depth-first / IDA* search, memory-free heuristic | **4.98×** one optimized CPU core (15-puzzle, 1,536-core GPU). Weaker than our 16 threads; memory-heavy variants do worse | Horie & Fukunaga, *Block-Parallel IDA\* for GPUs*, SoCS 2017 |
| Monte Carlo playouts (Nested Monte Carlo Search) | Up to **420×** a single CPU core on a low-cost GPU. Variable playout lengths waste lockstep time | Buzer & Cazenave, *Monte Carlo search on the GPU* (LION) |
| Bit-vector BFS with perfect hashing (puzzle state spaces, Peg Solitaire family) | Up to **27×** a single CPU core | Edelkamp, Sulewski & Yücel, *Perfect Hashing for State Space Exploration on the GPU*, ICAPS 2010 |

**Summary of the literature:**

- **Good fit:** random rollouts and table-building BFS.
- **Poor fit:** deep DFS with a big transposition table. Per-thread tables don't fit, a shared table suffers contention and latency, and warps diverge on every branch.

### 9.3 Our workload, split by GPU fit (probe `gpu_split.py` on the §5.6 data)

With the CPU-optimal ladder (16 tries per rung, *k* = 38 → 24), the share of work that lands on the deep, GPU-friendly rungs is:

| GPU takes rungs with *k* ≥ | Residual piles | Deals won there | Share of all nodes | Left for the CPU |
|---|---|---|---|---|
| 36 | ≤16 | 36.0% | 15.1% | 84.9% of nodes, 64.0% of deals |
| 34 | ≤18 | 71.1% | 42.5% | 57.5% of nodes, 28.9% of deals |
| 32 | ≤20 | 93.1% | 71.0% | 29.0% of nodes, 6.9% of deals |
| 30 | ≤22 | 98.3% | 95.5% | 4.5% of nodes, 1.7% of deals |

If GPU attempts are cheap, the schedule should **buy far more deep tries**:

| Tries per deep rung (*k* = 38…32, ≤20 piles) | Deals won without any residual over 20 piles | Mean deep-rung nodes/deal |
|---|---|---|
| 16 | 93.1% | 18,915 |
| 64 | **99.1%** | 31,407 |
| 256 | **100%** (of 40) | 79,288 |

**Key point:** for nearly every deal, a GPU could find the win alone, using only small brute-force searches. It would spend somewhat more total
nodes (31k vs 27k per deal), but on far cheaper hardware nodes. The CPU would handle the roughly 1% left plus verification.

### 9.4 Do the small searches need a cache? (probe `no_tt.py`)

On a GPU, a per-attempt cache is the awkward part, so we measured randomized first-win DFS with all three dead-end tests, with and without the
dead-state cache. Setup: 180 residuals per row, 300k-node cap.

| *k* | Residual piles | Mean nodes, with cache | Mean nodes, no cache | Ratio | No-cache p99 | No-cache runs hitting the cap |
|---|---|---|---|---|---|---|
| 38 | 14 | 45 | 182 | 4.1× | 7,114 | 0% |
| 36 | 16 | 223 | 2,628 | 11.8× | 38,723 | 0% |
| 34 | 18 | 657 | ≥17,218 | ≥26× | ≥300k | 2% |
| 32 | 20 | 1,375 | ≥37,913 | ≥28× | ≥300k | **34%** |

The no-cache means leave out capped runs, so they understate the cost.

**Conclusions:**

- **A GPU kernel needs a cache from about 16 piles up.** Without one, transpositions blow up roughly 12× at 16 piles and over 28× at 20 piles,
  with heavy tails.
- **The cache can be small.** With it, a 20-pile search touches only about 1,400 states. A lossy table of 2–4k entries (8 B each, 16–32 KB) fits in one
  thread block's **shared memory** (Turing: up to 64 KB per SM). Overflow only forgets entries, which costs time but never correctness.
- **This sets the kernel's shape:** one attempt per warp or block, sharing a small table in shared memory, rather than one attempt per thread. Fewer
  attempts run at once, which is why the T550 gain needs a prototype to confirm.

### 9.5 Verdict

- **Do it later, behind a gate (PLAN Phase 5 track F, then Phase 8), not first.**
  - The CPU solver is required anyway as the reference, for hard deals, and for verification.
  - A tuned CPU solver is projected at a few thousand deals/s on this laptop, already about 100× Masten's rate.
- **The GPU's natural job is the deep-rung lottery:**
  - long random prefix with no tests;
  - small bounded DFS on ≤18–20 piles, with gated dead-end tests and a small per-attempt cache in shared memory (§9.4);
  - millions of independent attempts.

  It is also the natural place to build the *k*=11 endgame table, which at 1.15 GB fits in the T550's 4 GB.
- **Expected gain on the T550:** 1,024 cores against 16 strong CPU threads suggests roughly **2–5×** the CPU on that portion. This is an estimate and needs a prototype.
- **Larger GPUs:** an RTX 4090-class card has 16× the cores and would scale the same kernel much further. The CPU's share (~1% of deals) then becomes the limit.
- **Toolchain:** CUDA C++ kernels compiled at runtime with NVRTC and launched from Rust with `cudarc`.
  - It needs the CUDA Toolkit.
  - Portable `wgpu`/WGSL was considered and rejected for now: WGSL has no 64-bit integers, so the 52-bit masks would have to be split, and its subgroup support is limited.
- **Gate:** start only if the tuned CPU profile shows deep-rung attempts are ≥50% of time, and a ≤1-week prototype kernel gives ≥2× end-to-end deals/s.
- **Measured in Phase 5 (§10.8):** the prototype works and its wins all verify. The GPU alone matches the 16 CPU threads.
  Together they reach about 1.7× the CPU alone, short of the 2× gate, because the GPU and CPU share this laptop's cooling.
- **Measured in Phase 8 (§10.8):** built into `acc batch` anyway, the hybrid reaches a median 2.15× the CPU alone on 1M-deal
  runs, but about 1.4× sustained over 100M deals, as the CPU slows with heat and the GPU doesn't. The small per-attempt
  cache was never needed: sleep sets and a 2,000-position budget let the GPU win 99.4% of the deals it is given.

---

## 10. Phase 5 in Rust: what made the solver faster

All numbers are in docs/BENCHMARKS.md (Phase 5); every run is a line in `research/experiments/phase5/summary.jsonl`.
Position counts below are means over the 10,000 training deals and are deterministic.

### 10.1 Restarts: much shorter than Phase 4's

| Thorough-search restarts | Mean positions | p99 |
|---|---|---|
| Luby × 10,000 (Phase 4) | 38,086 | 258,350 |
| Luby × 1,000 | **27,170** | 205,860 |
| Luby × 500 | 26,204 | 205,057 |
| Geometric, ×1.5 from 10,000 | 45,037 | 356,766 |
| Never restart | 1.14M (50 deals not won at 100M) | 37.8M |

Short restarts work because the dead-position table persists across them, so each fresh random dive is cheap and
learns from the last. The optimum is broad, from 200 to 1,000.

### 10.2 The ladder: many more tries at a deep switch point than the probes suggested

An attempt plays 36 random moves (16 piles left), then a first-win search with a small budget. The probes' best ladder
used 16 tries per rung (§5.6). In Rust, with the dead table shared across attempts, more tries kept paying:

| Ladder (rungs 36→24 or 38→24, step 2) | Mean positions | p99 |
|---|---|---|
| none (Luby × 1,000 only) | 27,170 | 205,860 |
| 38 start, 16 tries, budget 2,000 | 20,935 | 205,770 |
| 38 start, 64 tries, budget 2,000 | 15,131 | 132,245 |
| **36 start, 256 tries, budget 4,000** | **13,257** | 103,336 |
| 36 start, 1,024 tries, budget 4,000 | 12,956 | 94,320 |

- The adopted ladder (36 start, 256 tries) wins 94.7% of deals on its first rung.
- One rung alone (36, 1,024 tries) wins 99.4% of deals before the thorough search starts.
- Starting at 34 or less is worse, and so is 40 or more.
- Budgets from 1,000 to 4,000 and budget growth per rung barely matter.

### 10.3 Move order: uniform random is best

Biased orders were tried inside the search and for the prefix. All lost or tied:

| Policy | Inside the search | For the prefix |
|---|---|---|
| leaps first / slides first | 13,193 / 13,169 (tie) | 14,362 / 47,651 |
| most moves left afterwards (mobility) | 13,872, and 2× the cost per position | 17,056 |

Even without the ladder, mobility ordering saved positions (20,814 against 27,170), but each position cost about 2.5× more.

After Phase 8 a soft bias did pay for the prefix: a leap 8 times as likely as a slide took 16% off the CPU's positions a
deal and made the GPU 1.25× faster. Inside the search it still ties (BENCHMARKS.md, "After Phase 8 — less random
prefixes").

### 10.4 Commutation: sleep sets

**The fact.** Two moves commute whenever the left move's pile lies strictly left of the right move's target. Both stay legal,
and either order reaches the same position: neither reads the piles the other changes, and the gap the left one closes
shifts the right one's mover and target together.

**The method.** After a node has searched move *a*, a later sibling *b* that commutes with *a* need not try *a* again. *b*
then *a* reaches a position already searched under *a* then *b*. Each frame carries its asleep moves as two bitmasks, and
the child's set is a mask and a shift (`dfs::asleep_after`).

**Why it is sound here.** The table stores only positions proven dead, and a frame is marked dead only when every move was
searched or is asleep. An asleep move is covered by an earlier sibling's completed search.

**How it was checked:**
- A tree search with sleep sets (no cache) reaches exactly the positions a plain exhaustive search reaches.
- Verdicts match plain search on 1,000,000 random residuals (`acc selftest --residuals`).

**Effect:**
- **Positions:** with the ladder, 13,257 → **9,652 (−27%)**, and p99 103,336 → 70,844. Without it, 27,170 → 18,684 (−31%).
- **Time: +29%.** The repeats it removes were mostly cheap table hits, so time gains less than positions.

### 10.5 Endgame tables

One bit per suit/rank pattern, built from the table one pile below (`acc endgame build`). The winnable counts for 1–8 piles
match an independent Python enumeration exactly (`research/probes/endgame_check.py`).

| Tables to | Patterns at the top | Winnable | Size | Build (16 threads) | Mean positions | Time vs none |
|---|---|---|---|---|---|---|
| none | — | — | — | — | 9,652 | — |
| 9 piles | 34.3M | 27.0% | 4.3 MB | 0.8 s | 8,191 | +8% |
| 10 piles | 547M | 30.7% | 68 MB | 21 s | 7,429 | +9% |
| **11 piles** | 9.21B | 35.0% | 1.15 GB | 7 min | **5,943** | **+26%** (validation: 11,600 → 14,674 deals/s) |

Time gains less than positions because each lookup computes the pattern's index, about one table step per card, and the
10- and 11-pile tables live in main memory. A win found in the table ends the search with up to 10 moves still to play;
those moves come from walking the table, and the reference engine replays the whole line as always.

A 12-pile table (163 billion patterns, 20 GB, several hours to build) was not built. Gains are already shrinking, and every
lookup into 20 GB would be a memory and TLB miss.

### 10.6 Symmetry-keyed cache across deals: does not pay

Dead positions of 12–16 piles were keyed by their exact pattern index (collision-free up to 16 piles) in a per-thread table
kept from deal to deal.

| Variant | Mean positions | Time |
|---|---|---|
| tables to 10 piles | 7,429 | — |
| 10 + pattern cache (4 to 64 MB) | 6,828–6,970 | **−39%** |
| tables to 11 piles | 5,943 | — |
| 11 + pattern cache (16 MB) | 5,718 | −41% |

- It saves only 4–8% of positions.
- Table size makes no difference, so little carries from one deal to the next, as the probe found (§5.11).
- Computing a pattern key at every 11–16-pile position costs far more than that saves.

### 10.7 Engine options

- **Probing the table before building the child** is 6% slower than prefetching and probing after: the build hides the
  memory latency.
- **Skipping the table at low pile counts** loses at every threshold.
- **The connectivity gate** at 16–26 piles is within noise of 19.
- **The left-leaf test** is still not worth its cost.

### 10.8 Parallel games: CPU threads and the GPU

**CPU.** One deal per thread scales to 4,519 deals/s on 16 threads (training deals, Phase 4 search). The ~3× per-thread
gap against one idle thread is hardware:

| Effect | Size |
|---|---|
| Hyper-threads | 1.31× per P-core |
| E-cores | 62% of a P-core |
| Shared laptop power budget | ~14% |

The new runner removed the remaining overhead: workers verify their own deals, and there is no chunk barrier. That alone
took Phase 4's 100,000-deal run from 4,110 to 4,539 deals/s.

**GPU (NVIDIA T550, `accgpu`).** One ladder attempt per thread.

- **First version: 10M positions/s.** Positions were copied through local memory and shifted a byte at a time, which
  saturated the memory bus.
- **Second version: 255M positions/s and 18,840 deals/s alone, 99.4% of deals won on the GPU.**
  - The ≤16-card residual lives in two 64-bit registers.
  - Moves are made and undone in place.
  - Legal moves are recomputed with byte-parallel bit tricks.
  - Per-depth state lives in shared memory.
  - Two rungs (38 and 36 random moves) run in one launch.
  - Two launches are kept in flight.
  - The CPU verifies every GPU win: 2.4 million lines, none failed.
- **Hybrid** (GPU and CPU on one stream of deals; the CPU verifies GPU lines and takes GPU leftovers first):

  | Run (1M deals each, rested before) | Deals/s |
  |---|---|
  | CPU only | 19,705, then 16,085 |
  | CPU + GPU | 30,675, then 30,220 |

  That is **1.56–1.88×, about 1.7×**, short of the 2× gate.
- **Why not 2×:** inside the hybrid the GPU keeps about its solo rate, but the CPU slows to about 12,800 deals/s. The T550
  at 30 W and the CPU share one cooling system. On a desktop with separate power budgets the two rates would add up.
- **Phase 8, the runner in `acc batch`** (docs/BENCHMARKS.md Phase 8): a feeder thread keeps the GPU busy from the CPU
  workers' own queue; the workers verify every GPU line, then solve the GPU's misses from the start.

  | Runs | CPU only | CPU + GPU | Ratio |
  |---|---|---|---|
  | 1M deals each, alternating with no rest (median of 3) | 13,148 deals/s | 28,282 deals/s | 2.15× (1.48–2.28×) |
  | Long runs: 20M deals (Phase 7), 100M deals (Phase 8) | 13,422 deals/s | 19,281 deals/s | about 1.4× |

  Over the long run the GPU kept its rate (214M positions/s against 228M in the short runs). The CPU's median time per
  deal more than doubled, to 1.15 ms, and the GPU's share of the deals rose from 58–65% to 82%. Heat is the likely cause;
  it wasn't measured directly.

## Sources

- [Accordion (card game) — Wikipedia](https://en.wikipedia.org/wiki/Accordion_(card_game))
- [Mark Masten — Accordion (Idle Year), Solitaire Win Rates](https://solitairewinrates.com/Accordion.html) ·
  [archived copy, Dec 2022](https://web.archive.org/web/20221208175830/https://solitairewinrates.com/Accordion.html)
- [Michael Keller — How I learned to play (the) Accordion, Solitaire Laboratory](https://solitairelaboratory.com/accordion.html)
- [Blake & Gent — The Winnability of Klondike Solitaire and Many Other Patience Games (arXiv 1906.12314, JAIR)](https://arxiv.org/abs/1906.12314)
- [Gent — CP 2024 talk slides (Late Binding Solitaire estimate)](https://sites.cs.st-andrews.ac.uk/people/ipg1/Klondike/CP2024V3.pdf)
- [CodeAbbey forum — Patience Accordion problem suggestion](https://www.codeabbey.com/index/forum_topic/9b0521aa56dc935e3a707a2107b8f08a)
- [A Monte Carlo Tree Search Player for Birds of a Feather Solitaire (AAAI)](https://ojs.aaai.org/index.php/AAAI/article/view/5036)
- [black-hole-solver (solver for Black Hole / Golf, used by KPatience)](https://github.com/OpenMandrivaAssociation/black-hole-solver)
- [Longpré & McKenzie — The complexity of Solitaire (Klondike NP-complete)](https://doi.org/10.1007/978-3-540-74456-6_18)
- Gomes, Selman & Kautz — *Boosting Combinatorial Search Through Randomization*, AAAI 1998
- Luby, Sinclair & Zuckerman — *Optimal Speedup of Las Vegas Algorithms*, Information Processing Letters, 1993
- Cazenave — *Nested Monte-Carlo Search*, IJCAI 2009 · Rosin — *Nested Rollout Policy Adaptation for Monte Carlo Tree Search*, IJCAI 2011
- [Yan, Diaconis, Rusmevichientong & Van Roy — Solitaire: Man Versus Machine, NeurIPS 2004](https://papers.neurips.cc/paper_files/paper/2004/file/48c3ec5c3a93a9e294a8a6392ccedeb4-Metadata.json)
- Zobrist — *A New Hashing Method with Application for Game Playing*, 1970
- [Horie & Fukunaga — Block-Parallel IDA* for GPUs (SoCS 2017; arXiv 1705.02843)](https://arxiv.org/abs/1705.02843)
- [Buzer & Cazenave — Monte Carlo search on the GPU (LION)](https://www.lamsade.dauphine.fr/~cazenave/papers/MonteCarloGPU_LION.pdf)
- [Edelkamp, Sulewski & Yücel — Perfect Hashing for State Space Exploration on the GPU (ICAPS 2010)](https://ojs.aaai.org/index.php/ICAPS/article/view/13414)
