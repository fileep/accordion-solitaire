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
