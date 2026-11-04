# Accordion Solitaire Solver — Implementation Plan

Companion to [docs/RESEARCH.md](docs/RESEARCH.md), which holds the rules, prior results,
measurements and design rationale. This file covers what we build, in what order, and how
we know each piece is done.

---

## 1. Goals and defaults

**Goal:** solve as many random deals of open Accordion as possible, as fast as possible,
across thousands of games in parallel. Targets are 100% of deals on a 1M-deal benchmark, and
then a 10M+ deal run reported with a Wilson 95% confidence interval.

**Priority: find wins, don't prove losses.** No random deal has ever been shown unwinnable,
so the solver is tuned for time to first win.

**Solve any deck a user enters.** A user can type or paste a 52-card order, or build one in the visualizer, and
the program validates it and tries to solve it with the same solver and schedule used for batch runs. You get the
winning line to replay, or, if it isn't won, a dossier and escalation as below. This works from the command line (`acc solve`) and in the
visualizer ("Solve" button).

**Any deal we fail to win is never quietly dropped.** It gets a full dossier and goes through an
escalation process that keeps trying to win it (§5). Proving a loss is the last step of that process, not a goal in itself.

**Rule set:**

| Item | Rule |
|---|---|
| Deal | 52-card deck, all cards dealt face up in one row ("open"/"thoughtful" Accordion) |
| Move | Pile at position *i* moves onto pile *i−1* or *i−3* when their **top cards share suit or rank** |
| Pile | Moves as a unit; only top cards matter; gaps close up |
| Optional moves | Any legal move, any order |
| **Win** | **One pile remains.** This is the only win condition; it always takes exactly 51 moves |

**Out of scope:**
- lenient scoring such as "≤5 piles";
- the deal-one-at-a-time (stock) game.

**Future variant:** *Accordion's Revenge*, where a chosen card must finish on top. The engine keeps one hook
for it (an optional "required final card" in the rules config); no other work is planned for it now.

**Tech stack:**

| Component | Choice | Why (details in RESEARCH §7) |
|---|---|---|
| Engine + solver | **Rust** (stable, `std::arch` AVX2/BMI2, rayon) | C-level speed, safe concurrency, easy on Windows, and it compiles to **WebAssembly** |
| Visualizer | **TypeScript + Vite** web app, engine via **WASM** | One rules implementation for solver and UI |
| GPU (optional, Phase 8) | **CUDA** kernels via NVRTC, launched from Rust with `cudarc` | NVIDIA T550 on this machine; decided by a measured go/no-go gate |
| Analysis | Python (numpy/pandas/matplotlib) on solver JSONL output | Plotting and stats only, never in the hot path |

---

## 2. Architecture

```
                ┌────────────────────────── Rust workspace ──────────────────────────┐
  deal seeds ──►│  accordion-core     cards, rules, deal RNG, reference engine,       │
                │                     fast engine (bitmask movegen, Zobrist hash,     │
                │                     dead-end tests), file formats, verifier         │
                │        ▲                                                            │
                │  accordion-solver   DFS + dead-state TT, random-prefix attempts,    │
                │                     escalation schedule, endgame table + canonical  │
                │                     cache, batch runner, hard-deal pipeline         │
                │        ▲                    ▲                                       │
                │  acc (CLI)          accordion-gpu (optional): CUDA deep-rung        │
                │                     lottery for batches of deals                    │
                │  accordion-wasm     wasm-bindgen wrapper of core                    │
                └────────┬────────────────────────────────────────────────────────────┘
                         │ .wasm
                ┌────────▼─────────┐   one selected deal / solve file at a time
                │  viz/ (TS+Vite)  │ ◄──── (solved games, hard-deal dossiers)
                │  enter deck ·    │
                │  solve (WASM) ·  │ ────► export deck for `acc solve` (native)
                │  play · undo ·   │
                │  replay slider   │
                └──────────────────┘
```

The solver runs headless and writes results to files. The visualizer is a separate tool for
**selected** deals (verification and human play). It never shows live batch runs.

### Repository layout

```
Cargo.toml                 workspace
crates/core/               accordion-core
crates/solver/             accordion-solver
crates/cli/                acc binary
crates/wasm/               accordion-wasm
crates/gpu/                accordion-gpu (Phase 8, only if the gate passes)
viz/                       Vite + TypeScript app
research/probes/           Python probes behind RESEARCH.md (reproducible)
research/experiments/      experiment configs + result JSONL + notebooks
hard-deals/                dossiers for every deal not yet won (§5), plus INDEX.md
testdata/                  golden deals and solutions shared by Rust, TS and CUDA tests
docs/                      RESEARCH.md, FORMATS.md, BENCHMARKS.md, RESULTS.md
```

---

## 3. File formats (frozen in Phase 1)

**Cards:** two characters, rank `A23456789TJQK` and suit `CDHS`. Example: `TH` is the ten of hearts.

**Deal:** 52 cards, left to right, space-separated. Deals generated from a `u64` seed use a specified PRNG
(SplitMix64 seeding xoshiro256\*\*, then Fisher–Yates). It is written once in `accordion-core` and reached from TS via WASM.

**Deck input** (user-entered decks, CLI and visualizer). The parser is lenient but the validation is strict:

- Cards are separated by spaces, commas or newlines. Case-insensitive.
- Ranks may be `A 2–9 T J Q K`, with `10` accepted for `T`. Suits may be letters (`C D H S`) or symbols (`♣ ♦ ♥ ♠`). `10h`, `TH` and `10♥` are all the same card.
- The order is left to right, as dealt.
- Validation requires exactly 52 distinct cards. Errors name each problem: unknown token (with its position), duplicate cards (with both positions), and missing cards.
  Nothing is solved until the deck is valid.
- Accepted from a command-line string, a text file, stdin, or a solve file's `deal` field. It is always written back in the canonical
  `TH`-style form, so a deck entered once can be shared and replayed exactly.
- **Position input** (optional): the same notation with piles separated by `|`, each listed bottom→top. This lets the solver start from a game already
  in progress, such as the current position in the visualizer.

**Move:** `7H>7C` means "pile whose top card is 7H moves onto pile whose top card is 7C". It is unambiguous because top cards are unique.
Batch files also allow a compact form: one byte per move (index·2 + leap flag), base64-encoded.

**Solve file** (`.json` for one game, `.jsonl` with one object per line for batch output):

```json
{
  "format": "accordion-solve/1",
  "rules": { "deal": "open", "leaps": [1, 3], "match": "suit-or-rank", "win": "one-pile" },
  "deal": { "seed": 12345, "cards": "KS 7H 2D ... (52)" },
  "result": "win",
  "moves": ["7H>7C", "QS>QD", "..."],
  "stats": { "nodes": 18342, "attempts": 3, "time_us": 912, "schedule": "ladder-v1", "solver": "acc 0.1.0+<commit>" }
}
```

- `result` is `win | unknown | loss`. `loss` is written only after a completed, independently cross-checked
  exhaustive search (§5).
- For `unknown` deals, `moves` holds the **best line found** (fewest piles left), so it can be replayed and continued by hand.
- The hard-deal dossier format is defined in §5.

---

## 4. Phases

Phases run in order:
`0 → 1 → 2 → 3 → 4 → 5 → 6 → 7`, with `8` (GPU) optional after a measured gate.

### Phase 0 — Toolchain and scaffold ✅ done

- **Rust:** rustup, stable 1.99, on the **GNU target** (`x86_64-pc-windows-gnu`).
  - It needs no Visual Studio Build Tools and uses the same LLVM code generation as MSVC.
  - The bundled GCC only links, so dependencies must be pure Rust for now.
  - Add the MSVC target and VS Build Tools later if we want Windows profilers that need PDB debug info.
- **Node.js** 26.13 LTS, with Vite 8 and TypeScript 7.
  - **wasm-pack** 0.15 comes from npm; its install script is approved in `viz/package.json`.
  - `wasm-opt` is off because its binaryen download fails on Windows. It only shrinks the file.
- **Not yet:** the CUDA Toolkit, until Phase 8 passes its gate.
- **Repo:** `git init` on `main`, with the workspace layout from §2.
- **Release profile:** `lto="fat"`, `codegen-units=1`, `panic="abort"`, line-table debug info. `target-cpu=native` applies only to native
  targets (`.cargo/config.toml`), so WASM stays portable.

**Done:** `cargo test`, `acc`, `npm run build` and `npm run dev` all work, and the visualizer loads the engine from WebAssembly.

### Phase 1 — Reference engine and formats (correctness first)

- `Card`, `Deal`, seeded deal generator, card/move/solve-file parsing and printing.
- **Deck input parser and validator** (§3 "Deck input"): lenient tokens, strict 52-distinct-card check, precise error messages.
  The same code serves the CLI and, via WASM, the visualizer.
- **Reference engine:** deliberately simple `Vec<Vec<Card>>` piles, used as the test oracle.
- `acc deal --seed N` and `acc verify file.jsonl`. `verify` replays every move, checks legality, and checks that it ends in one pile.
- Golden test data in `testdata/`.

**Done when:** format round-trip tests pass and hand-built golden games verify.

### Phase 2 — Fast engine and dead-end tests (the hot path)

Design from RESEARCH §6:

- **State (128 B, two cache lines):**
  - top cards as 1-byte codes, stored after 3 sentinel bytes;
  - `len`;
  - `present`: bit = card code, one 16-bit row per suit;
  - legal-move masks `l1`/`l3`;
  - a 64-bit hash.

  Use copy-make on a fixed 52-deep stack, with each frame keeping its untried moves as two masks. There is no recursion and no allocation.
- **Card code** `suit<<4 | rank`, so the match test is two nibble checks on `a^b` (masks `0x0F` and `0x70`). The padding sentinel `0x7F` never matches.
- **Movegen:** legal moves are bitmasks, iterated with `tzcnt`. After a move, update them incrementally:
  - shift the bits above the move;
  - recompute at most 7 changed relations;
  - clear `l1` bit 0 and `l3` bits 0–2.

  This is verified in `research/probes/encoding_check.py` over 817,743 moves. The random prefix picks a uniform move in O(1) with `popcnt` + `pdep`.
- **Hash:** Zobrist over adjacency pairs. A slide is 3 XORs and a leap is 7, and the child's hash is known before the child is built, so the TT can be prefetched.
- **Dead-end tests** (proven sound, RESEARCH §5.8), run **only where they pay** (RESEARCH §5.10):
  - **Connectivity, reduced to the 4 suits.** Cards are connected exactly when the suits stay linked through shared ranks.
    - A move can only disconnect the position when it removes the last rank two suits share. Detecting that takes ≤3 ANDs on the suit rows.
    - The full check (≤4-suit closure) runs only on that event.
    - It also catches every orphan, so there is no separate orphan test.
  - **Left-leaf pair** (two cards whose only match lies to their left): runs only when ≥2 single-match cards exist, tracked from suit/rank counts.
  - **Gates by piles left.** Starting values, to be tuned with the Rust benchmark:

    | Piles left | Tests |
    |---|---|
    | ≥ 20 | None (measured firing rate ≤0.1%); TT only |
    | 17–19 | Connectivity on event |
    | 11–16 | Connectivity on event; left-leaf when triggered |
    | ≤ 10 | Endgame table lookup instead (Phase 6) |

  - **Random-prefix moves run no tests at all.** Filtering every random move gave no higher win rate and cost ~450–500 test
    evaluations per attempt. The brute force's first node, under the same gates, is the end-of-random-moves check.
- **Leftmost-card constraint** (the leftmost card can never be the final card): evaluate as an additional test or as move ordering.
- **Differential testing:** run 10M+ random playouts comparing fast and reference engines on legal moves, states, and incremental against
  recomputed hash. Separately, run 1M random positions to check that **no dead-end test ever fires on a position the
  reference search can win**.
- **Benchmarks** (`criterion`): movegen, apply, hash, each dead-end test, TT probe, and DFS nodes/s. Run DFS nodes/s with the gates on and off,
  and sweep the gate values to set them.

**Done when:** differential and soundness tests pass. Single-thread targets: apply+movegen ≤ 5 ns, DFS with TT and
dead-end tests ≥ 20M nodes/s (about 100× the Python probe).

### Phase 3 — Visualizer (selected deals only)

Vite + TypeScript; cards drawn in SVG/CSS. It opens **one deal at a time**: by seed, a user-entered deck, a single solve file,
or one entry picked by seed from a batch or `hard-deals/` file. It has no batch dashboards or live solver views.

- **Enter deck:**
  - *Paste/type* a deck in any accepted notation (§3), with validation errors shown inline (unknown token, duplicates, missing cards).
  - *Card picker:* click the 52 cards in deal order from a grid, so each card can only be used once. Reorder by drag. Undo the last pick.
  - *Random* or *by seed* as shortcuts.
  - A valid deck can then be played, solved (Phase 4) or exported.

- **Layout:** 52 piles wrapped into rows (reflowing), with position numbers. Each pile shows its top card, a depth badge, and the cards beneath it on hover.
- **Play mode:** click or drag a pile, legal targets (1-left / 3-left) highlight, drop to move. Moves animate: the pile slides, then the gap closes.
  - Unlimited **undo/redo** (`Ctrl+Z` / `Ctrl+Y`) and a move-history list where clicking jumps to that point.
  - Restart, piles-left counter, moves-available counter, win/stuck detection.
- **Replay mode:** load a solve file (or a hard-deal dossier's best line).
  - Transport: play/pause, step back/forward, a **slider over moves 0–51**, and **speed 0.25×–32×**.
  - Keyboard: space plays/pauses, arrows step.
  - Every file is verified through WASM, and an illegal move is flagged with its index.
  - "Continue from here": leave replay at any move and play on by hand. This is how a human attacks a hard deal.
- Export the current game in solve-file format, so a human win on a hard deal can be verified and recorded.
- Solving a deck is wired in during Phase 4, once the solver exists.

**Done when:** a human can play a full game with undo, and solver output replays correctly at every speed, including scrubbing.

### Phase 4 — Baseline solver and batch runner

- **First-win DFS:**
  - randomized move ordering;
  - a dead-state TT kept for the whole deal, across restarts;
  - Phase 2 dead-end tests;
  - node-budget cut-off.
- **TT:** 64-byte buckets of tagged entries, per thread. Clear it per deal with an epoch counter. Lossy is safe because every win is replayed by
  the reference engine before it counts.
- **Restarts:** Luby schedule over node budgets as the baseline.
- **`acc batch`:**
  - seed ranges;
  - rayon work-stealing over all 16 threads;
  - streaming JSONL output;
  - per-deal cap. A capped deal is written as `unknown` **and its dossier is created** (§5).
  - Summary: win rate with Wilson CI, nodes/time percentiles, deals/s.
- **Solve a user's deck (CLI):** `acc solve` takes `--cards "<52 cards>"`, `--file deck.txt`, stdin, `--seed N`, or `--position "<piles>"`.
  - It validates the input and runs the full schedule on all threads (a portfolio of attempts), showing progress: attempts, rung, best piles-left so far.
  - It writes a solve file and prints the winning line.
  - Options: `--time` cap (default e.g. 60 s). With `--escalate`, if the cap is hit it carries on through the §5 stages automatically.
  - An unwon deck always gets a dossier, exactly like a batch deal.
- **Solve a user's deck (visualizer):** build `accordion-wasm` with the solver included, and run it in a **Web Worker** so the UI stays responsive.
  - **Solve** on an entered deck, or **Solve from here** on the current position during play.
  - Shows progress and has a Cancel button. On a win, it opens the solution in replay mode (slider, speeds, step).
  - WASM is single-threaded and roughly 1.5–2× slower than native per thread. That is still milliseconds for a typical deal.
  - If the browser run hits its time limit, the result is shown as "not solved yet". The page offers the deck as a file plus the exact `acc solve --file … --escalate` command for the
    native solver, and the resulting solve file loads straight back in.
  - *Optional later:* `acc serve`, a localhost-only endpoint the visualizer can use to run the native multi-threaded solver directly.

**Done when:** ≥99.9% wins on 100k deals, all wins verified, every `unknown` has a dossier. `acc solve` and the visualizer's Solve /
Solve-from-here both solve user-entered decks (including hand-typed decks with deliberate typos, which must be rejected with clear errors).

### Phase 5 — Random-prefix / brute-force trade-off and restart schedules (core research)

**Attempt** = play *k* prefix moves sampled from a policy (plain moves, no tests; the brute force's first node is the dead-end check), then first-win DFS on
the residual (52−*k* piles) with node budget *B*. Each attempt ends in one of three ways, and each is a signal:

- **win;**
- **refuted:** the residual is proved dead, so *k* was too deep or the prefix bad;
- **budget-out:** the residual is too big for *B*.

For one deal, expected cost to first win is

```
T(k, B) = E[nodes per attempt | k, B] / P(win per attempt | k, B)
```

The probes already confirm this (RESEARCH §5.5–5.6):

- the best **fixed** switch point (*k*=28) averages about 49k nodes per deal;
- fixed *k* ≥ 32 leaves some deals never won;
- an **escalation ladder** averages about 27k nodes and won every deal. It makes up to 16 attempts at *k*=38, then 36, 34 and so on down to 24.

Steps:

1. **Measurement harness** (`acc experiment`). Over a training set of deals (e.g. 2,000) × grid of *k* (or residual size) × *B*,
   log every attempt's outcome and node count.
2. **Schedule optimizer** (Python). Compute each schedule's exact expected cost per deal from per-rung (*p*, mean cost), as in
   `research/probes/schedule_eval.py`, with bootstrap CIs. Minimize mean cost subject to p99/worst-deal limits and 100% coverage. Candidates:
   - fixed (*k*, *B*);
   - **escalation ladder:** tune start *k₀*, tries per rung *m*, step *s*, and *B* growth;
   - Luby over rungs;
   - **adaptive per deal:** a bandit over rungs. A refuted attempt pushes toward a smaller *k*; a budget-out pushes toward a bigger *B*.
3. **Switch point by residual shape** (pile count or an estimated subtree size) versus a fixed move count.
4. **Prefix policies:**
   - uniform random moves, no tests (current best);
   - softmax over cheap features (mobility, connectivity margin, Keller's sweepers, slide vs leap);
   - NRPA-style adaptation within a deal.
5. **Cache reuse across attempts:** measure how the persistent dead-state TT shifts the optimum.
6. Validate on **held-out** deals only.

**Done when:** the tuned schedule beats the Phase 4 baseline on held-out mean and p99 cost, with 100% wins on 100k held-out deals.

### Phase 6 — Symmetry: endgame table, endgame cache, canonical deals (the "backward" side)

Suits and ranks are arbitrary labels, so a position's future depends only on which piles match which (RESEARCH §4.4, §5.11).
The search spends most nodes at **10–14 piles** (RESEARCH §5.9). We exploit the symmetry in four ways:

- **Canonical key.** Relabel suits and ranks in order of first appearance, in one pass.
  The ordered match matrix is the coarsest exact key; in the probe it performed the same as relabelling. So relabelling is the default and the matrix is a
  benchmark variant.
- **Complete 1-bit endgame table for ≤10 piles,** indexed by a perfect ranking of canonical patterns (DP over position, suits used, ranks used).
  - 547M patterns, 68 MB, against 5.7×10^16 raw sequences.
  - Precomputed once and valid for every deal.
  - At ≤10 piles it replaces all dead-end tests and removes 5–44% of nodes.
  - Build *k*=11 (1.15 GB) only if measured worthwhile; it is a GPU candidate.
- **Symmetry-keyed endgame cache for 11–16 piles,** per thread, kept across attempts of a deal:
  - each entry is DEAD, or WIN plus the winning move as (position, distance). That move is label-free, so one entry answers every equivalent
    position, effectively a cached endgame policy;
  - the probe measured **16% fewer nodes and 28% fewer entries** than exact-card keys, which saved nothing;
  - **no global cross-deal cache.** Cross-deal hits were only 2–3%, too little to pay for synchronization. Re-measure at 1M-deal scale before reconsidering.
- **Canonical deal ID.** Relabel the whole deck.
  - A user-entered deck that is a relabelled copy of a known deal (solved, or in `hard-deals/`) is recognized at once, and the stored solution is
    relabelled back to the user's cards.
  - It also dedupes the hard-deal registry.
- **Raw Zobrist keys above 16 piles,** where symmetric repeats are rare and a canonical key isn't worth computing.
- Re-tune Phase 5, since cheaper leaves move the optimum deeper.

**Done when:** table and cache answers match plain search on 1M random residuals, and the end-to-end speed-up is measured (or there is a
documented reason to drop a piece).

### Phase 7 — Scale-up and hard-deal pipeline

- Run 1M deals, then 10M+. Every `unknown` goes through §5.
- Report: win rate with a Wilson 95% CI (unknowns counted both ways), throughput, cost percentiles. Compare with Masten (30M/30M) and
  Solvitaire (99.99948 ± 0.00052%).
- Results in `docs/RESULTS.md`. A hard-deal gallery lives in `hard-deals/INDEX.md`, with every entry loadable in the visualizer.

### Phase 8 (optional) — GPU deep-rung lottery

**Why it might pay.** The probe data (RESEARCH §9) shows that **deep rungs** do almost all the useful work if we can afford many more tries there.
A deep rung is a long random prefix plus a small brute force on ≤20 piles. With 64 tries per deep rung, **99.1% of deals were won without ever
brute-forcing more than 20 piles**. That workload is GPU-friendly:

- small state;
- no large shared tables;
- millions of independent attempts.

The large DFS with a big TT stays on the CPU, which is poor GPU territory: published GPU IDA* is only about 5× one CPU core.

**Design:**

- **Kernel input:** a batch of (deal, attempt-seed) pairs.
- **Work per warp or thread block** (one attempt each):
  - random prefix with no tests;
  - bounded explicit-stack DFS on the small residual (≤18–20 piles), with the same gated, event-driven dead-end tests;
  - write the move list on a win.
- **Cache:** each attempt gets a small lossy dead-state table (2–4k entries, 16–32 KB) in **shared memory**.
  - RESEARCH §9.4 shows it is required: without a cache, 20-pile searches cost over 28× more and 34% hit a 300k-node cap.
  - With a cache, a 20-pile search touches only about 1,400 states, so the small table is enough.
  - Residuals of ≤14 piles may skip the cache entirely, with one attempt per thread.
- **CPU side:**
  - verifies every GPU win with the reference engine;
  - feeds deals the GPU did not win into the CPU ladder;
  - keeps the GPU queue full (pipelined, so CPU and GPU work concurrently).
- **Correctness:** CUDA rules code is tested against the Rust engine with the same differential tests and golden files as Phase 2.
- **Also a GPU candidate:** building the *k*=11 endgame table (1.15 GB fits in the T550's 4 GB). This kind of bit-table BFS is a known GPU strength (27× in Edelkamp et al.).

**Go/no-go gate.** Start Phase 8 only if all of these hold:

- (a) Phase 5 is tuned;
- (b) profiling shows deep-rung attempts are ≥50% of CPU time;
- (c) a prototype kernel (≤1 week) shows **≥2× end-to-end deals/s** on the T550 against the 16-thread CPU solver.

The T550 is entry-level (1,024 CUDA cores, 4 GB). Expect a modest gain here; larger NVIDIA GPUs scale the same kernel much further.

**Done when:** the gate is either failed and documented, or the hybrid runner is faster end to end with every GPU win verified.

---

## 5. Unsolved-deal protocol ("never leave a deal behind")

A deal that hits its cap is **`unknown`, never `loss`**. Each one gets a dossier, `hard-deals/<seed>.json`, plus a line in `hard-deals/INDEX.md`.

**The dossier holds:**

| Field | Content |
|---|---|
| Identity | Seed, 52 cards, rules, solver version and **git commit**, schedule config, all RNG seeds, so every attempt is reproducible |
| Attempt log | Per rung: attempts, wins, refuted, budget-outs, nodes, wall time; the same for every later escalation stage |
| Best line | Fewest piles reached, plus its move list and final position. It loads in the visualizer for replay and "continue by hand" |
| Deal features | Initially playable cards, initial legal moves, suit/rank spread, same-suit runs, Keller sweeper score. Used to study what makes deals hard |
| Search stats | Dead-end test firing rates, TT fill and collisions, table/cache hit rates |
| Status history | Each escalation stage with date, result and the reason it moved on |

**Escalation stages.** Each stage keeps trying to win; stop at the first verified win.

1. **Re-run:** 10× cap with fresh random streams.
2. **Portfolio:** all threads, mixing prefix policies (uniform, heuristic, NRPA), move orderings, and much longer deep-rung lotteries.
3. **Exhaustive search from the root:**
   - lossless TT, disk-backed if needed;
   - all dead-end tests, endgame table, canonical cache;
   - root split into independent subtrees across threads;
   - checkpointed so it can be paused and resumed.
4. **Human attempt** in the visualizer, starting from the dossier's best line.
5. **Loss classification.** Only if stage 3 **completes** with no win, and a **second, independent run** with a different traversal order and fresh TT
   agrees, is the deal marked `loss`. The proof record stores node counts, configs and commits. No random deal has ever been proven unwinnable, so any loss is
   treated as a notable result: re-verify it with the Python reference engine on the residual subproblems.

Wins found at any stage are verified and written back to the dossier, and the deal moves to "solved (hard)" in the index. That keeps the record of
what made it hard.

---

## 6. Testing and verification rules (all phases)

- **No win counts until replayed by the reference engine**, whether it was found on the CPU, the GPU, or by a human.
- **Dead-end tests must be sound:** a standing test checks that none fires on a reference-winnable position.
- **Deck input:** parser tests cover every notation variant, plus duplicates, missing cards, extra cards and junk tokens. One shared test file is used by Rust and TS.
- Fast engine ⇔ reference engine (⇔ CUDA kernels, if Phase 8) differential tests run on every change.
- Rust, TS (via WASM) and CUDA read the same `testdata/` golden files.
- Benchmarks run on fixed seed sets; record them in `docs/BENCHMARKS.md` with CPU/GPU and commit.
- Experiments use disjoint deal sets: `train` (tuning), `validation`, `test` (final reports only).

## 7. Milestones

| # | Milestone | Acceptance |
|---|---|---|
| M0 | Toolchain + skeleton | `cargo test`, `npm run dev` work |
| M1 | Reference engine + formats | golden games verify; formats frozen |
| M2 | Fast engine + dead-end tests | 10M differential playouts and 1M soundness checks pass; ≥20M DFS nodes/s/thread |
| M3 | Visualizer | enter/validate a deck (paste or card picker); human play with undo; replays solver files with slider/speeds; continue-by-hand |
| M4 | Baseline solver + solve-a-deck | ≥99.9% on 100k deals, all verified, every unknown has a dossier; `acc solve` and visualizer Solve / Solve-from-here work on user-entered decks |
| M5 | Tuned schedule | beats baseline on held-out mean and p99; 100% on 100k held-out |
| M6 | Endgame table + cache | verified; measured speed-up |
| M7 | Big run | 10M+ deals, Wilson CI, results write-up, hard-deal gallery |
| M8 | GPU gate | gate result documented; if passed, hybrid runner faster end to end |

## 8. Risks

| Risk | Mitigation |
|---|---|
| Rare super-hard deals (Masten's 1999 solver needed about 2 h on one) | §5 protocol: escalation, portfolio, exhaustive search, human attempt; nothing dropped |
| An unsound dead-end test silently discards wins | Proof for each test, a standing soundness test against the reference search, and a second independent run before any `loss` |
| Lossy TT pruning hides a win | Completeness only; disabled in exhaustive (stage 3) search |
| Tuned schedule overfits the training deals | Held-out validation/test sets; Luby fallback rung |
| GPU effort doesn't pay off on an entry-level GPU | Strict go/no-go gate with a ≤1-week prototype; CPU solver is complete without it |
| CUDA rules drift from Rust rules | Shared golden files and differential tests; CPU verification of every GPU win |
| Windows toolchain friction | rustup GNU-target fallback; everything scripted in Phase 0 |

## 9. Decided

- **Rules:** open Accordion, moves 1 or 3 left, suit-or-rank match, **win = one pile only**. Accordion's Revenge maybe later.
- **Stack:** Rust engine/solver, TypeScript + WASM visualizer for selected deals, Python for analysis. CUDA only if the Phase 8 gate passes.
- **Strategy:** Masten-style random prefix + brute force with a tuned escalation ladder.
  - Dead-end tests run only where they pay: none during random moves, then gated by piles left and event-triggered.
  - Suit/rank symmetry powers the endgame table, the endgame cache and canonical deal IDs.
  - Every unwon deal is documented and escalated.
- **User decks:** any entered 52-card deck (or in-progress position) can be validated and solved, from the CLI (`acc solve`) or the visualizer (WASM solver).

**Installed (Phase 0):** Rust (GNU target) and Node.js LTS. The CUDA Toolkit comes only at Phase 8.
