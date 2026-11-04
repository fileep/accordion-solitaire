# Accordion Solitaire Solver

A fast solver for open Accordion solitaire (all 52 cards dealt face up; a pile moves onto the pile 1 or 3 to
its left when the top cards share a suit or rank; win = one pile), plus a browser visualizer for playing and
replaying selected deals.

- [PLAN.md](PLAN.md): what we build, in what order, and how each phase is accepted
- [docs/RESEARCH.md](docs/RESEARCH.md): rules, prior results, measurements and design rationale

## Layout

| Path | Contents |
|---|---|
| `crates/core` | `accordion-core`: cards, rules, engines, file formats |
| `crates/solver` | `accordion-solver`: search, schedules, endgame table, batch runner |
| `crates/cli` | `acc` command-line tool |
| `crates/wasm` | `accordion-wasm`: the core compiled to WebAssembly for the visualizer |
| `viz/` | Vite + TypeScript visualizer |
| `research/probes/` | Python probes behind the numbers in RESEARCH.md |
| `research/experiments/` | experiment configs and results |
| `hard-deals/` | dossiers for every deal not yet won |
| `testdata/` | golden deals and solutions shared by Rust and TypeScript tests |

## Build and run

Requires Rust (stable, via rustup) and Node.js LTS.

```bash
cargo test --workspace
```

```bash
cargo run --release -p accordion-cli
```

```bash
cd viz && npm install && npm run dev
```

`npm run dev` rebuilds the WebAssembly engine (`npm run wasm`) before starting Vite.
