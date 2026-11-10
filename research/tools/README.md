# The author's GPU tuning harness

These scripts are kept as the record of how the GPU kernel's speed-ups in [docs/BENCHMARKS.md](../../docs/BENCHMARKS.md)
were measured. They are the author's private harness and **do not run from this repository alone**: they need parts
that aren't published (`machine.py`, which holds a lock on the machine and runs jobs at chosen priorities, and the
`ov/` folders of kernels, tools and queues), named by the `ACC_SCRATCH` environment variable. Without it they stop
at once and say so.

| Script | What it did |
|---|---|
| `ab.py` | Timed kernels against each other in one hold of the machine, alternated, with the GPU's clock and the CPU's load logged and suspect runs flagged. GPU alone through `accgpu`, or a hot `acc batch` with `--hot`. |
| `kcheck.py` | What a kernel compiles to, without the GPU: registers, spills, stack and code size from NVRTC's own `ptxas`, for `sm_75` (the T550). Imports `ab.py` from beside it. |
| `sim.py` | Simulated what the kernel's attempts would cost under a policy, through `mine.exe`, built from a private simulator and expected beside the script. |

Nothing in `acc` or the tests uses them. To time a change to the kernel yourself, `acc batch --gpu-kernel FILE`
and `accgpu --kernel FILE` run another version of `kernel.cu` without a rebuild (see `accgpu --help`).
