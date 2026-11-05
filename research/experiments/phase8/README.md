# Phase 8 experiments

The measurement behind PLAN.md Phase 8 and the Phase 8 section of [docs/BENCHMARKS.md](../../../docs/BENCHMARKS.md): the
CPU solver alone against the CPU solver with GPU ladder attempts in front of it (`acc batch --gpu`).

- `summary.jsonl`: one line per run, as in [phase5/](../phase5/README.md). A run with the GPU also records its settings
  (`gpu`) and what it did (`gpu_totals`: deals given, won and verified, lines that failed verification, attempts,
  positions, launches). Its position and time percentiles cover only the deals solved on the CPU.

## The comparison

Seeds 2,000,000–2,999,999 (the Phase 5 GPU-gate set), 1,000,000 deals per run. The two configurations alternate after a
110 s warm-up, with no rest between runs:

```bash
cargo run --release -p accordion-cli -- experiment --set 2000000:1000000 --warmup 60 --repeat 3 --out research/experiments/phase8 --config '{"name": "cpu", "solver": {"search": {"endgame_piles": 11}}}' --config '{"name": "cpu + gpu", "solver": {"search": {"endgame_piles": 11}}, "gpu": {}}'
```

`cpu` uses all 16 threads. `cpu + gpu` uses 15 CPU workers plus the GPU's feeder thread, with the default GPU settings
(`accordion_gpu::Settings`: rungs of 38 and 36 random moves with 256 and 1,024 tries, 2,000 positions per search).
