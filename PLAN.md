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
| GPU (prototype in Phase 5, runner in Phase 8) | **CUDA** kernels via NVRTC, launched from Rust with `cudarc` | NVIDIA T550 on this machine; decided by a measured go/no-go gate |
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
crates/gpu/                accordion-gpu (Phase 5 prototype; Phase 8 runner only if the gate passes)
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
`0 → 1 → 2 → 3 → 4 → 5 → 7`. Phase 6 was merged into Phase 5. Phase `8` (the GPU runner) follows only if the Phase 5 GPU gate passes.

### Phase 0 — Toolchain and scaffold ✅ done

- **Rust:** rustup, stable 1.99, on the **MSVC target** (`x86_64-pc-windows-msvc`), with Visual Studio 2022 Build Tools
  (C++ workload, MSVC 14.44, Windows 11 SDK).
  - The GNU target was tried first, to avoid that install. It failed in Phase 1: crates built on `windows-sys` (clap's colors,
    and later criterion and CUDA loading) need import libraries made by `dlltool`. rustup's bundled `dlltool` can't run
    without an assembler it doesn't ship.
  - MSVC is Rust's standard Windows setup, and it also gives the Phase 2 profilers the PDB debug info they need.
- **Node.js** 26.13 LTS, with Vite 8 and TypeScript 7.
  - **wasm-pack** 0.15 comes from npm; its install script is approved in `viz/package.json`.
  - `wasm-opt` is off because its binaryen download fails on Windows. It only shrinks the file.
- **CUDA (added for Phase 5):** NVRTC 12.9.86, the CUDA runtime compiler, unpacked into `.tools/cuda-nvrtc-12.9.86/` (git-ignored).
  - It comes from NVIDIA's PyPI package `nvidia-cuda-nvrtc-cu12`: 76 MB, against 315 MB for NVIDIA's redist zip of the same version.
    The package was unpacked as a plain zip, without pip.
  - The download matched PyPI's SHA-256. The DLLs are not code-signed, but their sizes and CRCs match NVIDIA's official archive
    on developer.download.nvidia.com.
  - The driver (596.52) supports CUDA up to 13.2, and the T550 is compute capability 7.5. Nothing else from the CUDA Toolkit is needed: kernels are
    compiled at run time and launched through the driver.
  - NVRTC finds its `nvrtc-builtins` DLL through `PATH`, so its `bin` folder must be on `PATH`.
  - `research/probes/gpu_smoke.py` checks the setup by compiling a kernel and checking its output against the CPU.
- **Repo:** `git init` on `main`, with the workspace layout from §2.
- **Release profile:** `lto="fat"`, `codegen-units=1`, `panic="abort"`, line-table debug info. `target-cpu=native` applies only to native
  targets (`.cargo/config.toml`), so WASM stays portable.

**Done:** `cargo test`, `acc`, `npm run build` and `npm run dev` all work, and the visualizer loads the engine from WebAssembly.

### Phase 1 — Reference engine and formats (correctness first) ✅ done

- `Card`, `Deal`, seeded deal generator, card/move/solve-file parsing and printing.
- **Deck input parser and validator** (§3 "Deck input"): lenient tokens, strict 52-distinct-card check, precise error messages.
  The same code serves the CLI and, via WASM, the visualizer.
- **Reference engine:** deliberately simple `Vec<Vec<Card>>` piles, used as the test oracle.
- `acc deal --seed N` and `acc verify file.jsonl`. `verify` runs every rules-compliance check in §6.1.
- **Golden test data** in `testdata/`, generated by an independent Python implementation (`testdata/tools/make_golden.py`):
  - valid games;
  - every cheating move a buggy solver could make;
  - false wins;
  - unwinnable deals with exhaustive proofs;
  - 300 mutated near-miss lines.

  The full catalogue is in §6.2.
- `docs/FORMATS.md` specifies every format exactly. The WASM crate exposes the deck parser, seeded deals and the verifier to the visualizer.

**Done when:** format round-trip tests pass, the §6.2 catalogue passes against the Python implementation, and `acc verify` runs on it.

### Phase 2 — Fast engine and dead-end tests (the hot path) ✅ done

**Outcome** (measurements in docs/BENCHMARKS.md):

- **Code:**
  - `accordion-core::fast` is the engine;
  - `accordion-core::selfcheck` holds the fast-vs-reference checks;
  - `accordion-solver::{dfs, table}` is the first-win search and the epoch-tagged dead-position table;
  - `acc selftest` and `acc bench` are the CLI commands.
- **Correctness:**
  - 10,000,000 lockstep games (409M moves) with no disagreement;
  - 1,000,000 soundness positions with no test firing on a winnable one;
  - exhaustive counts match the golden data;
  - the portable (non-AVX2) code paths pass the same tests.
- **Speed:** **26.9–28.3M nodes/s** per thread, with table and dead-end tests, against a ≥20M target. Plain DFS won 100/100 full deals,
  averaging about 89k positions per deal.
- **Not met:** the ≤5 ns `apply` + movegen target. `apply` measures 16 ns. It is left for later tuning, since the node-rate target it was
  meant to serve is met.
- **Tests:** the gates were tuned by `acc bench --sweep`.
  - **Connectivity:** on trigger at ≤19 piles.
  - **Left-leaf:** off by default. It cuts 5–9% of nodes but costs more time than it saves.
  - **Leftmost-card constraint:** gave no cheap sound test beyond these, so it moves to the Phase 5 move-ordering experiments.

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
  - **Gates by piles left**, as tuned (starting values were 19 and 16):

    | Piles left | Tests |
    |---|---|
    | ≥ 20 | None (measured firing rate ≤0.1%); TT only |
    | ≤ 19 | Connectivity on trigger (on entering the zone and after any move that removes the last rank two suits share) |
    | any | Left-leaf off by default (not worth its cost); available via `Gates` |
    | ≤ 10 | Endgame table lookup instead (Phase 5, track E) |

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

### Phase 3 — Visualizer (selected deals only) ✅ done

**Outcome:**

- **Built:** `viz/` (Vite + TypeScript, rules from WASM).
  - deal by seed, random or entered deck (paste with live errors, or card picker with undo);
  - hand play with click to move and legal-target highlights, undo/redo (Ctrl+Z / Ctrl+Y), move history, win/stuck status;
  - replay of `.json`/`.jsonl` files (open or paste), with a game picker, transport, slider and 0.25–32× speeds;
  - verification shown per game; replay stops before an illegal move;
  - continue from here, verified export, and a stack info line on hover/focus.
- **Tested** in the browser pane (play, undo/redo, deck errors, picker, replay at 32×, scrubbing, continue, phone width).
- **TS golden tests:** `npm test` runs `viz/tests/golden.test.ts` on the WASM engine against `testdata/` and passes.
- **Not exercised in the browser:** drag-and-drop, the file-picker path (the same parser is used by paste) and the download itself
  (export is covered by the TS test).
- **Deferred:** drag to reorder picked cards.
- **Added with Phase 4** (requested during review):
  - **Solve & replay:** solves from the position on screen with the WASM solver in a Web Worker, with a cancel button. If that position
    can't be won it solves from the start instead. The line is verified, then opens in Replay and plays automatically.
  - **Two-step move animation:** the moving pile, raised and outlined, slides onto its target while a copy of the covered card stays put,
    then the row closes up. The last pile moved keeps an outline, and a caption names the move.
  - **"You win!" / "Game over" box** over the board, with Undo, Restart, New random deal, Show a solution (after a loss) and hide.
  - **Seed box:** the seed deals on Enter, and the separate Deal button is gone. Replay now defaults to 1x.

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

### Phase 4 — Baseline solver and batch runner ✅ done

**Outcome** (numbers in docs/BENCHMARKS.md):

- **Solver:**
  - `accordion-solver::solve` is randomized first-win DFS under a Luby restart schedule. The dead-position table persists across restarts.
  - `portfolio` runs it on N threads, with the first win stopping the rest.
  - It tracks the best line for dossiers, and a stop flag lets a run be cancelled.
- **`acc batch`:**
  - **100,000 deals (seeds 0–99,999): 100,000 won, every one verified, 95% CI [99.9962%, 100%].**
  - It ran at 4,110 deals/s on 16 threads; positions per deal: median 19,573, max 2.5M (229 ms).
  - `acc verify` re-checked the whole output file independently.
  - The per-thread table defaults to 1 MB: small tables that stay in the core's own cache were about 1.7× faster than 16 MB.
- **`acc solve`:**
  - It takes a seed, typed or piped cards, a file, or `--position`, and solves on all cores with live progress.
  - It writes a verified solve file.
  - `--escalate` retries with 10x and 100x limits and fresh orders. A deal that isn't won gets a dossier in `hard-deals/` and an INDEX.md row.
- **Gates:**
  - `crates/solver/tests/gates.rs`: the unwinnable golden deals and positions are never reported won, the controls are won, and the first
    100 deals win and verify.
  - The dossier path was exercised on the constructed unwinnable deal.
- **Deviations:**
  - Limits are in positions searched (`--max-nodes`), not wall time, so runs are reproducible.
  - The visualizer's "export the deck and run `acc solve --escalate`" fallback and `acc serve` are not built yet.
  - "Exhausted" (every line searched) is reported as not won rather than `loss`, because the table is lossy. Proofs remain Phase 7.

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

**Done when:**

- ≥99.9% wins on 100k deals, all wins verified, and every `unknown` has a dossier.
- The unwinnable golden deals are never reported as `win`, and the winnable controls are won.
- `acc solve` and the visualizer's Solve / Solve-from-here both solve user-entered decks. Hand-typed decks with deliberate typos must be
  rejected with clear errors.

### Phase 5 — Speed: find the fastest strategy, by measurement ✅ done

**Outcome** (numbers in docs/BENCHMARKS.md, findings in RESEARCH.md §10):

- **5.0× faster than Phase 4, with every deal still won.**
  - **Held-out test** (100,000 deals, seeds 1,000,000–1,099,999, run once): 14,805 against 2,952 deals/s. All 100,000 won and
    verified, 95% CI [99.9962%, 100%].
  - **Validation:** 14,674 against 2,945 deals/s.
  - **Positions per deal:** 6.4× fewer (mean 5,940, p99 46,699).
  - **Single thread** (the browser solver): 3.6× faster per deal.
- **What did it** (each measured on its own):
  - short Luby restarts (1,000 positions instead of 10,000);
  - a random-prefix ladder in front of the thorough search: 36 random moves, then 256 tries per rung, each a search with a
    4,000-position budget;
  - sleep sets, which skip move orders that differ only by commuting moves;
  - endgame tables to 11 piles (`acc endgame build`).
- **Tried and dropped, with numbers:**
  - biased move orders, in the search and in the prefix;
  - probing the table before building;
  - skipping the table near the end;
  - new gate values and the left-leaf test;
  - a suit/rank pattern cache for 12–16 piles kept across deals: −39% time.
- **Parallel games:**
  - **CPU:** the new runner removed the chunk barrier and the main-thread verification (+10%). The rest of the per-thread gap
    is hardware.
  - **GPU** (`accgpu`, crates/gpu):
    - The kernel reaches 255M positions/s and 18,840 deals/s alone.
    - CPU + GPU is about 1.7× the CPU alone. **The 2× gate is not met** on this laptop, where both share one cooling system,
      so Phase 8 is not started.
- **Every strategy falls back to the thorough search.** The ladder is a stage in front of it.
  - A test starves every ladder search and checks that the thorough stage still wins every deal.
  - On the 100,000 regression deals, one deal fell through every rung and was won by the thorough stage.
- **Correctness:** 1,000,000 random residuals give the same verdict with and without sleep sets and tables
  (`acc selftest --residuals`). The gate tests run under three strategies. The visualizer's solver has its own test.
- **Tools:**
  - `acc experiment`: deal sets, JSON configurations, warm-up, interleaved repeats, `summary.jsonl`;
  - `acc endgame build`;
  - `--config` and `--endgame` on `acc batch` and `acc solve`;
  - `--threads` on `acc batch`;
  - `accgpu`.
- **Deviations:**
  - NRPA and nested Monte Carlo prefix policies were not built, since the mobility-guided prefix already lost to uniform
    random.
  - Make/unmake for the CPU engine and profile-guided optimization were not tried.
  - The adaptive per-deal ladder was not built, because the tuned fixed ladder's optimum is flat.
  - Timing needed a warm-up protocol: this laptop's clocks drift up to ±25% with heat.

**Goal:** more deals per second in batch runs and less time per deal for a single solve (`acc solve`, the visualizer), without ever
winning fewer deals. This phase also takes in the old Phase 6 (symmetry, endgame tables and caches) and the GPU prototype that
decides Phase 8. Every idea is measured against the Phase 4 solver on the same deals, and kept only if it wins on deals it wasn't tuned on.

**Starting point** (Phase 4, docs/BENCHMARKS.md). Batch time is roughly deals × positions per deal × time per position ÷ threads, so each
track below attacks one factor:

| Factor | Phase 4 | Track |
|---|---|---|
| Positions per deal | mean ≈ 37,900 · median 19,573 · p99 266,452 · max 2.5M | C (search), E (tables) |
| Time per position, one thread on an idle machine | 35–37 ns (`acc bench`) | D (engine) |
| Time per position per thread, 16-thread batch | ≈ 103 ns (16 threads × 24.3 s ÷ 3.79B positions) | B (parallelism) |
| Deals/s, 16 threads | 4,110 | all of them; F (GPU) adds to it |

The 3× gap between the middle two rows is the first lead.
- **Hardware:** two hyper-threads share a core, 8 of the 12 cores are slower E-cores, and all-core clocks are lower.
- **Our pipeline:** verification and output run on the main thread between chunks of 4,096 deals, and each chunk waits for its slowest deal.

Track B measures how much of the gap each cause accounts for.

**Every fast strategy runs first; none replaces the thorough search.**

- The solver becomes a chain of stages:
  - fast stages go first, each with its own budget: a tuned ladder, table-backed search, GPU attempts;
  - the **last stage is always the Phase 4 solver**: complete randomized DFS from the start of the deal, with Luby restarts;
  - after that come `acc solve --escalate` and the §5 protocol.
- A deal a stage doesn't win passes to the next one, along with what was learned (its dead-position table). Nothing is dropped.
- Batch summaries count wins by stage. They show what each stage contributes, and that the fallback still catches the rest.
- **Forced-fallback test:**
  - every fast stage is made to fail (budget 0), and the test deals must still be won;
  - the unwinnable golden deals must still never be won.
- Anything that prunes must first give the same verdict as plain search, on the golden cases and on 1M random residuals (§6.3). That covers
  commutation, endgame tables and caches, and new dead-end tests.
- Whatever finds a win, it counts only after the reference engine replays it.

#### Track A — measure first

- **`acc experiment`** runs a grid of configurations over a deal set.
  - It writes one JSONL row per (configuration, deal): result, stage, positions, attempts and µs.
  - A header records machine, commit and threads.
  - Output goes to `research/experiments/phase5/`.
- **Deal sets**, kept disjoint (§6.4):

  | Set | Seeds | Use |
  |---|---|---|
  | train | 200,000–209,999 | tuning |
  | hard | the 1,000 deals of seeds 0–99,999 that took the most positions in Phase 4 | tuning the slow tail |
  | validation | 300,000–309,999 | choosing between finalists |
  | test | 1,000,000–1,099,999 | the final report only, run once |
  | regression | 0–99,999 | must stay at 100% |

- **Metrics:**
  - for search changes, positions per deal (mean, p99, max). These are deterministic for a given seed, so even small differences are real;
  - for engine changes, ns per position;
  - end to end: deals/s on 16 threads, and wall time per deal for `acc solve` and the WASM solver.
- **Timing:** every timing runs 3 times on mains power, interleaving configurations (A B A B) so laptop heat and clock drift hit them
  equally. Report the median and the spread.
- **Decision rule:**
  - keep a change only if validation is faster by more than the run-to-run spread, p99 is no worse, and wins stay at 100%;
  - a change that shifts where the search spends its positions (tables, ordering) triggers a re-tune of the schedule.
- **Profile counters** (experiment builds only):
  - positions, table hits, dead-end cuts and time, by piles left;
  - the batch runner's split between solving, verifying, writing and waiting.
- **Leaderboard:** one table in docs/BENCHMARKS.md lists every variant tried, kept or dropped, with its numbers and the reason.

#### Track B — use all 16 threads (parallel games on the CPU)

- **Scaling curve:** 1, 2, 4, 8, 12 and 16 threads; P-cores only against all cores; one or two threads per P-core.
- **No serial work:** workers take seeds from a shared counter, then solve, verify and serialize each deal themselves. One writer thread
  collects the output, so the chunk barrier and the main-thread verification go away.
- **Per-deal overhead:** dealing, result allocation and the table reset, measured on a stream of easy deals.
- **Single deals:** compare all threads on one deal (portfolio) with one thread, by deal difficulty. This sets when `acc solve` should split.
- **Estimate:** 1.1–1.3×. The measurement decides how much of the gap is ours to remove.

#### Track C — fewer positions per win (search strategy)

The solver is already depth-first and stops at its first win. These are ways to reach that win sooner.

1. **Restart schedule** for today's DFS: Luby unit from 1k to 1M positions, no restarts, and geometric growth.
2. **Random prefix + brute force** (Masten-style, RESEARCH §5.5–5.6). An attempt plays *k* random moves with no tests and no table, then
   runs first-win DFS on what is left with budget *B*.
   - **Ladder:** start at *k₀* with *m* tries. Then step *s* moves earlier and grow *B*. The thorough DFS from the start is the last rung.
   - **Grid:** *k₀* 30–40, *s* ∈ {2, 4}, *m* ∈ {4, 8, 16, 32}, and *B* per rung. It runs on the train and hard sets, with the
     dead-position table kept across attempts.
   - **Adaptive per deal:** a refuted attempt moves *k* earlier, and a budget-out raises *B*.
   - **Switch point by shape:** start the brute force at a pile count or an estimated subtree size, instead of after a fixed number of moves.
   - **Rough prior:**
     - Python probes on 40 deals put the best ladder at about 27k positions per deal, about 1.4× fewer than Phase 4's ≈38k mean.
     - This step measures it in Rust at 10,000 deals.
3. **Move ordering**, in the DFS and in the prefix. Candidates:
   - uniform random (today);
   - leaps first, or slides first;
   - most moves left afterwards (mobility);
   - a softmax over cheap features;
   - nested Monte Carlo or NRPA as a prefix policy.

   Ties are broken at random, so restarts stay diverse.
4. **Skip repeated move orders (commutation).**
   - Two moves **commute** whenever the left move's pile lies strictly left of the right move's target. Both stay legal, and either order
     gives the same position.
   - Far-apart moves are the common case, so most positions are reached in many orders.
   - Today the table catches the repeats, but each one costs a full child build and a probe. A 1 MB table also forgets many, which are then
     searched again.
   - **Sleep sets** explore each group of commuting moves in one order only. They are safe with our table because it stores only positions
     proven dead.
   - **Test:** measure positions and time with and without, and check by exhaustive search that verdicts are unchanged.
   - **Upper bound:** the share of positions that are table hits today.

#### Track D — cheaper positions (engine cost)

- **Check the table before building the child.** The child's hash is known first, so a table hit could skip the 16 ns build; today every
  hit pays for it.
- **Copy-make against make/unmake:** today each step copies a 128-byte position.
- **Table use by depth:** probe and store only above some pile count, since for a tiny subtree a lookup can cost more than re-searching it.
  Re-check the prefetch.
- **Re-tune the dead-end gates** (connectivity gate, left-leaf) for the new search mix.
- **Profile-guided optimization**, which needs rustup's `llvm-tools` component (a small download).
- **Two searches interleaved per thread** to hide memory stalls, only if profiling shows stalls.
- **Estimate:** 1.2–1.5×.

#### Track E — caches and symmetry

A position's future depends only on its **pattern**: which piles share a suit or a rank, not which suits and ranks they are (RESEARCH §4.4).
So anything learned about a pattern holds in every deal, and patterns are far fewer than card sequences. At 10 piles, 547M patterns stand
for 5.7×10^16 sequences.

1. **Table size.**
   - Sweep 256 KB to 256 MB per thread on the train and hard sets.
   - Phase 4 found small tables that stay in cache fastest for typical deals; a big one may pay on hard deals.
   - Then try growing the table once a deal passes about 1M positions.
2. **Complete endgame tables:** one bit per pattern (winnable or not), built once and valid for every deal.

   | Piles | Patterns | Size | Lives in | Build time (estimate, 16 threads) |
   |---|---|---|---|---|
   | 8 (and fewer) | 2.28M | 0.3 MB | L2 cache | under a second |
   | 9 | 34.3M | 4.3 MB | L3 cache | seconds |
   | 10 | 547M | 68 MB | RAM | minutes |
   | 11 | 9.21B | 1.15 GB | RAM | about half an hour |
   | 12 | 163B | 20 GB | RAM (48 GB) | hours; only if 11 pays well |

   - **One lookup at *K* piles settles the rest of the game.**
     - A **loss** prunes the subtree.
     - A **win** means the deal is won with *K*−1 moves still to play. The remaining moves come from walking the table: at each step,
       take a child it marks winnable. The whole line is then replayed by the reference engine, as always.
   - RESEARCH §5.9: 5–44% of brute-force positions sit below 10 piles, and 22–79% below 12.
   - `acc endgame build` writes the tables to a file kept out of git. They are checked against plain search on 1M random residuals.
   - The browser solver can use the small tables (8 or 9 piles).
3. **Symmetry-keyed endgame cache, 11–16 piles.**
   - It is per thread and keyed by the relabelled pattern. Each entry is DEAD, or WIN plus the winning move.
   - The move (pile, slide or leap) carries no card labels, so one entry serves every equivalent position.
   - The probe measured 16% fewer positions and 28% fewer entries than exact-card keys.
   - **WIN entries pay only across deals.** Within one deal a stored WIN is never reused, because the first win ends the search.
     - Pattern keys don't depend on the deal, so each thread can simply keep its cache from one deal to the next, with no locking.
     - Measure cross-deal hits at 10k–100k deals per thread; the probe saw 2–3% after 60 deals.
     - Try a cache shared between threads only if those numbers justify the synchronization.
4. **Canonical deal ID:** relabel the whole deck.
   - A user-entered deck that is a relabelled copy of a known deal is answered at once.
   - The hard-deal registry has no duplicates.

#### Track F — GPU feasibility (NVIDIA T550, RESEARCH §9)

- **Installed:** the NVIDIA driver (596.52, CUDA ≤ 13.2) and NVRTC 12.9 in `.tools/` (Phase 0 notes). `research/probes/gpu_smoke.py`
  compiled a kernel and ran it on the T550, and its output matched the CPU.
- **The work that fits a GPU:**
  - **Deep-rung attempts:** a long random prefix plus a small DFS on ≤18–20 piles, with a small dead-position table in shared memory.
    RESEARCH §9.3–9.4: with 64 tries per deep rung, 99.1% of deals were won without brute-forcing more than 20 piles.
  - **Building the 11-pile endgame table** is the other candidate.
- **Steps:**
  1. Model it on the CPU first. Run the deep-rung-only strategy in Rust, count attempts and positions per deal, and from those get the
     attempt rate the GPU would need. Stop here if that rate is out of reach.
  2. Build a prototype kernel, differential-tested against the Rust engine on the golden files.
  3. Hybrid run: the GPU takes deep rungs for a batch. Meanwhile the CPU verifies every GPU win and runs everything else through the normal
     stage chain.
- **Gate:**
  - if the hybrid reaches **≥2× the tuned CPU solver's deals/s end to end**, Phase 8 builds it into `acc batch`;
  - if not, the numbers go into RESEARCH §9 and the GPU work stops.

#### Track G — combine and confirm

- Combine the winners, then **re-tune the schedule last**: tables and cheaper positions move the best switch point deeper.
- Pick the final configuration on validation, then run it once on the 100k test seeds and on the regression set.
- Make it the default for `acc batch`, `acc solve` and the visualizer.
- Write the leaderboard into docs/BENCHMARKS.md and the findings into RESEARCH.md.

**Order:** A → B → C → D → E → F → G.
1. Measure first.
2. Then take the cheap, certain gains.
3. Then search changes before engine tuning, because the best engine depends on where the search spends its positions.
4. Then the tables, which shift the schedule.
5. The GPU comes last, because it must beat the tuned CPU.

**Done when:**

- every track has a measured result on the leaderboard, kept or dropped with its numbers;
- the adopted pipeline beats Phase 4 on held-out mean and p99 time per deal, and on deals/s. The target is **2× deals/s**, a goal rather
  than a gate;
- all 100k test deals and the whole regression set are won and verified, with wins counted by stage, and the forced-fallback test passes;
- every pruning method has passed its soundness check;
- the GPU gate is decided, with numbers.

### Phase 6 — merged into Phase 5

Its symmetry work (endgame tables, the symmetry-keyed cache and canonical deal IDs) is Phase 5 track E, so Phase 5 now goes straight to Phase 7.

### Phase 7 — Scale-up and hard-deal pipeline

- Run 1M deals, then 10M+. Every `unknown` goes through §5.
- Report: win rate with a Wilson 95% CI (unknowns counted both ways), throughput, cost percentiles. Compare with Masten (30M/30M) and
  Solvitaire (99.99948 ± 0.00052%).
- Results in `docs/RESULTS.md`. A hard-deal gallery lives in `hard-deals/INDEX.md`, with every entry loadable in the visualizer.

### Phase 8 (only if the Phase 5 GPU gate passes) — GPU deep-rung lottery

**Status: not started.** The Phase 5 prototype (`accgpu`, crates/gpu) works and all its wins verify. CPU + GPU measured
about 1.7× the CPU alone on this laptop, short of the 2× gate (docs/BENCHMARKS.md, Phase 5 track F). The design below was
validated by the prototype. Its kernel keeps the residual in registers instead of a per-attempt table, and relies on sleep
sets instead of a cache. On a machine where CPU and GPU don't share cooling, re-run the gate first.

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

**Go/no-go gate (decided in Phase 5, track F).** Build this only if the prototype hybrid (GPU deep rungs plus the CPU stage chain) reaches
**≥2× the end-to-end deals/s** of the tuned 16-thread CPU solver.

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

## 6. Rules compliance and testing (all phases)

### 6.1 How a win is proven real

Every claimed win passes `verify` before it counts. That covers wins from the CPU solver, the GPU, a human in the visualizer, or a loaded file.
`verify` replays the moves with the **reference engine**, which is deliberately simple and separate from the fast engine the solver
searches with. The rules are implemented a third time, independently, in Python (`testdata/tools/make_golden.py`). Rust and Python must
agree on every golden file.

| Check | Cheat or bug it catches |
|---|---|
| Format and rules are the supported ones | A file made under different rules |
| The deal is exactly 52 distinct cards, and equals the seed's deal when a seed is given | Invented, duplicated or missing cards; the wrong deal |
| The mover is a top card | Moving a buried card |
| The target is a top card | Landing on a buried card |
| The target is exactly 1 or 3 piles to the left | Moving 0, 2 or 4+ piles, or to the right |
| Mover and target share a suit or a rank | A non-matching move |
| The whole pile moves and the gap closes (reference engine) | Splitting piles; positions drifting out of sync |
| Text and compact move lists agree | Two encodings telling different stories |
| After replay, all 52 cards are present exactly once | Engine bugs that create, drop or copy cards |
| A win is one pile after exactly 51 moves, with nothing after it | "Wins" that stop early; extra moves |
| `final_card`, when the rules set it, is on top | Accordion's Revenge violations |

### 6.2 Golden test catalogue (`testdata/`, regenerated by `make_golden.py`)

| Group | Cases | Must hold |
|---|---|---|
| Seeded deals | 8 seeds, including 0 and 2^64−1 | Rust's deal = Python's deal |
| Valid games | Snake (51 slides); seed-42 win (26 leaps) in text, compact and both forms; partial line; Revenge final card; 3-record batch | Verifies, with exact move and pile counts |
| Cheating moves | Buried mover, buried target, moving right, onto itself, 2 left, 4 left, no match, a move after the win, an illegal compact move, text/compact disagreeing | Rejected at exactly the injected move |
| False wins | Stopping at 50 moves, wrong seed, a duplicate card in the deal, wrong Revenge card | Rejected |
| **Unwinnable** | see below | Rust exhaustive search finds no win and the **same number of reachable positions** as Python; `loss` records verify; fake wins on them are rejected |
| Mutations | 300 near-miss variants of 4 real wins: retargeted, re-mover'd, swapped, dropped, duplicated, extended, truncated, diverging | The Rust verifier agrees with the Python rule checker on every one (10 stay legal wins; 290 are rejected at the same move) |
| Deck and position input | Notation variants, unknown tokens, duplicates, missing, 51 and 53 cards, empty input, an empty pile | Same issues at the same positions |

The unwinnable cases:

- **No legal moves at all:** 1 reachable position.
- **Constructed dead end:** 4 opening moves, 353 positions, no win, and the best line leaves 44 piles. The first 43 cards can never move, and the
  playable tail shares no suit or rank with the three frozen cards it can reach.
- **Realistic dead end:** 36 legal random moves from seed 42 lead into a 16-pile position with 71 reachable positions, none a win.

### 6.3 Compliance gates by phase

- **Phase 1 ✅:** everything in 6.2 passes.
- **Phase 2:**
  - fast engine ⇔ reference engine differential tests on 10M+ random playouts (legal moves, states, hashes);
  - the fast engine's exhaustive counts must equal `exhaustive.json`.
- **Phase 3:**
  - a loaded file that fails verification is flagged at the failing move, and the UI never animates an illegal move;
  - human wins are exported and verified like solver wins.
- **Phase 4 onward:**
  - solver output is written as `win` only after it verifies, and batch summaries count verified wins only;
  - the solver's test suite runs the unwinnable golden deals and must **never** report `win` on them; they end `unknown`, or `loss` once exhaustive mode exists;
  - it must still win the winnable controls.
- **Phase 5:**
  - before any pruning method is adopted, its verdicts must match plain search on the golden cases and on 1M random residuals. That covers commutation (sleep sets),
    endgame tables, symmetry caches and new dead-end tests;
  - the unwinnable golden positions must read as dead;
  - the forced-fallback test (every fast stage fails) must still win the test deals and never win the unwinnable ones;
  - GPU prototype wins are verified on the CPU, and its rules code runs the same golden files.
- **Phase 7:**
  - exhaustive mode must prove all three unwinnable golden cases lost;
  - any real deal it marks `loss` gets the independent second run of §5.
- **Phase 8:** every GPU win is verified on the CPU, and the CUDA rules code runs the same golden files.

### 6.4 Other standing rules

- **Dead-end tests must be sound:** a standing test checks that none fires on a reference-winnable position.
- **Deck input:** parser tests cover every notation variant, plus duplicates, missing cards, extra cards and junk tokens. One shared test file is used by Rust and TS.
- Rust, TS (via WASM) and CUDA read the same `testdata/` golden files.
- Benchmarks run on fixed seed sets; record them in `docs/BENCHMARKS.md` with CPU/GPU and commit.
- Experiments use disjoint deal sets: `train` (tuning), `validation`, `test` (final reports only).

## 7. Milestones

| # | Milestone | Acceptance |
|---|---|---|
| M0 ✅ | Toolchain + skeleton | `cargo test`, `npm run dev` work |
| M1 ✅ | Reference engine + formats | §6.2 golden catalogue passes (cheats rejected, unwinnable deals proven by exhaustive search, mutations agree with Python); formats frozen |
| M2 ✅ | Fast engine + dead-end tests | 10M differential playouts and 1M soundness checks pass; exhaustive counts match; ≥20M DFS nodes/s/thread (measured 26.9–28.3M) |
| M3 ✅ | Visualizer | enter/validate a deck (paste or card picker); human play with undo; replays solver files with slider/speeds; continue-by-hand |
| M4 ✅ | Baseline solver + solve-a-deck | ≥99.9% on 100k deals, all verified, every unknown has a dossier; never `win` on the unwinnable golden deals; `acc solve` and visualizer Solve / Solve-from-here work on user-entered decks |
| M5 ✅ | Fastest strategy | every track measured on the leaderboard; adopted pipeline beats Phase 4 on held-out mean and p99 and on deals/s (target 2×, measured 5.0×); 100% on 100k held-out + regression set; pruning checked sound (1M residuals); GPU gate decided (1.7×, not met) |
| M6 | (merged into M5) | — |
| M7 | Big run | 10M+ deals, Wilson CI, results write-up, hard-deal gallery; exhaustive mode proves the golden unwinnable cases lost |
| M8 | GPU runner | only if the Phase 5 gate passed: hybrid runner built into `acc batch`, faster end to end, every GPU win verified |

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
- **Stack:** Rust engine/solver, TypeScript + WASM visualizer for selected deals, Python for analysis. CUDA prototype in Phase 5, built into the runner (Phase 8) only if it passes its gate.
- **Strategy:** Masten-style random prefix + brute force with a tuned escalation ladder.
  - Dead-end tests run only where they pay: none during random moves, then gated by piles left and event-triggered.
  - Suit/rank symmetry powers the endgame table, the endgame cache and canonical deal IDs.
  - Every unwon deal is documented and escalated.
- **User decks:** any entered 52-card deck (or in-progress position) can be validated and solved, from the CLI (`acc solve`) or the visualizer (WASM solver).

**Installed (Phase 0–1):** Rust (MSVC target) with Visual Studio 2022 Build Tools, and Node.js LTS. NVRTC 12.9, the CUDA runtime compiler, was added in `.tools/` for the Phase 5 GPU prototype (Phase 0 notes).
