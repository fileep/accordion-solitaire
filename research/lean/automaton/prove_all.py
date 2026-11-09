# Proves every loss so far in Lean's kernel, in batches that share one Lean start-up, and writes a row per batch as it goes.
# Seeds already in the output are skipped, so it can be stopped and run again. The seeds come from the names of the loss records.
#
#   python research/lean/automaton/prove_all.py hard-deals/proofs research/lean/automaton/lean_results.csv <build dir>     (from the repository root)
#
# The build dir holds Accordion.olean and Certificate.olean, from `lean -R research/lean/automaton -o <dir>/X.olean research/lean/automaton/X.lean`.
# Times are the CPU time of Lean, start-up included: wall time means little at idle priority beside a running batch.
import csv
import ctypes
import os
import re
import subprocess
import sys
from pathlib import Path

from lean_certificates import HEADER, source  # through dawg.py, this process and Lean run at idle priority

FIELDS = ["seeds", "nodes", "edges", "claims", "cpu_seconds", "ok"]
MAX_SEEDS, MAX_NODES = 40, 15000
LEAN = str(Path.home() / ".elan" / "toolchains" / "leanprover--lean4---v4.35.0" / "bin" / "lean.exe")


def cpu_seconds(process):
    created, ended, kernel, user = (ctypes.c_ulonglong() for _ in range(4))
    ctypes.windll.kernel32.GetProcessTimes(ctypes.c_void_p(int(process._handle)), ctypes.byref(created), ctypes.byref(ended),
                                           ctypes.byref(kernel), ctypes.byref(user))
    return (kernel.value + user.value) / 1e7


def prove(batch, texts, build, env, writer):
    file = build / "Current.lean"
    file.write_text(HEADER + "".join(t for t, _ in texts), encoding="utf-8", newline="
")
    process = subprocess.Popen([LEAN, "-j", "1", str(file)], env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, encoding="utf-8")
    stdout, stderr = process.communicate()
    axioms = re.findall(r"depends on axioms: \[(.*?)\]", stdout)
    ok = process.returncode == 0 and "error" not in stdout and len(axioms) == len(batch) and all(set(a.split(", ")) <= {"propext", "Quot.sound"} for a in axioms)
    sizes = [size for _, size in texts]
    writer.writerow({"seeds": " ".join(map(str, batch)), "nodes": sum(s[0] for s in sizes), "edges": sum(s[1] for s in sizes),
                     "claims": sum(s[2] for s in sizes), "cpu_seconds": round(cpu_seconds(process), 1), "ok": ok})
    if not ok:
        print(batch, "failed:", stdout[:400], stderr[:400], flush=True)


def main():
    directory, out, build = Path(sys.argv[1]), Path(sys.argv[2]), Path(sys.argv[3])
    done = {int(s) for r in csv.DictReader(out.open(encoding="utf-8")) for s in r["seeds"].split()} if out.exists() else set()
    seeds = sorted(int(p.name.split("-")[1]) for p in directory.glob("seed-*-loss.json"))
    env = dict(os.environ, LEAN_PATH=str(build))
    with out.open("a", newline="", encoding="utf-8") as f:
        writer = csv.DictWriter(f, fieldnames=FIELDS, lineterminator="
")
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
