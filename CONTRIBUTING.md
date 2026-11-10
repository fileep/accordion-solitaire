# Contributing

Thank you for wanting to help. Reports of what went wrong are as welcome as changes.

The licence hasn't been chosen yet (see [README.md](README.md)). Until it has, please open an issue before putting
work into a large change.

## Reporting a problem

Say what you ran, from which folder, and what it printed, word for word. The output of `acc doctor` tells most of the
rest. A deal that went wrong is best given by its seed (`acc show --seed N`). Two words in a report are always a bug,
whatever else happened: **UNVERIFIED** (a win that failed the independent check) and **PROOF FAILED** (a loss that one
of the two exhaustive searches didn't confirm). Please report those with the run's report and its `.events.log`.

## Building and testing

You need Rust, through [rustup](https://rustup.rs). On Windows, use Rust's MSVC target with the Visual Studio Build
Tools (the "Desktop development with C++" workload). From the project's folder:

```
cargo build --release -p accordion-cli
cargo test --workspace --release
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

The tests run optimised (`--release`): unoptimised, the solver is many times slower, and some tests run real batches
of deals, which takes minutes. Without an NVIDIA card and NVRTC the GPU kernel's tests pass without running anything;
with them, they compare the kernel with the CPU engines.

The visualizer needs Node.js 22.18 or later and the WebAssembly target:

```
rustup target add wasm32-unknown-unknown
cd viz
npm ci
npm test
```

CI ([.github/workflows/ci.yml](.github/workflows/ci.yml)) runs all of this on Windows, Linux and macOS.

Builds use every instruction the building computer's CPU has ([.cargo/config.toml](.cargo/config.toml) says how to
make a binary for others).

## How the code is written

- `cargo fmt` (lines up to 110 characters, [rustfmt.toml](rustfmt.toml)) and no warnings from clippy.
- Match the code around a change: its naming, how much it comments, and its idioms. Comments say why, not what.
- What the program prints is in plain, short sentences that say what to do next, for someone who has barely used a
  terminal.
- No new dependencies without a good reason: the program builds from the crates in `Cargo.lock`.

## What needs the most care

- **Results.** Nothing counts as a win until the reference rules in `crates/core` have replayed it, and a loss counts
  only when two exhaustive searches agree (`acc prove`). A change to the rules, the engines, verification or proving
  needs tests that show it changes no verdict: the golden deals in `testdata/`, `acc selftest`, and the solver's gates.
- **Checkpoints.** A long run must be able to carry on with a newer build. Keep the checkpoint's `VERSION`, never
  rename or remove a field of the saved arguments, and give a new field `#[serde(default)]`. The test against
  `crates/cli/tests/data/v3-checkpoint.json` checks an old checkpoint still loads.
- **Speed claims.** A number says what it was measured on (machine, commit, date) and how, and changes are timed
  against the code before them in alternated runs. [docs/BENCHMARKS.md](docs/BENCHMARKS.md) records each step, with
  what was tried and not kept, and what was considered and not tried, and why.
- **The published record.** `hard-deals/` holds the dossiers and proofs of the project's own runs. Runs write to
  `output/` unless told otherwise; add to the record only on purpose, with `--dossiers hard-deals`.

## Documentation

The guides in `docs/` are written for people new to the command line: a copyable command for each step, on each
system, with what you should see. When a command or flag changes, change [docs/COMMANDS.md](docs/COMMANDS.md) with
it, and [CHANGELOG.md](CHANGELOG.md).
