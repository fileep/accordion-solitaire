"""CUDA smoke test: compile a kernel with NVRTC, run it on the GPU via the driver API, check the answer.

Usage: python gpu_smoke.py [directory holding nvrtc64_*.dll]
(default: the NVRTC unpacked into .tools/ in Phase 5, see PLAN.md Phase 0)

The kernel evaluates the engine's match test (card byte suit<<4 | rank; match when (a^b)&0x0F == 0 or (a^b)&0x70 == 0)
for all 52x52 card pairs, and the result is compared with "same suit or same rank" computed directly in Python.
"""
import ctypes as C
import os
import sys
import time
from pathlib import Path

DEFAULT = Path(__file__).resolve().parents[2] / ".tools/cuda-nvrtc-12.9.86/nvidia/cuda_nvrtc/bin"
bin_dir = Path(sys.argv[1]) if len(sys.argv) > 1 else DEFAULT
os.add_dll_directory(str(bin_dir))
# NVRTC loads nvrtc-builtins with a plain LoadLibrary, which searches PATH, not the directories added above.
os.environ["PATH"] = f"{bin_dir};{os.environ['PATH']}"
nvrtc = C.WinDLL(str(next(p for p in bin_dir.glob("nvrtc64_*.dll") if ".alt." not in p.name)))
cuda = C.WinDLL("nvcuda.dll")

major, minor = C.c_int(), C.c_int()
assert nvrtc.nvrtcVersion(C.byref(major), C.byref(minor)) == 0
print(f"NVRTC {major.value}.{minor.value}")

SRC = rb"""
extern "C" __global__ void matches(const unsigned char* cards, unsigned char* out, int n) {
    int i = blockIdx.x * blockDim.x + threadIdx.x;
    if (i >= n * n) return;
    unsigned char x = cards[i / n] ^ cards[i % n];
    out[i] = ((x & 0x0F) == 0) | ((x & 0x70) == 0);
}
"""


def check(res, what, lib=cuda):
    if res != 0:
        if lib is cuda:
            msg = C.c_char_p()
            cuda.cuGetErrorString(res, C.byref(msg))
            raise SystemExit(f"{what} failed: {res} {msg.value}")
        raise SystemExit(f"{what} failed: {res}")


# Compile to PTX for compute capability 7.5 (Turing).
prog = C.c_void_p()
check(nvrtc.nvrtcCreateProgram(C.byref(prog), SRC, b"matches.cu", 0, None, None), "nvrtcCreateProgram", nvrtc)
opts = (C.c_char_p * 1)(b"--gpu-architecture=compute_75")
t = time.perf_counter()
res = nvrtc.nvrtcCompileProgram(prog, 1, opts)
size = C.c_size_t()
nvrtc.nvrtcGetProgramLogSize(prog, C.byref(size))
log = C.create_string_buffer(size.value)
nvrtc.nvrtcGetProgramLog(prog, log)
if res != 0:
    raise SystemExit(f"compile failed:\n{log.value.decode()}")
nvrtc.nvrtcGetPTXSize(prog, C.byref(size))
ptx = C.create_string_buffer(size.value)
check(nvrtc.nvrtcGetPTX(prog, ptx), "nvrtcGetPTX", nvrtc)
print(f"compiled to {size.value} bytes of PTX in {1000 * (time.perf_counter() - t):.0f} ms")

# Driver API: primary context, load the PTX, launch.
check(cuda.cuInit(0), "cuInit")
dev = C.c_int()
check(cuda.cuDeviceGet(C.byref(dev), 0), "cuDeviceGet")
name = C.create_string_buffer(256)
cuda.cuDeviceGetName(name, 256, dev)
cc = [C.c_int(), C.c_int()]
cuda.cuDeviceGetAttribute(C.byref(cc[0]), 75, dev)  # COMPUTE_CAPABILITY_MAJOR
cuda.cuDeviceGetAttribute(C.byref(cc[1]), 76, dev)  # COMPUTE_CAPABILITY_MINOR
sms = C.c_int()
cuda.cuDeviceGetAttribute(C.byref(sms), 16, dev)  # MULTIPROCESSOR_COUNT
drv = C.c_int()
cuda.cuDriverGetVersion(C.byref(drv))
print(f"device {name.value.decode()}, compute {cc[0].value}.{cc[1].value}, {sms.value} SMs, driver CUDA {drv.value // 1000}.{drv.value % 1000 // 10}")

ctx = C.c_void_p()
check(cuda.cuDevicePrimaryCtxRetain(C.byref(ctx), dev), "cuDevicePrimaryCtxRetain")
check(cuda.cuCtxSetCurrent(ctx), "cuCtxSetCurrent")
mod, fn = C.c_void_p(), C.c_void_p()
check(cuda.cuModuleLoadData(C.byref(mod), ptx), "cuModuleLoadData")
check(cuda.cuModuleGetFunction(C.byref(fn), mod, b"matches"), "cuModuleGetFunction")

cards = bytes(s << 4 | r for s in range(4) for r in range(13))
n = len(cards)
d_cards, d_out = C.c_uint64(), C.c_uint64()
check(cuda.cuMemAlloc_v2(C.byref(d_cards), C.c_size_t(n)), "cuMemAlloc")
check(cuda.cuMemAlloc_v2(C.byref(d_out), C.c_size_t(n * n)), "cuMemAlloc")
check(cuda.cuMemcpyHtoD_v2(d_cards, cards, C.c_size_t(n)), "cuMemcpyHtoD")
n_arg = C.c_int(n)
args = (C.c_void_p * 3)(C.cast(C.byref(d_cards), C.c_void_p), C.cast(C.byref(d_out), C.c_void_p), C.cast(C.byref(n_arg), C.c_void_p))
block = 256
grid = (n * n + block - 1) // block
check(cuda.cuLaunchKernel(fn, grid, 1, 1, block, 1, 1, 0, None, args, None), "cuLaunchKernel")
check(cuda.cuCtxSynchronize(), "cuCtxSynchronize")
out = C.create_string_buffer(n * n)
check(cuda.cuMemcpyDtoH_v2(out, d_out, C.c_size_t(n * n)), "cuMemcpyDtoH")

expected = bytes(int(a >> 4 == b >> 4 or a & 0xF == b & 0xF) for a in cards for b in cards)
got = out.raw
assert got == expected, "GPU match table differs from the CPU's"
print(f"GPU match test agrees with the CPU on all {n * n} card pairs ({sum(got)} matching, incl. each card with itself)")

cuda.cuMemFree_v2(d_cards)
cuda.cuMemFree_v2(d_out)
cuda.cuModuleUnload(mod)
cuda.cuDevicePrimaryCtxRelease(dev)
