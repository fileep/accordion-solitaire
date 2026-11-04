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
