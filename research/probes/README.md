# Research probes

Naive Python prototypes (about 220k nodes/s) used for the measurements in
[docs/RESEARCH.md](../../docs/RESEARCH.md). They measure the shape of the search space; they are
not the real engine. Run them from this folder with Python 3.

| Script | Measures | RESEARCH § |
|---|---|---|
| `probe.py width 5` | distinct states per depth from one deal | 5.1 |
| `probe.py reach 10 12 14 16`, `reach2.py 18 20 22 24` | all states reachable from a k-pile row | 5.2 |
| `patterns.py` | canonical suit/rank equality patterns per length (endgame table sizes) | 5.3 |
| `tradeoff.py` | random-prefix length vs win odds and cost | 5.5 |
| `schedule.py`, then `schedule_eval.py schedule_data.json` | per-deal data, then exact expected cost of restart schedules | 5.6 |
| `order.py` | move ordering and randomized Luby restarts | 5.7 |
| `deadends.py` | soundness and value of the orphan / connectivity / left-leaf dead-end tests | 5.8 |
| `depth_profile.py` | where brute-force nodes sit by pile count | 5.9 |
| `deadend_timing.py` | earliest possible and observed firing of each test; firing rate by piles left | 5.10 |
| `event_rates.py` | how often a move creates a cheap trigger event | 5.10 |
| `prefix_check.py` | filtering every random move vs one check at the end (writes `prefix_check_data.json`) | 5.10 |
| `symmetry.py` | endgame cache keyed by exact cards vs relabelled suits/ranks vs match matrix | 5.11 |
| `gpu_split.py schedule_data.json` | share of ladder work on deep (GPU-friendly) rungs | 9.3 |
| `no_tt.py` | whether small brute forces need a transposition table | 9.4 |
| `encoding_check.py` | card encoding, incremental move masks and hash updates vs full recomputation | 6.1–6.4 |
| `endgame_check.py 8` | independent count of winnable suit/rank patterns per pile count, to check `acc endgame build` | 10.5 |
| `gpu_smoke.py` | CUDA setup check: compiles a kernel with NVRTC, runs the match test for all card pairs on the GPU and compares with the CPU (Windows only: it loads the NVIDIA DLLs by name) | 9.1 |

`schedule_data.json` is the dataset behind §5.6 and §9.3.
