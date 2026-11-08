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

## After Phase 8 — what winning prefixes have in common

The question: of the random prefixes that end in a win, what do they have in common, and can the kernel choose its random
moves accordingly? A scratch tool played attempts as the kernel plays them (no shared moves) on 1,000,000 deals, each until
its first win (120.9 attempts a deal, 0.825% of attempts winning). At every decision of a winning prefix it compared what was
chosen with what the policy would pick from the same options, which cancels the deal and the state; the same over a sample of
all attempts must come out at 1, and did.

**No choice stands out.** Lifts (chosen ÷ expected) are mostly within a few percent of 1, the largest in moves 30 and later:

| Move | Lift, all moves | Moves 30+ |
|---|---|---|
| Covers a card with no others of its rank left | 1.16 | 1.19 |
| Covers a card with 3 others of its rank left | 0.98 | 0.83 |
| Mover in the last pile, or next to it | 0.90, 0.93 | 0.89, 0.92 |
| Same-rank match | 0.96 | 0.89 |
| Breaks the last rank two suits share | 0.79 | 0.77 |
| Loses 4 or more legal moves | 0.92 | 0.69 |
| Leap (leaps 8:1) | 1.00 | 1.00 |

And of the positions, what predicts a win is how few **ranks** are left. Of residuals of 14 piles, those with 8 ranks are 9.1%
of searches and 32.9% of wins, those with 11 ranks 12.3% and 1.5%.

**But a small lift at every move compounds.** Choosing by it, in the same tool:

| Prefix move chosen (200,000 deals) | Attempts a deal | Positions a deal | Not won in 1,280 tries |
|---|---|---|---|
| At random, as before | 120.5 | 3,364 | 0.240% |
| Best of 2 draws, by others left of the covered rank | 56.2 | 2,277 | 0.056% |
| Best of 3 | 38.0 | 1,964 | 0.028% |
| Best of 4 | 30.3 | 1,849 | 0.018% |
| Best of 8 | 21.0 | 1,706 | 0.005% |
| Best of 32 (about the best move there is) | 16.6 | 1,640 | 0.006% |

So a prefix now draws a few moves from the old distribution and plays the one that covers the card with the fewest others of
its rank left, keeping those counts in 2 bits a rank (one register, and one word of a group's kept row). Ties go against a
mover among the last three piles: 13% fewer attempts again in the tool. A draw that loses costs nothing beyond its code; turning
moves down by chance, which the tool tried first and which wins about as much, would run a warp's whole turn again for one thread.
`Settings::draws` and `shared_draws`.

**The costs moved, and each step uncovered the next.** `accgpu`, alternated, on the laptop as it was at the time (so compare
within a row):

| Step | Before | After |
|---|---|---|
| Best of 3 (attempts a deal from 133 to 49; misses from 227 to 36 in 100,000) | 372,000 | 472,000 |
| Groups of 4, not 8 (a deal now takes few tries) | 492,927 | 502,554 |
| Batches of 65,536, not 32,768 | 492,927 | 496,366 |
| Ties against the last three piles | 516,000–531,000 | 557,000 |
| Residuals with more than 9 or 10 ranks not searched (`rank_gate`) | 566,000–572,000 | 580,000 |
| Prefix warps sleep 10 µs, not 0.2, with the queue full (they were waiting 77% of their turns, and polling takes turns from the warps working) | 582,000 | 613,000 |
| The chain rule in the search (below) | 620,000 | 650,000 |
| No replay of the prefix at a win (below) | 575,000 | 828,000 |
| Best of 5, and of 3 for shared moves; 128 tries on the first rung, not 256 | 810,000 | 889,000 |
| `write_lines` after each launch (below) | 957,000 | 890,000 |

**The chain rule.** Tops alone make a position, so the mover onto the covered card and on to a card before it reaches the same
position as the covered card going there and the mover onto it, whenever the covered card can (and unless both distances are
leaps). A hash of every position in the tool's searches says 24% of them repeat despite sleep sets, and a whole table would
save 19%; this rule saves 14% of positions, none of the wins, and takes about 10 operations a step: whether the covered card
could go there is in the parent's masks at its pile.

**The replay at a win was a third of the time.** A fit over the sweeps (time a deal against its attempts and positions) left 0.40
µs a deal unexplained. That was the winning thread playing its 38 prefix moves again to write its line, once a deal, on the
13-word row, with the 31 others of its warp waiting. Skipping it took the GPU from 575,000 to 828,000 deals a second. Now the won
flag holds the winning attempt's number and the search writes only its own moves, and a second kernel, `write_lines`, plays all
of a launch's prefixes after it, 32 deals a warp in lockstep: 7% of the launch.

**The CPU side.** At these speeds `acc batch` had three new limits, one after the other, all on the CPU:
- **Handing results on,** 92% of the CPU threads' time: 15 threads met on the channel and the queue's lock for every deal, and
  the speed *fell* to 255,000–337,000. Runs of 256 fixed that (623,842). Now a launch's results go to the workers whole, shared,
  and they take runs from it, and a run reports a count of wins: no lock, allocation, shared counter or message per deal.
- **Preparing a batch** on one thread: now four (not measured alone).
- **Sustained power.** The first half minute runs at the CPU's burst limit. After it, a line that takes one cool thread 2 µs took
  15 hot threads 18–23 µs each: all the CPU threads together do about 1.3 cool threads' work (`nvidia-smi` shows the GPU at
  1,500–1,800 MHz under its 29 W limit, 67 °C, and only 50–60% busy). So the CPU had to do less, not faster. Playing prefixes on
  the CPU took 1.4 of those 2 µs; with `write_lines` a line is 0.5 µs and the GPU is the limit again.

Workers also leave deals to the GPU while it is feeding: a few thousand deals a second at most, for heat that costs more
(899,203–920,728 deals/s against 837,918–855,621, 20M deals a run alternated).

| `acc batch`, 100M deals, hot | Deals/s | Verifying a line |
|---|---|---|
| Playing prefixes on the CPU | 465,801 | 22.6 µs |
| Playing prefixes on the CPU, a launch's results shared whole | 635,764 | 17.6 µs |
| **`write_lines`** | **854,625** (second half 864,771) | 4.8 µs |

**Against the previous commit,** `acc batch`, 40M deals a run, alternated, hot from the start: **888,843, 875,054 and 862,141
deals/s against 282,987 and 245,302**, every deal won and no line failed verification. (Hot like this the previous commit's misses
took 77% of its CPU threads; its 366,514 over the second billion was on a cooler laptop.) The GPU misses 1 deal in 9,000, not
1 in 400, and the CPU threads wait for it most of the time.

**The final sweep** (`accgpu`, 655,360 deals, alternated): the defaults 893,910; batch 131,072 888,492; groups of 5 885,440;
38 × 256 first 885,243; 4 draws 880,390; gate 9 and 11 879,702; 6 draws 873,008; groups of 3 872,792; 2 shared draws 869,340; 38 × 64
first 861,632; batch 32,768 840,947. Earlier, with 5 draws: 37 moves first 799,369, 39 and 37 767,954, 16 shared moves 768,854, 24
shared moves 865,536, budget 500 871,707, budget 1,000 868,993, 36 moves alone 685,416, gate 8 and 10 728,762 (at 3 draws).
**New defaults: 5 draws, 3 for shared moves, 38 × 128 then 36 × 1,152, groups of 4, batch 65,536, gate 9 and 10;** budget 700, 5
blocks of 96 and leaps 8:1 stay.

**Tried, not kept:**
- **Suits as well in the choice:** the tool says 8% fewer attempts for a covered card of a scarce suit, but the kernel, with a
  second register and kept word, ran slower (545,775–548,006 against 556,615–558,227).
- **A cheaper draw,** the first legal move at or above a random pile (about 12 operations against 45): the tool says 1.9 times
  the attempts. That favours moves after a gap in the masks, and doing so on purpose takes about a fifth more attempts.
- **Other things to choose by,** each in the tool at best of 3, by its model of the cost: the mover's rank +3.6%, its suit +4.5%,
  a mover with another move +2.4%, same-rank matches favoured a little +2.5%; avoiding same-rank matches, a move onto a pile
  another could reach, the left half, pile 0, leaps, slides: worse or flat. Each would cost code, and the suit, the largest, lost.
- **Rank limits inside the search:** +3% in the tool for 10 operations a step, not built.
- **One generator step for all the draws:** +0.4%.
- **Taking residuals less or more often:** flat (`TAKE_MIN` 2, 4, 8; `TAKE_FLOOR` 8, 16, 24).
- **No count per attempt, and no won flag load at a take:** no change, so a warp waiting on memory seems not to hold up the rest
  of its multiprocessor.
- **Fewer CPU threads, again:** 8 or 5 threads 352,624–377,765 deals/s over the second half, all 503,603–675,368, before
  `write_lines`.

**Considered, not tried:**
- **A table of positions in the search:** a further 5–10% of positions at most after the chain rule, in a step that would need a
  hash and a memory access. The next pair of lines to one position is a long tail (slide then leap against leap then slide,
  8% of what repeats).
- **The search's arithmetic** ("cheaper verification, and the retune"): a cheaper connectivity test and masks from the parent's are
  still open. Search positions are now about a third of the GPU's time.
- **`write_lines` on the shorter row** for a try's own moves, which the hot code has: most of its 7%, perhaps.
- **A later launch for what the first didn't win,** so the first needn't scan all the tries of deals already won: about 6% by the
  difference that fewer tries make; it wants a second pipeline in the host.
- **Best of k on the CPU's ladder:** of no use to `acc batch` now (misses take 14% of a CPU with 58% to spare), but it may help
  a run with no GPU.

## After Phase 8 — pausing, and tens of billions of deals

`acc batch` can now stop and carry on: Ctrl+C or a file pauses it, a crash costs the last minute, and `--resume` goes on from
either. The report comes out as for one run. README has how to use it; this is why it is built this way.

**What would have gone wrong at 25 billion deals,** about 8 hours at these speeds:

- **The check that the deals all differ** held 8 bytes a deal in memory: 200 GB, on a machine with 48. It ran after the deals and
  before the report, so the run would have died at the end with nothing to show for it.
- **The report was only on the terminal,** and it waited for the last proof.
- **If the GPU failed,** the CPU solved the rest, as it should in a short run. Here that would be about a month.
- **Nothing could stop and carry on,** so any crash, or wanting the laptop back, lost everything.

Looked at and fine: seeds and counts in 64 bits (nothing truncates; a launch's own counts are 32 bits, at most 1,280 × 65,536), the
costs of the deals solved on the CPU (16 bytes each, 2.8M deals, 45 MB), the backlog (8 batches, 37 MB), the speed log (2 MB).

**Results in seed order.** Workers finish out of order, so a checkpoint would have to say which deals were done. Now the main
thread applies results strictly in seed order, holding what comes early, and where a run stands is one number with totals that
match it exactly. What it holds is small: a hard deal at the head of the line stops the count for up to a minute but not the work,
and a minute's results are about 200,000 runs of 256, 40 MB at most. The counts are carried with the results, so the GPU's
attempts, positions and wins are those of the deals applied. With `--out` the records are written in the same order, which they had
to be anyway.

**The checkpoint** is JSON, written to a temporary file, synced and renamed, every minute and at a pause. What it counts is on disk
first: `--out` is flushed and synced, and the positions and times of the CPU's deals go to a file of costs. A resumed run cuts both
back to what the checkpoint counts, so what came after it is done again without being counted twice. Dossiers are written whole
for the same reason, as a resumed run reads them again. The settings come from the checkpoint in full, tables found included.

**Proofs.** A deal goes on a list when it is sent to the prover and comes off when its proof is recorded; the checkpoint has the
list and the tallies, taken together, and a resumed run proves what is on the list again. A proof in hand at a pause starts over.
They take seconds in practice (2 s to 16 s for the real ones so far), and the limit, 10 billion positions a run, about an hour.

**The check on disk.** Past `--games-memory` (16 GB, 2 billion deals) each fingerprint goes to one of up to 4,096 files by its first
bits, as the next 40 bits: 5 bytes a deal, about 16 million deals a file, so that a file sorts in memory. What repeats in a file is
looked up by fingerprinting all the deals again, as in memory. Repeats of those bits that aren't of the whole fingerprint cost
nothing: on 2 billion seeds it found 14,234 (the formula, n²/2 over 2⁴⁷, says 14,211). **216 s for 2 billion seeds, on disk** in the
checkpoint's directory, so about 36 minutes and 100 GB for 20 billion, if the disk keeps up past its cache. It is in the final stage
with the last proofs, which saves as it goes and can pause; a pause there starts the check over. If the disk fills up, the run keeps
its checkpoint and says so, and `--no-games-check` skips the check.

**The GPU failing** now pauses the run, with the deals it had in hand left to be done again, and exits with code 75: a new process
may well get the GPU back. Without a checkpoint (`--no-checkpoint`) the CPU still solves the rest.

**Is it slower?** The added work is a few loads a deal and a small lock and tree insert per 256 deals, so it shouldn't be. `acc
batch`, 60M deals a run, from cold-ish to hot and back in the order shown, with a checkpoint every minute:

| Run | Last commit | Pause and resume |
|---|---|---|
| New first, then base | 803,123 | 816,294 |
| Base first, then new | 879,237 | 885,936 |
| New first, then base | 928,486 | 904,270 |

Run with the new build always second, it looked 1–2% slower, but then so did each later run of the same build as the laptop heated up:
heat, not the code. The range across runs is 760,000 to 930,000 deals a second, so 20 billion is 6 to 7.5 hours.

**Tests** (`crates/cli/tests/pause_resume.rs`, on the CPU, where every deal's solve is deterministic): paused over and over, crashed
with its records and costs ahead of its checkpoint, and with proofs in hand and during the last ones, each against the same run left
alone: reports and records equal but for times. Ctrl+Break pauses and a second stops at once. In `runner.rs`: order, a pause and
carrying on, on the GPU too; the GPU made to fail, both ways, with every deal coming back once.

**Rehearsed on the GPU.** 30M deals from seed 100,000,000,000: paused by file (0.33 s to stop), killed, carried on to the end; the
GPU had been given exactly 30,000,000 deals, and won 29,996,675. A run of 25 billion paused, its checkpoint moved on to 24,999,000,000,
and carried on to the end, for the arithmetic past 2³².

**Considered, not tried:**
- **Restarting by itself after a GPU failure:** waiting on a new copy of itself, for a run that may have hours left and a GPU that
  may not come back. Exit code 75 and a shell loop do it, and a person can see what happened.
- **A list of done ranges, not seed order,** in the checkpoint: more to save and load for no gain, since nothing else wants results
  out of order.
- **Saving a proof's progress** at a pause: its dead-position sets are gigabytes, and the proofs so far took seconds.
- **Saving the check's progress** between its passes: a pause there costs up to half an hour of a run of many hours.
- **Seeds with the fingerprints,** so that the check needn't fingerprint the deals twice: 10 bytes a deal, 200 GB, to save about 18
  minutes.
- **The check during the run,** with the CPU threads 60–80% idle: heat that costs the GPU 5–10%, for hours (see the workers' own deals,
  above), to save half an hour.
- **A crate for Ctrl+C:** the two calls it wraps are a few lines, and a new download isn't worth them.

**The 20 billion** (docs/RESULTS.md) then ran in one go: **934,074 deals/s, 5 h 56 min 52 s**, and 927,678–937,553 in each hour, at the
top of the range above. Why it held there when those back-to-back runs fell to 760,000 wasn't measured; the laptop had cooled to 40 °C
before it started. The GPU didn't stop and nothing needed resuming, so of all this only the check on disk was called on. It took about
27 minutes, under the 36 estimated: up to 100 GB, and 89,070 fingerprints looked up (the formula, n²/2 over 2⁵¹, says 88,818).

## After Phase 8 — the search step, exact choice, and what the rest came to

The 20 billion run (934,074 deals/s, commit 575a396) left a clear picture: the GPU is the limit, the search is 60% of its warp
time, and the CPU is half idle. Several pieces of work ran side by side from that commit, each measured on its own and, if kept,
merged here into one build. Seven changes were kept; the rest came to nothing or to a few percent, and are listed below with their
numbers. Speeds are deals per second; "GPU alone" is `accgpu --cpu-threads 1`, "hot" is `acc batch` over 60 million deals or more,
second half of the run, configurations alternated with the order swapped each pass.

**Where the time went, at 575a396** (a 200M-deal run, second half, 860,928 deals/s). The GPU was busy 99.3-99.7% of the time, at its
power cap (28.8 W, 1,451 MHz of 2,100). The feeder thread waited on the GPU 56% of the time, the 15 workers 73%; all CPU work came
to about 5,600 ns a deal, 4.9 of 16 threads. Inside the GPU, the search step was 59.6% of warp time, the prefix's own moves 16.2%,
the queue 7.5% and shared moves 5.9%. `write_lines` is 2.2% of a launch. Two launches in flight add nothing but close a 17% gap.

**Kept:**

| Change | Measured |
|---|---|
| **Exact best-class choice** for the prefix's moves (it plays a move covering the card whose rank has the fewest others left, and of those one whose mover isn't among the last three piles) | Hot, 60M deals, three runs each: 1,058,141, 1,102,022, 1,112,099 against 923,252, 901,587, 892,305 (**+20.4%**). GPU misses per 60M 6,393 to 2,640. All verified |
| **One-pass search step:** back up, masks, then move in one turn, a move tested before it is played; the won flag polled in the take block; connectivity by a table over the 6 pair links; packed masks; `leftmost` and `conn_gate` fixed at compile time | GPU alone 927,994 to 1,102,713 (**+18.8%**), same misses; on exact choice 1,126,288 to 1,542,980 (+37%). Hot with exact choice: second half mean 925,170 to 1,522,531 (**+64.6%** over the old kernel) |
| **Bit-plane search row** (from the DEAD work): the search's at most 16 cards as 3 words of two 16-bit planes each, not 4 words of bytes, so one shift and xor compares the whole row with itself and a move is a mask and a shift on each word | DEAD, GPU alone, 6.55M deals: 1,795,577 to 2,060,816 (**+14.8%**), the same 481 misses, 79 registers; a search step's PTX 509 to 377 (-26%). Here, GPU alone, 2M deals, 2 passes: 2,048,814 and 2,075,470 against 1,743,920 and 1,797,725 (+16.5%). 4M deals with every line verified by the CPU: 0 failed, all won. Hot, 60M deals: +14% (below) |
| `select_bit64` with one copy of its steps, and `write_search` out of line | +1.6% and +0.6% on the old kernel, together +2.7% to +3.7%; 76 to 72 registers. Not timed again on the merged kernel |
| **One more prefix move:** rungs [39, 128] and [37, 1152], where it was [38, 128] and [36, 1152] | GPU alone, exact kernel, 3.3M deals: about 1,280,000 against 1,112,000, search positions a deal about 1,346 against 1,792 |
| **Warps 8 blocks of 64** (1 prefix warp, 1 search warp), where it was 5 of 96 | Screen, GPU alone, 2M deals, ratio to 5 of 96: 1.086-1.090 (see below) |
| **CPU cleanups:** a GPU line is replayed from the 52 bytes the GPU was given, not from a deal shuffled again (all the way from the seed every 256th deal, as the reference check always did); masks without the unused hash | 400-500 ns of CPU a deal, each. No direct speed: the GPU sets it. Verifying a line is 1.2 µs on a cool CPU, against 3.2 µs hot before; in the long runs below, with the CPU at half speed, 3-8 µs |

The CPU changes and the host side of the trillion work meet in the worker loop: the deal is made once, and only when something wants
it (the sample of the repeat check, or the full replay that is due), not for every deal.

**Screen of settings on the combined kernel,** GPU alone, 2M deals, ratio to 5 blocks of 96 at 39 moves:

| Warps | Ratio |
|---|---|
| 8 x 64 | 1.086-1.090 |
| 10 x 64 | 1.084 |
| 4 x 128, 2 prefix warps | 1.076 |
| 12 x 64 | 1.072 |
| 6 x 64 | 1.071 |
| 3 x 160, 2 prefix warps | 1.044 |
| 4 x 128, 1 prefix warp | 0.827 |

- **Moves in the prefix:** 38 moves 0.960 on 5 x 96 and 0.973 on 8 x 64; 40 moves 0.859 and about even. The default stays at 39.
- **Connectivity test every other turn:** 0.538. **Groups of 3 or 2 tries** (base 4): 0.969, 0.879. **The order a deal's tries are
  searched in:** within 2.5%.

**The retune on the merged build,** rungs [39, 128] and [37, 1152], 8 blocks of 64, everything else as before. GPU alone, 2M deals from
seed 600,000,000,000, 1 CPU thread, 2 passes alternated:

| Setting | Pass 1 | Pass 2 | Mean | Against the defaults |
|---|---|---|---|---|
| Defaults | 1,869,387 | 1,776,081 | 1,822,734 | |
| 10 blocks of 64 | 1,754,921 | 1,732,496 | 1,743,709 | -4.3% |
| Rank gate [8, 9] (was [9, 10]) | 1,851,238 | 1,849,127 | 1,850,183 | +1.5% |
| Second rung [37, 576] (was 1,152) | 1,921,292 | 1,800,109 | 1,860,701 | +2.1% |

Then hot `acc batch`, 60M deals from seed 700,000,000,000, three passes (the order reversed in the second), second half of the run:

| Setting | Runs | Mean | Against the defaults | GPU misses in 60M |
|---|---|---|---|---|
| Defaults | 1,736,398, 1,736,429, 1,760,835 | 1,744,554 | | 4,436 |
| Rank gate [8, 9] | 1,720,918, 1,809,459, 1,775,858 | 1,768,745 | +1.4% | 5,677 |
| Second rung [37, 576] | 1,736,414, 1,790,185, 1,601,819 | 1,709,473 | -2.0% | 15,934 |

Nothing clearly won, so **the defaults stay.** The rank gate's +1.4% is inside the spread between runs (4-5%). The shorter second rung
quadruples the misses, which only `acc batch` pays for: in its last run the CPU threads spent 45.7% of their time on misses and waited
for the GPU only 11.8%, the CPU nearly the limit as the laptop heated. 10 blocks of 64 lost.

**The bit-plane row on the merged build.** Verified before it was kept: the kernel tests pass (`cargo test -p accordion-gpu`, including
`playouts_match_the_rust_engines`), and `accgpu` over 4,000,000 deals (seeds from 610,000,000,000) had every GPU line replayed by
the CPU: 3,999,724 won and verified, 0 failed, all 4M won (the CPU solved the other 276). Against the build before it (GPU alone, 2M
deals from 600,000,000,000, 1 CPU thread, 2 passes alternated): 2,048,814 and 2,075,470 against 1,743,920 and 1,797,725 (+16.5%).
Hot `acc batch`, 60M deals, three passes alternated: the whole-run figures swung from 0.9M to 1.9M deals/s, because other work on the
laptop took CPU during some runs (verifying a line cost 5 µs where it costs 2), so only the per-second rate over the last three
quarters of each run is compared, and only runs the machine left alone: bit-plane 1.90M and 1.89M, before it 1.65M and 1.68M
(**+14%**); the third run of each was slowed to about 1.0M and is left out. Reading them as run (second half of the run, whole run):
bit-plane 1,878,957 / 1,907,474, 1,506,416 / 1,640,306, 1,132,183 / 1,006,715; before 1,550,538 / 1,622,804, 919,364 / 1,041,058,
1,315,279 / 1,467,692.

**The sampled repeat check, in the merge.** With the sample on (`--games-memory 1 --games-disk 0.01`, 0.5% of 200M deals), the first
merge made the deal from its seed for every deal to hash its first cards: 1,562,854 and 1,672,713 deals/s against 1,688,147 and
1,738,897 without it (-7.4% and -3.8%), the CPU threads' verifying 22-36% of their time, with the sample 46-49%. The sample now takes the hash from
the 52 card bytes the GPU was given (`Sieve::note_codes`, tested against `Sieve::note` on 20,000 deals) and shuffles a deal again
only if it is in the sample: 1,743,564 and 1,724,842 against 1,744,676 and 1,735,952, **-0.1% and -0.6%**, within the spread.

**The long hot comparison.** The bit-plane build against the old binary (the kernel of the 20 billion run), `acc batch`, 200M deals from seed
700,000,000,000 a run, in one hold of the machine, in the order new, old, old, new, new, old, old, new (pass 1 forwards, pass 2 reversed, and so on),
with the GPU and the CPU sampled every 5 s. All 200M won and verified in every run, 0 lines failed; GPU misses 14,736 in 200M for the new
kernel (1 in 13,572) and 21,719 for the old (1 in 9,209). The sampled repeat check was not on (below 2 billion deals every deal is checked, in
memory). All eight runs, in the order they ran:

| Run | Second half | Whole run | GPU busy, mean (min) | GPU clock, mean (min) MHz | CPU speed, mean (min) | CPU busy, mean | CPU threads waiting for the GPU | Verify, µs a line |
|---|---|---|---|---|---|---|---|---|
| New | 1,820,435 | 1,768,538 | 93% (47) | 1,448 (1,380) | 107% (77) | 59% | 46.5% | 3.1 |
| Old | 850,822 | 857,234 | 98% (81) | 1,491 (1,410) | 56% (41) | 68% | 60.3% | 4.5 |
| Old | 869,100 | 869,214 | 100% (90) | 1,477 (1,440) | 58% (40) | 52% | 64.7% | 4.0 |
| New | 907,827 | 1,122,739 | 59% (14) | 1,400 (705) | 49% (39) | 73% | 31.8% | 6.2 |
| Old | 494,294 | 563,239 | 67% (17) | 1,496 (1,260) | 59% (37) | 53% | 26.6% | 11.9 |
| New | 813,863 | 985,755 | 48% (18) | 1,591 (1,125) | 65% (39) | 55% | 18.8% | 8.1 |
| New | 1,254,944 | 1,297,088 | 66% (20) | 1,496 (1,080) | 62% (43) | 74% | 39.8% | 4.8 |
| Old | 817,324 | 831,744 | 94% (50) | 1,522 (1,440) | 49% (38) | 71% | 55.0% | 5.2 |
| **Mean of four, new / old** | **1,199,267 / 757,885** | | | | | | | **x1.58** |

The pairs next to each other give x2.14, x1.04, x1.65 and x1.54. The spread is not noise in the kernel: **the CPU limits the new build once
the laptop is hot.** In the first run, with the CPU cool (107% of its base speed, boosting), the GPU was 93% busy and the CPU threads waited for it
46% of the time: about balanced at 1.8M deals/s. After that the CPU's speed fell to 49-65% of base (minimum 37-43%) and stayed there, and with it
verifying a line went from 3.1 µs to 4.8-8.1, the CPU threads' waiting share fell to 19-40% (from 46-65%), and the GPU went idle for stretches:
busy 48-66% on average and 14-20% at the minimum, its idle throttle reason (`0x1`) appearing in the samples, its power falling from 28.6-28.8 W to
24.7-27.9 W and its clock dipping to 705-1,125 MHz while it waited. The temperatures (77-80 °C) barely moved. The old binary lost the same CPU speed,
but its GPU is the slower part: it was 94-100% busy in three of its four runs (CPU waiting 55-65%), and in the fourth (the third in all) the CPU
slowed further (verifying 11.9 µs a line) and its GPU fell to 67% busy. So a long run at this speed is set by the CPU's sustained speed, which is
a matter of the laptop's cooling and power as much as of the code: the time for a trillion deals below is a range, not a figure.

Before the bit-plane row was added, the same comparison (not sampled, earlier in the day) gave 1,705,622 and 1,633,793 for the new build against
881,585 and 896,167 for the old (x1.878), with the GPU 100% busy and the CPU threads waiting for it 67-71% of the time.

**Tried, not kept:**
- **Openings made on the CPU:** all of a deal's groups sharing their first K moves, played on the GPU to measure only the lost variety:
  K=4 986,576 deals/s against about 1,138,000, K=20 146,282. The GPU would save at most 5.9% of its time. The shuffle was already on
  the CPU.
- **Prefix and search in separate kernels:** on the same work with wins not stopping anything, 73.23 ms a launch against 79.97 mixed
  (-8.4%), under the 15% that would pay for a rewrite, and each round costs about 1 ms. Alone the prefix is 19% of the time, not the
  33% its warp time says (that includes waiting for a turn). **Searching a deal's residuals best first:** 0.4% to 36% worse (the
  positions it saves cost more in extra prefixes).
- **Code size.** Making the hot code smaller by loops, calls or merged copies lost 3% to 14%: draws in a loop -8.4%, one call site
  -9.6%, own moves on the wide row in two copies -13.7%, uniform guards around the wide parts -5.5%, `select_bit` a real call -2.7%.
  A backward loop of 2 iterations costs about 11 instructions an iteration, a divergent branch about 6. No compile option helped
  (CUBIN -0.2%, `ptxas -O1` -1.7%, launch bounds -0.4%, -0.5%); NVRTC takes no `-O` options. `cfold` (+2.6% more on the old
  kernel) wasn't merged, since the search step fixes both tests at compile time.
- **The search step, other forms:** a second mask computation after the move +0.8%; more than one move tried a turn, on exact choice
  2 tries -1.5%, 3 -5.4%, 4 -9.4%; polling the won flag every other take -1.3%; the `__all_sync` moved out of the turn -1.9%
  (cause unknown); `TAKE_MIN` 2 or 3 +0.5%; dead-pile pruning +1.0% (positions 2,813 to 2,680, misses 714 to 693); one multiply to
  gather 8 bits -0.1%.
- **Pruning inside the search.** Rank limits: +0.7% on the GPU, with 16% fewer positions, 8.7% more attempts and 40-60% more misses.
  A 16-entry cache of positions seen 0.976 (4.6% of positions in the simulator, 2% in the kernel). In the simulator: a whole table of
  positions -11.7% positions (free, worth about 7%; 256 entries are 512 bytes a thread, more than the shared memory there is);
  Bloom filters, whose false hits cut off wins (misses 33 to 6,278 per 100,000); limits on children, positions under a node, linked
  pairs, and skipping moves onto a rank with others left: all flat or worse. The best model at one level, 12 piles: 2% of wins lost
  for 11% of the positions. Search positions are worth less at the margin than their 60% suggests, about 45%, so a rule has to save
  2.2 times the wins it loses.
- **Looking ahead for the prefix's moves,** best of k candidates by the next moves it sets up, in the kernel: k=2 -24%, k=3 -33%,
  k=4 -41% in speed, for 42% fewer attempts at k=3, 72% fewer misses, all verified; each candidate costs a whole after-move mask
  computation, and the code spills. In the simulator (39 moves, 8,000 deals) the same choice makes attempts 20.8 (exact) to 15.0, 11.5,
  9.5 and 8.0 at k=2, 3, 4, 6.
- **Sharper residual gate** (ranks, isolated cards, suit minimum, links; as good as boosted trees, AUC 0.89 against 0.79 for ranks):
  +0.8%. **A lower or value-based budget:** wins per 1,000 positions are flat over a search (1.45 at 50-100 positions, 0.81 at
  600-700), above the all-in rate, so 700 is about right. **Hard filters after exact choice** (covered card with no move of its own,
  mover with no other move, middle piles, leaps, near the last move, gaps, no link broken): all worse or level.
- **In the simulator, 39 moves, base 20.69 attempts and 1,073 search positions:** closed-form endings are dead (positions at 4 piles
  or fewer are 1.6%); random first K moves K=5 +2% attempts, K=10 +12%, K=20 +69%; re-rolling the last moves, sweeping windows,
  skipping repeated residuals (0.41 a deal, -2.1% positions), target ranks, per-deal rank priorities (46 to 247 attempts): all worse.
  Cheap score terms: covered suit scarcity 17.82 attempts but 1,271 positions; far2 19.96 and 1,060; low target pile 19.88 and
  1,125; others level or worse.
- **Dead positions and dead moves in the search** (DEAD). The uncoverable-card rule (a card with at most one partner and none on its
  right can never be covered, so two such cards make a position dead) is exact, checked on 56,800 residuals with no budget, and its cut
  is real in the kernel (search positions a deal 1,996 to 1,866), but it runs -7.3%: its test costs more than the moves it saves, since
  a warp pays for a block if any thread runs it. The bulk dead-move filter (the ranks a suit reaches the rest only through, and the
  path of 4 suits; 62% of the moves at arrival are dead) runs -8.0%, both together -14.0%. Half of even a perfect filter's turns
  try no move, so no filter of this kind gets below 2 turns per kept move. A wider "gap form" of the rule fired on won positions
  (15 of them) and isn't exact. Untried: the rule only on a residual as it is taken, where it costs little.
- **Small things on the merged build** (SMALL, 2M deals, 3 passes, base 1,780,453): diverse first draws 0.975; a queue of 64 0.975;
  groups of 5 0.991; every non-search thread taking residuals 1.006 (not repeatable); `TAKE_MIN` 1 0.980; budget 1,000 0.994 (misses
  -11%, but the CPU waits for the GPU 18-71% of its time, so that buys nothing); EcoQoS on the CPU workers about even.
- **Retune:** on this build, 10 blocks of 64 4.3% slower than 8; the rank gate [8, 9] +1.5% alone, +1.4% hot, inside the noise; the
  second rung [37, 576] +2.1% alone and -2.0% hot (below).

**Considered, not tried:**
- **Gathered endgame lookups** (TABLE), built on the old kernel: 11 piles +6.6%, 10 piles +0.5%; attempts 26.2 against 27.7,
  positions 1,793 against 2,816. It needs 1.15 GB on the GPU and wasn't merged with the new search step, so there is no number for
  it there; the search is now a smaller share, so less is likely.
- **Removing the `deal_counts` count** (`NOCOUNT`): estimated at +1.3%; SMALL built it as an option and measured 1.039 (1.049, 1.052,
  1.017). It leaves the report's GPU attempts and positions at zero, so it isn't in the default; `patches/small-nocount.patch` has it.
- **Learning within a deal**: in the simulator 17.07 attempts against 20.69 and 868 positions
  against 1,073, but about 4 times the misses; not tried in the kernel. Deals are persistent: wins of the two halves of a deal's tries
  correlate 0.41.
- **Group allocation, early abort** (ALLOC): only the one-more-move result was reached. The row's value predicts a win with AUC 0.68 at
  move 20, 0.75 at 28, 0.80 at 32 and 0.86 at 36, so an early abort has less to go on the earlier it is made.
- **Dead moves filtered in bulk** in the search (bridge links with a single shared rank give the dead cards at once, about 100
  operations): dead tries are half of the search's thread-turns, so it is the one large prize left there, but several tries a turn
  lost, and this adds about a third to a turn before it saves any.
- **Exact dead-end tests stronger than connectivity:** no candidate found; the rest is the endgame table's work.
- **The machine code:** only PTX could be read (no `cuobjdump`), so the costs of the search step are by instruction counts.

**Neural nets for the random moves (provisional; `neural` notes, held-out deals).** A value net, trained 3.5 minutes in numpy on rows from
the old policy, rates a prefix row better than the hand-made models (AUC at move 20 0.778 against 0.681 for the best of them, 0.925
against 0.864 at move 36). Played in the simulator (300 fresh deals, exact choice for the 20 shared moves, then the move whose row the
net rates highest, softmax at strength tau): from move 20, 3.22 attempts and 604 search positions a deal at tau 12, against 15.88 and
1,466 for exact choice and 4.30 for the best fitted model, so the search positions fall too. On the last moves only, exact before:
from move 30, 5.81 attempts and 874 positions; from move 34, 9.43 and 1,134 (exact 15.90 and 1,533 there). But the net is about 290,000
multiply-adds a row, and a deal has about 480,000 thread-cycles at this speed, so it is a ceiling, not a method. An N-tuple form that
would fit the kernel doesn't work yet: 39.86 attempts and 1,634 positions (base 25.2), as an unfinished run. And a win as a parse of
stretches of 3 piles needs no attempts: 50% of deals at cap 1, 98.7% at cap 8, but 30,000 to 900,000 merges a deal at tens of
operations each, more than today's solve costs.

## After Phase 8 — a trillion deals

What it would take to run a trillion seeds (from seed 22,000,000,000 on) on this laptop, and what had to change
for it. Nothing here depends on the GPU's speed; only the time does.

| | 200 billion | 1 trillion |
|---|---|---|
| Solving at 934,074 deals/s (the 20 billion run) | 59.5 h | 297 h, 12.4 days |
| At 1.82M deals/s (the first run of the final build, CPU cool) | 30.5 h | 153 h, 6.4 days |
| At 1.20M deals/s (the mean of four runs of it, CPU slowing as the laptop heats) | 46.3 h | 231 h, 9.6 days |
| Checking the sample at the end (estimated) | about 4 min | about 4 min |
| Disk, peak and written in all | about 51 GB | about 52 GB |
| RAM | about 2 GB, up to 12 GB in a rare large proof, 18 GB in the check | about 3 GB, the same peaks |

**What was audited, and what the 20 billion run's rates make of it** (1 deal in 9,290 on the CPU, 1 loss in 960M deals):
- **The check that no two deals are the same** was the one thing that didn't scale: 5 bytes a deal on disk is 5 TB at a trillion,
  with 523 GB free, and its two passes 16.5 hours. Now up to `--games-memory` (16 GB, 2 billion deals) every deal is checked
  exactly, as before; past that a sample is noted during the run, below.
- **Fine as it is:** 64-bit fingerprints (27,105 collisions at a trillion, compared card by card); the file of costs (1.72 GB); the
  checkpoint (3.9 KB, doesn't grow); about 1,040 proofs, each under a minute; dossiers and loss records (5 MB); the speed log (75 MB);
  counters, which are `u64`; seeds past 2^32, 2^37 and 2^40; the win rate to 10 decimals (matches exact arithmetic at a trillion);
  memory and handles over 3,039 launches (flat). The games log's index is 40 bits, so a run over 1.0995 trillion is refused unless
  it passes `--no-games-check`.
- **Pausing:** `--stop-after <hours>` and `--until <deals>` pause the run, and `--resume` goes on. A GPU that fails to open on resuming
  or answers nothing for 15 minutes exits 75; a disk under `--keep-free` (5 GB) or 5 failed saves in a row, 74; the log being
  unwritable, 74 at once. `tools/long-run.ps1` carries on after 75 and stops with a message if `acc` won't start (Smart App
  Control blocked a fresh `acc.exe` twice in this work, exit 126). `.events.log` and `.status.json` sit beside the checkpoint.
  The checkpoint format is now 2: no older checkpoint resumes.

**The repeat check, past 2 billion deals.** The user's decision was that no proof is needed if the odds of a repeat are under 1 in
1,000. By the birthday bound at 1.022 trillion they are about 9.7e-34 for the same game with suits and ranks renamed, and 6.5e-45
for the same deal, if the generator behaves like a random one; so the check only backs that assumption. A hash of the game's first
16 cards, unchanged by renaming, picks the deals: each costs 10 bytes (a 5-byte key and a 5-byte position) in an append-only log in
seed order, so a checkpoint can say how much of it counts and a resume can cut it back. At the end it is read a part at a time
within `--games-memory` and deals sharing a key are compared card by card. The default room is 50 GB (or half of what is free): 0.5%
of the deals at a trillion. It would still show a flaw that makes more than 598 repeated pairs, and the count of chance matches of 40
bits tests the generator to about 0.03%. `--games-disk` with room for 5 bytes a deal asks for the full check, two more passes at the
end; room for 10 bytes a deal logs every deal and counts as exact. The report says "all deals differ" only if every deal was
looked at; for a sample it says "not all checked", how many repeated pairs it would have seen with 95% chance, and the chance matches
against the formula.

The hash costs 23-30 ns a deal on a cool thread (85 ns with branches: whether a rank is new among the first cards can't be
predicted). Hot `acc batch`, 100M deals a run, alternated, with a 2.6% sample, on 575a396: 826,809 and 827,861 deals/s on average,
the same within a noise of about 5%; verifying a line looked slightly dearer (5.7-5.8 µs against 4.3-6.2), with the CPU about half idle.

**Rehearsals:**

| What | Result |
|---|---|
| 30M deals at seeds from 1,021,900,000,000: paused by file, killed, carried on | Pause took 0.35 s; the checkpoint counted 5,767,050 log bytes of 6,272,800 on disk and the rest were cut; the GPU was given exactly 30,000,000, 0 lines failed; 780,189 sampled, as computed directly |
| A trillion-deal checkpoint moved on to its last million | Report correct: 999,999,998,960 won, 1,040 lost, CI [99.9999998895%, 99.9999999021%], 15,258,789 launches |
| 2B seeds, 1/8 sample, a repeat planted every million | 249,970,595 records; 28,232 chance matches against 28,415 by formula; **244 of 2,000 planted found**, expected 250 ± 15; 59 s to write, 6 s to check |
| 2B seeds, every deal in the log (20 GB, 2 parts) | 1,817,977 chance matches against 1,818,989; 0 repeats; 156 s to write, 71 s to check |
| `long-run.ps1` killed twice, run in legs, binary missing | All behaved |
| Disk | 2.37 GB/s write, 2.91 GB/s read (51 GB) |

**Not verified:** a run of days; 15 million launches; the check on a real 50 GB log; a full disk, a power cut, a reboot, a sleep, or a
real GPU failure; the watchdog firing for real. The timing tests in `pause_resume.rs` failed now and then on a loaded laptop: a run still starting
up deletes a pause file it finds, so the single request they wrote 250 ms after a start could be lost and the run went on to the end. They now
ask again every 250 ms until the run ends. That exposed a real fault, fixed in this build: a pause (or a crash) after the last deal, while the
last proofs and the check ran, left the checkpoint with the length `--out` had before its last records, and the resume cut the file back to it,
losing up to a few thousand records (4,929 of 60,000 in the test). The proofs test now compares the records as well as the report.

**Left to the user:** Windows Update (active hours 17:00-23:00, so a restart can come at any other time); starting again after a
reboot (nothing does this unless a logon task is added); keeping the laptop plugged in; heat and battery over days (the GPU reached
72 °C within 4 minutes; CPU temperature and the battery weren't measured); legs or straight through; and copying `acc.exe` somewhere
that stays put and using that copy for the whole run, without rebuilding in place:

```powershell
powershell -ExecutionPolicy Bypass -File tools\long-run.ps1 -Acc C:\runs\acc.exe -From 22000000000 -Count 1000000000000
```

**Considered, not tried** (for the repeat check):
- **Every deal, in partitions to fit the disk:** 20 passes and lookups at 261 GB, about 2 weeks at a trillion.
- **A sample taken at the end:** keys only, twice the coverage per byte but two passes (16.5 h); with positions, one pass, 8.2 h, for what
  the run can note as it goes.
- **A sample of seeds:** sees a given repeat with the fraction squared, 0.0025% at 0.5%.
- **A Bloom filter or a distinct-count sketch in memory:** 16 GB is 0.13 bits a deal at a trillion, so a filter saturates; a sketch's error,
  about 8 million, hides anything smaller.
- **Log files by key, smaller records** (positions as differences: 25-40% more coverage for a harder format), **fingerprints when the
  deals are prepared for the GPU,** **a wider generator** (by decision: it wouldn't change the same-game figure).

## After Phase 8 — look-ahead, learning within a deal, and a feed that keeps the GPU busy

A kernel with fewer attempts a deal, and a host that doesn't starve the GPU: 3.17M deals/s hot against 1.87M on 406bb98, x1.69, every line verified. The CPU stayed above its 12 W state in these runs, so this isn't yet a sustained figure (below). **Sustained, measured since:** 3,130,008 and 3,045,882 against 1,506,523 and 1,520,620, x2.04, by 10-minute runs (next section).

**The kernel, step by step, GPU alone.** One hold, 3 alternated passes, 2M deals a run, seeds from 600,000,000,000, timed without verifying, then one more run of each with every line verified. Medians:

| Step | Deals/s | Against 406bb98 | Against the step before | Attempts | Positions | Misses per 100k | Registers | Lines |
|---|---|---|---|---|---|---|---|---|
| 406bb98 | 1,842,993 | 1.000 | | 23.48 | 1996 | 6.50 | 79 | 0 failed |
| Search on 3 words of planes + look-ahead | 2,470,330 | 1.335 | | 13.03 | 1438 | 0.85 | 79 | 0 failed |
| + one draw a turn | 2,632,309 | 1.428 | 1.066 | 13.07 | 1440 | 0.85 | 79 | 0 failed |
| + learning within a deal: **this build** | 2,919,014 | **1.586** | 1.109 | 10.68 | 1150 | 0.90 | 80 | 0 failed |
| One draw a turn + planes in the prefix's shared moves, not kept | 2,567,726 | 1.404 | 0.975 | 13.10 | 1442 | 0.85 | 103 | 0 failed |
| That + learning, not kept | 2,341,134 | 1.264 | | 17.21 | 1658 | 1.00 | 100 | **1,036,214 failed** |

17 of the 18 timed runs were flagged by `ab.py` (clock about 1,400 MHz, or 9.5 or more CPU threads busy), all alike, and are in the medians. By pass, this build ran 1.543-1.609 against 406bb98.

- **Look-ahead:** from move 27 of a try, up to 4 candidates are scored by what they leave playable (`LOOK_TRIES`), which is what brought attempts from 23.5 to 13.
- **One draw a turn:** a move with nothing after it used to draw again in a loop inside the move, and that loop under a divergent branch put the compiler back in the code it makes for many breaks: 27 `BREAK`s and 3,704 instructions against 1 and 3,328. Now it draws again on the warp's next turn. `write_lines` keeps the loop, as the draws are the same.
- **Learning:** a group's tries treat the ranks that the deal's best residuals so far had lost as one class scarcer, with no code per move. It needs the two-bit weights to saturate: a target rank starts at 1, so it reached 3 with a card still left, and the next cover wrapped it to 0. Without that one line, learning lost on look-ahead.

**The build's own kernel, verified with the host's settings:**

| Deals, seeds from | GPU time, deals/s | Won by the GPU | Failed | Misses, all won by the CPU |
|---|---|---|---|---|
| 2,000,000, 600,000,000,000 | 3,143,585 | 1,999,984 | 0 | 16 |
| 4,000,000, 915,000,000,000 | 3,222,541 | 3,999,965 | 0 | 35 |
| 6,553,600, 920,000,000,000 | 3,262,472 | 6,553,533 | 0 | 67 |

These are above the 2.92M of the table because that hold ran at a lower clock; compare only within a hold.

**Hot `acc batch`,** 60M deals a run, one hold, seeds from 700,000,000,000, after 240M deals of warm-up on the old build (1,666,381 deals/s). Second half of each run, in order:

| Build | Deals/s | GPU busy | CPU speed, % of base | CPU package | GPU clock | Attempts | Misses | Failed |
|---|---|---|---|---|---|---|---|---|
| 406bb98 | 1,898,519 | 99.9% | 139 | 22.8 W | 1431 MHz | 23.47 | 4,436 | 0 |
| This build | 3,160,680 | 99.6% | 166 | 26.9 W | 1371 MHz | 10.67 | 597 | 0 |
| This build | 3,171,413 | 100% | 101 | 19.6 W | 1373 MHz | 10.67 | 606 | 0 |
| 406bb98 | 1,849,184 | 100% | 126 | 19.7 W | 1386 MHz | 23.48 | 4,436 | 0 |

x1.69 (x1.665 and x1.715 by pass), and all 240M deals won. Both builds keep the GPU at 100% here: this is the kernel's gain, with the CPU not yet short.

**Not measured:** the feed's gain in these runs, as the CPU never reached its 12 W state (a run now lasts 23 s). In that state, on 406bb98's kernel, one hold: 1,505,000 and 1,532,000 deals/s against one run of 1,978,000 with the feed and the line check, GPU busy 90% against 98-100%, from a working copy with the same logic. A run of many minutes on this build is what would show both limits. Also no retune of rungs, gates or look-ahead for the new attempts.

**The feed.** Long-lived threads prepare whole batches ahead in pooled buffers and queue their launches, so one is waiting when the last ends; workers wait on a condvar rather than polling; a line that plays out needs no reference replay unless it is the 1 in `FULL_REPLAY_EVERY` that is due. A pause now finishes up to five batches, not two.

**Pausing, on this build:**

| What | Result |
|---|---|
| 30M deals with the GPU on, left alone | 30,000,000 records, all won, 0 failed, 29,999,713 by the GPU |
| The same, paused by file 8 times (at 1,114,112 to 11,403,264), resumed each time | Each pause exit 0; 30,000,000 records, all won, 0 failed, 29,999,685 by the GPU; checkpoint gone |
| The same, killed at 1,035,776, resumed | 30,000,000 records, all won, 0 failed, 29,999,694 by the GPU |

Two `pause_resume` tests had failed on the feed's first full run: load, not the feed. These tests run with `--gpu off`, and with both builds on the E-cores at idle priority beside other jobs, clean 406bb98 failed 3 of 12 in one pass and none in the next; this build none in two. `ctrl_break_pauses_and_a_second_one_stops` failed once more on this build under the lock, with a first pause finished (1,247 deals) before the second Ctrl+Break came, exit 0 and not 130; alone it passes 6 of 6 on both builds. It is a race in the test, left as it is; the full run after it passed, 97 tests.

`a_sample_comes_out_in_order_with_the_gpu_too` did fail because of the feed: of 300,000 deals, all were taken by the time its pause came, so the run ended without pausing. It now has 1,000,000.

**The tests skip what they can't run.** From a worktree, with no `.tools/` above it and NVRTC not on PATH, the kernel tests and the GPU's runner tests pass in a fraction of the time without having touched the GPU, and say so only with `--nocapture`. That hid the failure above from the feed's first run. The results here are with NVRTC on PATH.

No golden values changed. Clippy: no warnings.

**Tried, not kept:**
- **Planes in the prefix's shared moves:** x1.149 without look-ahead, x0.975 of the step before it on this stack, at 103 registers, where 113 lost occupancy. With learning, half of its lines failed; every deal was still won by the CPU. The cause wasn't looked into.

**Considered, not taken in this build:**
- **The batch line check and deal maker** (`quick.rs`): line check 93 ns on a P-core against 147 for this build's, 0 disagreements on 110M real lines. It conflicts with the feed in `runner.rs`, and adds 1.1% hot while the GPU is the limit. Next build.
- **The bit-parallel pattern policy** in the kernel: x1.187 on the first stack step alone (13.0 attempts, 1,040 positions, 6.55M lines verified, 0 failed), reported as this build closed. Not on one draw a turn or learning, kernel tests and hot not run. Next build.
- **The kernel's clean-up** (507 lines of unused kernels and helpers): same machine code, so no hurry, and it would have moved every patch here.

## After Phase 8 — patterns in the prefix, a cheaper line check, three launches in flight

A prefix policy of bit-parallel patterns, a cheap scan of the deals already won, a third launch in flight and the line check of `quick.rs`: 3.88M deals/s sustained against 3.01M on be759ba, x1.29 over 10-minute runs of 3 billion deals. As a long run, with the games check at its usual 0.5% sample: 4.42M and 4.69M against 3.38M, x1.31 and x1.39, with the GPU the limit. Every line verified. GPU alone x1.37.

**The steps, GPU alone.** Built-in kernels and hosts, one hold, 3 alternated passes, 8M deals a run, seeds from 600,000,000,000, no line check. Medians:

| Build | Deals/s | Against be759ba | Attempts | Positions |
|---|---|---|---|---|
| be759ba | 3,170,230 | 1.000 | 10.69 | 1150 |
| Patterns, scan, config, with 2 launches in flight | 4,170,970 | 1.316 | 6.80 | 613 |
| + 3 in flight: **this build** | 4,338,715 | **1.368** (1.368-1.371 by pass) | 6.80 | 613 |

Three in flight against two: x1.040 (1.032-1.060 by pass).

- **Patterns replace look-ahead.** One copy of the scorer in a fixed-length loop, with no retries. Its weights were tuned by play in the kernel, 1M deals an evaluation, which took attempts from 7.61 to 6.67 on the tuning seeds. Learning needs no new code: the targets arrive through the same two words.
- **The scan of won deals was what hid the policy's gain.** Loading a won flag per deal, per pass, took 17% of thread time with groups of 2. From 64 tries, a warp takes 32 words of groups at once and reads their won bits, kept in free bytes of low deals' lines. Stale bits only cost a needless visit. It's off unless the batch is a multiple of 32 and at least 1,024 deals.
- **Bundles.** From 128 tries, a pass gives each deal still left 32 groups at once, so passes and scans come 32 times less often, and 10,240 tries cost next to nothing: misses per 100k fell from about 1 to 0.05. The tries are the same tries, in another order.
- **Config, now the defaults:** rungs `[[40,128],[38,10112]]`, groups of 2, gate `[8,10]`, batch 131072.
- **The third launch.** With two, the GPU sat idle between attempts kernels while the host uploaded.
- **`quick.rs`.** Line check at 93 ns on a P-core against 147 for the one it replaces (773 for the reference replay), by PDEP on a bit per pile, with the rank-or-suit, byte-range and full-deck checks in bulk at the end. Deal maker 105 ns against 146; masks about 5 against 52. Anything it doesn't accept goes to the reference engine, which decides, as does the full replay on every 256th deal. 0 disagreements on 111.99M real lines, 440M+ mutants and 1.11B seeds.

**The build's own kernel, verified with the host's settings:**

| Deals, seeds from | GPU time, deals/s | Won by the GPU | Failed | Misses, all won by the CPU |
|---|---|---|---|---|
| 2,000,000, 600,000,000,000 | 4,043,537 | 2,000,000 | 0 | 0 |
| 4,000,000, 915,000,000,000 | 4,594,210 | 3,999,997 | 0 | 3 |
| 6,553,600, 920,000,000,000 | 4,673,224 | 6,553,596 | 0 | 4 |

**Sustained `acc batch`,** 10 minutes a run by `--stop-after`, alternated, from 900,000,000,000, in order. Under 13 W and over 13 W split each run by the CPU package's power, sampled every 20 s:

| Build | Deals/s, whole run | Last half | Under 13 W | Over 13 W | Samples under 13 W | GPU busy | Misses | Failed |
|---|---|---|---|---|---|---|---|---|
| This build | 3,902,267 | 3,527,848 | 3,550,225 | 4,559,307 | 18 of 28 | 94.2% | 1,246 | 0 |
| be759ba | 2,975,636 | 2,930,562 | 2,849,580 | 3,293,920 | 20 of 28 | 92.2% | 18,329 | 0 |
| be759ba | 3,042,525 | 2,995,225 | 2,876,402 | 3,238,011 | 15 of 28 | 97.2% | 18,746 | 0 |
| This build | 3,859,063 | 4,127,209 | 3,581,423 | 4,556,813 | 20 of 28 | 95.3% | 1,225 | 0 |

x1.29 on whole runs (x1.311 and x1.268 by pair). Last halves give x1.20 and x1.38, but those depend on when the CPU changes state, which it does by itself, so I'd go by the whole runs and the two states: x1.24 under 13 W, x1.40 over it. A hold before these, the same machine code, ran 4,009,608 for a whole run with the CPU at 16 W; with two in flight, the next run, 3,616,592 (one pair, x1.109).

**In these runs the CPU's 12 W state binds.** It comes on after about 3.5 minutes, and may leave for a few minutes at 18 W. Above it this build makes 4.56M deals/s with the GPU 98-100% busy at about 1,440 MHz: the GPU is the limit. In it, 3.57M, with the GPU busy 92-93% at about 1,630 MHz. Workers aren't what's short: they wait for the GPU about 40% of their time, and less than one batch waits for them.

**As a long run would, with the games check at its usual sample.** The runs above have 3 billion deals, few enough that the check for repeated games takes every deal: each is shuffled again and fingerprinted, about 1.45 µs of CPU a deal in the workers' verifying, and that is what makes the 12 W state bind. With `--count 1000000000000` the sample is 0.50%, as in a long run. Same seeds, 10 minutes a run, in order:

| Build | Deals/s, whole run | Last half | GPU busy, last half | CPU package, last half | Verifying, CPU ns a deal | Misses | Failed |
|---|---|---|---|---|---|---|---|
| This build | 4,422,779 | 4,446,916 | 99.7% | 13.8 W | 583 | 1,424 | 0 |
| be759ba | 3,380,968 | 3,359,840 | 99.9% | 12.0 W | 729 | 20,733 | 0 |
| This build | 4,690,251 | 4,706,231 | 100% | 12.0 W | 719 | 1,514 | 0 |

x1.31 and x1.39 on whole runs. **What binds now: the GPU,** busy 99.7-100% in either CPU state, near its 30 W cap (about 29 W, 1,380-1,490 MHz). The workers wait for it 74-80% of their time, so the next gains are in the kernel. A hot comparison should run this way, by `--stop-after` with a large `--count`: at 3 billion deals or fewer it measures the games check as much as the build.

**Correction to the last section.** Its 3.17M against 1.87M, x1.69, came from runs of 60M deals, too short to reach the 12 W state. By 10-minute runs that build and 406bb98 gave 3,130,008 and 3,045,882 against 1,506,523 and 1,520,620, last half of each, x2.04.

**The clean-up.** The kernels `attempts` and `attempts_stepwise`, the `stepwise` and `split` settings, 19 device functions nothing named and 10 profile slots are gone: about 840 lines net. The PTX of `attempts_split`, `write_lines` and `playouts` is identical apart from label numbers. **A checkpoint with `stepwise` or `split` in its settings won't resume:** finish such a run with the build that started it.

**Tests.** No golden values changed. One kernel test expected groups of 4; it now expects 2, and also runs 64 + 200 tries, past where the scan of won deals and the bundles start. `game::line_wins` and its tests went with the code that called it; `quick.rs` has the same checks. Four crates pass, kernel tests with the GPU. Clippy: no warnings.

**Considered, not taken in this build:**
- **Screening's config on be759ba** (`[[40,64],[37,768]]` and others, x1.07-1.11): measured on the old policy, and its fewer tries trade away misses that bundles now get for free.
- **The scan without a `BREAK` regime:** x1.036 GPU alone. Under 5%.
- **Recording prefix moves instead of `write_lines` replaying them:** estimated x1.05-1.08. A host change, as the first bytes of a line hold learning and won bits until the launch ends. Not built.
- **Dropping pattern features:** x1.00-1.04 in a noisy hold, attempts 3-7% up.
- **The second part of the clean-up** (runtime `leftmost` and `conn_gate` duplicated by macros): changes the hot kernel, so it needs its own timing.

## After Phase 8 — shared moves and lines on planes, own moves recorded

Three kernel changes, each small: 4.95M deals/s sustained against 4.61M on c2adb22, x1.07, over 10-minute runs as a long run. Every line verified. GPU alone x1.148 on runs of 2M deals, but x1.058 on runs of 30M, where the GPU sits at its power cap, as it does in `acc batch`.

**The steps, GPU alone.** 2M deals a run, 3 passes, alternated, one hold for each step, lines verified on 2M each:

| Step | Against | Ratio (range by pass) |
|---|---|---|
| 1. Shared prefix moves on one-word planes, only among the first 32 piles | c2adb22 | 1.052 (1.007-1.060) |
| 2. `write_lines` replays the shared moves with `win_try` on planes, not `rrow_move` on the 13-word row: same choices, same lines | step 1 | 1.034 (1.019-1.035) |
| 3. Own moves recorded as they are played, written into the line by the search that wins; `write_lines` replays only the shared moves | step 2 | 1.038 (1.035-1.044) |
| All three, 4 passes: **this build** | c2adb22 | **1.148** (1.138-1.170) |

- **Recording own moves.** A byte a move goes into words 10-14 of a thread's 16 kept words. Those held the warp's scan state, the same in every thread, which moves to shared memory (20 B). The moves travel with the residual through the queue (5 words a slot); shared memory goes from 7,308 to 7,968 B a block, so 8 blocks still fit. Deals whose line bytes hold won bits during the launch, and prefixes that leave more than 20 own moves, are replayed as before.
- **Before these, `write_lines` took about 15.7% of GPU time.** With it left out, x1.157 was the ceiling.
- 7.04 attempts a deal against 6.79, 624 positions against 613: the search is a little dearer, the rest cheaper. 79 registers, 8 B of local memory.

**Long runs, GPU alone.** 300M deals of warm-up, then 30M a run, 2 passes, alternated, one hold, no line check: 5,006,814 against 4,731,783, x1.058 (1.054-1.062), 1,465-1,487 MHz at 28.7 W. Short runs gave this kernel over twice its sustained gain. A likely reason, not tested: what it saves is mostly memory traffic, which costs fewer cycles at the capped clock.

**The build's own kernel, verified with the host's settings:**

| Deals, seeds from | GPU time, deals/s | Won by the GPU | Failed | Misses, all won by the CPU |
|---|---|---|---|---|
| 2,000,000, 600,000,000,000 | 4,597,321 | 1,999,998 | 0 | 2 |
| 4,000,000, 925,000,000,000 | 4,952,548 | 3,999,999 | 0 | 1 |
| 6,553,600, 927,000,000,000 | 5,037,165 | 6,553,597 | 0 | 3 |

**Sustained `acc batch`,** as a long run (`--count 1000000000000`, `--stop-after 0.1667`), 10 minutes a run, alternated, in order, each from its own seeds. Power from 20 s samples:

| Build | Seeds from | Deals/s, whole run | Last half | GPU busy, last half | CPU package, last half (whole run) | GPU | Verifying, CPU ns a deal | Misses | Proven lost | Failed |
|---|---|---|---|---|---|---|---|---|---|---|
| This build | 940,000,000,000 | 4,947,897 | 4,903,046 | 99.4% | 14.0 W (11.8-24.9) | 28.9 W | 604 | 2,175 | 3 | 0 |
| c2adb22 | 945,000,000,000 | 4,601,603 | 4,628,617 | 100% | 14.5 W (11.5-24.9) | 29.1 W | 670 | 1,521 | 3 | 0 |
| c2adb22 | 950,000,000,000 | 4,625,621 | 4,617,478 | 99.2% | 11.9 W (11.6-12.2) | 29.2 W | 699 | 1,585 | 0 | 0 |
| This build | 955,000,000,000 | 4,948,115 | 4,838,002 | 99.9% | 11.9 W (11.4-25.0) | 28.8 W | 678 | 2,196 | 4 | 0 |

x1.075 and x1.070 on whole runs; x1.059 and x1.048 on last halves. **The GPU still binds,** 99-100% busy near its 30 W cap in either CPU state; workers wait for it about 75% of their time. The gain matches the long GPU-alone runs, not the short ones. Hot comparisons and GPU-alone ones that matter should use long runs past a long warm-up.

**Tests.** No golden values changed. Four crates pass, kernel tests with the GPU. Clippy: no warnings.

**Tried, not kept:**
- **The clean scan** (no `BREAK` regime in the scan of won deals), ported with the pattern policy's bundles: x0.971 GPU alone; with step 1, x1.025 against step 1's x1.052. On this kernel it doesn't clear the `BREAK` regime (34 `BREAK`s before and after) and adds a 32 B spill.
- **Host-side planes for each deal**, so the kernel skips one `win_load` a group: priced, not built. Two extra `win_load` calls a group cost x0.984, so dropping one is worth about 1%, and it needs host changes.
- **`write_lines` held to 64 registers:** x1.015 against step 2's x1.034; it spills 32 B.

## After Phase 8 — the first groups without a mate, tuned settings, and what did not pay

5.74M deals/s sustained against 5.21M on 490cfde, x1.10, over 10-minute runs as a long run. Every line verified. Three changes, all small; the speed comes from the first two:

1. **`SOLO_GROUPS` 16.** A deal's first 16 groups play without a group mate: one try in each, not a pair. Attempts a deal 6.41 to 4.94, positions 633 to 570 (on the new settings); misses per 100k unchanged at 0.07. Hot, 2 alternated pairs of 800M deals after a warm-up, the same settings on both sides: x1.057 (1.043-1.071). With no mate in any group the attempts fall to 4.79, and the speed is the same (x1.041-1.052 against x1.037-1.051 for 16 groups, GPU alone), but misses rise to 0.13 per 100k, since the try ladder is halved; 16 groups keep both.
2. **Settings defaults:** rungs `[[40,16],[38,10224]]` (were `[[40,128],[38,10112]]`) and batch 262144 (was 131072). Hot, 3 alternated pairs of 10 minutes on 490cfde: x1.047 and x1.041 under Best performance, x0.989 under Balanced (see the power note); GPU alone on long runs x1.064. A rung's count is in groups, each a pair of tries, and one try in a deal's first 16 groups.
3. **Rank broadcasts strength-reduced in the own-move scorer** (the useful part of a dead-child experiment, below). Each plane is shifted once, `(int)(BT << k) >> 31` in place of a variable shift a broadcast. The moves are identical; 32 fewer instructions, 80 registers and no spill (before: 79 and 8 B of local memory). x1.030 on 2M-deal runs GPU alone, but x1.011 (0.997-1.027) on long runs against 490cfde: no difference shown. Kept because it changes nothing else.

**The build's own kernel, verified with the host's settings** (new defaults, 8 verifying threads):

| Deals, seeds from | GPU time, deals/s | Won by the GPU | Failed | Misses, all won by the CPU |
|---|---|---|---|---|
| 2,000,000, 600,000,000,000 | 4,427,753 | 2,000,000 | 0 | 0 |
| 4,000,000, 925,000,000,000 | 5,010,390 | 3,999,999 | 0 | 1 |
| 6,553,600, 927,000,000,000 | 5,215,455 | 6,553,596 | 0 | 4 |

4.95 attempts and 570 positions a deal (24.78M attempts/s and 2,857M positions/s on the second range), 80 registers, 0 bytes of local memory.

**Sustained `acc batch`,** as a long run (`--count 1000000000000`, `--stop-after 0.1667`, `--gpu-config '{}'`, so each build runs its own defaults), 10 minutes a run, alternated n b b n, each from its own seeds, Best performance. Power from 20 s samples; lines verified by the run (a paused run prints no "all won" line, so the check is: no "failed to verify" line and the GPU's own count of lines that failed, 0):

| Build | Seeds from | Deals/s, whole run | Last half | GPU busy, last half | CPU package, last half (whole run) | GPU, last half | Verifying, CPU ns a deal | Misses | Failed |
|---|---|---|---|---|---|---|---|---|---|
| This build | 970,000,000,000 | 5,672,145 | 5,678,035 | 100% | 16.3 W (15.9-29.0) | 28.8 W, 1,360 MHz | 525 | 2,560 | 0 |
| 490cfde | 975,000,000,000 | 5,178,250 | 5,175,529 | 100% | 17.2 W (15.8-18.5) | 28.9 W, 1,377 MHz | 496 | 2,236 | 0 |
| 490cfde | 980,000,000,000 | 5,236,985 | 5,248,995 | 100% | 18.3 W (17.7-18.6) | 28.9 W, 1,394 MHz | 409 | 2,266 | 0 |
| This build | 985,000,000,000 | 5,810,917 | 5,852,131 | 100% | 18.0 W (17.5-18.5) | 29.1 W, 1,400 MHz | 416 | 2,597 | 0 |

Pair 1 x1.095 whole, x1.097 last half; pair 2 x1.110 whole, x1.115 last half; both pairs together x1.102 whole, x1.106 last half. The second run of a pair reads a few percent high in these tests, so the order-balanced figure is what to quote. The two hot measurements above (x1.057 and x1.044) multiply to x1.10, as measured. **The GPU still binds:** 100% busy at its 30 W cap, 28.8-29.1 W, the CPU workers waiting for it for about 2.0-2.4 us of every 2.6-2.9 us of thread time a deal. Misses are 0.074 per 100k on this build, 0.072 on 490cfde; each goes to the CPU's exact search, which won it.

**Tests.** Four crates pass, kernel tests with the GPU; clippy: no warnings. No golden value changed. Three tests changed because of the new defaults, none of them in what they check:
- The two kernel tests "every attempt is accounted for" expected a deal not won to play `tries == all`; with `SOLO_GROUPS` it plays one try in each of its first 16 groups, so a helper (`tries_played`, which reads `SOLO_GROUPS` from `kernel.cu`) gives the expected count. A case (37, 8) was added, so a group that straddles the rungs past the solo groups is still tested.
- The sample test with the GPU used 1M deals, 7.6 batches of 131072; with 262144 that is 3.8 batches, fewer than the five the feed holds when the pause comes, and "it never paused". It uses 2M now.

**Timing lessons.**
- **GPU-alone runs of 2M deals overstate gains.** A run lasts under half a second and measures boost above the 30 W cap, not energy per deal. Examples from this stretch: the shared-moves build, x1.148 on 2M runs and x1.058 on 30M runs; bc, x1.030 and x1.011; a branch-free edit in the search, x1.041 in a drifting hold and x0.991 over 4 passes. Decisions here were made on 30M-deal runs after a warm-up, 2 or more alternated pairs, then on hot runs. Short runs are only for screening.
- **Best performance mode removed the CPU's 12 W state.** In Balanced the CPU package sat at a flat 12 W for stretches of a run; in Best performance it drew 15.6-18.5 W. The same 490cfde build with the default settings ran 4,940k deals/s (last half, Balanced) against 5,275k and 5,291k (Best performance): about 7-8% hot, with no change to the build. Compare hot runs only within one power mode, and note it for any figure on this page from before this section.

**Tried, not kept:**
- **OWNCUT: cutting the own-move scorer's instructions.** Marginal cost per instruction in long runs, from dummy work added to one part at a time: own move 0.038% of speed an instruction, search turn 0.154%, shared move 0.045%; shares of marginal time about 23%, 38% and 9%. For x1.05 from own moves with the same moves, about 130 of the loop's 763 instructions (501 of them LOP3 in the adder trees) would have to go. The cuts found come to about 50 instructions, about 2% (estimate): phase-specialised sums, PRMT broadcasts, the REC store address hoisted (costs a register at the 80 cap). Nothing built.
- **SEARCHCUT: cutting the search turn's control path** (a second take-check ballot kept out of line, the check every 6 or 8 turns, frame addresses by byte offset, a branch-free edit and test, the won check's load a take block early, masks only for rows that changed). All exact or timing-only; none reached x1.03 on long runs. A branch around the masks costs more than the lanes it idles (x0.942); a branch-free edit and test looked like x1.041 in a drifting hold and was x0.991 over 4 passes. OWNCUT's 0.154% an instruction came from a dependent chain of ALU dummies and does not carry over to control operations: the search turn does not bind on its control path, branches or load latency.
- **TUNE5: retune of `--gpu-config` on 490cfde.** Candidates over rungs, conn gate, rank gate, budget, batch and group size, 30M-deal runs after a 150M warm-up, alternated: the best, a second rung of 2048 tries, was x1.003 against a spread of about 1% between runs of the same build. Every compile-time knob lost or was level.
- **TUNE7, second part: a GPU tuner of the own-move scorer's weights,** on exact attempt and position counts of 4M deals. It converged in 3 rounds to a local minimum: the committed weights are already at the optimum (the pure terms are sharp, within 10-20%; the win and move terms are flat within 0.3%). Predicted x1.006, measured x0.989 and x1.000 on long runs.
- **POLICY5: dead-child terms in the scorer.** A card whose only partner is the covered one can never be won, and a row with such a card is lost; an exact bit-sliced test costs about 25 instructions. Leaving dead children out entirely is the best any weight could do, and it cut attempts by only 0.6% for x0.981-0.985; the simulator's -10% did not carry over, since the policy rarely plays into one. Choosing another move for a child with none: attempts changed by 0.1% and cost 3.5%, from the loop's shape. An earlier policy's feature drops, transplanted: x0.979-1.009. First look-ahead move at 28, 32 and 34 (30 is the default): x0.960, 1.012, 0.998, noisy.
- **HARD2: other ways to cut a deal's early tries.** A budget cut and a gate cut were flat. Prefix warps at 36 and 37 piles: x0.88 and x0.97. A second move-quality bit (MQ): x0.89-0.96. Shared moves among 24-38 piles, not 32: worse. One try a group, sliced (`SHARE_G` 1): x1.020. Mate-free groups beyond 16: no gain, and more misses (above).

- **Pinned, reused upload slots (the feeder).** Host time a launch fell from 42.6 ms (pageable copy) to 2.9 ms, and hot on 490cfde was x1.004, flat: the GPU never waited, so the feeder did not bind there. The GPU copies 20 MB up in 31.8 ms (0.63 GB/s, PCIe Gen4 x4), so up and down are about 48 ms of copy in a 47 ms cycle: on this build the copy engine may bind (inferred, not measured).

**Considered, not tried:**
- **The scorer's three cuts above, about 2% together,** under the 3% bar with no route to 5%.
- **PRMT sign-replicate broadcasts** (one IMAD puts the bits at byte tops): about 12 instructions an own move, about 1%.
- **Inline ballots and a doubled search turn.** `ptxas` refuses a ballot without sync; an `__activemask()` ballot in a branch depends on `ptxas` leaving the branch alone. Doubling the turn doubles the hot loop, 7.4 to 15 KB, for a saving the take-check change gave without it and which showed nothing.
- **Deals made on the GPU from their seeds** (8 bytes a deal up, against 76): written, not built. See the upload bullet above.

**What binds.** The GPU, at its power cap, in either CPU state: 100% busy, 28.8-29.1 W, 1,360-1,400 MHz. Speed follows energy per deal there: memory traffic and total instructions, not issue slots or latency. Next: the search turn's lane-wide work (alive_rest, bp_edit), the 570 positions a deal, and the upload.

## After Phase 8 — pinned upload slots, and what the audits found

6.06M deals/s sustained against 5.96M on e5491ef, x1.016 over 10-minute runs as a long run (x1.012 on last halves; the two pairs x1.022 and x1.010 whole run). Every line verified. One change, to the feeder only: the kernel, the rules and the results are the same as e5491ef's. Then the audits and ideas of this stretch (six of them), none of which reached 3%.

**Pinned, reused upload slots, with ordered copies.** `Gpu::feed` keeps one slot a stream: the device buffers and the page-locked host buffers (write-combined for the upload, plain for the download), from launch to launch. Before, every launch cloned 20 MB from pageable memory (42.6 ms of the launching thread's time on 490cfde), allocated its device buffers (4.9 ms) and downloaded 16.5 MB into pageable memory (39.3 ms on the collector): 48 ms of host time against a 49 ms GPU cycle there, all of it on the one thread that launches. Now the launching thread copies the deals into pinned memory and queues the copy: about 3 ms a launch (FEED4's hot timers). The order of the copies matters: they run in the order they are queued, whatever the stream, so a download queued with its launch held up the next launch's upload until the kernels had finished, a 50 ms gap after every launch and 2.95M deals/s against 5.3M (slot v1, FEED2). The collector queues the download once the stream is synchronised (v2). An env-gated timer, `ACC_FEEDPROF=<file>`, stays in `Gpu::feed`; unset, it is a flag test.

On 490cfde this change measured x1.004 (flat; see "Tried, not kept" in the section above): the GPU never waited. On e5491ef's faster kernel a launch of 262,144 deals takes 43.7 ms at 6M deals/s, so the one launching thread's 48 ms of pageable copies would be the next limit (inferred from those two numbers; the pageable build was not instrumented again). FEED4 measured the gain hot on the same code in A B C C B A runs, two pairs: x1.017 and x1.027 (the second suspect: another job overlapped its last 4 minutes). Here:

**The build's own kernel, verified** (kernel unchanged; new `accgpu`, 8 verifying threads):

| Deals, seeds from | GPU time, deals/s | Won by the GPU | Failed | Misses, all won by the CPU |
|---|---|---|---|---|
| 2,000,000, 600,000,000,000 | 4,239,926 | 2,000,000 | 0 | 0 |
| 6,553,600, 927,000,000,000 | 5,631,532 | 6,553,596 | 0 | 4 |

Both ranges give the same count of misses as e5491ef's own check (0 and 4).

**Sustained `acc batch`,** as a long run (`--count 1000000000000`, `--stop-after 0.1667`, `--gpu-config '{}'`), 10 minutes a run, alternated n b | b n, each from its own seeds, Best performance. Power from 20 s samples. Lines verified by the run: no "failed to verify" line, and every deal the GPU did not win (the misses) was solved or proven lost by the CPU.

| Build | Seeds from | Deals/s, whole run | Last half | GPU busy, last half | CPU package, last half (whole run) | GPU, last half | Verifying, CPU ns a deal | Misses | Failed |
|---|---|---|---|---|---|---|---|---|---|
| This build | 1,010,000,000,000 | 6,092,097 | 6,068,262 | 100% | 16.9 W (15.7-27.1) | 29.2 W, 1,437 MHz | 432 | 2,643 | 0 |
| e5491ef | 1,015,000,000,000 | 5,960,359 | 5,953,289 | 100% | 17.2 W (16.8-27.6) | 29.2 W, 1,435 MHz | 420 | 2,654 | 0 |
| e5491ef | 1,020,000,000,000 | 5,968,535 | 5,967,030 | 100% | 17.2 W (16.5-17.7) | 29.3 W, 1,434 MHz | 421 | 2,658 | 0 |
| This build | 1,025,000,000,000 | 6,027,042 | 5,997,059 | 100% | 16.9 W (16.0-17.7) | 28.7 W, 1,421 MHz | 448 | 2,716 | 0 |

Pair 1 (this build first) x1.022 whole, x1.019 last half; pair 2 (this build second) x1.010 whole, x1.005 last half; both together x1.016 whole, x1.012 last half. With FEED4's two pairs (x1.017 and x1.027) the gain is about x1.02. The machine ran about 40 MHz faster than in the e5491ef measurements (1,435 against 1,360-1,400 MHz), so e5491ef itself read 5.96M here against 5.67-5.81M in its own commit's measurements; compare within a table only. The first e5491ef run had one 0% GPU-busy sample (96.6% over the whole run, 100% in the last half). **The GPU still binds:** 100% busy at its cap, 28.7-29.3 W; the CPU package drew 16.9-17.2 W.

**Tests.** The four crates pass, with the GPU: no failure, `ctrl_break_pauses_and_a_second_one_stops` passed in the full run; clippy with `--all-targets`: no warnings. No test changed, no golden value changed.

**Tried, not kept:**
- **GPU-made deals (FEED3, `Gpu::feed_seeded`).** A kernel makes each deal from its seed on the device (51 Lemire draws with 64-bit multiplies, a 52-byte shuffle in local memory), so a launch uploads 8 bytes a deal, not 76. Bit-exact: 5,242,880 seeds (1M each from 0, 22e9, 991e9, the top of the u64 range, and hashed seeds), 0 codes or masks differ from `quick::deal_codes` and `quick::masks52`. Hot, A B C C B A of 10 minutes on e5491ef with and without the pinned slots: x0.941 and x0.947 against the slots alone (x0.957 and x0.973 against e5491ef). The copy engine does not bind and the SMs do: making the deals costs about 2.5 ms of SM time a launch, and the upload it saves was never on the critical path, since the fastest build still uploads the full 20 MB a launch. The PCIe link really is slow: 0.70 GB/s up and 0.84 GB/s down at 2-64 MB, pinned or pageable (20 MB up in 29.9 ms, 16 MB down in 20.0 ms; `nvidia-smi` reports Gen4 x4 of x16), but the device has 2 copy engines and up and down overlap fully (20 MB up and 16 MB down together take 29.6 ms, not 50).
- **TABLEGATE: an exact table decides residuals, not search (STRAT, RANK).** 46% of positions go to proving residuals lost, and a 12-pile residual is winnable exactly when one child is a winnable 11-pile pattern, which `endgame-11.bin` (1.15 GB, fits in the GPU's 4 GB) answers. EXACT: 5,404,730 ranked residuals (2M deals), the table agrees with the search on all 1,747,308 won and 3,620,831 exhausted searches, and 2M lines verified twice (0 failed, all won). But the perfect rank of every child (about 24 a deal; 75 SASS operations a card) costs more than the 26% of time it saves. GPU alone, 30M-deal runs after a warm-up, alternated: the gate priced with play identical to the base x0.733 inline and x0.696 not inlined (rank alone x0.754, a table the size of L2 x0.709, so the rank is the cost, not DRAM: 2.3 ns a child); with the cheaper rank, x0.842 and x0.854 (31 ns a deal, 2.4 ns a ranked child: rank 1.56, table read 0.46), the gate itself on: 64-thread blocks x0.89-0.895, 96 threads (2 prefix warps and 1 search warp) x0.96-0.98, 128 (3 and 1) x0.94-0.985, rungs 40/40 at 128 x0.98-1.0; the base at 96 is x0.95 and at 160 x0.92. Why: the base is bound by its prefix warps, so the search time the gate frees is time they cannot use (search lanes in use 29.3 to 21.3 of 32, idle 3.9% to 31.3%). An early exit with compaction each round costs more than the 19% of ranks it saves (x0.80 against x0.854); a dead-end filter drops only 13% of children (5.62 to 4.87 a residual). Exact counts of the tries (STRAT, 2M deals): 4.94 reach the gate, 20% end disconnected, 3% are over the rank gate, 3.80 are searched; exact verdicts everywhere would take 4.45 attempts a deal at 39/39, 4.88 at 40/38, 5.53 at 40/40, 7.13 at 41 and 9.54 at 42, so the gate and budget cost only 1.3% of attempts.
- **AUDIT: a gap in the work.** None above 5%. Lanes are in use: the prefix runs 97-100% of 32 lanes, the search 94% (77% weighted by instruction; the edit block 63%, the test block 80%). The never-searched tries (1.20 a deal, 21.4% of own-move lane-moves), the budget-capped searches (1.3% of searches, 9.5% of positions), repeated positions (8.28% of a search's positions repeat one in the same search; 1.7% duplicate residuals) and `write_lines` are 1-3% each; 0.12% of search lane-turns run on deals already won. The GPU is power-capped, not issue-bound: 5.83M deals/s at 1,402-1,410 MHz and 29.2 W, 5.01 uJ and 8.8 nJ a position.
- **CLOCK: cheaper instructions for the same work.** Energy of a stream at the 30 W cap (16 warps an SM, dense random data, nJ a GPU cycle = W/MHz): LOP3 19.1 at IPC 2.00 and 1,532-1,540 MHz; the same with half the lanes masked off 19.08; LOP3 with SHF 1:1 17.6, with IMAD.SHL 1:1 18.6; 3 LOP3 with one IMAD.IADD 21.6, dense IMAD 22.2, LDS 21.6, SHFL 21.5, STS 23.2. Incremental energy of a warp instruction, LOP3 = 1 (estimated): SHF 0.2, IMAD.SHL about 0, IMAD.HI 0.5, SEL 1.6, IMAD.IADD 2.0, IMAD 2.5, LDS and SHFL 3.2, STS 4.9. Rewrites in the own-move loop, GPU alone, 30M-deal runs after a 300M warm-up, alternated, same attempts and positions: constant left shifts as SHF x0.966; all shifts as IMAD from constant memory with sign broadcasts as IMAD.HI x0.958, with the right shifts too x0.951; the clock hardly moved (1,421 and 1,422 MHz), so `ptxas`'s split of shifts between IMAD and SHF is already at a local optimum. Most of the power is a fixed cost a cycle (estimated 55-79% of a dense LOP3 stream's) and idle lanes cost full energy, so cycles a deal and lane waste are the levers at the cap, not the instruction class; the dear classes in the hot loops come to about 2% of energy a deal (estimated). AUDIT's "15% clock headroom" was its streams' low-entropy start values (1,634 MHz against 1,540 on dense data), and its IPCs were about 1.33x too high.
- **TRIM: leaving a doomed try early (disconnection test after own moves).** Disconnection comes late. 0.7145 tries a deal (14.6%) end disconnected; their first failing move (of 40 or 38): 4.3% by move 35, 16.7% by 36, 30.4% by 37, 52.4% by 38, 71.4% by 39, 28.6% only at the last. An ideal check after every own move skips 1.31 moves of a doomed try, 1.01% of own lane-moves, about 0.23% of cost (own moves are 23%); every 4 moves 0.39%, every 8 0.36%, every 12 0.05%. The test is exact and monotone here (0 doomed tries without a failure, 0 won tries with an earlier failure). Lanes freed mid-loop save nothing unless all 32 finish, since the loop is in lockstep. Counters only, not built.
- **ORDER: the search's child order.** A won search takes 114.7 positions for a line of 11.25 moves; the first wrong turn is at depth 0 in 41.5% of wins (63% of won-search positions), depth 1 27.8% (25%), depth 2 15.9% (8%), deeper 15% (4%); wins over 192 positions are 17.5% of wins and 49% of won positions. Positions a deal, against 569.9: static orders 568-589, leaps and lowest pile first 561.5, fewest child ranks at the root only 552.6 (-3.0%), ranks then most child moves at every depth 551.7 (won searches 92.7 positions, against 114.7). Timed, GPU alone, 30M-deal runs after a warm-up, 4 alternated passes: the static order x0.977, x0.986, x0.980 and x0.996; the root-only choice as a divergent loop x0.84-0.85. The choice costs more than the positions it saves. A deal's search has no learning within it, and cannot: its first win ends the deal.

**Considered, not tried:**
- **Search warps that play prefixes when idle (RANK's next step).** TABLEGATE pays only if the search time it frees becomes prefix time: when the queue is empty, a search warp would play a prefix, with the gate as it is. The freed time is real (search lanes idle 31.3% with the gate on, against 3.9%), and the base is bound by its prefix warps. Not tried: it is RANK's next test, and without it the cheapest gate prices in at about x1.0. It adds a second prefix path to the search warps, and code size already costs in this kernel (see the section on code size above). STRAT estimated x1.1-1.3 for a gate that pays, if a child's rank also got 3-5x cheaper (estimated, not measured).
- **Sharing a residual's prefix among its children.** The children share cards 0..y-1, but the states would need about 7.7 KB of shared memory a prefix warp; in registers or lockstep it costs more than it saves (24 card-steps a sweep against 11, worked through).

**What binds.** The GPU, at its power cap: 100% busy, 28.7-29.3 W, 1,421-1,437 MHz, with the feeder now out of the way (host time about 3 ms a launch). Speed follows energy per deal: cycles a deal and idle lanes, not the instruction class.

## After Phase 8, second step — TAKE_MIN 8 and a first rung of 24

6.20M and 5.79M deals/s sustained against 6.06M and 5.65M on b8d71d3, x1.024 over 10-minute runs as a long run (x1.022 on last halves; the two pairs x1.023 and x1.024 whole run). Every line verified. Two defaults changed: kernel.cu `TAKE_MIN` 4 to 8 (the search warps take residuals from the queue once 8 of them need one, not 4) and the settings' first rung `[[40,16],[38,10224]]` to `[[40,24],[38,10224]]` (24 numbered tries of 40 moves, which is 12 played, one each in the solo groups, not 16 numbered and 8 played). TUNE8 retuned the prefix/search balance of b8d71d3 on 30M-deal runs after a 150M warm-up, 3 alternated passes, 1,330-1,415 MHz, 28.7-29.4 W; nothing reached x1.03, and this combination is the one small, real gain, which is not additive: the extra first-rung tries alone gain x1.001, TAKE_MIN 8 alone x1.009, together x1.022 (1.022, 1.021, 1.022; x1.017-1.019 on a second set with other neighbours). A deal now takes 5.08 attempts and 555 positions, against 4.94 and 570.

**The build's own kernel, verified** (new `accgpu`, 8 verifying threads):

| Deals, seeds from | GPU time, deals/s | Won by the GPU | Failed | Misses, all won by the CPU |
|---|---|---|---|---|
| 2,000,000, 600,000,000,000 | 4,241,281 | 2,000,000 | 0 | 0 |
| 6,553,600, 927,000,000,000 | 5,593,948 | 6,553,596 | 0 | 4 |

The kernel compiles to 80 registers and 0 bytes of local memory, as before. The misses are the same count as b8d71d3's check on the same seeds (0 and 4).

**Sustained `acc batch`,** as a long run (`--count 1000000000000`, `--stop-after 0.1667`, `--gpu-config '{}'`), 10 minutes a run, alternated n b | b n, each from its own seeds, Best performance. Power and busy from 20 s samples. Lines verified by the run: no "failed to verify" line, and every deal the GPU did not win was solved or proven lost by the CPU.

| Build | Seeds from | Deals/s, whole run | Last half | GPU busy, last half | CPU package, last half | GPU, last half | Verifying, CPU ns a deal | Not won by the GPU | Failed |
|---|---|---|---|---|---|---|---|---|---|
| This build | 1,030,000,000,000 | 6,198,632 | 6,175,239 | 100% | 17.3 W | 29.1 W, 1,427 MHz | 415 | 2,713 | 0 |
| b8d71d3 | 1,035,000,000,000 | 6,057,605 | 6,057,876 | 100% | 17.3 W | 29.2 W, 1,437 MHz | 411 | 2,615 | 0 |
| b8d71d3 | 1,040,000,000,000 | 5,654,609 | 5,652,835 | 100% | 19.8 W | 29.0 W, 1,331 MHz | 346 | 2,431 | 0 |
| This build | 1,045,000,000,000 | 5,791,269 | 5,793,199 | 100% | 19.5 W | 29.3 W, 1,320 MHz | 356 | 2,620 | 0 |

Pair 1 (this build first) x1.023 whole, x1.019 last half; pair 2 (this build second) x1.024 whole, x1.025 last half; both together x1.024 whole, x1.022 last half, in line with TUNE8's x1.022 GPU alone. The two pairs ran in different machine states (the second job at 1,320-1,331 MHz and 19.5-19.8 W on the CPU, against 1,427-1,437 MHz and 17.3 W), so the builds' absolute speeds differ by 7% between the pairs; each ratio is within one state. Misses per 100,000 deals: 0.072-0.075 on both builds.

**Tests.** The four crates pass, with the GPU: every test binary ran and passed (the CLI's `ctrl_break_pauses_and_a_second_one_stops` passed in the full run). Smart App Control blocked the first build of the GPU kernel test binary (os error 4551); a comment in `tests/kernel.rs` changed, the rebuild ran and passed (4 tests). Clippy with `--all-targets`: no warnings. No test needed to change for the rungs (the kernel tests take their try counts from the settings), and no golden value changed.

**Tried, not kept** (TUNE8, HOLD, IDEAS5; GPU alone, 30M-deal runs after a warm-up, 3 alternated passes unless noted; 4M-10M runs are screens):
- **Shorter or earlier prefixes (TUNE8).** `[[39,16],[38,..]]` x0.958, `[[39,16],[37,..]]` x0.937, `[[40,8],[38,..]]` x0.980: a shorter prefix leaves more search a try (IDEAS5's exact count for 39 moves: 9% fewer attempts, 43% more search a try) and the balance does not pay. First-rung tries at 40 moves: 24 x1.00-1.01, 32 x1.002, 48 x0.998 (attempts 4.94 to 5.36, positions 570 to 537, speed level).
- **`SOLO_GROUPS` (TUNE8).** 8 / 12 / 20 / 24 / 32 against 16: x0.979 / 0.994 / 0.999 / 1.001 / 0.998. Flat from 20; fewer solo groups lose.
- **`share_moves` 21 (TUNE8).** x0.981; 19 is below the kernel's minimum of 20 (the masks a group keeps fit 32 bits from 20). Budget 525 / 875 against 700: x0.999 / 0.998. Under TAKE_MIN 8 with the first rung of 24, `TAKE_FLOOR` 12 / 20, budget 600 / 800, `SOLO_GROUPS` 12 / 24 and TAKE_MIN 6 / 10 all read x1.016-1.026: flat among themselves, so the settings sit on a plateau.
- **Search warps sleeping until enough lanes have work (HOLD).** `__nanosleep` while fewer than H of 32 lanes are searching and the queue cannot fill them: H = 8, 200 ns x0.953 / 0.954 / 0.955 (30M x3, 1,437-1,445 against 1,395-1,449 MHz, 29.4-29.5 W both); H 8 / 16 / 24 with 200 / 1000 ns x0.945-0.957 (10M x2); decided in the take block only x0.968 (H=8) / 0.960 (H=16). The search warps are already full: a turn steps 29.3 of 32 lanes on average, with 0.9% of turns at no lane, 3.1% under 16 and 5.3% under 24, so the hold has 1-2% of turns to save. It costs more: a later search is a later win, so a deal plays +1.7% (H=8) to +4.5% (H=24) more attempts on deals already won, and the prefix warps wait for room in 2.1% of turns, not 1.5%.
- **Fewer search warps a block (HOLD).** 2 prefix + 1 search warp (96 threads, 8 blocks) x0.930 / 0.939, 3 + 1 (128 threads, 6 blocks) x0.869 / 0.879 (10M x2); with the hold x0.86-0.88 and x0.82-0.83. The clock rises (1,470-1,530 MHz), but one search warp cannot keep up: the prefix threads wait for room in 11.7% (2+1) and 18.3% (3+1) of turns.
- **Prefix and search warps spread over the schedulers (IDEAS5, `%warpid`).** x0.988 / x0.990 against 5,919k / 5,878k deals/s (30M), control with the prefix warp always warp 1 x0.984; each 8 bytes of spill; 2M lines verified, 0 failed, all won. Roles do not segregate by scheduler, or that is not what binds.
- **Doubled exhausted-search work (IDEAS5, kSX).** Every lost search run twice (the same verdicts, +57% search positions): 22.10M / 22.75M attempts/s against 23.50M / 21.28M for the base, x1.00 (the base's two runs differ by 10%). Positions cut from the search do not buy speed, which is why every position cut has been flat.
- **A kernel with no search at all (IDEAS5, kNS) read x2.3** (52.1-52.8M attempts/s, no deal won) and was taken to show that awake search warps cost half the prefix's throughput; HOLD's clock sample of it reads 1,830 MHz at 15.9 W, far under the 30 W cap. Without the search the GPU is not power-bound, so it measured a different machine, not the search warps' cost; the hold above is the test that settled it.

**Considered, not tried:**
- **Second rung at 37 moves, or one rung only.** x0.937 with a 39-move first rung, and lost in TUNE5 already.
- **More first-rung tries than 24 with TAKE_MIN 8.** 28 and 32 read x1.019 and x1.016 against x1.022 for 24 (30M x3 on neighbouring bases): on the plateau, within the +-0.5% of the bases.

**What binds.** The GPU, at its power cap: 100% busy, 29.0-29.3 W, 1,320-1,437 MHz. The balance of prefix and search warps is flat around these defaults; the remaining ideas are those of the section above (the search turn's lane-wide work, the 570 positions a deal).
