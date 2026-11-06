# Benchmarks

Numbers are recorded with the machine, toolchain and code they came from. Rerun the commands to compare.

## Phase 2 — fast engine and first-win search

**Setup:**

- **Machine:** Intel i7-1260P (4 P + 8 E cores, 16 threads), 48 GB RAM, Windows 11, on mains power.
- **Toolchain:** Rust 1.99.1, `x86_64-pc-windows-msvc`, release profile (fat LTO, `target-cpu=native`: AVX2, BMI2).
- **Code:** working tree on top of commit `df9bc2f`, at the end of Phase 2.

### Correctness gates (`acc selftest`, all 16 threads)

```bash
cargo run --release -p accordion-cli -- selftest --playouts 10000000 --positions 1000000
```

| Check | Result |
|---|---|
| Lockstep random playouts, fast vs reference engine | **10,000,000 games, 409,462,215 moves, no disagreement** (199 s). Each move checks top cards, legal moves, incremental vs recomputed masks and hash, the random-move picker, both dead-end tests against naive implementations, and the disconnection trigger |
| Dead-end soundness | **1,000,000** small positions. A test fired on 291,129; every one the reference search could finish was confirmed unwinnable; 6 were too large for its 200,000-position budget. None fired on a winnable position (10 s) |
| Exhaustive counts (`cargo test`) | The fast engine reaches exactly 1 / 353 / 946 / 71 positions on the golden cases, as the reference engine and Python do |
| Portable code paths | The same tests pass when built for baseline x86-64 (no AVX2 or BMI2), the paths WebAssembly uses |

### Per-operation costs (`cargo bench -p accordion-core`, one thread)

These run over about 8,000 positions from 200 random playouts. "Time" is the median per position.

| Operation | Throughput | Time |
|---|---|---|
| `apply`: move, close the gap, incremental masks, hash and disconnection trigger | 62.7 M/s | 16.0 ns |
| `hash_after`: the child's hash only, used to prefetch its table slot | 324 M/s | 3.1 ns |
| `random_move`: popcnt + pdep | 551 M/s | 1.8 ns |
| `full_masks`, recomputed from scratch (reference only) | 101 M/s | 9.9 ns |
| `full_hash`, recomputed from scratch (reference only) | 75 M/s | 13.4 ns |
| `connected` (positions with ≤ 19 piles) | 137 M/s | 7.3 ns |
| `left_leaf_pair` (positions with ≤ 19 piles) | 99 M/s | 10.1 ns |

**Optimizations, and what each bought:**

| Change | Effect |
|---|---|
| 64×64 Zobrist table indexed straight from card bytes (no index lookup, no bounds checks) | `hash_after` 5.8 → 3.1 ns |
| AVX2 blend to close the gap, instead of a `memmove` call | `apply` 24.3 → 17.7 ns |
| Branch-free match test, mask merge and disconnection trigger | `apply` 17.7 → 16.0 ns |
| Epoch-tagged table, so a per-search clear is O(1) instead of a 4 MB wipe | Small (18-pile) searches 8.9 → 18 M nodes/s |

The plan's 5 ns target for `apply` + movegen was **not met** (16 ns). The governing target, search speed with table and tests, was
exceeded (below), so the remaining `apply` cost is left for later tuning. The likeliest gains are avoiding the 128-byte frame
copy and keeping the masks in registers across the DFS loop.

### Search speed (`acc bench`, one thread)

```bash
cargo run --release -p accordion-cli -- bench
```

**Workload:** from each of 300 seeded deals, play *k* uniform random moves, then run the first-win DFS on what is left.

- 4 MB dead-position table, cleared per search.
- 2M-position budget.
- Default gates (connectivity at ≤ 19 piles on trigger, left-leaf off).

| *k* (piles at start) | Searches | Positions | Time | **M nodes/s** | Won / dead / out of budget |
|---|---|---|---|---|---|
| 26 (26) | 298 | 24,690,855 | 918 ms | **26.9** | 237 / 60 / 1 |
| 30 (22) | 289 | 4,845,319 | 177 ms | **27.4** | 111 / 178 / 0 |
| 34 (18) | 272 | 372,551 | 13 ms | **28.3** | 23 / 249 / 0 |
| 0 (full deal, 100 deals, 5M budget) | 100 | 8,889,856 | 329 ms | **27.0** | **100 / 0 / 0** |

**Phase 2 target: ≥ 20M nodes/s per thread. Met.**

Plain first-win DFS with fixed move order and no restarts already won all 100 full deals, at about 89k positions (3.3 ms) per deal.

### When to run the dead-end tests (`acc bench --sweep`)

Same workload. Times are in ms, and lower is better. Node counts barely change across the connectivity settings, since all of them catch the same
dead ends.

| Setting | k=26 | k=30 | k=34 |
|---|---|---|---|
| No dead-end tests | 2,303 (69.2M nodes, 6 out of budget) | 758 (21.3M) | 78.8 (2.3M) |
| Every test at every node | 1,013 | 200 | 16.5 |
| Connectivity at **every node** ≤ 19, left-leaf ≤ 16 | 1,005 | 193 | 16.1 |
| Connectivity **on trigger** ≤ 19, left-leaf ≤ 16 | 958 | 182 | 16.4 |
| **Connectivity on trigger ≤ 19, left-leaf off (default)** | **871** | **168** | **14.0** |
| Connectivity on trigger ≤ 22, left-leaf off | 901 | 173 | 13.7 |
| Connectivity on trigger ≤ 52, left-leaf off | 878 | 174 | 14.2 |
| Connectivity on trigger ≤ 16, left-leaf off | 926 | 174 | 14.0 |
| Connectivity on trigger ≤ 13, left-leaf off | 988 (3 out of budget) | 198 | 15.4 |

**Findings:**

- **The connectivity test is essential.** It makes searches 2.6–5.6× faster than no tests.
- **Triggering beats checking continuously.** Running the closure only after a move that removes the last rank two suits shared is
  faster than running it at every node. The gate value itself matters little between 16 and 52, because the trigger already keeps
  checks rare high in the tree.
- **Gating too low hurts.** At 13 piles, dead ends are missed and searches grow.
- **The left-leaf test doesn't pay.** It cuts 5–9% of nodes but costs more time than it saves at every depth, so it is off by default and
  stays available as an option.

## Phase 4 — solving whole deals

Same machine and toolchain. The solver is randomized first-win DFS with Luby restarts (unit 10,000 positions); the dead-position
table persists across restarts; dead-end gates are the Phase 2 defaults.

### 100,000 deals (`acc batch`, 16 threads)

```bash
cargo run --release -p accordion-cli -- batch --from 0 --count 100000 --out results.jsonl
```

| Measure | Result |
|---|---|
| Deals | 100,000 (seeds 0–99,999), limit 50M positions each |
| Won | **100,000**, every one verified by the reference engine during the run, and again by `acc verify` on the output file |
| Win rate | **100%**, 95% CI **[99.9962%, 100%]** (Wilson) |
| Positions per deal | median 19,573 · p90 90,049 · p99 266,452 · p99.9 537,836 · max 2,527,046 |
| Time per deal (one core) | median 1.67 ms · p90 8.5 ms · p99 26 ms · p99.9 52 ms · max 229 ms |
| Throughput | **4,110 deals/s**, 156M positions/s overall (24.3 s wall) |

### Per-thread table size (10,000 deals, seeds 10,000–19,999)

| Table per thread | Deals/s | Median ms/deal |
|---|---|---|
| 2^12 buckets (256 KB) | 4,962 | 1.44 |
| 2^13 (512 KB) | 4,617 | 1.48 |
| **2^14 (1 MB), default** | 4,631 | 1.50 |
| 2^16 (4 MB) | 3,489 | 2.07 |
| 2^18 (16 MB) | 2,822 | 2.63 |

Positions per deal barely change. A typical deal needs only about 20,000 positions, so a table that stays in the core's own cache wins.
The differences among 2^12–2^14 are within run-to-run noise, so the default is 2^14, leaving room for hard deals. `acc solve`,
which puts every thread on one deal, keeps 16 MB per thread.

Earlier fix: building the per-thread search with rayon's `map_init` created a fresh table for every split of the work. A
thread-local search took 2,000 deals from 844 to 2,724 deals/s.

### Single deals (`acc solve`, 16 threads)

| Input | Result |
|---|---|
| Seed 42 | won in 0.03 s (110k positions) |
| Seed 7, typed in lowercase on stdin | won (58k positions) |
| Seed 42 after 40 golden moves (`--position`, 12 piles) | won in 11 moves (923 positions) |
| Constructed unwinnable deal (`testdata/games/unwinnable_dead_end.json`) | every line searched, no win (12,080 positions); dossier and index written |

## Phase 5 — the fastest strategy, by measurement

**Setup:**

- Same machine and toolchain; NVIDIA T550 Laptop GPU (driver 596.52, NVRTC 12.9).
- **Code:** working tree on top of commit `f88abcb`.
- Every run is a line in `research/experiments/phase5/summary.jsonl`; the configuration lists are in
  `research/experiments/phase5/grids/`.

**How to read the numbers:**

- **Positions per deal** are deterministic for a given seed and configuration, so search changes compare exactly.
- **Times** on this laptop drift by up to ±25% with heat, between a short burst state and a lower sustained state. Timings
  below come from one command with `--warmup 40` and configurations interleaved over `--repeat 3` (spread ±1–5%), unless
  marked otherwise.
- **Deal sets** (disjoint): train 200,000–209,999 for tuning; validation 300,000–309,999 for choosing; test
  1,000,000–1,099,999, run once for the report.

### Result

| Measure | Phase 4 | **Phase 5** | Gain |
|---|---|---|---|
| Validation: deals/s, 16 threads, sustained | 2,945 | **14,674** | **5.0×** |
| Validation: positions per deal, mean / p99 / max | 38,074 / 282,305 / 1,054,221 | 5,940 / 46,699 / 241,526 | 6.4× fewer |
| **Test, 100,000 deals run once:** won | 100,000 | **100,000**, all verified, 95% CI [99.9962%, 100%] | |
| Test: deals/s | 2,952 | **14,805** | **5.0×** |
| Test: ms per deal on one core, median / p99 / max | 2.59 / 40.0 / 397 | 0.47 / 8.66 / 137 | 5.5× / 4.6× / 2.9× |
| One thread pinned to a P-core, no tables (the browser solver's setting): ms per deal | 1.67 | 0.46 | 3.6× |
| `acc solve` on the 5 hardest Phase 4 deals, 16 threads | 0.03–0.04 s | 0.03 s | both instant |

On the test set, 94.6% of deals were won on the ladder's first rung and the rest on later rungs; the thorough stage was
not needed. On seeds 0–99,999, `acc batch` with the new defaults won all 100,000, every one verified and re-checked by
`acc verify`; 1 deal fell through every rung and was won by the thorough stage.

The adopted strategy (the new defaults):

```json
{"luby_unit": 1000, "ladder": {"start": 36, "end": 24, "step": 2, "tries": 256, "budget": 4000, "growth": 200},
 "search": {"sleep_sets": true, "endgame_piles": 11}}
```

- `acc batch` uses the largest endgame tables found in `endgame/`.
- `acc solve` stops at 10 piles, because loading 1.2 GB would take longer than a solve.
- The browser solver has no tables.
- Build the tables with `acc endgame build --piles 11`: 7 minutes, 1.2 GB.

### Leaderboard (training deals; positions are means)

Each block starts from the previous block's winner.

**Track B — threads** (Phase 4 search, deals/s; affinity set with `start /affinity`):

| Threads (cores) | Deals/s | Threads (cores) | Deals/s |
|---|---|---|---|
| 1 (P-core) | 593 | 8 (4 P-cores with hyper-threads) | 2,764 |
| 1 (E-core) | 370 | 8 (E-cores) | 2,494 |
| 2 (one P-core, hyper-threads) | 776 | 12 (4 P + 8 E) | 4,204 |
| 4 (one per P-core) | 2,082 | **16 (all)** | **4,519** |
| | | 24 (oversubscribed) | 4,483 |

The new runner (workers verify their own deals, no chunk barrier) took the 100,000-deal Phase 4 run from 4,110 to 4,539
deals/s. CPUs are about 98% busy; the rest of the per-thread gap is hardware (RESEARCH.md §10.8).

**Track C — search strategy:**

| Variant | Mean positions | p99 | Kept? |
|---|---|---|---|
| Phase 4 (Luby × 10,000) | 38,086 | 258,350 | — |
| Luby × 1,000 | 27,170 | 205,860 | ✅ (500: 26,204; 100: 29,133; 100,000: 72,535; never restarting: 50 deals not won) |
| + ladder 38→24, 16 tries, budget 2,000 (the probes' best) | 20,935 | 205,770 | |
| + ladder 38→24, 64 tries, budget 2,000 | 15,131 | 132,245 | |
| **+ ladder 36→24, 256 tries, budget 4,000** | **13,257** | 103,336 | ✅ |
| ladder with 1,024 tries | 12,956 | 94,320 | no gain in time |
| search order: leaps first / slides first / mobility | 13,193 / 13,169 / 13,872 | | ❌ (mobility is 2× slower) |
| prefix order: mobility / leaps first / slides first | 17,056 / 14,362 / 47,651 | | ❌ |
| **+ sleep sets** | **9,652** | 70,844 | ✅ +29% deals/s (8,949 → 11,504) |

**Track D — engine options** (ladder + sleep sets; repeat 3):

| Variant | Deals/s | Mean positions |
|---|---|---|
| **base** | **11,536** | 9,652 |
| probe the table before building the child | 10,844 | 9,652 |
| skip the table at ≤ 4 / 6 / 8 / 10 piles | 11,035 / 11,313 / 10,744 / 10,765 | 9,963 / 9,882 / 10,390 / 11,377 |
| connectivity gate 16 / 22 / 26 | 11,376 / 11,493 / 11,531 | 9,675 / 9,703 / 9,703 |
| + left-leaf at ≤ 12 | 10,064 | 9,387 |
| connectivity at every node | 10,990 | 9,652 |

**Track E — tables and symmetry** (repeat 3; the two blocks ran at different times, so compare within a block):

| Variant | Deals/s | Mean positions |
|---|---|---|
| ladder + sleep sets | 10,493 | 9,652 |
| + endgame tables to 6 / 8 / 9 / 10 piles | 10,420 / 10,453 / 11,311 / 11,461 | 9,856 / 9,155 / 8,191 / 7,429 |
| tables to 10 piles | 12,229 | 7,429 |
| **tables to 11 piles** | **13,845** | **5,943** |
| 10 + pattern cache for 12–16 piles, 4 / 16 / 64 MB, kept across deals | 7,454 / 6,801 / 6,694 | 6,828 / 6,970 / 6,916 |
| 11 + pattern cache, 16 MB | 8,197 | 5,718 |
| pattern cache alone, 16 MB | 5,120 | 8,771 |

On validation, the 11-pile tables add 26% (11,600 → 14,674 deals/s).

| Endgame table | Patterns | Winnable | Size | Build, 16 threads |
|---|---|---|---|---|
| 9 piles | 34,260,563 | 9,264,631 | 4.3 MB | 0.8 s |
| 10 piles | 546,727,587 | 167,848,803 | 68 MB | 20 s |
| 11 piles | 9,212,838,075 | 3,228,523,525 | 1.15 GB | 418 s |

**Track G — the ladder re-tuned with sleep sets and 11-pile tables:** the optimum is flat, so the setting was left as it was.

| Ladder | Mean positions |
|---|---|
| 36 start, 256 tries, budget 1,000–4,000 | 5,943–5,960 |
| 36 start, 1,024 tries | 6,002–6,034 |
| 37 start | 7,364 |
| 38 start | 9,517–10,571 |

### Track F — GPU (`accgpu`, crates/gpu)

One ladder attempt per GPU thread: a random prefix, then a first-win search of the ≤16 piles left, with sleep sets and the
connectivity test.

| Kernel / runner | Positions/s | Deals/s | Notes |
|---|---|---|---|
| v1: positions copied through local memory | 10M | 39 (512 deals) | memory-bound |
| v2: residual in registers, make/undo, shared-memory stack, 32-thread blocks | 214M | 15,992 | 99.46% won on the GPU (prefix 36, 1,024 tries) |
| v2, two rungs (38 then 36) in one launch, two launches in flight | **255M** | **18,840** | 99.44% won on the GPU; GPU 98–100% busy |

The CPU verified every GPU win: about 2.4 million lines over these runs, none failed.

**The gate: CPU and GPU together against the CPU alone.**

- **Setup:** 1M deals per run (seeds 2,000,000–2,999,999), 60 s of rest before each run, 15 CPU workers alongside the GPU
  feeder.
- **How the hybrid shares work:** the CPU verifies every GPU line, then takes GPU leftovers, then fresh deals.

```bash
target/release/accgpu --mode hybrid --cpu-threads 15 --from 2000000 --count 1000000
```

| Run | CPU only (16 threads) | CPU + GPU |
|---|---|---|
| 1 | 19,705 deals/s | 30,675 deals/s (GPU won 58%) |
| 2 | 16,085 deals/s | 30,220 deals/s |

- **Result: 1.56–1.88×, about 1.7×. The 2× gate is not met.** All 4M deals were won, with no verification failures.
- **Why:** inside the hybrid the GPU keeps about its solo rate, but the CPU slows from about 18,000 to about 12,800 deals/s.
  They share this laptop's cooling, and the CPU-only rate itself falls from 19,705 to 16,085 between runs as the laptop
  heats.

Two lessons came out of the runner:

- **Feed the GPU from a free thread.** With 16 CPU workers on 16 hardware threads, the GPU sat idle 60–90% of the time
  waiting for its feeder thread to be scheduled.
- **Hand verification to the CPU workers.** The feeder should only launch and collect.

### Correctness gates (Phase 5)

| Check | Result |
|---|---|
| `acc selftest --residuals 1000000` | **1,000,000** random residuals of 10–24 piles. Sleep sets, gated tests and endgame tables (to 11 piles) agree with plain search on every one: 149,244 won (every line replayed on the reference engine), 848,528 lost, 2,228 too large for 5M positions |
| Sleep sets | A tree search with sleep sets and no cache reaches exactly the positions of a plain exhaustive search (60 residuals). Verdicts are unchanged across 6 option sets × 40 residuals, fixed and random order |
| Endgame tables | Pattern counts match the independent Python DP for 1–12 piles. Winnable counts for 1–8 piles match an independent Python enumeration (`research/probes/endgame_check.py`). Verdicts match plain search on 3,000 random positions; won lines replay |
| Gate tests (`crates/solver/tests/gates.rs`) | Golden positions and unwinnable deals give the right verdict under the default strategy, with endgame tables, and with the Phase 4 solver. **With every ladder search starved (budget 0), the thorough stage wins all of the first 100 deals**, every line verified |
| Visualizer (`npm test`) | The in-page WASM solver wins seeded deals and a mid-game position, and reports both unwinnable golden deals as exhausted |

## Phase 8 — CPU and GPU together (`acc batch --gpu`)

**Setup:** same machine; code on top of commit `af14316`. `acc batch` and `acc experiment` now run the GPU ladder
attempts of Phase 5 track F next to the CPU solver whenever an NVIDIA GPU is found (`--gpu auto`, the default):

- **Feeder:** one thread sends the GPU batches of 4,096 deals from the same queue as the CPU workers, two launches in
  flight. It only launches and collects.
- **CPU workers:** 15 of them. Each verifies the GPU's lines before taking a new deal, and solves a deal the GPU didn't
  win from the start, with the full stage chain.
- **Kernel:** unchanged from Phase 5. It now also accepts positions of fewer than 52 piles (for the tests) and counts
  positions and attempts per deal.

**CPU alone against CPU + GPU** (1M deals per run, seeds 2,000,000–2,999,999, alternating after a 110 s warm-up;
`research/experiments/phase8/`):

| Run | CPU only (16 threads) | CPU + GPU (15 threads + GPU) | GPU's share of the wins | Ratio |
|---|---|---|---|---|
| 1 | 17,392 deals/s | 25,786 deals/s | 65% | 1.48× |
| 2 | 13,148 deals/s | 28,282 deals/s | 60% | 2.15× |
| 3 | 12,571 deals/s | 28,674 deals/s | 58% | 2.28× |
| **Median** | **13,148** | **28,282** | | **2.15×** |

- **Every deal was won in all six runs.** The GPU won 99.4% of the deals it was given, and all 1,830,604 of its lines
  verified.
- **The CPU alone slows as the laptop heats** (±18% across the three runs); CPU + GPU stays within ±5%. Each CPU-only run
  came straight after a hybrid run, which leaves the shared cooling hot, so runs 2 and 3 may flatter the GPU somewhat. Run
  1 (1.48×) came after the CPU-only warm-up.
- **Inside the hybrid** the GPU wins about 16,800 deals/s at 228M positions/s, close to its solo rate (18,971). The CPU
  workers solve about 11,400 deals/s, including the 0.6% the GPU couldn't win.
- **Per-deal costs** for the deals the CPU solved in the hybrid: mean 6,252 positions and p99 52,040, against 5,886 and
  47,088 for the CPU alone. The GPU's leftovers are harder than average.

The Phase 5 gate (≥2× end to end) measured 1.56–1.88× with the prototype runner and 60 s of rest before each run. These
runs give 1.48–2.28×. On this laptop the gain depends on how hot it is.

**Sustained, over 100M deals** (seeds 0–99,999,999, 86 minutes; docs/RESULTS.md): CPU + GPU held 18,900–19,700 deals/s
over each stretch of 10M deals after the first two minutes, 19,281 deals/s overall. That is about 1.4× the CPU alone's
13,422 deals/s over the 25-minute Phase 7 run of 20M deals, not the 2.15× of the short runs above.

- **The GPU held up:** 214M positions/s and 15,800 wins/s, against 228M and 16,800 in the short runs.
- **The CPU slowed:** 23M positions/s and 3,500 deals/s, against 56–74M and 8,900–12,000; median 1.15 ms per deal,
  against 0.47–0.61 ms. Heat is the likely cause, since the two share one cooling system, but it wasn't measured directly.
- **The GPU's share** of the deals rose to 82%, against 58–65% in the short runs.

### Correctness gates (Phase 8)

| Check | Result |
|---|---|
| Playouts (`crates/gpu/tests/kernel.rs`) | GPU random playouts: 64 from each golden deal and position, and one from each of 20,000 seeded deals. At every step the GPU's legal moves equal the reference engine's, its connectivity and link-broken tests equal the fast engine's, and undo restores the position. Both of the kernel's rules codes are covered: the row (more than 16 piles) and the registers (16 or fewer). Planted bugs in a move mask, the link test and the leap distance each make it fail |
| Golden positions | With no prefix and no budget, every GPU try is a complete search: it wins the winnable 12-pile golden position (the line replays) and never wins the 16-pile dead end. 8,192 long random prefixes never win either unwinnable 52-pile case |
| Golden and random deals | Every golden deal recorded as a win is won on the GPU, no unwinnable one is, and all lines on 4,096 random deals verify |
| Runner (`crates/cli/src/runner.rs`) | With the GPU, every deal comes back exactly once, verified; the GPU's wins match its count. Without NVRTC or a GPU, `--gpu auto` falls back to the CPU and `--gpu on` stops with the reason |

## After Phase 8 — the GPU kernel's occupancy

**Finding:** shared memory, not compute, capped how many attempts the T550 runs at once. Each attempt kept 272 bytes there
(its 64-byte row, its 48 prefix moves, and 160 bytes of search state per depth), so a multiprocessor's 64 KB held 7
blocks of 32 threads: 224 attempts, 22% of its 1,024 threads. Registers (55 a thread) would have allowed nearly all of
them. The driver's occupancy calculator gives these counts.

**Occupancy matters.** Padding each block with unused shared memory, so fewer fit (original kernel, 100,000 deals,
seeds 2,000,000–2,099,999, two alternating runs each):

| Attempts per multiprocessor | Positions/s | vs 224 |
|---|---|---|
| **224 (original)** | **258M, 254M** | 1.00 |
| 160 | 209M, 207M | 0.81 |
| 128 | 183M, 182M | 0.71 |
| 64 | 94M, 94M | 0.37 |

**Two cuts to the shared memory per attempt:**

- **First cut, 160 bytes:** the prefix's moves are no longer kept (a winning attempt replays its prefix from the same
  random numbers to write its line), and the row shares bytes with the search state, which it never needs at the same
  time.
- **Second cut, 96 bytes:** the untried moves at a depth are always its legal moves minus its asleep ones, so they
  stay in registers for the current depth and are recomputed when the search backs up.
- **Overlay bug, found and fixed:** the row was laid out byte by byte and the search state in 16-bit values, so a
  thread's search state overlaid other threads' rows. With 32-thread blocks the warp kept all its threads in one phase
  and nothing showed (every line verified); with 64 threads, warps in different phases corrupted each other's rows
  (`CUDA_ERROR_ILLEGAL_ADDRESS`). Now all of it is byte-major (byte i of thread t at i × block + t), so a thread's
  overlay only ever touches its own bytes, at the cost of two byte accesses per 16-bit value.

Same deals, three alternating runs each (means):

| Kernel | Bytes / attempt | Attempts / multiprocessor | Deals/s | vs original |
|---|---|---|---|---|
| Original | 272 | 224 | 18,474 | — |
| First cut, own row array (no overlay) | 224 | 288 | 19,921 (later block, warmer: original about 18,200) | +9% |
| First cut, byte-major overlay | 160 | 384 | 21,296 | +15% |
| Second cut, 32-thread blocks | 96 | 512 | 21,094 | +14% |
| **Second cut, 64-thread blocks (adopted)** | **96** | **640** | **21,448** | **+16%** |
| Second cut, 96-thread blocks | 96 | 672 | 21,335 | +15% |
| Second cut, separate arrays (no overlay, 20,000 deals) | 160 | 384 | about 20,000 | +8% |

The byte-major access costs about 6% (the first cut with the unsafe 16-bit overlay ran 22,560), and past about 400
attempts per multiprocessor the gain flattens. Every version won the same 99,468 of the 100,000 deals on the GPU, and
every line verified. The adopted kernel passes the GPU tests (playouts against the Rust engines, golden verdicts, lines on
random deals) and the runner's tests with the GPU.

**Heat-soaked, CPU and GPU together** (`acc batch`, 4M deals a run after a 2M-deal warm-up, fresh seeds from 302,000,000):

| Run | CPU threads | Deals/s | GPU positions/s | Deals won on the GPU |
|---|---|---|---|---|
| Normal mix 1 | 15 + GPU feeder | 22,395 | 285M | 92% |
| GPU-focused (CPU verifies and takes the GPU's misses) | 3 + GPU feeder | 20,355 | 273M | 97% |
| Normal mix 2 | 15 + GPU feeder | 21,785 | 266M | 88% |

- **The GPU doesn't gain from a quieter CPU** on this laptop: its rate drifts down with time (285M → 273M → 266M)
  whatever the CPU does. The CPU's own deals add 1,800–2,600 deals/s, so the normal mix stays the default.
- **Against Phase 8:** the 100M run held 19,281 deals/s with the old kernel; heat-soaked runs now give about 22,000.

## After Phase 8 — where the time goes, settings sweeps, and the leftmost-card test

**Where a CPU solve's time goes** (a copy of `solve` timed by phase, its positions checked equal to `solve`'s; 20,000
deals, one thread):

| Phase | Time per deal | Share |
|---|---|---|
| Searches of residuals that turn out dead | 536 µs | 75% |
| Random prefix moves (about 55 ns each) | 119 µs | 17% |
| Winning searches | 27 µs | 4% |
| Verification (record, card notation, reference replay) | 30 µs | 4% |
| Deal, fingerprint, bookkeeping | 3 µs | <1% |

A deal takes 61 attempts on average: 13 stuck in the prefix, 6 dead at once, 41 dead after a search of about 90
positions, one won. An 11-pile table lookup costs about 167 ns (66 ns for the pattern key, the rest a memory wait), about
30% of the time.

**Skipping residuals with few legal moves doesn't pay** (235,000 recorded attempts): those with 1–2 moves are 37% of
searches but 9% of search time, and hold 7% of the wins. Searching one pays when its chance of winning times the cost of
a deal (about 5,900 positions) beats its own cost, and that holds at every move count. A logistic model on residual
features (moves, frozen stretches, matching pairs) did no better than about 1.2× even in projection.

**Where the GPU's time goes** (one 38-move rung, 20,000 deals): with the search budget at 0 an attempt costs about 22 ns
of GPU time, with it about 161 ns, so the prefix (and the search's setup) is about 13% and the search about 87%. A
search position costs about 8× a prefix move.

**GPU settings** (`accgpu`, 100,000 deals, two runs each). Giving up sooner speeds the GPU and leaves more to the CPU:

| Rungs, budget | Deals/s on the GPU | Won on the GPU |
|---|---|---|
| **38 × 256, 36 × 1,024, 2,000 (default)** | **20,390–21,074** | 99.47% |
| first rung 36 / 39 / 40 / 42 moves | 17,800–18,900 / 19,700–20,100 / 19,200–20,100 / 17,500–17,700 | 99.61 / 99.44 / 99.42 / 99.40% |
| 38 × 128 or 38 × 512 first | 19,700–20,900 / 20,600–20,700 | 99.42 / 99.51% |
| second rung 36 × 512 / × 2,048 / × 4,096 | 22,100–22,600 / 19,000–19,900 / 19,200–19,500 | 98.31 / 99.83 / 99.94% |
| one rung 38 × 1,280 | 23,400–24,500 | 96.21% |
| second rung 37 × 1,024 | 22,200–23,100 | 98.49% |
| budget 1,000 / 4,000 / 8,000 | 20,800–21,100 / 20,100–20,200 / 19,800 | 99.33 / 99.52 / 99.54% |

In `acc batch` (1M deals a run, hot) the 37-move second rung looked about 5% ahead before the leftmost test; with it,
no setting beats the default (below). A deal the GPU misses costs the CPU about 4–7 ms; starting the CPU's ladder at
34 for those deals instead of 36 saves positions but not time (7.2 against 7.1 ms), and 32, 30 or fewer tries are slower.

**CPU ladder, re-checked** (`acc experiment`, train set, repeat 3): 256 tries 5,943 positions, 128 and 512 tries 6,022
each (time within noise), start 35 6,212, start 34 7,006 (23% slower). The defaults stay.

**The leftmost-card test.** Pile 0 never moves, so its top card never covers anything; every other top card can only be
covered by a matching card that moves. So the other top cards must be connected among themselves, and pile 0's card
must match one of them: stronger than plain connectivity, which lets pile 0's card link groups that can never meet
(`AH 2H AS`: 2H can only slide onto AH, stranding AS). `Position::connected_without_leftmost`, gated like the
connectivity test (≤19 piles) with two more triggers: a move onto pile 0, and a move that breaks a link among the
other cards. Now on by default (`Gates::leftmost`, `accordion_gpu::Settings::leftmost`).

| | Mean positions | p99 | Deals/s (repeat 3) |
|---|---|---|---|
| Train set, before | 5,943 | 45,853 | 12,525 ±2% |
| **Train set, leftmost test** | **4,938 (−17%)** | 38,928 | **16,136 ±4% (+29%)** |
| Validation set, before | 5,940 | 46,699 | 12,562 ±3% |
| **Validation set, leftmost test** | **4,886 (−18%)** | 37,070 | **16,711 ±5% (+33%)** |

A larger gate (24 or 30 piles) doesn't cut more. On the GPU (`accgpu`, 100,000 deals, three alternating runs) it goes from
20,092 to **30,559 deals/s (+52%)**, and the GPU wins 99.53% of its deals instead of 99.47%: an attempt costs 59 positions
instead of 82.

**Soundness:** `acc selftest --positions 1000000 --residuals 1000000` passes with the test in: on the 349,257 of
1,000,000 random positions where a test fired, the reference search confirmed every one lost (13 too large), and on
1,000,000 residuals the full search with it agrees with plain search (149,244 won and replayed, 848,528 lost, 2,228 too
large). The GPU's playouts match `connected_without_leftmost` at every step, and its golden and random-deal tests pass
with the test off and on.

**CPU and GPU together** (`acc batch`, 1M fresh deals a run after a warm-up, the order rotated each round):

| | Round 1 | Round 2 | Round 3 |
|---|---|---|---|
| Before (no leftmost test) | 27,076 | 22,448 | 21,358 |
| **Leftmost test on CPU and GPU** | **39,173** | **31,987** | **32,706** |
| + second rung 37 × 1,024 | 38,514 | 32,004 | 31,854 |

Hot (rounds 2 and 3) that is about 32,300 against 21,900 deals/s, +48%, and about 1.7× the Phase 8 100M run's 19,281.
Every deal was won and every line verified.

**Copy-free prefix on the GPU: tried, not kept.** The prefix shifts the row of cards on every move (each removed pile
moves the cards to its right one place). A version that keeps every card on a fixed slot, marks removed piles in a
64-bit mask and finds the piles 1 and 3 to the left with bit scans played exactly the same games (same wins, GPU tests
passing), and made the prefix about 30% cheaper on its own (one 38-move rung, search budget 0: 14–15 ns against
21–22 ns of GPU time per attempt). But with the search on, every variant was 13–20% slower per attempt (121–130 against
107–112 ns): inlined or not, the deal loaded from global memory or from the row, the residual built with fixed shifts,
the whole prefix out of line, the row's padding restored. Registers and occupancy were unchanged (56 against 58
registers, 640 attempts per multiprocessor). The likeliest cause is how a warp's threads line up between the prefix and
the search, which needs a profiler (Nsight Compute) to confirm. The shifting prefix stays.

## After Phase 8 — the runner's CPU time, and GPU memory and table experiments

**Where the CPU workers' time goes next to the GPU** (`acc batch` now prints it: `CPU threads ...`). Measured inside
the runner, 2M deals, cool laptop:

| CPU worker time | Share |
|---|---|
| Solving their own deals | 70.4% |
| Verifying the GPU's lines (97.5 µs a line) | 20.1% |
| Solving the GPU's misses | 7.2% |
| Handing results on, waiting for the GPU, other | 2.3% |

Without a GPU the runner costs nothing over a bare solve (26,059–26,662 deals/s against 22,590–24,694 for a timed copy
of `solve` on the same 200,000 deals). **Lean verification:** without record files, the reference engine replays each
line directly (every move legal, one pile at the end) instead of writing the record's card notation and parsing it
back; the seed and format checks the record path adds hold by construction here. A GPU line now costs 21–58 µs
instead of 50–126 µs (4–12% of worker time instead of 11–27%); a matched hot pair gave 41,295 against 38,277 deals/s
(+8%). A test checks that both paths pass a solved line and reject an illegal or short one.

**GPU, cheap fixes** (`accgpu`, 100,000 deals, two alternating runs each):

| Kernel | Deals/s on the GPU |
|---|---|
| Before | 32,282 / 31,902 |
| xorshift32 + one packed atomic per attempt | **32,723 / 32,421 (+1.5%)** |
| + `__fns` for the k-th set bit | 31,363 / 30,957 (slower than the loop) |
| `__fns` with the 64-bit generator | 29,766 / 29,498 |

The 32-bit generator is worth about 5% against the 64-bit one; `__fns` is slower than clearing bits in a loop.

**GPU, more resident attempts.** The search's moves and covered cards moved from shared memory into two 128-bit
register stacks, leaving 64 bytes of shared memory per attempt: 1,024 attempts per multiprocessor (the T550's
maximum, 58 registers each) instead of 640. Throughput stays at about 320M positions/s (32,352–32,729 deals/s with
4,096-deal batches, 33,119–33,462 with 16,384): the kernel is limited by its work per position now, not by
latency. Batches of 16,384 deals are the new default (+1.5%); in `acc batch` the CPU then waits up to about a second
for the GPU's last batches, which is about 3% of a 1M-deal run and nothing in long ones.

**The endgame table on the GPU: tried, not kept.** The rank of an 11-card pattern ported to CUDA agrees with
`endgame::rank` on 65,536 random patterns and costs about 0.5 ns of GPU time when all of a warp's threads rank
together (about 1 billion a second). A CPU imitation of GPU attempts (38 random moves, budget 2,000, no dead table)
put the table's saving at 13.7 search positions per try going to 3.0 plus 2.5 lookups. Built in (the table copied to
the GPU, positions of that many piles answered from it, lines finished by the CPU from its own table before
verification, with a test), it is slower at every size:

| GPU table | None | 11 piles (1.15 GB) | 10 (68 MB) | 9 (4.3 MB) | 8 (285 KB) |
|---|---|---|---|---|---|
| Deals/s on the GPU | 32,536 / 31,805 | 25,156 / 24,630 | 22,648 / 22,351 | 22,390 / 22,248 | 24,061 / 23,919 |

Even the 8-pile table, which fits in the GPU's 512 KB L2 cache, loses, so memory isn't the reason: in the search
only the one or two threads of a warp that reach the table's pile count rank at any moment, and the rest of the warp
waits on them. Making it pay would need the lookups gathered across a warp's threads.

**CPU and GPU together, before and after this round** (`acc batch`, 1M fresh deals a run, alternated): within the
laptop's heat drift (36,143–47,948 against 34,313–40,745 deals/s, whichever ran first in a pair winning); the GPU
went from 268–300M to 295–306M positions/s and verification's share of CPU time halved.

## After Phase 8 — proving losses (`acc prove`, and `acc batch` for the deals it doesn't win)

**The prover's set of dead positions.** It held each dead position as a heap-allocated byte string in a standard hash
set (SipHash, unbounded). Now it is an open-addressing table of whole positions, one 64-byte slot each (the position's
row, with its pile count and the engine's incremental hash in the row's spare sentinel bytes), indexed by that hash, so
nothing is hashed twice or allocated. When a position is pushed, its children's slots are prefetched. The table starts
at 256 KB and doubles at half full, up to a memory cap (8 GB by default); there it fills to three quarters, and then a
newcomer replaces the position with the fewest piles near its home, which only costs time (that position may be searched
again). The independent run also uses the leftmost-card test at every position, and keeps positions of 12–16 piles
by suit/rank pattern in a second, 8-byte-key set; positions the endgame tables answer aren't stored.

**Seed 112,249,515** (the billion-deal run's 19.4M-position loss, docs/RESULTS.md), one thread, two runs each:

| Run | Before | After | |
|---|---|---|---|
| Independent (tests, sleep sets, tables, random order) | 12.15–12.35 s, 31.8M positions | 5.87–5.94 s, 31.7M positions | 2.1× |
| Plain (rules only, fixed order) | 25.86–26.17 s, 111.0M positions | 9.74–9.94 s, 111.0M positions | 2.65× |
| Both | 38.3 s | 15.8 s | 2.4× |

The plain run still stores exactly the 19,386,305 positions the Python check counts. Each run's table reached 4 GB
(67M slots for 19.4M positions). At the new rates, about 5.6M positions/s
for the independent run and 11M/s for the plain one, the default limit of 10 billion positions a run is up to about
half an hour of one core each.

**Pattern keys for 12–16 piles** (`acc bench --prove`, 300 deals per depth, lost positions after k random moves, one
thread; totals over the positions every run proved lost):

| Random moves | Lost | By pattern to 16 piles | By cards only | Plain run |
|---|---|---|---|---|
| 22 (30 piles) | 13 | 720,151 positions, 476,185 stored, 0.13 s | 799,273, 545,765, 0.13 s | 8,700,427, 2,956,790, 0.86 s |
| 26 (26 piles) | 60 | 1,270,747, 818,901, 0.27 s | 1,518,888, 1,021,356, 0.31 s | 14,135,515, 4,891,505, 1.38 s |
| 30 (22 piles) | 178 | 809,882, 533,031, 0.14 s | 900,349, 603,317, 0.12 s | 13,909,412, 5,107,262, 1.27 s |

Pattern keys search 10–16% fewer positions and store 13–20% fewer, at about the same time (computing a pattern costs
about what it saves); on seed 112,249,515 they make no difference (0.3% fewer positions). They stay on for the memory.
After 14 or 18 random moves almost every position is won (299 of 300), and the few lost ones are tiny.

**Checks.** `acc selftest --proofs 10000`: on 10,000 random positions of 14–44 piles, the plain, independent and
small-memory runs (a 256 KB set, which forgot 1.19 billion positions between them) all agree with a plain recursive
search: 6,771 won (every line replayed), 3,082 lost with the plain run storing exactly the positions the recursive
search reached, 147 too large for it (183 s). The default 3,000 take about 40 s. The solver crate's tests check the
same on 400 positions, and that the set finds everything it holds and forgets only at its cap.

**In `acc batch`.** A deal the solver doesn't win goes to a prover thread while the run goes on: the independent run,
then, if it found no win, the plain run. A proven loss gets a `loss` record in `hard-deals/proofs/`, a win is verified
by the reference engine and counted (stage `prove`), and an undecided deal counts both ways in the interval as before.
Seeds 202,346,000–202,347,999 (2,000 deals): the run finished in 0.1 s and seed 202,346,720 was proven lost 0.05 s
later. With the solver squeezed to 300 positions a deal (`--gpu off --max-nodes 300`), the prover won all 33 deals the
solver left (within 3M positions a run); with a 2,000-position proof limit it won 8 and left 25 undecided.

## After Phase 8 — where the GPU's time goes

**Measuring it.** `accgpu --profile` compiles the kernel with its profile counters (`Settings::profile`, `-DPROFILE`;
normal builds compile them out). Each thread adds up the clock cycles it spends in each phase of an attempt, and
counts the steps of the prefix and search loops twice: once per thread, and once per warp (by the lowest thread
stepping), so that their ratio is how many of a warp's 32 threads step together. It also counts the attempts taken and
skipped, and the searches by length. The profiled build ran about 10% slower. All runs: `accgpu`, seeds 200,000–299,999,
the GPU on its own.

**Where the time went** (the kernel as it was, `attempts`; a thread waiting on others in its warp counts where it is):

| Thread time | Share |
|---|---|
| Skipping attempts whose deal was already won | 56.4% |
| Search | 22.5% |
| Taking work | 12.8% |
| Random prefix | 6.5% |
| Loading the deal, the dead-end gate, writing results | 1.7% |

- **Divergence, not work.** Of a warp's 32 threads, 1.5 took each search step together and 6.9 each prefix step.
  Each thread runs its own attempt start to finish, so the threads of a warp soon drift apart, and the warp runs
  them nearly one at a time. The skipping and taking work are mostly threads waiting their turn.
- **Skips.** 87.3% of the 128M attempt slots belong to deals already won (a deal needs 162 attempts on average, and its
  1,280 slots are spread over the launch).
- **Search lengths.** 60% of searches end within 16 positions, 24% within 64, 12% within 256; 0.4% go past 1,024. So
  making a warp's threads wait for each other at the end of every attempt would waste most of the warp too.

**The step-at-a-time kernel** (`attempts_stepwise`, now the default; `"stepwise": false` keeps the old one). Each turn
of its loop moves every thread one step through what it is doing, a prefix move or a search position. The threads
doing the same thing step together, and a thread that finishes an attempt takes the next one at once. The warp takes
attempts 32 at a time with one atomic and checks their deals' won flags with one load. Attempts it can't use yet
wait, one per thread, for the next threads that need one. An attempt does exactly what it did before (the same
random numbers for the same attempt number). Lockstep went from 1.5 to 14.4 threads of 32 in the search and from 6.9 to 16.6 in the prefix.

The warp's time per turn then split as: search 37%, prefix 30%, loading the deal 21%, finishing an attempt 6% and
taking work 6%. Loading read the deal one byte at a time, each load waiting on the one before, since the loop
stopped at the first sentinel. It now loads 13 words at once and computes the move masks in registers (both
kernels).

| GPU alone, 100,000 deals | Deals/s | Positions/s |
|---|---|---|
| `attempts`, before | 34,879 | 334M |
| `attempts`, words loaded at once | 35,789 | 343M |
| `attempts_stepwise` | 44,575 | 484M |
| **`attempts_stepwise`, words loaded at once** | **49,801–50,346** (three runs) | **533–539M** |

Both win the same 99,513 deals (99.51%), and no line failed verification.

**Tried, not kept:**
- **Rows as 32-bit words.** The prefix's move shifted the piles above the mover down one byte at a time, about 25
  of them; shifting a word at a time needs about 7. But each card read and written then costs a word read and
  write, and the mask update reads up to 8 cards: 48,154 deals/s.
- **Loading the won flag with the deal**, so that a thread starting an attempt waits on memory once, not twice:
  49,050–49,111. Other warps hide the wait already.
- **64 registers** (`__launch_bounds__` with 16 blocks a multiprocessor), so all 1,024 threads fit at once. The
  kernel takes 72 registers, so 14 blocks of 64 fit, and the launch's last two blocks a multiprocessor wait their
  turn. Capped at 64 it spilled nothing, but ran at 45,766–45,831. Launching only 14 blocks a multiprocessor:
  49,114–49,268.

**In `acc batch`** (CPU and GPU together, 1M fresh deals a run from seed 1,000,000,000, alternated):

| Pair | Step at a time | Before | |
|---|---|---|---|
| 1 (laptop cool) | 72,215 deals/s; GPU 528M positions/s, 68% of the deals | 49,239; 329M, 70% | +47% |
| 2 (warm) | 60,242; GPU 514M positions/s, 80% of the deals | 46,515; 320M, 72% | +30% |

The laptop's heat moves these runs a lot (the CPU did 115M positions/s in the first, cool run and 64–76M in the
others), so pair 2 is the fairer one. No GPU line failed verification. The GPU tests now run every attempt check on
both kernels, and also check that a deal's tries stop only once it is won.

## After Phase 8 — the GPU's bit operations

**Tools.** `accgpu --ptx <file>` writes the kernels' PTX; `accgpu --kernel <file>` compiles another kernel.cu at run
time, so a change can be timed without rebuilding (and without a new binary for Windows' Smart App Control to judge).
The profile now also says how prefixes end: 49.2% are searched, 25.9% get stuck (after 32.7 of their 36–38 moves on
average) and 24.9% leave a disconnected residual. Counting PTX instructions between markers put the attempt start at
1,340 instructions (the loader's per-card match test, unrolled 52 times), the prefix branch at 731 and the search
branch at 862. The attempt start ran on most turns of the loop, since some thread of the warp nearly always starts one.

**Kept** (`accgpu`, 100,000 deals, GPU alone; the laptop's GPU slows by up to 5% as it warms, so changes were timed
in alternation with the kernel before them):

| Change | Before | After |
|---|---|---|
| Move masks four cards at a time, in the loader (from the 13 words it loads, each against itself shifted by one or three cards with `__funnelshift_l`) and in the search (four 32-bit words, not byte tests on two 64-bit halves with a 64-bit multiply). `match4`: per byte, `(d & 0x0F) + 0x7F` and `(d & 0x70) + 0x7F` carry into bit 7 exactly when rank and suit differ; `gather4`: one 32-bit multiply moves the four flags together. A deal's cards-present mask is a constant | 50,100 deals/s | 62,500 (+25%) |
| The k-th legal move by halving on popcounts, not by clearing the lowest bit k times (a loop that made the warp wait for its largest k), for the prefix's 52-bit and the search's 16-bit masks | 62,000 | 70,000 (+13%) |
| The current depth's asleep moves in a register; shared memory only holds the depths above (written when the search goes deeper, read when it backs up) | 68,300 | 69,700 (+2%) |
| The host works out each position's move masks and cards present once (`fast.rs`, the reference for them anyway), and the kernel divides attempt numbers by the deal count with a 64-bit multiply-high by a reciprocal | 96 registers, 10 blocks a multiprocessor | 72 registers, 14 blocks; about +2% |
| The dead-end test's trigger (`breaks_link`) on all four suits at once in 16-bit lanes: the covered card's suit row copied to every lane with one multiply, "shares a rank" read from bit 15 after adding 0x7FFF | 69,600 | 70,600 (+1.5%) |

All of them give the same wins (99,513 of 100,000) and the same moves for the same random numbers; the GPU tests'
move-by-move playouts check the loader's masks, the search's masks and the link test against the Rust engines.
Together: **about 70,200 deals/s and 760M positions/s, against 50,100 and 536M at the step-at-a-time kernel's
start (1.4×)**. The old one-attempt-per-thread kernel shares the helpers and went from 35,800 to 40,300 deals/s.

**In `acc batch`** (1M fresh deals a run from seed 2,000,000,000, alternated with the previous commit's build): 86,696
against 62,668 deals/s (+38%), then 74,617 against 55,704 (+34%). The GPU did 700–757M positions/s against
489–511M and took 81–87% of the deals; no GPU line failed verification.

**Tried, not kept:**
- **Ending a prefix as soon as the cards disconnect** (exact: a disconnected position can't be won). The test fired on
  4.8M prefixes, but only 1.4 moves before their end on average, and checking after every move cost more: 43,700
  against 50,100.
- **Piles by slot.** Each pile keeps the slot it started in, a 52-bit mask says which slots still hold one, and a move
  writes the mover over its target and clears its own bit: no shifting, neighbours by one `clz` or `ffs`, and since
  slot order is pile order, exactly the same moves. But it needs more registers (95) and more arithmetic per move
  than shifting about 25 bytes: 52,000 against 62,000 at the same occupancy; capped at 80, 72 or 64 registers it
  spilled (50,500, 47,900, 30,600).
- **Shifting the prefix's row four bytes a turn** (independent loads): 61,500 against 62,000.
- **Branch-free card helpers in the search**: 68,300 against 68,100; the compiler already used selects.
- **Fewer registers for more threads**: capped at 80 or 72 registers the 96-register kernel spilled and lost (57,500,
  54,200); the 72-register kernel capped at 64 broke even (66,000 against 65,700).

- **Tries in rounds**, to cut the attempt slots of deals already won (87% of a launch's 1,280 slots a deal): each
  launch ran a round of tries on the deals not yet won, then the host read back the won flags and relaunched the
  rest, two batches alternating so that one's round ran while the other's flags came back. Each attempt kept its
  random numbers (seeded by try * deals + deal), so the wins were the same (99,513, every line verified, the GPU
  tests passing with rounds of 64). But every split was slower than one launch (alternated, 100,000 deals): rounds
  of 64 tries 58,700–58,900 against 68,100–68,200 (−14%), of 128 62,700–63,600 (−7%), of 256 65,400–65,800 (−3.6%),
  and two rounds (640 + 640, or 1,024 + 256) 64,700–65,400 against 65,700–66,100 (−1 to −2%). In the step-at-a-time
  kernel a skipped slot costs little (a warp takes 32 with one atomic and one load), while each round's end leaves
  the GPU part idle as its last searches finish, more so as the rounds shrink. Pooling fixes the small rounds: two
  pools of deals, each topped up from the next batches to a batch's worth, take turns, and every launch gives each
  deal in its pool its next round (growing 64, 128, 256, 512, then the rest), so a launch is always full and a deal
  leaves as soon as it is won. Skipped slots fell from 86% to 67%, and the GPU alone ran 71,800–72,900 deals/s
  against 68,100–70,100 (about +5%, in either order). But in `acc batch` (1M fresh deals, alternated) the gain was
  lost in the laptop's heat: +2.5% and −3.5% in two pairs where the pool ran second, on the warmer laptop. Not
  kept.

**What is left.** Each turn of the loop still runs the prefix branch and the search branch one after the other, each
with about half the warp's threads. Splitting them (prefixes in some warps or a kernel of their own, searches of the
residuals they leave in others) would let each warp run one branch; it is the next large step, and a larger change.

## After Phase 8 — less random prefixes

**The question.** An attempt's 36–38 random moves chose uniformly among the legal moves, and half its prefixes were
wasted: 25.9% got stuck (no move left with more than one pile) and 24.9% left the cards disconnected, both certain
losses. Could the random moves steer away from those dead ends, or towards positions with more moves, and win a deal
in fewer attempts? Each policy was a kernel file timed with `accgpu --kernel` (100,000 deals, seeds 200,000–299,999,
the GPU on its own, in alternation with the kernel before it). Policies can't change which deals are winnable, only
how soon an attempt finds a line; a winning attempt replays its prefix with the same random numbers to write its line.

**Kept** (`prefix_move` in kernel.cu, both attempt kernels):
- **No move into a stuck position.** A move that leaves no legal move, with more than one pile, is taken back
  (`row_unmove`) and another legal move chosen; when every legal move does that, the position itself is lost and the
  prefix ends there. A rejected move could never have led to a win, so this only spares attempts that would lose.
- **A leap is 8 times as likely as a slide** (`Settings::leap_weight`, a power of two; 1 gives every legal move the
  same chance). Weighting by a runtime argument with a division cost about 4%, so the kernel takes the weight as a
  shift.

| GPU alone, 100,000 deals | Deals/s | Deals missed |
|---|---|---|
| Uniform (before) | about 70,500 | 487 |
| No stuck moves, uniform (`leap_weight` 1) | 73,428–73,689 | |
| **No stuck moves, leaps 8:1** (the default) | **88,057–88,131** | **171** |

The leap weight, slide to leap, with stuck moves taken back (kernel files with the weights as constants):

| Slide : leap | 2 : 3 | 1 : 2 | 1 : 3 | 1 : 4 | 1 : 6 | **1 : 8** | 1 : 12 | 1 : 1000 (leaps first) |
|---|---|---|---|---|---|---|---|---|
| Deals/s | 77,500 | 81,600 | 82,700 | 85,400 | 85,500 | **89,400** | 86,100 | 85,100 |

A soft bias wins: always leaping when a leap is legal did worse, as leaps first did in the CPU's ordered search
(RESEARCH §10.3), so the prefix still needs its randomness to spread the attempts.

**In `acc batch`** (1M fresh deals a run from seed 4,000,000,000, alternated with the previous commit's build):

| Pair | Less random | Before | |
|---|---|---|---|
| 1 | 109,062 deals/s | 83,333 | +31% |
| 2 | 95,496 | 75,775 | +26% |
| 3 | 94,685 | 75,443 | +26% |
| 4 | 95,615 | 74,772 | +28% |

The GPU won 99.82% of the deals it took, against 99.51%, so fewer fall back to the CPU; no GPU line failed
verification. The GPU tests run the golden positions with the default weight on both kernels, and every line they
return, its replayed prefix included, passes the Rust verifier.

**Tried, not kept:**
- **No move that disconnects the cards** (the search's dead-end test, run on the move's result when it could have
  broken a link): −16%, and −12% when checked only once 16 or fewer piles remain. A disconnected prefix ends at the
  gate for almost nothing; the attempts this rescued went on to search residuals that were nearly always lost anyway,
  spending the search budget.
- **A floor of 2, 3 or 4 legal moves** after each prefix move (falling back to any move that leaves one): −3%, −6%
  and −5% at uniform weights, and 83,600 against 89,400 with leaps at 8:1. The GPU missed fewer deals, but the
  checks cost more than they saved.
- **Slides twice as likely as leaps:** −13%.
- **Longer prefixes** (rungs of 40 then 38 moves, not 38 then 36): 69,700.
- **One rung** of 38 moves for all 1,280 tries: 91,000 deals/s on the GPU alone, but 1,646 deals missed per 100,000
  against 171, each of them left to the much slower CPU.

**Considered, not tried:**
- **Choosing moves by how many moves they leave** (each legal move weighted by the moves after it, or always the
  move that leaves the most). The floors found that count by playing each candidate and taking it back, shifting the
  row both ways. A move changes only the links near the mover's new place and the gap it leaves, so a peek could
  count them in about 8 match tests a candidate without shifting anything; but at 5–10 candidates for each of 36–38
  moves, in a prefix already about 30% of the warp's time (profile above), it would have to save many attempts, and
  at 8:1 there are few left to save: a floor of 2 took the attempts a deal (attempts/s over deals/s) only from 130 to
  125, where at uniform weights a floor of 4 took them from 169 to 116.
- **Why leaps help** is a guess, not measured. A slide removes only the covered card, so the others keep their order
  and one new pair of neighbours forms; a leap carries a card three places left over two cards that stay, and three
  new pairs form, each a fresh chance of a match. Counting the legal moves left after leaps and after slides, or the
  stuck and disconnected prefixes at weights 1 and 8, would test it.

### The CPU ladder

The CPU's ladder (36 random moves, then a search of 4,000 positions, 256 tries a rung, rungs down to 24 moves) got the
same two options: `Ladder::leap_weight` (now 8) and `Ladder::unstuck` (the stuck move taken back; off). A weight of 1
draws exactly as before: the same positions, deal for deal. `acc experiment`, test set (100,000 deals, seeds
1,000,000–1,099,999), 16 threads, endgame tables to 11 piles; positions are the same in every run, the rates move
with the laptop's heat:

| Prefix | Mean positions | p99 positions | Won |
|---|---|---|---|
| Uniform (before) | 4,789 | 36,423 | all |
| Stuck moves taken back | 4,778 | 36,890 | all |
| Leaps 4:1 | 4,064 | 30,825 | all |
| **Leaps 8:1** (the default) | **4,034** | **30,379** | all |
| Leaps 16:1 | 4,115 | 32,334 | all |
| Leaps 8:1, stuck moves taken back | 4,009 | 30,458 | all |

Timed in five interleaved pairs after a 20-second warm-up, leaps at 8:1 ran 24,241–26,478 deals/s against
22,818–24,483, faster in every pair (uniform ran first in each, on the cooler laptop): **median 25,852 against 23,296
(+11%)**, 0.59 against 0.66 ms a deal. The validation set agreed (3,937 against 4,886 positions).

**In `acc batch`** (1M deals, seeds 5,000,000,000–5,000,999,999 every run, the GPU in front; the old prefix by
`--config '{"ladder": {"leap_weight": 1}}'`, in the order new, old, old, new, new, old): 106,156, 95,653 and 95,811
deals/s against 104,763, 98,249 and 95,762, so no change that heat doesn't hide. The GPU took 82–93% of the deals.
The CPU's own deals took 4,340–4,742 positions on average against 5,133–5,493, but a CPU 11% faster on a tenth to a
fifth of the deals is worth only 1–2% in all. Every deal was won, and no line failed verification.

**Tried, not kept:**
- **Taking back stuck moves** (`unstuck`): no fewer positions on the CPU (4,778 against 4,789; with leaps at 8:1,
  4,009 against 4,034, inside the spread of the weights around it), so not worth checking each move. Left as an
  option, off; on the GPU it paid about 4%.
- **Leaps weighted in the search's own move order** (weights 2, 4 and 8, with leaps at 8:1 in the prefix): 4,064, 4,043
  and 4,008 positions against 4,034, a tie, as leaps first was in Phase 5 (Track C). Taken back out; the search
  passes a weight of 1.

**Considered, not tried:**
- **Re-tuning the ladder** (start, tries, budget) around the new prefix. Phase 5 found the optimum flat, and with the
  GPU in front the CPU solves only a tenth to a fifth of the deals, so even a large CPU gain barely moves `acc batch`.
- **Why taking back stuck moves pays on the GPU but not on the CPU** is not known. Counting the attempts each deal
  needs with and without it, on both, would show whether it saves attempts on the CPU at all or only time on the GPU.

## After Phase 8 — a truer breakdown, and the dead-end test without a loop

**A truer breakdown.** The profiled build (`accgpu --profile`) now runs at half the normal speed. Its counters, some
of them indexed at run time, take it to 80 registers with 184 bytes of local memory, and they sit in the short
phases, which it overstates two to three times (taking work 15.2%, loading 15.8%, writing results 11.5%). A build
with only the phase timers ran 9% slow, and split the time: **search 45.9%, prefix 37.6%**, taking work 6.1%, loading
the deal 5.4%, writing results 5.1%. Its "threads busy 74%" divides by the 16 blocks a multiprocessor launched, of
which 12 fitted; it isn't idle time. A deal took 129 attempts, an attempt 69 positions (37 prefix moves, 32 search
positions); 15.8 and 14.8 threads of 32 stepped together in the search and the prefix; 57.5% of prefixes were
searched, 32.5% left disconnected cards and 10.0% ended above 16 piles (every move lost); half the searches ended
within 15 positions.

**Doubling ablations.** Each kernel did one part twice (with an input the compiler can't see is the same), giving the
same moves and wins; the slowdown is that part's cost (`accgpu`, 100,000 deals, alternated with the kernel before):

| Done twice | Cost |
|---|---|
| Each prefix move (played, taken back, played again) | +30% |
| The search's dead-end test (`connected_without_leftmost`) | +18% |
| The search's move masks | +5.5% |
| Loading the deal | +4% |
| The per-attempt statistics atomic (left out instead) | −1% |

**The dead-end test without a loop.** `connected` walked the suit graph a suit at a time until nothing changed, one
to three turns depending on the cards, so a warp waited for its slowest thread whenever any of them needed the test.
It now takes three fixed steps on all four suits at once, in 16-bit lanes as `breaks_link` does: the suits holding a
rank of a set are bit 15 of each lane after `(present & ranks × 0x0001000100010001) + 0x7FFF…`, and the ranks of the
flagged suits are their lanes masked by `flags − (flags >> 15)`, folded together. From the lowest suit present,
three steps reach every suit linked to it. It agreed with the loop on 2.8M card sets in Python (every way four suits
can share three ranks among them, and random sets), and the GPU tests' playouts check it against the Rust engines.

| Kernel (against the kernel before, in the same runs) | Registers, blocks a multiprocessor | Change |
|---|---|---|
| The old loop, forced inline | 72, 14 | −1% |
| New test, only where the trigger fired | 80, 12 | +1.5% |
| New test, every thread every search step | 79, 12 | +4% to +6% |
| **Both, capped at 72 registers** (`__launch_bounds__(BLOCK, 14)`) | **72, 14** | **+12%** |

Left to itself the compiler kept the test's 64-bit constants in registers and lost two blocks; capped, it fits 72
without spilling. **Kept: 101,474–101,811 against 90,369–90,967 deals/s, 915–918M positions/s against 814–820M**, the
same 99,829 deals won.

**In `acc batch`** (1M deals, seeds 6,000,000,000–6,000,999,999 every run, alternated with the previous commit's
build): 117,513 against 104,218 (+13%), 104,178 against 100,505 (+4%, the old build first), 108,071 against 96,471
(+12%). The GPU did 862–903M positions/s against 801–807M in every run; the laptop's heat took the CPU from 82M to
34M positions/s over the six runs. Every deal was won, and no line failed verification.

**Tried, not kept:**
- **Testing only where the trigger fired**, at 72 registers: 98,300–99,300 against 102,800–103,600. The test now
  costs less than the branch around it.
- **64 registers for 16 blocks a multiprocessor:** 8 bytes spilled, 95,200–95,600 against 100,400–100,600.
- **Settings, re-swept** (one run each, against 93,020 and 91,426 for the defaults): budget 1,000 95,040 (201 deals
  missed, against 171), 1,500 93,689, 3,000 91,403, 4,000 90,072; rungs of 38 × 512 and 36 × 768 89,867, of 38 × 128
  and 36 × 1,152 94,323 (150 missed). Inside the noise, so unchanged; an alternated run would settle the two that
  looked faster.

**Considered, not tried:**
- **Playing several prefix moves at once.** Two moves commute when the left one's pile is left of the right one's
  target (RESEARCH §10.4): neither touches the other's cards, both stay legal, and either order gives the same
  position. So a batch of them can be played with one pass that closes all their gaps, instead of a shift per move.
  Simulated on 3,000 random deals with leaps at 8:1 (scratch script: each batch drawn by the policy, a move at a time,
  keeping those that commute with the moves already drawn), a 36-move prefix took 6.7 batches of 5.3 moves and
  shifted 26% of the bytes; capped at 4 moves a batch, 10 batches and 43%. The prefix is 38% of the time and its
  shifting most of that, so this is next. What it does to the attempts a deal needs has to be measured: a batch can't
  use the moves its own moves create.
- **The same in the search.** Sleep sets already skip the second order of every commuting pair; a search move is a
  few register operations, not a shift in shared memory; and a first-win search has to visit the positions in between
  (a win may go A then C, not A then B). Little to gain.
- **Shifting whichever side of the gap is shorter:** about halves the longest shift; batches would save more.
- **A residual queue in each warp**, so that the prefix and the search each run with all 32 threads (each has about
  half now): the largest ceiling and the largest change, best tried after batches change the balance.
- **The same branch-free test on the CPU** (fast.rs `connected` is the same loop): the CPU solves a tenth of the deals.
- **A lighter `--profile`:** the timers alone measure well; the counters could become an option of their own.

## After Phase 8 — batched prefix moves, and one branch a turn (tried, not kept)

Both ideas aimed at the prefix and the lockstep (`accgpu --kernel` files, 100,000 deals, against the kernel at
c255fd6 in the same runs, 98,000–105,000 deals/s; every variant's lines verified).

**What a shift is worth.** After each prefix move, shifting the bytes back up and down again (two more shift loops,
the same row after) ran at 73,148–73,777 against 98,614–99,156: one shift is about 17% of the time. That is the
most batching could act on.

**Batched prefix moves.** Moves commute when the left one's pile is left of the right one's target, so a turn can draw
several by the prefix's policy (each next one among the moves that commute with those drawn), put each mover on its
target, and close all the gaps in one pass. A winning line replays the same draws and writes each batch right to
left, an order in which every move is legal as written.

How far batches hurt the attempts (attempts a second over deals a second, first version):

| Moves a batch, at most | 1 (now) | 2 | 4 | 8 | no limit |
|---|---|---|---|---|---|
| Attempts a deal | 130 | 131 | 139 | 160 | 180 |
| GPU deals won | 99,829 | 99,831 | 99,720 | 99,468 | 99,028 |

So a batch of 2 costs nothing in wins and larger ones cost more and more: as suspected, the moves a batch can't see
(those its own moves create) matter. But every version was slower:

| Version | Batch of 2 | Others |
|---|---|---|
| Masks recomputed byte by byte in the closing pass | 62,146 | 4: 63,456; 8: 57,643; no limit: 51,178; single moves below 20–28 piles: 43,700–49,900 |
| Masks by dropping the gone piles' bits, then a window around each move | 67,355 | 3: 62,994; 4: 56,064; caps shrinking with the row, min(B, (piles − 16) / step): 39,872–50,043; without the register cap (80 registers, no spill): 66,210 |
| The same, windows unrolled (6 bits from each target, 9 cards loaded once) | 75,330 | single moves through the batch code: 80,837; 3: 71,988; 4: 64,532 |

Even single moves through the batch code lost 22%, and each larger batch lost more. In lockstep, a heavier prefix
step lengthens every turn, the ones where most of the warp is searching too; the shift it saves is a sixth of the
time at most.

**One branch a turn.** The warp runs both branches every turn, each with about half its threads. Letting it run only
one:
- **By majority** (the prefix when prefix threads × a ≥ search threads × b): 1:1 99,551–100,751 against
  101,880–103,653; 2:1 97,644; 3:2 97,989; 2:3 93,410; 1:2 90,723. Lockstep hardly moved (15.4 and 16.2 of 32 in the
  search and the prefix, against 15.7 and 14.7): the majority flips every few turns, so threads never gather.
- **With hysteresis** (the prefix until at most P threads are left in it, then the search until fewer than S are):
  P 0, S 8 / 12 / 16 / 20: 82,722 / 86,142 / 85,442 / 81,291; P 4, S 12: 97,260; P 2, S 8: 89,035; against
  97,666–99,037. A thread that finishes its search waits for the prefix turns instead of starting its next attempt.

What was left was to give each branch warps of its own; that is the next section.

## After Phase 8 — split warps (`attempts_split`, the default)

**The kernel.** `attempts_stepwise` runs the prefix step and the search step every turn, each with about half the
warp. `attempts_split` gives each its own warps: the first half of a block's warps play prefixes, all 32 threads in
step (a prefix's length is fixed), and put each residual worth searching (its 16 cards, the cards present, the random
state, the attempt and the prefix's length: 37 bytes) in a queue of 32 in shared memory; the other half search, each
thread taking the next residual as soon as it has finished one. An attempt plays the same moves as in
`attempts_stepwise` (the residual carries the random state on), so both win the same deals. Every slot carries a
number that says whether it is free for the next entry, written, or copied out; prefix warps reserve runs of free
entries and search warps take runs of written ones, with a compare-and-swap each. A first version counted the slots
freed instead, which let a prefix warp overwrite a slot one search warp was still copying while another had freed a
later one; and a search warp could read the tail, then a head another warp had already moved past it, and take
entries that did not exist (it hung with two search warps a block). A won search replays its prefix on its own bytes
to write the line. `"split": false` in `--gpu-config` keeps `attempts_stepwise`, and the GPU tests run all three
kernels.

`accgpu`, 100,000 deals (seeds 2,000,000–2,099,999), alternated:

| Kernel | Registers, blocks a multiprocessor | Deals/s | Positions/s |
|---|---|---|---|
| `attempts_stepwise` | 72, 14 | 99,304–100,327 | 894–905M |
| **`attempts_split`** | **72, 12** | **122,608–128,610** | **1,064–1,116M** |

Both won the same 99,808 deals with no line failing verification; in each pair the split kernel was 23% to 28%
faster (the lower figures came from a warm laptop).

**In `acc batch`** (1M deals, seeds 6,000,000,000–6,000,999,999 every run, alternated with `"split": false` in the
same build): **137,944 against 113,834 (+21%), 133,543 against 104,491 (+28%, the old kernel first), 130,586 against
104,925 (+24%)**. The GPU did 1,050–1,074M positions/s against 857–903M, and won 99.82% of the deals it was given
against 99.81%. Every deal was won, and no line failed verification.

**How many warps of each kind** (prototype kernel files; blocks of 64 hold one of each):

| Prefix : search warps (block) | Blocks a multiprocessor | Deals/s |
|---|---|---|
| **1 : 1 (64)** | **12** | **124,468–127,153** |
| 2 : 2 (128) | 6 | 123,064 |
| 3 : 3 (192) | 4 | 121,111 |
| 2 : 1 (96) | 8 | 99,209–100,520 |
| 3 : 1 (128) | 6 | 81,016–82,262 |
| 3 : 2 (160) | 5 | 117,209 |
| 4 : 3 (224) | 4 | 117,140–118,563 |
| 1 : 2 (96) | 8 | 93,123–94,776 |
| 1 : 3 (128) | 6 | 66,960–67,885 |
| 2 : 3 (160) | 5 | 109,331 |
| 3 : 4 (224) | 4 | 108,546–109,806 |

Fewer warps of either kind lose about in proportion, so the two are balanced at 1 : 1, and larger blocks lose a little
even at 1 : 1.

**Where its time goes** (`accgpu --profile`, which now times each warp's parts and runs 16% slow): prefix warps taking
work 5.9%, loading the deal 6.4%, moves 29.9%, queueing 6.8%; search warps searching 35.9%, taking residuals and
writing results 13.9%, waiting for residuals 1.1%. **25.5 of 32 threads step together in the prefix and 23.6 in the
search, against 14.8 and 15.8 in `attempts_stepwise`.** The rest wait on the queue: prefix threads have a residual it
has no room for in 20.0% of their turns, search threads have none to take in 17.3%. Searches run from a few positions
to the 2,000 budget, so the queue fills and empties by turns.

Doubling ablations in the split kernel (against it in the same runs, 125,342–130,555 deals/s):

| Done twice | Cost |
|---|---|
| Each prefix move (played, taken back, played again) | +37% |
| Two more shifts after each prefix move | +33% |
| The prefix move's mask update | +12% |
| The search's dead-end test | +5% |
| The search's move masks | +4% |

**So the prefix warps are what limits it now**: one shift is about 16% of the time and the mask update after a move
12%, while the search's dead-end test costs +5% against +18% in `attempts_stepwise`.

**Tried, not kept:**
- **Another ratio or larger blocks** (the table above).
- **A different queue.** 16 entries (13 blocks fit) 124,505 and 64 (10 blocks) 127,321, against 125,252–126,361 for
  32. Entries of 28 bytes (the prefix's length is 52 minus the residual's piles, and its cards present follow from its
  bytes), so more blocks fit: 32 entries (still 12 blocks) 125,757–127,786; 24 and 20 (13 blocks) 123,703–124,963; 16
  (14 blocks) 121,963–122,100; against 127,334–129,505. More warps don't help: the prefix warps' work does.
- **Not checking at the take whether the deal was won meanwhile:** 120,505–122,780 against 123,690–126,948; the same
  positions a second, spent on residuals of won deals.
- **Loading the won flag with the deal** in the prefix warps: 124,153–125,008 against 123,690–126,948. Other warps
  hide the wait.
- **Shifting the shorter side of the gap** (a row start per thread; the cards left of the gap move up one place and the
  start with them, in one loop whose direction each thread sets): 121,061–122,296 against 121,397–123,989 for one side
  with the same row start, which already cost a little. The step is known only at run time, so the compiler can't
  tell the next load from the last store and keeps every load waiting for the store before it; the one-sided loop's
  four loads at a time overlap. Unrolled by hand four bytes at a time (up to three cells too many): 117,576–119,258;
  both: 119,925–119,928.

**Considered, not tried:**
- **The prefix row in registers:** built later, "the prefix row in registers" below (+16%).
- **Batches of 2 in the prefix warps:** no cost in wins, and in registers two gaps cost hardly more than one to close.
  The batch code above lost 22% even on single moves, so it needs the row in registers first.
- **Warps changing role** when the queue runs full or empty: the gain is bounded by the waits above (20% and 17% of
  turns), and a warp's threads would have to finish what they hold first.

## After Phase 8 — shared early prefix moves, measured first

**The idea.** A group of G tries at a deal plays its first K prefix moves once, keeps the row, and each try then plays
its own remaining moves and searches. RESEARCH §5.5 found that up to about 20 random moves barely change an attempt's
chance of a win (85% to 83%, 24 deals in Python, uniform moves); here the question is how much the tries' sharing
those moves correlates them, so that a deal needs more of them.

**Measured exactly, without saving any work:** a kernel file (`accgpu --kernel`) gives each group of G consecutive
tries at a deal the random numbers of the group's first try for the first K prefix moves, then each try its own
random numbers for the rest of its prefix and its search; a won line replays the same way. The prefix warps still
play every move, so only the attempts a deal needs and the wins change. Seeds 2,000,000–2,099,999 (100,000 deals),
3,000,000–3,199,999 and 4,000,000–4,199,999 (200,000 each); the moves a prefix of about 37 saves are K × (1 − 1/G):

| Group G : shared moves K | Attempts a deal (each range) | More attempts | Prefix moves saved | Prefix moves a deal |
|---|---|---|---|---|
| 1 : 20 (none shared, the same random streams otherwise) | 128.4 / 128.0 / 129.1 | — | — | 100% |
| 4 : 10 | 128.8 / 129.5 / 129.2 | +0.5% | 20% | 80% |
| 2 : 20 | 129.6 / 128.9 / 130.1 | +0.8% | 27% | 74% |
| **4 : 15** | **129.6 / 129.9 / 129.6** | **+0.9%** | **30%** | **71%** |
| 16 : 10 | 131.8 / 132.1 (two ranges) | +2.7% | 25% | 77% |
| 8 : 15 | 132.2 / 131.8 (two ranges) | +2.7% | 35% | 67% |
| **4 : 20** | **131.4 / 131.4 / 131.9** | **+2.4%** | **41%** | **60%** |
| 8 : 20 | 134.8 / 134.9 / 135.4 | +5.1% | 47% | 56% |
| 4 : 25 (first range only) | 134.2 | +4.5% | 51% | 51% |
| 4 : 30 (first range only) | 144.1 | +12% | 61% | 44% |

Runs repeat to about ±0.5%. Wins held (99,780–99,827 of 100,000 against 99,808, and 199,568–199,632 of 200,000) except
at 30 shared moves (99,719). So sharing 15 to 20 moves in groups of 4 costs 1% to 2.5% more attempts and spares 30% to
40% of the prefix moves, and the prefix warps are what limits `attempts_split`.

Built later: "shared early prefix moves, built", below.

## After Phase 8 — the prefix row in registers

**The row.** `attempts_split`'s prefix threads now keep their row as 13 32-bit words in registers (card i is byte
i % 4 of word i / 4, SENTINEL after the last pile), not 64 bytes of shared memory. Registers can't be indexed at run
time, so everything is unrolled over the 13 words:
- **Playing a move** shifts every word down a byte with a funnel shift and keeps the bytes below the gap, then puts
  the mover's byte onto its target with a byte permute in the one word that holds it: about seven operations a word,
  always all 13, where the shared-memory loop ran to the longest gap of the warp's 32 threads.
- **The masks after a move** come from a window of the cards x − 6 to x + 5 around the pile x that moves: the padded
  row's words from (x + 2) / 4 on are shifted into place by 8, 4, 2 and 1 words (27 selects, keeping only the words
  still needed), then by the byte offset. The cards after the move are two byte permutes of the window, and all the
  matches four cards at a time (`match4`); the bits a move can change are row_move's, counted from x − 6.
- **No move is taken back.** The masks come from the window before the move is played, so a move that would leave no
  move is rejected unplayed.
- **The residual is free:** its 16 cards are the first four words. A deal loads straight into the words, with no 52
  byte stores, and a won search replays its prefix in registers too.

The same random numbers give the same moves. Checked three ways: a kernel file that played both rows side by side in
every prefix thread found no difference in the move, the piles, the masks, the cards present or any card over
100,000 deals (about 470M prefix moves), and trapped at once when a difference was forced. The test playouts now play
every move on both rows and flag any difference after it, in the masks, the covered card, the cards or the residual
(a forced difference failed the test). And both kernels win the same 99,808 deals.

`accgpu`, 100,000 deals (seeds 2,000,000–2,099,999), alternated:

| Kernel | Registers, local memory | Deals/s | Positions/s |
|---|---|---|---|
| Row in shared memory (`ed32edb`) | 72, 0 | 126,338–127,393 | 1,096–1,106M |
| The same, allowed 80 registers | 75, 0 | 127,805–128,239 | 1,109–1,112M |
| Row in registers, 72 registers | 72, 16 bytes spilled | 141,243–146,214 | 1,238–1,281M |
| **Row in registers, 80 registers** (`__launch_bounds__(BLOCK, 768 / BLOCK)`) | **80, 0** | **147,778–149,212** | **1,297–1,309M** |

Shared memory holds the kernel to 12 blocks of 64 anyway, so 80 registers lose no blocks.

**In `acc batch`** (1M deals, seeds 6,000,000,000–6,000,999,999 every run, alternated with a build of `ed32edb`):
**159,152 against 146,007 (+9%) with the CPU cool, 138,772 against 122,797 (+13%) with it hot** (another pair, run
while the CPU fell from 45M to 10M positions/s, isn't comparable). The GPU did 1,199–1,204M positions/s against
1,047–1,066M. Every deal was won, and no line failed verification. The hotter the CPU, the more deals the GPU solves
(85% to 99% here), and the more of its gain shows.

**The balance moved.** `accgpu --profile`: prefix threads waiting for room in the queue in 26.4% of their turns
(19.8% before), search threads with no residual to take in 9.6% (17.6%). The prefix warps are no longer what limits
the kernel; the search warps are nearer to it.

**Tried, not kept:**
- **Giving back the prefix threads' shared memory** (the search threads' asleep stacks only): 14 blocks fit, at
  138,547–139,136 against 141,243–142,810 for 12 (both at 72 registers). More warps still don't help.

**Considered, not tried:**
- **More search warps than prefix warps** now that the prefix warps wait more: tried with shared moves, next section.
- **Batches of 2 in the prefix warps:** cheaper now (one funnel-shift pass can close two gaps), but the prefix warps
  have slack, so they would pay only with more search warps.

## After Phase 8 — shared early prefix moves, built

**The kernel.** A prefix thread of `attempts_split` now takes a group of a deal's tries at a time
(`Settings::share_group`, numbered so that group j of deal d is j × deals + d) and plays them one after another: the
first `Settings::share_moves` prefix moves once, from random numbers of the group's own, then each try's remaining moves
from the row they left, with random numbers of its own, which its search carries on. Details:
- **The kept row is in shared memory,** in the prefix threads' 16 words each, where their unused bytes were (the search
  threads' bytes moved to the second part of `s_mem`): the first 40 cards, the cards present and both masks. In
  registers it spilled 88 bytes and lost. Sharing fewer than 12 moves leaves more than 40 cards on a deal, so that row
  is marked missing and each try plays the shared moves again (`share_moves` is at least 12 for that reason; the kernel
  still gets it right, checked at 0, 8 and 11 moves).
- **A won deal skips the group's remaining tries.** A group stuck before its shared moves are done ends all at once: its
  other tries would play the same moves and search with the same numbers.
- **A won line replays** with the group's numbers for the first `share_moves` moves, then the try's own. Every line went
  through the reference engine and none failed, and seven of a group's eight tries come from the kept row, so this checks
  it. New test: a group that spans both rungs and a short last one still run every try.
- **Warps:** `Settings::prefix_warps` says how many of a block's warps play prefixes (`block` a multiple of 32).

**At 1:1 warps it hardly pays.** The prefix warps were already waiting for queue room in 26% of their turns (previous
section), so the moves saved mostly become more waiting. **With fewer prefix warps the saving becomes search.** `accgpu`,
100,000 deals (seeds 2,000,000–2,099,999), alternated in two sets; rows of one set are comparable, heat moves the sets by
several percent:

| Prefix : search warps (block) | Group : shared | Deals/s | Won | Attempts a deal |
|---|---|---|---|---|
| 1:1 (64) | none | 142,509–145,006 | 99,808 | 130.7 |
| 1:1 (64) | 4 : 15 | 145,807–149,991 | 99,809 | 132.9 |
| 1:1 (64) | 4 : 20 | 137,861–147,421 | 99,797 | 134.0 |
| 1:2 (192) | 4 : 25 | 143,153–150,793 | 99,786 | 134.4 |
| 1:2 (192) | 4 : 30 | 144,674–152,127 | 99,735 | 144.6 |
| 1:2 (192) | 8 : 20 | 145,881–152,722 | 99,785 | 135.5 |
| 1:2 (192) | 8 : 25 | 152,669–159,474 | 99,751 | 142.9 |
| 3:5 (256) | 4 : 15 | 138,136–143,553 | 99,809 | 129.9 |
| 3:5 (256) | 4 : 20 | 146,604–150,663 | 99,797 | 131.2 |
| 3:5 (256) | 4 : 25 | 152,521–158,220 | 99,786 | 134.9 |
| Second set: 1:1 (64) | none | 135,312–145,559 | 99,808 | 130.7 |
| 1:2 (96) | 4 : 25 | 145,833–158,344 | 99,786 | 135.2 |
| 1:2 (96) | 4 : 30 | 149,314–158,745 | 99,735 | 145.5 |
| 1:2 (96) | **8 : 20** | **151,755–160,980** | **99,785** | **136.5** |
| 1:2 (96) | 8 : 25 | 158,542–164,573 | 99,751 | 143.8 |
| 1:2 (96) | 8 : 30 | 144,891–147,048 | 99,590 | 164.7 |
| 1:2 (96) | 16 : 20 | 151,649–157,995 | 99,754 | 144.8 |
| 1:2 (96) | 16 : 25 | 149,167–156,881 | 99,683 | 158.6 |

At 1:2 without shared moves the prefix warps limit it badly (108,578–114,329), and at 1:3 (block 128) they still do with
4 : 25 (117,887–118,965). More sharing helps until the tries are too alike: 30 shared moves in groups of 8 cost a quarter
more attempts. A thread's tries run back to back here, so a win is often known only after the next ones are queued; that
is why the attempts a deal rise a little more than in the measurement above.

**In `acc batch`** the misses count too, since they go to the CPU (1M deals, seeds 6,000,000,000–6,000,999,999 every run,
alternated; the CPU's speed moves with heat from 10M to 106M positions/s, so it is in the table):

| Setting | Deals/s (CPU positions/s) | GPU positions/s |
|---|---|---|
| No sharing, 1:1 (block 64) | 146,827 (15M), 148,362 (58M), 150,519 (53M), 145,599 (30M) | 1,181–1,263M |
| **8 : 20, 1:2 (block 96)** | **158,532 (13M), 172,093 (57M), 168,712 (61M), 161,473 (27M)** | **1,464–1,500M** |
| 8 : 22, 1:2 (block 96) | 160,399 (15M), 159,715 (32M), 165,995 (45M), 158,688 (41M) | 1,443–1,522M |
| 8 : 25, 1:2 (block 96) | 136,959 (12M), 122,231 (11M), 162,914 (32M), 164,990 (38M) | 1,220–1,586M |

An earlier set: no sharing 144,734–160,777, 4 : 15 at 1:1 145,938–159,536, 4 : 25 at 3:5 156,610–161,334, and 8 : 25 at
1:2 160,761–174,155. So **8 : 20 at 1:2 gives +8% to +16% over the same conditions without sharing,** and 186,447 deals/s
with the CPU cool. Every deal was won and no line failed verification.

## After Phase 8 — prefix warps in lockstep

**The rewrite.** With shared moves a prefix thread's work has a fixed shape, so a warp's 32 threads now do it together.
Each takes a group of tries; together they play the groups' shared moves and keep the rows, then play try after try, and
before starting the next they queue all of their residuals. A thread whose group ends early (its deal won, or the group
short) waits for the warp's next groups. That gives three things at once:
- **Every thread is on the same move,** so at move m a row has at most 52 − m cards, and a move touches only the words
  that can hold them: the try's own moves run on 8, 7, 6, 5 then 4 words, with a uniform branch between those lengths,
  and on 3 barrel-shift stages where 4 were needed (x ≤ 29 from move 22 on). Rows of at most 32 cards keep their masks
  in 32 bits (64-bit shifts take several instructions each). (The next section keeps only the 8-word copy.)
- **Taking work happens once a cycle.** It took 9.4% of thread time when each thread took a group as it ended, so that
  some thread's scan of won deals stalled the warp every few turns; now it is under 1%.
- **The threads stay together:** 29.6 of 32 stepping, against 23.6 when they waited for queue room one by one.

**A move does less, too.** Only the links a move can change get tested: two `match4` a move where there were six (cards
3 to 6 against the card before each; cards 5 to 8 against the card three before them, with the first place used for the
mover). The mask to select from is chosen before selecting a bit, not after selecting from both. The prefix no longer
tracks the cards present; they come from the residual's 16 cards at the end. The kept row is 10 words, and holds what a
row needs at any `share_moves` from 4 up, so nothing is ever played twice. A rung's prefix shorter than the shared moves
(the tests' small positions) means no sharing in that launch.

**Checked.** Every length of the row plays the same moves as the 13-word row and the byte row: the test playouts play all
that fit each position and flag any difference (new: all 10 forms, the masks after the move, the row, and the cards
present at the end). Same wins as the previous kernel, 99,785 of 100,000, as the random numbers are the same.

**Two smaller changes.** Search threads keep their asleep moves in one 32-bit word a depth, one access where there were
four bytes on a contended bank (+1% where the search limits it). They also take residuals only once 4 of them need one or
fewer than 16 are searching, not on nearly every turn (+5.6% where the search limits it, +1.5% here; 2 needing one lost a
little, and 8 lost).

`accgpu`, 100,000 deals (seeds 2,000,000–2,099,999), one CPU thread, alternated:

| Kernel, warps (block) | Deals/s | Won | Positions/s |
|---|---|---|---|
| Previous commit, 1:2 (96) | 164,452–166,678 | 99,785 | 1,538–1,558M |
| **Lockstep, 1:2 (96)** | **225,734–226,087** | **99,785** | **2,120–2,124M** |
| Lockstep, 1:1 (64) | 196,738 | 99,785 | 1,824M |
| Lockstep, 1:3 (128) | 201,875–201,938 | 99,785 | 1,890M |
| Lockstep, 1:4 (160; 95 registers) | 172,439–178,666 | 99,785 | 1,589–1,646M |

**In `acc batch`** (1M deals, seeds 6,000,000,000–6,000,999,999 every run, alternated with the previous commit's build):
**237,930, 232,996 and 237,338 deals/s against 184,494, 172,415 and 170,934 (+29% to +39%)**, GPU 2,056–2,128M
positions/s against 1,475–1,505M. Every deal was won and no line failed verification.

**The balance moved again:** prefix threads wait for queue room in 37% of their turns (profile build), search threads
starve in 3%, and searching is 54% of thread time. The search is what limits it now.

## After Phase 8 — the search step, and what code size costs

The search limited the kernel after the lockstep rewrite (54% of thread time), so these came next. All but the last two
leave its results the same, which is a strong check: the same residuals and the same order give the same 99,785 wins of
100,000. `accgpu`, 100,000 deals (seeds 2,000,000–2,099,999), one CPU thread, alternated; each row is against the kernel
of the row above it, in the same runs.

| Change | Deals/s | Before |
|---|---|---|
| **One undo and one mask computation a turn:** threads going down and backing up had a copy each, and a warp with both kinds ran both, one after the other. Now they join at the end of the turn | 253,763–268,669 | 213,387–225,375 |
| **No trigger for the dead-end test:** every thread ran it anyway, and a position it finds dead is dead whatever its parent, so working out whether it was needed (two `breaks_link`) only cost time | 273,058–284,425 | 253,763–268,669 |
| The other small ones together: one bit selection, the connectivity test on 32-bit words, and a taken residual's masks at the end of the turn with the others (each about +1%, by leaving each out) | 285,547–289,401 | 273,058–284,425 |
| **A fixed search order:** leaps first, then slides, the highest pile first. Most searches run out of moves within their budget, so the order hardly matters to what they find (99,784 wins), and it needs no random numbers: no generator, no bit selection, and no random state in the queue | 291,514–303,171 | 280,381–289,043 |
| **One copy of a try's moves,** on 8 words, instead of six for 8 down to 4 words (below) | 342,512–345,602 | 299,062–301,692 |
| Tidying what that left, and one connectivity test for both forms of it | 353,964–359,263 | 348,329–357,582 |
| The search's cards on four 32-bit words, so that every shift is one funnel shift (it lost before the code shrank) | 359,943–363,758 | 354,918–358,476 |

**Code size is what the tiers cost.** Keeping fewer of the row lengths of the previous section ran *faster*, though every
move then does more work:

| Own-move copies (words) | PTX instructions in `attempts_split` | Deals/s |
|---|---|---|
| 8, 8, 7, 6, 5, 4 | 4,899 | 299,062–301,692 |
| 8, 8, 6, 4 | | 317,568–321,225 |
| 8, 8, 7, 5 | | 315,624–323,063 |
| 8, 8, 5 | | 322,894–329,122 |
| 8, 8 (4 then 3 barrel stages) | 3,737 | 335,807–342,898 |
| **8** | **3,415** | **342,512–345,602** |

Hot code is about 760 PTX instructions in a prefix warp's cycle and as many in a search warp's step, with all the warps of
a multiprocessor sharing the instruction cache. So from here a change has to pay for its code.

**Warps are no longer the limit.** Half the blocks lose nothing (`blocks_per_sm`, same kernel: 8 blocks 284,273–284,644, 4
blocks 287,974–290,233, 3 blocks 280,584–281,652), and fewer attempts run on deals already won (131.5 a deal at 4
blocks, 136.5 at 8). The multiprocessor is limited by its instructions, not by how many threads wait to run them.

**In `acc batch`** (1M deals, seeds 6,000,000,000–6,000,999,999 every run, alternated with the previous commit's build):
**350,850, 359,269 and 347,102 deals/s against 245,289, 228,531 and 226,911 (+43% to +58%)**, GPU 3,193–3,316M positions/s
against 2,037–2,129M. Every deal was won and no line failed verification; the GPU now takes 98% of the deals.

**Tried, not kept** (`accgpu`, same conditions):
- **A thread backing up makes its next move in the same turn,** by saving its untried moves at each depth in shared memory
  (twice the bytes a thread, so 5 blocks of 96), with dead ends backed out the next turn: same wins, 275,209–278,643
  against 286,790–290,233. With the move and covered-card stacks in shared memory too, 263,458–265,448. A shared-memory
  access costs far more than a register operation.
- **The kept row in registers,** at 95 registers: 285,285–285,802 against 286,074–286,205 with it in shared memory.
- **More registers** (`__launch_bounds__` for 6 blocks of 96, 95 registers, no spill): 357,936–359,357 against
  356,133–362,786 at 80 with 24 bytes spilled.
- **Taking residuals:** the won flag's load used only after the turn's step, a queue of 48 or 64, 2, 3 or 6 threads
  needing one or a floor of 8 or 24: all within 1% (285,759–297,201).
- **No counting of attempts at all,** to see what the atomic add costs: nothing measurable (285,759–291,538).
- **Two steps of the connectivity test instead of three** (a suit three links away counts as cut off): 102 fewer wins and
  more attempts, 283,052–290,084 against 293,959–299,920.
- **A linear congruential generator for prefix moves:** the same speed.
- **Row lengths for the shared moves too** (13, 11, 9, 8 words): 277,343–281,514 against 293,959–299,920; the code again.
- **One copy of the move for shared and own moves** (on 13 words): 338,179–346,411 against 348,329–357,582.
- **The won flag at a try's start used after its first moves:** 290,694–294,707 against 293,959–299,920.
- **Search warps with few threads at work sleeping to wait for residuals** (fewer than 8 or 16): 292,009–296,730.
- **Cards present by a loop** instead of 16 unrolled steps: 348,873–354,734 against 348,329–357,582.
- **`write_line` out of line:** 286,323–294,873 against 299,062–301,692 (a real call costs the whole kernel).

## After Phase 8 — cheaper verification, and the retune

**The CPU was about to become the limit.** Over 60M deals the kernel above held 364,797 deals/s, the GPU a steady
3,370M positions/s, but the hot CPU spent 51.9% of its time replaying lines (21.5 µs each, 11 µs cool) and 31.4% on misses,
which left 11.7% for deals of its own: about 415,000 deals/s was all it could keep up with. Nothing held the GPU back
either, so a slower CPU would have let its queue grow without limit. So, first:
- **`replay_tops`:** a line is replayed by the reference rules on the top cards alone, which is all they look at, without
  allocating (the reference engine allocates a vector per pile, about 150 allocator calls a line). A test checks it
  against the reference engine, on random legal and illegal moves, and during a run 1 deal in 256 goes through the
  reference engine as well, which has to agree. `--out`, which writes records, still uses the whole `verify`.
  **1.2 µs a line cool, 2.2–5 µs hot**, against 11 and 21–25 µs. The freed time goes to the CPU's own deals (50–60% of its
  time where it had 3–13%).
- **The GPU waits** while more than 8 batches wait for the CPU threads, so that what waits stays small on a hot laptop.
- CPU threads take the GPU's results 32 at a time, and the per-deal tally no longer allocates a string for the stage.

**Misses cost far more when the laptop is hot.** A deal the GPU misses took 2–3 ms cool but about 10 ms hot, the CPU having
fallen to about a quarter of its speed, and the GPU itself loses about 10% (3,000–3,200M positions/s against 3,400M). Short
cool runs flatter settings with more misses; hot, they run into the CPU (5M deals, `acc batch`, in the order run):

| Rungs (budget 500 unless shown) | Misses | Deals/s, cool to hot | CPU time on misses, hot |
|---|---|---|---|
| 38 × 256, 36 × 1,024, budget 2,000 | 0.21% | 381,991, 344,660, 336,192 | 57% |
| 38 × 256, 36 × 1,024 | 0.30% | 388,884, 376,729, 333,458 | 77% |
| 38 × 256, 36 × 768 | 0.51% | 391,501, 373,027, 267,070 | 85% |
| 38 × 512, 37 × 1,024 | 0.61% | 377,612, 363,926, 240,732 | 87% |
| 37 × 1,280 | 0.63% | 365,473, 367,837, 229,893 | 88% |
| 38 × 256, 36 × 512 | 1.05% | 328,883, 311,602, 166,730 | 91% |

So a retune by `accgpu` alone weighs each 0.1% of misses as 1% of the speed (score below): roughly what these show.

**The retune**, one setting at a time from the defaults, `accgpu`, 300,000–400,000 deals, seeds from 10,000,000, two
alternated repeats. Warps first (groups of 8 sharing 20 moves, budget 2,000):

| Prefix : search warps (block), blocks a multiprocessor | Deals/s |
|---|---|
| **1:2 (96), 5** | **351,558** |
| 2:3 (160), 5 | 347,045 |
| 1:2 (192), 4 | 345,931 |
| 3:5 (256), 3 | 344,614 |
| 1:2 (96), 8 | 342,614 |
| 3:4 (224), 4 | 340,897 |
| 1:1 (64), 8 and 12 | 335,870, 333,810 |
| 1:3 (128), 6 | 316,944 |

Group and shared moves (1:2, 5 blocks): **8 : 20 355,283**; 8 : 22 353,797 (0.22% misses); 6 : 22 350,170; 8 : 24 349,756; 12 : 20
347,842; 4 : 24 345,738; 16 : 20 344,205 (0.24%); 4 : 20 342,816; 8 : 18 337,393; 16 : 16 330,602; 4 : 16 319,232; 32 : 12
309,715. Below 20 moves the row's masks need 64 bits, which shows.

Search budget and leap weight (0.20% misses at 2,000):

| Budget | Deals/s | Misses | | Leap weight | Deals/s | Misses |
|---|---|---|---|---|---|---|
| 300 | 372,148 | 0.43% | | 2 | 341,328 | 0.26% |
| 500 | 376,942 | 0.30% | | 4 | 358,198 | 0.21% |
| 1,000 | 371,219 | 0.23% | | **8** | **362,902** | **0.20%** |
| 1,500 | 365,892 | 0.21% | | 16 | 356,384 | 0.22% |
| 2,000 | 362,902 | 0.20% | | 32 | 350,002 | 0.26% |
| 3,000 | 353,790 | 0.20% | | | | |
| 5,000 | 344,885 | 0.19% | | | | |
| 10,000 | 335,040 | 0.19% | | | | |

How deep, and when to give up (budget 500):

| Rungs | Deals/s | Misses | Attempts a deal |
|---|---|---|---|
| 38 × 256, 36 × 256 | 405,030 | 2.89% | 129 |
| 37 × 640 | 395,881 | 2.28% | 101 |
| 38 × 256, 36 × 512 | 382,828 | 1.02% | 135 |
| 38 × 384, 36 × 512 | 380,759 | 0.88% | 147 |
| 38 × 512, 37 × 1,024 | 378,389 | 0.61% | 167 |
| 38 × 256, 36 × 768 | 372,552 | 0.50% | 137 |
| 37 × 1,280 | 372,798 | 0.63% | 110 |
| 37 × 256, 36 × 512 | 371,786 | 0.82% | 98 |
| **38 × 256, 36 × 1,024** | **369,106** | **0.30%** | **139** |
| 38 × 512, 36 × 1,024 | 366,618 | 0.25% | 159 |
| 38 × 256, 36 × 2,048 | 358,148 | 0.07% | 141 |
| 37 × 2,560 | 357,284 | 0.15% | 115 |
| 38 × 1,024, 36 × 1,024 | 356,936 | 0.19% | 176 |
| 38 × 256, 36 × 4,096 | 349,470 | 0.02% | 142 |

Earlier, at budget 2,000: 38 × 1,280 alone 387,044 with 1.79% misses, 39 × 1,280 294,633 with 5.2%, 40 × 1,280 199,694
with 12.6%, 36 × 1,280 294,861 with 0.14%; 38 × 128 first 338,858, 39 × 256 first 290,970. The optimum in depth is sharp (38
then 36) and in tries flat, so the rungs stay.

`acc batch`, 20M deals a run, alternated, the laptop hot throughout (a minute each):

| Setting | Deals/s |
|---|---|
| **Budget 700, batch 32,768** | **361,030, 368,337** |
| Budget 700 | 357,061, 359,056 |
| Budget 1,000 | 368,035, 346,207 |
| Budget 500 | 353,924, 354,520 |
| Budget 2,000 | 361,034, 344,031 |
| Budget 700, 8 blocks | 334,992, 349,824 |
| Budget 500, 36 × 2,048 | 330,230, 330,749 |

Fine sweep at budget 700 (`accgpu`, 400,000 deals): batch 16,384 373,725, 24,576 380,684, **32,768 388,502**, 65,536
389,796; 4, 5 or 6 blocks 385,308, 388,502, 388,410; 38 × 192 or 38 × 320 first 379,849 (0.24%), 386,511 (0.29%); 36 × 896,
× 1,024, × 1,280 or × 1,536 384,854 (0.34%), 388,502 (0.26%), 380,773 (0.18%), 377,513 (0.13%).

**New defaults: budget 700, batch 32,768, 5 blocks of 96 a multiprocessor;** the rungs, the split of warps, the shared moves
and the leap weight stay. `acc batch`, 10M deals a run, alternated with the previous commit's build: **384,756, 375,189 and
357,512 deals/s against 355,059, 350,266 and 347,689**, every deal won and no line failed verification.

**Left as they are, measured:**
- **Fewer CPU threads,** to leave the GPU more of the cooling (5M deals, hot; `--threads`): every core 365,122–412,570, 12
  348,410–378,690, 8 291,283–367,652, 6 256,200–373,755, 5 249,611–356,655, 4 193,106–317,831. Hot, the threads are needed for
  the misses.
- **The CPU's ladder for the GPU's misses** (`acc experiment`, 5,315 misses at these settings): 3,641 deals/s for the
  default, within its ±8% for start 33 to 35 (3,730–3,830), 128 or 512 tries (3,692, 3,971), and budgets of 2,000 to 16,000
  (3,450–3,795); start 32 and 64 tries are slower (3,260, 2,815).

**The second billion** (seeds 1,000,000,000–1,999,999,999, docs/RESULTS.md) ran these settings: **366,514 deals/s over 45 min
28 s**, 363,504–377,216 over every 100M deals, the GPU at 3,206M positions/s throughout, against 43,855 deals/s and 331M
positions/s for the first billion. Heat-soaked, misses took 71.3% of the CPU threads' time, and verifying lines 11.8% (4.9 µs
a line).

**Considered, not tried:**
- **A cheaper connectivity test:** a suit pair's shared ranks in 16-bit lanes and a table over the 6 pairs, about 40
  operations against 55, in a step of about 290.
- **Masks worked out from the parent's** in the search, as the prefix does (about 50 operations against 88), which needs the
  parent's masks at hand on the way back up as well.
- **The CPU's cost per miss,** now the largest thing the CPU threads do: 10 ms hot against 2–3 ms cool, though its ladder
  is flat. Keeping a miss off the CPU altogether (a later GPU launch of the misses alone, with more tries) would have to beat
  simply more tries, which lost above.
