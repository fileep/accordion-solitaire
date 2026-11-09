# Test data

`v3-checkpoint.json` and `v3-checkpoint.costs` are a checkpoint in format 3 from before the file of costs had a limit, so
without `all_cpu_deals`, `total_micros`, `most_nodes` or `most_micros`. They were saved by `acc` built from commit dbf57d4,
with:

    acc batch --no-log --gpu off --endgame off --threads 1 --from 0 --count 3000 --dossiers d --checkpoint cp.json --stop-after 0.00003

Only the value of `dir` was changed by hand, as it held a path on the machine that made it; `tests/batch_limits.rs` puts
in its own folder. Don't regenerate them with a newer `acc`: being old is what they are for.
