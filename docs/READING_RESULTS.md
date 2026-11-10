# Reading the results

What acc prints while a run goes and when it ends, what every word means, where its files are, and how to check its
results yourself. The examples are real output from the test laptop, on 2 threads without the GPU unless they say
otherwise.

## While a run goes

When a run starts, it says what it is doing:

```
same as: acc batch --from 202346700 --count 100 --threads 2 --gpu off
seeds 202346700..202346799 (100 deals), up to 50000000 positions per deal, 2 threads
speed log: logs\batch-20261110-005531Z.csv
checkpoint: checkpoints/batch-202346700-100.json; to pause, press Ctrl+C or create checkpoints/batch-202346700-100.pause
```

- `same as:` is the `acc batch` command that `acc run` stands for ([COMMANDS.md](COMMANDS.md#acc-batch)).
- `seeds`: the deals, how many positions one deal may search before it counts as not won, and the threads (with the GPU:
  `15 CPU threads + GPU`, and a line `GPU: <its name> (<n> multiprocessors), kernel compiled in <s> s`).
- `speed log`: a file with a row a second of the run's speed ([FORMATS.md](FORMATS.md#the-speed-log)).
- `checkpoint`: where the run is saved, every minute, and the file that pauses it if created.
- A `note:` is advice, such as that there are no endgame tables. A run can go on regardless.

Then **the progress line**, once a second where the output is a terminal (rewritten in place), or once a minute where it
goes to a file:

```
  █████████░░░░░░░  61.4%  122,880 done · 77,120 left · 1,018 deals/s (avg 1,563) · 1m 16s to go · 0 lost
```

| Part | Means |
|---|---|
| `█████████░░░░░░░  61.4%` | How much of the run is done. |
| `122,880 done · 77,120 left` | Deals done and left. "Done" counts deals in seed order, as the checkpoint does: a deal is done once it and every deal before it are. |
| `1,018 deals/s` | The speed over the last 10 seconds. |
| `(avg 1,563)` | The speed since this session started. |
| `1m 16s to go` | At the speed of the last 10 seconds. Past 48 hours, in days: `175d 09h to go`. |
| `0 lost` | Deals proven impossible so far. After it come only the counts that aren't zero: `1 proving`, `2 undecided`, `3 UNVERIFIED`, `4 PROOF FAILED` (meanings below). With proofs off (`--no-proofs`, `--prove 0`) it says `5 not won, no proofs` instead. |

From a trillion deals on, counts are short so that the line fits a terminal: `50.0T done · 50.0T left`, and the percentage
has two decimals.

A deal the solver doesn't win is announced, and proved while the run goes on:

```
seed 202346720: not won; proving it lost, up to 10000000000 positions a run
  ████████████████ 100.0%  100 done · 0 left · 1,094 deals/s (avg 1,094) · finished in 0s · 0 lost, 1 proving
finishing the proof of the deal not won
seed 202346720: independent run: lost after 100283 positions (74970 dead, 16 MB) in 0.06 s
seed 202346720: plain run:       lost after 379939 positions (74970 dead, 16 MB) in 0.16 s
seed 202346720: proven lost
```

When a run pauses it says so, and how to carry on:

```
pausing: finishing what is in hand (a second Ctrl+C stops at once, at the last checkpoint)
paused after 27854 of 200000 deals
carry on with: acc batch --resume checkpoints/batch-0-200000.json
2026-11-10 00:39:36 UTC Paused at 27,854 of 200,000 deals. acc resume carries on; acc results shows where it stands.
```

The last line is `acc run`'s (or `acc resume`'s); the others are the run's own.

## What the words mean

| Word | Means |
|---|---|
| **position** | One arrangement of the piles. The solver counts the positions it generates; it is the measure of how hard a deal was. |
| **won** | A winning line was found, and replayed move by move by a check separate from the search, which confirmed every move legal and one pile at the end. Nothing counts as won before that. |
| **not won** | The solver gave up on the deal. It is then either *exhausted* or *hit the limit*, and goes to the prover. |
| **exhausted** | The solver searched every line it could see and found no win. Not a proof: its table of lost positions is allowed to forget, and it skips lines it judges equivalent. |
| **hit the limit** | The solver reached its limit of positions for one deal (`--max-nodes`, 50 million) first. |
| **proving** | A deal not won that the prover is working on now. |
| **lost**, **proven impossible** | Two exhaustive searches, made differently, both went through every position reachable from the deal without finding a win ([ARCHITECTURE.md](ARCHITECTURE.md#how-results-are-checked)). The deal can't be won. |
| **undecided** | A proof search reached its limit (`--prove`, 10 billion positions) first, so the deal is neither won nor proven lost. It counts as a loss for the low end of the win rate's interval and as a win for the high end. |
| **UNVERIFIED** | A winning line that failed its check. This should never happen: it would be a bug, worth reporting. The deal doesn't count as won. |
| **PROOF FAILED** | The two proof searches disagreed, or a proof's winning line failed its check. Also a bug, worth reporting. |
| **stage** | Where a deal was won: `gpu` (the GPU's attempts), `ladder-0` to `ladder-6` (the processor's rungs of random-prefix attempts, cheapest first), `thorough` (the processor's full search), `prove` (the prover found a win the solver missed). |
| **same game** | Two deals that differ only in the names of the suits or ranks play exactly alike. The check for repeated games looks for those, not only for identical deals. |
| **95% CI** | The 95% confidence interval (Wilson's): the range in which the true win rate of all random deals lies, with 95% confidence, given this many deals. |

## The report, line by line

At the end a run prints its report. A run of more than a minute also saves it as `checkpoints/<name>.report.txt`, where
`acc results` shows it again. This is the report of the project's 20 billion deal run
([research/runs/batch-2000000000-20000000000.report.txt](../research/runs/batch-2000000000-20000000000.report.txt)), made by
an earlier version on the test laptop with its GPU:

```
deals            20000000000 (seeds 2000000000..21999999999)
won              19999999980 (every one verified by the reference rules); by stage: gpu 19997847246, ladder-0 244759, ladder-1 562320, ladder-2 678360, ladder-3 418683, ladder-4 168107, ladder-5 54944, ladder-6 17231, thorough 8330
not won          20 by the solver (19 exhausted, 1 hit the limit of 50000000 positions)
proofs           20 proven lost, 0 won, 0 undecided (two exhaustive runs of up to 10000000000 positions each)
failed to verify 0
distinct games   all 20000000000 deals differ, even with suits and ranks renamed
win rate         100.000000%   95% CI [100.000000%, 100.000000%] (Wilson; undecided deals counted as losses for the lower bound, wins for the upper)
positions/deal   mean 64686  median 39262  p90 138582  p99 418504  p99.9 970789  max 50000001 (2152754 deals solved on the CPU)
ms/deal (1 core) mean 8.99  median 6.65  p90 19.12  p99 38.36  p99.9 80.62  max 3155.9 (2152754 deals solved on the CPU)
GPU              NVIDIA T550 Laptop GPU: won 19997847246 of the 20000000000 deals it was given (99.99%), 0 lines failed verification; 553594.7M attempts, 2632M positions/s
CPU threads      verifying GPU lines 16.3% (2.6 us a line), the GPU's misses 6.0%, own deals 0.0%, waiting for the GPU 77.2%, handing results on 0.4%, other 0.1%
throughput       934074 deals/s, 7M CPU positions/s on 15 CPU threads + GPU (21411.6 s wall)
```

| Line | Says |
|---|---|
| `deals` | How many deals, and their seeds, first to last. |
| `won` | Deals won, and at which stage. "Verified" means replayed by a check separate from the search: the reference rules, or, for most of the GPU's lines since the second billion, a quicker check of the same rules, with 1 deal in 256 replayed by the reference engine as well ([RESULTS.md](RESULTS.md#how-every-win-is-checked)). |
| `not won` | Deals the solver gave up on, as exhausted or at the limit. Here 20. |
| `proofs` | What the prover found of those: proven lost, won after all, undecided; and `FAILED` if any proof failed. Missing with proofs off. |
| `failed to verify` | Winning lines that failed their check. Anything but 0 is a bug. |
| `repeat odds` | (Not in this report; this version adds it.) The chance that any two of the run's deals are the same game, or the same deal, if the generator that makes deals from seeds behaves like a random one. For a trillion deals it is about 1 in 10^33. |
| `distinct games` | The result of the check for repeated games. `all N deals differ` means every deal was checked. Past about 2.1 billion deals (or when memory is short) a run checks a sample instead, and the line says `not all checked`, how many deals the sample held, how many repeated pairs it would have seen with 95% chance, and how many near-matches it found against how many chance alone would give, which tests the generator. `NOT CHECKED` says why the check couldn't run; `not checked (--no-games-check)` that it was turned off. |
| `win rate` | Deals won divided by deals, and the 95% interval. This version prints 10 decimals (12 from 10 trillion deals), so it would have said `99.9999999000%   95% CI [99.9999998455%, 99.9999999353%]` here; the earlier version's 6 decimals rounded it to 100%. |
| `positions/deal` | The positions a deal took: mean, median, and the 90th to 99.9th percentiles and the most. With the GPU, over the deals solved on the processor only: mostly the GPU's misses, the harder ones. |
| `ms/deal (1 core)` | The same for time, in milliseconds on one thread. |
| `cost sample` | (Only for runs with more than 125 million deals solved on the processor.) The median and percentiles above cover the first 125 million of them, a fair sample as deals are random; the means and the most cover them all. |
| `GPU` | The card, the deals it won of those it was given, its lines that failed their check (anything but 0 is a bug), its attempts, and positions a second. `STOPPED EARLY` says if the GPU failed during the run. |
| `CPU threads` | What the processor's threads did, as shares of their time: checking the GPU's lines (and the time per line), solving the GPU's misses, solving deals of their own, waiting for the GPU, handing results on. Here the GPU set the speed, and the threads mostly waited for it. |
| `throughput` | Deals a second over the whole run, the processor's positions a second, the hardware, and the time it took (`over N sessions` if it paused and carried on). |
| `commits` | (Only if the run carried on with a different build.) The project's versions it ran on. |

`acc run` then sums it up in one line: `In short: 99 of 100 deals won; 1 proven impossible.`

## `acc status` and `acc results`

`acc status` shows where the runs here stand, from the status file each run writes beside its checkpoint every minute:

```
batch-0-200000 (seeds 0 to 199,999)
  state       going
  saved       2026-11-10 00:39:07 UTC (27 seconds ago)
  done        19,473 of 200,000 deals (9.7%), 180,527 left
  speed       540 deals a second (541 over the whole run); about 5m 34s to go at that speed
  won         19,473 so far
  lost        0 proven impossible
  proving     0 being proved
  undecided   0 not decided either way
  disk free   283 GB
  next        acc pause batch-0-200000 pauses it; acc results batch-0-200000 shows the win rate so far
```

`state` is `going`, `finishing` (every deal is done, the last checks are running), `paused` (with why, if it was the disk
or the GPU), `stopped unexpectedly` (a crash, a power cut or a closed window: `acc resume` carries on), or `finished`. The
figures are as of the last save, so they can be up to a minute old.

`acc results` on a run that isn't finished gives the win rate so far:

```
batch-0-200000 isn't finished: it is paused, at 19,473 of 200,000 deals (9.7%). These figures are as of its last save, 2026-11-10 00:39:07 UTC (3 seconds ago).
  won          19,473 of 19,473 deals
  win rate     100.000000% so far; with 95% confidence between 99.980277% and 100.000000%
The deals not decided yet count as lost for the lower figure and as won for the higher. The check that no two deals are the same game is made at the end of the run.
acc resume batch-0-200000 carries on; acc results batch-0-200000 shows the win rate so far
```

On a finished run it prints the report, and after either it lists the deals proven impossible, with their loss records.

## Where the files go

Everything is in the folder the run was started from.

| File | What it is |
|---|---|
| `checkpoints/<name>.json` | The checkpoint: where the run stands and its settings. Gone once the run has finished. `<name>` is `batch-<first seed>-<count>`. |
| `checkpoints/<name>.costs` | The positions and time of each deal solved on the processor (the first 125 million), for the report. Gone at the end. |
| `checkpoints/<name>.games` | The sample of the check for repeated games, for runs past about 2.1 billion deals. Gone at the end. |
| `checkpoints/games-<from>-<count>/` | Work files of the full check on disk, when `--games-disk` asks for it. Gone at the end. |
| `checkpoints/<name>.status.json` | Where the run stands, for `acc status`, rewritten every minute and kept at the end ([FORMATS.md](FORMATS.md#the-status-file)). |
| `checkpoints/<name>.events.log` | What has happened to the run: each session, pause, GPU failure and proof, with the time ([FORMATS.md](FORMATS.md#the-log-of-events)). |
| `checkpoints/<name>.report.txt` | The report, once finished (for runs of more than a minute). |
| `checkpoints/<name>.lock`, `.pause` | The lock held while the run goes, and the file that asks it to pause. |
| `checkpoints/<name>.old-1.*` | An earlier run of the same deals, kept by `acc run --new`. |
| `logs/batch-<time>Z.csv` | The speed log of one session ([FORMATS.md](FORMATS.md#the-speed-log)). |
| `output/hard-deals/seed-<seed>.json` | The dossier of a deal not won at first: what was tried, the best line, and how it ended ([FORMATS.md](FORMATS.md#dossiers)). |
| `output/hard-deals/proofs/seed-<seed>-loss.json` | The loss record of a deal proven impossible: a solve file with result `loss`. |
| `output/hard-deals/INDEX.md` | A table of the dossiers, and of the gallery (`--gallery`). |
| `output/hard-deals/gallery/` | The run's hardest deals won, as solve files, with `acc batch --gallery N`. |
| `endgame/endgame-<k>.bin` | The endgame tables, one file for each pile count up to the largest built. |

A run of less than a minute keeps none of the files in `checkpoints`: what it printed is all there is.

## Checking the results yourself

Every result can be checked again, on any computer, from the seeds alone.

- **A win.** The big runs keep no winning lines (at 51 bytes a line packed tight, 22 billion would take 1.1 TB), but a deal
  once won stays won: solve it again and check the line.

  ```
  acc solve --seed 42 --out win.json
  acc verify win.json
  ```

  `acc verify` replays every move with the reference rules, a deliberately simple version of the rules separate from
  the search: `line 1  seed 42  ok: win in 51 moves`. To keep every line of a run, give it `--save-wins wins.jsonl`
  (about 350 bytes a deal), then `acc verify wins.jsonl`.
- **A loss.** `acc verify output/hard-deals/proofs` checks that each loss record's moves are legal; a loss itself can't be
  confirmed by replaying a line, so it says how to prove it again:

  ```
  line 1  seed 202346720  ok: loss claimed (20 moves checked; a loss can't be confirmed by replay)
  line 1  to re-check this, run: acc prove --seed 202346720
  ```

  `acc prove --seed 202346720` makes both exhaustive searches again. With Python 3, an independent program that shares no
  code with acc searches every reachable position again and must find no win, and exactly as many positions as acc
  prove's plain search (from the project's folder):

  ```
  python testdata/tools/check_loss.py 202346720
  ```

  ```
  seed 202346720: 3H 5S QH 5C AD 7C ...
    reachable positions 74970, won False, fewest piles 32 (3 s)
  ```

  [research/lean](../research/lean/README.md) has a further check of the project's losses, as proofs for the Lean proof
  assistant.
- **The deals.** The deal of a seed is fixed by the generator in [FORMATS.md](FORMATS.md#deals-from-seeds): `acc deal --seed 42`
  prints it, and the Python version in `testdata/tools/make_golden.py` makes the same.
- **The program.** `acc selftest` checks the fast engine and the solver's shortcuts against slow, simple versions of the
  same rules, and `acc doctor --full` runs it with fewer cases.
