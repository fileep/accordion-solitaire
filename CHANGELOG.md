# Changelog

What changes between releases, newest first. There has been no release yet: the program's history until now is in
[docs/PLAN.md](docs/PLAN.md) and [docs/BENCHMARKS.md](docs/BENCHMARKS.md).

## Unreleased

Making the program usable by other people. None of this changes how a deal is solved, proved or verified.

### Added

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

### Changed

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

- The card example in `acc solve --help`, `acc deal --help` and the visualizer (`A♣`, not `♣A`, which couldn't be read).
- `acc selftest --help`'s example now has the counts `acc doctor --full` uses.
- `--keep-free` and the disk warnings work off Windows too, through `df` (not tested there yet).
- A pause file made while a run is starting up is kept for it, where it used to be deleted as left from before; only one
  made before the run's process began is.
