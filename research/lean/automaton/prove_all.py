# Proves every loss so far in Lean's kernel, one seed at a time, and writes a row per seed as it goes. Seeds already in
# the output are skipped, so it can be stopped and run again. The seeds come from the names of the loss records.
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

from lean_certificates import source  # through dawg.py, this process and Lean run at idle priority

FIELDS = ["seed", "nodes", "edges", "claims", "cpu_seconds", "axioms", "ok"]
LEAN = str(Path.home() / ".elan" / "toolchains" / "leanprover--lean4---v4.35.0" / "bin" / "lean.exe")


def cpu_seconds(process):
    created, ended, kernel, user = (ctypes.c_ulonglong() for _ in range(4))
    ctypes.windll.kernel32.GetProcessTimes(ctypes.c_void_p(int(process._handle)), ctypes.byref(created), ctypes.byref(ended),
                                           ctypes.byref(kernel), ctypes.byref(user))
    return (kernel.value + user.value) / 1e7


def main():
    directory, out, build = Path(sys.argv[1]), Path(sys.argv[2]), Path(sys.argv[3])
    done = {int(r["seed"]) for r in csv.DictReader(out.open(encoding="utf-8"))} if out.exists() else set()
    seeds = sorted(int(p.name.split("-")[1]) for p in directory.glob("seed-*-loss.json"))
    env = dict(os.environ, LEAN_PATH=str(build))
    with out.open("a", newline="", encoding="utf-8") as f:
        writer = csv.DictWriter(f, fieldnames=FIELDS, lineterminator="\n")
        if not done:
            writer.writeheader()
        for seed in seeds:
            if seed in done:
                continue
            text, (nodes, edges, claims) = source(seed)
            file = build / "Current.lean"
            file.write_text(text, encoding="utf-8", newline="\n")
            process = subprocess.Popen([LEAN, "-j", "1", str(file)], env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, encoding="utf-8")
            stdout, stderr = process.communicate()
            found = re.search(r"depends on axioms: \[(.*?)\]", stdout)
            axioms = found.group(1) if found else ""
            ok = process.returncode == 0 and "error" not in stdout and set(axioms.split(", ")) <= {"propext", "Quot.sound"} and axioms != ""
            writer.writerow({"seed": seed, "nodes": nodes, "edges": edges, "claims": claims, "cpu_seconds": round(cpu_seconds(process), 1), "axioms": axioms, "ok": ok})
            f.flush()
            if not ok:
                print(seed, "failed:", stdout[:400], stderr[:400], flush=True)


if __name__ == "__main__":
    main()
