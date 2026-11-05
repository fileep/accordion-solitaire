# Experiments

Configurations, JSONL results and analysis scripts from `acc experiment` runs (PLAN.md Phase 5 onwards). Tuning uses the
`train` deal set; finalists are chosen on `validation`; final numbers come from `test`, run once.

- [phase5/](phase5/README.md): the speed study: restarts, the random-prefix ladder, move order, sleep sets, engine options,
  endgame tables, the symmetry cache, and the GPU gate.
- [phase8/](phase8/README.md): the CPU solver alone against CPU + GPU (`acc batch --gpu`).
