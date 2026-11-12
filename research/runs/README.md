# Run records

The speed logs and reports behind [docs/RESULTS.md](../../docs/RESULTS.md). `acc batch` writes them to `logs/` and
`checkpoints/`, which git ignores; these are exact copies. The 721 loss records are in
[hard-deals/proofs/](../../hard-deals/proofs/).

| Run | Seeds | Build | Speed log | Report |
|---|---|---|---|---|
| 100M | 0–99,999,999 | af14316 plus uncommitted changes | not kept | not kept; its figures are in docs/RESULTS.md |
| First billion | 0–999,999,999 | a761123 (the log says `f91bd67+dirty`, as the run started before that commit was made) | `batch-20261105-055704Z.csv` | not kept; its figures are in docs/RESULTS.md |
| Second billion | 1,000,000,000–1,999,999,999 | d167ccf | `batch-20261106-001633Z.csv` | `billion2-report.txt` |
| 20 billion | 2,000,000,000–21,999,999,999 | 926e105 | `batch-20261106-052839Z.csv` | `batch-2000000000-20000000000.report.txt` |
| Trillion (2026-11-09 to 11-12) | 0–999,999,999,999 | fbbff00 | `batch-20261109-060927Z.csv`, `batch-20261110-064914Z.csv`, `batch-20261110-130628Z.csv`, `batch-20261111-060545Z.csv`, `batch-20261112-035656Z.csv`, `batch-20261112-042833Z.csv`, one a session | `batch-0-2000000000000.report.txt`, with `batch-0-2000000000000.events.log` |

Each speed log has a row a second: time, deals done and left, deals/s, GPU wins/s, GPU positions/s and the GPU's backlog.
`billion2-report.txt` is the run's console output, progress bars included.

The trillion run was started as 2 trillion deals, so its files are named `batch-0-2000000000000` and the first speed logs say
so. On 2026-11-11, at 833,733,001,216 deals, it was paused and the count in its checkpoint lowered to 1,000,000,000,000; it then
finished normally, with its proofs, its check for repeated games and its report. It ran in 6 sessions, with pauses for the
battery, a Windows Update restart and that change. The report's `commits` line gives `fbbff00, a55ea74+dirty`: the second is
the repository's commit when it was resumed, read by `git`, while the program was fbbff00 throughout. The events log has a line
for each session start and each loss proved.

## Tests of the record

- `generator-tests.txt`: whether seeds give uniformly random deals, from `crates/core/examples/deal_stats.rs`.
- `losses.csv`: every proven loss re-derived and explained, from `testdata/tools/classify_losses.py`.

## After a long run

Copy its speed logs, its report (`checkpoints/<name>.report.txt`) and `.events.log` here, and fill in the table.
Then, in this order, with nothing else running, bring the record of losses up to date:

```bash
python testdata/tools/classify_losses.py hard-deals/proofs research/runs/losses.csv
```

Check every loss's certificate in Python, with no code shared with the program that built it:

```bash
python research/lean/automaton/check_certificates.py hard-deals/proofs research/lean/automaton/certificates.csv
```

Prove them in Lean's kernel (`build` as in [research/lean/README.md](../lean/README.md); 12 jobs need about 36 GB):

```bash
python research/lean/automaton/prove_all.py hard-deals/proofs research/lean/automaton/lean_results.csv build 12
```

Check those proofs again in nanoda, a separate kernel:

```bash
python research/lean/automaton/replay.py research/lean/automaton/lean_results.csv research/lean/automaton/replay_results.csv build 12
```

Then this prints every figure the paper takes from the run, and says if any loss is missing from the steps above:

```bash
python research/paper/paper_numbers.py
```

## A second generator

The win rate treats deals as uniform draws, which rests on the generator. To test that, the `second-generator` feature deals from
ChaCha20 with mask-and-redraw instead of xoshiro256\*\* with Lemire's method, sharing no code or method. The same seed gives another
deal, so its results go in folders of their own, and its report has a `generator` line. At the 0.7 losses per billion of the main run,
200 billion deals should give about 140 losses, in about 10 to 11 hours:

```bash
cargo build --release -p accordion-cli --features second-generator --target-dir target/chacha
```

```powershell
powershell -ExecutionPolicy Bypass -File C:\runs\guard.ps1 -Acc C:\runs\acc-chacha.exe -Count 200000000000 -Checkpoint checkpoints\chacha-0-200000000000.json -StartExtra "--dossiers hard-deals-chacha"
```

Its losses are compared by count only: Lean's `deal` is the main generator, so they have no proofs in Lean. The paper reports this
run, which follows the trillion run. Copy its report here as `chacha-0-200000000000.report.txt`, and `paper_numbers.py` compares it
with the main run.
