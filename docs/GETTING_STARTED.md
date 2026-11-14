# Getting started

This page takes you from a computer that has never seen this project to a finished run of deals. You don't need to know
how to program. Each step gives the exact thing to type and what you should see.

The examples were run on the test machine: a Windows 11 laptop with an Intel i7-1260P (16 threads) and an NVIDIA T550.
Your numbers will differ. Windows is tested. On Linux and macOS acc builds and passes its tests on every push (CI), but
this guide's steps haven't been followed there, so the steps for them are the usual ones for any Rust program.

## Words used here

- **Terminal:** a window where you type commands and the computer answers in text. On Windows it is PowerShell.
- **Command:** one line you type, then press Enter. In this guide each command is in a grey box; type it (or copy and
  paste it) exactly, then press Enter.
- **Folder:** what Windows Explorer and macOS Finder show; also called a directory. The terminal is always "in" one
  folder, shown before the place you type.
- **Deal:** one shuffle of the 52 cards, laid out in a row to play.
- **Seed:** the number of a deal. Deal 42 is always the same 52 cards, on every computer, so anyone can check a result.
- **Run:** a range of deals played one after another, such as deals 0 to 999,999, and counted.
- **Checkpoint:** the file in which a run saves where it is, every minute, so that it can pause and carry on later.
- **Thread:** one strand of work the processor can do at a time. A processor has several (the test laptop's has 16), and
  acc solves one deal on each; `--threads 2` tells it to use two.
- **JSON:** a plain-text way of writing settings, such as `{"max_nodes": 1000}`, which acc reads from a file.
- **Exit code:** a number a program leaves behind when it ends: 0 if all went well. In PowerShell, `$LASTEXITCODE` shows
  the last one; elsewhere `echo $?`.

## 1. Open a terminal

- **Windows:** press the Windows key, type `powershell`, and click **Windows PowerShell**. A blue or black window opens
  with a line like `PS C:\Users\you>`: that is the folder it is in.
- **Linux:** open the Terminal application (on Ubuntu, Ctrl+Alt+T).
- **macOS:** open Terminal, in Applications, Utilities.

Keep this window open for the next steps.

## 2. Install Rust

Rust is the programming language acc is written in. Its installer, rustup, is free and also installs `cargo`, the tool
that builds Rust programs.

**Windows:**

1. Go to [rustup.rs](https://rustup.rs) and download `rustup-init.exe` (the 64-bit one).
2. Run it. Rust on Windows needs Microsoft's C++ build tools. If the installer offers to install them for you (through the
   Visual Studio installer), accept. Otherwise install the
   [Build Tools for Visual Studio](https://visualstudio.microsoft.com/visual-cpp-build-tools/) yourself, and in their
   installer tick **Desktop development with C++**.
3. When rustup asks how to install, press Enter for the default.
4. Close PowerShell and open a new one, so that it finds the new programs.

**Linux** (Debian or Ubuntu; other systems have their own names for a C compiler and curl):

```bash
sudo apt install build-essential curl
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Press Enter for the default installation, then close the terminal and open a new one. Don't use a `cargo` or `rustc`
from your system's package manager instead: they are usually older than this project needs.

**macOS:**

```bash
xcode-select --install
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Then close the terminal and open a new one.

**Check it**, on any system:

```
cargo --version
```

You should see something like `cargo 1.99.1 (5f94df478 2026-08-27)` (the test machine's; a newer version is fine). acc
uses parts of Rust added in 2025, so it needs 1.89 at the very least; it has only been built with 1.99. If yours is older,
or Rust was installed a while ago, bring it up to date first:

```
rustup update
```

If Windows says `cargo : The term 'cargo' is not recognized`, see
[TROUBLESHOOTING.md](TROUBLESHOOTING.md#cargo-or-acc-is-not-recognized).

## 3. Get the code

From the project's page, https://github.com/fileep/accordion-solitaire, download it as a ZIP file (on GitHub: the green **Code** button, then **Download ZIP**) and unpack
it, for example into your Documents folder. Or, if you have git, copy the address from the same button and type
`git clone` followed by a space and the address.

Then go into the project's folder: the one that holds `README.md` and `Cargo.toml`. `cd` means "change directory". On
Windows, if you unpacked it into Documents as `accordion-solitaire`:

```powershell
cd $HOME\Documents\accordion-solitaire
```

On Linux or macOS:

```bash
cd ~/Documents/accordion-solitaire
```

Use your own folder's name if it is different (a ZIP from GitHub adds `-main` to it). To check that you are in the right
place, type `dir` on Windows or `ls` elsewhere: you should see `Cargo.toml`, `README.md`, `crates` and `docs` among the
names. If all you see is one folder with nearly the same name (Windows' **Extract All** often makes a folder inside a
folder), go into that one too, with `cd` and its name, and look again.

On many Windows computers the Documents folder that Explorer shows is in OneDrive (`$HOME\OneDrive\Documents`). For long
runs a folder that isn't synced is better, such as `C:\acc` (make it with `mkdir C:\acc`): a run's files grow to many GB,
and a program that syncs them can hold them open while the run saves.

## 4. Build acc

```
cargo install --locked --path crates/cli
```

This builds acc from its source and puts the program where the terminal finds it (in `.cargo\bin` in your home
folder). It needs the internet the first time, to fetch the parts that acc is built from; `--locked` takes the exact
versions of them that acc was tested with (listed in `Cargo.lock`). It prints many `Compiling ...` lines and ends with a
line like:

```
   Installed package `accordion-cli v0.1.0 (C:\Users\you\Documents\accordion-solitaire\crates\cli)` (executable `acc.exe`)
```

It takes a few minutes, more on a slow computer (the time hasn't been measured on a quiet machine). A warning that says
`default toolchain implicitly overridden ... by rustup toolchain file` is harmless: the project asks for the stable Rust,
which is what you have.

Check it:

```
acc --version
```

You should see `acc 0.1.0`.

What else is worth knowing about the build:

- acc is built for the computer that builds it, using every instruction its processor has, which makes it faster. A
  copy of `acc.exe` may not run on a different computer: build it there. ([`.cargo/config.toml`](../.cargo/config.toml) says
  how to build one copy for many computers.)
- After you change or update the code, the same command with `--force` builds and installs it again:
  `cargo install --locked --path crates/cli --force`. Not while a run is going: on Windows the running `acc.exe` can't
  be replaced, and it ends with `Access is denied`. Pause the run first (`acc pause`).
- Without installing: `cargo build --release -p accordion-cli` builds it into the project's folder, as
  `target\release\acc.exe` on Windows and `target/release/acc` elsewhere. Then type `.\target\release\acc.exe` (Windows) or
  `./target/release/acc` instead of `acc` in every command in these documents. Older notes in this project write
  `cargo run --release -p accordion-cli -- deal --seed 42`: that means "build acc if it has changed, then run
  `acc deal --seed 42`".

## 5. Choose the folder to run in

acc keeps a run's files in the folder it is started from: `checkpoints` (where each run is saved, with its report),
`logs` (its speed), `output` (notes on deals it didn't win), and `endgame` (optional tables, below). A run can only carry on
from the folder it started in, so **always start acc from the same folder**. The project's folder is a good choice: those
folders are created there as needed, and git leaves them alone. The rest of these documents assume you are there.

## 6. Check the computer

```
acc doctor
```

It checks what acc needs, in a few seconds, and says what to do about anything that isn't right. On the test laptop it
printed (shortened):

```
PASS  build     acc 0.1.0, release build
PASS  CPU       16 threads; the build uses its AVX2 and BMI2
PASS  memory    48 GB
PASS  folder    C:\Users\you\Documents\accordion-solitaire can be written to
PASS  disk      283 GB free
WARN  tables    no endgame tables in ./endgame
                Optional: acc endgame build --piles 11 builds them in about 7 minutes (1.2 GB on disk), and they
                make solving about 1.2-1.3x faster.
WARN  GPU       NVIDIA driver (CUDA 13.2) found, but not NVRTC, the free compiler acc needs to use the card
                To add it (NVIDIA's own software, under NVIDIA's licence; a 76 MB download), in the project's folder run:
                   powershell -ExecutionPolicy Bypass -File tools\get-nvrtc.ps1
                ...
PASS  runs      none going or paused here
PASS  battery   35%, on the charger
PASS  solving   100 of 100 deals won on 2 threads, every win checked by the rules, in 1.3 s

acc works here; the WARN lines say how it could work better.
```

**PASS** means fine, **WARN** that acc works but could work better, and **FAIL** that something must be put right first;
each WARN and FAIL says how. Without an NVIDIA card the GPU line is a PASS: acc then uses the processor alone. To use the
card, see [GPU_SETUP.md](GPU_SETUP.md); you can do that later.

## 7. Look at one deal

```
acc show --seed 42
```

```
Deal 42. Rules: all 52 cards dealt face up in a row; a pile moves onto the pile next to it, or the one three to its left, when their top cards match in suit or rank; won when one pile is left.

   1-13  2S 2C KS TH 6H 4C 6D 9D 4S QH 8C 8D 8S
  14-26  7C AC 4D 6S 2D 5S 3S AH TC 5H 3D QS JC
  27-39  JS 9C 5D AD 6C 3C JD QD 4H TD AS 2H KC
  40-52  7H QC 3H KD TS KH 8H JH 9S 7S 9H 7D 5C

acc show --seed 42 --solve finds a way to win it.
```

Each card is its rank then its suit: `TH` is the ten of hearts, `AC` the ace of clubs, `KS` the king of spades (`C D H S`
for clubs, diamonds, hearts, spades; `T J Q K A` for ten, jack, queen, king, ace). The deal is in rows of 13 to make it
easier to read; in the game it is one row of 52. Now let acc win it:

```
acc show --seed 42 --solve
```

It searches for a winning line, checks every move of it against the rules, then shows the deal again and the moves. On
the test laptop, with `--threads 2` added (yours says how many threads it used, and its figures will differ):

```
52 piles, 2 threads, up to 200000000 positions
won (14694 positions, 95 attempts, 0.01 s)
verified by the reference engine: 51 moves, 1 pile left
Deal 42. Rules: ...
   ...
A winning line, 51 moves, every one checked by the rules:
   1. 9H onto JH, 51 piles left
   2. 3S onto 6S, 50 piles left
  ...
  50. 7C onto 7D, 2 piles left
  51. 7C onto KC, 1 pile left
```

`9H onto JH` means: the pile with the nine of hearts on top moves onto the pile with the jack of hearts on top (they share
a suit, and it is three places to the left). A position is one arrangement of the piles; the solver counts how many it
looked at. The line you get may differ from someone else's for the same deal: the search tries moves in a random order,
and any winning line will do. A note at the start that there are `no endgame tables` is only advice (step 9).

A winning line can be saved in a file and checked again later, by anyone:

```
acc solve --seed 42 --out win.json
acc verify win.json
```

```
line 1  seed 42  ok: win in 51 moves
1 record, 1 ok, 0 failed
```

`acc verify` replays every move under the rules, with code kept separate from the search. `acc help` lists every
command, and `acc help solve` (or any other command) says more about one.

## 8. A first run

```
acc run --quick
```

This plays deals 0 to 9,999 and counts how many it wins. On the test laptop, with `--threads 2 --gpu off` added (it
was busy with other work), it printed:

```
same as: acc batch --from 0 --count 10000 --threads 2 --gpu off
note: no endgame tables in endgame; `acc endgame build --piles 11` builds them (about 7 minutes, 1.2 GB) and makes solving about 1.2-1.3x faster
seeds 0..9999 (10000 deals), up to 50000000 positions per deal, 2 threads
speed log: logs\batch-20261110-024027Z.csv
checkpoint: checkpoints/batch-0-10000.json; to pause, press Ctrl+C or create checkpoints/batch-0-10000.pause
  ████████████████ 100.0%  10,000 done · 0 left · 6,402 deals/s (avg 6,402) · finished in 2s · 0 lost

deals            10000 (seeds 0..9999)
won              10000 (every line replayed: by the reference rules, or a GPU line by the quick check and 1 in 256 by the reference engine too); by stage: ladder-0 9739, ladder-1 243, ladder-2 18
not won          0 by the solver (0 exhausted, 0 hit the limit of 50000000 positions)
proofs           0 proven lost, 0 won, 0 undecided (two exhaustive runs of up to 10000000000 positions each)
failed to verify 0
repeat odds      9.3e-50 that any two of these deals are the same game (suits and ranks renamed), ...
distinct games   all 10000 deals differ, even with suits and ranks renamed
win rate         100.0000000000%   95% CI [99.9616001623%, 100.0000000000%] (Wilson; ...)
positions/deal   mean 5597  median 2965  p90 12832  p99 37140  p99.9 114901  max 248119
ms/deal (1 core) mean 0.31  median 0.16  p90 0.70  p99 2.18  p99.9 6.47  max 21.5
throughput       6402 deals/s, 36M CPU positions/s on 2 threads (1.6 s wall)

In short: 10,000 of 10,000 deals won; none proven impossible.
A run as short as this one keeps no report: the one above is all there is.
Next: acc run --from 10k --deals 10k plays the next 10,000 deals.
```

Typed as above, without `--threads 2 --gpu off`, the first line is `same as: acc batch --from 0 --count 10000`: acc then
uses every thread, and the GPU if it can.

The first line is the `acc batch` command that `acc run` stands for: `acc batch` is the engine underneath, with every
setting ([COMMANDS.md](COMMANDS.md#acc-batch)). The line with the bar is the progress line, which updates once a second
while the run goes. The lines from `deals` to `throughput` are the run's report, and the `In short` line says it in
words. [READING_RESULTS.md](READING_RESULTS.md) explains every line.

On your computer, with every thread (and the GPU, if acc can use it), it takes a few seconds. A run as short as this
keeps nothing in `checkpoints` (only its speed log, in `logs`): what it printed is its report, and `acc results` has
nothing to show for it. A run of more than a minute keeps its report in `checkpoints`, where `acc results` finds it
again.

On a laptop whose battery is under 30%, a run of more than 10,000 deals waits for the charger and 60% before it starts,
and says so; [LONG_RUNS.md](LONG_RUNS.md#laptops) explains why, and `--no-battery-guard` starts it at once.

## 9. Optional: the endgame tables

The endgame tables hold, for every way the last few piles can look, whether they can still be won. With them the
solver needs to search less, and runs on the processor are about 1.2 to 1.3 times faster. They are built once:

```
acc endgame build --piles 11
```

On the test laptop this took about 7 minutes on every thread and 1.15 GB of disk in the `endgame` folder (measured with an
earlier build; not timed again). Runs load them into memory. `acc endgame build` without `--piles` builds tables to 10 piles
instead: 68 MB, about a minute. acc finds the tables by itself in the `endgame` folder of the folder it runs in.

## 10. A real run

```
acc run
```

This plays a million deals (0 to 999,999). On the processor alone that takes about a minute on a computer like the test
laptop; with its GPU, a few seconds (both estimates: see [LONG_RUNS.md](LONG_RUNS.md#how-long-and-how-much)). Ctrl+C pauses a
run at any time, and `acc resume` carries on with it. When it has finished it says what to type to play the next million.

## Next

- Bigger runs, overnight runs, and laptops: [LONG_RUNS.md](LONG_RUNS.md).
- What each word and number in the output means: [READING_RESULTS.md](READING_RESULTS.md).
- Using an NVIDIA card: [GPU_SETUP.md](GPU_SETUP.md).
- Every command: `acc help`, `acc help run` and so on, or [COMMANDS.md](COMMANDS.md).
- Something went wrong: [TROUBLESHOOTING.md](TROUBLESHOOTING.md).
