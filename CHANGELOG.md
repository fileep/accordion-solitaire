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
- A run's lock: two windows can't run the same checkpoint at once.
- `.status.json` also says what has been won, where the dossiers go, how often it saves, and the checkpoint's name.
- `verify` takes several files and folders.
- `tools/get-nvrtc.ps1` and `tools/get-nvrtc.sh` (not tested on Linux yet) download NVIDIA's NVRTC, check it and
  unpack it where acc finds it. `ACC_NVRTC_DIR` names another folder holding it. On Linux, acc starts itself again with
  that folder on `LD_LIBRARY_PATH` (not tested yet).
- `tools/long-run.sh`, the shell counterpart of `tools/long-run.ps1` (not tested yet), and a `-Gpu` setting for
  `tools/long-run.ps1`.
- CI on Windows, Linux and macOS; `CONTRIBUTING.md`, `.gitattributes`, `.editorconfig` and this file.

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

### Fixed

- The card example in `acc solve --help` and `acc deal --help` (`A♣`, not `♣A`, which couldn't be read).
- `--keep-free` and the disk warnings work off Windows too, through `df` (not tested there yet).
