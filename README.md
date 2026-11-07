# Accordion Solitaire Solver

A fast solver for open Accordion solitaire (all 52 cards dealt face up; a pile moves onto the pile 1 or 3 to
its left when the top cards share a suit or rank; win = one pile), plus a browser visualizer for playing and
replaying selected deals.

- [PLAN.md](PLAN.md): what we build, in what order, and how each phase is accepted
- [docs/RESEARCH.md](docs/RESEARCH.md): rules, prior results, measurements and design rationale
- [docs/FORMATS.md](docs/FORMATS.md): cards, deck input, seeded deals, moves and solve files (`accordion-solve/1`)

## Layout

| Path | Contents |
|---|---|
| `crates/core` | `accordion-core`: cards, rules, engines, file formats |
| `crates/solver` | `accordion-solver`: search, schedules, endgame table, batch runner |
| `crates/cli` | `acc` command-line tool |
| `crates/wasm` | `accordion-wasm`: the core compiled to WebAssembly for the visualizer |
| `crates/gpu` | `accordion-gpu`: GPU ladder attempts (CUDA via NVRTC), run by `acc batch` next to the CPU; `accgpu` measures the GPU alone |
| `viz/` | Vite + TypeScript visualizer |
| `research/probes/` | Python probes behind the numbers in RESEARCH.md |
| `research/experiments/` | experiment configs and results |
| `hard-deals/` | dossiers for every deal not yet won |
| `testdata/` | golden deals and solutions shared by Rust and TypeScript tests |

## Build and run

Requires Rust (stable, via rustup) and Node.js LTS. On Windows, use Rust's MSVC target with Visual Studio Build Tools
(C++ workload); the GNU target can't build crates that depend on `windows-sys`.

```bash
cargo test --workspace
```

```bash
cargo run --release -p accordion-cli -- deal --seed 42
```

```bash
cargo run --release -p accordion-cli -- deal --cards "KS 7H 10d ..."
```

```bash
cargo run --release -p accordion-cli -- verify testdata/games/batch.jsonl
```

Build the endgame tables once: one bit per suit/rank pattern for up to 11 piles (7 minutes, 1.2 GB in `endgame/`, not in
git). `batch` and `solve` find and use them automatically, and run without them otherwise:

```bash
cargo run --release -p accordion-cli -- endgame build --piles 11
```

Solve one deal or position on every core (a deal that isn't won gets a dossier in `hard-deals/`):

```bash
cargo run --release -p accordion-cli -- solve --seed 42
```

```bash
cargo run --release -p accordion-cli -- solve --cards "KS 7H 10d ..." --escalate
```

Solve a range of deals and report the win rate with a confidence interval, and check that the deals are all different
games (`--config '<json>'` changes the strategy, see `accordion_solver::Config`). With an NVIDIA GPU, `batch` also runs
ladder attempts on it, which about doubles the deals per second here; every GPU win is verified on the CPU, and a deal
the GPU doesn't win goes through the CPU solver as usual (`--gpu off` turns it off):

```bash
cargo run --release -p accordion-cli -- batch --from 0 --count 100000 --out results.jsonl
```

Add `--gallery 20` to keep a run's 20 hardest deals as verified solve files in `hard-deals/gallery/`, listed in
`hard-deals/INDEX.md` and loadable in the visualizer. Results of the big runs are in [docs/RESULTS.md](docs/RESULTS.md).

A deal `batch` doesn't win is then proved lost if it can be, on a thread of its own while the run goes on, the way
`prove` does it (below). The run waits for the last proof, which can take an hour; `--prove <positions a run>` sets the
limit and `--prove 0` turns proofs off. A proven loss gets a `loss` record in `hard-deals/proofs/`.

A long `batch` can stop and carry on. **Ctrl+C pauses it:** it finishes what is in hand (under a second, or as long as a
hard deal takes), saves, and says how to carry on. A second Ctrl+C stops it at once. So does anything else that stops it,
a crash or a power cut included: every minute it saves a checkpoint, by default `checkpoints/batch-<from>-<count>.json`,
so only the last minute is done again. To pause a run with no console at hand, create the file beside the checkpoint that
ends `.pause`. Carry on from the same directory; the deals, settings and totals all come from the checkpoint, and the
report comes out as for one run:

```bash
cargo run --release -p accordion-cli -- batch --resume checkpoints/batch-0-100000.json
```

A checkpoint from before the `stepwise` and `split` settings went won't resume: finish that run with the build that started it.
Only `--threads`, `--checkpoint-every`, `--log-dir`, `--no-log`, `--games-memory`, `--no-games-check`, `--keep-free`, `--until`
and `--stop-after` can be given with `--resume`. A run that ends removes its checkpoint, and if it took over a minute it leaves its report beside where that
was (`.report.txt`). Starting a run whose checkpoint is still there is an error, so a paused run can't be overwritten by
accident; `--checkpoint <path>` puts it elsewhere and `--no-checkpoint` keeps none.

If the GPU fails, or no deal finishes for 15 minutes, the run stops rather than go on at the CPU's speed, and exits with code
75: a new process usually gets the GPU back. If the disk has under `--keep-free` left (5 GB), or checkpoints keep failing to
save, it pauses and exits with code 74: make room and carry on. For a run of days, `tools/long-run.ps1` sees it through: it
starts the run, or carries on if its checkpoint is there, and carries on again after a GPU failure or a crash until that stops
making progress. Give it a copy of `acc.exe` that has been seen to start, as Smart App Control can block a rebuilt one:

```powershell
powershell -ExecutionPolicy Bypass -File tools\long-run.ps1 -Acc C:\runs\acc.exe -From 22000000000 -Count 200000000000
```

A run can go in legs: `--stop-after 8` pauses it after 8 hours of the session and `--until 50000000000` once that many deals are
done, and either goes with `--resume` (or `-StopAfterHours` and `-Until` on the script). Beside the checkpoint,
`.status.json` says where the run stands, updated every minute, and `.events.log` says what has happened to it (sessions,
pauses, GPU failures, proofs), which outlasts the terminal's history. Neither takes the place of the report.

The report gives the odds that any two of the deals are the same game, by the birthday bound and on the condition that the
generator behaves like a random one: about 1 in 10³³ at a trillion deals. The check backs that condition. Up to `--games-memory`
(16 GB, 2 billion deals) it looks at every deal, at the end, and only then does the report say that all the deals differ. Past
that it looks at a sample, picked by a hash of the game as the deals are solved, 10 bytes each in up to `--games-disk` (by default
50 GB, or half of what is free if that's less), and the report says how many repeats it would have seen. Room for 5 bytes a deal
in `--games-disk` asks for the full check on disk instead, two more passes over the deals at the end. `--no-games-check` skips
the check. See "pausing, and tens of billions of deals" and "a trillion deals" in [docs/BENCHMARKS.md](docs/BENCHMARKS.md).

Prove a deal or position lost by two independent exhaustive searches, each with an exact (lossless) set of dead
positions. A typical winnable deal is far out of reach, but lost deals tend to be small: the three found in two billion
deals took 75 thousand, 3.4 million and 19 million positions. A proven loss is written as a `loss` record, and the deal's dossier
says `loss (proven)`:

```bash
cargo run --release -p accordion-cli -- prove --cards "KS 7H 10d ..."
```

Compare solver configurations on a fixed deal set (results append to `research/experiments/phase5/summary.jsonl`; see
[research/experiments/phase5/README.md](research/experiments/phase5/README.md)):

```bash
cargo run --release -p accordion-cli -- experiment --set validation --warmup 40 --repeat 3 --config '{"name": "default"}'
```

Check the fast engine against the reference engine (lockstep playouts), dead-end soundness, and that sleep sets and
endgame tables never change a verdict; and measure search speed and the dead-end gate settings (results in
[docs/BENCHMARKS.md](docs/BENCHMARKS.md)):

```bash
cargo run --release -p accordion-cli -- selftest --playouts 10000000 --positions 1000000 --residuals 1000000
```

```bash
cargo run --release -p accordion-cli -- bench --sweep
```

```bash
cargo bench -p accordion-core
```

The golden test data in `testdata/` is generated by an independent Python implementation of the formats:

```bash
python testdata/tools/make_golden.py
```

The GPU needs an NVIDIA driver for CUDA 12.9 or later and NVRTC, the CUDA runtime compiler, either on `PATH` or unpacked
in `.tools/` (PLAN.md Phase 0 notes). Without them, `batch` says so and runs on the CPU only. The kernel's tests run with
the rest (`cargo test --workspace`) and are skipped without a GPU. To measure the GPU on its own:

```bash
cargo run --release -p accordion-gpu -- --from 2000000 --count 100000
```

```bash
cd viz && npm install && npm run dev
```

`npm run dev` rebuilds the WebAssembly engine (`npm run wasm`) before starting Vite. In the visualizer you can deal by seed (type
it and press Enter), at random, or from your own deck; play with undo; press **Solve & replay** to have the solver find a win from
the position on screen and replay it move by move; or open a solve file to replay it with a slider and speeds.
