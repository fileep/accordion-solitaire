# Proves every loss so far in Lean's kernel, in batches that share one Lean start-up, and writes a row per batch as it goes.
# Seeds already in the output are skipped, so it can be stopped and run again. The seeds come from the names of the loss records.
#
#   python research/lean/automaton/prove_all.py hard-deals/proofs research/lean/automaton/lean_results.csv <build dir>     (from the repository root)
#
# The build dir holds Accordion.olean and Certificate.olean, from `lean -R research/lean/automaton -o <dir>/X.olean research/lean/automaton/X.lean`.
# Times are the user CPU time of Lean, start-up included: wall time means little at idle priority beside a running batch. A Lean
# that passes MEMORY_CAP_GB is killed and its batch marked failed, so that it can't push a long run into paging.
import csv
import ctypes
import os
import re
import subprocess
import sys
import tempfile
import time
from ctypes import wintypes
from pathlib import Path

from lean_certificates import HEADER, source  # through dawg.py, this process and Lean run at idle priority

FIELDS = ["seeds", "nodes", "edges", "claims", "cpu_seconds", "peak_gb", "ok"]
MAX_SEEDS, MAX_NODES = 40, 15000
MEMORY_CAP_GB = 8.0
LEAN = str(Path.home() / ".elan" / "toolchains" / "leanprover--lean4---v4.35.0" / "bin" / "lean.exe")


class Memory(ctypes.Structure):
    _fields_ = [("cb", wintypes.DWORD), ("PageFaultCount", wintypes.DWORD), ("PeakWorkingSetSize", ctypes.c_size_t), ("WorkingSetSize", ctypes.c_size_t),
                ("QuotaPeakPagedPoolUsage", ctypes.c_size_t), ("QuotaPagedPoolUsage", ctypes.c_size_t), ("QuotaPeakNonPagedPoolUsage", ctypes.c_size_t),
                ("QuotaNonPagedPoolUsage", ctypes.c_size_t), ("PagefileUsage", ctypes.c_size_t), ("PeakPagefileUsage", ctypes.c_size_t)]


ctypes.windll.psapi.GetProcessMemoryInfo.argtypes = (ctypes.c_void_p, ctypes.POINTER(Memory), wintypes.DWORD)


def memory_gb(process):
    """(committed now, peak committed) in GB."""
    m = Memory()
    m.cb = ctypes.sizeof(Memory)
    ctypes.windll.psapi.GetProcessMemoryInfo(ctypes.c_void_p(int(process._handle)), ctypes.byref(m), m.cb)
    return m.PagefileUsage / 2**30, m.PeakPagefileUsage / 2**30


def cpu_seconds(process):
    created, ended, kernel, user = (ctypes.c_ulonglong() for _ in range(4))
    ctypes.windll.kernel32.GetProcessTimes(ctypes.c_void_p(int(process._handle)), ctypes.byref(created), ctypes.byref(ended),
                                           ctypes.byref(kernel), ctypes.byref(user))
    return user.value / 1e7


def prove(batch, texts, build, env, writer):
    file = build / "Current.lean"
    file.write_text(HEADER + "".join(t for t, _ in texts), encoding="utf-8", newline=chr(10))
    with tempfile.TemporaryFile("w+", encoding="utf-8") as out:
        process = subprocess.Popen([LEAN, "-j", "1", str(file)], env=env, stdout=out, stderr=subprocess.STDOUT)
        killed = False
        while process.poll() is None:
            if memory_gb(process)[0] > MEMORY_CAP_GB:
                process.kill()
                killed = True
                break
            time.sleep(0.25)
        process.wait()
        out.seek(0)
        stdout = out.read()
    axioms = re.findall(r"depends on axioms: \[(.*?)\]", stdout)
    ok = not killed and process.returncode == 0 and "error" not in stdout and len(axioms) == len(batch) and all(set(a.split(", ")) <= {"propext", "Quot.sound"} for a in axioms)
    sizes = [size for _, size in texts]
    writer.writerow({"seeds": " ".join(map(str, batch)), "nodes": sum(s[0] for s in sizes), "edges": sum(s[1] for s in sizes),
                     "claims": sum(s[2] for s in sizes), "cpu_seconds": round(cpu_seconds(process), 1), "peak_gb": round(memory_gb(process)[1], 2), "ok": ok})
    if not ok:
        print(batch, "failed" + (" at the memory cap" if killed else "") + ":", stdout[:400], flush=True)


def main():
    directory, out, build = Path(sys.argv[1]), Path(sys.argv[2]), Path(sys.argv[3])
    done = {int(s) for r in csv.DictReader(out.open(encoding="utf-8")) for s in r["seeds"].split()} if out.exists() else set()
    seeds = sorted(int(p.name.split("-")[1]) for p in directory.glob("seed-*-loss.json"))
    env = dict(os.environ, LEAN_PATH=str(build))
    with out.open("a", newline="", encoding="utf-8") as f:
        writer = csv.DictWriter(f, fieldnames=FIELDS, lineterminator=chr(10))
        if not done:
            writer.writeheader()
        batch, texts = [], []
        for seed in seeds:
            if seed in done:
                continue
            batch.append(seed)
            texts.append(source(seed))
            if len(batch) >= MAX_SEEDS or sum(size[0] for _, size in texts) >= MAX_NODES:
                prove(batch, texts, build, env, writer)
                f.flush()
                batch, texts = [], []
        if batch:
            prove(batch, texts, build, env, writer)


if __name__ == "__main__":
    main()
