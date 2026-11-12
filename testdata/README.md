# Test data

Golden deals, solutions and deck-input cases shared by the Rust, TypeScript (via WASM) and CUDA tests
(`crates/core/tests/golden.rs`, `crates/gpu/tests/kernel.rs`). They come from an implementation that shares no code with
the Rust one, so the tests check both the code and the written rules in [docs/FORMATS.md](../docs/FORMATS.md).

- `tools/make_golden.py` builds the files here: the deal generator and the deck-input rules re-implemented in Python from
  docs/FORMATS.md, and wins found by a small Python search.
- `tools/check_loss.py` re-checks proven losses with those Python rules: an exhaustive search of every position reachable
  from the deal, which must find no win and count exactly the dead positions `acc prove` counted. It also shows why the
  deal is lost.
- `tools/classify_losses.py` does that for every loss record in a folder and writes one row per deal, as in
  [research/runs/losses.csv](../research/runs/losses.csv).

Run them from the repository root, as each script's first lines show.
