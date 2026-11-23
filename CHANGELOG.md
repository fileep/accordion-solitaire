# Changelog

What changes between releases, newest first. The program's history before the first release is in
[docs/PLAN.md](docs/PLAN.md) and [docs/BENCHMARKS.md](docs/BENCHMARKS.md).

## Unreleased

### Added

- `game-tree/game-tree.html`: a 3D explorer of any deal's game tree, one web page with nothing to install, which can be
  pasted into a blog post. Trees that fit the chosen size (up to 40 million positions, or a number typed in) are drawn
  whole, so all 721 lost deals can be seen complete; bigger trees are samples made by short solver-like searches. Click a position to
  see it and its best line, and redraw the tree from it. `node game-tree/test-engine.mjs` checks its engine against
  the proofs. Pictures of the examples are in `game-tree/screenshots/`.

## 0.1.2, 2026-11-21

For archiving on Zenodo. The links of the paper "The win rate of open Accordion solitaire" point at this release. The
program is the same as in 0.1.0 and still reports that version.

### Added

- `.zenodo.json`, which describes the project to Zenodo. Zenodo reads one licence from `CITATION.cff`, where this
  project names two, and would not archive a release without it. The Zenodo record names MIT; the project stays under
  MIT or Apache-2.0, at your option.

## 0.1.1, 2026-11-21

Corrections to the documents. The program is the same as in 0.1.0 and still reports that version.

### Fixed

- The GPU plays at most 384 tries a deal, not 400: a deal's first 16 tries go alone, and only the later ones go in pairs
  (`docs/RESULTS.md`, `docs/ARCHITECTURE.md`, and the paper's Figure 2 in `research/paper/figures.py`).
- `docs/RESULTS.md` counts five GPU runs, and `docs/LONG_RUNS.md` says that with the GPU about 1 deal in 18,000 is
  solved on the processor.
- `hard-deals-chacha/INDEX.md` says that it is the record of the second-generator run, and that its seeds give these
  deals only in a build with the `second-generator` feature.

## 0.1.0, 2026-11-16

The first release: the solver, the run records and the proofs behind the paper "The win rate of open Accordion
solitaire".

Making the program usable by other people, a second deal generator to test the results with, and the record of the
trillion-deal run. Apart from the second generator, which is only there when built in, none of this changes how a deal is
dealt, solved, proved or verified.

### Added

- A licence: MIT or Apache-2.0, at your option (`LICENSE-MIT`, `LICENSE-APACHE`).
- `acc run`, `acc resume`, `acc status`, `acc pause` and `acc results`: start a run of deals with a few plain options
  (`--deals 2m`, `--hours 8`, `--overnight`, `--quick`, `--gentle`), carry on with it, see where it stands, pause it
  from another window and read what it found. A run started this way starts again by itself after a GPU failure or a
  crash, and on a laptop pauses while the battery is low and carries on once it has charged.
- `acc doctor`: checks the build, the CPU, memory, disk space, the folder, the endgame tables, the NVIDIA driver and
  NVRTC, runs here and the battery, and says what to do about each problem.
- `acc show` (a deal and its solution in words) and `acc settings` (the solver's settings as a file to edit).
- `acc` alone prints a start page, and `acc help` a grouped list of the commands. Each command's help has examples.
- Counts can be written as `5000`, `2m`, `1.5b`, `100t` or `1e12`.
- `--config`, `--gpu-config` and `experiment --config` take the name of a JSON file as well as JSON itself.
- A run's lock: two windows can't run the same checkpoint at once. A run that an older acc started, which takes no
  lock, is judged by when its files were written, and `acc resume` and `acc run` touch it only with `--force`.
- `.status.json` also says what has been won, where the dossiers go, how often it saves, and the checkpoint's name.
- `verify` takes several files and folders.
- `tools/get-nvrtc.ps1` and `tools/get-nvrtc.sh` (not tested on Linux yet) download NVIDIA's NVRTC, check it and
  unpack it where acc finds it. `ACC_NVRTC_DIR` names another folder holding it. On Linux, acc starts itself again with
  that folder on `LD_LIBRARY_PATH` (not tested yet).
- `tools/long-run.sh`, the shell counterpart of `tools/long-run.ps1` (not tested yet), and a `-Gpu` setting for
  `tools/long-run.ps1`.
- CI on Windows, Linux and macOS; `CONTRIBUTING.md`, `.gitattributes`, `.editorconfig` and this file.
- Documentation for people new to the command line, indexed in `docs/README.md`: `GETTING_STARTED.md` (from a fresh
  computer to a finished run), `COMMANDS.md` (every command, flag and setting, exit codes, quoting in each shell),
  `LONG_RUNS.md` (time, disk and memory up to 100 trillion deals, pausing, laptops), `READING_RESULTS.md`,
  `GPU_SETUP.md`, `TROUBLESHOOTING.md`, `VISUALIZER.md` and `ARCHITECTURE.md`. `FORMATS.md` describes the files a run
  writes. A test checks that `COMMANDS.md` names every command and flag, that the commands the documents show are real,
  and that their links lead somewhere.
- The `second-generator` build feature (`--features second-generator`): deals from ChaCha20 with mask-and-redraw instead
  of xoshiro256** with Lemire's method, sharing no code with the main generator, to test whether the win rate rests on the
  generator. The same seed gives another deal, and the report has a `generator` line. See
  [research/runs/README.md](research/runs/README.md).
- The record of the trillion-deal run (seeds 0 to 999,999,999,999: 721 proven lost, none undecided): its report, log of
  events and speed logs in `research/runs/`, the 721 loss records in `hard-deals/`, every loss re-derived by the
  independent Python rules (`research/runs/losses.csv`), and every loss proved in Lean's kernel and checked again in
  nanoda (`research/lean/`).

### Changed

- CI uses `actions/checkout` and `actions/setup-node` version 7, which run on Node 24 (version 4 ran on the deprecated
  Node 20).
- Runs of up to 100 trillion deals fit in memory: the positions and time of each deal solved on the CPU are kept for
  the first 125 million such deals only, as a fair sample for the report's percentiles; means, totals and maxima still
  cover every deal. A run that stays under that number reports exactly what it did before. Older checkpoints carry on.
- Dossiers go to `output/hard-deals` and experiments to `output/experiments` unless told otherwise, so that a run
  doesn't write into the project's published record in `hard-deals/`. A checkpoint keeps the folder it started with.
- The progress line and the report stay readable at trillions of deals.
- Plainer error messages, which say what to do. `--count 0`, a range past the last seed and impossible memory settings
  are refused at once.
- Builds use the building computer's own CPU instructions on every system (they did on Windows only), and the
  WebAssembly target is no longer installed for everyone: the visualizer's steps add it.
- `tools/long-run.ps1` uses the GPU when it can (`-Gpu auto`), where it used to insist on it.
- `PLAN.md` moved to `docs/PLAN.md`. The author's private tuning scripts moved from `tools/` to `research/tools/`,
  which says they don't run on their own.
- `README.md` is a short front page; what it used to explain is in the guides above. `docs/RESULTS.md` describes the
  GPU's current settings, and no longer says the GPU adds 1.4x as if that were still so.

### Fixed

- Builds and passes its tests on Linux and macOS (CI) as well as Windows. Clippy 1.100 asked for an iterator in the
  endgame tables' save; a constant only the x86-64 paths use was dead code on Apple silicon; and three tests assumed
  the author's computer: the memory flags `acc run` adds under 32 GB, macOS's battery line in `acc doctor`, and a
  pause test's timing on a slow machine.
- The card example in `acc solve --help`, `acc deal --help` and the visualizer (`A♣`, not `♣A`, which couldn't be read).
- `acc selftest --help`'s example now has the counts `acc doctor --full` uses.
- `--keep-free` and the disk warnings work off Windows too, through `df` (not tested there yet).
- A pause file made while a run is starting up is kept for it, where it used to be deleted as left from before; only one
  made before the run's process began is.
- A run that ends at the last seed there is (`--from 18446744073709551615 --count 1`) no longer overflows working out its
  last seed, which stopped a debug build.
