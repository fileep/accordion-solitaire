# Using an NVIDIA graphics card

acc can use an NVIDIA graphics card (a GPU) beside the processor. The GPU makes the first, cheap attempts at every deal,
and wins nearly all of them; the processor checks each of its winning lines and solves the few deals it misses. On the
test laptop (Intel i7-1260P with an NVIDIA T550) that is about 5 million deals a second, against roughly 15,000 on the
processor alone (an older measurement: see [LONG_RUNS.md](LONG_RUNS.md#how-long-and-how-much)). Without a GPU everything
works, more slowly.

What you need:

- An NVIDIA card. Only the T550 (a laptop card, compute capability 7.5) has been tried. acc compiles its GPU code
  for the card it finds each time a run starts, so other NVIDIA cards that NVRTC 12.9 knows should work too; none has been
  tried yet. AMD and Intel graphics can't be used, and macOS has no NVIDIA support.
- NVIDIA's driver, supporting CUDA 12.9 or later.
- NVRTC, NVIDIA's free CUDA compiler, which acc uses to compile its GPU code. It is NVIDIA's software, under NVIDIA's licence
  (the CUDA Toolkit end user licence agreement), and isn't part of this project: you fetch it yourself, in one command.

`acc doctor` checks the first three: its `GPU` line says what is there and what is missing, with the steps.

## 1. Check the driver

```
nvidia-smi
```

This comes with NVIDIA's driver. At the top right of its table it says `CUDA Version: 13.2` (the test laptop's): that
must be 12.9 or more. If the command isn't found, or the version is lower, install the current driver for your card from
[nvidia.com/drivers](https://www.nvidia.com/drivers), restart if it asks you to, and check again. `acc doctor` says the
same in its `GPU` line: `the NVIDIA driver supports CUDA 12.4; acc needs 12.9 or later` means update the driver.

## 2. Add NVRTC

**Windows**, in PowerShell, in the project's folder:

```powershell
powershell -ExecutionPolicy Bypass -File tools\get-nvrtc.ps1
```

It asks PyPI (the Python package index, where NVIDIA publishes NVRTC as `nvidia-cuda-nvrtc-cu12`) for version 12.9.86,
downloads it (76 MB, kept in `.tools\downloads`), checks that it is exactly the right file by its SHA-256, and unpacks it
into `.tools\cuda-nvrtc-12.9.86`. Python isn't needed, and nothing else on the computer is changed. It ends with:

```
NVRTC 12.9.86 is in C:\Users\you\Documents\accordion-solitaire\.tools\cuda-nvrtc-12.9.86
acc finds it there when it runs in C:\Users\you\Documents\accordion-solitaire, or when it is a copy under it, as target\release\acc.exe is.
Elsewhere, acc finds it when the environment variable ACC_NVRTC_DIR names that folder. To set it for windows
opened from now on: setx ACC_NVRTC_DIR "C:\Users\you\Documents\accordion-solitaire\.tools\cuda-nvrtc-12.9.86"
Run acc doctor to check.
```

`-ExecutionPolicy Bypass` lets PowerShell run this one script without changing any setting. The unpacking and checking
were tested on the test laptop with a copy of the file; the download itself wasn't run while this was written.

By hand, if you prefer:

1. Download `nvidia_cuda_nvrtc_cu12-12.9.86-py3-none-win_amd64.whl` from
   [pypi.org/project/nvidia-cuda-nvrtc-cu12/12.9.86/#files](https://pypi.org/project/nvidia-cuda-nvrtc-cu12/12.9.86/#files).
2. It is a zip file: rename it to end in `.zip`, and unpack it into a folder named `.tools\cuda-nvrtc-12.9.86` in the
   project's folder, so that `.tools\cuda-nvrtc-12.9.86\nvidia\cuda_nvrtc\bin` holds `nvrtc64_120_0.dll`.

Or give the script the file you downloaded, which it checks the same way:
`powershell -ExecutionPolicy Bypass -File tools\get-nvrtc.ps1 -Wheel $HOME\Downloads\nvidia_cuda_nvrtc_cu12-12.9.86-py3-none-win_amd64.whl`

**Linux** (not tested yet):

```bash
sh tools/get-nvrtc.sh
```

It does the same with the Linux file (it needs `curl` or `wget`, and `python3` to read PyPI's answer and unpack the file),
unpacks it into `.tools/cuda-nvrtc-12.9.86`, and makes the `libnvrtc.so` link acc's CUDA library looks for. acc then
finds it by starting itself again with that folder on `LD_LIBRARY_PATH`. This has only been type-checked, never run on
Linux: please report how it goes.

**With the CUDA Toolkit 12** already installed, NVRTC is part of it. If acc doesn't find it by itself, set `ACC_NVRTC_DIR`
to the folder holding `nvrtc64_120_0.dll` (Windows) or `libnvrtc.so.12` (Linux).

### Where acc looks for NVRTC

In order: where the system looks for libraries (`PATH` on Windows, the loader's path on Linux); the folder
`ACC_NVRTC_DIR` names (the one holding the library, or one the download was unpacked into); and the newest
`.tools\cuda-nvrtc-*` in the folder acc is run from, or the folder acc's own program file is in, or any folder above
either. So if you installed acc with `cargo install`, run it from the project's folder, or set `ACC_NVRTC_DIR` once with
the `setx` line the script prints (then open a new terminal).

## 3. Check that it works

```
acc doctor
```

The `GPU` line should now say `PASS  GPU       NVIDIA driver (CUDA 13.2) and NVRTC found`. This only looks for the
libraries; to try the card itself:

```
acc doctor --gpu
```

It compiles the GPU code and solves 20,000 deals with the card, and its `GPU test` line says how many it won and how long
it took. If a run is using the card already, it says so first: the test would share the card with it for a few seconds.

When a run starts with the GPU it prints a line like `GPU: <the card's name> (<n> multiprocessors), kernel compiled in <s> s`,
the threads line says `15 CPU threads + GPU` on a 16-thread processor (one thread feeds the card), and the report has a `GPU` line
([READING_RESULTS.md](READING_RESULTS.md#the-report-line-by-line)).

## Choosing how a run uses it

- `--gpu auto`, the default, uses the card if it works and the processor alone if not, saying why in one line at the start
  (such as `Your NVIDIA card could be used, but one free component (NVRTC) is missing. ...`).
- `--gpu on` refuses to start without the card. Use it for a long run sized for the GPU's speed, so that a missing driver
  doesn't leave it going a hundred times slower.
- `--gpu off` never uses it. `acc run --gentle` leaves it alone too, unless `--gpu` is given.

A run keeps its choice when it carries on. If the card fails during a run, the run pauses with exit code 75 rather than go
on at the processor's speed; `acc run` and `acc resume` start it again after a minute, and a new start usually gets the
card back ([TROUBLESHOOTING.md](TROUBLESHOOTING.md#gpu-stopped)).

## Good to know

- Every winning line the GPU finds is replayed on the processor by a separate check of the rules before it counts. Which
  deals the GPU wins, and with which lines, depends on timing, so two runs of the same deals can split the work
  differently.
- Windows resets a graphics card whose work seems stuck for 2 seconds. acc's launches take well under a second on the
  T550; a much slower card might need a smaller `batch` in the GPU's settings ([COMMANDS.md](COMMANDS.md#the-gpus-settings)).
  This hasn't been tried.
- On a laptop the processor and the GPU share one cooling system and one charger: see
  [LONG_RUNS.md](LONG_RUNS.md#laptops).
- The GPU's settings (`acc settings --gpu`) are the fastest found on the T550 ([BENCHMARKS.md](BENCHMARKS.md)). Another card
  may prefer others; `acc batch --gpu-config` and `acc experiment` are for finding out.
- What was tested in this version: the GPU's engine is the one that ran the project's big runs. The newer commands around
  it, `acc doctor --gpu` and `acc run` with the GPU, were only compiled, not run with a card, as the card was busy with a
  long run at the time.
