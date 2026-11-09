# Run records

The speed logs and reports behind [docs/RESULTS.md](../../docs/RESULTS.md). `acc batch` writes them to `logs/` and
`checkpoints/`, which git ignores; these are exact copies. The 23 loss records are in
[hard-deals/proofs/](../../hard-deals/proofs/).

| Run | Seeds | Build | Speed log | Report |
|---|---|---|---|---|
| 100M | 0–99,999,999 | af14316 plus uncommitted changes | not kept | not kept; its figures are in docs/RESULTS.md |
| First billion | 0–999,999,999 | a761123 (the log says `f91bd67+dirty`, as the run started before that commit was made) | `batch-20261105-055704Z.csv` | not kept; its figures are in docs/RESULTS.md |
| Second billion | 1,000,000,000–1,999,999,999 | d167ccf | `batch-20261106-001633Z.csv` | `billion2-report.txt` |
| 20 billion | 2,000,000,000–21,999,999,999 | 926e105 | `batch-20261106-052839Z.csv` | `batch-2000000000-20000000000.report.txt` |
| Trillion (planned) | 0–999,999,999,999 | 1089289 | to add | to add |

Each speed log has a row a second: time, deals done and left, deals/s, GPU wins/s, GPU positions/s and the GPU's backlog.
`billion2-report.txt` is the run's console output, progress bars included.

## Tests of the record

- `generator-tests.txt`: whether seeds give uniformly random deals, from `crates/core/examples/deal_stats.rs`.
- `losses.csv`: every proven loss re-derived and explained, from `testdata/tools/classify_losses.py`.

## After the trillion run

Copy its speed log, its report (`checkpoints/batch-0-1000000000000.report.txt`) and `.events.log` here, and fill in the table.
Then bring the record of losses up to date:

```bash
python testdata/tools/classify_losses.py hard-deals/proofs research/runs/losses.csv
```
