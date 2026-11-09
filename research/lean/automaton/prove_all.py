# Proves every loss so far in Lean's kernel, one seed at a time, and writes a row per seed as it goes. Seeds already in
# the output are skipped, so it can be stopped and run again. The seeds come from the names of the loss records.
#
#   python research/lean/automaton/prove_all.py hard-deals/proofs research/lean/automaton/lean_results.csv <build dir>     (from the repository root)
#
# The build dir holds Accordion.olean and Certificate.olean, from `lean -R research/lean/automaton -o <dir>/X.olean research/lean/automaton/X.lean`.
import csv
import os
import re
import subprocess
import sys
import time
from pathlib import Path

from lean_certificates import source  # through dawg.py, this process and Lean run at idle priority

FIELDS = ["seed", "nodes", "edges", "claims", "seconds", "axioms", "ok"]
LEAN = str(Path.home() / ".elan" / "bin" / "lean.exe")


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
            start = time.time()
            result = subprocess.run([LEAN, "-j", "1", str(file)], env=env, capture_output=True, text=True, encoding="utf-8")
            found = re.search(r"depends on axioms: \[(.*?)\]", result.stdout)
            axioms = found.group(1) if found else ""
            ok = result.returncode == 0 and "error" not in result.stdout and set(axioms.split(", ")) <= {"propext", "Quot.sound"} and axioms != ""
            writer.writerow({"seed": seed, "nodes": nodes, "edges": edges, "claims": claims, "seconds": round(time.time() - start, 1), "axioms": axioms, "ok": ok})
            f.flush()
            if not ok:
                print(seed, "failed:", result.stdout[:400], result.stderr[:400], flush=True)


if __name__ == "__main__":
    main()
