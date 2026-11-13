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
| Second generator, ChaCha20 (2026-11-12 to 11-13) | 0–99,999,999,999, other deals | 7b735bf, then 8137d5d | `batch-20261112-181044Z.csv`, `batch-20261112-182518Z.csv`, `batch-20261113-011935Z.csv`, `batch-20261113-012458Z.csv`, `batch-20261113-023159Z.csv`, `batch-20261113-023853Z.csv`, `batch-20261113-040439Z.csv`, one a session | `chacha-0-200000000000.report.txt`, with `chacha-0-200000000000.events.log` |

Each speed log has a row a second: time, deals done and left, deals/s, GPU wins/s, GPU positions/s and the GPU's backlog.
`billion2-report.txt` is the run's console output, progress bars included.

The trillion run was started as 2 trillion deals, so its files are named `batch-0-2000000000000` and the first speed logs say
so. On 2026-11-11, at 833,733,001,216 deals, it was paused and the count in its checkpoint lowered to 1,000,000,000,000; it then
finished normally, with its proofs, its check for repeated games and its report. It ran in 6 sessions, with pauses for the
battery, a Windows Update restart and that change. The report's `commits` line gives `fbbff00, a55ea74+dirty`: the second is
the repository's commit when it was resumed, read by `git`, while the program was fbbff00 throughout. The events log has a line
for each session start and each loss proved. The report's `won` line says every win was "verified by the reference rules":
that is the wording of the run's build. In fact 255 of every 256 GPU lines were checked by `quick::line_wins`, a third
implementation of the rules, 1 deal in 256 went through the reference engine as well, which also checks that the GPU had that
seed's deal, and every CPU win through the reference rules (docs/RESULTS.md, "How every win is checked").

The second-generator run ran in 7 sessions. Its first, on 7b735bf, was paused at 1,172,832,256 deals for a build with more
threads making deals for the GPU (8137d5d), which ran the rest; a Claude app update killed the second session at 14:23 local on
2026-11-12, and the third resumed from the last checkpoint, at 12,957,515,776. Its report's `commits` line gives
`7b735bf, 8137d5d, cbb7992, e3b37a7, 35ec01b`: the last three are the repository's commit when it was resumed, read by `git`, as
with the trillion run. Its 81 loss records are in [hard-deals-chacha/proofs/](../../hard-deals-chacha/proofs/), and
`losses-chacha.csv` is their check with the Python rules. The other speed logs of those days (`batch-20261112-180918Z`,
`-182359Z`, `batch-20261113-011841Z`, `-011907Z`) are short tests of the builds and of lowering the count, not part of the run.

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
ChaCha20 with mask-and-redraw instead of xoshiro256\*\* with Lemire's method: no generator code or way of drawing an index is
shared, and the Fisher–Yates shuffle is the same (docs/FORMATS.md, "The second generator"). The same seed gives another
deal, so its results go in folders of their own, and its report has a `generator` line. At the 0.7 losses per billion of the main run,
100 billion deals should give about 72 losses (the project's run was started as 200 billion and cut to 100 billion, as the
trillion was, so its files are named `chacha-0-200000000000`). It runs slower than the main generator's runs: making ChaCha20 deals on the
processor, about 5 times the work of a main deal, sets its speed (1.3–2M deals/s on the test laptop, against 5–6.6M):

```bash
cargo build --release -p accordion-cli --features second-generator --target-dir target/chacha
```

Then see it through with that build, as any long run ([docs/LONG_RUNS.md](../../docs/LONG_RUNS.md)), for example:

```powershell
powershell -ExecutionPolicy Bypass -File tools\long-run.ps1 -Acc path\to\acc-chacha.exe -From 0 -Count 100000000000 -Gpu on --dossiers hard-deals-chacha
```

or `acc run --from 0 --deals 100b --gpu on` with that build, which also pauses for the battery. The project's own run was seen
through by the author's battery watchdog, which isn't published, under the name `chacha-0-200000000000`.

It gave 81 losses in 100 billion deals, 0.810 per billion (exact 95% interval 0.643 to 1.007), against 0.721 in the trillion run:
a ratio of 1.12 (0.88 to 1.42; exact two-sided p = 0.33), so no sign that the generator matters. With about 72 losses expected,
the comparison would catch a difference of about 40% four times in five, and cannot rule out smaller ones. Its losses are proved in
Lean as theorems about `ChaCha.deal seed` ([research/lean/README.md](../lean/README.md)). `paper_numbers.py` compares the two runs
from `chacha-0-200000000000.report.txt`.
