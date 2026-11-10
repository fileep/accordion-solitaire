# Commands

Every command of `acc`, with every flag, by what it is for. `acc help` lists the commands, and `acc help <command>` (or
`acc <command> --help`) shows a command's flags and examples. A test checks that every command and every flag the program
has is on this page.

- **Start here:** [`acc doctor`](#acc-doctor), [`acc run`](#acc-run), [`acc status`](#acc-status), [`acc pause`](#acc-pause),
  [`acc resume`](#acc-resume), [`acc results`](#acc-results)
- **One deal:** [`acc deal`](#acc-deal), [`acc show`](#acc-show), [`acc solve`](#acc-solve), [`acc prove`](#acc-prove),
  [`acc verify`](#acc-verify)
- **Advanced:** [`acc batch`](#acc-batch), [`acc settings`](#acc-settings), [`acc endgame build`](#acc-endgame-build),
  [`acc selftest`](#acc-selftest)
- **For developers** (not in `acc help`'s list): [`acc bench`](#acc-bench), [`acc experiment`](#acc-experiment)
- [Settings files](#settings-files), [exit codes](#exit-codes), [quoting in each shell](#quoting-in-each-shell),
  [environment variables](#environment-variables)

**Counts** (numbers of deals, positions and the like) can be written `5000`, `5_000`, `2m`, `1.5b`, `100t` or `1e12`: k, m,
b and t are a thousand, a million, a billion (10^9) and a trillion (10^12), in either case. Commas are refused, as some
shells split words at them. A count must come out whole: `1.5k` is 1500, `1.2345k` is refused.

**Seeds** number the deals from 0 to 18,446,744,073,709,551,615 (2^64 - 1). `--from` is the first seed of a run, and the run
plays that many consecutive seeds.

## Start here

### `acc doctor`

Checks that this computer is ready: the build, the processor, memory, disk space, the folder, the endgame tables, the
NVIDIA driver and NVRTC, the runs here, the battery, and a few seconds of solving (100 deals on 2 threads, on the processor).
Each line is PASS, WARN or FAIL, with what to do. It exits with 0 unless a line is a FAIL.

```
acc doctor
```

| Flag | Default | What it does |
|---|---|---|
| `--gpu` | off | Also tries the NVIDIA card: compiles the GPU code and solves 20,000 deals with it. It says so first if a run is going here, as the test then shares the card with it. |
| `--full` | off | Also runs `acc selftest` with fewer cases than its defaults (a minute or two, on every thread). |

The NVIDIA line only looks for the driver and NVRTC; it doesn't use the card, so it is safe while a run uses the card.

### `acc run`

Plays a range of deals and counts how many are won. It shows the `acc batch` command it stands for, runs it, pauses and
carries on as needed, and at the end says the result in one line and what to type next.

```
acc run --quick
acc run --deals 50m --gentle
acc run --overnight
acc run --from 1m --deals 1m
```

| Flag | Default | What it does |
|---|---|---|
| `--deals N` | a million (with `--hours`, as many as the hours allow) | How many deals to play: at most 281,474,976,710,656 (2^48), the most the check for repeated games can take. |
| `--hours H` | none | Pause after H hours (`0.5` is half an hour). With `--deals` too, whichever comes first. The same command carries on for another H hours, as does `acc resume --hours H`; `acc resume` alone has no time limit. |
| `--overnight` | off | The same as `--hours 8`. |
| `--quick` | off | 10,000 deals, to try it out. |
| `--from S` | 0 | The first deal's seed. Runs that start where the last one ended never repeat a deal. |
| `--gpu auto\|on\|off` | auto (off with `--gentle`) | Use an NVIDIA card: if there is one that works, always (refuse to start without it), or never. |
| `--save-wins FILE` | none | Save every winning line in FILE (about 475 bytes a deal), to check with `acc verify`. Refused with `--hours` alone, as the file could fill the disk; with `--deals` the size is said first. |
| `--settings FILE` | none | Solver settings from a file ([settings files](#settings-files)); `acc settings --out FILE` writes one to edit. |
| `--gpu-settings FILE` | none | GPU settings from a file; `acc settings --gpu --out FILE` writes one. |
| `--no-proofs` | off | Don't try to prove that the deals not won can't be won. |
| `--new` | off | Play these deals again although a run of them is paused or finished here. The old run's files are kept, renamed `<name>.old-1.*` (then `.old-2`, and so on), and are no longer listed as a run. |
| `--force` | off | Go ahead with a run of these deals that an older acc started, which takes no lock, once you are sure nothing is running it (see `acc resume`). |
| `--dry-run` | off | Print the `acc batch` command this stands for, and stop. |
| `--threads N` | every thread (with `--gentle`, half) | Processor threads to use. |
| `--gentle` | off | Leave the computer usable: half the threads, a lower priority, and no GPU unless `--gpu` asks for it. |
| `--once` | off | Don't start the run again by itself when it stops with an error. |
| `--no-battery-guard` | off | Don't pause the run when the battery runs low. |
| `--battery-pause PCT` | 30 | Pause the run when the battery falls under PCT percent. |
| `--battery-resume PCT` | 60 | After a pause for the battery, carry on once the charger is in and the battery is back to PCT percent. |

How it behaves:

- The run is named after its deals, `batch-<from>-<count>`, and keeps its files in `checkpoints` in the current folder.
  If a run of the same deals is paused here, `acc run` carries on with it, with the settings it began with, and says so;
  `--gpu`, `--settings`, `--gpu-settings`, `--no-proofs` and `--save-wins` can't change partway and are then left as they
  were. If those deals have been played here already, it refuses and says so, unless `--new` is given.
- It passes `--games-memory` and `--prove-memory` to `acc batch` only when the computer has less memory than their
  defaults assume: half of the memory, at most 16 GB, for the check for repeated games, and a quarter, at most 8 GB, for
  each proof.
- **Starting again by itself** (as `tools/long-run.ps1` does): when `acc batch` ends with exit code 0 the run is finished
  or paused (finished if its checkpoint is gone), and `acc run` stops. With 74 (the disk is nearly full), 130 (stopped at
  once by a second Ctrl+C), 1 or 2 (an error, which a new start would only repeat) it stops and says what to do. With any
  other code, such as 75 (the GPU failed) or a crash, it waits 60 seconds and starts it again, from its last checkpoint; it
  gives up after 6 tries in a row that get no further. `--once` turns this off.
- **Ctrl+C** reaches the run, which pauses (finishing what it has in hand, usually under a second), and `acc run` stays to
  say where it stopped. A second Ctrl+C stops the run at once; the last checkpoint, at most a minute old, stands.
- **The battery guard**, on a computer with a battery (Windows; Linux too, not tested there yet; not on macOS): every 30 seconds it reads
  the battery. Under `--battery-pause` it asks the run to pause; once the charger is in and the battery is back to
  `--battery-resume`, it carries on with it. If the pause takes more than 60 seconds while the battery is under 10%, it
  stops the run at once (the last checkpoint stands). A battery that can no longer be read counts as low. A battery under
  `--battery-pause` when the run starts makes it wait in the same way first, and it says so. A run of 10,000 deals or
  fewer is left alone unless the battery is under 10%. It changes no setting of the computer. Without a battery it does
  nothing.

### `acc status`

Shows how the runs started in this folder are going, from the status file each keeps beside its checkpoint (it never
opens a checkpoint itself). With one run, or a name, it shows it in full: its state, deals done of all, speed, deals won,
lost, being proved and undecided so far, free disk, and what to type next.

```
acc status
acc status batch-0-1000000
```

| Argument | What it is |
|---|---|
| `NAME` | Which run, as the list names it (`batch-0-1000000`, or `0-1000000`, or one of its files). Default: all of them. |

A run is **going** while it holds its lock file. A run from an acc older than the lock is judged by the times in its
files: going while its status says running or finishing and was written in the last 5 minutes, or while a session it has
started (as its log of events says) has yet to save for the first time, within 5 minutes; **finishing when last seen** if
its status says finishing but is older, as such an acc can go for hours in its last checks without writing it. A run that
is neither going nor paused, but has a checkpoint, **stopped unexpectedly** (a crash, a power cut, a closed window): `acc
resume` carries on with it. The files `acc run --new` kept (`.old-1`) aren't listed. If nothing is found, it says which
folder it looked in: acc must be run from the folder where the run started, and a run that took under a minute keeps no
files.

### `acc pause`

Pauses a run that is going in another window, by creating the file it watches for (`<checkpoint>.pause`), and waits up to
30 seconds for it to take the request, saying when it has paused. A run that may be going (one of an older acc, finishing
when last seen) is asked too; if it doesn't take the request in time, the file is removed again. Ctrl+C in the run's own
window does the same.

```
acc pause
acc pause batch-0-1000000
```

| Argument | What it is |
|---|---|
| `NAME` | Which run, when several are going here. With one, it is used; with none, `acc pause` refuses. |

### `acc resume`

Carries on with a paused run, or one that stopped unexpectedly, with the same supervision as `acc run`. With one such run
here it is used; with several it lists them and asks for a name. It refuses a run that is going, and one that started in
another folder. It also refuses a run that an older acc started, as that acc takes no lock: nothing can be sure that it,
or a script that starts it again (such as `tools/long-run.ps1`), isn't going, and two processes on one checkpoint would
spoil the run. `--force` carries it on all the same, once you are sure.

```
acc resume
acc resume batch-0-1000000
acc resume --hours 2 --gentle
```

| Flag | Default | What it does |
|---|---|---|
| `NAME` | the one paused run | Which run, when several are paused here. |
| `--hours H` | none | Pause again after H hours. Without it there is no time limit, even for a run started with `--hours`. |
| `--overnight` | off | The same as `--hours 8`. |
| `--force` | off | Carry on with a run that an older acc started, once you are sure nothing is running it. |
| `--dry-run` | off | Print the `acc batch` command this stands for, and stop. |
| `--threads N` | every thread (with `--gentle`, half) | Processor threads to use. |
| `--gentle` | off | Half the threads and a lower priority. A run that uses the GPU keeps using it. |
| `--once` | off | Don't start the run again by itself when it stops with an error. |
| `--no-battery-guard` | off | Don't pause the run when the battery runs low. |
| `--battery-pause PCT` | 30 | Pause the run when the battery falls under PCT percent. |
| `--battery-resume PCT` | 60 | After a pause for the battery, carry on at PCT percent with the charger in. |

Everything else (the deals, the solver's and the GPU's settings, proofs, `--save-wins`) comes from the checkpoint.

### `acc results`

Shows what a run has found. For a finished run, its report, after one plain line. For one that isn't finished, the win
rate so far with its 95% interval, counted as `acc batch` counts it (the deals not decided yet count as lost for the lower
figure and as won for the higher), a word on each count that isn't zero, and that the check for repeated games comes at
the end. Either way it then lists the deals proven impossible so far, with their loss records.

```
acc results
acc results batch-0-1000000
```

| Argument | What it is |
|---|---|
| `NAME` | Which run, when there are several here. |

## One deal

### `acc deal`

Prints the 52 cards of a numbered deal, or checks that a deck you typed or pasted is the 52 different cards.

```
acc deal --seed 42
acc deal --file deck.txt
acc deal --seed 42 --json
```

| Flag | Default | What it does |
|---|---|---|
| `--seed N` | | The deal for this seed. |
| `--cards "..."` | | A deck as text: 52 cards like `"KS 7H 10d AC"`, separated by spaces or commas ([FORMATS.md](FORMATS.md#deck-input)). |
| `--file FILE` | | Read the deck from a file, or `-` for standard input. |
| `--json` | off | Print a solve-file record (result `unknown`, no moves) instead of the bare cards. |

One of `--seed`, `--cards` and `--file` is needed.

### `acc show`

Shows a deal laid out to read, in rows of 13, with the rules in one line; and with `--solve`, or for a solve file, a way to
win it, move by move in words (`12. 7H onto 7C, 31 piles left`).

```
acc show --seed 42
acc show --seed 42 --solve
acc show win.json
acc show --cards "2S 2C KS TH 6H 4C 6D 9D 4S QH 8C 8D 8S 7C AC 4D 6S 2D 5S 3S AH TC 5H 3D QS JC JS 9C 5D AD 6C 3C JD QD 4H TD AS 2H KC 7H QC 3H KD TS KH 8H JH 9S 7S 9H 7D 5C" --solve
```

(The last is deal 42, typed in. A deck must have all 52 cards: `acc deal --seed N` prints one to start from.)

| Flag | Default | What it does |
|---|---|---|
| `FILE` | | A solve file (the line in it is checked, then shown), a dossier (its deal), or a file holding a deck. |
| `--seed N` | | Deal number N. Seeds take the short forms of counts too: `--seed 1m` is deal 1,000,000. |
| `--cards "..."` | | A deck as text, as for `acc deal`. |
| `--solve` | off | Find a winning line first, as `acc solve` does. |
| `--threads N` | 0 (every thread) | Threads for `--solve`. |

### `acc solve`

Finds a winning line for one deal (or a game part-played) with every thread, checks it with the reference rules, and
prints it as a solve file ([FORMATS.md](FORMATS.md#solve-files)). A deal it doesn't win gets a dossier.

```
acc solve --seed 42
acc solve --seed 42 --out win.json
acc solve --file deck.txt --escalate
```

| Flag | Default | What it does |
|---|---|---|
| `--seed N` | | The deal for this seed. |
| `--cards "..."` | | A deck as text. |
| `--file FILE` | | Read the deck from a file, or `-` for standard input. |
| `--position "..."` | | A game in progress: piles separated by `\|`, each listed bottom to top (all 52 cards). |
| `--max-nodes N` | 200m | Positions to search in all, across the threads, before giving up. |
| `--threads N` | 0 (every thread) | Threads to use. |
| `--rng-seed N` | 1 | Seed for the order in which the search tries moves, so that a search can be repeated exactly, with `--threads 1` (threads race each other to a win, so with more the positions counted vary from run to run). |
| `--out FILE` | standard output | Write the solve file here. |
| `--escalate` | off | If not won, try again with 10 times and then 100 times the limit, with fresh random orders, before giving up. |
| `--dossiers DIR` | `output/hard-deals` | Where a deal that isn't won gets its dossier. |
| `--config SETTINGS` | the defaults | Solver settings: a JSON file's name, or the JSON itself ([settings files](#settings-files)). `--max-nodes` and `--rng-seed` override theirs. |
| `--endgame-dir DIR` | `endgame` | Where the endgame tables are. |
| `--endgame auto\|off\|N` | auto | Endgame tables to use: a pile count, `off`, or the largest found, up to 10 piles (larger ones take longer to load than one deal takes to solve). |

One of `--seed`, `--cards`, `--file` and `--position` is needed. "No win found; every line was searched" is not a proof
that the deal can't be won: `acc prove` is.

### `acc prove`

Proves that a deal can't be won, by two separate exhaustive searches, each keeping an exact list of the positions known to
be lost: one with the rules alone, in a fixed order, and one with everything the solver uses, in a random order. Both must
finish without a win. It also finds a win, if there is one, and says when a search reached its limit. It is for deals that
are nearly stuck: a typical deal has far too many positions to search them all. The result is a `loss` (or `win`) solve
record, and the deal's dossier is updated.

```
acc prove --seed 1005616798
acc prove --seed 1005616798 --max-positions 2b --memory 4
```

| Flag | Default | What it does |
|---|---|---|
| `--seed N` | | The deal for this seed. |
| `--cards "..."` | | A deck as text. |
| `--file FILE` | | Read the deck from a file, or `-` for standard input. |
| `--position "..."` | | A game in progress, as for `acc solve`. |
| `--max-positions N` | 10b | Positions each of the two searches may generate before giving up. |
| `--memory GB` | 8 | Memory each search's list of lost positions may take. Past it, positions are forgotten and searched again: slower, never wrong. |
| `--rng-seed N` | 1 | Seed for the second search's move order. |
| `--pattern-piles N` | 16 | The second search stores positions of up to N piles (at most 16) by their pattern of suits and ranks, so that positions that are the same game with suits and ranks renamed are searched once (0: by cards only). |
| `--out FILE` | standard output | Write the result here. |
| `--dossiers DIR` | `output/hard-deals` | Where the deal's dossier is. |
| `--endgame-dir DIR` | `endgame` | Where the endgame tables are. |
| `--endgame auto\|off\|N` | auto | Endgame tables for the second search: a pile count, `off`, or the largest found, up to 11. |

### `acc verify`

Checks solve files by replaying every move under the reference rules ([FORMATS.md](FORMATS.md#verification-acc-verify)):
that each move is legal, that the cards are the deal's, and that a win ends in one pile. A loss record's moves are checked
the same way, but a loss can't be confirmed by replaying it; `acc verify` says how to prove it again. Given a dossier, it
says what that is and where its loss record is. (`acc batch` writes a loss record for each deal it proves lost; `acc prove`
only marks the dossier `loss (proven)`, and writes a record with `--out FILE`.)

```
acc verify win.json
acc verify wins.jsonl
acc verify output/hard-deals/proofs
```

| Flag | Default | What it does |
|---|---|---|
| `PATHS` | | Solve files (`.json`, or `.jsonl` with one record a line), folders of them (the files directly in the folder), or `-` for standard input. |
| `--quiet`, `-q` | off | Only print failures and the summary. |

It exits with 1 if any record fails.

## Advanced

### `acc batch`

The engine behind `acc run`: solves a range of numbered deals, on every thread and on the GPU if there is one, and reports
how many were won and how fast. Every win is replayed by a check separate from the search before it counts; every deal not
won gets a dossier and, while the run goes on, a proof attempt. `acc batch -h` shows the everyday flags, `acc batch --help`
all of them. Unlike `acc run`, it never starts itself again: see [exit codes](#exit-codes).

```
acc batch --count 1m
acc batch --from 5b --count 100k --gpu off
acc batch --count 2t --stop-after 8
acc batch --resume checkpoints/batch-0-2000000000000.json
```

#### Which deals

| Flag | Default | What it does |
|---|---|---|
| `--from S` | 0 | First seed. |
| `--count N` | 1000 | Number of consecutive seeds. |

#### Speed and machine

| Flag | Default | What it does |
|---|---|---|
| `--threads N` | 0 (every thread) | Processor threads. With the GPU, one of them feeds it. |
| `--gpu auto\|on\|off` | auto | Ladder attempts on an NVIDIA GPU in front of the processor's solver: if there is a GPU that works, always (an error without one), or never. Which deals the GPU wins, and their lines, depend on timing; every line is still checked. |
| `--endgame auto\|off\|N` | auto | Endgame tables to use: a pile count, `off`, or the largest found in `--endgame-dir`. |
| `--endgame-dir DIR` | `endgame` | Where the endgame tables are (`acc endgame build`). |

#### Stopping and carrying on

| Flag | Default | What it does |
|---|---|---|
| `--resume CHECKPOINT` | | Carry on with the paused or crashed run in this checkpoint, from the folder it started in, with the settings it started with. Only `--threads`, `--checkpoint-every`, `--log-dir`, `--no-log`, `--games-memory`, `--no-games-check`, `--keep-free`, `--until` and `--stop-after` may be given with it. |
| `--until N` | | Pause once N of the run's deals are done: a way to run in legs. |
| `--stop-after H` | | Pause after H hours of this session (each start counts from itself). |
| `--checkpoint FILE` | `checkpoints/batch-<from>-<count>.json` | Where to keep the checkpoint, with the run's other files beside it. |
| `--no-checkpoint` | off | Keep no checkpoint: the run can't pause, and if the GPU fails the processor solves the rest. |
| `--checkpoint-every S` | 60 | How often to save the checkpoint, in seconds. |
| `--keep-free GB` | 5 | Pause, with exit code 74, when less than this is free on the checkpoint's disk. |

Ctrl+C pauses the run; a second Ctrl+C stops it at once. Creating a file named like the checkpoint but ending `.pause`
pauses it too (one made before the run started is taken to be left from an earlier session, and deleted). Starting a run
whose checkpoint is still there is refused, so that a paused run can't be overwritten by mistake, and so is starting one
that is going in another window (the `.lock` file).

#### Output

| Flag | Default | What it does |
|---|---|---|
| `--out FILE` | | Write one checked solve record per deal here (JSON Lines, compact moves), in seed order: about 475 bytes a deal. |
| `--dossiers DIR` | `output/hard-deals` | Where deals that aren't won get their dossiers, and proven losses their loss records (in `proofs/`). |
| `--no-dossiers` | off | Don't write dossiers. |
| `--gallery N` | 0 | Keep the run's N hardest deals (most positions) as checked solve files in `<dossiers>/gallery/`, replacing the gallery there, and list them in its `INDEX.md`. With the GPU, only deals solved on the processor are ranked. |
| `--log-dir DIR` | `logs` | Where the speed log goes: a CSV row a second, in `batch-<UTC time>Z.csv`. |
| `--no-log` | off | Don't write the speed log. |

#### Proving losses

| Flag | Default | What it does |
|---|---|---|
| `--prove N` | 10b | Positions each of the two exhaustive searches may generate when proving a deal the solver didn't win (0: don't prove). Proofs run one at a time on a thread of their own while the run goes on, and the run waits for the last at the end. Most take under a second; one can take an hour. |
| `--prove-memory GB` | 8 | Memory each proof search's list of lost positions may take. |

#### Repeated-game check

At the end the run checks that no two of its deals are the same game, even with the suits and ranks renamed.

| Flag | Default | What it does |
|---|---|---|
| `--games-memory GB` | 16 | Memory the check may take. A run whose deals fit (8 bytes a deal: up to about 2.1 billion deals at 16 GB) has every deal checked at the end. Past that it checks a sample of them, noted as it goes. |
| `--games-disk GB` | 50, or half of what is free if less | Room the sample may take on disk, at 11 bytes a deal, beside the checkpoint (or in the system's temporary folder without one). Room for 5 bytes for every deal asks for the full check on disk instead, which takes two more passes over the deals at the end. |
| `--no-games-check` | off | Skip the check. |

#### Research

| Flag | Default | What it does |
|---|---|---|
| `--max-nodes N` | 50m | Positions one deal may search, on one thread, before it counts as not won. |
| `--config SETTINGS` | the defaults | Solver settings: a JSON file's name, or the JSON itself, such as `'{"ladder": {"start": 36}}'` ([settings files](#settings-files)). `--max-nodes` overrides `max_nodes`. |
| `--gpu-config SETTINGS` | the defaults | GPU settings: a JSON file's name, or the JSON itself, such as `'{"budget": 4000}'`. |
| `--table N` | 14 | Each thread's table of lost positions, as log2 of 64-byte buckets (14 is 1 MB: small tables that stay in the processor's own cache were fastest). |

Hidden from `--help`, for timing the GPU kernel and testing:

| Flag | What it does |
|---|---|
| `--gpu-kernel FILE` | Compile this kernel source instead of the built-in one, to time a variant without rebuilding. |
| `--gpu-define NAME=VALUE` | A define for the kernel's compile; may be repeated. |
| `--plant-repeats N` | For testing the repeated-game check: every Nth deal is the deal before it. |

### `acc settings`

Prints the solver's settings, or the GPU's, as they are unless told otherwise, as JSON to edit. The file it writes is what
`acc run --settings`, `acc run --gpu-settings`, `--config` and `--gpu-config` take. See [settings files](#settings-files).

```
acc settings
acc settings --out my-settings.json
acc run --settings my-settings.json
```

| Flag | Default | What it does |
|---|---|---|
| `--gpu` | off | The GPU's settings instead of the solver's. |
| `--out FILE` | standard output | Write them to FILE (refused if it exists). |

### `acc endgame build`

Builds the endgame tables: for every pile count from 1 up to `--piles`, one bit for each pattern of suits and ranks, saying
whether it can be won. Runs, `acc solve` and `acc prove` find them in `endgame` by themselves and are faster with them.

```
acc endgame build
acc endgame build --piles 11
```

| Flag | Default | What it does |
|---|---|---|
| `--piles N` | 10 | Largest pile count. 10 is 68 MB and about a minute; 11 is 1.15 GB and about 7 minutes (on the test laptop, every thread). |
| `--dir DIR` | `endgame` | Where to save them. |
| `--threads N` | 0 (every thread) | Threads to use. |

### `acc selftest`

Checks this build in depth: the fast engine against the slow, simple reference engine over random games; every dead end the
solver detects confirmed by a plain search; the search's shortcuts (sleep sets, dead-end tests, endgame tables) against
plain search; and the exhaustive prover against a plain recursive search. It prints a line for each check and ends with
exit code 1 if any disagrees, which would be a bug worth reporting.

```
acc selftest
acc selftest --playouts 20k --positions 20k --residuals 2k --proofs 300
```

| Flag | Default | What it does |
|---|---|---|
| `--playouts N` | 100k | Random games played by both engines side by side. |
| `--positions N` | 100k | Random small positions; any dead end the solver detects in one must be confirmed by the reference search. |
| `--residuals N` | 100k | Random part-played deals on which sleep sets and endgame tables must agree with plain search. |
| `--proofs N` | 3000 | Random positions on which the exhaustive prover must agree with a plain recursive search. |
| `--endgame-dir DIR` | `endgame` | Endgame tables to check (the largest found there; tables to 8 piles are built if there are none). |

With the default counts it takes some minutes on every thread; `acc doctor --full` runs it with 20k, 20k, 2k and 300.

## For developers

These two are hidden from `acc help`'s list. They measure and tune the solver; their results are in
[BENCHMARKS.md](BENCHMARKS.md) and [research/experiments/](../research/experiments/README.md). A third program, `accgpu`,
runs the GPU's part alone, for measuring the kernel: `cargo run --release -p accordion-gpu -- --help` lists its flags.

A build with `cargo install --locked --path crates/cli --features second-generator` makes its deals with a second
generator (ChaCha20 in place of xoshiro256**): a different set of deals for the same seeds, to check that the win rate
doesn't depend on the generator. Its reports say so on a `generator` line. Its results are not those of the main runs, so
keep such a build apart from the usual one.

### `acc bench`

Measures how fast the search is on one thread: random prefixes of a few depths, then a search, over a fixed set of deals.

```
acc bench
acc bench --sweep
acc bench --prove --budget 1m
```

| Flag | Default | What it does |
|---|---|---|
| `--deals N` | 300 | Deals for each prefix depth. |
| `--depths LIST` | 26,30,34 | Random moves played before the search starts (comma-separated); 0 searches the full deal. |
| `--budget N` | 2m | Positions one search may generate before giving up. |
| `--table N` | 16 | Table of lost positions, as log2 of 64-byte buckets (16 is 4 MB). |
| `--sweep` | off | Compare the dead-end tests' settings instead of timing only the default. |
| `--prove` | off | Time the exhaustive prover instead, with `--budget` positions a search and the endgame tables in `./endgame`. |

### `acc experiment`

Compares solver settings on a fixed set of deals, and appends a summary row per setting to `<out>/summary.jsonl`.

```
acc experiment --set validation --config '{"name": "default"}' --dry
acc experiment --set 0:5000 --config a.json --config b.json --repeat 3
```

| Flag | Default | What it does |
|---|---|---|
| `--set SET` | | Deal set: `train` (seeds 200,000-209,999), `validation` (300,000-309,999), `test` (1,000,000-1,099,999), `regression` (0-99,999), `hard` (`<out>/sets/hard.txt`), a range `FROM:COUNT`, or a file of seeds. Needed. |
| `--config SETTINGS` | | A configuration, as a JSON file's name or the JSON itself: `{"name": "...", "solver": {...}, "table": 14, "threads": 16, "gpu": {...}, "note": "..."}` (`solver` and `gpu` as in [settings files](#settings-files)). Repeatable. Without `gpu` the GPU isn't used. |
| `--grid FILE` | | A JSON file holding a list of configurations, run after the `--config` ones. |
| `--repeat N` | 1 | Run every configuration N times, interleaved, to measure the spread of times. |
| `--threads N` | 0 (every thread) | Threads, unless a configuration says otherwise. |
| `--table N` | 14 | Table of lost positions per thread, unless a configuration says otherwise. |
| `--endgame-dir DIR` | `endgame` | Endgame tables, for configurations that set `search.endgame_piles`. |
| `--out DIR` | `output/experiments` | Where `summary.jsonl`, `sets/` and `deals/` go. The project's own results are in `research/experiments/phase5`: give that folder to add to them. |
| `--deals` | off | Also write a row per deal (first repeat only) to `<out>/deals/<set>-<name>.jsonl`. |
| `--dry` | off | Print the results without appending them to `summary.jsonl`. |
| `--warmup S` | 0 | Before measuring, run the first configuration for S seconds, unrecorded, so that a laptop's processor settles at its sustained speed first. |

## Settings files

`acc settings` prints the solver's settings as JSON, and `acc settings --gpu` the GPU's. Save them with `--out`, change what
you like, and give the file to `acc run --settings` (or `--gpu-settings`), or to `acc batch --config` (or `--gpu-config`).
Fields you leave out keep their defaults, and a field that isn't one of these is an error, so a misspelt name doesn't
silently do nothing. The defaults are the fastest found ([BENCHMARKS.md](BENCHMARKS.md)): changing them is for experiments,
and every win is still checked whatever they say.

```
acc settings --out my-settings.json
acc run --settings my-settings.json
```

### The solver's settings

| Field | Default | What it does |
|---|---|---|
| `max_nodes` | 100000000 | Positions to generate in all before giving up. Runs and `acc solve` take it from `--max-nodes` instead (50m a deal in a run, 200m for `acc solve`). |
| `luby_unit` | 1000 | The thorough search's first restart, in positions; later ones follow the Luby sequence (1 1 2 1 1 2 4 ...) times this. |
| `restarts` | `"luby"` | The thorough search's restarts: `"luby"`, `{"geometric": {"percent": 150}}` (each budget 150% of the last), or `"never"`. |
| `search.gates.connectivity` | 19 | The connectivity test for a dead end, at positions with at most this many piles (0: off). |
| `search.gates.left_leaf` | 0 | The left-leaf test, at positions with at most this many piles (0: off; it cuts positions but costs more time than it saves). |
| `search.gates.every_node` | false | Run the connectivity test at every position in its range, not only when a move could have changed the answer (for measuring; the answers are the same). |
| `search.gates.leftmost` | true | The stronger connectivity test that leaves the first pile's card out. |
| `search.order` | `"uniform"` | How the search picks its next move: `"uniform"` (at random), `"leaps_first"`, `"slides_first"`, or `"mobility"` (the one leaving the most moves). |
| `search.sleep_sets` | true | Skip move orders that differ only in the order of moves that don't affect each other. |
| `search.table_min_piles` | 0 | Use the table of lost positions only for positions with more piles than this. |
| `search.probe_first` | false | Look a position up in the table before building it, instead of after. |
| `search.endgame_piles` | 0 | Answer positions of up to this many piles from the endgame tables (0: off). Runs set it from `--endgame` unless the file does. |
| `search.pattern_piles` | 0 | Keep lost positions of up to this many piles (at most 16) by their pattern of suits and ranks, in a table kept from deal to deal (0: off). |
| `search.pattern_table` | 16 | That table's size, as log2 of 64-byte buckets. |
| `ladder` | (below) | The ladder of random-prefix attempts before the thorough search; `null` turns it off. |
| `ladder.start` | 36 | Random moves before each search, on the first rung. |
| `ladder.end` | 24 | Random moves on the last rung. |
| `ladder.step` | 2 | Random moves fewer on each next rung (0: only one rung). |
| `ladder.tries` | 256 | Attempts per rung. |
| `ladder.budget` | 4000 | Positions each search may generate on the first rung. |
| `ladder.growth` | 200 | Each next rung's budget, in percent of the last (200 doubles it). |
| `ladder.prefix` | `"uniform"` | How the random moves are chosen, as for `search.order`. |
| `ladder.leap_weight` | 8 | With `"uniform"`, how much likelier a move three to the left is than a move one to the left (1: all alike). |
| `ladder.unstuck` | false | Take back a random move that leaves no move with more than one pile left, and choose another. |
| `seed` | 0 | Seeds the order in which moves are tried. Runs set it themselves: each deal's own seed in `acc batch`, `--rng-seed` in `acc solve`. |

### The GPU's settings

| Field | Default | What it does |
|---|---|---|
| `rungs` | `[[40, 24], [38, 376]]` | One or two rungs, each `[random moves before the search, tries per deal]`. Both run in one launch, the second on the deals the first didn't win. The random moves must be 36 to 48, so that at most 16 piles are left to search. |
| `budget` | 700 | Positions each search may generate. |
| `batch` | 262144 | Deals per launch. Keep a launch well under the 2 seconds after which Windows resets a graphics card that seems stuck. |
| `block` | 64 | Threads per block (at most 256). |
| `blocks_per_sm` | 8 | Blocks per multiprocessor. |
| `conn_gate` | 19 | The connectivity test at positions with at most this many piles. |
| `leftmost` | true | The stronger connectivity test that leaves the first pile's card out. |
| `leap_weight` | 8 | How much likelier a random move three to the left is than one to the left: a power of two (1: all alike). |
| `seed` | 24301 | Seeds the GPU's random moves. |
| `profile` | false | Compile the kernel with its profile counters: slower, for measuring only. |
| `prefix_warps` | 1 | Warps of a block that play the random moves; `block` must hold more. |
| `share_group` | 2 | A deal's tries go in groups of this many, which play their first `share_moves` random moves once (1: no sharing; at most 64). |
| `share_moves` | 20 | From 20 up to the shorter rung's random moves, when `share_group` is more than 1. |
| `rank_gate` | `[8, 10]` | For each rung, the most ranks a position may have for the search to be tried on it. |

The GPU's design and the measurements behind these are in [ARCHITECTURE.md](ARCHITECTURE.md#the-gpu) and
[BENCHMARKS.md](BENCHMARKS.md).

## Exit codes

What acc gives back when it ends, which scripts can test (`$LASTEXITCODE` in PowerShell, `$?` in bash).

| Code | Meaning | What to do |
|---|---|---|
| 0 | Done. For `acc batch`, `acc run` and `acc resume`: the run finished **or paused**. It paused if its checkpoint is still there (or `.status.json` says `"state": "paused"`). | Paused: `acc resume`. |
| 1 | An error, said on the line starting `acc:`. Or a run finished with a winning line that failed its check or a failed proof (the report says), `acc verify` found a bad record, `acc doctor` a FAIL, or `acc selftest` a disagreement. | Read the message ([TROUBLESHOOTING.md](TROUBLESHOOTING.md)). The last four would be bugs: please report them. |
| 2 | The command line couldn't be read: a misspelt flag, a missing value. It says which. | `acc help <command>`. |
| 74 | The run paused because the disk is nearly full (under `--keep-free`), saving the checkpoint failed five times in a row, or the log of the repeated-game check's sample couldn't be written. Nothing is lost. (A dossier or `--out` that can't be written is an error, exit code 1.) | Make room, then `acc resume`. |
| 75 | The run paused because the GPU failed, or no deal finished for 15 minutes with the GPU. Nothing is lost. | `acc resume`: a new process usually gets the GPU back. `acc run` and `acc resume` do this by themselves. |
| 130 | The run stopped at once, on a second Ctrl+C. Its last checkpoint, at most a minute old, stands. | `acc resume`. |

Closing the window, logging off or shutting down stops a run without a pause: the last checkpoint stands, and `acc resume`
carries on from it. On Linux and macOS, `kill` (SIGTERM) is meant to pause a run as Ctrl+C does (not tested there yet).

## Quoting in each shell

A setting given as JSON on the command line has to reach acc with its double quotes. Shells differ in how they pass them,
so the simplest way is a file: `acc settings --out my-settings.json`, edit it, and pass its name. To type the JSON itself:

| Shell | Type |
|---|---|
| Windows PowerShell 5.1 (the one Windows has) | `acc batch --config '{\"ladder\": {\"start\": 34}}'` (it drops plain inner double quotes) |
| PowerShell 7.3 or later | `acc batch --config '{"ladder": {"start": 34}}'` (not tried here) |
| Command Prompt (cmd) | `acc batch --config "{\"ladder\": {\"start\": 34}}"` (single quotes are not quotes there) |
| bash, zsh (Linux, macOS, Git Bash) | `acc batch --config '{"ladder": {"start": 34}}'` |

If the quotes didn't arrive, acc says `your shell removed the quotes; put the JSON in a file and pass its name`.

Other differences between shells:

- Windows PowerShell 5.1 has no `&&`: type the commands one at a time.
- A program in the current folder needs `.\` before it in PowerShell (`.\target\release\acc.exe`) and `./` in bash.
- Setting an environment variable for this window: `$env:ACC_NVRTC_DIR = "C:\nvrtc"` in PowerShell, `set ACC_NVRTC_DIR=C:\nvrtc` in
  cmd, `export ACC_NVRTC_DIR=/opt/nvrtc` in bash.
- Cards can be typed with letters (`AC`) everywhere; `♣` and the other suit signs work too, where the terminal can type them.

## Environment variables

| Variable | What it does |
|---|---|
| `ACC_NVRTC_DIR` | A folder holding NVRTC (the folder with the library, or one the NVRTC download was unpacked into), for when it isn't on the system's search path or in `.tools` ([GPU_SETUP.md](GPU_SETUP.md)). |
| `ACC_FAKE_BATTERY` | For tests: a file holding `<percent> <1 or 0 for the charger>`, `none` or `unreadable`, read by `acc run` and `acc resume` in place of the battery (every second instead of every 30). |
| `ACC_COST_SAMPLE_MOST` | For tests: how many deals' positions and times a run keeps for its percentiles, instead of 125 million. |
| `ACC_NVRTC_RESTARTED` | Set by acc itself on Linux when it starts again with NVRTC on the loader's path, so that it does so only once. |
