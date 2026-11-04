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
