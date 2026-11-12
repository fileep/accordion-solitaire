# Long runs

How to run acc for hours, days or longer: how big a run to choose, what it takes, how to pause and carry on, and what to
do on a laptop. [GETTING_STARTED.md](GETTING_STARTED.md) comes first; the commands are in [COMMANDS.md](COMMANDS.md).

## How long and how much

These are **estimates**, worked out from measured speeds and sizes; no run of most of these sizes has been made with this
version. The speeds are the test laptop's (Intel i7-1260P, 16 threads, NVIDIA T550 laptop GPU, Windows 11):

- **With the GPU**, about 5 million deals a second: the trillion run averaged 5.0 million over 56 hours, and held 6.4 to
  6.7 million a second in its best hours; 4.5 to 5.3 million were measured on a warmer desk
  ([BENCHMARKS.md](BENCHMARKS.md), November 2026). A bigger card should be faster; none has been tried.
- **On the processor alone**, about 15,000 deals a second on 16 threads: runs in Phase 7 held 13,422 to 19,512
  ([RESULTS.md](RESULTS.md)). That was older code, and the processor alone hasn't been timed since, so today's figure may be
  somewhat higher.

| Deals | Time with the GPU | Time on the processor alone | Disk, at most | Memory, at most | Deals proven impossible (expected) |
|---|---|---|---|---|---|
| 10,000 (`--quick`) | seconds | about a second | almost none | 2 GB | none |
| 1 million (the default) | seconds | about a minute | 16 MB | 2 GB | none |
| 100 million | 20 s | 2 hours | 1.6 GB | 4 GB | none, usually |
| 1 billion | 3 to 4 minutes | 19 hours | 2 GB | 11 GB, at the end | about 1 |
| 10 billion | 35 minutes | 8 days | 52 GB | 20 GB, at the end | about 10 |
| 100 billion | 6 hours | 2.5 months | 52 GB | 20 GB, at the end | about 100 |
| 1 trillion | 2.3 days | 2 years | 52 GB | 20 GB, at the end | about 1,000 |
| 10 trillion | 23 days | not practical | 53 GB | 20 GB, at the end | about 10,000 |
| 100 trillion | 7.5 months | not practical | 55 GB | 20 GB, at the end | about 100,000 |

The disk and memory are the most a run needs at any time, on the processor alone and with the endgame tables to 11
piles built; with the GPU, or without the tables, a little less. The memory leaves out a rare long proof: one that is
still going at the end, beside the check for repeated games, can take up to `--prove-memory` (8 GB) more. Where they go:

- **The check for repeated games** ([READING_RESULTS.md](READING_RESULTS.md#the-report-line-by-line)). Up to about 2.1 billion
  deals it checks every deal at the end, in memory: 8 bytes a deal, so 8 GB for a billion. Past that, or past half the
  computer's memory with `acc run`, it checks a sample, noted on disk as the run goes: 50 GB, or half of the free space if
  that is less. That sample is most of the disk in the table, and it is read back in parts of up to 16 GB at the end.
- **Each deal solved on the processor** costs 16 bytes (its positions and time, for the report's percentiles), in memory
  and on disk, for the first 125 million such deals only: 2 GB at most. With the GPU only about 1 deal in 9,000 is solved
  on the processor; without it, every deal is.
- **The endgame tables**, if built to 11 piles: 1.15 GB on disk and about as much memory.
- **Proofs** of the deals the solver doesn't win take a few MB each, and rarely up to `--prove-memory` (8 GB) for each of
  their two searches. About 1 deal in 1.4 billion is proven impossible (721 in the project's trillion), and each gets a
  dossier and a loss record of a few KB in `output/hard-deals`: under 1 GB at 100 trillion.
- **The speed log** grows by about 6 MB a day.
- `--save-wins` (`acc batch --out`) adds about 475 bytes a deal: 475 GB a billion. It is refused with `--hours` alone.

At the end of a run of billions of deals, the check for repeated games and the last proofs add some minutes. The largest
run the check can take is 281 trillion deals (2^48), and `acc run` takes no more; a longer one has to be split into runs
of their own (`--from`), or made with `acc batch --no-games-check`.

## Choosing the deals

A run plays consecutive seeds, from `--from` on. The same seed is the same deal everywhere, so:

- To **check** the project's results, play seeds it has played: 0 to 999,999,999,999 (a trillion) are in
  [RESULTS.md](RESULTS.md).
- To find **new** results, choose seeds no one has used, and say which in what you publish. Seeds go up to
  18,446,744,073,709,551,615; anything from 10 trillion (`--from 10t`) up was unused by this project as of November 2026.
- Runs that start where the last ended never repeat a deal: `acc run` says what to type for the next range when it finishes.

```
acc run --from 10t --deals 1b
```

## Starting

- `acc run --deals 5b` plays 5 billion deals and is done when they are.
- `acc run --hours 8` plays for 8 hours and pauses; `acc run --overnight` is the same. Without `--deals`, a run with hours
  has 100 trillion deals, so it is the hours that end each session. The same command carries on for another 8 hours, as
  does `acc resume --hours 8`; `acc resume` alone carries on with no time limit, until it is paused.
- `acc run --deals 100b --hours 8` plays 100 billion deals, 8 hours at a time.
- `--gentle` leaves the computer usable meanwhile: half the threads, a lower priority, and the GPU left alone.
- `--dry-run` shows the `acc batch` command it would run, and stops.

Before a run with hours, acc says: `Leave this window open, keep the charger in and the lid open. Ctrl+C pauses safely.`

## Pausing

Any of these pauses a run. It finishes the deals in hand (usually under a second), saves, and stops; nothing is lost.

- **Ctrl+C** in its window. A second Ctrl+C stops it at once instead, losing at most the last minute.
- **`acc pause`** in another window, from the same folder. It creates the file `checkpoints/<name>.pause`, which the run
  watches for; creating that file any other way works too.
- **The hours**: `acc run --hours H` or `acc resume --hours H` (`acc batch --stop-after H`).
- **A number of deals**: `acc batch --until N` pauses once N of the run's deals are done.
- **The battery guard**, below.

A run also pauses by itself when the disk is nearly full (less than 5 GB free: exit code 74), and when the GPU fails (75).
[TROUBLESHOOTING.md](TROUBLESHOOTING.md) says what to do then.

## Carrying on

```
acc resume
```

From the same folder. With one paused run here it carries on with it; with several, it lists them and asks which
(`acc resume batch-0-1000000`). `acc status` shows where each stands, and `acc results` the win rate so far.

What can change on carrying on: the threads, the hours, `--gentle`, the battery settings. What can't: the deals, the
solver's and the GPU's settings, whether it uses the GPU, proofs, `--save-wins`; they come from the checkpoint, so the
report comes out as if the run had gone in one go. To change them, start a new run (`acc run --new` keeps the old one's
files, renamed).

A run carries on only from the folder it started in, as its files' paths are relative to it. If you moved the folder, move
it back.

A run started by an older acc, from before runs took a lock, is a special case: nothing can tell for sure that it isn't
still going (in its last checks it can write nothing for hours), or that a script seeing it through, such as
`tools/long-run.ps1`, isn't about to start it again. Two processes on one checkpoint would spoil the run, so `acc resume`
and `acc run` refuse such a run until you add `--force`, which you should do only once you are sure nothing is running
it. Once this version has carried on a run whose file of costs holds more than 125 million deals, finish it with this
version too: an older one would report the percentiles' sample as if it were all of them.

## Crashes, power cuts and reboots

A run saves its checkpoint every minute, writing it whole or not at all, so a crash, a power cut, a closed window or a
reboot loses at most the last minute of work. Afterwards `acc status` shows the run as `stopped unexpectedly`, and the same
`acc resume` carries on from the last checkpoint.

Nothing starts a run again by itself after a reboot: open a terminal, go to the run's folder, and type `acc resume`. To have
it start at logon, you could add a task to the Windows Task Scheduler (or a systemd user service on Linux); that is a change
to your system, so it is left to you, and it hasn't been tried with acc.

`acc run` and `acc resume` start a run again by themselves when it stops for a reason that a new start may get past (the
GPU failing, a crash), after 60 seconds, up to 6 times in a row without progress. `--once` turns this off.

## Laptops

A laptop can run for days if it is set up for it. What acc does by itself:

- It keeps the computer from going to sleep while a run goes (on Windows; the screen may still turn off). Closing the lid
  still puts it to sleep, unless you change that setting.
- `acc run` and `acc resume` watch the battery (Windows; Linux too, not tested yet; not macOS). When it falls under 30%
  they pause the run, and once the charger is in and the battery is back to 60% they carry on with it. If the battery falls
  under 10% while the pause is still finishing, after a minute they stop the run at once; its last checkpoint stands.
  `--battery-pause` and `--battery-resume` change the two figures, and `--no-battery-guard` turns this off. It never
  changes a setting of the computer. A battery that is under 30% when a run starts makes it wait the same way first, with
  a line that says so (Ctrl+C stops waiting). A run of 10,000 deals or fewer, such as `--quick`, is over in moments, and
  the guard leaves it alone unless the battery is under 10%.

Why a run can drain a battery on the charger: a laptop under full load can use more power than its charger gives. On the
test laptop, the trillion run took the battery down 5.5 to 6.8 Wh an hour with the processor and the GPU at
full speed ([BENCHMARKS.md](BENCHMARKS.md), "Holding the battery level"). The guard is for that.

What you may choose to set yourself (acc changes none of these):

- **The charger in, and the lid open**, or "When I close the lid: Do nothing" (Windows: Control Panel, Power Options,
  "Choose what closing the lid does").
- **Sleep** after a time on the charger: acc already keeps the computer awake while a run goes, but not between runs.
- **Windows Update** restarts: set active hours (Settings, Windows Update, Advanced options), or pause updates for the run.
  After a restart, `acc resume` carries on.
- **The power mode** (Settings, System, Power): "Best performance" is fastest; "Balanced" is cooler and drains the battery
  more slowly. `--gentle` or fewer `--threads` lower the load too.
- **Heat:** give the laptop room to breathe, on a hard surface. Hot hardware slows itself down, which costs speed but no
  results.
- **The critical battery action** (Power Options, advanced settings): Windows hibernates or shuts down at a critical level
  even on the charger; the guard's pause comes before that with the default settings.

On Linux, start acc under `systemd-inhibit` to keep the computer awake (`systemd-inhibit acc run --overnight`); on macOS,
under `caffeinate -i` (`caffeinate -i acc run --overnight`). Neither has been tried with acc yet.

## Scripts in `tools/`

For most people `acc run` and `acc resume` do everything needed. The scripts are for seeing a long `acc batch` through
with a program file of your choosing. (The project's trillion-deal run was seen through by the author's own battery watchdog,
which isn't published; `acc run`'s battery guard now does the same job.)

- `tools/long-run.ps1` (Windows) starts the run, or carries on with it, and carries on again after a GPU failure or a crash
  until it stops making progress:

  ```powershell
  powershell -ExecutionPolicy Bypass -File tools\long-run.ps1 -Acc .\target\release\acc.exe -From 0 -Count 1000000000
  ```

  `-Gpu on` refuses to start without the GPU, and `-StopAfterHours 8` pauses each session after 8 hours. For a run of
  days, give it a copy of `acc.exe` that has been seen to start, kept somewhere a rebuild won't replace it: Windows' Smart
  App Control can block a newly built program.
- `tools/long-run.sh` does the same with `sh` on Linux and macOS (not tested there yet):
  `sh tools/long-run.sh --acc ./target/release/acc --from 0 --count 1000000000`.
- `tools/get-nvrtc.ps1` and `tools/get-nvrtc.sh` fetch NVRTC for the GPU ([GPU_SETUP.md](GPU_SETUP.md)).

## The project's published record

The `hard-deals` folder of the project holds the dossiers and proofs of the project's own runs, as published. Runs write
their own to `output/hard-deals` instead, so that they never change that record. To add to it on purpose, give
`--dossiers hard-deals` to `acc batch`, `acc solve` or `acc prove`.
