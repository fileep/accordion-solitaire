# Proves every loss so far in Lean's kernel, in batches that share one Lean start-up, and writes a row per batch as it goes.
# Seeds already in the output are skipped, so it can be stopped and run again. The seeds come from the names of the loss records.
#
#   python research/lean/automaton/prove_all.py hard-deals/proofs research/lean/automaton/lean_results.csv <build dir> [jobs]     (from the repository root)
#   python research/lean/automaton/prove_all.py hard-deals-chacha/proofs research/lean/automaton/lean_results_chacha.csv <build dir> [jobs] --chacha
#
# --chacha proves the second generator's losses (lean_certificates.py), and the build dir also needs ChaCha.olean.
# The build dir holds Accordion.olean and Certificate.olean, from `lean -R research/lean/automaton -o <dir>/X.olean research/lean/automaton/X.lean`.
# Times are the user CPU time of Lean, start-up included: wall time means little at idle priority beside a running batch. A Lean
# that passes MEMORY_CAP_GB is killed and its batch marked failed, so that it can't push a long run into paging. Jobs is 1 by default:
# give more only with nothing else running, as each Lean takes up to about 3 GB. A failed batch is tried again one seed at a time, and
# a seed that has never passed is tried again on the next run. Each batch leaves its .olean in the build dir, for replay.py.
import csv
import ctypes
import functools
import multiprocessing
import os
import re
import subprocess
import sys
import tempfile
import threading
import time
from concurrent.futures import ThreadPoolExecutor
from ctypes import wintypes
from pathlib import Path

from lean_certificates import CHACHA_HEADER, HEADER, source  # through dawg.py, this process and Lean run at idle priority

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


def prove(batch, texts, build, env, record, header=HEADER):
    file = build / f"B{batch[0]}.lean"
    file.write_text(header + "".join(t for t, _ in texts), encoding="utf-8", newline=chr(10))
    with tempfile.TemporaryFile("w+", encoding="utf-8") as out:
        process = subprocess.Popen([LEAN, "-j", "1", "-R", str(build), "-o", str(file.with_suffix(".olean")), str(file)], env=env, stdout=out, stderr=subprocess.STDOUT)
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
    record({"seeds": " ".join(map(str, batch)), "nodes": sum(s[0] for s in sizes), "edges": sum(s[1] for s in sizes),
            "claims": sum(s[2] for s in sizes), "cpu_seconds": round(cpu_seconds(process), 1), "peak_gb": round(memory_gb(process)[1], 2), "ok": ok})
    if not ok:
        print(batch, "failed" + (" at the memory cap" if killed else "") + ":", stdout[:400], flush=True)
        if len(batch) > 1:
            for seed, text in zip(batch, texts):
                prove([seed], [text], build, env, record, header)


def main():
    chacha = "--chacha" in sys.argv
    args = [a for a in sys.argv if a != "--chacha"]
    directory, out, build = Path(args[1]), Path(args[2]), Path(args[3])
    jobs = int(args[4]) if len(args) > 4 else 1
    make, header = (functools.partial(source, records=str(directory)), CHACHA_HEADER) if chacha else (source, HEADER)
    rows = list(csv.DictReader(out.open(encoding="utf-8"))) if out.exists() else []
    done = {int(s) for r in rows if r["ok"] == "True" for s in r["seeds"].split()}
    todo = sorted(s for s in (int(p.name.split("-")[1]) for p in directory.glob("seed-*-loss.json")) if s not in done)
    env = dict(os.environ, LEAN_PATH=str(build))
    lock = threading.Lock()
    with out.open("a", newline="", encoding="utf-8") as f:
        writer = csv.DictWriter(f, fieldnames=FIELDS, lineterminator=chr(10))
        if not rows:
            writer.writeheader()

        def record(row):
            with lock:
                writer.writerow(row)
                f.flush()

        with multiprocessing.Pool(jobs) as sources, ThreadPoolExecutor(jobs) as lean:
            batch, texts = [], []
            for seed, made in zip(todo, sources.imap(make, todo)):
                batch.append(seed)
                texts.append(made)
                if len(batch) >= MAX_SEEDS or sum(size[0] for _, size in texts) >= MAX_NODES:
                    lean.submit(prove, batch, texts, build, env, record, header)
                    batch, texts = [], []
            if batch:
                lean.submit(prove, batch, texts, build, env, record, header)


if __name__ == "__main__":
    main()
