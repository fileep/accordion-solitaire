# Accordion Solitaire Solver

Accordion is a patience game: the 52 cards are dealt face up in a row, and a pile may be moved onto the pile next to it,
or onto the one three places to its left, when their top cards share a suit or a rank. It is won when one pile is left.
`acc`, the program here, plays numbered deals, finds a winning line for every deal it can, proves the others impossible,
and counts how many of each there were.

**The result so far** ([docs/RESULTS.md](docs/RESULTS.md), as of 2026-11-06): of 22,000,000,000 random deals, 21,999,999,977
were won, every win checked by replaying it under the rules, and the other 23 were proven impossible. About 1 deal in 960
million can't be won (95% interval: 1 in 637 million to 1 in 1.44 billion). The last 20 billion took 5 h 57 min on a laptop
with an NVIDIA T550 graphics card, at 934,074 deals a second.

## What you need

| | |
|---|---|
| A 64-bit computer | Windows 10 or 11 is tested. Linux and macOS are expected to work, but are not tested yet (see below). |
| Rust, the language acc is written in | Free, from [rustup.rs](https://rustup.rs). On Windows also Microsoft's C++ build tools. [docs/GETTING_STARTED.md](docs/GETTING_STARTED.md) has every step. |
| An NVIDIA graphics card | Optional. It makes long runs about a hundred times faster or more: [docs/GPU_SETUP.md](docs/GPU_SETUP.md). |
| Node.js | Only for the visualizer, which plays and replays deals in a browser: [docs/VISUALIZER.md](docs/VISUALIZER.md). |

## Quick start

In a terminal, in the project's folder (the same commands in Windows PowerShell, on Linux and on macOS):

```
cargo install --path crates/cli
acc doctor
acc run --quick
acc show --seed 42 --solve
```

The first builds `acc` and puts it where the terminal finds it (a few minutes). `acc doctor` checks the computer,
`acc run --quick` plays 10,000 deals, and `acc show` shows one deal won move by move. If anything is new to you, start with
[docs/GETTING_STARTED.md](docs/GETTING_STARTED.md): it explains each step and what you should see.

## Want to... type this

| Want to | Type |
|---|---|
| Check that this computer is ready | `acc doctor` |
| Play a million deals and count the wins | `acc run` |
| Play for 8 hours, then carry on another night | `acc run --overnight` |
| Leave the computer usable while it plays | `acc run --gentle` |
| See how the runs started here are going | `acc status` |
| Pause a run from another window (or press Ctrl+C in its own) | `acc pause` |
| Carry on with a paused run | `acc resume` |
| See what a run found, finished or not | `acc results` |
| See one deal, and a way to win it | `acc show --seed 42 --solve` |
| Save a deal's winning line, and check it | `acc solve --seed 42 --out win.json`, then `acc verify win.json` |
| Prove that a deal can't be won | `acc prove --seed 202346720` |
| See every command | `acc help` |

## Where next

- [docs/GETTING_STARTED.md](docs/GETTING_STARTED.md): from a fresh computer to a finished run.
- [docs/LONG_RUNS.md](docs/LONG_RUNS.md): runs of hours or days, how big a run can be, laptops and the battery.
- [docs/READING_RESULTS.md](docs/READING_RESULTS.md): what the progress line and the report say, and how to check them yourself.
- [docs/COMMANDS.md](docs/COMMANDS.md): every command and setting. [docs/TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md): when something goes wrong.
- [docs/README.md](docs/README.md): all the documentation, including how it was built and measured.

## Platforms

| System | Status |
|---|---|
| Windows 10 and 11, 64-bit (x86-64) | Built and tested, with and without the GPU, on a Windows 11 laptop (Intel i7-1260P, NVIDIA T550). |
| Linux, 64-bit | Expected to work, not tested yet; CI will show. Not tried: the GPU there (`tools/get-nvrtc.sh`), the shell scripts in `tools/`. acc doesn't keep Linux awake during a run (start it under `systemd-inhibit`). |
| macOS | Expected to work on the CPU, not tested yet; CI will show. There is no NVIDIA GPU support on macOS, and acc doesn't read the battery or keep the computer awake there (use `caffeinate -i`). |

## Project layout

| Path | Contents |
|---|---|
| `crates/core` | `accordion-core`: cards, rules, the engines, the file formats |
| `crates/solver` | `accordion-solver`: the search, the endgame tables, proofs of loss |
| `crates/cli` | `acc`, the command-line program, with the batch runner and the front end (`acc run` and the rest) |
| `crates/gpu` | `accordion-gpu`: the NVIDIA GPU's part of a run (CUDA, compiled with NVRTC); `accgpu` measures the GPU alone |
| `crates/wasm` | the core and the solver compiled to WebAssembly for the visualizer |
| `viz/` | the visualizer, a web page (Vite and TypeScript) |
| `tools/` | scripts for users: fetching NVRTC for the GPU, seeing a long run through |
| `docs/` | the documentation ([docs/README.md](docs/README.md)) |
| `hard-deals/` | the published record: the dossier and proof of every deal the project's runs didn't win, and a gallery of hard wins |
| `testdata/` | golden deals and solutions shared by the Rust and TypeScript tests, made by an independent Python version |
| `research/` | how it was built and measured: probes, experiments, the big runs' logs and reports, Lean proofs, private tools |

## Licence

The licence hasn't been chosen yet. NVRTC, which the GPU needs, is NVIDIA's software under NVIDIA's licence: it isn't part
of this project, and each user fetches it ([docs/GPU_SETUP.md](docs/GPU_SETUP.md)).
