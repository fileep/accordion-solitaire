# Troubleshooting

Find the message you saw (search this page for a few of its words), or the exit code. Each entry says what it means,
whether any work is lost, and what to type next. Errors from acc start with `acc:`. If nothing here fits, run `acc doctor`
and see [Reporting a problem](#reporting-a-problem).

## Is my work lost?

Almost never. A run saves its checkpoint every minute, whole or not at all, and writes each dossier and proof as soon as
it has it. Whatever stops a run (Ctrl+C, `acc pause`, a full disk, the GPU failing, a crash, a power cut, a closed window,
a reboot), `acc resume` from the same folder carries on from the last checkpoint, and at most the last minute is done
again. The report at the end is the same as if the run had never stopped.

The only ways to lose a run are to delete its checkpoint (`checkpoints/<name>.json`) or the files beside it, or to move the
folder it started in.

## Exit codes

| Code | What happened | Next |
|---|---|---|
| 0 | Finished, or paused (it paused if `checkpoints/<name>.json` is still there). | Paused: `acc resume`. |
| 1 | An error: the message is on the line starting `acc:`. Or a check failed: see [UNVERIFIED](#unverified-proof-failed-failed-to-verify). | Find the message below. |
| 2 | acc couldn't read the command line. The message says which word. | `acc help <command>` lists the flags. |
| 74 | The run paused: the disk is nearly full, or saving kept failing. Nothing is lost. | [Disk nearly full](#less-than-5-gb-free). |
| 75 | The run paused: the GPU failed or stopped answering. Nothing is lost. | `acc resume` ([GPU stopped](#gpu-stopped)). |
| 130 | Stopped at once by a second Ctrl+C. The last checkpoint stands. | `acc resume`. |
| -1073741795 (Windows), "Illegal instruction" (Linux) | This copy of acc was built for a different processor. | [Illegal instruction](#illegal-instruction). |

## Installing and starting

### `cargo` or `acc` is not recognized

`cargo : The term 'cargo' is not recognized as the name of a cmdlet, function, script file, or operable program.` (or the
same for `acc`), or `command not found` on Linux and macOS.

The terminal doesn't know where Rust's programs are. Close the terminal and open a new one: a window opened before Rust
was installed doesn't know about it. If it still says so, on Windows, for this window:

```powershell
$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
```

On Linux and macOS: `. "$HOME/.cargo/env"`. To make it last, add `%USERPROFILE%\.cargo\bin` to your user Path in Windows'
environment variables (search Start for "environment variables"), or that line to `~/.bashrc` or `~/.zshrc`. `acc`, once
built with `cargo install`, is in the same folder.

### `error: linker 'link.exe' not found`

On Windows, Rust needs Microsoft's C++ build tools. Install the
[Build Tools for Visual Studio](https://visualstudio.microsoft.com/visual-cpp-build-tools/) with **Desktop development
with C++** ticked, open a new terminal, and build again. On Linux the same kind of message (`linker 'cc' not found`) means
a C compiler is missing: `sudo apt install build-essential`. On macOS: `xcode-select --install`.

### `An Application Control policy has blocked this file`

Windows' Smart App Control (or another application control) blocked a newly built `acc.exe`. It decides by the file's
contents, so the same build can be blocked once and allowed later, and a rebuild changes it. Build again
(`cargo install --path crates/cli --force`) and try once more; for a long run, use a copy of `acc.exe` that has been seen
to start, kept where a rebuild won't replace it. Whether to turn Smart App Control off is your decision (Windows Security,
App & browser control); acc doesn't need it off.

### Illegal instruction

The program stops at once with nothing printed and exit code -1073741795 on Windows, or `Illegal instruction` on Linux.
acc is built for the processor of the computer that builds it, and this copy came from another computer. Build it on this
one (`cargo install --path crates/cli --force`). To build one copy for many computers, see the note in
[.cargo/config.toml](../.cargo/config.toml).

### `not built for this CPU: slower than it could be`

A line of `acc doctor`: this copy of acc was built without instructions this processor has (built elsewhere, or for many
computers), so it runs, more slowly. Build it on this computer: `cargo install --path crates/cli --force`.

## Starting a run

### `no runs found in ...\checkpoints`

`acc: no runs found in C:\...\checkpoints. A run keeps its files where it was started, so acc must be run from that same folder`

`acc status`, `acc resume`, `acc pause` and `acc results` look in the `checkpoints` folder of the folder the terminal is
in. Go to the folder the run was started from (`cd`, then the folder), and try again. A run of less than a minute keeps no
files at all, so there is nothing to find after it: what it printed was its report.

### `this run is already going in another window`

The run is going in another window or in the background (its lock is held). `acc status` shows it; `acc pause` pauses it.
If no window is running it, a stuck acc process may be: it holds the lock until it ends. Nothing is lost.

### `these deals have been played here already`

`acc: these deals have been played here already (checkpoints\batch-0-200000.report.txt): acc results batch-0-200000 shows what they found. acc run --from 200k plays the next ones; adding --new plays these again`

`acc run` doesn't play the same deals twice in one folder by mistake. Play the next ones with the `--from` it suggests, or
add `--new` to play these again (the earlier run's files are kept, renamed `.old-1`).

### `a run stopped here is waiting at ...`

```
acc: a run stopped here is waiting at checkpoints/batch-0-1000000.json: carry on with `acc batch --resume checkpoints/batch-0-1000000.json`, or delete checkpoints/batch-0-1000000.* to start again
```

From `acc batch`: a paused or crashed run of the same deals is there, and starting again would throw its work away. Carry
on with it (`acc resume` does the same), or, to start again on purpose, delete its files as the message says, or use
`acc run --new`, which keeps them.

### `this run started in ...: resume it from there`

Also `<name> started in <folder>, and can only carry on from there`. A run carries on only from the folder it started in.
Go there (`cd "<folder>"`) and run the same command. If the folder was moved or renamed, put it back.

### `expected a whole number such as 5000, 2m, 1.5b or 100t`

`error: invalid value '1,000' for '--deals <DEALS>': expected a whole number such as 5000, 2m, 1.5b or 100t`

Counts can't have commas (shells split words at them). Write `1000`, `1_000` or `1k`. A count must also be whole: `1.5k`
is fine, `1.2345k` isn't.

### `--count must be at least 1`, `goes past the last seed there is`

A run needs at least one deal, and its last seed can be at most 18,446,744,073,709,551,615. Choose a smaller `--from` or
count.

### `your shell removed the quotes`

`acc: --config: key must be a string at line 1 column 2: your shell removed the quotes; put the JSON in a file and pass its name`

Windows PowerShell 5.1 drops double quotes inside arguments. Put the settings in a file (`acc settings --out my-settings.json`,
edit it, then `--settings my-settings.json`), or see [quoting in each shell](COMMANDS.md#quoting-in-each-shell).

### `unknown field`

```
acc: --config: unknown field `strat`, expected one of `start`, `end`, `step`, `tries`, `budget`, `growth`, `prefix`, `leap_weight`, `unstuck` at line 1 column 19
```

A settings file or `--config` names a field that doesn't exist: a spelling mistake, usually. The message lists the
fields that do; [COMMANDS.md](COMMANDS.md#settings-files) says what each means.

### `--save-wins can't go with --hours alone`

Every winning line takes about 350 bytes, so a run with no end could fill the disk. Give `--deals` as well; acc then says
how big the file will be.

### `--battery-resume (...) must be above --battery-pause (...)`

The run carries on at `--battery-resume` percent, so that must be above the percentage it pauses at.

## During a run

### `less than 5 GB free`

`less than 5 GB free in .: pausing; make room and carry on` (exit code 74), or `paused: the disk was nearly full` in
`acc status`.

The run paused before the disk filled. Nothing is lost. Make room on that disk (empty the recycle bin, move files
elsewhere), then `acc resume`. `--keep-free` (5 GB) sets the margin. The same exit code follows
`checkpoint: ...; trying again next time` five times in a row: something kept the checkpoint from being saved, such as a
program holding it open (a backup or antivirus scan); once that has stopped, `acc resume`.

### `checkpoint: ...; trying again next time`

One save of the checkpoint failed, usually because another program had the file open for a moment. The last checkpoint
stands, and the next save, a minute later, tries again. After five failures in a row the run pauses (exit code 74).

### Proving takes a long time

`seed N: not won; proving it lost, up to 10000000000 positions a run`, then a long wait, or `finishing the proofs of the N
deals not won` at the end. Most proofs take under a second; a rare one takes up to an hour. The run goes on meanwhile,
and waits only for the last at the end. Ctrl+C still pauses the run: a proof that wasn't finished is made again when the
run carries on.

### `GPU stopped`

`GPU stopped: ...` then `(pausing: the N deals it had in hand are done again)`, exit code 75; or
`no deal finished in 15 minutes: stopping; the last checkpoint stands`.

The GPU failed during the run, or stopped answering (the graphics driver may have reset it). The run paused rather than
go on at the processor's much slower speed. Nothing is lost. `acc resume` carries on, and a new process usually gets the
GPU back; `acc run` and `acc resume` do this by themselves after 60 seconds, up to 6 times in a row. If it keeps
happening: update the graphics driver, check the card's temperature, or go on without the GPU by starting a new run with
`--gpu off` (a paused run keeps its setting). On a slow card, launches that take near 2 seconds can be reset by Windows: a
smaller `batch` in the GPU's settings ([COMMANDS.md](COMMANDS.md#the-gpus-settings)) should help; this hasn't been tried.

### `stopped unexpectedly (last seen ...)`

In `acc status`: the run's process ended without pausing: a crash, a power cut, a closed window or a reboot. Nothing but
the last minute is lost: `acc resume` carries on.

### The run ended with no message

The window closed or the computer restarted (Windows Update, a power cut), or it ran out of memory. In the last case it
prints at most `memory allocation of ... bytes failed` and stops. `acc resume` carries on; to need less memory, carry on
with `acc batch` and a lower `--games-memory` (or `--no-games-check`):

```
acc batch --resume checkpoints/batch-0-1000000.json --games-memory 4
```

A proof's memory (`--prove-memory`) can only be chosen when a run starts. [LONG_RUNS.md](LONG_RUNS.md#how-long-and-how-much)
says how much a run needs.

### `acc batch stopped with exit code ... try 3 of 6 without getting further`

`acc run` or `acc resume` starts the run again after it stopped for a reason a new start may get past. After 6 tries in a
row that get no further it gives up and says where to look: `checkpoints/<name>.events.log` says what happened each time.

## The GPU

[GPU_SETUP.md](GPU_SETUP.md) has the whole setup. The messages:

| Message | Means | Next |
|---|---|---|
| `Your NVIDIA card could be used, but one free component (NVRTC) is missing. Run acc doctor for how to add it. Continuing on the CPU, which is much slower.` | The driver is there, NVRTC isn't. | `acc doctor` shows the steps: [GPU_SETUP.md](GPU_SETUP.md#2-add-nvrtc). |
| `no GPU (no NVIDIA driver found); solving on the CPU only` | No NVIDIA driver: no NVIDIA card, or no driver installed for it. | Nothing, without an NVIDIA card. With one, install its driver. |
| `the NVIDIA driver supports CUDA 12.4; the kernel needs 12.9 or later` | The driver is too old. | Update it from [nvidia.com/drivers](https://www.nvidia.com/drivers). |
| `the NVRTC found doesn't know this NVIDIA card (compute_XY), which is newer than it` | The card is newer than NVRTC 12.9. | A newer CUDA 12 NVRTC may know it: install the CUDA Toolkit 12 and set `ACC_NVRTC_DIR`. Not tried. |
| `the kernel didn't compile. NVRTC said: ...` | Not expected with the built-in kernel. | Please report it, with the output of `acc doctor`. |
| `no CUDA device: ...` | The driver found no usable card. | Check the card is enabled and the driver works (`nvidia-smi`). |
| `--gpu on: ...` | With `--gpu on`, acc refuses to start without the GPU, and says why after the colon. | Fix that, or use `--gpu auto` or `--gpu off`. |

## UNVERIFIED, PROOF FAILED, failed to verify

These words mean a check inside acc disagreed with the search: a winning line that failed its replay under the rules
(`UNVERIFIED`, `failed to verify 3`, `lines failed verification`), or two proofs that disagreed (`PROOF FAILED`). They
should never appear. The deal isn't counted as won or lost, and the run ends with exit code 1. **Please report it**, with the
run's report and its `checkpoints/<name>.events.log`, which names the seeds. `acc selftest` failing is reported the same
way.

## Notes

Lines starting `note:` are advice; the run goes on.

| Note | Means |
|---|---|
| `note: no endgame tables in endgame; ...` | Solving goes about 1.2 to 1.3 times faster on the processor with them: `acc endgame build --piles 11` (7 minutes, once). |
| `note: Windows refused to stay awake for the run; it may go to sleep` | Set the computer not to sleep while plugged in, or keep it busy. |
| `note: can't lock ...` | The disk can't lock files (some network drives), so acc can't stop the same run being started twice: don't. |
| `note: the system refused a lower priority; the run goes at the normal one` | `--gentle` couldn't lower the priority; it still uses half the threads. |
| `note: this run went as far as here on commit X, and carries on with Y` | The run carries on with a different version of acc. The report lists both. |

## Reporting a problem

See [CONTRIBUTING.md](../CONTRIBUTING.md#reporting-a-problem): what you typed, from which folder, what it printed word for
word, and the output of `acc doctor`.
