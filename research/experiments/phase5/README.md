# Phase 5 experiments

Every measurement behind PLAN.md Phase 5 and the Phase 5 section of [docs/BENCHMARKS.md](../../../docs/BENCHMARKS.md).

- `summary.jsonl`: one line per run of one configuration over one deal set. Each line records:
  - date, commit, CPU;
  - the full solver configuration;
  - wins by stage;
  - positions and µs percentiles;
  - throughput.

  It is appended by `acc experiment`.
- `grids/`: the configuration lists, in the order they were run. Every entry spells out the settings that were defaults at
  the time (`luby_unit`, `ladder`, `search.sleep_sets`), so a grid means the same thing under later defaults.
- `sets/hard.txt`: the 1,000 deals of seeds 0–99,999 that took the most positions under the Phase 4 solver (made with
  `hardest.py` from a `--deals` run). It is biased against the Phase 4 configuration that picked it, so tail comparisons use
  p99/max over the unbiased train and validation sets instead.
- `deals/`: one row per deal, when a run used `--deals`. These files are large and not in git.

## Deal sets (disjoint)

| Set | Seeds | Use |
|---|---|---|
| train | 200,000–209,999 | tuning |
| hard | `sets/hard.txt` | the slow tail (see the caveat above) |
| validation | 300,000–309,999 | choosing between finalists |
| test | 1,000,000–1,099,999 | the final report only, run once |
| regression | 0–99,999 | must stay at 100% |
| GPU gate | 2,000,000–2,999,999 | the long CPU/hybrid comparison runs only |

## Reproducing

Position counts are deterministic for a given seed and configuration, so search changes compare exactly. Times on this
laptop drift with heat by up to ±25% between runs. They are only compared within one command: after `--warmup 40`, with
`--repeat 3` and the configurations interleaved, the spread is ±1–5%.

```bash
cargo run --release -p accordion-cli -- experiment --set train --grid research/experiments/phase5/grids/c3-order-sleep.json
```

```bash
cargo run --release -p accordion-cli -- experiment --set validation --warmup 40 --repeat 3 --config '{"name": "phase 5"}'
```

Endgame-table configurations need the tables first:

```bash
cargo run --release -p accordion-cli -- endgame build --piles 11
```

| Grid | Question |
|---|---|
| `c1-restarts`, `c1b-small-restarts` | Luby unit and geometric restarts for the thorough search |
| `c2-ladder` … `c2d-ladder` | random prefix + brute force: start, tries, budget, rungs, growth |
| `c3-order-sleep` | move-order policies (search and prefix) and sleep sets |
| `d1-engine` | probe-before-build, table depth, connectivity gate, left-leaf |
| `e1-endgame` | endgame tables to 6, 8, 9, 10 piles |
| `e2-patterns` | 11-pile tables; the symmetry-keyed cache for 12–16 piles, kept across deals |
| `g1-retune` | the ladder again, with sleep sets and 11-pile tables |

The GPU gate runs use `accgpu` (crates/gpu), not `acc experiment`; their commands are in docs/BENCHMARKS.md.
