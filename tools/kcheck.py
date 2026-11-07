#!/usr/bin/env python3
"""kcheck.py: what a kernel compiles to, with no GPU run and no machine lock (it runs NVRTC on the E-cores at idle priority, through machine.py s).
  python kcheck.py base=kernel.cu x=variant.cu:DEF=1 [--config '{...}'] [--json FILE] [--listing DIR]
For each variant: registers, spill bytes and stack (ptxas -v), code bytes of the entry kernel, and "loop code", the bytes in loops of 1-12 KB (a stand-in for hot code size; see loop_code).
Numbers come from NVRTC's own ptxas for sm_75; the driver finalizes the real run from PTX, and its register count can differ by a little
(accgpu prints the driver's). A smaller number here predicts nothing about speed by itself: time it with ab.py.
"""
import argparse, ctypes, json, os, re, struct, subprocess, sys, tempfile, time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from ab import S, TOOLS, fail, parse_variant

BIN_GLOB = TOOLS + "/.tools"
DEFAULTS = {"block": 64, "prefix_warps": 1, "share_group": 4, "share_moves": 20, "rank_gate": [9, 10], "leftmost": True, "conn_gate": 19, "profile": False, "split": True, "stepwise": True}


def host_flags(config):
    """The same -D flags as accordion_gpu::compile."""
    c = dict(DEFAULTS)
    c.update(json.loads(config) if config else {})
    flags = [f"-DBLOCK={c['block']}", f"-DPREFIX_THREADS={32 * c['prefix_warps']}", f"-DSHARE_G={c['share_group']}",
             f"-DSHARE_K={c['share_moves'] if c['share_group'] > 1 else 0}", f"-DRANK_GATE={c['rank_gate'][0]}", f"-DRANK_GATE2={c['rank_gate'][1]}",
             f"-DLEFTMOST={int(c['leftmost'])}", f"-DCONN_GATE={c['conn_gate']}"]
    return flags + (["-DPROFILE"] if c["profile"] else [])


def nvrtc():
    os.environ["CUDA_CACHE_DISABLE"] = "1"  # a cached compile has no ptxas output, so no spill figures; each variant takes about 15 s
    base = TOOLS + "/.tools"
    bin_dir = None
    for d in sorted(os.listdir(base)):
        p = f"{base}/{d}/nvidia/cuda_nvrtc/bin"
        if d.startswith("cuda-nvrtc") and os.path.isdir(p):
            bin_dir = p
    if not bin_dir:
        fail(f"no NVRTC under {base}")
    os.add_dll_directory(os.path.abspath(bin_dir))
    os.environ["PATH"] = os.path.abspath(bin_dir) + os.pathsep + os.environ["PATH"]
    dll = [f for f in os.listdir(bin_dir) if re.fullmatch(r"nvrtc64_\d+_0\.dll", f)][0]
    return ctypes.CDLL(os.path.abspath(f"{bin_dir}/{dll}"))


def compile_cubin(nv, source, flags):
    prog = ctypes.c_void_p()
    if nv.nvrtcCreateProgram(ctypes.byref(prog), source.encode(), None, 0, None, None):
        fail("nvrtcCreateProgram failed")
    opts = ["--gpu-architecture=sm_75", "--ptxas-options=-v"] + flags
    arr = (ctypes.c_char_p * len(opts))(*[o.encode() for o in opts])
    t = time.time()
    rc = nv.nvrtcCompileProgram(prog, len(opts), arr)
    seconds = time.time() - t
    n = ctypes.c_size_t()
    nv.nvrtcGetProgramLogSize(prog, ctypes.byref(n))
    buf = ctypes.create_string_buffer(n.value)
    nv.nvrtcGetProgramLog(prog, buf)
    log = buf.value.decode(errors="replace")
    if rc:
        return None, log, seconds
    nv.nvrtcGetCUBINSize(prog, ctypes.byref(n))
    out = ctypes.create_string_buffer(n.value)
    nv.nvrtcGetCUBIN(prog, out)
    return out.raw, log, seconds


def sections(elf):
    shoff, = struct.unpack_from("<Q", elf, 0x28)
    shentsize, shnum, shstrndx = struct.unpack_from("<HHH", elf, 0x3A)
    hdr = lambda i: struct.unpack_from("<IIQQQQIIQQ", elf, shoff + i * shentsize)
    stroff = hdr(shstrndx)[4]
    out = {}
    for i in range(shnum):
        h = hdr(i)
        name = elf[stroff + h[0]:elf.index(b"\0", stroff + h[0])].decode()
        out[name] = {"offset": h[4], "size": h[5], "info": h[7]}
    return out


def loops(code):
    """Backward branches of sm_75 code: (first instruction, last instruction)."""
    spans = []
    for i in range(0, len(code) // 16):
        w0, w1 = struct.unpack_from("<II", code, i * 16)
        if w0 & 0xFFF == 0x947:  # bra (see S/repr/py/dis.py)
            rel = w1 if w1 < 0x80000000 else w1 - (1 << 32)
            if rel < 0:
                spans.append((i + 1 + rel // 16, i))
    return sorted(set(spans))


def loop_code(spans, least=1024, most=12288):
    """The compiler lays cold paths at the end, and their branches back make whole-kernel spans, so those say nothing. Loops of 1-12 KB are the
    prefix and search bodies and their parts: their sizes, largest first, and the bytes they cover together, a stand-in for hot code size."""
    sizes, covered = [], set()
    for a, b in spans:
        n = (b - a + 1) * 16
        if least <= n <= most:
            sizes.append(n)
            covered.update(range(a, b + 1))
    return len(covered) * 16, sorted(sizes, reverse=True)


def parse_log(log, entry):
    """ptxas -v says per function: stack frame, spill stores and loads; and registers used."""
    out = {}
    for m in re.finditer(r"Function properties for (\S+)\s*\n[^\n]*?(\d+) bytes stack frame, (\d+) bytes spill stores, (\d+) bytes spill loads", log):
        out[m.group(1)] = {"stack": int(m.group(2)), "spill_stores": int(m.group(3)), "spill_loads": int(m.group(4))}
    return out.get(entry) or next(iter(out.values()), None)


def check(nv, v, config, entry, listing):
    src_path = S + "/ov/base/kernel.cu" if v["path"] == "builtin" else v["path"]
    if v["path"].lower().endswith(".exe"):
        fail(f"{v['name']}: kcheck needs a .cu, not an exe")
    source = open(src_path, encoding="utf-8").read()
    if source.startswith("﻿"):
        fail(f"{v['name']}: the source starts with a BOM; NVRTC refuses it")
    flags = host_flags(v["config"] or config) + [f"-D{d}" for d in v["defines"]]
    cubin, log, seconds = compile_cubin(nv, source, flags)
    if cubin is None:
        return {"kernel": v["name"], "error": log[-1500:]}
    sec = sections(cubin)
    name = ".text." + entry
    if name not in sec:
        return {"kernel": v["name"], "error": f"no {name} in the CUBIN: {[k for k in sec if k.startswith('.text')]}"}
    s = sec[name]
    code = cubin[s["offset"]:s["offset"] + s["size"]]
    hot, sizes = loop_code(loops(code))
    if listing:
        os.makedirs(listing, exist_ok=True)
        open(f"{listing}/{v['name']}.cubin", "wb").write(cubin)
        open(f"{listing}/{v['name']}.log", "w").write(log)
    r = {"kernel": v["name"], "registers": s["info"] >> 24, "text_bytes": s["size"], "other_text": {k[6:]: x["size"] for k, x in sec.items() if k.startswith(".text.") and k != name},
         "loop_code_bytes": hot, "loops_bytes": sizes, "compile_s": seconds, "spill": parse_log(log, entry)}
    return r


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("variants", nargs="+", help="NAME=PATH[:DEF=VALUE...][@JSON]; the first is the reference")
    ap.add_argument("--config", help="GPU settings JSON, for the defines the host passes")
    ap.add_argument("--entry", default="attempts_split")
    ap.add_argument("--json", help="write the results here too")
    ap.add_argument("--listing", help="keep each CUBIN and ptxas log in this folder (S/repr/py/dis.py reads the CUBIN)")
    a = ap.parse_args()
    for t in a.variants:
        parse_variant(t)  # fail now, not after waiting for hot timing
    if not os.environ.get("KCHECK_BG"):
        env = dict(os.environ, KCHECK_BG="1")
        sys.exit(subprocess.call([sys.executable, S + "/machine.py", "s", "--", sys.executable, os.path.abspath(__file__), *sys.argv[1:]], env=env))
    nv = nvrtc()
    os.chdir(os.path.dirname(os.path.abspath(__file__)))
    results = [check(nv, parse_variant(t), a.config, a.entry, a.listing) for t in a.variants]
    print(f"{'kernel':<14} {'regs':>4} {'spill st/ld':>13} {'stack':>6} {'code':>7} {'vs first':>8} {'loop code':>9} {'vs first':>8}  compile  loops of 1-12 KB, bytes")
    base = next((r for r in results if "error" not in r), None)
    for r in results:
        if "error" in r:
            print(f"{r['kernel']:<14} ERROR {r['error']}")
            continue
        sp = r["spill"]
        spill = f"{sp['spill_stores']}/{sp['spill_loads']}" if sp else "?"
        stack = sp["stack"] if sp else "?"
        print(f"{r['kernel']:<14} {r['registers']:>4} {spill:>13} {stack:>6} {r['text_bytes']:>7} {r['text_bytes'] - base['text_bytes']:>+8}  {r['loop_code_bytes']:>9} {r['loop_code_bytes'] - base['loop_code_bytes']:>+8}  {r['compile_s']:5.1f} s  {' '.join(map(str, r['loops_bytes']))}")
    if a.json:
        json.dump(results, open(a.json, "w"), indent=1)
    return 1 if any("error" in r for r in results) else 0


if __name__ == "__main__":
    sys.exit(main())
